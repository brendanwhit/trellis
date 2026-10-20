# Batch 6: Wizard Deduplication

**Date:** 2026-04-21
**Classification:** architectural
**Commit:** `800f816`

## Findings Resolved
- #12: Extracted `wizard_validate_repo_path` and `wizard_load_branches` as shared helpers in tui/mod.rs. Both NewSessionScreen and AdoptSessionScreen now delegate to these instead of duplicating the path validation (~25 lines each) and branch loading (~15 lines each) logic.

## Verification
```
running 40 tests
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

## Notes
The `crate::git` import in adopt_session.rs became unused after extracting `wizard_load_branches` (which calls `git::list_branches` internally) and was removed. The remaining structural similarities between the two screens (render layout, handle_event structure, list navigation) are coincidental similarity — both are wizard screens with the same UX pattern but they evolve independently and a shared trait would be more complex than the duplication.
