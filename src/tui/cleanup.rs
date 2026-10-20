// trellis/src/tui/cleanup.rs — worktree cleanup screen

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::git;
use crate::manager::Manager;
use crate::utils::truncate_start;

use super::confirm::ConfirmScreen;
use super::theme;
use super::{ActionResult, ScreenAction, ScreenBehavior};

#[derive(Clone)]
enum RowSource {
    /// Worktree Trellis tracks in its DB.
    Tracked { wt_id: i64 },
    /// Worktree git knows about but Trellis doesn't (created out-of-band, or
    /// its DB row was lost).
    Orphan,
}

struct WorktreeRow {
    source: RowSource,
    branch: String,
    session_name: String,
    path: String,
    /// The owning repo's working tree, used both for `git worktree remove` on
    /// orphans and for the staleness check.
    repo_path: String,
    repo_default_branch: String,
    stale: Option<bool>, // None = still checking
}

/// Inputs the staleness worker needs per row. Lives across the thread
/// boundary, so it owns its strings.
struct StaleCheckInput {
    path: String,
    repo_path: String,
    branch: String,
    default_branch: String,
}

pub struct CleanupScreen {
    rows: Vec<WorktreeRow>,
    /// Selected rows, keyed by worktree path. Path is unique across both
    /// tracked and orphan rows, so it works as a stable identity for either.
    selected: HashSet<String>,
    table_state: TableState,
    error_message: Option<String>,
    /// True between confirming a delete and the tick that runs it. Drives the
    /// "Working…" overlay so the user sees something during the (potentially
    /// slow) `git worktree remove` calls instead of a frozen confirm modal.
    working: bool,
    stale_rx: Option<mpsc::Receiver<HashSet<String>>>,
}

impl CleanupScreen {
    pub fn new(manager: &Manager) -> Self {
        let tracked = manager.get_all_worktrees();
        let orphans = manager.list_orphan_worktrees();
        let sessions = manager.get_sessions();
        let repos = manager.get_repos();

        let session_map: HashMap<i64, String> = sessions
            .into_iter()
            .filter_map(|s| s.id.map(|id| (id, s.name.clone())))
            .collect();
        let repo_map: HashMap<i64, crate::models::Repo> = repos
            .into_iter()
            .filter_map(|r| r.id.map(|id| (id, r)))
            .collect();

        let mut rows: Vec<WorktreeRow> = Vec::new();

        // DB-tracked worktrees
        for wt in &tracked {
            let Some(wt_id) = wt.id else { continue };
            let Some(repo) = repo_map.get(&wt.repo_id) else { continue };
            let session_name = wt
                .session_id
                .and_then(|sid| session_map.get(&sid).cloned())
                .unwrap_or_else(|| "Unattached".to_string());
            rows.push(WorktreeRow {
                source: RowSource::Tracked { wt_id },
                branch: wt.branch.clone(),
                session_name,
                path: wt.path.clone(),
                repo_path: repo.path.clone(),
                repo_default_branch: repo.default_branch.clone(),
                stale: None,
            });
        }

        // Worktrees git knows about but Trellis doesn't.
        // `list_orphan_worktrees` already excludes the main worktree, missing
        // dirs, detached HEAD, and paths Trellis already tracks.
        for orphan in &orphans {
            rows.push(WorktreeRow {
                source: RowSource::Orphan,
                branch: orphan.branch.clone(),
                session_name: "(orphan)".to_string(),
                path: orphan.path.clone(),
                repo_path: orphan.repo.path.clone(),
                repo_default_branch: orphan.repo.default_branch.clone(),
                stale: None,
            });
        }

        rows.sort_by(|a, b| {
            a.session_name
                .to_lowercase()
                .cmp(&b.session_name.to_lowercase())
                .then_with(|| a.branch.to_lowercase().cmp(&b.branch.to_lowercase()))
        });

        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }

        // Spawn background thread for staleness check. Hand it owned data
        // rather than database handles so it can outlive `manager`.
        let inputs: Vec<StaleCheckInput> = rows
            .iter()
            .map(|r| StaleCheckInput {
                path: r.path.clone(),
                repo_path: r.repo_path.clone(),
                branch: r.branch.clone(),
                default_branch: r.repo_default_branch.clone(),
            })
            .collect();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stale_paths = HashSet::new();
            for input in &inputs {
                let merged = git::is_branch_merged(
                    &input.repo_path,
                    &input.branch,
                    &input.default_branch,
                )
                .unwrap_or(false);
                let has_remote = git::has_remote_branch(&input.repo_path, &input.branch)
                    .unwrap_or(true);
                if merged || !has_remote {
                    stale_paths.insert(input.path.clone());
                }
            }
            let _ = tx.send(stale_paths);
        });

        Self {
            rows,
            selected: HashSet::new(),
            table_state,
            error_message: None,
            working: false,
            stale_rx: Some(rx),
        }
    }

    fn check_stale_results(&mut self) {
        if let Some(ref rx) = self.stale_rx {
            if let Ok(stale_paths) = rx.try_recv() {
                for row in &mut self.rows {
                    row.stale = Some(stale_paths.contains(&row.path));
                }
                self.stale_rx = None;
            }
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        if self.rows.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let max = self.rows.len() as i32 - 1;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_current(&mut self) {
        if let Some(idx) = self.table_state.selected() {
            if idx < self.rows.len() {
                let path = self.rows[idx].path.clone();
                if !self.selected.remove(&path) {
                    self.selected.insert(path);
                }
            }
        }
    }

    fn select_all(&mut self) {
        for row in &self.rows {
            self.selected.insert(row.path.clone());
        }
    }

    fn deselect_all(&mut self) {
        self.selected.clear();
    }

    fn stale_count(&self) -> usize {
        self.rows.iter().filter(|r| r.stale == Some(true)).count()
    }

    fn status_line(&self) -> Line<'_> {
        let total = self.rows.len();
        let stale = self.stale_count();
        let sel = self.selected.len();

        Line::from(vec![
            Span::styled(
                format!(" {} worktrees", total),
                Style::default().fg(theme::TEXT_DIM),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} stale", stale),
                Style::default().fg(theme::YELLOW),
            ),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{} selected", sel),
                Style::default().fg(theme::ACCENT),
            ),
        ])
    }

    fn perform_delete(&mut self, manager: &mut Manager) {
        // Snapshot the rows to delete up-front: source + paths the user
        // selected. We mutate `self.rows` and `self.selected` as we go.
        let targets: Vec<(String, RowSource)> = self
            .rows
            .iter()
            .filter(|r| self.selected.contains(&r.path))
            .map(|r| (r.path.clone(), r.source.clone()))
            .collect();
        let mut errors = Vec::new();

        for (path, source) in targets {
            let result = match source {
                RowSource::Tracked { wt_id } => manager.cleanup_worktree(wt_id),
                RowSource::Orphan => {
                    // Find the repo path the row remembers; if the row was
                    // somehow already removed, skip.
                    let repo_path = self
                        .rows
                        .iter()
                        .find(|r| r.path == path)
                        .map(|r| r.repo_path.clone());
                    match repo_path {
                        Some(rp) => git::remove_worktree(&rp, &path).map_err(|e| e.to_string()),
                        None => Err("row missing".to_string()),
                    }
                }
            };
            match result {
                Ok(()) => {
                    self.rows.retain(|r| r.path != path);
                    self.selected.remove(&path);
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path, e));
                }
            }
        }

        if !errors.is_empty() {
            self.error_message = Some(errors.join("; "));
        } else {
            self.error_message = None;
        }

        // Fix cursor if it's past the end
        if !self.rows.is_empty() {
            let max = self.rows.len() - 1;
            let current = self.table_state.selected().unwrap_or(0);
            if current > max {
                self.table_state.select(Some(max));
            }
        } else {
            self.table_state.select(None);
        }
    }

    fn render_footer(&self) -> Paragraph<'_> {
        super::rename::render_footer_bindings(&[
            ("esc", "back"),
            ("j/k", "navigate"),
            ("space", "toggle"),
            ("a", "select all"),
            ("A", "deselect all"),
            ("d", "delete"),
        ])
    }
}

impl ScreenBehavior for CleanupScreen {
    fn tick(&mut self, manager: &mut Manager) -> ScreenAction {
        self.check_stale_results();
        if self.working {
            // The previous frame already painted the "Working…" overlay (draw
            // runs before tick in the main loop), so blocking here is fine.
            self.perform_delete(manager);
            self.working = false;
        }
        ScreenAction::None
    }

    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let error_height = if self.error_message.is_some() { 1 } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title
                Constraint::Length(1), // status bar
                Constraint::Length(error_height), // error
                Constraint::Min(0),    // table
                Constraint::Length(1), // footer
            ])
            .split(area);

        // Title
        let title = Paragraph::new(Line::from(vec![Span::styled(
            " Cleanup Worktrees",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(title, chunks[0]);

        // Status bar
        let status = Paragraph::new(self.status_line()).style(Style::default().bg(theme::HEADER_BG));
        f.render_widget(status, chunks[1]);

        // Error
        if let Some(ref err) = self.error_message {
            let err_widget =
                Paragraph::new(format!(" Error: {}", err)).style(Style::default().fg(theme::RED));
            f.render_widget(err_widget, chunks[2]);
        }

        // Table
        let header = Row::new(vec![
            Cell::from(" "),
            Cell::from("Branch"),
            Cell::from("Session"),
            Cell::from("Path"),
            Cell::from("Status"),
        ])
        .style(theme::style_header())
        .height(1);

        let table_rows: Vec<Row> = self
            .rows
            .iter()
            .map(|row| {
                let is_selected = self.selected.contains(&row.path);

                let checkbox = if is_selected {
                    Span::styled("[x]", Style::default().fg(theme::BLUE))
                } else {
                    Span::styled("[ ]", Style::default().fg(theme::TEXT_DIM))
                };

                let branch_style = if row.stale == Some(true) {
                    Style::default().fg(theme::YELLOW)
                } else {
                    Style::default().fg(theme::TEXT)
                };

                let status_span = match row.stale {
                    None => Span::styled("checking…", Style::default().fg(theme::TEXT_DIM)),
                    Some(true) => Span::styled("stale", Style::default().fg(theme::YELLOW)),
                    Some(false) => Span::styled("ok", Style::default().fg(theme::GREEN)),
                };

                Row::new(vec![
                    Cell::from(checkbox),
                    Cell::from(Span::styled(&*row.branch, branch_style)),
                    Cell::from(Span::styled(&*row.session_name, Style::default().fg(theme::TEXT))),
                    Cell::from(Span::styled(
                        truncate_start(&row.path, 40),
                        Style::default().fg(theme::TEXT_DIM),
                    )),
                    Cell::from(status_span),
                ])
            })
            .collect();

        let widths = [
            Constraint::Length(3),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Length(10),
        ];

        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(theme::style_cursor())
            .style(theme::style_default())
            .block(Block::default().style(theme::style_default()));

        let mut state = self.table_state.clone();
        f.render_stateful_widget(table, chunks[3], &mut state);

        // Footer
        let footer = self.render_footer();
        f.render_widget(footer, chunks[4]);

        if self.working {
            super::working::render_overlay(f, area, "Deleting worktrees");
        }
    }

    fn handle_event(&mut self, event: &Event, _manager: &mut Manager) -> ScreenAction {
        // Always check for background staleness results (even on non-key events)
        self.check_stale_results();

        // Drop any input that arrives while a delete is running so a queued
        // keypress can't trigger a follow-up action mid-operation.
        if self.working {
            return ScreenAction::None;
        }

        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        match code {
            KeyCode::Esc => ScreenAction::Pop,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
                ScreenAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.toggle_current();
                ScreenAction::None
            }
            KeyCode::Char('A') => {
                // Shift+A: deselect all
                self.deselect_all();
                ScreenAction::None
            }
            KeyCode::Char('a') => {
                self.select_all();
                ScreenAction::None
            }
            KeyCode::Char('d') => {
                if self.selected.is_empty() {
                    return ScreenAction::None;
                }
                let count = self.selected.len();
                ScreenAction::Push(Box::new(ConfirmScreen::new(
                    "Confirm Deletion".to_string(),
                    format!("Delete {} worktree(s)?", count),
                )))
            }
            _ => ScreenAction::None,
        }
    }

    fn on_child_result(&mut self, result: ActionResult, _manager: &mut Manager) -> ScreenAction {
        if let ActionResult::Confirmed(true) = result {
            // Defer the actual delete to the next tick so the "Working…"
            // overlay (rendered while `working` is true) lands before the
            // blocking `git worktree remove` calls start.
            self.working = true;
        }
        ScreenAction::None
    }

    fn is_modal(&self) -> bool {
        false
    }
}
