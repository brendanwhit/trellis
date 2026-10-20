# Batch 4: Minor DRY and Naming Fixes

**Date:** 2026-04-21
**Classification:** mechanical
**Commit:** `4ee68c2`

## Findings Resolved
- #9: Renamed `is_version_number` to `looks_like_claude_command` with doc comment explaining the Claude detection heuristic
- #10: Renamed `claude_session.rs` to `claude_pane.rs` and updated all imports
- #25: Added doc comment to `fuzzy_match` explaining lower-is-better scoring convention
- #28: Extracted shared `render_footer_bindings` utility in rename.rs, adopted by cleanup.rs, history.rs, and session_list.rs
- #29: Extracted shared `SelectedRepo` struct to tui/mod.rs, adopted by new_session.rs and adopt_session.rs
- #31: Added `get_all_config`, `set_config`, `get_all_worktrees` methods to Manager; updated settings.rs and cleanup.rs to use them instead of reaching into `manager.conn` directly

## Verification
```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

## Notes
None.
