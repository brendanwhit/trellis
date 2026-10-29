# Remote repos in the New-session wizard

## Goal

Let the user start a session on a GitHub repo they have not cloned yet (e.g. `faradayio/aviary`) straight from the New-session wizard. PickRepo becomes one list of local repos plus uncloned GitHub repos; picking a remote one clones it, then the wizard continues to PickBranch as usual.

## Behavior

- **One list.** Remote repos appear in the PickRepo list, after all local repos, tagged `(remote)`. The existing fuzzy filter covers both. The trailing "add repo" item is unchanged. When a name collides with another entry, the label is annotated the same way local labels are: remote labels fall back to `owner/name (remote)`.
- **Fetch.** On open, a `Job` (`tui/job.rs`) lists repos for the authenticated user (`gh api user --jq .login`) and every org (`gh api user/orgs --paginate --jq '.[].login'`) via `gh repo list <owner> --limit 1000 --no-archived --json nameWithOwner`. The list renders immediately with local repos; remote entries merge in when the job lands, preserving the filter text and highlighted entry. The job's spinner shows while pending.
- **Failure.** `gh` missing, logged out or offline degrades to the local-only list with a brief error line. It never blocks the wizard.
- **De-dupe.** A remote repo is hidden when a local repo's `origin` URL resolves to the same `owner/name` (ssh `git@github.com:o/n.git`, https, with or without `.git`; case-insensitive). An unparseable origin matches nothing. Local origins are read on the worker, not the UI thread.
- **Clone.** Selecting a remote entry runs `gh repo clone <owner/name> <dest>` on a `Job`, with `<dest>` = `<chosen repos dir>/<name>`. When more than one repos dir is configured, picking a remote repo first opens a destination step: a fuzzy-filterable list of `repos_dirs()` with the first preselected (Enter accepts it); Esc returns to PickRepo without cloning. With exactly one repos dir the step is skipped. Only configured repos dirs are offered (no free-form path). If `<dest>` is already a git repo it is used as is. Input is ignored while the clone runs. On success the wizard proceeds exactly as for a picked local repo (selected repo set, PickBranch); the repo is registered in the DB lazily by `create_session`, as for any local pick. On failure the error is shown and the wizard stays on PickRepo.
- **No cache.** Remote results live only as long as the wizard screen.

## Out of scope

Remote branches in PickBranch, PR numbers, the `R` review shortcut, any review prompt, on-disk caching, and other repo pickers (adopt, cleanup).
