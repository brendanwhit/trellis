# Codebase Health Check — 2026-04-21

**Scope:** Full codebase (`/Users/tom/dev/trellis`)
**Context:** Rust TUI application for managing tmux sessions and git worktrees together, built with ratatui/crossterm/rusqlite. The entire codebase is actively developed with recent commits touching session list, tmux integration, and inline TUI interactions.

## Executive Summary

The trellis codebase is well-structured for its size (~5,500 lines of Rust across 25 source files) with clear separation between TUI, business logic, and data layers. However, **the central `SessionListScreen` has grown into a 1,593-line god object** that embeds five inline interaction modes and mixes data building with I/O side effects, making it the highest-risk area for regressions. **Three entire screen modules (`ActionMenuScreen`, `ReviewScreen`, `NewTabScreen`) are dead code** — replaced by inline modes but never removed. Additionally, **shared utilities for text input handling and modal rendering exist but are inconsistently adopted**, creating UX inconsistencies where keyboard shortcuts work in some input fields but not others. The error handling contract in `Manager` is mixed — some methods return `Result`, others panic — creating unpredictable crash risk.

## Finding Counts

| Severity | Count |
|----------|-------|
| Critical | 1     |
| Important| 13    |
| Minor    | 20    |
| **Total**| **34** |

## Findings

### Critical

#### 1. SessionListScreen is a 1,593-line god object with too many responsibilities
- **Category:** Complexity, Extensibility
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:150`
- **Details:** This single struct handles session list display, row building, fuzzy filtering, sorting, cursor navigation, session switching, inline confirmation, inline renaming, inline review input, inline new-tab input, inline action menu, rendering for all inline modes, footer rendering, and mouse event handling. The `InlineMode` enum embeds 5+ mini-screens inside one struct, each with its own rendering logic (lines 1314-1481) and key handler. The `build_rows` method (~190 lines) also performs I/O side effects (tmux pane capture, window renaming) during what appears to be a data-building method. Adding any new inline interaction requires touching at minimum five separate locations within this file.
- **Suggestion:** Extract each `InlineMode` variant into its own struct implementing a shared `InlineBehavior` trait with `handle_key` and `render` methods. Separate the side-effecting logic (pane capture, window renaming) from pure row data construction.

### Important

#### 2. Three dead screen modules: ActionMenuScreen, ReviewScreen, NewTabScreen
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/action_menu.rs:19`, `/Users/tom/dev/trellis/src/tui/review.rs:27`, `/Users/tom/dev/trellis/src/tui/new_tab.rs:24`
- **Details:** All three screen structs are never constructed anywhere in the codebase. Their `Screen` enum variants exist and are matched in `behavior()`/`behavior_mut()`, but no code ever creates them. All three were replaced by `InlineMode` variants in `session_list.rs`. These are entire dead modules adding ~500 lines of unmaintained code.
- **Suggestion:** Remove all three modules, their `mod` declarations, and their `Screen` enum variants.

#### 3. Mixed error handling in Manager — some methods return Result, others panic
- **Category:** Inconsistency, Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/manager.rs:164`, `/Users/tom/dev/trellis/src/manager.rs:233`
- **Details:** `rename_session`, `set_base_branch`, `delete_session`, `cleanup_worktree`, `add_tab`, and `checkout_and_review` all return `Result<_, String>`. But `create_session` (line 187) panics on failure, and `adopt_session` returns a bare `Session` with no error path despite calling `.unwrap()` internally. The `adopt_session.rs` TUI screen wraps the call in `catch_unwind` to mask panics — the only place in the codebase that does this. Callers must know which methods are crash-safe and which are not.
- **Suggestion:** Make `create_session` and `adopt_session` return `Result<_, String>` to match the convention established by the majority of `Manager` methods. Remove the `catch_unwind` wrapper.

#### 4. Duplicated text input handling — edit_branch.rs and settings.rs reimplement shared logic
- **Category:** DRY Violations, Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/tui/edit_branch.rs:145`, `/Users/tom/dev/trellis/src/tui/settings.rs:64`
- **Details:** The `rename.rs` module provides a shared `input_handle_key` function used by 5 other screens. However, `edit_branch.rs` and `settings.rs` both reimplement the same character input, backspace, delete, left/right arrow, home/end logic. Critically, the duplicated versions lack Ctrl+U (clear line) and Ctrl+W (delete word) support that the shared version has, creating a UX inconsistency where keyboard shortcuts work in some input fields but not others.
- **Suggestion:** Refactor both screens to delegate to the shared `input_handle_key` from `rename.rs`.

#### 5. Duplicated cursor rendering in edit_branch.rs and settings.rs
- **Category:** DRY Violations, Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/tui/edit_branch.rs:248`, `/Users/tom/dev/trellis/src/tui/settings.rs:215`
- **Details:** Both screens manually compute cursor character length, before/cursor/after string slicing, and build cursor-highlighted `Line` spans. This exact logic is already encapsulated in `render_input()` from `rename.rs`, which five other modal screens use. Changes to cursor rendering behavior would need to be applied in three places.
- **Suggestion:** Use the shared `render_input` function from `rename.rs`.

#### 6. Duplicated modal box rendering in confirm.rs, help.rs, edit_branch.rs, settings.rs
- **Category:** DRY Violations, Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/tui/confirm.rs:28`, `/Users/tom/dev/trellis/src/tui/help.rs:103`, `/Users/tom/dev/trellis/src/tui/edit_branch.rs:196`, `/Users/tom/dev/trellis/src/tui/settings.rs:141`
- **Details:** Four screens manually construct the same center-a-box-on-screen pattern using `Layout::vertical/horizontal` and `Fill/Length` constraints. This exact pattern is already extracted as `render_modal_box` in `rename.rs`, which four other screens use. Each copy is ~15 lines of identical layout and block styling code.
- **Suggestion:** Migrate all four screens to use the existing `render_modal_box` function.

#### 7. Inline rename cursor uses char index but input_handle_key uses byte index
- **Category:** Confusing Code (potential bug)
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:1372-1374`
- **Details:** The `input_handle_key` function in `rename.rs` maintains `cursor` as a byte offset into a UTF-8 string (`*cursor += c.len_utf8()`). However, the inline rename rendering code uses `chars().take(*cursor_pos)` and `chars().nth(*cursor_pos)`, treating it as a character index. For ASCII text this works, but for any multi-byte input (emoji, accented characters) this would display the cursor in the wrong position or panic.
- **Suggestion:** Use byte slicing in the inline rename rendering (as the modal `render_input` function does correctly), or use the shared `render_input` function directly.

#### 8. `fetch_and_pull` silently discards errors from both fetch and pull
- **Category:** Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/git.rs:193-196`
- **Details:** `fetch_and_pull` runs both `git fetch origin` and `git pull origin <branch>` but discards the return values entirely. If either fails (network error, merge conflict, auth failure), the caller proceeds to create a worktree from potentially stale state with no indication anything went wrong.
- **Suggestion:** Return a `Result` type and either handle or surface the error to the user.

#### 9. `is_version_number` used as proxy for "is this a Claude process" with no explanation
- **Category:** Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:123-127`
- **Details:** The function `is_version_number` checks if a string matches "1.2.3" pattern, then is used to determine `is_claude`. This relies on the non-obvious fact that Claude's `pane_current_command` reports as a version number. There is no comment explaining this indirection.
- **Suggestion:** Add a comment explaining why version numbers indicate a Claude process, or rename to `is_claude_command`.

#### 10. `claude_session.rs` name is misleading — it handles pane analysis, not session management
- **Category:** Naming & Organization
- **Location:** `/Users/tom/dev/trellis/src/claude_session.rs:1`
- **Details:** This file contains four distinct capabilities: session ID lookup from PIDs, message extraction from JSONL, message summarization, and tmux pane output classification. The name "claude_session" suggests session management, but it is really a grab bag of Claude-pane introspection utilities.
- **Suggestion:** Rename to `claude_pane.rs` or `claude_introspect.rs`.

#### 11. Duplicated utc_now timestamp function across two modules
- **Category:** DRY Violations, Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/manager.rs:14`, `/Users/tom/dev/trellis/src/db.rs:302`
- **Details:** Two independent functions produce the same RFC3339 UTC timestamp — `utc_now()` in manager.rs and `utc_now_iso()` in db.rs. Both import `time::OffsetDateTime` and call the exact same formatting logic. Different names add confusion.
- **Suggestion:** Extract a single shared function in `utils.rs` and call it from both modules.

#### 12. Duplicated wizard pattern between NewSessionScreen and AdoptSessionScreen
- **Category:** Complexity, DRY Violations
- **Location:** `/Users/tom/dev/trellis/src/tui/new_session.rs:1`, `/Users/tom/dev/trellis/src/tui/adopt_session.rs:1`
- **Details:** Both screens (687 and 543 lines respectively) share nearly identical patterns: Step enum, SelectedRepo struct, filter input state, repo path entry mode, list navigation, filter application, list item building, modal box rendering. The `finish_add_repo` and `enter_repo_path_mode` methods are nearly identical.
- **Suggestion:** Extract the shared wizard infrastructure into a reusable component that both screens compose.

#### 13. `is_child_row` checks for "wt:" prefix that no code ever creates
- **Category:** Confusing Code (likely dead logic)
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:220-222`
- **Details:** `is_child_row` checks for `key.starts_with("wt:")` in addition to `"win:"`, but `build_rows` only ever creates `RowKind::Window` child rows which produce `"win:"` keys. No code path creates `"wt:"` prefixed keys.
- **Suggestion:** Remove the `"wt:"` check if vestigial.

#### 14. Screen enum requires parallel updates in two match arms for every new screen
- **Category:** Extensibility
- **Location:** `/Users/tom/dev/trellis/src/tui/mod.rs:86`
- **Details:** The `Screen` enum has 14 variants and requires `behavior()` and `behavior_mut()` methods that each contain an identical 14-arm match statement. Every new screen requires updating both methods in lockstep.
- **Suggestion:** Replace the `Screen` enum with `Box<dyn ScreenBehavior>` in the screen stack, or use a macro to generate both match arms from a single definition.

### Minor

#### 15. BackgroundResult enum variants are never constructed
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/mod.rs:45-48`
- **Details:** `BackgroundResult::StaleWorktrees` and `BackgroundResult::CheckoutComplete` are defined but never constructed. The `bg_tx`/`bg_rx` fields on `App` are stored but never used. The cleanup screen uses its own internal channel instead.
- **Suggestion:** Remove the `BackgroundResult` enum and `bg_tx`/`bg_rx` fields.

#### 16. `Manager::get_stale_worktrees` is never called
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/manager.rs:459`
- **Details:** Marked `#[allow(dead_code)]` and never called. `CleanupScreen` duplicates the same logic inline.
- **Suggestion:** Either remove this method or refactor `CleanupScreen` to use it.

#### 17. `tmux::get_pane_pid` is never called
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tmux.rs:281`
- **Details:** `pub` and marked `#[allow(dead_code)]`, but nothing calls it.
- **Suggestion:** Remove the function.

#### 18. `App::push` is never called externally
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/mod.rs:147`
- **Details:** Marked `#[allow(dead_code)]` and never called from anywhere.
- **Suggestion:** Remove the method.

#### 19. Dead match arms for "rename" and "rename-tab" in handle_action_picked
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:919`, `/Users/tom/dev/trellis/src/tui/session_list.rs:947`
- **Details:** These actions are never added to the menu items. Comment confirms: `// "rename" removed -- use 'r' directly`.
- **Suggestion:** Remove the dead match arms.

#### 20. Stale `#[allow(dead_code)]` on `current_session`
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:487`
- **Details:** The method is actively used. The annotation and comment are stale.
- **Suggestion:** Remove the `#[allow(dead_code)]` annotation.

#### 21. Stale `#[allow(unused_mut)]` on `mgr` in main()
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/main.rs:23`
- **Details:** `mgr` does not need to be mutable. The annotation suppresses a warning about unnecessary `mut`.
- **Suggestion:** Remove both the annotation and the `mut` keyword.

#### 22. Unused `manager` parameter in `go_to_branch_step`
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/tui/new_session.rs:163`
- **Details:** Parameter is explicitly discarded with `let _ = manager; // may be used later`.
- **Suggestion:** Remove the parameter if not needed imminently.

#### 23. `conversation_index::index_path` is pub but only used internally
- **Category:** Dead Code
- **Location:** `/Users/tom/dev/trellis/src/conversation_index.rs:9`
- **Details:** Only called from `parse_index` within the same module.
- **Suggestion:** Reduce visibility to private.

#### 24. `_repo_name` and `_branch_dir` use underscore prefix but are actively used
- **Category:** Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/manager.rs:683-696`
- **Details:** Leading underscore in Rust conventionally signals "unused", but these variables are actively used to construct `wt_path` and in the `add_worktree` call.
- **Suggestion:** Remove the leading underscores.

#### 25. `fuzzy_match` scoring convention is undocumented — lower scores are better
- **Category:** Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/fuzzy.rs:1-31`
- **Details:** The scoring inversion (lower = better) is not documented. An exact match of "abc" in "abc" returns `Some(-2)`, which is non-intuitive without context.
- **Suggestion:** Add a doc comment explaining the scoring convention.

#### 26. `try_rename_claude_window` has hidden side effects inside `build_rows`
- **Category:** Confusing Code
- **Location:** `/Users/tom/dev/trellis/src/tui/session_list.rs:108-121`
- **Details:** Called inside `build_rows`, which readers expect to be a pure data-building method. It unconditionally renames a tmux window (system side effect) and mutates the window reference.
- **Suggestion:** Move rename logic out of `build_rows` into its own lifecycle step.

#### 27. Duplicated repo sort-by-most-recent-session logic
- **Category:** DRY Violations
- **Location:** `/Users/tom/dev/trellis/src/tui/review.rs:30`, `/Users/tom/dev/trellis/src/tui/session_list.rs:749`
- **Details:** Both build a `HashMap<i64, String>` mapping repo_id to most recent `last_selected_at`, then sort repos by that map. Identical logic in two places.
- **Suggestion:** Extract into a shared helper. (Note: `review.rs` itself is dead code per finding #2, so removing it would also resolve this.)

#### 28. Duplicated footer rendering pattern
- **Category:** DRY Violations
- **Location:** `/Users/tom/dev/trellis/src/tui/cleanup.rs:222`, `/Users/tom/dev/trellis/src/tui/history.rs:298`, `/Users/tom/dev/trellis/src/tui/session_list.rs:996`
- **Details:** Three screens use the identical pattern for rendering key-description footer bars with styled spans and separators.
- **Suggestion:** Extract a shared `render_footer_bindings` utility function.

#### 29. Duplicated SelectedRepo struct
- **Category:** DRY Violations
- **Location:** `/Users/tom/dev/trellis/src/tui/new_session.rs:29`, `/Users/tom/dev/trellis/src/tui/adopt_session.rs:29`
- **Details:** Both files define a private `SelectedRepo` struct with identical fields for the same purpose.
- **Suggestion:** Extract to a shared location.

#### 30. Inconsistent list navigation — wrapping vs clamping
- **Category:** Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/tui/new_session.rs:374`, `/Users/tom/dev/trellis/src/tui/edit_branch.rs:322`
- **Details:** `new_session.rs` and `adopt_session.rs` use `rem_euclid` for wrapping navigation; `edit_branch.rs`, `action_menu.rs`, `history.rs`, and `cleanup.rs` use clamping. The same type of filterable list picker behaves differently depending on which screen you are on.
- **Suggestion:** Pick one style and apply consistently.

#### 31. TUI screens bypass Manager to access db directly
- **Category:** Naming & Organization
- **Location:** `/Users/tom/dev/trellis/src/tui/settings.rs:7`
- **Details:** `settings.rs` and `cleanup.rs` import `crate::db` directly and reach through `manager.conn` to call db functions, bypassing the intended `Manager` orchestration layer.
- **Suggestion:** Add missing convenience methods to `Manager`.

#### 32. `switch.rs` test named `json_format_matches_python` references legacy Python implementation
- **Category:** Naming & Organization
- **Location:** `/Users/tom/dev/trellis/src/switch.rs:80`
- **Details:** The test name references a Python implementation that no longer exists in the active codebase.
- **Suggestion:** Rename to `json_serialization_format` or similar.

#### 33. `archive/` directory contains dead Python predecessor
- **Category:** Naming & Organization
- **Location:** `/Users/tom/dev/trellis/archive/`
- **Details:** A complete Python project with `.venv/` and `.pytest_cache/` that is not referenced by the Rust codebase.
- **Suggestion:** Consider removing from the repo (it remains in git history).

#### 34. Inconsistent filter case handling — history.rs normalizes on input, session_list.rs normalizes on comparison
- **Category:** Inconsistency
- **Location:** `/Users/tom/dev/trellis/src/tui/history.rs:225`, `/Users/tom/dev/trellis/src/tui/session_list.rs:641`
- **Details:** Both produce case-insensitive results but via different mechanisms. If the filter rendering ever showed raw input, they would display differently.
- **Suggestion:** Adopt one consistent approach across all filter inputs.

## Category Summaries

### Dead Code
Three entire screen modules (`ActionMenuScreen`, `ReviewScreen`, `NewTabScreen`) are dead — replaced by inline modes but never removed. Several `#[allow(dead_code)]` annotations suppress warnings on genuinely dead code, and a few suppress warnings on code that is actually used (stale annotations). The `BackgroundResult` infrastructure is scaffolded but unwired.

### Complexity
The `SessionListScreen` is a 1,593-line god object that is the clear focal point of complexity. It embeds five inline interaction modes, each with separate key handling and rendering logic. The `build_rows` method mixes data construction with I/O side effects. The `NewSessionScreen` and `AdoptSessionScreen` share substantial structural similarity that could be extracted.

### DRY Violations
The most impactful duplication is text input handling and cursor rendering — shared utilities exist in `rename.rs` (`input_handle_key`, `render_input`, `render_modal_box`) but `edit_branch.rs` and `settings.rs` reimplement them, creating both code duplication and UX inconsistency. The `utc_now` timestamp helper is duplicated across `manager.rs` and `db.rs`. Footer rendering, repo sorting, and `SelectedRepo` structs are also duplicated.

### Confusing Code
The most significant finding is the byte-index vs char-index mismatch in inline rename rendering, which is a latent bug for non-ASCII input. `fetch_and_pull` silently discards errors. The `is_version_number` heuristic for Claude detection is unexplained. `catch_unwind` is used in one place to mask panics rather than fixing the underlying error contract.

### Extensibility
The `Screen` enum requires parallel updates in two match arms for every new screen. The `SessionListScreen`'s `InlineMode` enum is a growing conditional chain. Hard-coded values (default layout, subsystem directories, Claude session path) limit configurability. The action menu items and handlers must be kept in sync manually.

### Inconsistent Patterns
The most impactful inconsistency is mixed error handling in `Manager` — some methods return `Result`, others panic. Text input handling is inconsistent because shared utilities exist but aren't universally adopted. List navigation behavior (wrapping vs clamping) varies across screens. Filter case normalization strategy differs between history and session list.

### Naming & Organization
`claude_session.rs` is misleadingly named for its actual contents. Some TUI screens bypass the `Manager` layer to access the database directly. The `archive/` directory contains a dead Python predecessor. A test name references the legacy Python implementation.
