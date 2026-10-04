//! The `?` help, generated from the keymap: a title per section, then each
//! binding's keys and what they do.

use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::state::app::Model;
use crate::state::{keymap, text};
use crate::view::layout;
use crate::view::theme::Role;

/// The width of the keys column.
const KEYS_WIDTH: usize = 16;

/// Draws the help over everything, scrolled by `scroll` lines.
pub fn render(model: &Model, scroll: usize, frame: &mut Frame) {
    let theme = &model.theme;
    let mut lines = Vec::new();
    for section in keymap::help() {
        lines.push(Line::from(Span::styled(
            section.title,
            theme.style(Role::Focus),
        )));
        for (keys, what) in section.lines {
            lines.push(Line::from(vec![
                Span::styled(format!("  {keys:<KEYS_WIDTH$}"), theme.style(Role::Cursor)),
                Span::styled(what, theme.style(Role::Text)),
            ]));
        }
    }
    let area = layout::help_area(frame.area());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.style(Role::Focus))
        .title(Span::styled(text::HELP_TITLE, theme.style(Role::Focus)))
        .title_bottom(
            Line::from(Span::styled(text::HELP_FOOTER, theme.style(Role::Muted))).right_aligned(),
        );
    let visible = usize::from(area.height.saturating_sub(2));
    let max = lines.len().saturating_sub(visible);
    let offset = u16::try_from(scroll.min(max)).unwrap_or(u16::MAX);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block).scroll((offset, 0)), area);
}
