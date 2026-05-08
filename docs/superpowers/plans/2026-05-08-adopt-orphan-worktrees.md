# Adopt Orphan Worktrees Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface git worktrees that exist on disk but Trellis has never tracked, and let the user wrap a managed session around any of them via a new `A` keybind on the session list.

**Architecture:** New top-level screen `tui/adopt_worktree.rs`. Two new `Manager` methods (`list_orphan_worktrees`, `adopt_worktree`) plus a pure helper (`filter_orphans`) for testability. `apply_layout` changes from `()` to `Result<(), TmuxError>` and is moved to run before DB writes in all three call sites (existing two + new), so a tmux failure never leaves orphan DB rows.

**Tech Stack:** Rust, ratatui, crossterm, rusqlite, existing `git::list_worktrees` (porcelain parser).

**Spec:** `docs/superpowers/specs/2026-05-06-adopt-orphan-worktrees-design.md`

---

## File Structure

| File | Purpose |
|------|---------|
| `src/manager.rs` | Add `OrphanWorktree`, `filter_orphans`, `list_orphan_worktrees`, `persist_adopted_worktree`, `adopt_worktree`. Refactor `apply_layout` to return `Result`. Reorder `create_session` and `checkout_and_review` to run tmux before DB writes. New `tests` module with manager-level fixture. |
| `src/tui/adopt_worktree.rs` | New screen: discovery list + inline name-prompt mode. |
| `src/tui/mod.rs` | Register `adopt_worktree` module. |
| `src/tui/session_list.rs` | New `A` keybind, footer entry, `action_adopt_worktree` method. |
| `src/tui/help.rs` | Add `A` to the session-list help text. |
| `README.md` | Add `A` row to the keybind table. |

---

### Task 1: Refactor `apply_layout` to return `Result` and reorder existing call sites

`apply_layout` currently swallows `tmux::new_session` errors silently — if the tmux session can't be created (collision, tmux not running), the function returns `()` and the caller proceeds to write DB rows for a session that doesn't exist in tmux. Fix the signature, fix both existing callers.

**Files:**
- Modify: `src/manager.rs` (lines 23-43, 159-225, 293-364)

- [ ] **Step 1: Change `apply_layout` signature to return `Result<(), tmux::TmuxError>`**

Replace the function at `src/manager.rs:23-43` with:

```rust
/// Create a tmux session and set up windows according to the default layout.
pub fn apply_layout(session_name: &str, working_dir: &str) -> Result<(), tmux::TmuxError> {
    tmux::new_session(session_name, working_dir)?;

    for (i, &(name, command)) in DEFAULT_LAYOUT.iter().enumerate() {
        if i == 0 {
            let _ = tmux::rename_window(session_name, 1, name);
        } else {
            let _ = tmux::new_window(session_name, name, Some(working_dir));
        }
        if let Some(cmd) = command {
            let target = format!("{}:{}", session_name, name);
            tmux::send_keys(&target, &[cmd, "Enter"]);
        }
    }

    let _ = tmux::select_window(session_name, 1);
    Ok(())
}
```

- [ ] **Step 2: Reorder `create_session` to call `apply_layout` before DB writes**

Find the existing `create_session` (`src/manager.rs:159-225`). Replace its body with:

```rust
    pub fn create_session(
        &self,
        repo_path: &str,
        base_branch: &str,
        session_name: &str,
        subdirectory: Option<&str>,
    ) -> Result<Session, String> {
        let repo = self.get_or_create_repo(repo_path);

        let default = &repo.default_branch;
        let start_dir = if base_branch == default {
            repo_path.to_string()
        } else {
            // Fetch latest before creating worktree (non-fatal — stale state is acceptable)
            let _ = git::fetch_and_pull(repo_path, default);

            let wt_path = self.worktree_path(&repo.name, base_branch);
            match git::create_worktree(repo_path, &wt_path, base_branch, default) {
                Ok(()) => {}
                Err(e) => {
                    // Branch/worktree may already exist - use it if the dir is there
                    if !Path::new(&wt_path).exists() {
                        return Err(format!("Worktree creation failed: {}", e));
                    }
                }
            }
            wt_path
        };

        let effective_dir = if let Some(sub) = subdirectory {
            Path::new(&start_dir).join(sub).to_string_lossy().to_string()
        } else {
            start_dir.clone()
        };

        // tmux first — fail loudly before touching the DB.
        apply_layout(session_name, &effective_dir)
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );

        // Record worktree if we created one
        if start_dir != repo_path {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: start_dir,
                    branch: base_branch.to_string(),
                    session_id: session.id,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
        }

        Ok(session)
    }
```

- [ ] **Step 3: Reorder `checkout_and_review` to call `apply_layout` before DB writes**

Find `checkout_and_review` (`src/manager.rs:293-364`). The relevant change is in the section after the existing-session early-return. Locate this block:

```rust
        // Create session
        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: branch.clone(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );
        apply_layout(&session_name, &wt_path);

        // Record worktree
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id: repo.id.unwrap(),
                path: wt_path.clone(),
                branch: branch.clone(),
                session_id: session.id,
                tmux_window: None,
                created_at: utc_now(),
            },
        );
```

Replace with:

```rust
        // tmux first — fail loudly before touching the DB.
        apply_layout(&session_name, &wt_path)
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: branch.clone(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );

        // Record worktree
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id: repo.id.unwrap(),
                path: wt_path.clone(),
                branch: branch.clone(),
                session_id: session.id,
                tmux_window: None,
                created_at: utc_now(),
            },
        );
```

- [ ] **Step 4: Build to verify the type-check passes**

Run: `cargo build`
Expected: clean build, no errors. The pre-existing 5 warnings remain (`new_session.rs:367` unused param + dead code in `rename.rs`) — those are unrelated to this work.

- [ ] **Step 5: Run tests to verify nothing regressed**

Run: `cargo test --quiet`
Expected: `40 passed; 0 failed`. (No new tests yet; all existing tests still pass.)

- [ ] **Step 6: Commit**

```bash
git add src/manager.rs
git commit -m "refactor: apply_layout returns Result; tmux runs before DB writes"
```

---

### Task 2: Add `OrphanWorktree`, `filter_orphans` (with tests), and `list_orphan_worktrees`

Discovery side. `filter_orphans` is the pure function (testable, no IO). `list_orphan_worktrees` is the thin IO wrapper that pulls in DB + git inputs and applies the on-disk-existence filter before delegating.

**Files:**
- Modify: `src/manager.rs` (add new types and methods near the other public methods; tests in the existing `#[cfg(test)] mod tests` will need to be created — manager.rs has no tests today)

- [ ] **Step 1: Add the `OrphanWorktree` struct and `filter_orphans` signature stub**

In `src/manager.rs`, add this single import alongside the existing imports (top of file, near `use crate::tmux;`):

```rust
use crate::git::GitWorktree;
```

(`HashMap` and `HashSet` are already imported via `use std::collections::{HashMap, HashSet};` — don't add duplicates.)

Add this struct after the existing `Manager` struct definition but before `impl Manager` (around `src/manager.rs:71`):

```rust
#[derive(Debug, Clone)]
pub struct OrphanWorktree {
    pub repo: Repo,
    pub branch: String,
    pub path: String,
}
```

Add this free function (not a method on `Manager`) near `apply_layout` at the top of the file:

```rust
/// Pure filter: given repos, the set of paths Trellis already tracks, and
/// per-repo `git worktree list` results, return the worktrees Trellis should
/// surface as adoptable. Excludes the repo's main path, anything in
/// `tracked_paths`, and detached-HEAD entries. The IO-level missing-directory
/// filter happens in `Manager::list_orphan_worktrees` before calling this.
fn filter_orphans(
    repos: &[Repo],
    tracked_paths: &HashSet<String>,
    git_results: &HashMap<i64, Vec<GitWorktree>>,
) -> Vec<OrphanWorktree> {
    let mut out: Vec<OrphanWorktree> = Vec::new();
    for repo in repos {
        let Some(repo_id) = repo.id else { continue };
        let Some(entries) = git_results.get(&repo_id) else { continue };
        for entry in entries {
            if entry.path == repo.path {
                continue; // main worktree
            }
            if tracked_paths.contains(&entry.path) {
                continue; // already tracked
            }
            let Some(branch) = entry.branch.as_ref() else {
                continue; // defensive: parser doesn't currently produce None
            };
            if branch == "(detached)" {
                continue;
            }
            out.push(OrphanWorktree {
                repo: repo.clone(),
                branch: branch.clone(),
                path: entry.path.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        a.repo.name.cmp(&b.repo.name).then_with(|| a.branch.cmp(&b.branch))
    });
    out
}
```

- [ ] **Step 2: Add the test module with TDD-style tests for `filter_orphans`**

At the very bottom of `src/manager.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitWorktree;

    fn make_repo(id: i64, name: &str, path: &str) -> Repo {
        Repo {
            id: Some(id),
            path: path.to_string(),
            name: name.to_string(),
            default_branch: "main".to_string(),
        }
    }

    fn make_gw(path: &str, branch: Option<&str>) -> GitWorktree {
        GitWorktree {
            path: path.to_string(),
            branch: branch.map(|b| b.to_string()),
            commit: None,
        }
    }

    #[test]
    fn filter_orphans_excludes_main_worktree() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![make_gw("/dev/trellis", Some("main"))],
        );
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_tracked_paths() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![make_gw("/wt/trellis/feat-a", Some("feat-a"))],
        );
        let mut tracked = HashSet::new();
        tracked.insert("/wt/trellis/feat-a".to_string());
        let result = filter_orphans(&[repo], &tracked, &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_detached_head() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![make_gw("/wt/trellis/probe", Some("(detached)"))],
        );
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_excludes_missing_branch_defensively() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(1, vec![make_gw("/wt/trellis/x", None)]);
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_orphans_includes_untracked_branch() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![
                make_gw("/dev/trellis", Some("main")),
                make_gw("/wt/trellis/feat-a", Some("feat-a")),
            ],
        );
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].branch, "feat-a");
        assert_eq!(result[0].path, "/wt/trellis/feat-a");
        assert_eq!(result[0].repo.name, "trellis");
    }

    #[test]
    fn filter_orphans_sorts_by_repo_then_branch() {
        let r1 = make_repo(1, "alpha", "/dev/alpha");
        let r2 = make_repo(2, "beta", "/dev/beta");
        let mut git_results = HashMap::new();
        git_results.insert(
            1,
            vec![
                make_gw("/wt/alpha/zeta", Some("zeta")),
                make_gw("/wt/alpha/aaa", Some("aaa")),
            ],
        );
        git_results.insert(
            2,
            vec![make_gw("/wt/beta/mid", Some("mid"))],
        );
        let result = filter_orphans(&[r1, r2], &HashSet::new(), &git_results);
        let names: Vec<(String, String)> = result
            .iter()
            .map(|o| (o.repo.name.clone(), o.branch.clone()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("alpha".to_string(), "aaa".to_string()),
                ("alpha".to_string(), "zeta".to_string()),
                ("beta".to_string(), "mid".to_string()),
            ]
        );
    }

    #[test]
    fn filter_orphans_skips_repos_without_git_results() {
        let repo = make_repo(1, "trellis", "/dev/trellis");
        let git_results: HashMap<i64, Vec<GitWorktree>> = HashMap::new();
        let result = filter_orphans(&[repo], &HashSet::new(), &git_results);
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass (TDD: tests were written against the implementation in Step 1, so they should pass on the first run)**

Run: `cargo test --quiet manager::tests::filter_orphans`
Expected: `7 passed; 0 failed`.

If anything fails, debug `filter_orphans` against the failing case before continuing.

- [ ] **Step 4: Add `list_orphan_worktrees` (the IO wrapper)**

Add this method to the `impl Manager` block, near the other public read methods (e.g. after `get_all_worktrees` at the end of `impl Manager`):

```rust
    /// Discover worktrees git knows about (per registered repo) that Trellis
    /// doesn't track. Filters out missing directories on disk before delegating
    /// the data filter to `filter_orphans`.
    pub fn list_orphan_worktrees(&self) -> Vec<OrphanWorktree> {
        let repos = db::get_repos(&self.conn);
        let tracked_paths: HashSet<String> = db::get_worktrees(&self.conn)
            .into_iter()
            .map(|wt| wt.path)
            .collect();
        let mut git_results: HashMap<i64, Vec<GitWorktree>> = HashMap::new();
        for repo in &repos {
            let Some(repo_id) = repo.id else { continue };
            let entries = match git::list_worktrees(&repo.path) {
                Ok(es) => es,
                Err(_) => continue, // skip repos whose git invocation fails
            };
            // Filter 4 (IO): drop entries whose directory no longer exists on
            // disk. `git worktree list` reports them until `git worktree prune`
            // runs.
            let live: Vec<GitWorktree> = entries
                .into_iter()
                .filter(|e| Path::new(&e.path).is_dir())
                .collect();
            git_results.insert(repo_id, live);
        }
        filter_orphans(&repos, &tracked_paths, &git_results)
    }
```

- [ ] **Step 5: Build to verify**

Run: `cargo build`
Expected: clean build.

- [ ] **Step 6: Run all tests**

Run: `cargo test --quiet`
Expected: `47 passed; 0 failed` (40 existing + 7 new).

- [ ] **Step 7: Commit**

```bash
git add src/manager.rs
git commit -m "feat: discover orphan worktrees via filter_orphans + list_orphan_worktrees"
```

---

### Task 3: Add adoption: `persist_adopted_worktree` (with test) and `adopt_worktree`

Adoption side. The DB writes are split into a private helper (`persist_adopted_worktree`) so the test can exercise just the DB behavior without touching tmux. The public `adopt_worktree` calls `apply_layout` first, then the helper.

**Note on test scope.** The spec asks for a test of `adopt_worktree` that asserts the rows land and reference each other. The plan tests `persist_adopted_worktree` instead, because invoking the public method requires a live tmux process (which the test environment doesn't have). The helper-test exercises the same behavior the spec is concerned with — DB rows landing correctly and the session/worktree linkage — without the tmux dependency. This is a deliberate deviation from a literal reading of the spec.

**Files:**
- Modify: `src/manager.rs`

- [ ] **Step 1: Add a test fixture for `Manager` to the existing tests module**

At the top of the `#[cfg(test)] mod tests` block (above the existing fixture helpers), add:

```rust
    use rusqlite::Connection;

    fn make_test_manager() -> Manager {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (id INTEGER PRIMARY KEY, path TEXT NOT NULL, name TEXT NOT NULL, default_branch TEXT NOT NULL);
             CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT NOT NULL, repo_id INTEGER NOT NULL, base_branch TEXT NOT NULL, created_at TEXT NOT NULL, last_selected_at TEXT, FOREIGN KEY (repo_id) REFERENCES repos(id));
             CREATE TABLE worktrees (id INTEGER PRIMARY KEY, session_id INTEGER, repo_id INTEGER NOT NULL, path TEXT NOT NULL, branch TEXT NOT NULL, tmux_window INTEGER, created_at TEXT NOT NULL, FOREIGN KEY (session_id) REFERENCES sessions(id), FOREIGN KEY (repo_id) REFERENCES repos(id));
             CREATE TABLE config (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        ).unwrap();
        Manager::new(conn)
    }
```

(This duplicates `db.rs::test_conn`'s schema. Acceptable — schema is small and stable. Alternative would be to expose a `pub(crate)` helper from `db.rs`, but that's wider-scope refactoring than this task requires.)

- [ ] **Step 2: Write the failing test for `persist_adopted_worktree`**

Add this test to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn persist_adopted_worktree_writes_session_and_worktree_rows() {
        let mgr = make_test_manager();
        let repo = db::add_repo(
            &mgr.conn,
            &Repo {
                id: None,
                path: "/dev/trellis".into(),
                name: "trellis".into(),
                default_branch: "main".into(),
            },
        );
        let session = mgr.persist_adopted_worktree(
            repo.id.unwrap(),
            "/wt/trellis/feat-a",
            "feat-a",
            "trellis-feat-a",
        );

        // Session row landed
        assert_eq!(session.name, "trellis-feat-a");
        assert_eq!(session.repo_id, repo.id.unwrap());
        assert_eq!(session.base_branch, "feat-a");
        let from_db = db::get_session_by_name(&mgr.conn, "trellis-feat-a").unwrap();
        assert_eq!(from_db.id, session.id);

        // Worktree row landed and references the session
        let worktrees = db::get_worktrees_for_session(&mgr.conn, session.id.unwrap());
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, "/wt/trellis/feat-a");
        assert_eq!(worktrees[0].branch, "feat-a");
        assert_eq!(worktrees[0].repo_id, repo.id.unwrap());
        assert_eq!(worktrees[0].session_id, session.id);
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --quiet manager::tests::persist_adopted_worktree_writes_session_and_worktree_rows`
Expected: FAIL with "no method named `persist_adopted_worktree` found for struct `Manager`" (or similar compile error).

- [ ] **Step 4: Implement `persist_adopted_worktree` and `adopt_worktree`**

In `impl Manager`, near `add_tab` (around `src/manager.rs:367`), add:

```rust
    /// Insert the session and worktree rows for a freshly-adopted worktree.
    /// Private helper — `adopt_worktree` is the public entry point.
    fn persist_adopted_worktree(
        &self,
        repo_id: i64,
        path: &str,
        branch: &str,
        session_name: &str,
    ) -> Session {
        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id,
                base_branch: branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );
        db::add_worktree(
            &self.conn,
            &Worktree {
                id: None,
                repo_id,
                session_id: session.id,
                path: path.to_string(),
                branch: branch.to_string(),
                tmux_window: None,
                created_at: utc_now(),
            },
        );
        session
    }

    /// Wrap a managed session around an existing on-disk worktree. Runs tmux
    /// first so a layout failure never leaves DB rows pointing at a
    /// non-existent tmux session.
    pub fn adopt_worktree(
        &self,
        repo_id: i64,
        path: &str,
        branch: &str,
        session_name: &str,
    ) -> Result<Session, String> {
        apply_layout(session_name, path)
            .map_err(|e| format!("Failed to create tmux session: {}", e))?;
        Ok(self.persist_adopted_worktree(repo_id, path, branch, session_name))
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --quiet manager::tests::persist_adopted_worktree_writes_session_and_worktree_rows`
Expected: `1 passed`.

- [ ] **Step 6: Run all tests**

Run: `cargo test --quiet`
Expected: `48 passed; 0 failed`.

- [ ] **Step 7: Commit**

```bash
git add src/manager.rs
git commit -m "feat: adopt_worktree + persist_adopted_worktree with DB-side test"
```

---

### Task 4: Create the `AdoptWorktreeScreen`

The screen renders the orphan list as a table and adopts the highlighted row via an inline name-prompt mode (mirrors `NewTab` and `Rename` patterns in `session_list.rs`).

**Files:**
- Create: `src/tui/adopt_worktree.rs`
- Modify: `src/tui/mod.rs` (register the new module)

- [ ] **Step 1: Create `src/tui/adopt_worktree.rs`**

```rust
// trellis/src/tui/adopt_worktree.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::manager::{Manager, OrphanWorktree};
use crate::switch::SwitchAction;
use crate::tmux;
use crate::utils::truncate_start;

use super::theme;
use super::{ScreenAction, ScreenBehavior};

enum InlineMode {
    None,
    NamePrompt {
        input: String,
        cursor: usize,
        repo_id: i64,
        path: String,
        branch: String,
    },
}

pub struct AdoptWorktreeScreen {
    rows: Vec<OrphanWorktree>,
    table_state: TableState,
    inline_mode: InlineMode,
    status_message: Option<String>,
}

impl AdoptWorktreeScreen {
    pub fn new(manager: &Manager) -> Self {
        let rows = manager.list_orphan_worktrees();
        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            rows,
            table_state,
            inline_mode: InlineMode::None,
            status_message: None,
        }
    }

    fn refresh(&mut self, manager: &Manager) {
        self.rows = manager.list_orphan_worktrees();
        if self.rows.is_empty() {
            self.table_state.select(None);
        } else {
            let cursor = self
                .table_state
                .selected()
                .unwrap_or(0)
                .min(self.rows.len() - 1);
            self.table_state.select(Some(cursor));
        }
    }

    fn cursor_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.table_state.selected().unwrap_or(0);
        if cur + 1 < self.rows.len() {
            self.table_state.select(Some(cur + 1));
        }
    }

    fn cursor_up(&mut self) {
        let cur = self.table_state.selected().unwrap_or(0);
        if cur > 0 {
            self.table_state.select(Some(cur - 1));
        }
    }

    fn start_adopt(&mut self) {
        let cur = match self.table_state.selected() {
            Some(c) if c < self.rows.len() => c,
            _ => return,
        };
        let row = &self.rows[cur];
        let default_name = tmux::sanitize_session_name(&row.branch);
        let cursor = default_name.len();
        self.inline_mode = InlineMode::NamePrompt {
            input: default_name,
            cursor,
            repo_id: row.repo.id.unwrap(),
            path: row.path.clone(),
            branch: row.branch.clone(),
        };
    }

    fn handle_name_prompt_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        manager: &mut Manager,
    ) -> ScreenAction {
        match code {
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            KeyCode::Enter => {
                let (input, repo_id, path, branch) = match &self.inline_mode {
                    InlineMode::NamePrompt {
                        input,
                        repo_id,
                        path,
                        branch,
                        ..
                    } => (input.trim().to_string(), *repo_id, path.clone(), branch.clone()),
                    _ => return ScreenAction::None,
                };
                if input.is_empty() {
                    self.status_message = Some("Name cannot be empty.".to_string());
                    return ScreenAction::None;
                }
                let name = tmux::sanitize_session_name(&input);
                if manager.get_session_by_name(&name).is_some() {
                    self.status_message =
                        Some(format!("Session '{}' already exists.", name));
                    return ScreenAction::None;
                }
                if tmux::session_exists(&name) {
                    self.status_message =
                        Some(format!("tmux session '{}' already exists.", name));
                    return ScreenAction::None;
                }
                match manager.adopt_worktree(repo_id, &path, &branch, &name) {
                    Ok(_) => {
                        self.inline_mode = InlineMode::None;
                        ScreenAction::Switch(SwitchAction::Session { target: name })
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Error: {}", e));
                        ScreenAction::None
                    }
                }
            }
            _ => {
                use super::rename::input_handle_key;
                if let InlineMode::NamePrompt { input, cursor, .. } = &mut self.inline_mode {
                    input_handle_key(input, cursor, code, modifiers);
                }
                ScreenAction::None
            }
        }
    }
}

impl ScreenBehavior for AdoptWorktreeScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let status_height = match &self.inline_mode {
            InlineMode::None => {
                if self.status_message.is_some() {
                    1
                } else {
                    0
                }
            }
            _ => 1,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(status_height),
                Constraint::Length(1),
            ])
            .split(area);

        if self.rows.is_empty() {
            let empty = Paragraph::new(vec![
                Line::from(Span::styled(
                    "No untracked worktrees found.",
                    Style::default().fg(theme::TEXT_DIM),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Use `n` to create new sessions.",
                    Style::default().fg(theme::TEXT_DIM),
                )),
            ])
            .alignment(Alignment::Center);
            f.render_widget(empty, chunks[0]);
        } else {
            let header = Row::new(vec![
                Cell::from("Repo"),
                Cell::from("Branch"),
                Cell::from("Path"),
            ])
            .style(theme::style_header())
            .height(1);

            // Path column gets whatever's left; truncate from the start so
            // the leaf segment stays visible.
            let path_width = chunks[0].width.saturating_sub(20 + 30 + 2) as usize;
            let table_rows: Vec<Row> = self
                .rows
                .iter()
                .map(|w| {
                    Row::new(vec![
                        Cell::from(w.repo.name.clone()),
                        Cell::from(w.branch.clone()),
                        Cell::from(truncate_start(&w.path, path_width.max(10))),
                    ])
                })
                .collect();

            let widths = [
                Constraint::Length(20),
                Constraint::Length(30),
                Constraint::Min(0),
            ];

            let table = Table::new(table_rows, widths)
                .header(header)
                .row_highlight_style(theme::style_cursor())
                .style(theme::style_default())
                .block(Block::default().style(theme::style_default()));

            let mut state = self.table_state.clone();
            f.render_stateful_widget(table, chunks[0], &mut state);
        }

        match &self.inline_mode {
            InlineMode::None => {
                if let Some(ref msg) = self.status_message {
                    let status = Paragraph::new(msg.as_str())
                        .style(Style::default().fg(theme::TEXT_DIM).bg(theme::BG));
                    f.render_widget(status, chunks[1]);
                }
            }
            InlineMode::NamePrompt {
                input,
                cursor: cursor_pos,
                ..
            } => {
                let prefix = "Session name: ";
                let suffix = "  [enter] confirm  [esc] cancel";

                let before = &input[..*cursor_pos];
                let cursor_char_len = input[*cursor_pos..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                let cursor_display = if *cursor_pos < input.len() {
                    input[*cursor_pos..*cursor_pos + cursor_char_len].to_string()
                } else {
                    " ".to_string()
                };
                let after = &input[(*cursor_pos + cursor_char_len).min(input.len())..];

                let spans = vec![
                    Span::styled(prefix, Style::default().fg(theme::ACCENT)),
                    Span::styled(before.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(
                        cursor_display,
                        Style::default().fg(theme::BG).bg(theme::ACCENT),
                    ),
                    Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(suffix, Style::default().fg(theme::TEXT_DIM)),
                ];
                let prompt = Paragraph::new(Line::from(spans))
                    .style(Style::default().bg(theme::BG));
                f.render_widget(prompt, chunks[1]);
            }
        }

        let footer = super::rename::render_footer_bindings(&[
            ("q", "Quit"),
            ("j/k", "Navigate"),
            ("enter", "Adopt"),
            ("esc", "Back"),
        ]);
        f.render_widget(footer, chunks[2]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        if matches!(self.inline_mode, InlineMode::NamePrompt { .. }) {
            return self.handle_name_prompt_key(*code, *modifiers, manager);
        }

        self.status_message = None;

        match code {
            KeyCode::Char('q') | KeyCode::Esc => ScreenAction::Pop,
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor_down();
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor_up();
                ScreenAction::None
            }
            KeyCode::Enter => {
                self.start_adopt();
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }

    fn on_resume(&mut self, manager: &mut Manager) {
        self.refresh(manager);
    }
}
```

- [ ] **Step 2: Register the module in `src/tui/mod.rs`**

In `src/tui/mod.rs`, find this line:

```rust
pub mod adopt_session;
```

Add `pub mod adopt_worktree;` on the line immediately after it:

```rust
pub mod adopt_session;
pub mod adopt_worktree;
```

(Other `pub mod` declarations may exist before/after — leave them as-is.)

- [ ] **Step 3: Build to verify**

Run: `cargo build`
Expected: clean build. (The screen has no consumers yet — that's fine; no `dead_code` warning because the screen is `pub`.)

- [ ] **Step 4: Run tests**

Run: `cargo test --quiet`
Expected: `48 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add src/tui/adopt_worktree.rs src/tui/mod.rs
git commit -m "feat: AdoptWorktreeScreen for discovering and adopting orphan worktrees"
```

---

### Task 5: Wire up the `A` keybind in the session list

**Files:**
- Modify: `src/tui/session_list.rs` (footer + action method + handle_event)

- [ ] **Step 1: Add `action_adopt_worktree` method to `SessionListScreen`**

Find `action_cleanup` in `src/tui/session_list.rs` (around line 893). Add this method directly after it:

```rust
    fn action_adopt_worktree(&self, manager: &Manager) -> ScreenAction {
        let screen = super::adopt_worktree::AdoptWorktreeScreen::new(manager);
        ScreenAction::Push(Box::new(screen))
    }
```

- [ ] **Step 2: Add the `A` keybind to `handle_event`**

Find the keybind match block in `handle_event` (around line 1505). Locate this line:

```rust
            KeyCode::Char('c') => self.action_cleanup(manager),
```

Add the `A` handler immediately after:

```rust
            KeyCode::Char('c') => self.action_cleanup(manager),
            KeyCode::Char('A') => self.action_adopt_worktree(manager),
```

- [ ] **Step 3: Add the footer entry**

Find `render_footer` (around line 961). Locate the entries array. Add `("A", "Adopt")` after `("c", "Cleanup")`:

```rust
    fn render_footer<'a>(&self) -> Paragraph<'a> {
        super::rename::render_footer_bindings(&[
            ("q", "Quit"),
            ("/", "Filter"),
            ("enter", "Switch"),
            ("h/l", "Collapse/Expand"),
            ("n", "New"),
            ("t", "Tab"),
            ("r", "Rename"),
            ("R", "Review"),
            ("d", "Delete"),
            ("H", "History"),
            (".", "Actions"),
            ("c", "Cleanup"),
            ("A", "Adopt"),
            ("S", "Settings"),
            ("?", "Help"),
        ])
    }
```

- [ ] **Step 4: Build to verify**

Run: `cargo build`
Expected: clean build.

- [ ] **Step 5: Run tests**

Run: `cargo test --quiet`
Expected: `48 passed; 0 failed`.

- [ ] **Step 6: Commit**

```bash
git add src/tui/session_list.rs
git commit -m "feat: A keybind opens the adopt-worktree screen from session list"
```

---

### Task 6: Update help screen text

**Files:**
- Modify: `src/tui/help.rs`

- [ ] **Step 1: Add the `A` line to the session-list help section**

In `src/tui/help.rs`, find the existing session-list keybind block in `help_lines()`. Locate this line (around line 33-35):

```rust
        Line::from(vec![
            Span::raw("  "),
            Span::styled("n", Style::default().fg(theme::ACCENT)),
            Span::styled("         New\u{2026} (session, tab, or PR review)", Style::default().fg(theme::TEXT)),
        ]),
```

Add a new line after it:

```rust
        Line::from(vec![
            Span::raw("  "),
            Span::styled("n", Style::default().fg(theme::ACCENT)),
            Span::styled("         New\u{2026} (session, tab, or PR review)", Style::default().fg(theme::TEXT)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("A", Style::default().fg(theme::ACCENT)),
            Span::styled("         Adopt orphan worktree", Style::default().fg(theme::TEXT)),
        ]),
```

(The spacing — three spaces before `A` aligns the description column with the other entries; `A` is one char, then 9 spaces matches the pattern used for `n`, `d`, etc.)

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add src/tui/help.rs
git commit -m "docs: add A keybind to help screen"
```

---

### Task 7: Update the README keybind table

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the `A` row to the keybind table**

In `README.md`, find the keybind table (lines 43-61). Locate this row:

```
| `c` | Cleanup worktrees |
```

Add the new row immediately after it:

```
| `c` | Cleanup worktrees |
| `A` | Adopt an existing worktree as a new session |
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: README keybind for adopting orphan worktrees"
```

---

### Task 8: Smoke test end-to-end

Trellis is a TUI; UI work warrants exercising it manually before declaring done.

- [ ] **Step 1: Build a release binary**

Run: `cargo build --release`
Expected: clean build, produces `target/release/trellis`.

- [ ] **Step 2: Set up an orphan worktree manually**

In any tracked repo (e.g. `~/dev/trellis`), create a worktree outside Trellis:

```bash
cd ~/dev/trellis
git worktree add /tmp/trellis-smoke-test -b smoke-test
```

- [ ] **Step 3: Open Trellis and adopt the orphan**

Run: `target/release/trellis`

Expected sequence:
- Press `A` on the session list. Screen opens, listing `/tmp/trellis-smoke-test` under `trellis` / `smoke-test`.
- Press `enter`. Inline name prompt opens, pre-filled with `smoke-test`.
- Press `enter` to confirm. Trellis exits and either switches to (if inside tmux) or attaches to a new tmux session named `smoke-test`, working directory `/tmp/trellis-smoke-test`, with `claude` and `shell` windows.

- [ ] **Step 4: Verify the worktree no longer appears as an orphan**

Re-launch trellis (`target/release/trellis`), press `A`. The list should be empty (or, at least, should no longer show `/tmp/trellis-smoke-test`).

- [ ] **Step 5: Verify cleanup-then-rediscovery doesn't bloat the DB**

From the session list, navigate to the smoke-test session and delete it (`d` → `y`). Then go to cleanup (`c`), select the smoke-test worktree, delete (`d` → `y`). Then press `A` again. The list should be empty (path is gone from disk; not in DB).

If the worktree directory was somehow not cleaned up (e.g., uncommitted changes blocked git's removal), the cleanup screen would have shown an error and kept it; in that case the orphan would reappear in `A`'s list — that's the expected and correct behavior.

- [ ] **Step 6: Clean up smoke-test artifacts**

```bash
git worktree remove --force /tmp/trellis-smoke-test 2>/dev/null || true
git branch -D smoke-test 2>/dev/null || true
```

- [ ] **Step 7: Final test sweep**

Run: `cargo test --quiet && cargo build --release`
Expected: `48 passed; 0 failed`, clean release build.

---

## Self-Review (already done before plan was committed)

**Spec coverage:** Every section in the spec has a corresponding task.
- Discovery filters 1-3 → `filter_orphans` (Task 2). Filter 4 → `list_orphan_worktrees` (Task 2 Step 4).
- UI (Task 4) covers the table layout, empty state, name prompt, footer, on_resume.
- Adopt flow (Task 4 Step 1, `handle_name_prompt_key`) covers DB collision check, tmux collision check, `adopt_worktree` call, switch action.
- Manager API: `OrphanWorktree`, `filter_orphans`, `list_orphan_worktrees`, `adopt_worktree` — all in Tasks 2 and 3. `apply_layout` signature change in Task 1.
- Edge cases: session-name collision (Task 4), detached HEADs (Task 2 filter), DB row uniqueness (acknowledged, not addressed).
- Files Touched: each entry has a corresponding task.
- Tests: `filter_orphans` (Task 2 Step 2). `persist_adopted_worktree` (Task 3 Step 2). `list_orphan_worktrees` not directly tested per spec.

**Type / signature consistency:** `OrphanWorktree { repo: Repo, branch: String, path: String }` is used the same way in `filter_orphans`, `list_orphan_worktrees`, and `AdoptWorktreeScreen`. `apply_layout(&str, &str) -> Result<(), TmuxError>` is consistent across Task 1 and Task 3. `adopt_worktree(repo_id: i64, path: &str, branch: &str, session_name: &str) -> Result<Session, String>` matches the spec and is used identically in `handle_name_prompt_key`.

**Placeholder scan:** No TBDs, TODOs, "implement later", "fill in details", or unspecified error handling. Every code step has the actual code; every command step has the exact command.
