// trellis/src/agent_pane.rs
//
// Agent panes announce themselves through a small status file instead of
// trellis scraping their screen. A harness extension (see `pi/trellis.ts`)
// writes `~/.local/share/trellis/panes/<pane>.json` keyed by its `$TMUX_PANE`
// (leading `%` dropped) and keeps it current across the agent's lifecycle:
//
//   { "agent": "pi", "pid": 12345, "state": "working", "title": "fix login" }
//
// `state` is one of `idle`, `working`, `prompting`. `title` is the first user
// message of the conversation and feeds the window auto-rename. The file is
// removed on a clean quit; a stale one (crash, pane reused) is rejected by the
// pid liveness check.
//
// Claude Code needs no extension: it publishes the same facts itself in
// `~/.claude/sessions/<pid>.json` (undocumented schema; `status` is
// busy/waiting/idle and `tmux` is `session:@win.%pane`). `read_status` falls
// back to that when a pane has no trellis status file. Claude reports a
// question asked in plain text as `idle`, so an idle Claude pane is upgraded to
// `prompting` when its screen shows the last reply ending in `?`
// (`asked_question`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::tmux;

#[derive(Debug, Clone, Deserialize)]
pub struct PaneStatus {
    pub agent: String,
    pub pid: u32,
    pub state: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub fn status_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home dir")
        .join(".local/share/trellis/panes")
}

/// Where Claude Code keeps its per-process session files.
fn claude_sessions_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().expect("no home dir").join(".claude"))
        .join("sessions")
}

/// Read the status for a tmux pane id (`%12`). `None` when the pane is not an
/// agent pane, the file is malformed, or the writer is no longer alive.
pub fn read_status(pane_id: &str) -> Option<PaneStatus> {
    let status = read_status_in(pane_id, &status_dir(), &claude_sessions_dir())?;
    Some(upgrade_if_asked_question(status, || tmux::capture_pane(pane_id, SCREEN_LINES)))
}

/// Claude's session file calls a plain-text question `idle`; promote it to
/// `prompting` when the pane's screen (read only for idle Claude panes) shows one.
fn upgrade_if_asked_question(
    mut status: PaneStatus,
    screen: impl FnOnce() -> Option<String>,
) -> PaneStatus {
    if status.agent == "claude" && status.state == "idle" && screen().is_some_and(|s| asked_question(&s)) {
        status.state = "prompting".to_string();
    }
    status
}

/// Enough scrollback to reach the last reply and the "Cooked for 3s" line below it.
const SCREEN_LINES: u32 = 40;

/// Whether an idle Claude Code screen ends with the assistant asking a question
/// in prose. Anchors on the "✻ Cooked for 3s" line Claude prints when a turn
/// ends, then checks that the nearest line of reply text above it ends in `?`
/// and that nothing has started a new turn below it.
fn asked_question(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let Some(idle_at) = lines.iter().rposition(|l| is_turn_end_marker(l)) else {
        return false;
    };

    // A typed prompt, tool output or a tool call below the marker means a newer
    // turn has begun and the question is stale.
    let new_turn_below = lines[idle_at + 1..].iter().map(|l| l.trim()).any(|l| {
        (l.starts_with('❯') && !l.trim_start_matches('❯').trim().is_empty())
            || l.starts_with('⎿')
            || is_tool_call(l)
    });
    if new_turn_below {
        return false;
    }

    lines[..idle_at]
        .iter()
        .rev()
        .find(|l| !is_chrome(l))
        .is_some_and(|l| l.trim_end().ends_with('?'))
}

/// "✻ Cooked for 3s", "✻ Worked for 1m 22s", "✻ Thought for 10s".
fn is_turn_end_marker(line: &str) -> bool {
    fn parse(line: &str) -> Option<()> {
        let mut chars = line.trim_start().chars();
        if chars.next()?.is_ascii() {
            return None;
        }
        let rest = chars.as_str();
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let mut words = rest.split_whitespace();
        let verb = words.next()?;
        let past_tense = verb.len() > 2
            && verb.ends_with("ed")
            && verb.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !(verb == "Thought" || past_tense) || words.next()? != "for" {
            return None;
        }
        let amount = words.next()?;
        let digits = amount.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let unit = match &amount[digits..] {
            "" => words.next()?,
            attached => attached,
        };
        matches!(unit.chars().next()?, 'h' | 'm' | 's').then_some(())
    }
    parse(line).is_some()
}

/// "⏺ Bash(ls)": a glyph, then a tool name and its argument list.
fn is_tool_call(line: &str) -> bool {
    let mut chars = line.chars();
    if chars.next().is_none_or(|c| c.is_ascii()) {
        return false;
    }
    let rest = chars.as_str();
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    !name.is_empty() && rest.trim_start()[name.len()..].starts_with('(')
}

/// Screen furniture that can sit between a reply and its turn-end marker:
/// input-box rules, the prompt, the mode line, and live spinner rows.
fn is_chrome(line: &str) -> bool {
    let s = line.trim();
    if s.starts_with(['─', '❯', '⏵']) {
        return true;
    }
    // "✻ Thinking…": glyph, then text up to an ellipsis with no "(" before it.
    let mut chars = s.chars();
    let glyph_then_space = chars.next().is_some_and(|c| !c.is_ascii()) && chars.as_str().starts_with(char::is_whitespace);
    glyph_then_space && chars.as_str().trim_start().split_once('…').is_some_and(|(phrase, _)| !phrase.is_empty() && !phrase.contains('('))
}

fn read_status_in(pane_id: &str, trellis_dir: &Path, claude_dir: &Path) -> Option<PaneStatus> {
    let key = pane_id.strip_prefix('%').unwrap_or(pane_id);
    if key.is_empty() {
        return None;
    }
    read_trellis_status(key, trellis_dir).or_else(|| read_claude_status(key, claude_dir))
}

fn read_trellis_status(key: &str, dir: &Path) -> Option<PaneStatus> {
    let raw = fs::read(dir.join(format!("{}.json", key))).ok()?;
    let status: PaneStatus = serde_json::from_slice(&raw).ok()?;
    if !process_alive(status.pid) {
        return None;
    }
    Some(status)
}

/// The subset of Claude Code's session file trellis relies on.
#[derive(Deserialize)]
struct ClaudeSession {
    pid: u32,
    #[serde(default)]
    tmux: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Find the live Claude session running in pane `key` (no leading `%`).
/// Unknown `status` values are skipped rather than guessed at, since the
/// schema is Claude's to change.
fn read_claude_status(key: &str, dir: &Path) -> Option<PaneStatus> {
    let pane_suffix = format!(".%{}", key);
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read(&path) else { continue };
        let Ok(session) = serde_json::from_slice::<ClaudeSession>(&raw) else { continue };
        if !session.tmux.as_deref().is_some_and(|t| t.ends_with(&pane_suffix)) {
            continue;
        }
        let state = match session.status.as_deref() {
            Some("busy") => "working",
            Some("waiting") => "prompting",
            Some("idle") => "idle",
            _ => continue,
        };
        if !process_alive(session.pid) {
            continue;
        }
        return Some(PaneStatus {
            agent: "claude".to_string(),
            pid: session.pid,
            state: state.to_string(),
            title: None,
        });
    }
    None
}

fn process_alive(pid: u32) -> bool {
    // Signal 0 performs the permission/existence check without delivering.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Collapse a conversation's first message into a short tmux window name.
pub fn summarize_message(message: &str, max_words: usize) -> String {
    let first_line = message.lines().next().unwrap_or("").trim().trim_start_matches('#').trim();
    let words: Vec<&str> = first_line.split_whitespace().take(max_words).collect();
    let name: String = words
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| ".,!?:;\"'`()[]{}".contains(c))
                .to_string()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if name.len() > 30 {
        if let Some(pos) = name[..30].rfind('-') {
            return name[..pos].to_string();
        }
        return name[..30].to_string();
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_basic() {
        assert_eq!(summarize_message("Fix the login bug", 4), "fix-the-login-bug");
    }

    #[test]
    fn summarize_truncates_words() {
        assert_eq!(
            summarize_message("one two three four five six", 4),
            "one-two-three-four"
        );
    }

    #[test]
    fn summarize_strips_punctuation() {
        assert_eq!(summarize_message("Hello, world!", 4), "hello-world");
    }

    #[test]
    fn summarize_strips_backticks() {
        assert_eq!(summarize_message("run `echo ok` with bash", 4), "run-echo-ok-with");
    }

    #[test]
    fn summarize_empty() {
        assert_eq!(summarize_message("", 4), "");
    }

    #[test]
    fn summarize_strips_heading() {
        assert_eq!(summarize_message("## Fix the bug", 4), "fix-the-bug");
    }

    #[test]
    fn status_rejects_dead_writer() {
        let raw = r#"{"agent":"pi","pid":4194303,"state":"idle"}"#;
        let status: PaneStatus = serde_json::from_str(raw).unwrap();
        // PID_MAX on macOS is 99998, on Linux defaults to 4194304, so this pid
        // cannot be a live process on either.
        assert!(!process_alive(status.pid));
    }

    #[test]
    fn status_accepts_live_writer_and_optional_title() {
        let raw = format!(
            r#"{{"agent":"pi","pid":{},"state":"working"}}"#,
            std::process::id()
        );
        let status: PaneStatus = serde_json::from_str(&raw).unwrap();
        assert!(process_alive(status.pid));
        assert_eq!(status.state, "working");
        assert!(status.title.is_none());
    }

    fn write_claude_session(dir: &Path, name: &str, pid: u32, tmux: &str, status: &str) {
        let body = format!(r#"{{"pid":{pid},"tmux":"{tmux}","status":"{status}"}}"#);
        fs::write(dir.join(format!("{}.json", name)), body).unwrap();
    }

    #[test]
    fn claude_status_maps_session_file_states_by_pane() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        for (status, expected) in [("busy", "working"), ("waiting", "prompting"), ("idle", "idle")] {
            write_claude_session(dir.path(), "s", me, "proj/feat:@3.%41", status);
            let got = read_claude_status("41", dir.path()).unwrap();
            assert_eq!(got.state, expected);
            assert_eq!(got.agent, "claude");
            assert_eq!(got.pid, me);
        }
    }

    #[test]
    fn claude_status_matches_the_exact_pane_only() {
        let dir = tempfile::tempdir().unwrap();
        write_claude_session(dir.path(), "s", std::process::id(), "proj/feat:@3.%141", "busy");
        assert!(read_claude_status("41", dir.path()).is_none());
    }

    #[test]
    fn claude_status_ignores_dead_pids_unknown_states_and_non_tmux_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        write_claude_session(dir.path(), "dead", 4194303, "p:@1.%7", "busy");
        write_claude_session(dir.path(), "shell", me, "p:@1.%8", "shell");
        fs::write(dir.path().join("none.json"), format!(r#"{{"pid":{me},"tmux":null,"status":"busy"}}"#)).unwrap();
        fs::write(dir.path().join("junk.json"), "not json").unwrap();
        fs::write(dir.path().join("x.key"), "ignored").unwrap();
        for pane in ["7", "8"] {
            assert!(read_claude_status(pane, dir.path()).is_none());
        }
    }

    #[test]
    fn trellis_status_file_wins_over_claude_session_file() {
        let trellis = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let me = std::process::id();
        fs::write(
            trellis.path().join("9.json"),
            format!(r#"{{"agent":"pi","pid":{me},"state":"prompting"}}"#),
        )
        .unwrap();
        write_claude_session(claude.path(), "s", me, "p:@1.%9", "busy");

        let got = read_status_in("%9", trellis.path(), claude.path()).unwrap();
        assert_eq!(got.agent, "pi");

        fs::remove_file(trellis.path().join("9.json")).unwrap();
        let got = read_status_in("%9", trellis.path(), claude.path()).unwrap();
        assert_eq!((got.agent.as_str(), got.state.as_str()), ("claude", "working"));
    }

    const QUESTION: &str = "\
❯ Without using any tools, ask me in plain text what my favorite color is.

⏺ What's your favorite color?

✻ Cooked for 1s

────────────────────────────────────────
❯ 
────────────────────────────────────────
  -- INSERT -- ⏸ manual mode on
";

    #[test]
    fn asked_question_detects_a_reply_ending_in_a_question_mark() {
        assert!(asked_question(QUESTION));
    }

    #[test]
    fn asked_question_ignores_a_finished_reply() {
        let done = QUESTION.replace("What's your favorite color?", "I haven't done either.");
        assert!(!asked_question(&done));
    }

    #[test]
    fn asked_question_needs_the_turn_end_marker() {
        let working = QUESTION.replace("✻ Cooked for 1s", "✻ Cooking… (3s)");
        assert!(!asked_question(&working));
        assert!(!asked_question(""));
    }

    #[test]
    fn asked_question_goes_stale_once_a_new_turn_starts_below_the_marker() {
        for next in ["❯ red", "⏺ Bash(ls)", "  ⎿  done"] {
            let screen = QUESTION.replace("────────────────────────────────────────\n❯ \n", &format!("{}\n❯ \n", next));
            assert!(!asked_question(&screen), "{next}");
        }
    }

    #[test]
    fn asked_question_looks_through_spinner_rows_and_recaps_below_the_marker() {
        let screen = QUESTION.replace(
            "✻ Cooked for 1s\n",
            "✻ Cooked for 1m 22s · done 12:16 PM\n※ recap: waiting for you to pick (a) or (b).\n",
        );
        assert!(asked_question(&screen));
    }

    #[test]
    fn turn_end_marker_accepts_claude_verbs_and_units() {
        for ok in ["✻ Brewed for 31s", "✻ Worked for 22s · done 12:12 PM", "✻ Thought for 10 s", "✻ Crunched for 3m 57s"] {
            assert!(is_turn_end_marker(ok), "{ok}");
        }
        for bad in ["Cooked for 1s", "✻ Cooking for 1s", "✻ Cooked for s", "✻ Cooked after 1s"] {
            assert!(!is_turn_end_marker(bad), "{bad}");
        }
    }

    fn status(agent: &str, state: &str) -> PaneStatus {
        PaneStatus { agent: agent.into(), pid: 1, state: state.into(), title: None }
    }

    #[test]
    fn idle_claude_asking_a_question_becomes_prompting() {
        let got = upgrade_if_asked_question(status("claude", "idle"), || Some(QUESTION.to_string()));
        assert_eq!(got.state, "prompting");
    }

    #[test]
    fn only_idle_claude_panes_are_upgraded_and_only_those_read_the_screen() {
        for (agent, state) in [("claude", "working"), ("claude", "prompting"), ("pi", "idle")] {
            let got = upgrade_if_asked_question(status(agent, state), || panic!("screen read for {agent}/{state}"));
            assert_eq!(got.state, state);
        }
        let got = upgrade_if_asked_question(status("claude", "idle"), || None);
        assert_eq!(got.state, "idle");
    }
}
