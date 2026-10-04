//! The renderer: `view(&Model, &mut Frame)` only reads the model. Every box
//! is drawn from it at the current size, so a resize needs nothing but a
//! redraw.

pub mod ask;
pub mod command_log;
pub mod commit;
pub mod dialogs;
pub mod diff;
pub mod grid;
pub mod help;
pub mod hit;
pub mod keybar;
pub mod layout;
pub mod panels;
pub mod pending;
pub mod query;
pub mod structure;
pub mod theme;

use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::state::app::{Modal, Model, Panel, SavedTab, TablesTab};
use crate::state::text;
use theme::Role;

/// Draws the whole screen.
pub fn view(model: &Model, frame: &mut Frame) {
    let area = frame.area();
    let Some(areas) = layout::areas(area, model.ctx) else {
        too_small(model, area, frame);
        return;
    };
    connection(model, areas.connection, frame);
    tables(model, areas.tables, frame);
    saved(model, areas.saved, frame);
    pending(model, areas.pending, frame);
    main(model, areas.main, frame);
    command_log::render(model, areas.log, frame);
    keybar::render(model, areas.keybar, frame.buffer_mut());
    match &model.modal {
        Some(Modal::Help { scroll }) => help::render(model, *scroll, frame),
        Some(Modal::ConfirmQuit) => confirm_quit(model, area, frame),
        Some(Modal::Picker(p)) => dialogs::picker(model, p, frame),
        Some(Modal::Password(p)) => dialogs::password(model, p, frame),
        Some(Modal::Trust(t)) => dialogs::trust(model, t, frame),
        Some(Modal::Problem(p)) => dialogs::problem(model, p, frame),
        Some(Modal::Notice(n)) => dialogs::notice(model, n, frame),
        Some(Modal::Commit(d)) => commit::commit(model, d, frame),
        Some(Modal::ConfirmDiscard) => commit::discard(model, frame),
        Some(Modal::QueueSwitch(q)) => commit::switch(model, q, frame),
        Some(Modal::EditValue(v)) => commit::value(model, v, frame),
        Some(Modal::Params(p)) => query::params(model, p, frame),
        Some(Modal::RunConfirm(c)) => query::confirm(model, c, frame),
        Some(Modal::SaveAs(s)) => query::save_as(model, s, frame),
        Some(Modal::Cell(c)) => query::cell(model, c, frame),
        Some(Modal::Ask(a)) => ask::render(model, a, frame),
        None => {}
    }
    if model.keychain_box() {
        dialogs::keychain(model, frame);
    }
}

fn too_small(model: &Model, area: Rect, frame: &mut Frame) {
    let y = area.y + area.height / 2;
    let line = Rect::new(
        area.x,
        y.min(area.bottom().saturating_sub(1)),
        area.width,
        1.min(area.height),
    );
    frame.render_widget(
        Paragraph::new(text::too_small(area.width, area.height))
            .style(model.theme.style(Role::Warning))
            .alignment(Alignment::Center),
        line,
    );
}

/// A panel's tabs as title spans: the active one bold, in the focus colour
/// when the panel is focused.
fn tabs(model: &Model, names: &[&str], active: usize, focused: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" - ", model.theme.style(Role::Muted)));
        }
        let style = if i == active {
            if focused {
                model.theme.style(Role::Focus)
            } else {
                model.theme.style(Role::Text).bold()
            }
        } else {
            model.theme.style(Role::Muted)
        };
        spans.push(Span::styled(name.to_string(), style));
    }
    spans
}

/// A box with the lazygit title: `[n]-Tab - Tab`, an optional right-hand
/// title and the list's counter on the bottom border.
fn panel_block(
    model: &Model,
    panel: Panel,
    width: u16,
    title: Vec<Span<'static>>,
    right: Option<Span<'static>>,
    counter: Option<String>,
) -> Block<'static> {
    let focused = model.focus == panel;
    let border = if focused {
        model.theme.style(Role::Focus)
    } else {
        model.theme.style(Role::Border)
    };
    let number = if panel == Panel::Main {
        Vec::new()
    } else {
        vec![Span::styled(format!("[{}]-", panel.number()), border)]
    };
    let left = Line::from([number, title].concat());
    let left_width = left.width();
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(left);
    // The right-hand title only where it fits beside the left one.
    if let Some(right) = right.filter(|r| left_width + r.width() + 3 <= usize::from(width)) {
        block = block.title(Line::from(right).right_aligned());
    }
    if let Some(counter) = counter {
        block = block.title_bottom(
            Line::from(Span::styled(counter, model.theme.style(Role::Muted))).right_aligned(),
        );
    }
    block
}

fn placeholder(
    model: &Model,
    area: Rect,
    block: Block<'static>,
    line: &'static str,
    frame: &mut Frame,
) {
    frame.render_widget(
        Paragraph::new(Span::styled(line, model.theme.style(Role::Dim))).block(block),
        area,
    );
}

fn counter(model: &Model, panel: Panel) -> Option<String> {
    model
        .list(panel)
        .filter(|l| l.len > 0)
        .map(|l| text::counter(l.selected, l.len))
}

fn connection(model: &Model, area: Rect, frame: &mut Frame) {
    let focused = model.focus == Panel::Connection;
    let staged = model.staged.total();
    let marker = if staged > 0 {
        Span::styled(format!("● {staged}"), model.theme.style(Role::Modified))
    } else {
        Span::styled("○ 0", model.theme.style(Role::Dim))
    };
    let title = tabs(model, &[text::PANEL_CONNECTION], 0, focused);
    let block = panel_block(
        model,
        Panel::Connection,
        area.width,
        title,
        Some(marker),
        None,
    );
    frame.render_widget(
        Paragraph::new(panels::fit_line(
            panels::connection_line(model),
            usize::from(area.width.saturating_sub(2)),
        ))
        .block(block),
        area,
    );
}

fn tables(model: &Model, area: Rect, frame: &mut Frame) {
    let focused = model.focus == Panel::Tables;
    let active = match model.tables_tab {
        TablesTab::Tables => 0,
        TablesTab::Views => 1,
    };
    let title = tabs(model, &[text::TAB_TABLES, text::TAB_VIEWS], active, focused);
    let block = panel_block(
        model,
        Panel::Tables,
        area.width,
        title,
        None,
        counter(model, Panel::Tables),
    );
    panels::list(frame, area, block, panels::tables_lines(model, area));
}

fn saved(model: &Model, area: Rect, frame: &mut Frame) {
    let focused = model.focus == Panel::Saved;
    let active = match model.saved_tab {
        SavedTab::Saved => 0,
        SavedTab::History => 1,
    };
    let title = tabs(
        model,
        &[text::TAB_SAVED, text::TAB_HISTORY],
        active,
        focused,
    );
    let block = panel_block(
        model,
        Panel::Saved,
        area.width,
        title,
        None,
        counter(model, Panel::Saved),
    );
    panels::list(frame, area, block, panels::saved_lines(model, area));
}

fn pending(model: &Model, area: Rect, frame: &mut Frame) {
    let focused = model.focus == Panel::Pending;
    let s = model.staged;
    let counts = Span::styled(
        format!("+{} ~{} -{}", s.inserts, s.updates, s.deletes),
        model.theme.style(Role::Muted),
    );
    let title = tabs(model, &[text::PANEL_PENDING], 0, focused);
    let block = panel_block(
        model,
        Panel::Pending,
        area.width,
        title,
        Some(counts),
        counter(model, Panel::Pending),
    );
    panels::list(frame, area, block, pending::lines(model, area));
}

fn main(model: &Model, area: Rect, frame: &mut Frame) {
    if model.query.shown && !model.query.tabs.is_empty() {
        query::render(model, area, frame);
        return;
    }
    let focused = model.focus == Panel::Main;
    let title = tabs(model, model.main_tabs(), model.main_tab_index(), focused);
    if crate::state::browse::shows_opened(model) {
        opened(model, area, title, frame);
        return;
    }
    if model.ctx == Panel::Pending {
        let right = diff::title(model)
            .map(|t| Span::styled(crate::state::grid::clean(&t), model.theme.style(Role::Name)));
        let block = panel_block(model, Panel::Main, area.width, title, right, None);
        let width = usize::from(area.width.saturating_sub(2));
        let lines = if model.pending_tab == 0 {
            diff::diff(model, width)
        } else {
            diff::sql(model)
        };
        match lines {
            Some(lines) => frame.render_widget(
                Paragraph::new(lines)
                    .style(model.theme.style(Role::Text))
                    .block(block),
                area,
            ),
            None => placeholder(model, area, block, text::NOTHING_STAGED_HINT, frame),
        }
        return;
    }
    let block = panel_block(model, Panel::Main, area.width, title, None, None);
    match panels::main_lines(model) {
        Some(lines) => frame.render_widget(
            panels::body(lines, block, model.theme.style(Role::Text)),
            area,
        ),
        None => placeholder(model, area, block, text::NOTHING_SELECTED, frame),
    }
}

/// The main view over the opened table: its Data grid, or Structure,
/// Indexes, Constraints and DDL (a view's Data and Columns).
fn opened(model: &Model, area: Rect, title: Vec<Span<'static>>, frame: &mut Frame) {
    let Some(opened) = &model.browse.opened else {
        return;
    };
    let name = Span::styled(
        crate::state::grid::clean(&format!("{}.{}", opened.target.schema, opened.target.table)),
        model.theme.style(Role::Name),
    );
    let tab = model.main_tab_index();
    let counter = (tab == 0).then(|| grid::counter(model)).flatten();
    let block = panel_block(model, Panel::Main, area.width, title, Some(name), counter);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);
    let lines = match tab {
        0 => {
            grid::render(model, inner, frame);
            return;
        }
        1 => structure::structure(model, width),
        2 => structure::indexes(model, width),
        3 => structure::constraints(model, width),
        _ => structure::ddl(model),
    };
    frame.render_widget(
        Paragraph::new(lines).style(model.theme.style(Role::Text)),
        inner,
    );
}

fn confirm_quit(model: &Model, area: Rect, frame: &mut Frame) {
    let staged = model.staged.total();
    let message = if model.committing.is_some() {
        text::QUIT_COMMITTING.to_string()
    } else if staged > 0 {
        text::quit_staged(staged)
    } else if model.running() {
        text::QUIT_RUNNING.to_string()
    } else {
        text::QUIT_PLAIN.to_string()
    };
    let width = (message.chars().count() as u16 + 6).max(32);
    let rect = layout::dialog_area(area, width, 5);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(model.theme.style(Role::Warning))
        .title(Span::styled(
            text::QUIT_TITLE,
            model.theme.style(Role::Warning),
        ))
        .title_bottom(
            Line::from(Span::styled(
                text::QUIT_FOOTER,
                model.theme.style(Role::Muted),
            ))
            .right_aligned(),
        );
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(vec![Line::default(), Line::from(format!(" {message}"))])
            .style(model.theme.style(Role::Text))
            .wrap(Wrap { trim: false })
            .block(block),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app::{Modal, Panel};
    use crate::state::keymap::BarContext;
    use crate::testing::fixtures::{model, model_sized};
    use crate::testing::snapshot::{assert_snapshot, buffer_text, draw};
    use crate::view::theme::Role;
    use ratatui::layout::Rect;

    #[test]
    fn the_empty_layout_at_each_size() {
        for (w, h) in [(148, 42), (100, 30), (80, 24), (79, 24)] {
            let m = model_sized(w, h);
            assert_snapshot(&format!("empty_{w}x{h}"), &draw(&m));
        }
    }

    #[test]
    fn too_small_says_so() {
        let text = buffer_text(&draw(&model_sized(79, 24)));
        assert!(
            text.contains("Seaquel needs at least 80×24 (this is 79×24)"),
            "{text}"
        );
    }

    #[test]
    fn the_help_at_148_and_80() {
        let mut m = model();
        m.modal = Some(Modal::Help { scroll: 0 });
        assert_snapshot("help_148x42", &draw(&m));
        let mut m = model_sized(80, 24);
        m.modal = Some(Modal::Help { scroll: 2 });
        assert_snapshot("help_80x24_scrolled", &draw(&m));
    }

    #[test]
    fn confirm_quit_with_staged_changes() {
        let mut m = model_sized(80, 24);
        m.staged.updates = 2;
        m.staged.deletes = 1;
        m.modal = Some(Modal::ConfirmQuit);
        assert_snapshot("confirm_quit_80x24", &draw(&m));
    }

    #[test]
    fn every_context_s_key_bar() {
        let mut out = String::new();
        for context in BarContext::ALL {
            let mut m = model();
            match context {
                BarContext::Connection => m.focus = Panel::Connection,
                BarContext::Tables => m.focus = Panel::Tables,
                BarContext::Saved => (m.focus, m.ctx) = (Panel::Saved, Panel::Saved),
                BarContext::Pending => (m.focus, m.ctx) = (Panel::Pending, Panel::Pending),
                BarContext::Main => m.focus = Panel::Main,
                BarContext::Grid
                | BarContext::CellEdit
                | BarContext::Find
                | BarContext::FilterForm => {
                    m = crate::testing::fixtures::browsing(148, 42);
                    m.browse.col = 1;
                    match context {
                        BarContext::CellEdit => {
                            m.browse.editing = Some(crate::state::browse::CellEdit {
                                row: crate::state::browse::GridRow::Page(0),
                                column: "customer".into(),
                                text: String::new(),
                                start: String::new(),
                            })
                        }
                        BarContext::Find => m.browse.finding = true,
                        BarContext::FilterForm => crate::state::browse::open_form(&mut m),
                        _ => {}
                    }
                }
                BarContext::Help => m.modal = Some(Modal::Help { scroll: 0 }),
                BarContext::ConfirmQuit => m.modal = Some(Modal::ConfirmQuit),
                BarContext::Commit
                | BarContext::CommitProd
                | BarContext::ConfirmDiscard
                | BarContext::QueueSwitch
                | BarContext::EditValue => {
                    m = crate::testing::fixtures::staged(
                        148,
                        42,
                        context == BarContext::CommitProd,
                    );
                    m.modal = Some(match context {
                        BarContext::Commit | BarContext::CommitProd => {
                            Modal::Commit(Default::default())
                        }
                        BarContext::ConfirmDiscard => Modal::ConfirmDiscard,
                        BarContext::QueueSwitch => {
                            Modal::QueueSwitch(crate::state::commit::QueueSwitch {
                                from: "conn-saved".into(),
                                to: None,
                            })
                        }
                        _ => Modal::EditValue(crate::state::commit::ValueEdit {
                            id: m.queue.entries()[0].id.clone(),
                            text: String::new(),
                            start: String::new(),
                        }),
                    });
                }
                BarContext::QueryInsert
                | BarContext::QueryNormal
                | BarContext::QueryCommand
                | BarContext::Completion
                | BarContext::Results
                | BarContext::Params
                | BarContext::RunConfirm
                | BarContext::SaveAs
                | BarContext::Cell => m = crate::testing::fixtures::query_context(context),
                BarContext::AskPrompt
                | BarContext::AskMention
                | BarContext::AskWaiting
                | BarContext::AskAnswer
                | BarContext::AskDone => m = crate::testing::fixtures::ask_context(context),
                other => crate::testing::fixtures::dialog(&mut m, other),
            }
            assert_eq!(m.bar_context(), context);
            let buf = draw(&m);
            let last = buffer_text(&buf).lines().last().unwrap().to_string();
            out.push_str(&format!("{context:?}\n{last}\n"));
        }
        crate::testing::snapshot::assert_text_snapshot("keybars", &out);
    }

    #[test]
    fn panels_one_to_three_connected_at_148_and_80() {
        use crate::testing::fixtures::connected;
        assert_snapshot("connected_148x42", &draw(&connected(148, 42)));
        let mut m = connected(80, 24);
        m.focus = Panel::Saved;
        m.ctx = Panel::Saved;
        assert_snapshot("connected_saved_80x24", &draw(&m));
        let mut m = connected(100, 30);
        m.focus = Panel::Saved;
        m.ctx = Panel::Saved;
        m.saved_tab = crate::state::app::SavedTab::History;
        assert_snapshot("connected_history_100x30", &draw(&m));
    }

    #[test]
    fn panel_one_says_what_it_is_doing() {
        use crate::state::app::{Attempt, Conn};
        use crate::testing::fixtures::connected;
        let mut m = connected(148, 42);
        let line = |m: &Model| buffer_text(&draw(m)).lines().nth(1).unwrap().to_string();
        assert!(
            line(&m).contains("✓ prod-analytics → pg · db.internal"),
            "{}",
            line(&m)
        );
        m.conn = Conn::Connecting(Attempt {
            attempt: 1,
            pending: crate::state::dialogs::Pending {
                connection_id: "conn-key".into(),
                ..Default::default()
            },
        });
        assert!(
            line(&m).contains("… keyed → pg · db.internal · ssh bastion"),
            "{}",
            line(&m)
        );
        m.conn = Conn::Closed {
            id: "conn-saved".into(),
        };
        assert!(
            line(&m).contains("✗ prod-analytics · closed"),
            "{}",
            line(&m)
        );
        m.conn = Conn::None;
        assert!(
            line(&m).contains("enter to pick a connection"),
            "{}",
            line(&m)
        );
    }

    #[test]
    fn the_connect_dialogs_at_80x24() {
        for (name, context) in [
            ("picker", BarContext::Picker),
            ("password", BarContext::Password),
            ("password_no_save", BarContext::PasswordNoSave),
            ("trust", BarContext::Trust),
            ("problem_retry", BarContext::ProblemRetry),
            ("notice", BarContext::Notice),
            ("keychain", BarContext::Keychain),
        ] {
            let mut m = model_sized(80, 24);
            crate::testing::fixtures::dialog(&mut m, context);
            assert_eq!(m.bar_context(), context);
            assert_snapshot(&format!("dialog_{name}_80x24"), &draw(&m));
        }
        // The picker's second step, with the remembered connection selected.
        let mut m = model_sized(80, 24);
        m.library = crate::testing::fixtures::library();
        m.modal = Some(Modal::Picker(crate::state::picker::Picker {
            stage: crate::state::picker::Stage::Connections {
                project_id: "project-b".into(),
            },
            selected: 1,
        }));
        assert_snapshot("dialog_picker_connections_80x24", &draw(&m));
    }

    /// Screen 1a's data: the staged edit of `48109.total` and the delete
    /// of `48106` (planned, as Core answers), the cursor on the edit, and
    /// the command log's lines.
    fn screen_1a(width: u16, height: u16) -> Model {
        use crate::state::app::update;
        use crate::state::log::{LogLine, Tag};
        use crate::testing::keys::{key, press};
        use crossterm::event::KeyCode;
        let mut m = crate::testing::fixtures::browsing(width, height);
        m.log.push(LogLine {
            time: "12:04:02".into(),
            tag: None,
            text: m.browse.page.as_ref().unwrap().sql.replace('\n', " "),
            elapsed: Some("18 ms".into()),
        });
        m.browse.row = 3;
        m.browse.col = 5;
        update(&mut m, key('e'));
        for _ in 0..7 {
            update(&mut m, press(KeyCode::Backspace));
        }
        for c in "3150.00".chars() {
            update(&mut m, key(c));
        }
        update(&mut m, press(KeyCode::Enter));
        m.browse.row = 6;
        update(&mut m, key('d'));
        m.browse.row = 3;
        for (tag, text) in [
            (
                Tag::Staged,
                "UPDATE \"public\".\"invoices\" SET \"total\" = $1 WHERE \"id\" = $2",
            ),
            (
                Tag::StagedDelete,
                "DELETE FROM \"public\".\"invoices\" WHERE \"id\" = $1",
            ),
        ] {
            m.log.push(LogLine {
                time: "12:04:19".into(),
                tag: Some(tag),
                text: text.into(),
                elapsed: None,
            });
        }
        m
    }

    #[test]
    fn screen_1a_browse_at_148_and_80() {
        assert_snapshot("browse_148x42", &draw(&screen_1a(148, 42)));
        assert_snapshot("browse_80x24", &draw(&screen_1a(80, 24)));
        let text = buffer_text(&draw(&screen_1a(148, 42)));
        assert!(text.contains("~ 48109"), "{text}");
        assert!(text.contains("- 48106"), "{text}");
        assert!(
            text.contains("total · numeric(12,2)  2975.00 → 3150.00"),
            "{text}"
        );
        assert!(text.contains("rows 1–15 of 48,112 · 18 ms"), "{text}");
        assert!(
            text.contains("~1 -1  48.1k"),
            "panel 2 marks the table: {text}"
        );
        assert!(text.contains("4 of 15"), "{text}");
    }

    #[test]
    fn the_grid_while_editing_and_filtering() {
        use crate::state::app::update;
        use crate::testing::keys::key;
        let mut m = crate::testing::fixtures::browsing(148, 42);
        m.browse.row = 1;
        m.browse.col = 1;
        update(&mut m, key('e'));
        assert_snapshot("browse_editing_148x42", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("Initech▌"), "{text}");
        assert!(text.ends_with("EDIT\n"), "the mode: {text}");

        let mut m = crate::testing::fixtures::browsing(148, 42);
        for c in "/zzz".chars() {
            update(&mut m, key(c));
        }
        let text = buffer_text(&draw(&m));
        assert!(text.contains("/zzz▌"), "{text}");
        assert!(
            text.contains("no rows match the filter · esc to clear"),
            "{text}"
        );
        assert!(
            text.contains("0 matches on this page · rows 1–15 of 48,112"),
            "{text}"
        );
        assert!(text.ends_with("FILTER\n"), "{text}");
    }

    #[test]
    fn the_filter_form_and_the_server_filter_and_sort_line() {
        use crate::state::app::update;
        use crate::testing::keys::key;
        let mut m = crate::testing::fixtures::browsing(148, 42);
        m.browse.col = 5;
        update(&mut m, key('F'));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("F total ="), "{text}");
        m.browse.form = None;
        m.browse.filter = Some(seaquel_core::domain::edits::Filter {
            column: "total".into(),
            op: seaquel_core::domain::edits::FilterOp::Gt,
            value: "3000".into(),
        });
        m.browse.sort = Some(seaquel_core::domain::edits::Sort {
            column: "issued_at".into(),
            direction: seaquel_core::domain::edits::SortDirection::Desc,
        });
        let text = buffer_text(&draw(&m));
        assert!(text.contains("F total > 3000 · sort issued_at ↓"), "{text}");
    }

    #[test]
    fn structure_indexes_constraints_and_ddl() {
        for (tab, name) in [
            (1, "structure"),
            (2, "indexes"),
            (3, "constraints"),
            (4, "ddl"),
        ] {
            let mut m = crate::testing::fixtures::browsing(148, 42);
            m.main_tab = tab;
            assert_snapshot(&format!("browse_{name}_148x42"), &draw(&m));
        }
    }

    // A cell wider than the screen, bytes and JSON: cut with `…`; the
    // cursor's column scrolls into view.
    #[test]
    fn wide_cells_and_columns_past_the_edge() {
        use seaquel_core::Value;
        let mut m = crate::testing::fixtures::browsing(80, 24);
        let page = m.browse.page.as_mut().unwrap();
        page.rows[0][1] = Value::Text("x".repeat(500));
        page.rows[1][1] = Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]);
        page.rows[2][1] = Value::Json(serde_json::json!({"a": 1}));
        // A page changed outside `update`: a new page as far as the cache goes.
        m.browse.page_gen += 1;
        let text = buffer_text(&draw(&m));
        assert!(text.contains(&format!("{}…", "x".repeat(39))), "{text}");
        assert!(text.contains("\\xdeadbeef"), "{text}");
        assert!(text.contains("{\"a\":1}"), "{text}");
        assert!(
            !text.contains("paid_at"),
            "the last column is off the edge: {text}"
        );
        m.browse.col = 6;
        let text = buffer_text(&draw(&m));
        assert!(text.contains("paid_at"), "{text}");
        assert!(!text.contains(" id "), "the first scrolled away: {text}");
    }

    // M5: nothing Core hands over reaches the terminal as a control, bidi
    // or zero-width character: cells, column names, types, defaults, index
    // names, DDL, and the command log.
    #[test]
    fn no_control_bidi_or_zero_width_character_reaches_the_screen() {
        use crate::state::browse::Meta;
        use seaquel_core::Value;
        let bad = "x\u{1b}]52;c;ZXZpbA==\u{7}y\u{9b}31m\rz\u{202e}w\u{2066}v\u{200b}u";
        let mut m = crate::testing::fixtures::browsing(148, 42);
        let page = m.browse.page.as_mut().unwrap();
        page.columns[1] = format!("cust{bad}");
        page.rows[0][1] = Value::Text(bad.into());
        page.rows[1][2] = Value::Json(serde_json::json!({ "k": bad }));
        let Meta::Loaded(meta) = &mut m.browse.meta else {
            unreachable!()
        };
        meta.columns[1].name = format!("cust{bad}");
        meta.columns[2].ty = format!("enum{bad}");
        meta.columns[2].default_value = Some(bad.into());
        meta.indexes[1].name = format!("idx{bad}");
        meta.ddl = Ok(format!("CREATE TABLE t (\n  \"{bad}\" text\n);"));
        m.browse.page_gen += 1;
        m.log.push(crate::state::log::LogLine {
            time: "12:00:00".into(),
            tag: None,
            text: format!("SELECT {bad}"),
            elapsed: None,
        });
        m.browse.col = 1;
        crate::state::browse::refresh(&mut m);
        for tab in 0..5 {
            m.main_tab = tab;
            let buf = draw(&m);
            for cell in buf.content() {
                for c in cell.symbol().chars() {
                    assert!(
                        !c.is_control()
                            && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'),
                        "tab {tab}: {:?} on screen",
                        c
                    );
                }
            }
            if tab != 1 && tab != 3 {
                let text = buffer_text(&buf);
                assert!(
                    text.contains('\u{fffd}'),
                    "tab {tab} shows a marker: {text}"
                );
            }
        }
    }

    // M6: a marker when columns go on past the right edge; the `F` form
    // says range operators compare as text; an edit shows its newlines.
    #[test]
    fn the_grid_marks_more_columns_and_the_form_notes_text_comparison() {
        use crate::state::app::update;
        use crate::testing::keys::{key, press};
        use crossterm::event::KeyCode;
        let mut m = crate::testing::fixtures::browsing(80, 24);
        let text = buffer_text(&draw(&m));
        assert!(text.lines().nth(2).unwrap().contains('›'), "{text}");
        m.browse.col = 6;
        let text = buffer_text(&draw(&m));
        assert!(!text.lines().nth(2).unwrap().contains('›'), "{text}");
        assert!(text.lines().nth(2).unwrap().contains('‹'), "{text}");

        let mut m = crate::testing::fixtures::browsing(148, 42);
        m.browse.col = 5;
        update(&mut m, key('F'));
        update(&mut m, press(KeyCode::Tab));
        update(&mut m, press(KeyCode::Right));
        let text = buffer_text(&draw(&m));
        assert!(!text.contains("compares as text"), "`!=` doesn't: {text}");
        update(&mut m, press(KeyCode::Right));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("F total > ▌  compares as text"), "{text}");

        let mut m = crate::testing::fixtures::browsing(148, 42);
        m.browse.col = 1;
        update(&mut m, key('e'));
        m.browse.editing.as_mut().unwrap().text = "a\nb\tc".into();
        let text = buffer_text(&draw(&m));
        assert!(text.contains("a↵b⇥c▌"), "{text}");
    }

    /// Probe F1: `schema_tables` gives no columns on any engine, so an
    /// unopened table's preview says how to load them rather than "0
    /// columns"; once they're known (opened, or read for completion) it
    /// lists them.
    #[test]
    fn a_preview_without_columns_says_how_to_load_them() {
        let mut m = crate::testing::fixtures::connected(148, 42);
        for t in &mut m.schema {
            t.columns.clear();
        }
        let text = buffer_text(&draw(&m));
        assert!(!text.contains("0 columns"), "{text}");
        assert!(
            text.contains(crate::state::text::LOAD_COLUMNS_HINT),
            "{text}"
        );
        let Some(crate::state::panels::Row::Item(i)) = m.selected_table_row() else {
            panic!("a table is selected")
        };
        m.schema[i].columns = vec![("line_marker".into(), "int8".into())];
        let text = buffer_text(&draw(&m));
        assert!(
            text.contains("1 columns") || text.contains("1 column"),
            "{text}"
        );
        assert!(text.contains("line_marker"), "{text}");
    }

    #[test]
    fn the_password_is_never_drawn() {
        let mut m = model_sized(80, 24);
        crate::testing::fixtures::dialog(&mut m, BarContext::Password);
        let text = buffer_text(&draw(&m));
        assert!(!text.contains("secret"), "{text}");
        assert!(text.contains("••••••"), "{text}");
        assert!(text.contains("[x] Save password"), "{text}");
    }

    #[test]
    fn a_narrow_bar_drops_middle_entries_and_keeps_the_help_key() {
        let m = model_sized(80, 24);
        let text = buffer_text(&draw(&m));
        let bar = text.lines().last().unwrap();
        assert!(bar.starts_with("Select: j/k | Open: enter"), "{bar}");
        assert!(bar.contains("| Keybindings: ?"), "{bar}");
        assert!(!bar.contains("Next panel"), "{bar}");
        assert!(bar.ends_with(&crate::state::text::version_label()), "{bar}");
    }

    #[test]
    fn the_focused_panel_s_border_is_the_focus_colour() {
        let mut m = model();
        m.focus = Panel::Saved;
        m.ctx = Panel::Saved;
        let buf = draw(&m);
        let areas = layout::areas(Rect::new(0, 0, 148, 42), m.ctx).unwrap();
        let focus = m.theme.style(Role::Focus).fg;
        let border = m.theme.style(Role::Border).fg;
        assert_eq!(buf[(areas.saved.x, areas.saved.y + 1)].fg, focus.unwrap());
        assert_eq!(
            buf[(areas.tables.x, areas.tables.y + 1)].fg,
            border.unwrap()
        );
    }

    /// Screen 1c: panel 4 focused on the delete of 48106 (`2 of 4`), its
    /// diff in the main view.
    fn screen_1c(width: u16, height: u16) -> Model {
        use crate::state::app::update;
        use crate::testing::keys::key;
        let mut m = crate::testing::fixtures::staged(width, height, true);
        update(&mut m, key('4'));
        update(&mut m, key('j'));
        m
    }

    #[test]
    fn screen_1c_pending_changes_at_148_and_80() {
        let m = screen_1c(148, 42);
        assert_snapshot("pending_148x42", &draw(&m));
        assert_snapshot("pending_80x24", &draw(&screen_1c(80, 24)));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("+1 ~2 -1"), "{text}");
        assert!(text.contains("▾ public.invoices"), "{text}");
        assert!(text.contains("M id 48109"), "{text}");
        assert!(text.contains("D id 48106"), "{text}");
        assert!(text.contains("A customer New Co"), "{text}");
        assert!(text.contains(crate::state::text::PENDING_HINT), "{text}");
        assert!(text.contains("2 of 4"), "{text}");
        assert!(text.contains("public.invoices · id 48106"), "{text}");
        assert!(text.contains("- customer"), "the whole row, red: {text}");
        assert!(text.contains("Hooli"), "{text}");
        let bar = text.lines().last().unwrap();
        assert!(
            bar.starts_with(
                "Unstage: space | Undo: u | Edit value: e | Commit: c | Discard all: D"
            ),
            "{bar}"
        );

        // The update's diff: `-` the loaded value, `+` the staged one.
        let mut m = screen_1c(148, 42);
        m.pending.selected = 0;
        let text = buffer_text(&draw(&m));
        assert!(text.contains("- total"), "{text}");
        assert!(text.contains("2975.00"), "{text}");
        assert!(text.contains("+ total"), "{text}");
        assert!(text.contains("3150.00"), "{text}");
        assert!(text.contains("  customer"), "unchanged columns: {text}");

        // The SQL tab: Core's plan and its values.
        m.pending_tab = 1;
        assert_snapshot("pending_sql_148x42", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains(crate::state::text::PLANNED_BY_CORE), "{text}");
        assert!(
            text.contains("UPDATE \"public\".\"invoices\" SET \"total\""),
            "{text}"
        );
        assert!(text.contains("$1 = 3150.00"), "{text}");
        assert!(text.contains("$2 = 48109"), "{text}");
    }

    #[test]
    fn nothing_staged_says_how_to_stage() {
        let mut m = crate::testing::fixtures::browsing(148, 42);
        m.focus = Panel::Pending;
        m.ctx = Panel::Pending;
        let text = buffer_text(&draw(&m));
        assert!(text.contains(crate::state::text::NOTHING_STAGED), "{text}");
        assert!(
            text.contains(crate::state::text::NOTHING_STAGED_HINT),
            "{text}"
        );
    }

    // An apply that stopped at a change marks it in panel 4 and says why
    // in its diff.
    #[test]
    fn a_failed_change_is_marked() {
        use crate::state::dialogs::CallError;
        let mut m = screen_1c(148, 42);
        let id = m.queue.entries()[1].id.clone();
        m.queue.mark_failed(
            &id,
            CallError::new(
                "NO_ROWS_AFFECTED",
                crate::state::text::no_row("public.invoices", "id 48106"),
            ),
        );
        let text = buffer_text(&draw(&m));
        assert!(text.contains("! id 48106"), "{text}");
        assert!(
            text.contains("no row matched in public.invoices · id 48106"),
            "{text}"
        );
    }

    #[test]
    fn the_commit_dialog_with_counts_prod_and_the_preview() {
        use crate::state::app::update;
        use crate::testing::keys::key;
        let mut m = screen_1c(148, 42);
        update(&mut m, key('c'));
        for c in "pro".chars() {
            update(&mut m, key(c));
        }
        assert_snapshot("commit_148x42", &draw(&m));
        let text = buffer_text(&draw(&m));
        for line in [
            "Commit 4 changes",
            "Run on prod-analytics in a single transaction:",
            "+1  INSERT  public.invoices",
            "~2  UPDATE  public.invoices",
            "-1  DELETE  public.invoices",
            crate::state::text::PROD_WARNING_DELETE,
            "type prod to confirm › pro",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
        assert!(
            text.lines()
                .last()
                .unwrap()
                .starts_with("Execute: enter | Preview SQL: tab | Cancel: esc"),
            "{text}"
        );
        // The preview: Core's statements with their values.
        update(
            &mut m,
            crate::testing::keys::press(crossterm::event::KeyCode::Tab),
        );
        let text = buffer_text(&draw(&m));
        assert!(
            text.contains("DELETE FROM \"public\".\"invoices\" WHERE \"id\" = $1"),
            "{text}"
        );
        assert!(text.contains("$1 = 48106"), "{text}");
        let mut m = crate::testing::fixtures::staged(80, 24, false);
        update(&mut m, key('c'));
        assert_snapshot("commit_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(!text.contains("type prod"), "{text}");
        assert!(
            text.lines()
                .last()
                .unwrap()
                .starts_with("Execute: enter | Preview SQL: p | Cancel: esc"),
            "{text}"
        );
    }

    #[test]
    fn the_discard_switch_and_value_dialogs() {
        use crate::state::app::update;
        use crate::testing::keys::key;
        let mut m = screen_1c(80, 24);
        update(&mut m, key('D'));
        assert_snapshot("dialog_discard_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(
            text.contains(&crate::state::text::discard_question(4)),
            "{text}"
        );

        let mut m = screen_1c(80, 24);
        m.modal = Some(Modal::QueueSwitch(crate::state::commit::QueueSwitch {
            from: "conn-saved".into(),
            to: Some("conn-ask".into()),
        }));
        assert_snapshot("dialog_switch_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(
            text.contains(&crate::state::text::staged_on(4, "prod-analytics")),
            "{text}"
        );
        assert!(
            text.contains("Keep them: k | Discard them: d | Cancel: esc"),
            "{text}"
        );

        let mut m = screen_1c(80, 24);
        m.pending.selected = 0;
        update(&mut m, key('e'));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("3150.00▌"), "{text}");
        assert!(text.contains("total"), "{text}");
    }

    // Review M1: at 80×24, 30 changes and 15 destructive statements: the
    // list (first 10, then "…and N more") and the prod field stay in view.
    #[test]
    fn a_long_commit_keeps_the_destructive_list_and_prod_in_view() {
        use crate::state::app::update;
        use crate::state::commit::{CommitDialog, Destructive};
        use crate::testing::keys::{key, press};
        use crossterm::event::KeyCode;
        let mut m = crate::testing::fixtures::browsing(80, 24);
        let mut effects = Vec::new();
        for row in 0..15 {
            m.browse.row = row;
            m.browse.col = 1;
            effects.extend(update(&mut m, key('e')));
            effects.extend(update(&mut m, key('x')));
            effects.extend(update(&mut m, press(KeyCode::Enter)));
            effects.extend(update(&mut m, key('d')));
        }
        crate::testing::fixtures::answer_plans(&mut m, &effects);
        assert_eq!(m.queue.entries().len(), 30);
        let list: Vec<Destructive> = (0..15)
            .map(|i| Destructive {
                sql: format!("DELETE FROM t{i}"),
                reason: "DELETE without WHERE".into(),
            })
            .collect();
        m.modal = Some(Modal::Commit(CommitDialog {
            typed: "pr".into(),
            preview: false,
            from_core: Some((list, 15)),
        }));
        assert_snapshot("commit_long_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("DELETE FROM t9"), "{text}");
        assert!(!text.contains("DELETE FROM t10"), "{text}");
        assert!(text.contains("…and 5 more"), "{text}");
        assert!(
            text.contains(crate::state::text::PROD_WARNING_DELETE),
            "{text}"
        );
        assert!(text.contains("type prod to confirm › pr"), "{text}");
        // The preview is cut with "…N more", the list and field still shown.
        update(&mut m, press(KeyCode::Tab));
        let text = buffer_text(&draw(&m));
        assert!(
            text.lines()
                .any(|l| l.contains('…') && l.contains(" more") && !l.contains("and")),
            "{text}"
        );
        assert!(text.contains("type prod to confirm › pr"), "{text}");
        assert!(text.contains("…and 5 more"), "{text}");
    }

    /// Screen 1b: `revenue_by_month.sql` (saved, modified) with the cursor
    /// after `i.iss`, the completion popup open, and the Explain tab with
    /// the design's ANALYZE plan.
    fn screen_1b(width: u16, height: u16) -> Model {
        crate::testing::fixtures::screen_1b(width, height)
    }

    #[test]
    fn screen_1b_query_at_148_and_80() {
        assert_snapshot("query_148x42", &draw(&screen_1b(148, 42)));
        assert_snapshot("query_80x24", &draw(&screen_1b(80, 24)));
        let text = buffer_text(&draw(&screen_1b(148, 42)));
        for line in [
            "[Q]-revenue_by_month.sql - untitled-1 - +",
            "INSERT · modified",
            " 1 SELECT c.name,",
            " 7 WHERE  i.iss",
            "Results - Explain - Messages",
            "ANALYZE ✓ · 412.3 ms",
            "node",
            "share of total",
            "Sort (sum(revenue)) DESC",
            "└─ HashAggregate",
            "      ├─ Seq Scan invoice_line_items li",
            "312,006",
            "238.9",
            "58%",
        ] {
            assert!(text.contains(line), "{line:?}: {text}");
        }
        // The popup lists the alias's columns with their types.
        for (column, ty) in [("issued_at", "date"), ("issuer_id", "int8")] {
            assert!(
                text.lines().any(|l| l.contains(column) && l.contains(ty)),
                "{column}: {text}"
            );
        }
        // The popup's keys while it's open, the editor's once it closes.
        let bar = text.lines().last().unwrap();
        assert!(
            bar.starts_with("Insert: tab | Choose: arrows | Close: esc"),
            "{bar}"
        );
        assert!(bar.ends_with("INSERT · Ln 7, Col 13"), "{bar}");
        let mut m = screen_1b(148, 42);
        crate::state::query::close_completion(&mut m);
        let text = buffer_text(&draw(&m));
        let bar = text.lines().last().unwrap();
        assert!(
            bar.starts_with(
                "Run: ctrl+r | Run statement: ctrl+e | Explain: ctrl+x | Ask AI: ctrl+k | Save: ctrl+s"
            ),
            "{bar}"
        );
    }

    #[test]
    fn the_editor_colours_what_the_scanner_found() {
        let m = screen_1b(148, 42);
        let buf = draw(&m);
        let text = buffer_text(&buf);
        let y = text
            .lines()
            .position(|l| l.contains("SELECT c.name,"))
            .unwrap() as u16;
        let line: String = text.lines().nth(y as usize).unwrap().to_string();
        let at =
            |needle: &str| -> u16 { line[..line.find(needle).unwrap()].chars().count() as u16 };
        let keyword = m.theme.style(Role::Deleted).fg.unwrap();
        assert_eq!(buf[(at("SELECT"), y)].fg, keyword);
        let y2 = y + 1;
        let line2: String = text.lines().nth(y2 as usize).unwrap().to_string();
        let string_x = line2[..line2.find("'month'").unwrap()].chars().count() as u16;
        assert_eq!(
            buf[(string_x, y2)].fg,
            m.theme.style(Role::Cursor).fg.unwrap()
        );
        let fn_x = line2[..line2.find("date_trunc").unwrap()].chars().count() as u16;
        assert_eq!(buf[(fn_x, y2)].fg, m.theme.style(Role::Name).fg.unwrap());
    }

    #[test]
    fn results_and_messages_after_a_run() {
        let m = crate::testing::fixtures::ran(148, 42);
        assert_snapshot("query_results_148x42", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("statement 1 of 2"), "{text}");
        assert!(text.contains("rows 1–3 of 3 · 4 ms"), "{text}");
        assert!(text.contains("Acme Corp"), "{text}");
        let mut m = crate::testing::fixtures::ran(80, 24);
        m.query.tabs[0].result_tab = crate::state::query::ResultTab::Messages;
        assert_snapshot("query_messages_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("3 rows"), "{text}");
        assert!(text.contains("2 rows affected"), "{text}");
        assert!(text.contains("SYNTAX_ERROR"), "{text}");
    }

    /// Screen 1d: the popup over an untitled tab, the request with its
    /// mentions, the answer and its status line, the sharing on the title.
    #[test]
    fn screen_1d_ask_at_148_and_80() {
        assert_snapshot(
            "ask_148x42",
            &draw(&crate::testing::fixtures::screen_1d(148, 42)),
        );
        assert_snapshot(
            "ask_80x24",
            &draw(&crate::testing::fixtures::screen_1d(80, 24)),
        );
        let text = buffer_text(&draw(&crate::testing::fixtures::screen_1d(148, 42)));
        for line in [
            "Ask AI",
            "schema shared · data not shared · read-only",
            "› top 10 customers by revenue this quarter, only @invoices with status paid,",
            "✓ generated in 1.8s · claude-sonnet",
            " 1 SELECT c.name, c.country, sum(li.quantity * li.unit_price) AS revenue",
            " 8 ORDER BY revenue DESC LIMIT 10;",
        ] {
            assert!(text.contains(line), "{line:?}: {text}");
        }
        let bar = text.lines().last().unwrap();
        assert!(
            bar.starts_with(
                "Insert: enter | Run: ctrl+r | Refine: tab | Save as: ctrl+s | Close: esc"
            ),
            "{bar}"
        );
        assert!(!text.contains("test-key-not-real"));
    }

    /// The popup's other stages at 80×24: the `@` list, waiting, an error
    /// and a note after Ctrl+R refused to run an UPDATE.
    #[test]
    fn the_ask_popup_s_stages_at_80x24() {
        use crate::state::keymap::BarContext;
        for (name, context) in [
            ("mention", BarContext::AskMention),
            ("waiting", BarContext::AskWaiting),
        ] {
            let mut m = crate::testing::fixtures::ask_context(context);
            m.size = (80, 24);
            assert_snapshot(&format!("ask_{name}_80x24"), &draw(&m));
        }
        let mut m = crate::testing::fixtures::ask_context(BarContext::AskMention);
        m.size = (80, 24);
        let text = buffer_text(&draw(&m));
        assert!(text.contains("public.invoices"), "{text}");
        assert!(text.contains("public.invoice_line_items"), "{text}");

        let mut m = crate::testing::fixtures::ask_context(BarContext::AskWaiting);
        m.size = (80, 24);
        let op = match &m.modal {
            Some(Modal::Ask(a)) => a.op,
            _ => unreachable!(),
        };
        crate::state::app::update(
            &mut m,
            crate::state::app::Msg::Generated {
                op,
                result: Err(crate::state::dialogs::CallError::new(
                    "NO_PROVIDER",
                    "No AI provider is configured.",
                )),
                elapsed_ms: 2,
            },
        );
        assert_snapshot("ask_error_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("No AI provider is configured."), "{text}");

        let mut m = crate::testing::fixtures::ask_context(BarContext::AskWaiting);
        m.size = (80, 24);
        let op = match &m.modal {
            Some(Modal::Ask(a)) => a.op,
            _ => unreachable!(),
        };
        crate::state::app::update(
            &mut m,
            crate::state::app::Msg::Generated {
                op,
                result: Ok(crate::state::query::SqlText(
                    "UPDATE invoices SET total = 0".into(),
                )),
                elapsed_ms: 2,
            },
        );
        crate::state::app::update(&mut m, crate::testing::keys::ctrl('r'));
        assert_snapshot("ask_not_run_80x24", &draw(&m));
        let text = buffer_text(&draw(&m));
        assert!(text.contains("Inserted, not run"), "{text}");
    }

    #[test]
    fn the_query_dialogs_at_80x24() {
        for (name, context) in [
            ("params", BarContext::Params),
            ("run_confirm", BarContext::RunConfirm),
            ("save_as", BarContext::SaveAs),
            ("cell", BarContext::Cell),
        ] {
            let mut m = crate::testing::fixtures::query_context(context);
            m.size = (80, 24);
            assert_snapshot(&format!("dialog_{name}_80x24"), &draw(&m));
        }
        let text = buffer_text(&draw(&crate::testing::fixtures::query_context(
            BarContext::RunConfirm,
        )));
        assert!(text.contains("DELETE FROM invoices"), "{text}");
        assert!(text.contains("DELETE without WHERE"), "{text}");
        assert!(text.contains("type prod to confirm › pr"), "{text}");
        let text = buffer_text(&draw(&crate::testing::fixtures::query_context(
            BarContext::Params,
        )));
        assert!(text.contains("from › 2026-01-01▌"), "{text}");
        assert!(text.contains("to   › "), "{text}");
        let text = buffer_text(&draw(&crate::testing::fixtures::query_context(
            BarContext::Cell,
        )));
        assert!(text.contains("\"lines\": ["), "pretty JSON: {text}");
    }

    // M5 for the editor: what's typed (or opened from a file) shows no
    // control, bidi or zero-width character, and a tab is spaces.
    #[test]
    fn the_editor_shows_no_control_characters() {
        let bad = "SELECT 'x\u{1b}]52;c;ZXZpbA==\u{7}y\u{202e}z\u{200b}' AS \"a\tb\"";
        let m = crate::testing::fixtures::querying(148, 42, bad, false);
        let buf = draw(&m);
        for cell in buf.content() {
            for c in cell.symbol().chars() {
                assert!(
                    !c.is_control()
                        && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'),
                    "{c:?} on screen"
                );
            }
        }
        assert!(buffer_text(&buf).contains('\u{fffd}'));
    }

    /// A 2 MB text draws a frame in under 16 ms in a release build (the
    /// timing is printed in a debug build, not checked).
    #[test]
    fn a_2_mb_text_draws_a_frame_fast() {
        let line = "SELECT id, customer, total FROM invoices WHERE total > 100; -- a comment\n";
        let text = line.repeat(2 * 1024 * 1024 / line.len());
        let mut m = crate::testing::fixtures::querying(148, 42, &text, false);
        crate::state::query::on_tick(&mut m);
        let tab = &m.query.tabs[0];
        assert!(tab
            .editor
            .highlight_current(seaquel_core::sql::SqlEngine::Postgres));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(148, 42)).unwrap();
        terminal.draw(|f| view(&m, f)).unwrap();
        let started = std::time::Instant::now();
        let frames = 20;
        for _ in 0..frames {
            terminal.draw(|f| view(&m, f)).unwrap();
        }
        let per_frame = started.elapsed() / frames;
        println!("2 MB text: {per_frame:?} per frame");
        if !cfg!(debug_assertions) {
            assert!(
                per_frame < std::time::Duration::from_millis(16),
                "{per_frame:?}"
            );
        }
        assert!(buffer_text(terminal.backend().buffer()).contains("SELECT id, customer"));
        // What a key and the next tick's highlighting cost (printed only).
        let started = std::time::Instant::now();
        for _ in 0..20 {
            crate::state::app::update(&mut m, crate::testing::keys::key('x'));
        }
        println!("2 MB text: {:?} per key", started.elapsed() / 20);
        let started = std::time::Instant::now();
        crate::state::query::on_tick(&mut m);
        println!("2 MB text: {:?} to highlight", started.elapsed());
    }

    // Review M2: quitting during a commit says what it does.
    #[test]
    fn the_quit_dialog_during_a_commit() {
        let mut m = crate::testing::fixtures::staged(80, 24, false);
        m.committing = Some(crate::state::commit::Committing {
            op: 1,
            ids: vec![],
            core_id: "core-1".into(),
            connection_id: "conn-saved".into(),
        });
        m.modal = Some(Modal::ConfirmQuit);
        let text = buffer_text(&draw(&m));
        assert!(
            text.contains("quitting stops it and it may be partly"),
            "{text}"
        );
    }
}
