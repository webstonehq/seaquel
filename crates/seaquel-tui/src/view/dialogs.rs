//! The connect dialogs (Decision 16) and the keychain wait box. Their keys
//! are on the key bar, generated from the keymap, so no box carries a
//! hand-written footer. A typed password is drawn as `•`s only.

use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::layout;
use super::panels::fit;
use super::theme::Role;
use crate::state::app::Model;
use crate::state::dialogs::{Notice, PasswordPrompt, Problem, TrustPrompt};
use crate::state::picker::{Picker, Stage};
use crate::state::secrets::SecretKind;
use crate::state::text;

/// A centred box `width` wide, tall enough for `lines` wrapped in it.
pub(super) fn draw_box(
    model: &Model,
    frame: &mut Frame,
    title: String,
    border: Role,
    width: u16,
    lines: Vec<Line<'static>>,
) {
    let area = frame.area();
    let width = width.min(area.width.saturating_sub(2)).max(20);
    let inner = usize::from(width.saturating_sub(4)).max(1);
    let rows: usize = lines.iter().map(|l| l.width().div_ceil(inner).max(1)).sum();
    let height = (rows as u16 + 2).min(area.height.saturating_sub(2));
    let rect = layout::dialog_area(area, width, height);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.style(border))
        .title(Span::styled(
            format!(" {title} "),
            model.theme.style(border),
        ));
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .style(model.theme.style(Role::Text))
            .wrap(Wrap { trim: false })
            .block(block.padding(ratatui::widgets::Padding::horizontal(1))),
        rect,
    );
}

pub fn picker(model: &Model, p: &Picker, frame: &mut Frame) {
    let theme = &model.theme;
    let area = frame.area();
    let width = 60.min(area.width.saturating_sub(4));
    let inner = usize::from(width.saturating_sub(4));
    let (title, items): (String, Vec<(String, String)>) = match &p.stage {
        Stage::Projects => (
            text::PICKER_PROJECTS.to_string(),
            model
                .library
                .projects
                .iter()
                .map(|p| (p.name.clone(), String::new()))
                .collect(),
        ),
        Stage::Connections { project_id } => (
            format!(
                "{} · {}",
                text::PICKER_CONNECTIONS,
                model
                    .library
                    .project(project_id)
                    .map_or(project_id.as_str(), |p| p.name.as_str())
            ),
            model
                .library
                .connections_of(project_id)
                .map(|c| {
                    let mut place = format!("{} · {}", c.engine_label(), c.place());
                    if c.tunnel.is_some() {
                        place.push_str(" · ssh");
                    }
                    (c.name.clone(), place)
                })
                .collect(),
        ),
    };
    let visible = usize::from(area.height.saturating_sub(6)).max(1);
    let mut lines = Vec::new();
    if items.is_empty() {
        lines.push(Line::from(Span::styled(
            match p.stage {
                Stage::Projects => text::NO_PROJECTS,
                Stage::Connections { .. } => text::NO_CONNECTIONS,
            },
            theme.style(Role::Dim),
        )));
    }
    let start = (p.selected + 1).saturating_sub(visible);
    for (i, (name, detail)) in items.iter().enumerate().skip(start).take(visible) {
        let detail = fit(detail, inner / 2);
        let name = fit(name, inner.saturating_sub(detail.chars().count() + 1));
        let gap = inner.saturating_sub(name.chars().count() + detail.chars().count());
        let mut line = Line::from(vec![
            Span::styled(name, theme.style(Role::Text)),
            Span::raw(" ".repeat(gap)),
            Span::styled(detail, theme.style(Role::Muted)),
        ]);
        if i == p.selected {
            line = line.style(theme.selection(true));
        }
        lines.push(line);
    }
    draw_box(model, frame, title, Role::Focus, width, lines);
}

pub fn password(model: &Model, prompt: &PasswordPrompt, frame: &mut Frame) {
    let theme = &model.theme;
    let what = match prompt.kind {
        SecretKind::Db => text::PASSWORD_TITLE_DB,
        SecretKind::Ssh => text::PASSWORD_TITLE_SSH,
        SecretKind::SshKey => text::PASSWORD_TITLE_SSH_KEY,
    };
    let name = model
        .library
        .connection(&prompt.pending.connection_id)
        .map_or(prompt.pending.connection_id.as_str(), |c| c.name.as_str());
    let mut lines = vec![Line::from(text::password_for(what, name))];
    if let Some(reason) = prompt.reason {
        lines.push(Line::from(Span::styled(reason, theme.style(Role::Warning))));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("> ", theme.style(Role::Muted)),
        Span::styled(prompt.input.masked(), theme.style(Role::Text)),
        Span::styled("▏", theme.style(Role::Cursor)),
    ]));
    lines.push(Line::default());
    if prompt.can_save {
        let check = if prompt.save { "[x]" } else { "[ ]" };
        lines.push(Line::from(Span::styled(
            format!("{check} {}", text::SAVE_PASSWORD),
            theme.style(if prompt.save { Role::Focus } else { Role::Text }),
        )));
        lines.push(Line::from(Span::styled(
            text::save_password_hint(model.store),
            theme.style(Role::Dim),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("[-] {}", text::SAVE_PASSWORD_OFF),
            theme.style(Role::Dim),
        )));
    }
    draw_box(model, frame, what.to_string(), Role::Focus, 64, lines);
}

pub fn trust(model: &Model, prompt: &TrustPrompt, frame: &mut Frame) {
    let theme = &model.theme;
    let lines = vec![
        Line::from(Span::styled(
            format!("{}:{}", prompt.host, prompt.port),
            theme.style(Role::Name).bold(),
        )),
        Line::default(),
        Line::from(text::TRUST_BODY),
        Line::default(),
        Line::from(Span::styled(
            prompt.fingerprint.clone(),
            theme.style(Role::Cursor),
        )),
    ];
    draw_box(
        model,
        frame,
        text::TRUST_TITLE.to_string(),
        Role::Warning,
        64,
        lines,
    );
}

pub fn problem(model: &Model, p: &Problem, frame: &mut Frame) {
    let theme = &model.theme;
    let mut lines: Vec<Line<'static>> = p
        .message
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        p.code.clone(),
        theme.style(Role::Dim),
    )));
    draw_box(model, frame, p.title.to_string(), Role::Deleted, 64, lines);
}

pub fn notice(model: &Model, n: &Notice, frame: &mut Frame) {
    draw_box(
        model,
        frame,
        text::APP_NAME.to_string(),
        Role::Focus,
        60,
        vec![Line::from(n.0.clone())],
    );
}

/// "Waiting for the keychain", over everything while a connect or a save
/// waits on a keychain dialog.
pub fn keychain(model: &Model, frame: &mut Frame) {
    let mut lines = vec![Line::from(text::keychain_body(model.store))];
    if model.saving.is_some() && model.conn.core_id().is_some() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            text::KEYCHAIN_SAVING,
            model.theme.style(Role::Dim),
        )));
    }
    draw_box(
        model,
        frame,
        text::keychain_title(model.store).to_string(),
        Role::Warning,
        60,
        lines,
    );
}
