//! The Data tab (Decision 11; screen 1a): the `/` filter line with the
//! server filter and sort on its right, the column names and types, the
//! rows (staged inserts first) with their `~`/`-`/`+` markers, and the
//! footer with the cursor's `column · type`, a staged cell's `old → new`
//! and the page's counts. Drawn from the model only; columns that don't fit
//! scroll so the cursor's stays in view, and a cell wider than its column
//! is cut with `…`.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::theme::Role;
use crate::state::app::{Model, Panel};
use crate::state::browse::{self, op_takes_value, op_text, GridRow};
use crate::state::grid::{self, COLUMN_GAP};
use crate::state::pending::Staging;
use crate::state::text;
use seaquel_core::domain::edits::{FilterOp, SortDirection};
use seaquel_core::Value;

/// The mark column: `~`, `-` or `+` and a space.
const MARK: usize = 2;

/// One drawn cell: its text, how it's styled, and whether it lines up on
/// the right.
struct Cell {
    text: String,
    role: Role,
    right: bool,
}

/// What a grid row draws.
struct DrawnRow {
    mark: Option<(char, Role)>,
    cells: Vec<Cell>,
    deleted: bool,
}

/// The cells of one grid row, as text and roles.
fn drawn_row(model: &Model, row: &GridRow, columns: &[String], right: &[bool]) -> DrawnRow {
    let column_right = |i: usize| right.get(i).copied().unwrap_or(false);
    let here = browse::here(model);
    let Some(opened) = &model.browse.opened else {
        return DrawnRow {
            mark: None,
            cells: Vec::new(),
            deleted: false,
        };
    };
    match row {
        GridRow::Insert(_) => DrawnRow {
            mark: Some(('+', Role::Added)),
            cells: columns
                .iter()
                .enumerate()
                .map(|(i, c)| match browse::cell_value(model, row, c) {
                    Some(v) => Cell {
                        right: column_right(i),
                        text: grid::display(&v),
                        role: Role::Added,
                    },
                    None => Cell {
                        text: text::DEFAULT_CELL.to_string(),
                        role: Role::Dim,
                        right: false,
                    },
                })
                .collect(),
            deleted: false,
        },
        GridRow::Page(i) => {
            let key = browse::row_key(model, *i).filter(|_| here);
            let deleted = key
                .as_ref()
                .is_some_and(|k| model.queue.deleted(&opened.target, k));
            let mut edited = false;
            let cells = columns
                .iter()
                .enumerate()
                .map(|(ci, c)| {
                    let staged = key
                        .as_ref()
                        .and_then(|k| model.queue.cell(&opened.target, k, c));
                    let value = browse::cell_value(model, row, c);
                    edited |= staged.is_some();
                    let text = match &value {
                        Some(v) => grid::display(v),
                        None => text::DEFAULT_CELL.to_string(),
                    };
                    let right = column_right(ci);
                    let role = if deleted {
                        Role::Deleted
                    } else if staged.is_some() {
                        Role::Modified
                    } else if matches!(value, Some(Value::Null)) {
                        Role::Dim
                    } else {
                        Role::Text
                    };
                    Cell { text, role, right }
                })
                .collect();
            let mark = if deleted {
                Some(('-', Role::Deleted))
            } else if edited {
                Some(('~', Role::Modified))
            } else {
                None
            };
            DrawnRow {
                mark,
                cells,
                deleted,
            }
        }
    }
}

/// The columns of `cols` that fit in `room`, with the width each is drawn
/// at: the last may be cut to what's left.
pub(super) fn shown_columns(
    widths: &[usize],
    cols: std::ops::Range<usize>,
    room: usize,
) -> Vec<(usize, usize)> {
    let mut shown: Vec<(usize, usize)> = Vec::new();
    let mut used = 0;
    for c in cols {
        let gap = if shown.is_empty() { 0 } else { COLUMN_GAP };
        let w = widths[c].min(room.saturating_sub(used + gap));
        if w == 0 {
            break;
        }
        shown.push((c, w));
        used += gap + w;
    }
    shown
}

/// The column drawn at `dx` columns into a row's cells (after its marker),
/// with `shown` from [`shown_columns`]; `None` in a gap or past the end.
pub(super) fn column_at(shown: &[(usize, usize)], dx: usize) -> Option<usize> {
    let mut x = 0;
    for (i, (c, w)) in shown.iter().enumerate() {
        if i > 0 {
            x += COLUMN_GAP;
        }
        if (x..x + w).contains(&dx) {
            return Some(*c);
        }
        x += w;
    }
    None
}

/// The grid row (into `browse::rows`) and column drawn at `(x, y)` of a
/// Data tab drawn into `inner`, as [`render`] draws it (the mouse, probe
/// F3). The column is `None` on the marker or a gap.
pub fn cell_at(model: &Model, inner: Rect, x: u16, y: u16) -> Option<(usize, Option<usize>)> {
    let (width, height) = (usize::from(inner.width), usize::from(inner.height));
    if height < 4 || width < 10 || model.browse.page.is_none() {
        return None;
    }
    // The filter line, the names and the types come first; the footer last.
    let line = usize::from(y.checked_sub(inner.y)?).checked_sub(3)?;
    let body = height - 4;
    let rows = browse::rows(model).len();
    let shown = window(rows, model.browse.row, body);
    if line >= shown.len() {
        return None;
    }
    let widths = &browse::cache(model).widths;
    let room = width.saturating_sub(MARK + 1);
    let cols = shown_columns(
        widths,
        grid::column_window(widths, model.browse.col, room),
        room,
    );
    let dx = usize::from(x.checked_sub(inner.x)?);
    let column = dx.checked_sub(MARK).and_then(|dx| column_at(&cols, dx));
    Some((shown.start + line, column))
}

/// The window of rows that keeps the cursor in `height` lines.
fn window(len: usize, selected: usize, height: usize) -> std::ops::Range<usize> {
    if height == 0 {
        return 0..0;
    }
    let start = (selected + 1).saturating_sub(height);
    start..len.min(start + height)
}

/// The filter line's right side: the server filter and the sort.
fn query_text(model: &Model) -> String {
    let mut parts = Vec::new();
    if let Some(f) = &model.browse.filter {
        let mut part = format!("F {} {}", f.column, op_text(f.op));
        if op_takes_value(f.op) {
            part.push(' ');
            part.push_str(&f.value);
        }
        parts.push(part);
    }
    if let Some(s) = &model.browse.sort {
        let arrow = match s.direction {
            SortDirection::Asc => "↑",
            SortDirection::Desc => "↓",
        };
        parts.push(format!("sort {} {arrow}", s.column));
    }
    grid::clean(&parts.join(" · "))
}

/// A line of `left` spans with `right` at its end, `width` wide.
fn split_line(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let used: usize = left.iter().map(Span::width).sum();
    let right_width: usize = right.iter().map(Span::width).sum();
    let mut spans = left;
    if used + right_width < width {
        spans.push(Span::raw(" ".repeat(width - used - right_width)));
        spans.extend(right);
    }
    Line::from(spans)
}

/// The `F` form, in place of the filter line while it's open.
fn form_line(model: &Model, columns: &[String], width: usize) -> Line<'static> {
    let theme = &model.theme;
    let Some(form) = &model.browse.form else {
        return Line::default();
    };
    let field = |active: bool, text: String| {
        let style = if active {
            theme.style(Role::Cursor).add_modifier(Modifier::REVERSED)
        } else {
            theme.style(Role::Text)
        };
        Span::styled(text, style)
    };
    use crate::state::browse::FormField;
    let op = browse::FILTER_OPS[form.op];
    let mut spans = vec![
        Span::styled("F ", theme.style(Role::Muted)),
        field(
            form.field == FormField::Column,
            grid::clean(&columns.get(form.column).cloned().unwrap_or_default()),
        ),
        Span::raw(" "),
        field(form.field == FormField::Op, op_text(op).to_string()),
    ];
    if op_takes_value(op) {
        spans.push(Span::raw(" "));
        spans.push(field(
            form.field == FormField::Value,
            format!("{}▌", grid::clean(&form.value)),
        ));
    }
    // Core compares range operators as text (`CAST(col AS <text>)`).
    if matches!(
        op,
        FilterOp::Gt | FilterOp::Lt | FilterOp::Ge | FilterOp::Le
    ) {
        spans.push(Span::styled(
            format!("  {}", text::COMPARES_AS_TEXT),
            theme.style(Role::Muted),
        ));
    }
    grid_fit(Line::from(spans), width)
}

fn grid_fit(line: Line<'static>, width: usize) -> Line<'static> {
    super::panels::fit_line(line, width)
}

/// Draws the Data tab into `inner` (the main view's box, inside its
/// border). Widths, alignment and the `/` filter's rows come from the
/// grid's cache (M1), kept per page.
pub fn render(model: &Model, inner: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    if height < 4 || width < 10 {
        return;
    }
    let browse_state = &model.browse;
    let focused = model.focus == Panel::Main;
    let cache = browse::cache(model);
    let columns = browse::columns(model);
    let types: Vec<String> = columns
        .iter()
        .map(|c| grid::clean(&browse::column_type(model, c)))
        .collect();
    let names: Vec<String> = columns.iter().map(|c| grid::clean(c)).collect();
    let rows = browse::rows(model);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // The filter line.
    if browse_state.form.is_some() {
        lines.push(form_line(model, &columns, width));
    } else {
        let (find, role) = if browse_state.finding {
            (format!("{}▌", grid::clean(&browse_state.find)), Role::Text)
        } else if browse_state.find.is_empty() {
            (text::FILTER_PLACEHOLDER.to_string(), Role::Dim)
        } else {
            (grid::clean(&browse_state.find), Role::Text)
        };
        lines.push(split_line(
            vec![
                Span::styled("/", theme.style(Role::Muted)),
                Span::styled(find, theme.style(role)),
            ],
            vec![Span::styled(query_text(model), theme.style(Role::Muted))],
            width,
        ));
    }

    let body = height - 4;
    let Some(page) = &browse_state.page else {
        let message = match &browse_state.failed {
            Some(e) => text::failed_line("table page", &e.code),
            None => text::PAGE_LOADING.to_string(),
        };
        lines.push(Line::from(Span::styled(message, theme.style(Role::Dim))));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    };

    let shown = window(rows.len(), browse_state.row, body);
    let drawn: Vec<DrawnRow> = rows[shown.clone()]
        .iter()
        .map(|r| drawn_row(model, r, &columns, &cache.right))
        .collect();
    let widths = &cache.widths;
    let room = width.saturating_sub(MARK + 1);
    let cols = grid::column_window(widths, browse_state.col, room);
    let col_widths = shown_columns(widths, cols.clone(), room);
    let more_left = cols.start > 0;
    let more_right = col_widths.last().is_some_and(|(c, _)| c + 1 < widths.len());

    let header = |values: &[String], role: Role, bold: bool, marks: bool| -> Line<'static> {
        let left = if marks && more_left { "‹ " } else { "  " };
        let mut spans = vec![Span::styled(left, theme.style(Role::Muted))];
        for (i, (c, w)) in col_widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
            }
            let mut style = theme.style(role);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(
                grid::pad(values.get(*c).map_or("", String::as_str), *w, false),
                style,
            ));
        }
        if marks && more_right {
            let used: usize = spans.iter().map(Span::width).sum();
            spans.push(Span::raw(" ".repeat(width.saturating_sub(used + 1))));
            spans.push(Span::styled("›", theme.style(Role::Muted)));
        }
        Line::from(spans)
    };
    lines.push(header(&names, Role::Header, true, true));
    lines.push(header(&types, Role::Muted, false, false));

    if rows.is_empty() {
        let message = if browse_state.find.is_empty() {
            grid::range_text(page)
        } else {
            text::NO_ROWS_MATCH.to_string()
        };
        lines.push(Line::from(Span::styled(message, theme.style(Role::Dim))));
    }
    for (offset, d) in drawn.iter().enumerate() {
        let index = shown.start + offset;
        let selected = index == browse_state.row;
        let row_style = if selected {
            theme.selection(focused)
        } else {
            Style::new()
        };
        let mut spans = vec![match d.mark {
            Some((c, role)) => Span::styled(format!("{c} "), theme.style(role)),
            None => Span::raw(" ".repeat(MARK)),
        }];
        for (i, (c, w)) in col_widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
            }
            let cell = &d.cells[*c];
            let editing = browse_state
                .editing
                .as_ref()
                .filter(|e| selected && e.row == rows[index] && columns.get(*c) == Some(&e.column));
            if let Some(edit) = editing {
                // Newlines and tabs show as markers (M6).
                let text = format!("{}▌", grid::clean(&edit.text));
                // The end of what's typed stays in view.
                let shown_text = if text.width() > *w {
                    let mut tail: String = text.chars().rev().collect();
                    tail = grid::fit(&tail, *w);
                    tail.chars().rev().collect()
                } else {
                    text
                };
                spans.push(Span::styled(
                    grid::pad(&shown_text, *w, false),
                    theme.style(Role::Text).add_modifier(Modifier::REVERSED),
                ));
                continue;
            }
            let mut style = theme.style(cell.role);
            if d.deleted {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            if selected && *c == browse_state.col && focused {
                let role = if cell.role == Role::Modified {
                    Role::Modified
                } else {
                    Role::Cursor
                };
                style = theme
                    .style(role)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD);
            }
            spans.push(Span::styled(grid::pad(&cell.text, *w, cell.right), style));
        }
        lines.push(Line::from(spans).style(row_style));
    }
    while lines.len() < height - 1 {
        lines.push(Line::default());
    }

    // The footer.
    let mut left = Vec::new();
    if let Some(column) = names.get(browse_state.col) {
        left.push(Span::styled(
            format!(
                "{column} · {}  ",
                types.get(browse_state.col).cloned().unwrap_or_default()
            ),
            theme.style(Role::Muted),
        ));
        if let Some(change) = staged_change(model, &rows, &columns[browse_state.col]) {
            left.push(Span::styled(change, theme.style(Role::Modified)));
        }
    }
    let mut counts = if browse_state.find.is_empty() {
        grid::range_text(page)
    } else {
        format!(
            "{} · {}",
            text::matches_on_page(cache.visible.len()),
            grid::range_text(page)
        )
    };
    counts.push_str(&format!(" · {}", grid::elapsed_text(page.elapsed_ms)));
    if browse_state.loading.is_some() {
        counts = format!("{} · {counts}", text::LOADING);
    }
    lines.push(split_line(
        left,
        vec![Span::styled(counts, theme.style(Role::Muted))],
        width,
    ));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The cursor cell's staged change: `old → new`.
fn staged_change(model: &Model, rows: &[GridRow], column: &str) -> Option<String> {
    let GridRow::Page(i) = rows.get(model.browse.row)? else {
        return None;
    };
    if !browse::here(model) {
        return None;
    }
    let opened = model.browse.opened.as_ref()?;
    let key = browse::row_key(model, *i)?;
    let entry = model.queue.cell(&opened.target, &key, column)?;
    let old = model.browse.page.as_ref()?.value(*i, column)?;
    let new = match &entry.staging {
        Staging::Update { value, .. } => grid::display(value),
        _ => text::DEFAULT_CELL.to_string(),
    };
    Some(format!("{} → {new}", grid::display(old)))
}

/// The main view's counter for the grid: `4 of 15`.
pub fn counter(model: &Model) -> Option<String> {
    let rows = browse::rows(model).len();
    (rows > 0).then(|| text::counter(model.browse.row.min(rows - 1), rows))
}
