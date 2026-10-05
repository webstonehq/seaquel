//! Panels 1–3 and what the main view shows for them: the
//! connection, the tables and views by schema, the saved queries by folder
//! and the history. Drawn from the model only.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::theme::Role;
use crate::state::app::{Conn, Load, Model, Panel, SavedTab, TablesTab};
use crate::state::panels::{approx_count, Row, TableKind};
use crate::state::text;

/// Panel 1's line: `✓ name → pg · host · ssh bastion`, or what it's doing.
pub fn connection_line(model: &Model) -> Line<'static> {
    let theme = &model.theme;
    let Some(id) = model.conn.id() else {
        return Line::from(Span::styled(
            format!("{} · {}", text::NOT_CONNECTED, text::PICK_HINT),
            theme.style(Role::Dim),
        ));
    };
    let Some(row) = model.library.connection(id) else {
        return Line::from(Span::styled(text::NOT_CONNECTED, theme.style(Role::Dim)));
    };
    let (mark, role) = match &model.conn {
        Conn::Connected { .. } => ("✓", Role::Added),
        Conn::Connecting(_) => ("…", Role::Modified),
        _ => ("✗", Role::Deleted),
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), theme.style(role)),
        Span::styled(row.name.clone(), theme.style(Role::Text).bold()),
    ];
    match &model.conn {
        Conn::Closed { .. } => {
            spans.push(Span::styled(
                format!(" · {}", text::CLOSED),
                theme.style(Role::Muted),
            ));
        }
        Conn::Failed { .. } => {
            spans.push(Span::styled(
                format!(" · {}", text::NOT_CONNECTED),
                theme.style(Role::Muted),
            ));
        }
        _ => {
            spans.push(Span::styled(" → ", theme.style(Role::Muted)));
            let mut place = format!("{} · {}", row.engine_label(), row.place());
            if let Some(tunnel) = &row.tunnel {
                place.push_str(&format!(" · ssh {}", tunnel.host));
            }
            spans.push(Span::styled(place, theme.style(Role::Text)));
        }
    }
    Line::from(spans)
}

/// `line` cut to `width` columns, ending in `…` when it doesn't fit.
pub fn fit_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let style = line.style;
    let mut spans = Vec::new();
    let mut used = 0;
    for span in line.spans {
        let w = span.width();
        if used + w < width {
            used += w;
            spans.push(span);
            continue;
        }
        let cut = fit(&span.content, width - used);
        spans.push(Span::styled(cut, span.style));
        break;
    }
    Line::from(spans).style(style)
}

/// The rows a list shows in `height` lines: the window that keeps
/// `selected` in view.
pub(super) fn window(len: usize, selected: usize, height: usize) -> std::ops::Range<usize> {
    if height == 0 {
        return 0..0;
    }
    let start = (selected + 1).saturating_sub(height);
    start..len.min(start + height)
}

/// One row: `left` on the left, `right` right-aligned, the selection
/// background across the width when selected.
pub(super) fn row_line(
    model: &Model,
    width: usize,
    left: Vec<Span<'static>>,
    right: Option<Span<'static>>,
    selected: Option<bool>,
) -> Line<'static> {
    let mut spans = left;
    if let Some(right) = right {
        let used: usize = spans.iter().map(Span::width).sum();
        let gap = width.saturating_sub(used + right.width());
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(right);
        }
    }
    let mut line = Line::from(spans);
    if let Some(focused) = selected {
        let used = line.width();
        if used < width {
            line.spans.push(Span::raw(" ".repeat(width - used)));
        }
        line = line.style(model.theme.selection(focused));
    }
    line
}

/// Cuts `text` to `width` columns, ending in `…` when it doesn't fit.
pub fn fit(text: &str, width: usize) -> String {
    let text = &crate::state::grid::clean(text);
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// What panel 2 says instead of its rows, if anything: no connection,
/// loading, or a failed read. The mouse's hit-testing asks too, so a click
/// never lands on rows that aren't drawn.
pub(super) fn tables_placeholder(model: &Model) -> Option<&'static str> {
    match model.schema_load {
        Load::Idle if model.conn.core_id().is_none() => Some(text::NO_CONNECTION),
        Load::Loading if model.schema.is_empty() => Some(text::LOADING),
        Load::Failed => Some(text::LOAD_FAILED),
        _ => None,
    }
}

/// Panel 2's body lines.
pub fn tables_lines(model: &Model, area: Rect) -> Vec<Line<'static>> {
    let theme = &model.theme;
    let width = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let empty = |s: &'static str| vec![Line::from(Span::styled(s, theme.style(Role::Dim)))];
    if let Some(placeholder) = tables_placeholder(model) {
        return empty(placeholder);
    }
    let rows = model.table_rows();
    if rows.is_empty() {
        return empty(match model.tables_tab {
            TablesTab::Tables => text::NO_TABLES,
            TablesTab::Views => text::NO_VIEWS,
        });
    }
    let list = model.list(Panel::Tables).copied().unwrap_or_default();
    let focused = model.focus == Panel::Tables;
    window(rows.len(), list.selected, height)
        .map(|i| {
            let selected = (i == list.selected).then_some(focused);
            match &rows[i] {
                Row::Group { name, count, open } => row_line(
                    model,
                    width,
                    vec![
                        Span::styled(if *open { "▾" } else { "▸" }, theme.style(Role::Muted)),
                        Span::styled(
                            fit(name, width.saturating_sub(6)),
                            theme.style(Role::Header),
                        ),
                    ],
                    Some(Span::styled(count.to_string(), theme.style(Role::Muted))),
                    selected,
                ),
                Row::Item(t) => {
                    let table = &model.schema[*t];
                    let mut right = match table.kind {
                        TableKind::Table => table.row_count.map(approx_count).unwrap_or_default(),
                        TableKind::View => "view".to_string(),
                        TableKind::MaterializedView => "matview".to_string(),
                    };
                    // The staged marker (`~1 -1`, design 1a), before the count.
                    let marker = staged_marker(model, &table.schema, &table.name);
                    if !marker.is_empty() {
                        right = format!("{marker}  {right}");
                    }
                    let room = width.saturating_sub(right.width() + 3);
                    row_line(
                        model,
                        width,
                        vec![Span::styled(
                            format!("  {}", fit(&table.name, room)),
                            theme.style(Role::Text),
                        )],
                        Some(Span::styled(
                            right,
                            theme.style(if marker.is_empty() {
                                Role::Muted
                            } else {
                                Role::Modified
                            }),
                        )),
                        selected,
                    )
                }
            }
        })
        .collect()
}

/// A table's staged changes as panel 2 marks them: `+1 ~1 -1`, empty when
/// none.
fn staged_marker(model: &Model, schema: &str, table: &str) -> String {
    if !crate::state::browse::here(model) {
        return String::new();
    }
    let target = seaquel_core::domain::edits::TableTarget {
        schema: schema.to_string(),
        table: table.to_string(),
    };
    let s = model.queue.table_counts(&target);
    [("+", s.inserts), ("~", s.updates), ("-", s.deletes)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(sign, n)| format!("{sign}{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first line of a statement, its whitespace folded.
fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Panel 3's body lines.
pub fn saved_lines(model: &Model, area: Rect) -> Vec<Line<'static>> {
    let theme = &model.theme;
    let width = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let focused = model.focus == Panel::Saved;
    let empty = |s: &'static str| vec![Line::from(Span::styled(s, theme.style(Role::Dim)))];
    match model.saved_tab {
        SavedTab::Saved => {
            let rows = model.saved_rows();
            if rows.is_empty() {
                return empty(text::NO_SAVED);
            }
            window(rows.len(), model.saved.selected, height)
                .map(|i| {
                    let selected = (i == model.saved.selected).then_some(focused);
                    match &rows[i] {
                        Row::Group { name, count, open } => row_line(
                            model,
                            width,
                            vec![
                                Span::styled(
                                    if *open { "▾" } else { "▸" },
                                    theme.style(Role::Muted),
                                ),
                                Span::styled(
                                    fit(name, width.saturating_sub(6)),
                                    theme.style(Role::Header),
                                ),
                            ],
                            Some(Span::styled(count.to_string(), theme.style(Role::Muted))),
                            selected,
                        ),
                        Row::Item(q) => {
                            let query = &model.saved_items[*q];
                            let marker = if query.shared {
                                text::SHARED_MARKER
                            } else {
                                " "
                            };
                            row_line(
                                model,
                                width,
                                vec![
                                    Span::styled(format!("{marker} "), theme.style(Role::Name)),
                                    Span::styled(
                                        fit(&query.name, width.saturating_sub(2)),
                                        theme.style(Role::Text),
                                    ),
                                ],
                                None,
                                selected,
                            )
                        }
                    }
                })
                .collect()
        }
        SavedTab::History => {
            if model.history_items.is_empty() {
                return empty(text::NO_HISTORY);
            }
            window(model.history_items.len(), model.history.selected, height)
                .map(|i| {
                    let item = &model.history_items[i];
                    let selected = (i == model.history.selected).then_some(focused);
                    let room = width.saturating_sub(item.when.width() + 2);
                    row_line(
                        model,
                        width,
                        vec![
                            Span::styled(format!("{}  ", item.when), theme.style(Role::Dim)),
                            Span::styled(fit(&one_line(&item.sql), room), theme.style(Role::Text)),
                        ],
                        None,
                        selected,
                    )
                })
                .collect()
        }
    }
}

/// What the main view shows for panels 2 and 3: a table's columns, a
/// schema's or folder's count, a saved query's or history row's SQL
/// (read-only).
pub fn main_lines(model: &Model) -> Option<Vec<Line<'static>>> {
    let theme = &model.theme;
    let title = |t: String| Line::from(Span::styled(t, theme.style(Role::Name).bold()));
    let muted = |t: String| Line::from(Span::styled(t, theme.style(Role::Muted)));
    let sql = |text: &str| -> Vec<Line<'static>> {
        text.lines()
            .map(|l| Line::from(Span::styled(l.to_string(), theme.style(Role::Text))))
            .collect()
    };
    match model.ctx {
        Panel::Tables => match model.selected_table_row()? {
            Row::Group { name, count, .. } => {
                Some(vec![title(name), muted(format!("{count} in this schema"))])
            }
            Row::Item(t) => {
                let table = &model.schema[t];
                // The columns are known once read (the table opened, or
                // completion needed them); `schema_tables` lists none on
                // any engine, so until then the preview says how to load
                // them, never "0 columns".
                let known = !table.columns.is_empty();
                let rows = match table.row_count {
                    Some(n) if table.kind == TableKind::Table => {
                        Some(format!("≈{} rows", approx_count(n)))
                    }
                    _ => None,
                };
                let summary = match (known, rows) {
                    (true, Some(rows)) => format!("{} columns · {rows}", table.columns.len()),
                    (true, None) => format!("{} columns", table.columns.len()),
                    (false, Some(rows)) => rows,
                    (false, None) => String::new(),
                };
                let mut lines = vec![title(format!("{}.{}", table.schema, table.name))];
                if !summary.is_empty() {
                    lines.push(muted(summary));
                }
                lines.push(Line::default());
                let name_width = table
                    .columns
                    .iter()
                    .map(|(n, _)| n.width())
                    .max()
                    .unwrap_or(0);
                lines.extend(table.columns.iter().map(|(name, ty)| {
                    Line::from(vec![
                        Span::styled(format!("{name:<name_width$}  "), theme.style(Role::Text)),
                        Span::styled(ty.clone(), theme.style(Role::Header)),
                    ])
                }));
                if model.conn.core_id().is_some() {
                    if known {
                        lines.push(Line::default());
                        lines.push(muted(text::OPEN_HINT.to_string()));
                    } else {
                        lines.push(muted(text::LOAD_COLUMNS_HINT.to_string()));
                    }
                }
                Some(lines)
            }
        },
        Panel::Saved => match model.saved_tab {
            SavedTab::Saved => match model.selected_saved_row()? {
                Row::Group { name, count, .. } => {
                    Some(vec![title(name), muted(format!("{count} saved queries"))])
                }
                Row::Item(q) => {
                    let query = &model.saved_items[q];
                    let mut lines = vec![title(query.name.clone())];
                    if query.shared {
                        lines.push(muted("shared with the project's repository".to_string()));
                    }
                    lines.push(Line::default());
                    lines.extend(sql(&query.sql));
                    Some(lines)
                }
            },
            SavedTab::History => {
                let item = model.history_items.get(model.history.selected)?;
                let mut lines = vec![muted(format!(
                    "{} · {} ms · {} rows",
                    item.when, item.elapsed_ms, item.rows
                ))];
                lines.push(Line::default());
                lines.extend(sql(&item.sql));
                Some(lines)
            }
        },
        _ => None,
    }
}

/// A paragraph of `lines` in `block`, wrapped.
pub fn body(lines: Vec<Line<'static>>, block: Block<'static>, style: Style) -> Paragraph<'static> {
    Paragraph::new(lines)
        .style(style)
        .wrap(Wrap { trim: false })
        .block(block)
}

/// Draws `lines` (no wrapping: each list row is one line) in `block`.
pub fn list(frame: &mut Frame, area: Rect, block: Block<'static>, lines: Vec<Line<'static>>) {
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
