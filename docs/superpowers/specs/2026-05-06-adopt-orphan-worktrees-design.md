# Adopt Orphan Worktrees

Surface git worktrees that exist on disk but Trellis has never tracked, and let the user wrap a managed session around any of them. Targets the case where a worktree was created outside Trellis (e.g., by Claude Code running `git worktree add`, or by hand), and the user now wants a tmux session for it.

## Goal

Make on-disk worktrees discoverable and adoptable through a dedicated screen, without auto-importing them. Adoption is always an explicit user action.

## Non-goals

- Adopting worktrees in repos Trellis hasn't seen yet. The user registers the repo first via `n` (new session); the worktree then appears in discovery.
- Background reconciliation. Discovery runs on-demand when the screen opens.
- Removing stale rows. The cleanup screen already handles tracked worktrees; this work is its inverse, not a replacement.

## Discovery

For each repo in the DB, run `git worktree list --porcelain` (already exposed via `git::list_worktrees`). Drop these entries:

1. The repo's main worktree itself (path equals `repo.path`).
2. Any path already present as `worktrees.path` in the DB.
3. Detached-HEAD entries — adopting them as sessions has no sensible `base_branch`.
4. Entries whose directory no longer exists on disk (`!Path::new(&path).is_dir()`). `git worktree list` will continue to report worktrees whose directories were deleted out from under it until `git worktree prune` runs; without this check, adoption would create a session pointing at a missing directory.

What remains is the orphan set: worktrees git knows about, that exist on disk, that Trellis doesn't track.

The combination of filters 2 and 4 is what prevents the "bloat with worktrees that linger after merge" failure mode:

- `cleanup_worktree` succeeds → directory and DB row both gone → not listed.
- `cleanup_worktree` partially succeeds (git remove failed but directory is already gone) → DB row deleted (per `manager.rs:441-447`), directory absent → filter 4 still excludes it from discovery even if `git worktree list` is stale.
- `delete_session(cleanup_worktrees=false)` → row stays with `session_id = NULL` → filter 2 excludes it.
- A genuinely orphaned-after-the-fact case (DB row dropped but directory still on disk) → would surface in discovery, and that's correct: the user can re-adopt or delete the directory.

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

The screen refreshes its discovery on `new()` and on `on_resume()` so the list stays accurate after returning from another screen. (Cleanup and history don't override `on_resume` today; this is a deliberate divergence — the orphan set is cheap to recompute, and freshness matters here in a way it doesn't for cleanup's background staleness check.)

## Adopt Flow

`enter` on a row puts the screen into an inline name-prompt mode (mirroring the `Rename` / `NewTab` inline modes in `session_list.rs`). The input is pre-filled with `tmux::sanitize_session_name(branch)`. Footer hint: `[enter] confirm  [esc] cancel`.

On confirm:

1. Validate the name is non-empty.
2. Reject if `manager.get_session_by_name(name)` returns `Some` (DB-managed collision) — show inline error, keep the prompt open.
3. Reject if `tmux::session_exists(name)` returns true (live unmanaged tmux session collision) — show inline error, keep the prompt open. Both checks are needed: `get_session_by_name` only sees DB-managed sessions, and `apply_layout` would silently no-op against an existing tmux session, leaving a Trellis-managed DB session glued onto someone else's tmux state.
4. Call `manager.adopt_worktree(repo_id, path, branch, session_name)`.
5. On success, write a `SwitchAction::Session { target: name }` and quit the TUI — the same exit path `n` (new session) and `R` (review) take.
6. On error, show the message inline and let the user retry or cancel.

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

`list_orphan_worktrees` is split into a pure filter and a thin IO wrapper, to keep the discovery logic testable without mocking git:

```rust
// Pure: no IO. Used directly by tests.
fn filter_orphans(
    repos: &[Repo],
    tracked_paths: &HashSet<String>,
    git_results: &HashMap<i64, Vec<GitWorktree>>,
) -> Vec<OrphanWorktree>;

// Public: gathers DB + git inputs, calls the pure function.
pub fn list_orphan_worktrees(&self) -> Vec<OrphanWorktree>;
```

The wrapper iterates `db::get_repos`, calls `git::list_worktrees` per repo, **pre-prunes any entry whose path is missing on disk** (filter 4 from §Discovery — `Path::new(&p).is_dir()`), builds the tracked-path set from `db::get_worktrees`, and hands the result to `filter_orphans`. `filter_orphans` applies the remaining three filters (main-path, DB-tracked-path, detached-HEAD) from §Discovery and sorts by `repo.name`, then `branch`, for stable display. Splitting it this way keeps `filter_orphans` IO-free and unit-testable.

For the detached-HEAD filter: `git::list_worktrees` sets `branch = Some("(detached)")` for detached entries (see `git.rs` `if line == "detached"`), so the filter is `branch != "(detached)"`. The parser never returns `None` for `branch` in current input, but the filter handles `None` defensively anyway.

`adopt_worktree` is structured tmux-first to avoid leaving DB rows pointing at a non-existent tmux session if the layout fails:

1. `apply_layout(name, path)` — creates the tmux session and applies the default `claude` + `shell` layout. **This requires changing `apply_layout` to return `Result<(), TmuxError>`** instead of swallowing errors silently. The two existing call sites (`create_session`, `checkout_and_review`) get updated to propagate the error — fixing a parallel latent bug.
2. If the layout call fails, return the error without touching the DB.
3. Insert the `session` row (name, repo_id, base_branch = the worktree's branch).
4. Insert the `worktree` row with `session_id` pointing at the session just inserted.
5. Return the new `Session`.

This is deliberately the same shape as `create_session`, minus the `git worktree add` step. No reuse of `create_session` itself, since its branch-vs-default logic doesn't apply when the path is already known.

## Edge Cases

- **Session-name collision.** Reject with the same inline error pattern the rename flow uses; user edits and retries.
- **Detached HEADs.** Excluded from discovery (see above).
- **Worktree path is inside a repo Trellis doesn't know about.** Not surfaced. Discovery only enumerates `git worktree list` for repos already in the DB.
- **DB row uniqueness.** `worktrees.path` has no UNIQUE constraint today, and the TUI is single-threaded so there's no real race. The exclusion filter on the discovery side prevents a second adopt of the same path. Not addressing the missing constraint in this work — flagged as a latent issue elsewhere in the codebase.

## Files Touched

- `src/manager.rs` — add `OrphanWorktree`, `list_orphan_worktrees`, `filter_orphans`, `adopt_worktree`. Change `apply_layout` to return `Result<(), TmuxError>`. Update `create_session` and `checkout_and_review` to propagate the error.
- `src/tui/adopt_worktree.rs` — new screen.
- `src/tui/mod.rs` — register module.
- `src/tui/session_list.rs` — add `A` keybind, footer entry.
- `src/tui/help.rs` — add the new keybind to the help text.
- `README.md` — add to the keybind table.

## Tests

- `manager::filter_orphans` — pure function, fully unit-testable. Cover: main-path exclusion, DB-tracked-path exclusion, detached-HEAD exclusion, sort order, multi-repo input. No git, no DB, no tmux.
- `manager::list_orphan_worktrees` — not directly tested. The IO wrapper is small; the data-shaping logic is covered by `filter_orphans` and the missing-directory filter is a one-line `is_dir()` check.
- `manager::adopt_worktree` — assert the session and worktree rows land in the DB and reference each other after a successful adopt. Skip the tmux side. (The DB tests already use in-memory SQLite via the pattern in `db.rs::test_conn`.)
- No new tests for the screen itself; matches the existing pattern across `tui/`.
