// trellis/src/notify.rs
//
// `trellis notify`: a Claude Code hook (Notification + Stop) that pings when a
// session needs input. Trellis isn't resident, so the ping lives here and the
// popup's row marker tells you which worktree it was.
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::{db, tmux};

pub const CONFIG_KEY: &str = "notify";
const SOUND: &str = "/System/Library/Sounds/Funk.aiff";
// A Stop hook only needs the final reply, so don't read a multi-MB transcript.
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

#[derive(Deserialize, Default)]
struct Payload {
    #[serde(default)]
    hook_event_name: String,
    #[serde(default)]
    notification_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    transcript_path: String,
}

#[derive(Debug, PartialEq)]
struct Alert {
    body: String,
}

/// Last assistant text block in a transcript, which is JSONL with one content
/// block per `assistant` line.
fn last_assistant_text(transcript: &str) -> Option<String> {
    transcript.lines().rev().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("type")?.as_str()? != "assistant" {
            return None;
        }
        v["message"]["content"]
            .as_array()?
            .iter()
            .rev()
            .find_map(|b| (b["type"] == "text").then(|| b["text"].as_str().map(str::to_string)).flatten())
    })
}

/// What to ping about, if anything. `idle_prompt` is skipped: it fires a minute
/// late and a finished reply already reaches `Stop`.
fn decide(payload: &Payload, last_reply: impl FnOnce() -> Option<String>) -> Option<Alert> {
    match payload.hook_event_name.as_str() {
        "Notification" if payload.notification_type != "idle_prompt" => Some(Alert {
            body: if payload.message.is_empty() { "Needs input".to_string() } else { payload.message.clone() },
        }),
        // Plain-text questions raise no Notification, so read the reply itself.
        "Stop" => last_reply()
            .filter(|t| t.trim_end().ends_with('?'))
            .map(|_| Alert { body: "Asked a question".to_string() }),
        _ => None,
    }
}

fn read_tail(path: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    // A mid-file start lands inside a line; drop the fragment.
    Some(if start > 0 { text.split_once('\n')?.1.to_string() } else { text })
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

    let Some(alert) = decide(&payload, || read_tail(&payload.transcript_path).and_then(|t| last_assistant_text(&t))) else {
        return;
    };
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

    fn assistant(text: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{}}}]}}}}"#, serde_json::json!(text))
    }

    #[test]
    fn dialogs_ping_but_idle_prompt_does_not() {
        for kind in ["permission_prompt", "elicitation_dialog"] {
            assert!(decide(&payload("Notification", kind), || None).is_some(), "{kind}");
        }
        assert!(decide(&payload("Notification", "idle_prompt"), || None).is_none());
    }

    #[test]
    fn stop_pings_only_when_the_reply_ends_in_a_question() {
        let stop = payload("Stop", "");
        assert!(decide(&stop, || Some("Does that match what you want?\n".into())).is_some());
        assert!(decide(&stop, || Some("All done.".into())).is_none());
        assert!(decide(&stop, || None).is_none());
    }

    #[test]
    fn unknown_events_never_ping() {
        assert!(decide(&payload("PreToolUse", ""), || Some("?".into())).is_none());
        assert!(decide(&Payload::default(), || Some("?".into())).is_none());
    }

    #[test]
    fn reply_text_is_the_last_assistant_text_block() {
        let transcript = [
            assistant("first?"),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            assistant("final."),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#.to_string(),
            r#"{"type":"attachment"}"#.to_string(),
        ]
        .join("\n");
        assert_eq!(last_assistant_text(&transcript).as_deref(), Some("final."));
    }

    #[test]
    fn stop_wiring_reads_question_from_a_transcript_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, [assistant("Which one?"), r#"{"type":"attachment"}"#.to_string()].join("\n")).unwrap();
        let alert = decide(&payload("Stop", ""), || read_tail(path.to_str().unwrap()).and_then(|t| last_assistant_text(&t)));
        assert!(alert.is_some());
    }

    #[test]
    fn tail_read_drops_the_partial_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let filler = "#".repeat(TRANSCRIPT_TAIL_BYTES as usize);
        std::fs::write(&path, format!("{filler}\n{}\n", assistant("tail?"))).unwrap();
        let text = read_tail(path.to_str().unwrap()).unwrap();
        assert_eq!(last_assistant_text(&text).as_deref(), Some("tail?"));
        assert!(!text.contains('#'));
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
