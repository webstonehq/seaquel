//! Panel 4 (design 1c): the staged changes grouped by table, each `A`, `M`
//! or `D` with its key and column, the one an apply stopped at marked `!`,
//! and the hint line. Drawn from the model only.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::panels::{fit, row_line, window};
use super::theme::Role;
use crate::state::app::{Model, Panel};
use crate::state::commit;
use crate::state::text;

/// One drawn row: a table's header, or an entry (its place in panel 4's
/// list).
enum Row {
    Table(String),
    Entry(usize),
}

/// Panel 4's rows: each table's header before its entries.
fn rows(model: &Model) -> Vec<Row> {
    let entries = model.queue.entries();
    let mut rows = Vec::new();
    let mut last_table = None;
    for (n, &i) in model.queue.display_order().iter().enumerate() {
        let target = &entries[i].target;
        if last_table != Some(target) {
            rows.push(Row::Table(format!("{}.{}", target.schema, target.table)));
            last_table = Some(target);
        }
        rows.push(Row::Entry(n));
    }
    rows
}

/// The lines the rows get in a body `height` lines tall: the hint takes the
/// last one when there's room for it.
fn list_room(height: usize) -> usize {
    if height >= 3 {
        height - 1
    } else {
        height
    }
}

fn selected_row(model: &Model, rows: &[Row]) -> usize {
    rows.iter()
        .position(|r| matches!(r, Row::Entry(n) if *n == model.pending.selected))
        .unwrap_or(0)
}

/// The entry (its place in panel 4's list) drawn on body line `line` of
/// a panel 4 in `area`, as [`lines`] draws it; `None` for a table's
/// header, the hint or past the end (the mouse, probe F3).
pub fn entry_at(model: &Model, area: Rect, line: usize) -> Option<usize> {
    let height = usize::from(area.height.saturating_sub(2));
    let rows = rows(model);
    let shown = window(rows.len(), selected_row(model, &rows), list_room(height));
    match rows
        .get(shown.start + line)
        .filter(|_| line < shown.len())?
    {
        Row::Entry(n) => Some(*n),
        Row::Table(_) => None,
    }
}

/// Panel 4's body lines.
pub fn lines(model: &Model, area: Rect) -> Vec<Line<'static>> {
    let theme = &model.theme;
    let width = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let entries = model.queue.entries();
    if entries.is_empty() {
        return vec![Line::from(Span::styled(
            text::NOTHING_STAGED,
            theme.style(Role::Dim),
        ))];
    }
    let rows = rows(model);
    // The hint takes the last line when there's room for it.
    let hint = height >= 3;
    let room = list_room(height);
    let selected_row = selected_row(model, &rows);
    let focused = model.focus == Panel::Pending;
    let order = model.queue.display_order();
    let mut out: Vec<Line<'static>> = window(rows.len(), selected_row, room)
        .map(|r| match &rows[r] {
            Row::Table(name) => Line::from(vec![
                Span::styled("▾ ", theme.style(Role::Muted)),
                Span::styled(
                    fit(name, width.saturating_sub(2)),
                    theme.style(Role::Header),
                ),
            ]),
            Row::Entry(n) => {
                let entry = &entries[order[*n]];
                let (sign, label, sub) = commit::entry_line(entry);
                let failed = model.queue.failure(&entry.id).is_some();
                let (mark, role) = if failed {
                    ('!', Role::Deleted)
                } else {
                    (
                        sign,
                        match sign {
                            'A' => Role::Added,
                            'D' => Role::Deleted,
                            _ => Role::Modified,
                        },
                    )
                };
                let label = fit(&label, width.saturating_sub(4));
                let sub_room = width.saturating_sub(label.width() + 5);
                let mut spans = vec![
                    Span::styled(format!(" {mark} "), theme.style(role)),
                    Span::styled(label, theme.style(Role::Text)),
                ];
                if !sub.is_empty() && sub_room > 1 {
                    spans.push(Span::styled(
                        format!("  {}", fit(&sub, sub_room)),
                        theme.style(Role::Muted),
                    ));
                }
                let selected = (*n == model.pending.selected).then_some(focused);
                row_line(model, width, spans, None, selected)
            }
        })
        .collect();
    if hint {
        while out.len() < room {
            out.push(Line::default());
        }
        let here = crate::state::browse::here(model);
        // A commit cut off by its connection may have landed (probe F1).
        let warn = model.queue.interrupted();
        let last = if warn {
            format!("! {}", text::MAYBE_APPLIED_HINT)
        } else if here {
            text::PENDING_HINT.to_string()
        } else {
            let id = model.queue.connection().unwrap_or_default();
            let name = model.library.connection(id).map_or(id, |c| c.name.as_str());
            text::staged_on(entries.len(), name)
        };
        out.push(Line::from(Span::styled(
            fit(&last, width),
            theme.style(if warn { Role::Warning } else { Role::Dim }),
        )));
    }
    out
}
