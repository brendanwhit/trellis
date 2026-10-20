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
    let Some((session, attached)) = tmux::pane_session(&pane) else { return };
    // You're already looking at it.
    if attached {
        return;
    }
    show(&session, &alert.body);
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
