// trellis/src/tui/working.rs — shared "Working…" overlay
//
// A screen sets a `working` flag (or analogous state) when the user kicks off a
// slow operation. The overlay paints in the screen's `render`; the operation
// runs synchronously in the next `tick`. Because the main loop draws before
// ticking (see App::run in mod.rs), the overlay is visible for the duration of
// the blocking call instead of the previous frame freezing in place.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::theme;

/// Paint a centered "<message>…" panel over `area`, dimming the background.
pub fn render_overlay(f: &mut Frame, area: Rect, message: &str) {
    let height = 5u16;
    let width = 40u16;

    let vert = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .split(area);

    let horiz = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .split(vert[1]);

    let box_area = horiz[1];

    f.render_widget(Clear, box_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .style(Style::default().fg(theme::TEXT).bg(theme::HEADER_BG));
    let inner = block.inner(box_area);
    f.render_widget(block, box_area);

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let para = Paragraph::new(format!("{}…", message))
        .style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(para, chunks[1]);
}
