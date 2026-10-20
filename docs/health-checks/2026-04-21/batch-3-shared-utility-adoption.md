# Batch 3: Shared Utility Adoption

**Date:** 2026-04-21
**Classification:** mechanical
**Commit:** `d737f7b`

## Findings Resolved
- #4: Replaced duplicated text input handling in edit_branch.rs and settings.rs with shared `input_handle_key` from rename.rs
- #5: Replaced duplicated cursor rendering in edit_branch.rs and settings.rs with shared `render_input` from rename.rs
- #6: Replaced duplicated modal box layout in edit_branch.rs and settings.rs with shared `render_modal_box` from rename.rs
- #7: Fixed byte-index vs char-index bug in inline rename, review, and new-tab rendering — all three inline modes now use byte slicing consistent with `input_handle_key`
- #11: Extracted shared `utc_now()` function to utils.rs, replaced duplicate copies in manager.rs and db.rs

## Verification
```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

## Notes
Finding #6 originally targeted confirm.rs and help.rs as well, but those screens use intentionally different modal styling (no titled block, different border approach) that doesn't map cleanly to `render_modal_box`. Only edit_branch.rs and settings.rs were converted since they matched the shared utility's style exactly. The edit_branch.rs filter input was split into a label area and input area to accommodate `render_input` which doesn't handle inline prefixes.
