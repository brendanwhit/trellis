// trellis/src/manager.rs
//! High-level orchestration: ties DB, tmux, and git together.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::db;
use crate::git;
use crate::models::{Repo, Session, SessionInfo, Worktree};
use crate::tmux;

use crate::utils::utc_now;

/// Default tmux layout: (window_name, optional command to send).
const DEFAULT_LAYOUT: &[(&str, Option<&str>)] = &[
    ("claude", Some("claude")),
    ("shell", None),
];

/// Create a tmux session and set up windows according to the default layout.
pub fn apply_layout(session_name: &str, working_dir: &str) {
    // Create the session
    if let Err(_) = tmux::new_session(session_name, working_dir) {
        return;
    }

    for (i, &(name, command)) in DEFAULT_LAYOUT.iter().enumerate() {
        if i == 0 {
            let _ = tmux::rename_window(session_name, 1, name);
        } else {
            let _ = tmux::new_window(session_name, name, Some(working_dir));
        }
        if let Some(cmd) = command {
            let target = format!("{}:{}", session_name, name);
            tmux::send_keys(&target, &[cmd, "Enter"]);
        }
    }

    // Focus the first window
    let _ = tmux::select_window(session_name, 1);
}

/// Find or create the on-disk worktree for (repo, branch).
///
/// Git's `worktree list` is the source of truth. If a worktree for `branch`
/// already exists, returns its recorded path (even if outside `worktrees_dir`).
/// If git considers a worktree prunable (dir gone), prunes and reattaches the
/// worktree to the still-existing branch. Otherwise creates fresh at the
/// trellis-convention path, branching from `start_point`.
///
/// `start_point` is only consulted when creating a NEW local branch. Callers
/// pass what they want the branch rooted at (e.g. `repo.default_branch`,
/// `session.base_branch`, or `format!("origin/{}", branch)` for PR review).
pub fn resolve_or_create_worktree(
    repo_path: &str,
    repo_name: &str,
    branch: &str,
    worktrees_dir: &str,
    start_point: &str,
) -> Result<String, String> {
    let mut needs_prune = false;
    let existing = git::list_worktrees(repo_path).map_err(|e| e.to_string())?;
    for wt in &existing {
        if wt.branch.as_deref() == Some(branch) {
            if !Path::new(&wt.path).exists() {
                needs_prune = true;
                break;
            }
            return Ok(wt.path.clone());
        }
    }

    if needs_prune {
        let _ = git::prune_worktrees(repo_path);
    }

    let wt_path = Path::new(worktrees_dir)
        .join(repo_name)
        .join(branch.replace('/', "-"))
        .to_string_lossy()
        .to_string();

    // Pick between `git worktree add -b <branch> <path> <start>` (new branch)
    // and `git worktree add <path> <branch>` (existing branch). After a prune,
    // the local branch ref still exists, so the existing-branch path is taken.
    let branches = git::list_branches(repo_path).map_err(|e| e.to_string())?;
    if branches.iter().any(|b| b == branch) {
        git::add_worktree_existing_branch(repo_path, &wt_path, branch)
            .map_err(|e| format!("Failed to attach worktree: {}", e))?;
    } else {
        git::create_worktree(repo_path, &wt_path, branch, start_point)
            .map_err(|e| format!("Failed to create worktree: {}", e))?;
    }
    Ok(wt_path)
}

/// Detect subsystem directories in a monorepo (e.g. workers/model_train, src/sojourner).
pub fn detect_subsystems(repo_path: &str) -> Vec<String> {
    let root = Path::new(repo_path);
    let mut subsystems = Vec::new();
    for parent_name in &["workers", "src", "libs", "pods"] {
        let parent = root.join(parent_name);
        if parent.is_dir() {
            let mut children: Vec<_> = match std::fs::read_dir(&parent) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().is_dir()
                            && !e.file_name().to_string_lossy().starts_with('.')
                    })
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect(),
                Err(_) => continue,
            };
            children.sort();
            for child in children {
                subsystems.push(format!("{}/{}", parent_name, child));
            }
        }
    }
    subsystems
}

pub struct Manager {
    pub conn: Connection,
}

impl Manager {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    // ------------------------------------------------------------------
    // Properties
    // ------------------------------------------------------------------

    pub fn repos_dir(&self) -> PathBuf {
        db::get_config(&self.conn, "repos_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("dev"))
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        db::get_config(&self.conn, "worktrees_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join("dev").join("worktrees"))
    }

    pub fn worktree_path(&self, repo_name: &str, branch: &str) -> String {
        self.worktrees_dir()
            .join(repo_name)
            .join(branch.replace("/", "-"))
            .to_string_lossy()
            .to_string()
    }

    fn find_or_create_worktree(
        &self,
        repo: &Repo,
        branch: &str,
        start_point: &str,
    ) -> Result<String, String> {
        resolve_or_create_worktree(
            &repo.path,
            &repo.name,
            branch,
            &self.worktrees_dir().to_string_lossy(),
            start_point,
        )
    }

    fn find_or_create_session(&self, repo: &Repo, branch: &str, wt_path: &str) -> Result<Session, String> {
        let session_name = tmux::sanitize_session_name(branch);

        // Existing DB row?
        if let Some(existing) = self.get_session_by_name(&session_name) {
            if existing.repo_id == repo.id.unwrap() && tmux::session_exists(&session_name) {
                return Ok(existing);
            }
            // Stale row — drop it.
            if let Some(id) = existing.id {
                db::delete_session(&self.conn, id);
            }
        }

        // Create fresh: DB row, tmux session via apply_layout, optional worktree DB row.
        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );
        apply_layout(&session_name, wt_path);

        if wt_path != repo.path && db::get_worktree_by_path(&self.conn, wt_path).is_none() {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: wt_path.to_string(),
                    branch: branch.to_string(),
                    session_id: session.id,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
        }

        Ok(session)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn get_repo_by_id(&self, repo_id: i64) -> Option<Repo> {
        db::get_repos(&self.conn)
            .into_iter()
            .find(|r| r.id == Some(repo_id))
    }

    fn get_repo_by_path(&self, path: &str) -> Option<Repo> {
        db::get_repos(&self.conn)
            .into_iter()
            .find(|r| r.path == path)
    }

    fn get_session_by_id(&self, session_id: i64) -> Option<Session> {
        db::get_sessions(&self.conn)
            .into_iter()
            .find(|s| s.id == Some(session_id))
    }

    fn get_worktree_by_id(&self, worktree_id: i64) -> Option<Worktree> {
        db::get_worktrees(&self.conn)
            .into_iter()
            .find(|wt| wt.id == Some(worktree_id))
    }

    fn get_or_create_repo(&self, repo_path: &str) -> Repo {
        if let Some(repo) = self.get_repo_by_path(repo_path) {
            return repo;
        }
        let default_branch = git::detect_default_branch(repo_path).unwrap_or_else(|_| "main".into());
        let name = Path::new(repo_path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        db::add_repo(
            &self.conn,
            &Repo {
                id: None,
                path: repo_path.to_string(),
                name,
                default_branch,
            },
        )
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Register repo if needed, create or reuse worktree, create or reuse DB
    /// session, create tmux session. If a live session for (repo, base_branch)
    /// already exists, routes to it (user-typed `session_name` ignored).
    pub fn create_session(
        &self,
        repo_path: &str,
        base_branch: &str,
        session_name: &str,
        subdirectory: Option<&str>,
    ) -> Result<Session, String> {
        let repo = self.get_or_create_repo(repo_path);

        // Spec: "n on branch X when X already has a session — route to the
        // existing session." Consult by branch BEFORE honoring the user-typed
        // name. The wizard's auto_session_name disambiguates to "feat-x-2",
        // "feat-x-3", etc. when a session for the branch exists; bypass that.
        let by_branch: Vec<Session> = db::get_sessions(&self.conn)
            .into_iter()
            .filter(|s| s.repo_id == repo.id.unwrap() && s.base_branch == base_branch)
            .collect();
        for s in by_branch {
            if tmux::session_exists(&s.name) {
                return Ok(s);
            }
            // Dead tmux session — drop stale row and continue.
            if let Some(id) = s.id {
                db::delete_session(&self.conn, id);
            }
        }

        // No live session for this branch. Resolve the worktree.
        let wt_path = if base_branch == repo.default_branch {
            repo.path.clone()
        } else {
            // Fetch latest before resolving worktree (non-fatal — stale is acceptable).
            let _ = git::fetch_and_pull(repo_path, &repo.default_branch);
            // New branches root at the repo's default branch.
            self.find_or_create_worktree(&repo, base_branch, &repo.default_branch)?
        };

        // Honor the user-typed session name (or fall back to sanitized branch).
        let chosen_name = if session_name.is_empty() {
            tmux::sanitize_session_name(base_branch)
        } else {
            tmux::sanitize_session_name(session_name)
        };

        let effective_dir = match subdirectory {
            Some(sub) => Path::new(&wt_path).join(sub).to_string_lossy().to_string(),
            None => wt_path.clone(),
        };

        let session = db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: chosen_name.clone(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        );
        apply_layout(&chosen_name, &effective_dir);

        if wt_path != repo.path && db::get_worktree_by_path(&self.conn, &wt_path).is_none() {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: wt_path.clone(),
                    branch: base_branch.to_string(),
                    session_id: session.id,
                    tmux_window: None,
                    created_at: utc_now(),
                },
            );
        }

        Ok(session)
    }

    /// Adopt an existing tmux session into trellis's management.
    pub fn adopt_session(
        &self,
        session_name: &str,
        repo_path: &str,
        base_branch: &str,
    ) -> Result<Session, String> {
        let repo = self.get_or_create_repo(repo_path);

        Ok(db::add_session(
            &self.conn,
            &Session {
                id: None,
                name: session_name.to_string(),
                repo_id: repo.id.unwrap(),
                base_branch: base_branch.to_string(),
                created_at: utc_now(),
                last_selected_at: None,
            },
        ))
    }

    /// Rename a session in both tmux and the DB.
    pub fn rename_session(&self, session_id: i64, new_name: &str) -> Result<Session, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        // Rename in tmux (if live)
        let _ = tmux::rename_session(&session.name, new_name);

        // Rename in DB
        self.conn
            .execute(
                "UPDATE sessions SET name = ?1 WHERE id = ?2",
                params![new_name, session_id],
            )
            .unwrap();

        Ok(Session {
            name: new_name.to_string(),
            ..session
        })
    }

    /// Update a session's base branch.
    pub fn set_base_branch(&self, session_id: i64, base_branch: &str) -> Result<Session, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        self.conn
            .execute(
                "UPDATE sessions SET base_branch = ?1 WHERE id = ?2",
                params![base_branch, session_id],
            )
            .unwrap();

        Ok(Session {
            base_branch: base_branch.to_string(),
            ..session
        })
    }

    /// Create or reuse worktree + session for a PR number or branch, ready
    /// for review. Returns (session, worktree_path).
    pub fn checkout_and_review(
        &self,
        repo_path: &str,
        pr_or_branch: &str,
    ) -> Result<(Session, String), String> {
        let repo = self.get_or_create_repo(repo_path);

        let branch = if pr_or_branch.chars().all(|c| c.is_ascii_digit()) && !pr_or_branch.is_empty() {
            let pr_num: i64 = pr_or_branch.parse().unwrap();
            git::get_pr_branch(repo_path, pr_num).map_err(|e| format!("Failed to get PR branch: {}", e))?
        } else {
            pr_or_branch.to_string()
        };

        git::fetch_branch(repo_path, &branch);

        // PR review roots the new local branch at the PR's HEAD on the remote,
        // not main — preserves the original `checkout_and_review` semantic.
        let start_point = format!("origin/{}", branch);
        let wt_path = self.find_or_create_worktree(&repo, &branch, &start_point)?;
        let session = self.find_or_create_session(&repo, &branch, &wt_path)?;
        Ok((session, wt_path))
    }

    /// Create or reuse a worktree + tmux window for the given session.
    /// If a worktree for `branch_name` already exists, attaches a new tmux
    /// window to it instead of erroring.
    pub fn add_tab(&self, session_id: i64, branch_name: &str) -> Result<Worktree, String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let repo = self
            .get_repo_by_id(session.repo_id)
            .ok_or_else(|| format!("Repo {} not found", session.repo_id))?;

        // New branches in a tab root at the session's base branch (preserves existing behavior).
        let wt_path = self.find_or_create_worktree(&repo, branch_name, &session.base_branch)?;
        let _ = tmux::new_window(&session.name, branch_name, Some(&wt_path));

        let worktree = if let Some(existing) = db::get_worktree_by_path(&self.conn, &wt_path) {
            existing
        } else {
            db::add_worktree(
                &self.conn,
                &Worktree {
                    id: None,
                    repo_id: repo.id.unwrap(),
                    path: wt_path,
                    branch: branch_name.to_string(),
                    session_id: Some(session_id),
                    tmux_window: None,
                    created_at: utc_now(),
                },
            )
        };

        Ok(worktree)
    }

    /// Kill tmux session, optionally remove worktrees, remove DB session.
    pub fn delete_session(&self, session_id: i64, cleanup_worktrees: bool) -> Result<(), String> {
        let session = self
            .get_session_by_id(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let repo = self.get_repo_by_id(session.repo_id);

        if cleanup_worktrees {
            let worktrees = db::get_worktrees_for_session(&self.conn, session_id);
            for wt in &worktrees {
                if let Some(ref r) = repo {
                    let _ = git::remove_worktree(&r.path, &wt.path);
                }
                db::delete_worktree(&self.conn, wt.id.unwrap());
            }
        } else {
            // Detach worktrees from this session to satisfy FK constraint
            self.conn
                .execute(
                    "UPDATE worktrees SET session_id = NULL WHERE session_id = ?1",
                    params![session_id],
                )
                .unwrap();
        }

        let _ = tmux::kill_session(&session.name);
        db::delete_session(&self.conn, session_id);

        Ok(())
    }

    /// Remove git worktree and delete DB record.
    pub fn cleanup_worktree(&self, worktree_id: i64) -> Result<(), String> {
        let wt = self
            .get_worktree_by_id(worktree_id)
            .ok_or_else(|| format!("Worktree {} not found", worktree_id))?;

        let repo = self
            .get_repo_by_id(wt.repo_id)
            .ok_or_else(|| format!("Repo {} not found", wt.repo_id))?;

        // Try to remove the git worktree; if it fails (e.g. directory already gone),
        // still delete the DB record
        if let Err(e) = git::remove_worktree(&repo.path, &wt.path) {
            // If the directory doesn't exist, just clean up the DB record
            if std::path::Path::new(&wt.path).exists() {
                return Err(format!("Failed to remove worktree: {}", e));
            }
        }
        db::delete_worktree(&self.conn, worktree_id);

        Ok(())
    }

    /// Return DB sessions enriched with live tmux state, plus unmanaged live sessions.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let db_sessions = db::get_sessions(&self.conn);
        let live_list = tmux::list_sessions();
        let live_by_name: HashMap<String, &tmux::TmuxSession> = live_list
            .iter()
            .map(|s| (s.name.clone(), s))
            .collect();
        let mut db_names: HashSet<String> = HashSet::new();

        let mut result: Vec<SessionInfo> = Vec::new();
        for session in &db_sessions {
            db_names.insert(session.name.clone());
            let live = live_by_name.get(&session.name);
            result.push(SessionInfo {
                id: session.id,
                name: session.name.clone(),
                repo_id: Some(session.repo_id),
                base_branch: Some(session.base_branch.clone()),
                created_at: Some(session.created_at.clone()),
                last_selected_at: session.last_selected_at.clone(),
                windows: live.map(|l| l.windows),
                attached: live.map(|l| l.attached).unwrap_or(false),
                live: live.is_some(),
                managed: true,
            });
        }

        // Include live tmux sessions not tracked in the DB
        for ts in &live_list {
            if !db_names.contains(&ts.name) {
                result.push(SessionInfo {
                    id: None,
                    name: ts.name.clone(),
                    repo_id: None,
                    base_branch: None,
                    created_at: None,
                    last_selected_at: None,
                    windows: Some(ts.windows),
                    attached: ts.attached,
                    live: true,
                    managed: false,
                });
            }
        }

        result
    }

    // ------------------------------------------------------------------
    // scan_existing and sub-scans
    // ------------------------------------------------------------------

    fn scan_repos(&self, home_dev: &Path, known_repos: &mut HashMap<String, Repo>) {
        if !home_dev.is_dir() {
            return;
        }
        let entries = match std::fs::read_dir(home_dev) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_dir() || entry.file_name() == "worktrees" {
                continue;
            }
            if path.join(".git").exists() {
                let path_str = path.to_string_lossy().to_string();
                if !known_repos.contains_key(&path_str) {
                    let default_branch =
                        git::detect_default_branch(&path_str).unwrap_or_else(|_| "main".into());
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let repo = db::add_repo(
                        &self.conn,
                        &Repo {
                            id: None,
                            path: path_str.clone(),
                            name,
                            default_branch,
                        },
                    );
                    known_repos.insert(path_str, repo);
                }
            }
        }
    }

    fn scan_worktrees(
        &self,
        home_dev: &Path,
        worktrees_root: &Path,
        known_repos: &mut HashMap<String, Repo>,
        known_worktree_paths: &mut HashSet<String>,
    ) {
        if !worktrees_root.is_dir() {
            return;
        }
        let repo_dirs = match std::fs::read_dir(worktrees_root) {
            Ok(e) => e,
            Err(_) => return,
        };
        for repo_entry in repo_dirs.filter_map(|e| e.ok()) {
            let repo_dir = repo_entry.path();
            if !repo_dir.is_dir() {
                continue;
            }
            let branch_dirs = match std::fs::read_dir(&repo_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for branch_entry in branch_dirs.filter_map(|e| e.ok()) {
                let branch_dir = branch_entry.path();
                if !branch_dir.is_dir() {
                    continue;
                }
                let branch_dir_str = branch_dir.to_string_lossy().to_string();
                if known_worktree_paths.contains(&branch_dir_str) {
                    continue;
                }

                let repo_dir_name = repo_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let repo_path_candidate = home_dev.join(&repo_dir_name).to_string_lossy().to_string();

                if !known_repos.contains_key(&repo_path_candidate) {
                    let candidate_path = Path::new(&repo_path_candidate);
                    if candidate_path.is_dir() {
                        let default_branch = git::detect_default_branch(&repo_path_candidate)
                            .unwrap_or_else(|_| "main".into());
                        let repo = db::add_repo(
                            &self.conn,
                            &Repo {
                                id: None,
                                path: repo_path_candidate.clone(),
                                name: repo_dir_name.clone(),
                                default_branch,
                            },
                        );
                        known_repos.insert(repo_path_candidate.clone(), repo);
                    } else {
                        continue;
                    }
                }

                let repo = &known_repos[&repo_path_candidate];
                let branch_name = branch_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                db::add_worktree(
                    &self.conn,
                    &Worktree {
                        id: None,
                        repo_id: repo.id.unwrap(),
                        path: branch_dir_str.clone(),
                        branch: branch_name,
                        session_id: None,
                        tmux_window: None,
                        created_at: utc_now(),
                    },
                );
                known_worktree_paths.insert(branch_dir_str);
            }
        }
    }

    /// For each managed session, check its tmux windows for panes sitting in
    /// worktree directories and adopt them (create or link DB records).
    fn adopt_session_worktrees(
        &self,
        worktrees_root: &Path,
        known_repos: &HashMap<String, Repo>,
    ) {
        let sessions = db::get_sessions(&self.conn);
        let all_windows = tmux::list_all_windows();

        for session in &sessions {
            let windows = match all_windows.get(&session.name) {
                Some(w) => w,
                None => continue,
            };

            for win in windows {
                let pane_path = Path::new(&win.path);

                // Check if this pane is inside the worktrees directory
                let rel = match pane_path.strip_prefix(worktrees_root) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                // Expected structure: <repo_name>/<branch_name>[/...]
                let mut components = rel.components();
                let repo_name = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };
                let branch_dir = match components.next() {
                    Some(c) => c.as_os_str().to_string_lossy().to_string(),
                    None => continue,
                };

                // The worktree path is worktrees_root/<repo>/<branch>
                let wt_path = worktrees_root
                    .join(&repo_name)
                    .join(&branch_dir)
                    .to_string_lossy()
                    .to_string();

                if let Some(existing) = db::get_worktree_by_path(&self.conn, &wt_path) {
                    // Already tracked — link to this session if orphaned
                    if existing.session_id.is_none() {
                        db::link_worktree_to_session(
                            &self.conn,
                            existing.id.unwrap(),
                            session.id.unwrap(),
                        );
                    }
                } else {
                    // Find the repo for this worktree
                    let repos_dir = self.repos_dir();
                    let repo_path_candidate =
                        repos_dir.join(&repo_name).to_string_lossy().to_string();
                    let repo = match known_repos.get(&repo_path_candidate) {
                        Some(r) => r,
                        None => continue,
                    };

                    db::add_worktree(
                        &self.conn,
                        &Worktree {
                            id: None,
                            repo_id: repo.id.unwrap(),
                            path: wt_path,
                            branch: branch_dir,
                            session_id: session.id,
                            tmux_window: Some(win.index),
                            created_at: utc_now(),
                        },
                    );
                }
            }
        }
    }

    fn scan_tmux_sessions(&self, known_repos: &HashMap<String, Repo>) {
        let known_session_names: HashSet<String> = db::get_sessions(&self.conn)
            .into_iter()
            .map(|s| s.name)
            .collect();

        for ts in tmux::list_sessions() {
            if known_session_names.contains(&ts.name) {
                continue;
            }
            let mut matched_repo: Option<&Repo> = None;
            for repo in known_repos.values() {
                if ts.name.starts_with(&repo.name) {
                    matched_repo = Some(repo);
                    break;
                }
            }
            if let Some(repo) = matched_repo {
                db::add_session(
                    &self.conn,
                    &Session {
                        id: None,
                        name: ts.name.clone(),
                        repo_id: repo.id.unwrap(),
                        base_branch: repo.default_branch.clone(),
                        created_at: utc_now(),
                        last_selected_at: None,
                    },
                );
            }
        }
    }

    /// First-run adoption: scan repos and worktrees dirs, scan live tmux sessions,
    /// and populate the DB with discovered state.
    pub fn scan_existing(&self) {
        let home_dev = self.repos_dir();
        let worktrees_root = self.worktrees_dir();
        let mut known_repos: HashMap<String, Repo> = db::get_repos(&self.conn)
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        let mut known_worktree_paths: HashSet<String> = db::get_worktrees(&self.conn)
            .into_iter()
            .map(|wt| wt.path)
            .collect();

        self.scan_repos(&home_dev, &mut known_repos);
        self.scan_worktrees(&home_dev, &worktrees_root, &mut known_repos, &mut known_worktree_paths);
        self.scan_tmux_sessions(&known_repos);
        self.adopt_session_worktrees(&worktrees_root, &known_repos);
    }

    // ------------------------------------------------------------------
    // Public convenience wrappers
    // ------------------------------------------------------------------

    pub fn get_repos(&self) -> Vec<Repo> {
        db::get_repos(&self.conn)
    }

    pub fn get_sessions(&self) -> Vec<Session> {
        db::get_sessions(&self.conn)
    }

    pub fn get_worktrees_for_session(&self, session_id: i64) -> Vec<Worktree> {
        db::get_worktrees_for_session(&self.conn, session_id)
    }

    pub fn touch_session(&self, session_id: i64) {
        db::touch_session(&self.conn, session_id);
    }

    pub fn get_session_by_name(&self, name: &str) -> Option<Session> {
        db::get_session_by_name(&self.conn, name)
    }

    pub fn get_all_config(&self) -> Vec<(String, String)> {
        db::get_all_config(&self.conn)
    }

    pub fn set_config(&self, key: &str, value: &str) {
        db::set_config(&self.conn, key, value);
    }

    pub fn get_all_worktrees(&self) -> Vec<Worktree> {
        db::get_worktrees(&self.conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    fn init_repo(dir: &Path) {
        Command::new("git").args(["init", "-q"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["commit", "--allow-empty", "-qm", "init"]).current_dir(dir).output().unwrap();
        Command::new("git").args(["branch", "-M", "main"]).current_dir(dir).output().unwrap();
    }

    fn make_repo(tmp: &Path) -> (String, String) {
        let canonical = tmp.canonicalize().unwrap();
        let repo = canonical.join("repo");
        let worktrees = canonical.join("worktrees");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&worktrees).unwrap();
        init_repo(&repo);
        (repo.to_string_lossy().to_string(), worktrees.to_string_lossy().to_string())
    }

    #[test]
    fn resolve_creates_when_no_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let path = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert!(Path::new(&path).exists());
        assert!(path.ends_with("feat-x"));
    }

    #[test]
    fn resolve_reuses_existing_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn resolve_recovers_from_prunable_worktree() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        let first = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        // Nuke the dir behind git's back to make it prunable.
        fs::remove_dir_all(&first).unwrap();
        let second = resolve_or_create_worktree(&repo, "myrepo", "feat/x", &wts, "main").unwrap();
        assert!(Path::new(&second).exists());
    }

    #[test]
    fn resolve_finds_worktree_at_nonconventional_path() {
        let tmp = tempdir().unwrap();
        let (repo, wts) = make_repo(tmp.path());
        // User created a worktree at an unusual path outside our convention.
        let unusual = tmp.path().canonicalize().unwrap().join("unusual-spot");
        Command::new("git")
            .args(["worktree", "add", "-b", "feat-y", unusual.to_str().unwrap(), "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        // Now ask trellis to find/create for feat-y — should return the unusual path.
        let found = resolve_or_create_worktree(&repo, "myrepo", "feat-y", &wts, "main").unwrap();
        assert_eq!(found, unusual.to_string_lossy().to_string());
    }

    // ------------------------------------------------------------------
    // find_or_create_session tests
    // ------------------------------------------------------------------

    fn make_manager() -> Manager {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn);
        Manager::new(conn)
    }

    /// Cleans up a tmux session at end of test (even on panic).
    struct SessionGuard(String);
    impl Drop for SessionGuard {
        fn drop(&mut self) {
            let _ = crate::tmux::kill_session(&self.0);
        }
    }

    /// Process-unique branch name. tmux::sanitize_session_name leaves `-` and
    /// alphanumerics alone, so the sanitized version is identical.
    fn unique_branch(suffix: &str) -> String {
        format!("trellistest-{}-{}", std::process::id(), suffix)
    }

    #[test]
    fn find_or_create_session_inserts_when_missing() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("fresh");
        let _guard = SessionGuard(crate::tmux::sanitize_session_name(&branch));
        let wt_path = tmp.path().join("wt").to_string_lossy().to_string();

        let session = mgr.find_or_create_session(&repo, &branch, &wt_path).unwrap();
        assert_eq!(session.base_branch, branch);
        assert!(crate::db::get_session_by_name(&mgr.conn, &session.name).is_some());
    }

    #[test]
    fn find_or_create_session_drops_stale_db_row_then_inserts() {
        let tmp = tempdir().unwrap();
        let (repo_path, _) = make_repo(tmp.path());
        let mgr = make_manager();

        let repo = mgr.get_or_create_repo(&repo_path);
        let branch = unique_branch("stale");
        let session_name = crate::tmux::sanitize_session_name(&branch);
        let _guard = SessionGuard(session_name.clone());

        // Pre-insert a "stale" DB row. The corresponding tmux session does not
        // exist yet (unique name + Drop cleanup guarantees this across runs).
        crate::db::add_session(&mgr.conn, &crate::models::Session {
            id: None,
            name: session_name.clone(),
            repo_id: repo.id.unwrap(),
            base_branch: branch.clone(),
            created_at: crate::utils::utc_now(),
            last_selected_at: None,
        });
        assert!(crate::db::get_session_by_name(&mgr.conn, &session_name).is_some());
        assert!(!crate::tmux::session_exists(&session_name));

        let wt_path = tmp.path().join("wt").to_string_lossy().to_string();
        let fresh = mgr.find_or_create_session(&repo, &branch, &wt_path).unwrap();
        assert_eq!(fresh.name, session_name);
        let rows: Vec<_> = crate::db::get_sessions(&mgr.conn)
            .into_iter()
            .filter(|s| s.name == session_name)
            .collect();
        assert_eq!(rows.len(), 1);
    }
}
