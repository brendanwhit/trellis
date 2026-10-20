# Batch 1: Dead Code Removal

**Date:** 2026-04-21
**Classification:** mechanical
**Commit:** `86114d6`

## Findings Resolved
- #2: Deleted action_menu.rs, review.rs, new_tab.rs and removed their Screen enum variants and match arms
- #13: Removed vestigial "wt:" prefix check from is_child_row, keeping only "win:"
- #15: Removed BackgroundResult enum, bg_tx/bg_rx fields from App, and on_background_result from ScreenBehavior trait
- #16: Removed unused Manager::get_stale_worktrees method
- #17: Removed unused tmux::get_pane_pid function
- #18: Removed unused App::push method
- #19: Removed dead "rename" and "rename-tab" match arms from handle_action_picked, plus now-unused row_key variable

## Verification
```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

## Notes
Removing the dead action menu paths exposed that RenameSessionScreen and RenameWindowScreen (in rename.rs) are also now dead — they were only constructed from the removed "rename" and "rename-tab" action menu paths. Inline rename has fully replaced them. These are new dead code findings not in the original report; they should be addressed in a future batch.
