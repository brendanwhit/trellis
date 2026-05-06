# Adopt Orphan Worktrees

Surface git worktrees that exist on disk but Trellis has never tracked, and let the user wrap a managed session around any of them. Targets the case where a worktree was created outside Trellis (e.g., by Claude Code running `git worktree add`, or by hand), and the user now wants a tmux session for it.

## Goal

Make on-disk worktrees discoverable and adoptable through a dedicated screen, without auto-importing them. Adoption is always an explicit user action.

## Non-goals

- Adopting worktrees in repos Trellis hasn't seen yet. The user registers the repo first via `n` (new session); the worktree then appears in discovery.
- Background reconciliation. Discovery runs on-demand when the screen opens.
- Removing stale rows. The cleanup screen already handles tracked worktrees; this work is its inverse, not a replacement.

## Discovery

For each repo in the DB, run `git worktree list --porcelain` (already exposed via `git::list_worktrees`). Drop two kinds of entries:

1. The repo's main worktree itself (path equals `repo.path`).
2. Any path already present as `worktrees.path` in the DB.

What remains is the orphan set: worktrees git knows about that Trellis doesn't. Detached-HEAD worktrees are skipped — adopting them as sessions has no sensible `base_branch`.

This avoids the "bloat with worktrees that linger after merge" failure mode by construction:

- Cleanup removes both the directory and the DB row, so git stops listing the path; it can't reappear in discovery.
- `delete_session` without cleanup keeps the worktree row (with `session_id = NULL`); discovery still excludes it because the path is in the DB.
- The only way an entry shows up is if git tracks it and Trellis genuinely never has — which is exactly the target case.

## UI

New screen `tui/adopt_worktree.rs`. Reachable via `A` from the session list (`a` is unassigned; `A` pairs with `R` for review).

Table layout, three columns:

```
Repo            Branch                Path
─────────────────────────────────────────────────────────────
trellis         feat-discover         …/worktrees/trellis/feat-discover
fdy             tom/cleanup-job       /tmp/quick-experiment
```

Path uses the existing `truncate_start` helper so the leaf segment stays visible (ellipsis at the start when the path overflows the column).

**Keys:**

- `j`/`k` / `↓`/`↑` — navigate
- `enter` — adopt the highlighted row (opens inline name prompt)
- `q`/`esc` — close

Footer shows the keybinds in the same style the other screens use. Empty state renders "No untracked worktrees found." with a hint to use `n` to create new sessions.

The screen refreshes its discovery on `new()` and on `on_resume()` so the list stays accurate after an adopt or after returning from another screen.

## Adopt Flow

`enter` on a row puts the screen into an inline name-prompt mode (mirroring the `Rename` / `NewTab` inline modes in `session_list.rs`). The input is pre-filled with `tmux::sanitize_session_name(branch)`. Footer hint: `[enter] confirm  [esc] cancel`.

On confirm:

1. Validate the name is non-empty.
2. Reject if `manager.get_session_by_name(name)` returns Some — show inline error, keep the prompt open for editing.
3. Call `manager.adopt_worktree(repo_id, path, branch, session_name)`.
4. On success, write a `SwitchAction::Session { target: name }` and quit the TUI — the same exit path `n` (new session) and `R` (review) take.
5. On error, show the message inline and let the user retry or cancel.

## Manager API

Two new methods on `Manager`:

```rust
pub struct OrphanWorktree {
    pub repo: Repo,
    pub branch: String,
    pub path: String,
}

pub fn list_orphan_worktrees(&self) -> Vec<OrphanWorktree>;

pub fn adopt_worktree(
    &self,
    repo_id: i64,
    path: &str,
    branch: &str,
    session_name: &str,
) -> Result<Session, String>;
```

`list_orphan_worktrees`:

- Iterates `db::get_repos`, calls `git::list_worktrees` per repo.
- Filters out the repo's main path and any path already in `db::get_worktrees`.
- Filters out detached-HEAD entries. `git::list_worktrees` sets `branch = Some("(detached)")` for these (see `git.rs` `if line == "detached"`), so the filter is `branch != "(detached)"`. Also drops entries with `branch.is_none()` defensively, even though the parser doesn't currently produce them.
- Sorts by `repo.name`, then `branch`, for stable display.

`adopt_worktree`:

- Insert a `worktree` row (path, branch, repo_id, session_id pointing at the session created in the next step).
- Insert a `session` row (name, repo_id, base_branch = the worktree's branch).
- Call the existing `manager::apply_layout(name, path)` to create the tmux session with the default `claude` + `shell` window layout.
- Return the new `Session`.

This is deliberately the same shape as `create_session`, minus the `git worktree add` step. No reuse of `create_session` itself, since its branch-vs-default logic doesn't apply when the path is already known.

## Edge Cases

- **Session-name collision.** Reject with the same inline error pattern the rename flow uses; user edits and retries.
- **Detached HEADs.** Excluded from discovery (see above).
- **Worktree path is inside a repo Trellis doesn't know about.** Not surfaced. Discovery only enumerates `git worktree list` for repos already in the DB.
- **DB row uniqueness.** `worktrees.path` has no UNIQUE constraint today, and the TUI is single-threaded so there's no real race. The exclusion filter on the discovery side prevents a second adopt of the same path. Not addressing the missing constraint in this work — flagged as a latent issue elsewhere in the codebase.

## Files Touched

- `src/manager.rs` — add `OrphanWorktree`, `list_orphan_worktrees`, `adopt_worktree`.
- `src/tui/adopt_worktree.rs` — new screen.
- `src/tui/mod.rs` — register module.
- `src/tui/session_list.rs` — add `A` keybind, footer entry.
- `src/tui/help.rs` — add the new keybind to the help text.
- `README.md` — add to the keybind table.

## Tests

- `manager::list_orphan_worktrees` — DB fixtures for repos + tracked worktrees, mock `git::list_worktrees` results, assert filtering. Existing tests in `db.rs` and `manager.rs` already use in-memory SQLite as a pattern; follow that. The git call needs an injection seam — easiest is to take a closure or pass a list-fn into the function for testability, mirroring how nothing else in `manager.rs` is tested against real git today. If injecting feels heavy, integration-test it manually and unit-test the filter logic against a fixed input list.
- `manager::adopt_worktree` — assert the worktree and session rows land in the DB and reference each other. Skip the tmux side.
- No new tests for the screen itself; matches the existing pattern.
