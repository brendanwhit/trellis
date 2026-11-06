# Create a new GitHub repo from the New-session wizard

## Goal

Start a brand-new project from the New-session wizard: create a private GitHub repo, clone it, and continue into PickBranch so a worktree branch can be started off its default branch.

## Behavior

- **Row.** PickRepo gets a `+ Create new repo...` row next to `+ Add new repo path...`.
- **Owner.** Selecting it looks up the owners on a `Job` (`gh api user` login first, then `gh api user/orgs`; the same lookup the remote repo listing uses). With more than one owner an owner step (fuzzy list, the user's login preselected) follows; with only the login it is skipped.
- **Name.** A text input. The name must be non-empty, match `[A-Za-z0-9._-]`, and not be `.` or `..`; a name that collides with a listed local or remote repo is rejected. Errors show inline and nothing is created.
- **Destination.** The existing clone-destination step (skipped with exactly one repos dir).
- **Create.** One `Job` runs `gh repo create <owner>/<name> --private --add-readme` (fixed visibility, no description/license/gitignore), then the existing clone path (`gh repo clone` into `<dir>/<name>`). The README commit guarantees the default branch exists; no branch-renaming flags are used, so whatever GitHub's default is (normally `main`) is what gets checked out. Input is ignored while it runs.
- **Success.** Proceeds exactly as after a remote clone: PickBranch for the new repo, where a typed name starts a new branch off the default branch.
- **Failure.** The error (name taken, no permission in the org, ...) shows and the wizard stays on the step it was on (name, or destination).
- **Escape.** Owner -> PickRepo; name -> owner (or PickRepo when the owner step was skipped); destination -> name, with the name kept.

## Out of scope

Other visibility options, templates, org repo-creation policy beyond showing gh's error, local-only `git init` repos.
