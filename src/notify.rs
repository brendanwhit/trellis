// trellis/src/notify.rs
//
// `trellis notify`: a Claude Code hook (Notification + Stop) that pings when a
// session needs input. Trellis isn't resident, so the ping lives here and the
// popup's row marker tells you which worktree it was.
use std::io::Read;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::{db, tmux};

pub const CONFIG_KEY: &str = "notify";
const SOUND: &str = "/System/Library/Sounds/Funk.aiff";

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    notification_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    last_assistant_message: String,
}

#[derive(Debug, PartialEq)]
struct Alert {
    body: String,
}

/// What to ping about, if anything. `idle_prompt` is skipped: it fires a minute
/// late and a finished reply already reaches `Stop`.
fn decide(payload: &Payload) -> Option<Alert> {
    match payload.hook_event_name.as_str() {
        "Notification" if payload.notification_type != "idle_prompt" => Some(Alert {
            body: if payload.message.is_empty() { "Needs input".to_string() } else { payload.message.clone() },
        }),
        // Plain-text questions raise no Notification, so read the reply itself.
        "Stop" if payload.last_assistant_message.trim_end().ends_with('?') => {
            Some(Alert { body: "Asked a question".to_string() })
        }
        _ => None,
    }
}

/// Frontmost macOS app's pid, via `lsappinfo` (no accessibility permission needed).
fn frontmost_pid() -> Option<u32> {
    let out = |args: &[&str]| Command::new("lsappinfo").args(args).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    let asn = out(&["front"])?;
    let info = out(&["info", "-only", "pid", asn.trim()])?;
    info.split("pid =").nth(1)?.split_whitespace().next()?.parse().ok()
}

fn parent_pid(pid: u32) -> Option<u32> {
    let out = Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// A tmux client's pid followed by its parents, up to the terminal app that hosts it.
fn ancestry(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    while let Some(parent) = parent_pid(*chain.last().unwrap()).filter(|p| *p > 1 && chain.len() < 16) {
        chain.push(parent);
    }
    chain
}

/// Attached isn't watching: a client stays attached after its terminal loses
/// focus. The pane must be on screen in a client whose terminal app is frontmost.
/// With the frontmost app unknown, assume nobody is watching and ping.
fn being_watched(visible: bool, client_ancestries: &[Vec<u32>], frontmost: Option<u32>) -> bool {
    visible && frontmost.is_some_and(|front| client_ancestries.iter().any(|chain| chain.contains(&front)))
}

fn enabled(conn: &rusqlite::Connection) -> bool {
    db::get_config(conn, CONFIG_KEY).map_or(true, |v| !v.eq_ignore_ascii_case("off"))
}

fn show(session: &str, body: &str) {
    Command::new("afplay").arg(SOUND).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
    // Values go through argv so quotes in a message can't break the script.
    Command::new("osascript")
        .args([
            "-e", "on run argv",
            "-e", "display notification (item 1 of argv) with title \"Trellis\" subtitle (item 2 of argv)",
            "-e", "end run",
            "--", body, session,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();
}

/// Hook entry point. Never fails the hook: a broken ping must not block Claude.
pub fn run(conn: &rusqlite::Connection) {
    if !enabled(conn) {
        return;
    }
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();
    let payload: Payload = serde_json::from_str(&raw).unwrap_or_default();

    let Some(alert) = decide(&payload) else { return };
    let Ok(pane) = std::env::var("TMUX_PANE") else { return };
    let Some(view) = tmux::pane_view(&pane) else { return };
    let ancestries: Vec<Vec<u32>> = view.client_pids.iter().map(|pid| ancestry(*pid)).collect();
    if being_watched(view.visible, &ancestries, frontmost_pid()) {
        return;
    }
    show(&view.session, &alert.body);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(event: &str, kind: &str) -> Payload {
        Payload { hook_event_name: event.into(), notification_type: kind.into(), message: "m".into(), ..Default::default() }
    }

    #[test]
    fn dialogs_ping_but_idle_prompt_does_not() {
        for kind in ["permission_prompt", "elicitation_dialog"] {
            assert!(decide(&payload("Notification", kind)).is_some(), "{kind}");
        }
        assert!(decide(&payload("Notification", "idle_prompt")).is_none());
    }

    #[test]
    fn stop_pings_only_when_the_reply_ends_in_a_question() {
        let stop = |reply: &str| Payload { last_assistant_message: reply.into(), ..payload("Stop", "") };
        assert!(decide(&stop("Does that match what you want?\n")).is_some());
        assert!(decide(&stop("All done.")).is_none());
        assert!(decide(&stop("")).is_none());
    }

    #[test]
    fn unknown_events_never_ping() {
        let other = Payload { last_assistant_message: "?".into(), ..payload("PreToolUse", "") };
        assert!(decide(&other).is_none());
        assert!(decide(&Payload { last_assistant_message: "?".into(), ..Default::default() }).is_none());
    }

    #[test]
    fn real_stop_payload_parses_into_a_ping() {
        let raw = r#"{"hook_event_name":"Stop","last_assistant_message":"Which color do you prefer, red or blue?","background_tasks":[]}"#;
        assert!(decide(&serde_json::from_str(raw).unwrap()).is_some());
    }

    #[test]
    fn watched_only_when_visible_in_a_client_whose_terminal_is_frontmost() {
        let clients = [vec![10, 20, 30], vec![11, 21]];
        assert!(being_watched(true, &clients, Some(30)));
        assert!(being_watched(true, &clients, Some(21)));
        // Attached but another app has focus, or the pane is in a background window.
        assert!(!being_watched(true, &clients, Some(99)));
        assert!(!being_watched(false, &clients, Some(30)));
        assert!(!being_watched(true, &[], Some(30)));
        assert!(!being_watched(true, &clients, None));
    }

    // Needs a running tmux client and macOS: cargo test -- --ignored watched_live --nocapture
    #[test]
    #[ignore = "needs tmux + macOS"]
    fn watched_live() {
        let pane = std::env::var("TMUX_PANE").expect("run inside tmux");
        let view = tmux::pane_view(&pane).unwrap();
        let chains: Vec<Vec<u32>> = view.client_pids.iter().map(|p| ancestry(*p)).collect();
        let front = frontmost_pid();
        println!("visible={} front={front:?} chains={chains:?} watched={}", view.visible, being_watched(view.visible, &chains, front));
        assert!(front.is_some());
        assert!(chains.iter().all(|c| c.len() >= 2));
    }

    #[test]
    fn missing_config_means_enabled_and_off_disables() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_schema(&conn);
        assert!(enabled(&conn));
        db::set_config(&conn, CONFIG_KEY, "off");
        assert!(!enabled(&conn));
        db::set_config(&conn, CONFIG_KEY, "on");
        assert!(enabled(&conn));
    }
}
