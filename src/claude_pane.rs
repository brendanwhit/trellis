// trellis/src/claude_pane.rs

use regex::Regex;
use std::fs;
use std::path::Path;

pub fn get_session_id(pane_pid: &str) -> Option<String> {
    if pane_pid.is_empty() {
        return None;
    }
    let pid_file = Path::new("/tmp/claude-sessions").join(format!("pid-{}", pane_pid));
    fs::read_to_string(pid_file).ok().map(|s| s.trim().to_string())
}

pub fn get_first_user_message(session_id: &str) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    if !projects_dir.exists() {
        return None;
    }
    let entries = match fs::read_dir(&projects_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };
    for entry in entries {
        // Skip unreadable entries (matches Python's behavior of iterating past errors)
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let jsonl = entry.path().join(format!("{}.jsonl", session_id));
        if jsonl.exists() {
            return first_user_message_from_jsonl(&jsonl);
        }
    }
    None
}

fn first_user_message_from_jsonl(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        // Skip lines that fail to parse (matches Python's try/except JSONDecodeError: continue)
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_user = entry
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "user")
            .unwrap_or(false);
        if !is_user {
            continue;
        }
        let text = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn summarize_message(message: &str, max_words: usize) -> String {
    let first_line = message.lines().next().unwrap_or("").trim().trim_start_matches('#').trim();
    let words: Vec<&str> = first_line.split_whitespace().take(max_words).collect();
    let name: String = words
        .iter()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| ".,!?:;\"'()[]{}".contains(c))
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
    if name.is_empty() {
        "claude".to_string()
    } else {
        name
    }
}

pub fn classify_pane(pane_text: &str) -> &'static str {
    let lines: Vec<&str> = pane_text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail: &[&str] = if lines.len() > 10 {
        &lines[lines.len() - 10..]
    } else {
        &lines
    };
    if tail.is_empty() {
        return "idle";
    }
    let tail_text: String = tail.join("\n");

    // Use LazyLock to avoid recompiling regexes on every call (this is a hot path —
    // called for every expanded window row on every render cycle)
    use std::sync::LazyLock;
    static PROMPTING_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"❯\s+1\.").unwrap());
    static WORKING_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^\x00-\x7f]\s+.*…").unwrap());

    // Permission dialog: ❯ followed by numbered choice + "Esc to cancel"
    if PROMPTING_RE.is_match(&tail_text) && tail_text.contains("Esc to cancel") {
        return "prompting";
    }

    // Active spinner: non-ASCII char followed by text containing …
    // Matches both short spinners ("✻ Envisioning…") and long ones
    // ("✳ Transition to implementation planning… (3m 51s · ↓ 406 tokens)")
    if WORKING_RE.is_match(&tail_text) {
        return "working";
    }

    "idle"
}

/// Find the most-recent Claude conversation that ran in `cwd`.
///
/// Returns the bare conversation UUID (no `.jsonl` extension), or `None` if
/// no prior conversation exists for this cwd or any I/O error prevents
/// reading the projects directory.
pub fn find_latest_conversation(cwd: &Path) -> Option<String> {
    let projects_dir = dirs::home_dir()?.join(".claude").join("projects");
    find_latest_conversation_in(&projects_dir, cwd)
}

/// Find the most-recent Claude conversation that ran in `cwd`, looking under
/// `projects_dir/<encoded-cwd>/`. Returns the bare UUID (no `.jsonl` extension).
///
/// `projects_dir` is the `~/.claude/projects` location (parameterized for tests).
fn find_latest_conversation_in(projects_dir: &Path, cwd: &Path) -> Option<String> {
    let encoded = cwd.to_string_lossy().replace('/', "-");
    let dir = projects_dir.join(&encoded);
    let entries = fs::read_dir(&dir).ok()?;

    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        match &best {
            Some((best_mtime, _)) if mtime <= *best_mtime => {}
            _ => best = Some((mtime, stem)),
        }
    }
    best.map(|(_, stem)| stem)
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
    fn summarize_empty() {
        assert_eq!(summarize_message("", 4), "claude");
    }

    #[test]
    fn summarize_strips_heading() {
        assert_eq!(summarize_message("## Fix the bug", 4), "fix-the-bug");
    }

    #[test]
    fn classify_idle_empty() {
        assert_eq!(classify_pane(""), "idle");
    }

    #[test]
    fn classify_working() {
        assert_eq!(classify_pane("✻ Envisioning…"), "working");
    }

    #[test]
    fn classify_prompting() {
        let text = "some output\n❯ 1. Yes\n  2. No\nEsc to cancel";
        assert_eq!(classify_pane(text), "prompting");
    }

    #[test]
    fn classify_idle_normal() {
        assert_eq!(classify_pane("❯ some command\noutput"), "idle");
    }

    #[test]
    fn classify_working_long_spinner() {
        // Multi-word spinner line with timing info (thinking/planning state)
        let text = "✳ Transition to implementation planning… (3m 51s · ↓ 406 tokens)\n  ⎿  ✔ Explore project context\n     ◼ Transition to implementation planning";
        assert_eq!(classify_pane(text), "working");
    }

    #[test]
    fn classify_idle_past_tense_spinner() {
        // Idle: spinner shows past tense with duration, no …
        assert_eq!(classify_pane("✻ Brewed for 31s"), "idle");
        assert_eq!(classify_pane("✻ Baked for 2m 36s"), "idle");
        assert_eq!(classify_pane("✻ Cogitated for 43s"), "idle");
        assert_eq!(classify_pane("✻ Cooked for 3m 17s"), "idle");
    }

    #[test]
    fn find_latest_returns_none_when_encoded_dir_missing() {
        use tempfile::tempdir;
        let projects = tempdir().unwrap();
        // /nope/here doesn't have a corresponding encoded dir in projects/
        let result = find_latest_conversation_in(projects.path(), Path::new("/nope/here"));
        assert!(result.is_none());
    }

    #[test]
    fn find_latest_returns_newest_jsonl_by_mtime() {
        use std::fs::File;
        use std::thread::sleep;
        use std::time::Duration;
        use tempfile::tempdir;

        let projects = tempdir().unwrap();
        // cwd we'll fake: /foo/bar -> encoded as "-foo-bar"
        let cwd = Path::new("/foo/bar");
        let encoded = projects.path().join("-foo-bar");
        fs::create_dir_all(&encoded).unwrap();

        // Create three jsonls with distinct mtimes by sleeping between creates.
        // 10ms is enough on every filesystem we care about.
        File::create(encoded.join("aaaa.jsonl")).unwrap();
        sleep(Duration::from_millis(10));
        File::create(encoded.join("bbbb.jsonl")).unwrap();
        sleep(Duration::from_millis(10));
        File::create(encoded.join("cccc.jsonl")).unwrap();
        // Now touch bbbb to make IT the newest by mtime.
        sleep(Duration::from_millis(10));
        File::create(encoded.join("bbbb.jsonl")).unwrap();

        let result = find_latest_conversation_in(projects.path(), cwd);
        assert_eq!(result.as_deref(), Some("bbbb"));
    }

    #[test]
    fn find_latest_ignores_non_jsonl_files() {
        use std::fs::File;
        use std::io::Write;
        use std::thread::sleep;
        use std::time::Duration;
        use tempfile::tempdir;

        let projects = tempdir().unwrap();
        let cwd = Path::new("/foo/bar");
        let encoded = projects.path().join("-foo-bar");
        fs::create_dir_all(&encoded).unwrap();

        // Create the .jsonl FIRST, then a .txt later. If extension filtering were
        // missing, the .txt's newer mtime would win and the result would be a
        // bare stem ("notes") that doesn't match what we want.
        File::create(encoded.join("real.jsonl")).unwrap();
        sleep(Duration::from_millis(10));
        let mut txt = File::create(encoded.join("notes.txt")).unwrap();
        txt.write_all(b"distractor").unwrap();

        let result = find_latest_conversation_in(projects.path(), cwd);
        assert_eq!(result.as_deref(), Some("real"));
    }

    #[test]
    fn find_latest_returns_none_when_dir_has_no_jsonl() {
        use tempfile::tempdir;

        let projects = tempdir().unwrap();
        let cwd = Path::new("/foo/bar");
        let encoded = projects.path().join("-foo-bar");
        fs::create_dir_all(encoded.join("subagents")).unwrap();
        // No .jsonl files at the top level.

        let result = find_latest_conversation_in(projects.path(), cwd);
        assert!(result.is_none());
    }

    #[test]
    fn find_latest_encoded_cwd_uses_dash_substitution() {
        use std::fs::File;
        use tempfile::tempdir;

        let projects = tempdir().unwrap();
        // Three-segment cwd to make the encoding non-trivial.
        let cwd = Path::new("/a/b/c");
        let expected_dir = projects.path().join("-a-b-c");
        fs::create_dir_all(&expected_dir).unwrap();
        File::create(expected_dir.join("only.jsonl")).unwrap();

        let result = find_latest_conversation_in(projects.path(), cwd);
        assert_eq!(result.as_deref(), Some("only"));
    }
}
