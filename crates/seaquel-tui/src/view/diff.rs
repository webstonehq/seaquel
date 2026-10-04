//! The main view over panel 4 (design 1c): the selected change as a diff
//! of its row (`-`/`+` for the changed cell, the whole row red for a
//! delete, green for an insert), and the SQL tab with Core's plan and its
//! values. Drawn from the model only; the TUI writes no SQL of its own.

use ratatui::text::{Line, Span};
use seaquel_core::Value;
use unicode_width::UnicodeWidthStr;

use super::panels::fit;
use super::theme::Role;
use crate::state::app::Model;
use crate::state::browse::key_text;
use crate::state::commit;
use crate::state::grid::{clean, display};
use crate::state::pending::{Entry, Plan, Staging};
use crate::state::text;

fn table(entry: &Entry) -> String {
    format!("{}.{}", entry.target.schema, entry.target.table)
}

/// The right-hand title: `public.items · id 12`.
pub fn title(model: &Model) -> Option<String> {
    let entry = commit::selected(model)?;
    Some(match entry.staging.key() {
        Some(key) => format!("{} · {}", table(entry), key_text(key)),
        None => format!("{} · {}", table(entry), text::NEW_ROW),
    })
}

/// The failure an apply marked the entry with, as the first lines.
fn failure_lines(model: &Model, entry: &Entry) -> Vec<Line<'static>> {
    match model.queue.failure(&entry.id) {
        Some(e) => vec![
            Line::from(Span::styled(
                clean(&format!("! {}: {}", e.code, e.message)),
                model.theme.style(Role::Deleted),
            )),
            Line::default(),
        ],
        None => Vec::new(),
    }
}

/// The Diff tab's lines (`None`: nothing staged).
pub fn diff(model: &Model, width: usize) -> Option<Vec<Line<'static>>> {
    let theme = &model.theme;
    let entry = commit::selected(model)?;
    let mut lines = failure_lines(model, entry);
    let what = match &entry.staging {
        Staging::Update { key, .. } | Staging::SetDefault { key, .. } => {
            format!("{} · {} (1 column)", table(entry), key_text(key))
        }
        Staging::Delete { key } => format!("{} · {} (delete)", table(entry), key_text(key)),
        Staging::Insert { .. } => format!("{} ({})", table(entry), text::NEW_ROW),
    };
    lines.push(Line::from(Span::styled(
        fit(&format!("@@ {what} @@"), width),
        theme.style(Role::Header),
    )));
    let pairs: Vec<(String, Value)> = match &entry.staging {
        Staging::Insert { values } => values.clone(),
        _ => entry.row.as_ref().clone(),
    };
    let name_width = pairs
        .iter()
        .map(|(c, _)| clean(c).width())
        .max()
        .unwrap_or(0)
        .min(32);
    let cell = |mark: &str, column: &str, value: String, role: Role| {
        let name = fit(column, name_width);
        let pad = name_width.saturating_sub(name.width());
        let room = width.saturating_sub(name_width + 4);
        Line::from(Span::styled(
            format!("{mark} {name}{}  {}", " ".repeat(pad), fit(&value, room)),
            theme.style(role),
        ))
    };
    for (column, value) in &pairs {
        match &entry.staging {
            Staging::Update {
                column: changed,
                value: new,
                ..
            } if changed == column => {
                lines.push(cell("-", column, display(value), Role::Deleted));
                lines.push(cell("+", column, display(new), Role::Added));
            }
            Staging::SetDefault {
                column: changed, ..
            } if changed == column => {
                lines.push(cell("-", column, display(value), Role::Deleted));
                lines.push(cell(
                    "+",
                    column,
                    text::DIFF_DEFAULT.to_string(),
                    Role::Added,
                ));
            }
            Staging::Delete { .. } => {
                lines.push(cell("-", column, display(value), Role::Deleted));
            }
            Staging::Insert { .. } => {
                lines.push(cell("+", column, display(value), Role::Added));
            }
            _ => lines.push(cell(" ", column, display(value), Role::Muted)),
        }
    }
    Some(lines)
}

/// The SQL tab's lines.
pub fn sql(model: &Model) -> Option<Vec<Line<'static>>> {
    let theme = &model.theme;
    let entry = commit::selected(model)?;
    let mut lines = failure_lines(model, entry);
    let dim = |t: &str| Line::from(Span::styled(t.to_string(), theme.style(Role::Dim)));
    match &entry.plan {
        Plan::Planned(plan) => {
            lines.push(dim(text::PLANNED_BY_CORE));
            lines.extend(
                plan.sql
                    .lines()
                    .map(|l| Line::from(Span::styled(clean(l), theme.style(Role::Text)))),
            );
            if !plan.params.is_empty() {
                lines.push(Line::default());
                let engine = crate::state::commit::queue_engine(model);
                for (i, value) in plan.params.iter().enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{} = ", super::commit::placeholder(&engine, i + 1)),
                            theme.style(Role::Muted),
                        ),
                        Span::styled(display(value), theme.style(Role::Text)),
                    ]));
                }
            }
        }
        Plan::Planning => lines.push(dim(text::PLANNING)),
        Plan::Unplanned => lines.push(dim(text::NOT_PLANNED_YET)),
        Plan::Empty => lines.push(dim(text::EMPTY_INSERT)),
    }
    Some(lines)
}
