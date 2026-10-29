# Review a remote branch or PR from the New-session wizard

## Goal

Reviewing someone's branch or PR is just the New-session wizard: pick the repo, pick the remote branch (or type a PR number), and the agent starts in a worktree of that branch. The separate `R` inline review input goes away; `R` becomes a shortcut into the wizard.

## Behavior

- **Unified branch list.** PickBranch lists local branches plus remote-only branches from `origin`, tagged `(remote)` like remote repos in PickRepo. A remote branch whose name matches a local branch is not listed twice. Filtering is the existing `fuzzy_match_multi`; remote entries sort after locals on equal score.
- **Refresh.** Entering the step shows local branches immediately and starts a `Job` that runs `git fetch origin` then `git for-each-ref refs/remotes/origin` (`origin/HEAD` excluded). Results merge into the visible list, keeping filter text and highlight. Failure (offline etc.) leaves the local list with a brief notice.
- **Pick local.** Unchanged.
- **Pick remote-only.** A `Job` runs `git fetch origin <branch>`; on success the wizard continues to PickAgent and the worktree is created at `origin/<branch>` (`create_session` takes an optional start point and skips the default-branch refresh). On failure the error shows and the wizard stays on PickBranch. Input is ignored while the job runs.
- **PR numbers.** When the query is all digits and no listed branch has exactly that name, a `PR #N` row follows the list. Picking it resolves the head branch with `git::get_pr_branch` on a `Job`, fetches it, then proceeds as for a remote-only branch. Resolve or fetch failure stays on PickBranch with the error.
- **New branch.** A typed name matching nothing still creates a new branch off the default branch.
- **No review prompt, no special agent.** PickAgent onward is unchanged.
- **`R` shortcut.** Opens the New wizard at PickRepo with the most recently used repo (latest `last_selected_at` among its sessions) highlighted. The inline review input, its Tab-cycling repo picker, `Manager::checkout_and_review`, `resolve_review_branch` and `find_or_create_session` (used only by review) are removed.

## Out of scope

Remote repo listing/cloning, history, cleanup and adopt screens.
