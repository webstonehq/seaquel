//! The command log (Decision 13): what Core reports, newest last, each line
//! with its time, an optional tag and how long it took. In memory only.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::theme::Role;
use crate::state::app::Model;
use crate::state::log::Tag;
use crate::state::text;

/// A tag's word and colour (the prototype's `staged `, `undo `, …).
fn tag(tag: Tag) -> (&'static str, Role) {
    match tag {
        Tag::Staged => ("staged ", Role::Modified),
        Tag::StagedDelete => ("staged ", Role::Deleted),
        Tag::Unstaged => ("unstaged ", Role::Muted),
        Tag::Undo => ("undo ", Role::Name),
        Tag::Committed => ("committed ", Role::Added),
        Tag::ReadOnly => ("read-only ", Role::Deleted),
        Tag::Error => ("error ", Role::Deleted),
    }
}

pub fn render(model: &Model, area: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.style(Role::Border))
        .title(Span::styled(text::COMMAND_LOG, theme.style(Role::Muted)));
    let rows = usize::from(area.height.saturating_sub(2));
    let lines: Vec<Line> = if model.log.is_empty() {
        vec![Line::from(Span::styled(
            text::LOG_EMPTY,
            theme.style(Role::Dim),
        ))]
    } else {
        model
            .log
            .last(rows)
            .map(|l| {
                let mut spans = vec![Span::styled(
                    format!("{}  ", l.time),
                    theme.style(Role::Dim),
                )];
                if let Some(t) = l.tag {
                    let (word, role) = tag(t);
                    spans.push(Span::styled(word, theme.style(role)));
                }
                spans.push(Span::styled(
                    crate::state::grid::clean(&l.text),
                    theme.style(Role::Text),
                ));
                if let Some(elapsed) = &l.elapsed {
                    spans.push(Span::styled(format!("  {elapsed}"), theme.style(Role::Dim)));
                }
                Line::from(spans)
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
