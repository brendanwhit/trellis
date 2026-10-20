# Resolution Plan

**Verification command:** `cargo test`
**Date:** 2026-04-21

## Won't Fix

### #33: archive/ contains dead Python predecessor
Historical artifact in a personal project; removing it is a git history preference, not a code health issue.

### #30: Inconsistent list navigation (wrapping vs clamping)
Both behaviors are reasonable UX choices per-context; clamping for short menus and wrapping for long lists is arguably intentional.

### #34: Inconsistent filter case handling
Both approaches produce identical case-insensitive results; the divergence only matters in a hypothetical future display change.

## Batches

### Batch 1: Dead code removal (mechanical, foundation)
Unblocks: #1
- #2: Remove three dead screen modules (ActionMenuScreen, ReviewScreen, NewTabScreen)
- #15: Remove unused BackgroundResult enum and bg_tx/bg_rx fields
- #16: Remove unused Manager::get_stale_worktrees
- #17: Remove unused tmux::get_pane_pid
- #18: Remove unused App::push
- #19: Remove dead "rename"/"rename-tab" match arms in handle_action_picked
- #13: Remove vestigial "wt:" check in is_child_row

### Batch 2: Stale annotations and naming cleanup (mechanical)
- #20: Remove stale #[allow(dead_code)] on current_session
- #21: Remove stale #[allow(unused_mut)] and mut on mgr in main()
- #22: Remove unused manager parameter in go_to_branch_step
- #23: Make conversation_index::index_path private
- #24: Remove leading underscores from _repo_name and _branch_dir
- #32: Rename json_format_matches_python test

### Batch 3: Shared utility adoption (mechanical)
- #4: Refactor edit_branch.rs to use shared input_handle_key
- #5: Refactor edit_branch.rs and settings.rs to use shared render_input
- #6: Refactor confirm.rs, help.rs, edit_branch.rs, settings.rs to use render_modal_box
- #7: Fix byte-index vs char-index bug in inline rename rendering
- #11: Extract shared utc_now to utils.rs

### Batch 4: Minor DRY and naming fixes (mechanical)
- #25: Add doc comment to fuzzy_match explaining scoring convention
- #9: Add comment or rename is_version_number to explain Claude detection
- #10: Rename claude_session.rs to claude_pane.rs
- #28: Extract shared render_footer_bindings utility
- #29: Extract shared SelectedRepo struct
- #31: Add missing convenience methods to Manager so TUI screens don't bypass it

### Batch 5: Manager error handling consistency (architectural)
- #3: Make create_session and adopt_session return Result, remove catch_unwind
- #8: Make fetch_and_pull return Result and surface errors

### Batch 6: Wizard deduplication (architectural)
- #12: Extract shared wizard infrastructure from NewSessionScreen and AdoptSessionScreen

### Batch 7: Screen enum simplification (architectural)
- #14: Replace Screen enum with Box<dyn ScreenBehavior> or macro-generate match arms

### Batch 8: SessionListScreen decomposition (architectural)
- #1: Extract InlineMode variants into separate structs; separate build_rows side effects
- #26: Move try_rename_claude_window out of build_rows into its own lifecycle step
