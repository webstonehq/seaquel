//! Ask AI's popup (Task 7; design 1d): `Ask AI` and the sharing line on its
//! top border, the request (with the `@` list under it), then the wait, the
//! error, or the status line and the generated SQL highlighted with the
//! connection's engine. Its keys are on the key bar (Decision 9). Drawn
//! from the model only; nothing here is logged.

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::layout;
use super::query::{class_role, line_runs};
use super::theme::Role;
use crate::state::app::Model;
use crate::state::ask::{self, Ask, Stage};
use crate::state::editor::{char_width, highlight, HlSpan};
use crate::state::grid;
use crate::state::text;

/// The popup's widest.
const WIDTH: u16 = 100;
/// The most `@` items listed at once.
const MENTION_ROWS: usize = 6;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// `text` in lines at most `width` columns wide, broken after a space
/// where it can be (control characters already replaced).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut column = 0;
    for word in text.split_inclusive(' ') {
        let w = word.trim_end_matches(' ').width();
        if column > 0 && column + w > width {
            out.push(std::mem::take(&mut line));
            column = 0;
        }
        for c in word.chars() {
            let cw = char_width(c, column);
            if column > 0 && column + cw > width && c != ' ' {
                out.push(std::mem::take(&mut line));
                column = 0;
            }
            line.push(c);
            column += cw;
        }
    }
    out.push(line);
    out
}

/// Draws the popup over the query view.
pub fn render(model: &Model, ask: &Ask, frame: &mut Frame) {
    let theme = &model.theme;
    let area = frame.area();
    let width = WIDTH.min(area.width.saturating_sub(2));
    let inner_width = usize::from(width.saturating_sub(4));
    let mut lines: Vec<Line<'static>> = vec![Line::default()];

    // The request.
    let label = if ask.refining {
        text::ASK_REFINE
    } else {
        text::ASK_PROMPT
    };
    let typing = ask.stage == Stage::Prompt;
    if ask.prompt.is_empty() && typing {
        lines.push(Line::from(vec![
            Span::styled(format!(" {label}"), theme.style(Role::Focus)),
            Span::styled("▌", theme.style(Role::Focus)),
            Span::styled(text::ASK_PLACEHOLDER, theme.style(Role::Dim)),
        ]));
    } else {
        let shown = format!(
            "{label}{}{}",
            grid::clean(&ask.prompt),
            if typing { "▌" } else { "" }
        );
        let role = if typing { Role::Text } else { Role::Muted };
        for (i, piece) in wrap(&shown, inner_width).into_iter().enumerate() {
            let piece = piece.trim_end().to_string();
            let mut spans = vec![Span::raw(" ")];
            if i == 0 {
                let rest = piece.strip_prefix(label).unwrap_or(&piece).to_string();
                spans.push(Span::styled(label, theme.style(Role::Focus)));
                spans.push(Span::styled(rest, theme.style(role)));
            } else {
                spans.push(Span::styled(piece, theme.style(role)));
            }
            lines.push(Line::from(spans));
        }
    }

    // The `@` list.
    if ask.mention.is_some() {
        let items = ask::mention_items(model);
        let selected = ask.mention.as_ref().map_or(0, |m| m.selected);
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("   {}", text::ASK_NO_MENTIONS),
                theme.style(Role::Dim),
            )));
        }
        let label_width = items.iter().map(|i| i.label.width()).max().unwrap_or(0);
        let start = (selected + 1).saturating_sub(MENTION_ROWS);
        for (i, item) in items.iter().enumerate().skip(start).take(MENTION_ROWS) {
            let mut line = Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    grid::pad(&grid::clean(&item.label), label_width, false),
                    theme.style(Role::Text),
                ),
                Span::raw("  "),
                Span::styled(item.detail.clone(), theme.style(Role::Muted)),
            ]);
            if i == selected {
                line = line.style(theme.selection(true));
            }
            lines.push(line);
        }
    }
    lines.push(Line::default());

    // What happened.
    if let Some(error) = &ask.error {
        for piece in wrap(&grid::clean(error), inner_width)
            .iter()
            .map(|p| p.trim_end())
        {
            lines.push(Line::from(Span::styled(
                format!(" {piece}"),
                theme.style(Role::Deleted),
            )));
        }
    }
    match ask.stage {
        Stage::Waiting { since } => {
            let frame_no = match (model.now, since) {
                (Some(now), Some(since)) => {
                    (now.saturating_duration_since(since).as_millis() / 100) as usize
                }
                _ => 0,
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", SPINNER[frame_no % SPINNER.len()]),
                    theme.style(Role::Focus),
                ),
                Span::styled(text::ASK_WAITING, theme.style(Role::Muted)),
            ]));
        }
        Stage::Answer | Stage::Prompt => {
            if let Some(answer) = &ask.answer {
                lines.push(Line::from(Span::styled(
                    format!(
                        " {}",
                        text::ask_generated(answer.elapsed_ms, answer.model.as_deref())
                    ),
                    theme.style(Role::Added),
                )));
                // The rows left for the SQL, the note and the borders.
                let used = lines.len() + 2 + usize::from(ask.note.is_some()) * 2;
                let room = usize::from(area.height.saturating_sub(2)).saturating_sub(used);
                sql_lines(model, &answer.sql, inner_width, room, &mut lines);
            }
        }
    }
    if let Some(note) = &ask.note {
        lines.push(Line::default());
        for piece in wrap(&grid::clean(note), inner_width)
            .iter()
            .map(|p| p.trim_end())
        {
            lines.push(Line::from(Span::styled(
                format!(" {piece}"),
                theme.style(Role::Warning),
            )));
        }
    }

    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = layout::dialog_area(area, width, height);
    let title = Span::styled(
        format!(" {} ", text::ASK_TITLE),
        theme.style(Role::Added).add_modifier(Modifier::BOLD),
    );
    let sharing = format!(" {} ", ask::sharing_line(model));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme.style(Role::Added))
        .title(Line::from(title));
    if text::ASK_TITLE.width() + sharing.width() + 6 <= usize::from(rect.width) {
        block = block
            .title(Line::from(Span::styled(sharing, theme.style(Role::Muted))).right_aligned());
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.style(Role::Text))
            .block(block),
        Rect::new(rect.x, rect.y, rect.width, rect.height),
    );
}

/// The SQL, numbered and highlighted as the editor is, cut at `width`
/// columns and `room` lines (the last says how many more there are).
fn sql_lines(model: &Model, sql: &str, width: usize, room: usize, out: &mut Vec<Line<'static>>) {
    let theme = &model.theme;
    let engine = crate::state::query::editor_engine(model);
    let hl = highlight(sql, engine);
    let all: Vec<&str> = sql.split('\n').collect();
    let gutter = all.len().to_string().len();
    let room = room.max(1);
    let shown = if all.len() > room {
        room - 1
    } else {
        all.len()
    };
    for (i, line) in all.iter().take(shown).enumerate() {
        let mut spans = vec![Span::styled(
            format!(" {:>gutter$} ", i + 1),
            theme.style(Role::Dim),
        )];
        let line = line.strip_suffix('\r').unwrap_or(line);
        let spans_of: &[HlSpan] = hl.get(i).map_or(&[], Vec::as_slice);
        for (text, class) in line_runs(line, spans_of, 0, width.saturating_sub(gutter + 2)) {
            let role = class.map_or(Role::Text, class_role);
            spans.push(Span::styled(text, theme.style(role)));
        }
        out.push(Line::from(spans));
    }
    if shown < all.len() {
        out.push(Line::from(Span::styled(
            format!(" … {} more lines", all.len() - shown),
            theme.style(Role::Dim),
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::wrap;

    #[test]
    fn the_request_wraps_after_a_space_and_a_long_word_is_cut() {
        assert_eq!(
            wrap("only @invoices with status", 15),
            ["only @invoices ", "with status"]
        );
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 4), [""]);
    }
}
