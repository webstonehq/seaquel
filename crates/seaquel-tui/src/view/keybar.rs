//! The key bar: `Label: key | Label: key …` from the keymap for the current
//! context (never written by hand), and the mode on the right (`EDIT`,
//! `FILTER`, … once later tasks have modes; `seaquel <version>` until then).
//! Entries that don't fit beside the mode are dropped from the middle: the
//! last (`Keybindings: ?`) always stays.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::state::app::Model;
use crate::state::keymap;
use crate::state::text;
use crate::view::theme::Role;

const SEPARATOR: &str = " | ";

/// The bar's left side as spans, at most `width` columns. The last entry
/// (`Keybindings: ?`, or a dialog's way out) always stays; entries before
/// it are kept in order while they fit, so a narrow bar loses its middle.
pub fn entries(model: &Model, width: usize) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let all = keymap::bar(model.bar_context());
    let Some((&last, rest)) = all.split_last() else {
        return Vec::new();
    };
    let cost = |(label, key): (&str, &str)| label.chars().count() + 2 + key.chars().count();
    let mut used = cost(last);
    let mut kept = Vec::new();
    for &entry in rest {
        let more = cost(entry) + SEPARATOR.len();
        if used + more > width {
            break;
        }
        used += more;
        kept.push(entry);
    }
    if used > width {
        return Vec::new();
    }
    kept.push(last);
    let mut spans = Vec::new();
    for (i, (label, key)) in kept.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(SEPARATOR, theme.style(Role::Border)));
        }
        spans.push(Span::styled(format!("{label}: "), theme.style(Role::Text)));
        spans.push(Span::styled(key, theme.style(Role::Cursor)));
    }
    spans
}

/// Draws the bar into `area` (one row).
pub fn render(model: &Model, area: Rect, buf: &mut Buffer) {
    let mode = model.mode().unwrap_or_else(text::version_label);
    let mode_width = mode.chars().count();
    let room = usize::from(area.width).saturating_sub(mode_width + 1);
    Line::from(entries(model, room)).render(area, buf);
    let role = if model.mode().is_some() {
        Role::Focus
    } else {
        Role::Muted
    };
    Line::from(Span::styled(mode, model.theme.style(role)))
        .right_aligned()
        .render(area, buf);
}
