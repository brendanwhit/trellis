// trellis/src/tui/adopt_worktree.rs

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};

use crate::manager::{Manager, OrphanWorktree};
use crate::switch::SwitchAction;
use crate::tmux;
use crate::utils::truncate_start;

use super::theme;
use super::{ScreenAction, ScreenBehavior};

enum InlineMode {
    None,
    NamePrompt {
        input: String,
        cursor: usize,
        repo_id: i64,
        path: String,
        branch: String,
    },
}

pub struct AdoptWorktreeScreen {
    rows: Vec<OrphanWorktree>,
    table_state: TableState,
    inline_mode: InlineMode,
    status_message: Option<String>,
}

impl AdoptWorktreeScreen {
    pub fn new(manager: &Manager) -> Self {
        let rows = manager.list_orphan_worktrees();
        let mut table_state = TableState::default();
        if !rows.is_empty() {
            table_state.select(Some(0));
        }
        Self {
            rows,
            table_state,
            inline_mode: InlineMode::None,
            status_message: None,
        }
    }

    fn refresh(&mut self, manager: &Manager) {
        self.rows = manager.list_orphan_worktrees();
        if self.rows.is_empty() {
            self.table_state.select(None);
        } else {
            let cursor = self
                .table_state
                .selected()
                .unwrap_or(0)
                .min(self.rows.len() - 1);
            self.table_state.select(Some(cursor));
        }
    }

    fn cursor_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.table_state.selected().unwrap_or(0);
        if cur + 1 < self.rows.len() {
            self.table_state.select(Some(cur + 1));
        }
    }

    fn cursor_up(&mut self) {
        let cur = self.table_state.selected().unwrap_or(0);
        if cur > 0 {
            self.table_state.select(Some(cur - 1));
        }
    }

    fn start_adopt(&mut self) {
        let cur = match self.table_state.selected() {
            Some(c) if c < self.rows.len() => c,
            _ => return,
        };
        let row = &self.rows[cur];
        let default_name = tmux::sanitize_session_name(&row.branch);
        let cursor = default_name.len();
        self.inline_mode = InlineMode::NamePrompt {
            input: default_name,
            cursor,
            repo_id: row.repo.id.unwrap(),
            path: row.path.clone(),
            branch: row.branch.clone(),
        };
    }

    fn handle_name_prompt_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        manager: &mut Manager,
    ) -> ScreenAction {
        match code {
            KeyCode::Esc => {
                self.inline_mode = InlineMode::None;
                ScreenAction::None
            }
            KeyCode::Enter => {
                let (input, repo_id, path, branch) = match &self.inline_mode {
                    InlineMode::NamePrompt {
                        input,
                        repo_id,
                        path,
                        branch,
                        ..
                    } => (
                        input.trim().to_string(),
                        *repo_id,
                        path.clone(),
                        branch.clone(),
                    ),
                    _ => return ScreenAction::None,
                };
                if input.is_empty() {
                    self.status_message = Some("Name cannot be empty.".to_string());
                    return ScreenAction::None;
                }
                let name = tmux::sanitize_session_name(&input);
                if manager.get_session_by_name(&name).is_some() {
                    self.status_message = Some(format!("Session '{}' already exists.", name));
                    return ScreenAction::None;
                }
                if tmux::session_exists(&name) {
                    self.status_message =
                        Some(format!("tmux session '{}' already exists.", name));
                    return ScreenAction::None;
                }
                match manager.adopt_worktree(repo_id, &path, &branch, &name) {
                    Ok(_) => {
                        self.inline_mode = InlineMode::None;
                        ScreenAction::Switch(SwitchAction::Session { target: name })
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Error: {}", e));
                        ScreenAction::None
                    }
                }
            }
            _ => {
                use super::rename::input_handle_key;
                if let InlineMode::NamePrompt { input, cursor, .. } = &mut self.inline_mode {
                    input_handle_key(input, cursor, code, modifiers);
                }
                ScreenAction::None
            }
        }
    }
}

impl ScreenBehavior for AdoptWorktreeScreen {
    fn render(&self, f: &mut Frame, area: Rect, _manager: &Manager) {
        let status_height = match &self.inline_mode {
            InlineMode::None => {
                if self.status_message.is_some() {
                    1
                } else {
                    0
                }
            }
            _ => 1,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(status_height),
                Constraint::Length(1),
            ])
            .split(area);

        if self.rows.is_empty() {
            let empty = Paragraph::new(vec![
                Line::from(Span::styled(
                    "No untracked worktrees found.",
                    Style::default().fg(theme::TEXT_DIM),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Use `n` to create new sessions.",
                    Style::default().fg(theme::TEXT_DIM),
                )),
            ])
            .alignment(Alignment::Center);
            f.render_widget(empty, chunks[0]);
        } else {
            let header = Row::new(vec![
                Cell::from("Repo"),
                Cell::from("Branch"),
                Cell::from("Path"),
            ])
            .style(theme::style_header())
            .height(1);

            // Path column gets whatever's left; truncate from the start so
            // the leaf segment stays visible.
            let path_width = chunks[0].width.saturating_sub(20 + 30 + 2) as usize;
            let table_rows: Vec<Row> = self
                .rows
                .iter()
                .map(|w| {
                    Row::new(vec![
                        Cell::from(w.repo.name.clone()),
                        Cell::from(w.branch.clone()),
                        Cell::from(truncate_start(&w.path, path_width.max(10))),
                    ])
                })
                .collect();

            let widths = [
                Constraint::Length(20),
                Constraint::Length(30),
                Constraint::Min(0),
            ];

            let table = Table::new(table_rows, widths)
                .header(header)
                .row_highlight_style(theme::style_cursor())
                .style(theme::style_default())
                .block(Block::default().style(theme::style_default()));

            let mut state = self.table_state.clone();
            f.render_stateful_widget(table, chunks[0], &mut state);
        }

        match &self.inline_mode {
            InlineMode::None => {
                if let Some(ref msg) = self.status_message {
                    let status = Paragraph::new(msg.as_str())
                        .style(Style::default().fg(theme::TEXT_DIM).bg(theme::BG));
                    f.render_widget(status, chunks[1]);
                }
            }
            InlineMode::NamePrompt {
                input,
                cursor: cursor_pos,
                ..
            } => {
                let prefix = "Session name: ";
                let suffix = "  [enter] confirm  [esc] cancel";

                let before = &input[..*cursor_pos];
                let cursor_char_len = input[*cursor_pos..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                let cursor_display = if *cursor_pos < input.len() {
                    input[*cursor_pos..*cursor_pos + cursor_char_len].to_string()
                } else {
                    " ".to_string()
                };
                let after = &input[(*cursor_pos + cursor_char_len).min(input.len())..];

                let spans = vec![
                    Span::styled(prefix, Style::default().fg(theme::ACCENT)),
                    Span::styled(before.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(
                        cursor_display,
                        Style::default().fg(theme::BG).bg(theme::ACCENT),
                    ),
                    Span::styled(after.to_string(), Style::default().fg(theme::TEXT)),
                    Span::styled(suffix, Style::default().fg(theme::TEXT_DIM)),
                ];
                let prompt =
                    Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG));
                f.render_widget(prompt, chunks[1]);
            }
        }

        let footer = super::rename::render_footer_bindings(&[
            ("q", "Quit"),
            ("j/k", "Navigate"),
            ("enter", "Adopt"),
            ("esc", "Back"),
        ]);
        f.render_widget(footer, chunks[2]);
    }

    fn handle_event(&mut self, event: &Event, manager: &mut Manager) -> ScreenAction {
        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) = event
        else {
            return ScreenAction::None;
        };

        if matches!(self.inline_mode, InlineMode::NamePrompt { .. }) {
            return self.handle_name_prompt_key(*code, *modifiers, manager);
        }

        self.status_message = None;

        match code {
            KeyCode::Char('q') | KeyCode::Esc => ScreenAction::Pop,
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor_down();
                ScreenAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor_up();
                ScreenAction::None
            }
            KeyCode::Enter => {
                self.start_adopt();
                ScreenAction::None
            }
            _ => ScreenAction::None,
        }
    }

    fn on_resume(&mut self, manager: &mut Manager) {
        self.refresh(manager);
    }
}
