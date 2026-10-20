# Batch 7: Screen Enum Simplification

**Date:** 2026-04-21
**Classification:** architectural
**Commit:** `566e9f7`

## Findings Resolved
- #14: Replaced the `Screen` enum (11 variants with two 11-arm match blocks) with `Box<dyn ScreenBehavior>` in the screen stack. All push sites now use `Box::new(screen)` instead of `Screen::Variant(screen)`. Adding a new screen no longer requires any changes to mod.rs.

## Verification
```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

## Notes
Removing the Screen enum also eliminated the dead `RenameSession` and `RenameWindow` variants that were exposed by Batch 1 but not in scope for that batch. The compiler now correctly reports the `RenameSessionScreen` and `RenameWindowScreen` structs themselves as dead code — they can be cleaned up in a future pass (the shared utilities in rename.rs — `input_handle_key`, `render_input`, `render_modal_box`, `render_footer_bindings` — are still actively used and must be kept).
