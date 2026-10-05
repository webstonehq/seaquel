//! The query view (design 1b): the editor box with its tabs,
//! highlighted lines, cursor and completion popup, the results box (Results,
//! Explain, Messages), and the query's dialogs. Drawn from the model only.
//!
//! The editor draws its own lines (the textarea keeps the buffer):
//! only the visible lines are read, tabs expand, control, bidi and
//! zero-width characters show as `�`, and wide characters are measured, so
//! a 2 MB text costs a frame what a short one does.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::layout;
use super::theme::Role;
use crate::state::app::{Model, Panel};
use crate::state::completion::popup_area;
use crate::state::editor::{char_width, Class, HlSpan, Mode};
use crate::state::explain;
use crate::state::grid::{self, COLUMN_GAP};
use crate::state::query::{
    CellView, ConfirmKind, ExplainView, Pane, ParamsForm, QueryTab, ResultTab, RunConfirm, SaveAs,
    Status,
};
use crate::state::text;

/// How each token class is coloured (the design's: red keywords, purple
/// functions, blue strings).
pub(crate) fn class_role(class: Class) -> Role {
    match class {
        Class::Keyword => Role::Deleted,
        Class::Function => Role::Name,
        Class::String => Role::Cursor,
        Class::Name => Role::Modified,
        Class::Number => Role::Header,
        Class::Comment => Role::Dim,
    }
}

/// Whether a character can't be shown as itself (`grid::clean`'s rule).
fn shown_as_replacement(c: char) -> bool {
    grid::clean(c.encode_utf8(&mut [0; 4])) != c.to_string()
}

/// One line as drawn from display column `left`, at most `width` columns:
/// runs of text with the class they're coloured with. Tabs are spaces to
/// the next stop; a character that can't be shown is `�`; a wide character
/// cut by either edge is spaces.
pub fn line_runs(
    line: &str,
    spans: &[HlSpan],
    left: usize,
    width: usize,
) -> Vec<(String, Option<Class>)> {
    let mut runs: Vec<(String, Option<Class>)> = Vec::new();
    let mut push = |text: &str, class: Option<Class>| match runs.last_mut() {
        Some((t, c)) if *c == class => t.push_str(text),
        _ => runs.push((text.to_string(), class)),
    };
    let end = left + width;
    let mut column = 0;
    let mut span = 0;
    for (byte, c) in line.char_indices() {
        if column >= end {
            break;
        }
        while span < spans.len() && spans[span].end <= byte {
            span += 1;
        }
        let class = spans
            .get(span)
            .filter(|s| s.start <= byte && byte < s.end)
            .map(|s| s.class);
        let w = char_width(c, column);
        let next = column + w;
        if next > left {
            if c == '\t' || column < left || next > end {
                let from = column.max(left);
                let to = next.min(end);
                push(&" ".repeat(to - from), class);
            } else if shown_as_replacement(c) {
                push("\u{fffd}", class);
            } else {
                push(c.encode_utf8(&mut [0; 4]), class);
            }
        }
        column = next;
    }
    runs
}

/// The query view in the main view's box.
pub fn render(model: &Model, area: Rect, frame: &mut Frame) {
    let Some(tab) = model.query.active() else {
        return;
    };
    let (editor, results) = layout::query_areas(area);
    editor_box(model, tab, editor, frame);
    results_box(model, tab, results, frame);
    // The popup goes over both boxes.
    popup(model, tab, editor, area, frame);
}

fn border(model: &Model, focused: bool) -> Style {
    if focused {
        model.theme.style(Role::Focus)
    } else {
        model.theme.style(Role::Border)
    }
}

/// A box's tabs as title spans: the active one bold (in the focus colour
/// when focused).
fn tab_spans(model: &Model, names: &[String], active: usize, focused: bool) -> Vec<Span<'static>> {
    let theme = &model.theme;
    let mut spans = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" - ", theme.style(Role::Muted)));
        }
        let style = if i == active {
            if focused {
                theme.style(Role::Focus)
            } else {
                theme.style(Role::Text).bold()
            }
        } else {
            theme.style(Role::Muted)
        };
        spans.push(Span::styled(grid::clean(name), style));
    }
    spans
}

fn editor_box(model: &Model, tab: &QueryTab, area: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let focused = model.focus == Panel::Main && model.query.pane == Pane::Editor;
    let style = border(model, focused);
    let names: Vec<String> = model.query.tabs.iter().map(|t| t.title.clone()).collect();
    let mut title = vec![Span::styled("[Q]-", style)];
    title.extend(tab_spans(model, &names, model.query.active, focused));
    title.push(Span::styled(
        format!(" - {}", text::QUERY_TABS_NEW),
        theme.style(Role::Muted),
    ));
    let editor = &tab.editor;
    let mut state = match editor.mode {
        Mode::Insert => text::MODE_INSERT.to_string(),
        Mode::Normal => text::MODE_NORMAL.to_string(),
    };
    if tab.modified {
        state.push_str(" · modified");
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Line::from(title));
    let left_width: usize = names.iter().map(|n| n.width() + 3).sum::<usize>() + 6;
    if left_width + state.width() + 3 <= usize::from(area.width) {
        block = block
            .title(Line::from(Span::styled(state, theme.style(Role::Modified))).right_aligned());
    }
    if let Some(command) = &editor.command {
        block = block.title_bottom(Line::from(Span::styled(
            format!(":{}▌", grid::clean(command)),
            theme.style(Role::Text),
        )));
    }
    frame.render_widget(block, area);

    let lines = editor.lines();
    let text_area = layout::editor_text_area(area, lines.len());
    let gutter = layout::gutter_width(lines.len());
    let height = usize::from(text_area.height);
    let width = usize::from(text_area.width);
    if height == 0 || width == 0 {
        return;
    }
    let (row, _) = editor.cursor();
    let engine = crate::state::query::editor_engine(model);
    let highlight = editor
        .highlight
        .as_ref()
        .filter(|_| editor.highlight_current(engine));
    let empty = lines.len() == 1 && lines[0].is_empty();
    let mut drawn: Vec<Line<'static>> = Vec::with_capacity(height);
    for (i, line) in lines.iter().enumerate().skip(editor.top).take(height) {
        let number_role = if i == row { Role::Text } else { Role::Dim };
        let mut spans = vec![Span::styled(
            format!("{:>w$} ", i + 1, w = usize::from(gutter) - 1),
            theme.style(number_role),
        )];
        if empty && i == 0 {
            let placeholder = tab.notice.as_deref().unwrap_or(text::EDITOR_PLACEHOLDER);
            spans.push(Span::styled(
                placeholder.to_string(),
                theme.style(Role::Dim),
            ));
        } else {
            let hl: &[HlSpan] = highlight
                .and_then(|h| h.lines.get(i))
                .map_or(&[], Vec::as_slice);
            for (text, class) in line_runs(line, hl, editor.left, width) {
                let role = class.map_or(Role::Text, class_role);
                spans.push(Span::styled(text, theme.style(role)));
            }
        }
        drawn.push(Line::from(spans));
    }
    let inner = Rect::new(
        text_area.x - gutter,
        text_area.y,
        text_area.width + gutter,
        text_area.height,
    );
    frame.render_widget(Paragraph::new(drawn), inner);

    // The cursor cell.
    if focused && editor.command.is_none() {
        let y = row.saturating_sub(editor.top);
        let x = editor.cursor_column().saturating_sub(editor.left);
        if y < height && x < width {
            let at = (text_area.x + x as u16, text_area.y + y as u16);
            let cursor = match editor.mode {
                Mode::Insert => theme.style(Role::Cursor).add_modifier(Modifier::REVERSED),
                Mode::Normal => theme
                    .style(Role::Focus)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            };
            frame.buffer_mut()[at].set_style(cursor);
        }
    }
}

/// The completion popup, under the prefix it completes.
fn popup(model: &Model, tab: &QueryTab, editor_area: Rect, bounds: Rect, frame: &mut Frame) {
    let editor = &tab.editor;
    let Some(popup) = &editor.completion else {
        return;
    };
    let lines = editor.lines();
    let text_area = layout::editor_text_area(editor_area, lines.len());
    let Some(line) = lines.get(popup.row) else {
        return;
    };
    let prefix: String = line.chars().take(popup.col).collect();
    let column = crate::state::editor::display_width(&prefix).saturating_sub(editor.left);
    let row = popup.row.saturating_sub(editor.top);
    let anchor = (
        text_area.x + column.min(usize::from(text_area.width)) as u16,
        text_area.y + row.min(usize::from(text_area.height)) as u16,
    );
    let rect = popup_area(bounds, anchor, &popup.items);
    let theme = &model.theme;
    let label_width = popup
        .items
        .iter()
        .map(|i| i.label.width())
        .max()
        .unwrap_or(0);
    let visible = usize::from(rect.height.saturating_sub(2));
    let start = (popup.selected + 1).saturating_sub(visible);
    let items: Vec<Line<'static>> = popup
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, item)| {
            let mut line = Line::from(vec![
                Span::styled(
                    grid::pad(&grid::clean(&item.label), label_width, false),
                    theme.style(Role::Text),
                ),
                Span::raw("  "),
                Span::styled(grid::clean(&item.detail), theme.style(Role::Muted)),
            ]);
            if i == popup.selected {
                line = line.style(theme.selection(true));
            }
            line
        })
        .collect();
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(theme.style(Role::Border)),
        ),
        rect,
    );
}

fn results_box(model: &Model, tab: &QueryTab, area: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let focused = model.focus == Panel::Main && model.query.pane == Pane::Results;
    let names: Vec<String> = text::RESULT_TABS.iter().map(|s| s.to_string()).collect();
    let title = tab_spans(model, &names, tab.result_tab.index(), focused);
    let with_rows: Vec<usize> = (0..tab.statements.len())
        .filter(|&i| tab.statements[i].page.is_some())
        .collect();
    let right = match tab.result_tab {
        _ if tab.op.is_some() => Some(text::RUNNING.to_string()),
        ResultTab::Results if with_rows.len() > 1 => tab.shown.and_then(|s| {
            with_rows
                .iter()
                .position(|&i| i == s)
                .map(|p| text::statement_of(p + 1, with_rows.len()))
        }),
        ResultTab::Explain => match &tab.explain {
            Some(ExplainView::Loaded(plan)) => Some(explain::header(plan)),
            _ => None,
        },
        _ => None,
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border(model, focused))
        .title(Line::from(title));
    if let Some(right) = right {
        block =
            block.title(Line::from(Span::styled(right, theme.style(Role::Muted))).right_aligned());
    }
    if tab.result_tab == ResultTab::Results {
        if let Some(page) = tab.shown_statement().and_then(|s| s.page.as_ref()) {
            if !page.rows.is_empty() {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        text::counter(tab.row.min(page.rows.len() - 1), page.rows.len()),
                        theme.style(Role::Muted),
                    ))
                    .right_aligned(),
                );
            }
        }
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match tab.result_tab {
        ResultTab::Results => results(model, tab, focused, inner, frame),
        ResultTab::Explain => explain_tab(model, tab, inner, frame),
        ResultTab::Messages => messages(model, tab, inner, frame),
    }
}

/// The result row and column drawn at `(x, y)` of the Results tab drawn
/// into `inner`, as [`results`] draws it (the mouse).
pub fn result_cell_at(
    model: &Model,
    inner: Rect,
    x: u16,
    y: u16,
) -> Option<(usize, Option<usize>)> {
    let tab = model.query.active()?;
    let statement = tab.shown_statement()?;
    let page = statement.page.as_ref()?;
    let (width, height) = (usize::from(inner.width), usize::from(inner.height));
    if height < 3 {
        return None;
    }
    // The names first, the footer last.
    let line = usize::from(y.checked_sub(inner.y)?).checked_sub(1)?;
    let body = height - 2;
    let start = (tab.row + 1).saturating_sub(body);
    let index = start + line;
    if line >= body || index >= page.rows.len() {
        return None;
    }
    let widths = &statement.widths;
    let room = width.saturating_sub(1);
    let cols = super::grid::shown_columns(widths, grid::column_window(widths, tab.col, room), room);
    let dx = usize::from(x.checked_sub(inner.x)?);
    let column = dx
        .checked_sub(1)
        .and_then(|dx| super::grid::column_at(&cols, dx));
    Some((index, column))
}

fn dim(model: &Model, text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), model.theme.style(Role::Dim)))
}

fn results(model: &Model, tab: &QueryTab, focused: bool, inner: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    let Some(statement) = tab.shown_statement() else {
        let line = if let Some(e) = &tab.run_error {
            Line::from(Span::styled(
                grid::clean(&text::apply_failed(&e.code, &e.message)),
                theme.style(Role::Deleted),
            ))
        } else if tab.op.is_some() {
            dim(model, text::RUNNING)
        } else if !tab.statements.is_empty() {
            dim(model, text::NO_ROWS_STATEMENT)
        } else {
            dim(model, text::NO_RESULTS)
        };
        frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), inner);
        return;
    };
    let Some(page) = &statement.page else {
        return;
    };
    if height < 3 {
        return;
    }
    let names: Vec<String> = page.columns.iter().map(|c| grid::clean(c)).collect();
    let widths = &statement.widths;
    let room = width.saturating_sub(1);
    let col_widths =
        super::grid::shown_columns(widths, grid::column_window(widths, tab.col, room), room);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
    let mut header = vec![Span::raw(" ")];
    for (i, (c, w)) in col_widths.iter().enumerate() {
        if i > 0 {
            header.push(Span::raw(" ".repeat(COLUMN_GAP)));
        }
        header.push(Span::styled(
            grid::pad(&names[*c], *w, false),
            theme.style(Role::Header).add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(header));
    let body = height - 2;
    if page.rows.is_empty() {
        lines.push(dim(model, text::NO_RESULT_ROWS));
    }
    let start = (tab.row + 1).saturating_sub(body);
    for (index, row) in page.rows.iter().enumerate().skip(start).take(body) {
        let selected = index == tab.row;
        let mut spans = vec![Span::raw(" ")];
        for (i, (c, w)) in col_widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(COLUMN_GAP)));
            }
            let cell = row.get(*c).unwrap_or(&seaquel_core::Value::Null);
            let role = if matches!(cell, seaquel_core::Value::Null) {
                Role::Dim
            } else {
                Role::Text
            };
            let mut style = theme.style(role);
            if selected && *c == tab.col && focused {
                style = theme
                    .style(Role::Cursor)
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD);
            }
            spans.push(Span::styled(
                grid::pad(&grid::display(cell), *w, grid::right_aligned(cell)),
                style,
            ));
        }
        let mut line = Line::from(spans);
        if selected {
            line = line.style(theme.selection(focused));
        }
        lines.push(line);
    }
    while lines.len() < height - 1 {
        lines.push(Line::default());
    }
    let mut counts = grid::range_text(page);
    if statement.capped {
        counts = format!("{} · {counts}", text::row_cap(crate::state::query::ROW_CAP));
    }
    if matches!(statement.status, Status::Done { .. }) {
        counts.push_str(&format!(" · {}", grid::elapsed_text(page.elapsed_ms)));
    }
    let column = names.get(tab.col).cloned().unwrap_or_default();
    let left = grid::fit(&column, width.saturating_sub(counts.width() + 2));
    let gap = width.saturating_sub(left.width() + counts.width());
    lines.push(Line::from(vec![
        Span::styled(left, theme.style(Role::Muted)),
        Span::raw(" ".repeat(gap)),
        Span::styled(counts, theme.style(Role::Muted)),
    ]));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn explain_tab(model: &Model, tab: &QueryTab, inner: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let width = usize::from(inner.width);
    let plan = match &tab.explain {
        None => {
            frame.render_widget(Paragraph::new(dim(model, text::EXPLAIN_HINT)), inner);
            return;
        }
        Some(ExplainView::Loading { .. }) => {
            frame.render_widget(Paragraph::new(dim(model, text::EXPLAINING)), inner);
            return;
        }
        Some(ExplainView::Failed(e)) => {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    grid::clean(&text::apply_failed(&e.code, &e.message)),
                    theme.style(Role::Deleted),
                )))
                .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        Some(ExplainView::Loaded(plan)) => plan,
    };
    let rows = explain::rows(plan);
    let analyze = plan.is_analyze;
    let headers: &[&str] = if analyze {
        &["rows", "self ms", "share of total"]
    } else {
        &["rows (est.)", "cost"]
    };
    let numbers: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let rows = r.rows.map(explain::rows_text).unwrap_or_default();
            if analyze {
                vec![
                    rows,
                    r.self_ms.map(|v| format!("{v:.1}")).unwrap_or_default(),
                    r.share.map(|v| format!("{v:.0}%")).unwrap_or_default(),
                ]
            } else {
                vec![rows, r.cost.map(|v| format!("{v:.1}")).unwrap_or_default()]
            }
        })
        .collect();
    let number_widths: Vec<usize> = (0..headers.len())
        .map(|i| {
            numbers
                .iter()
                .map(|n| n[i].width())
                .chain([headers[i].width()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let numbers_width: usize = number_widths.iter().map(|w| w + 2).sum();
    let node_width = width.saturating_sub(numbers_width + 1).max(4);
    let mut lines = Vec::new();
    let mut header = vec![Span::styled(
        grid::pad("node", node_width, false),
        theme.style(Role::Muted),
    )];
    for (h, w) in headers.iter().zip(&number_widths) {
        header.push(Span::raw("  "));
        header.push(Span::styled(
            grid::pad(h, *w, true),
            theme.style(Role::Muted),
        ));
    }
    lines.push(Line::from(header));
    for (r, n) in rows.iter().zip(&numbers) {
        let mut spans = vec![
            Span::styled(r.prefix.clone(), theme.style(Role::Dim)),
            Span::styled(
                grid::pad(&r.label, node_width.saturating_sub(r.prefix.width()), false),
                theme.style(Role::Text),
            ),
        ];
        for (i, (value, w)) in n.iter().zip(&number_widths).enumerate() {
            spans.push(Span::raw("  "));
            let role = if analyze && i == 2 && r.share.is_some_and(|s| s >= 50.0) {
                Role::Warning
            } else {
                Role::Text
            };
            spans.push(Span::styled(grid::pad(value, *w, true), theme.style(role)));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn messages(model: &Model, tab: &QueryTab, inner: Rect, frame: &mut Frame) {
    let theme = &model.theme;
    let width = usize::from(inner.width);
    let mut lines = Vec::new();
    if let Some(e) = &tab.run_error {
        lines.push(Line::from(Span::styled(
            grid::clean(&text::apply_failed(&e.code, &e.message)),
            theme.style(Role::Deleted),
        )));
    }
    if tab.statements.is_empty() && tab.run_error.is_none() {
        lines.push(dim(model, text::NO_RESULTS));
    }
    for s in &tab.statements {
        let (outcome, role) = match &s.status {
            Status::Running => (text::RUNNING.to_string(), Role::Muted),
            Status::Cancelled => (text::CANCELLED.to_string(), Role::Muted),
            Status::Failed(e) => (text::apply_failed(&e.code, &e.message), Role::Deleted),
            Status::Done {
                elapsed_ms,
                rows_affected,
            } => {
                let what = match (rows_affected, &s.page) {
                    (Some(n), _) => text::rows_affected(*n),
                    (None, Some(page)) => {
                        text::rows_returned(page.total_rows.max(page.rows.len() as u64))
                    }
                    (None, None) => "done".to_string(),
                };
                let what = if s.capped {
                    format!(
                        "{}; {}",
                        text::row_cap(crate::state::query::ROW_CAP),
                        text::NO_HISTORY_ROW
                    )
                } else {
                    what
                };
                (
                    format!("{what} · {}", grid::elapsed_text(*elapsed_ms)),
                    Role::Added,
                )
            }
        };
        let number = format!("{:>2}  ", s.index + 1);
        let outcome = grid::clean(&outcome);
        let sql_room = width.saturating_sub(number.width() + outcome.width() + 3);
        let sql = grid::fit(
            &grid::clean(&s.sql.split_whitespace().collect::<Vec<_>>().join(" ")),
            sql_room.max(8),
        );
        lines.push(Line::from(vec![
            Span::styled(number, theme.style(Role::Dim)),
            Span::styled(sql, theme.style(Role::Text)),
            Span::styled("  ", Style::new()),
            Span::styled(outcome, theme.style(role)),
        ]));
    }
    if let Some((statements, succeeded)) = tab.finished {
        if statements > 0 {
            let role = if succeeded {
                Role::Muted
            } else {
                Role::Warning
            };
            let ran = format!(
                "{statements} statement{} · {}",
                if statements == 1 { "" } else { "s" },
                if succeeded {
                    "all ran"
                } else {
                    "not all ran (history records a run only when all succeed)"
                }
            );
            lines.push(Line::from(Span::styled(ran, theme.style(role))));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// A dialog's box: centred, `width` × `height` at most, the warning colour
/// for the run confirmation.
fn dialog_block(model: &Model, title: &str, role: Role) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.style(role))
        .title(Span::styled(
            format!(" {title} "),
            model.theme.style(role).add_modifier(Modifier::BOLD),
        ))
}

fn draw_dialog(
    model: &Model,
    title: &str,
    role: Role,
    lines: Vec<Line<'static>>,
    width: u16,
    frame: &mut Frame,
) {
    let area = frame.area();
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = layout::dialog_area(area, width.min(area.width.saturating_sub(2)), height);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .style(model.theme.style(Role::Text))
            .wrap(Wrap { trim: false })
            .block(dialog_block(model, title, role)),
        rect,
    );
}

pub fn params(model: &Model, form: &ParamsForm, frame: &mut Frame) {
    let theme = &model.theme;
    let label = form.names.iter().map(|n| n.width()).max().unwrap_or(0);
    let mut lines = vec![Line::default()];
    for (i, (name, value)) in form.names.iter().zip(&form.values).enumerate() {
        let active = i == form.field;
        let value = if active {
            format!("{}▌", grid::clean(value))
        } else {
            grid::clean(value)
        };
        let style = if active {
            theme.style(Role::Focus)
        } else {
            theme.style(Role::Text)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} › ", grid::pad(&grid::clean(name), label, false)),
                theme.style(Role::Name),
            ),
            Span::styled(value, style),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(" {}", text::PARAMS_HINT),
        theme.style(Role::Muted),
    )));
    draw_dialog(model, text::PARAMS_TITLE, Role::Focus, lines, 64, frame);
}

pub fn confirm(model: &Model, confirm: &RunConfirm, frame: &mut Frame) {
    let theme = &model.theme;
    let mut lines = vec![Line::default()];
    let title = match &confirm.kind {
        ConfirmKind::Destructive { list, total, .. } => {
            lines.push(Line::from(format!(
                " {}",
                text::destructive_question(*total)
            )));
            for d in list.iter().take(10) {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(
                            "  {}",
                            grid::fit(
                                &grid::clean(
                                    &d.sql.split_whitespace().collect::<Vec<_>>().join(" ")
                                ),
                                50
                            )
                        ),
                        theme.style(Role::Text),
                    ),
                    Span::styled(format!("  {}", d.reason), theme.style(Role::Deleted)),
                ]));
            }
            let listed = list.len().min(10) as u32;
            if *total > listed {
                lines.push(Line::from(Span::styled(
                    format!("  {}", text::and_more(*total - listed)),
                    theme.style(Role::Muted),
                )));
            }
            text::RUN_CONFIRM_TITLE
        }
        ConfirmKind::Analyze { verb } => {
            lines.push(Line::from(format!(" {}", text::analyze_question(verb))));
            text::ANALYZE_TITLE
        }
    };
    if crate::state::query::prod(model) {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(" {}", text::PROD_WARNING),
            theme.style(Role::Warning),
        )));
        lines.push(Line::from(vec![
            Span::styled(format!(" {}", text::TYPE_PROD), theme.style(Role::Text)),
            Span::styled(
                format!("{}▌", grid::clean(&confirm.typed)),
                theme.style(Role::Focus),
            ),
        ]));
    }
    draw_dialog(model, title, Role::Warning, lines, 84, frame);
}

pub fn save_as(model: &Model, save: &SaveAs, frame: &mut Frame) {
    let theme = &model.theme;
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(format!(" {}", text::SAVE_AS_LABEL), theme.style(Role::Text)),
            Span::styled(
                format!("{}▌", grid::clean(&save.name)),
                theme.style(Role::Focus),
            ),
        ]),
    ];
    if let Some(error) = &save.error {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(" {}", grid::clean(error)),
            theme.style(Role::Deleted),
        )));
    }
    draw_dialog(model, text::SAVE_AS_TITLE, Role::Focus, lines, 60, frame);
}

pub fn cell(model: &Model, cell: &CellView, frame: &mut Frame) {
    let area = frame.area();
    let rect = layout::help_area(area);
    let lines: Vec<Line<'static>> = cell
        .text
        .split('\n')
        .skip(cell.scroll)
        .map(|l| Line::from(grid::clean(l)))
        .collect();
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .style(model.theme.style(Role::Text))
            .wrap(Wrap { trim: false })
            .block(dialog_block(model, &grid::clean(&cell.column), Role::Focus)),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(runs: &[(String, Option<Class>)]) -> String {
        runs.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn lines_expand_tabs_replace_controls_and_scroll() {
        let runs = line_runs("a\tb", &[], 0, 20);
        assert_eq!(text_of(&runs), "a   b");
        let runs = line_runs("x\u{1b}y\u{202e}z", &[], 0, 20);
        assert_eq!(text_of(&runs), "x\u{fffd}y\u{fffd}z");
        assert_eq!(text_of(&line_runs("0123456789", &[], 3, 4)), "3456");
        // A wide character cut by the left edge is a space.
        assert_eq!(text_of(&line_runs("日本語", &[], 1, 4)), " 本 ");
        let spans = [HlSpan {
            start: 0,
            end: 6,
            class: Class::Keyword,
        }];
        let runs = line_runs("SELECT 1", &spans, 0, 20);
        assert_eq!(
            runs,
            [
                ("SELECT".to_string(), Some(Class::Keyword)),
                (" 1".to_string(), None)
            ]
        );
    }
}
