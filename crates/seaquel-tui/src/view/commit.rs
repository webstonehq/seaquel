//! The commit dialog (design 1c), the discard question, the staged
//! changes' switch question and the value edit. Their keys are on the key
//! bar, generated from the keymap.

use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::dialogs::draw_box;
use super::panels::fit;
use super::theme::Role;
use crate::state::app::Model;
use crate::state::browse::key_text;
use crate::state::commit::{self, CommitDialog, QueueSwitch, ValueEdit};
use crate::state::grid::{clean, display};
use crate::state::pending::{Plan, Staging};
use crate::state::text;

/// The dialog's width.
const WIDTH: u16 = 76;

fn connection_name(model: &Model, id: &str) -> String {
    model
        .library
        .connection(id)
        .map_or(id.to_string(), |c| c.name.clone())
}

/// How many destructive statements the dialog lists before "…and N more".
const DESTRUCTIVE_SHOWN: usize = 10;

/// `lines` cut to `room`, the last kept line saying how many more there
/// were (`…N more`).
fn cap(model: &Model, mut lines: Vec<Line<'static>>, room: usize) -> Vec<Line<'static>> {
    if lines.len() <= room {
        return lines;
    }
    if room == 0 {
        return Vec::new();
    }
    let more = lines.len() - (room - 1);
    lines.truncate(room - 1);
    lines.push(Line::from(Span::styled(
        text::more(more),
        model.theme.style(Role::Muted),
    )));
    lines
}

/// The commit dialog: the counts (or the preview) on top, cut
/// to fit; the destructive list (the first 10) and the `prod` field below,
/// always shown.
pub fn commit(model: &Model, dialog: &CommitDialog, frame: &mut Frame) {
    let theme = &model.theme;
    let area = frame.area();
    let inner = usize::from(WIDTH.min(area.width.saturating_sub(2)).saturating_sub(4));
    let sendable: Vec<_> = model
        .queue
        .entries()
        .iter()
        .filter(|e| e.edit().is_some())
        .collect();
    let id = model
        .queue
        .connection()
        .or(model.conn.id())
        .unwrap_or_default();
    let mut top = Vec::new();
    if dialog.preview {
        // Core's statements, each with its values.
        let engine = commit::queue_engine(model);
        for entry in &sendable {
            match &entry.plan {
                Plan::Planned(plan) => {
                    top.push(Line::from(Span::styled(
                        fit(&one_line(&plan.sql), inner),
                        theme.style(Role::Text),
                    )));
                    let values: Vec<String> = plan
                        .params
                        .iter()
                        .enumerate()
                        .map(|(i, v)| format!("{} = {}", placeholder(&engine, i + 1), display(v)))
                        .collect();
                    if !values.is_empty() {
                        top.push(Line::from(Span::styled(
                            fit(&format!("  {}", values.join(", ")), inner),
                            theme.style(Role::Muted),
                        )));
                    }
                }
                _ => top.push(Line::from(Span::styled(
                    text::PLANNING,
                    theme.style(Role::Dim),
                ))),
            }
        }
    } else {
        top.push(Line::from(Span::raw(fit(
            &text::run_on(&connection_name(model, id), sendable.len() > 1),
            inner,
        ))));
        for k in commit::kind_lines(model) {
            let role = match k.sign {
                '+' => Role::Added,
                '-' => Role::Deleted,
                _ => Role::Modified,
            };
            top.push(Line::from(vec![
                Span::styled(format!("{}{}  ", k.sign, k.count), theme.style(role)),
                Span::styled(format!("{}  ", k.verb), theme.style(role).bold()),
                Span::styled(
                    fit(&k.tables.join(", "), inner.saturating_sub(12)),
                    theme.style(Role::Name),
                ),
            ]));
        }
    }
    let mut bottom = Vec::new();
    let destructive = commit::destructive(model);
    if !destructive.is_empty() {
        bottom.push(Line::default());
        bottom.push(Line::from(Span::styled(
            text::DESTRUCTIVE_HEADER,
            theme.style(Role::Warning),
        )));
        for d in destructive.iter().take(DESTRUCTIVE_SHOWN) {
            bottom.push(Line::from(Span::styled(
                fit(&format!("  {} · {}", d.reason, one_line(&d.sql)), inner),
                theme.style(Role::Deleted),
            )));
        }
        let total = dialog
            .from_core
            .as_ref()
            .map_or(destructive.len() as u32, |(_, total)| *total);
        let shown = destructive.len().min(DESTRUCTIVE_SHOWN) as u32;
        if total > shown {
            bottom.push(Line::from(Span::styled(
                text::and_more(total - shown),
                theme.style(Role::Muted),
            )));
        }
    }
    let planning = sendable.iter().filter(|e| e.plan == Plan::Planning).count();
    let unplanned = sendable
        .iter()
        .filter(|e| e.plan == Plan::Unplanned)
        .count();
    if planning > 0 {
        bottom.push(Line::default());
        bottom.push(Line::from(Span::styled(
            fit(&text::planning(planning), inner),
            theme.style(Role::Dim),
        )));
    } else if unplanned > 0 {
        bottom.push(Line::default());
        bottom.push(Line::from(Span::styled(
            fit(&text::not_planned(unplanned), inner),
            theme.style(Role::Deleted),
        )));
    }
    if commit::prod(model) {
        let deletes = sendable
            .iter()
            .any(|e| matches!(e.staging, Staging::Delete { .. }));
        bottom.push(Line::default());
        bottom.push(Line::from(Span::styled(
            fit(
                if deletes {
                    text::PROD_WARNING_DELETE
                } else {
                    text::PROD_WARNING
                },
                inner,
            ),
            theme.style(Role::Warning),
        )));
        bottom.push(Line::from(vec![
            Span::styled(text::TYPE_PROD, theme.style(Role::Muted)),
            Span::styled(clean(&dialog.typed), theme.style(Role::Text)),
            Span::styled("▌", theme.style(Role::Cursor)),
        ]));
    }
    // The box is at most the screen less 2 rows, its border 2 more.
    let room = usize::from(area.height.saturating_sub(4)).saturating_sub(bottom.len());
    let mut lines = cap(model, top, room);
    lines.extend(bottom);
    draw_box(
        model,
        frame,
        text::commit_title(sendable.len()),
        if commit::prod(model) {
            Role::Warning
        } else {
            Role::Focus
        },
        WIDTH,
        lines,
    );
}

fn one_line(sql: &str) -> String {
    clean(&sql.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// How a statement's `n`th value is named on the connection's engine.
pub(super) fn placeholder(engine: &str, n: usize) -> String {
    match engine {
        "postgres" => format!("${n}"),
        "mssql" => format!("@P{n}"),
        _ => format!("?{n}"),
    }
}

pub fn discard(model: &Model, frame: &mut Frame) {
    draw_box(
        model,
        frame,
        text::DISCARD_TITLE.to_string(),
        Role::Warning,
        60,
        vec![Line::from(text::discard_question(
            model.queue.entries().len(),
        ))],
    );
}

/// "Commit again?": the last commit lost its connection.
pub fn recommit(model: &Model, frame: &mut Frame) {
    draw_box(
        model,
        frame,
        text::RECOMMIT_TITLE.to_string(),
        Role::Warning,
        64,
        vec![Line::from(text::RECOMMIT_QUESTION)],
    );
}

pub fn switch(model: &Model, question: &QueueSwitch, frame: &mut Frame) {
    let mut lines = vec![Line::from(text::staged_on(
        model.queue.entries().len(),
        &connection_name(model, &question.from),
    ))];
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        match &question.to {
            Some(to) => text::switch_choices(&connection_name(model, to)),
            None => text::STAGING_ELSEWHERE.to_string(),
        },
        model.theme.style(Role::Muted),
    )));
    draw_box(
        model,
        frame,
        text::SWITCH_TITLE.to_string(),
        Role::Warning,
        64,
        lines,
    );
}

pub fn value(model: &Model, edit: &ValueEdit, frame: &mut Frame) {
    let theme = &model.theme;
    let Some(entry) = model.queue.entry(&edit.id) else {
        return;
    };
    let (column, key) = match &entry.staging {
        Staging::Update { key, column, .. } | Staging::SetDefault { key, column } => {
            (column.clone(), key_text(key))
        }
        _ => return,
    };
    let lines = vec![
        Line::from(Span::styled(
            clean(&format!(
                "{}.{} · {key}",
                entry.target.schema, entry.target.table
            )),
            theme.style(Role::Muted),
        )),
        Line::default(),
        Line::from(vec![
            Span::styled("> ", theme.style(Role::Muted)),
            Span::styled(clean(&edit.text), theme.style(Role::Text)),
            Span::styled("▌", theme.style(Role::Cursor)),
        ]),
    ];
    draw_box(
        model,
        frame,
        text::edit_value_title(&column),
        Role::Focus,
        64,
        lines,
    );
}
