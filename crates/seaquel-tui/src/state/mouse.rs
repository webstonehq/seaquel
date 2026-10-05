//! The mouse: a click focuses the box under it, selects
//! the row or cell under it (panels 2–4, the data grid, the results) and
//! switches to the tab whose title it's on; the wheel moves the selection
//! of the list or grid under the pointer, whichever box has the focus, and
//! scrolls the help and a cell. Where things are comes from `view::hit`,
//! the same geometry the view draws with. Shift-drag (and `--no-mouse`)
//! leave text selection to the terminal; the help says so.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::app::{Effect, Modal, Model, Panel, SavedTab, TablesTab};
use super::query::{self, Pane, ResultTab};
use super::{browse, editor::Normal};
use crate::view::hit::{self, Target};

/// A mouse event.
pub fn on_mouse(model: &mut Model, event: MouseEvent) -> Vec<Effect> {
    let (x, y) = (event.column, event.row);
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) if model.modal.is_none() => click(model, x, y),
        MouseEventKind::ScrollDown => {
            wheel(model, x, y, 1);
            Vec::new()
        }
        MouseEventKind::ScrollUp => {
            wheel(model, x, y, -1);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// A left click: focus the box, then select or switch to what's under it.
fn click(model: &mut Model, x: u16, y: u16) -> Vec<Effect> {
    let Some(target) = hit::at(model, x, y) else {
        return Vec::new();
    };
    let panel = match target {
        Target::Row(p, _) | Target::Tab(p, _) | Target::Box(p) => p,
        Target::Pending(_) => Panel::Pending,
        _ => Panel::Main,
    };
    model.focus_panel(panel);
    if panel == Panel::Main && model.query.shown {
        // Off a tab or a cell, the half of the query view under the click.
        model.query.pane = hit::query_pane(target).unwrap_or_else(|| {
            let areas = crate::view::layout::areas(
                ratatui::layout::Rect::new(0, 0, model.size.0, model.size.1),
                model.ctx,
            );
            match areas {
                Some(a)
                    if crate::view::layout::query_areas(a.main)
                        .0
                        .contains((x, y).into()) =>
                {
                    Pane::Editor
                }
                _ => Pane::Results,
            }
        });
    }
    match target {
        Target::Row(panel, index) => {
            if let Some(list) = model.list_mut(panel) {
                let before = list.selected;
                list.selected = index;
                if panel == Panel::Tables && index != before {
                    // Another table starts on Data, as `j`/`k` do.
                    model.main_tab = 0;
                }
            }
        }
        Target::Pending(Some(n)) => model.pending.selected = n,
        Target::Tab(panel, index) => switch_tab(model, panel, index),
        Target::QueryTab(index) => {
            model.query.active = index;
            if let Some(tab) = model.query.active_mut() {
                tab.editor.completion = None;
            }
        }
        Target::NewQueryTab => return query::new_tab(model),
        Target::ResultTab(index) => {
            if let Some(tab) = model.query.active_mut() {
                tab.result_tab = ResultTab::ALL[index];
            }
        }
        // An edit or a filter being typed keeps the cursor where it is.
        Target::GridCell(row, col) if !browse::typing(model) => {
            model.browse.row = row;
            if let Some(col) = col {
                model.browse.col = col;
            }
        }
        Target::ResultCell(row, col) => {
            if let Some(tab) = model.query.active_mut() {
                tab.row = row;
                if let Some(col) = col {
                    tab.col = col;
                }
            }
        }
        _ => {}
    }
    Vec::new()
}

/// A click on a title's tab.
fn switch_tab(model: &mut Model, panel: Panel, index: usize) {
    match panel {
        Panel::Tables => {
            let want = if index == 0 {
                TablesTab::Tables
            } else {
                TablesTab::Views
            };
            if model.tables_tab != want {
                model.cycle_tab(true);
            }
        }
        Panel::Saved => {
            let want = if index == 0 {
                SavedTab::Saved
            } else {
                SavedTab::History
            };
            if model.saved_tab != want {
                model.cycle_tab(true);
            }
        }
        Panel::Main if model.ctx == Panel::Tables => model.main_tab = index,
        Panel::Main if model.ctx == Panel::Pending => model.pending_tab = index,
        _ => {}
    }
}

/// One wheel step: the list or grid under the pointer moves its selection,
/// whatever has the focus; over the help or a cell, they scroll.
fn wheel(model: &mut Model, x: u16, y: u16, delta: isize) {
    let down = delta > 0;
    match &model.modal {
        Some(Modal::Help { .. }) => {
            model.scroll_help(down);
            return;
        }
        Some(Modal::Cell(_)) => {
            query::cell_scroll(model, down);
            return;
        }
        Some(_) => return,
        None => {}
    }
    let Some(target) = hit::at(model, x, y) else {
        return;
    };
    match target {
        Target::Row(panel, _) | Target::Tab(panel, _) | Target::Box(panel)
            if panel != Panel::Main && panel != Panel::Connection =>
        {
            model.step_list(panel, delta);
        }
        Target::Pending(_) => model.step_list(Panel::Pending, delta),
        Target::GridCell(..) | Target::Box(Panel::Main) | Target::Tab(Panel::Main, _)
            if !model.query.shown && browse::shows_grid(model) =>
        {
            if !browse::typing(model) {
                browse::move_cell(model, 0, delta);
            }
        }
        Target::Editor | Target::QueryTab(_) | Target::NewQueryTab => {
            if let Some(tab) = model.query.active_mut() {
                tab.editor.completion = None;
                tab.editor
                    .normal(if down { Normal::Down } else { Normal::Up });
            }
        }
        Target::ResultCell(..) | Target::ResultTab(_) | Target::Box(Panel::Main)
            if model.query.shown =>
        {
            query::results_move(model, 0, delta);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app::{update, Modal, SavedTab, TablesTab};
    use crate::state::panels::Row;
    use crate::state::query::ResultTab;
    use crate::state::text;
    use crate::testing::fixtures::{browsing, connected, querying, ran, staged};
    use crate::testing::keys::{click, wheel};
    use crate::view::layout;
    use ratatui::layout::Rect;
    use unicode_width::UnicodeWidthStr;

    fn areas(m: &Model) -> layout::Areas {
        layout::areas(Rect::new(0, 0, m.size.0, m.size.1), m.ctx).unwrap()
    }

    /// The x of the `i`th tab in a title that starts with `prefix`
    /// (`[2]-`), tabs separated by ` - `.
    fn tab_x(area: Rect, prefix: &str, names: &[&str], i: usize) -> u16 {
        let before: usize = names[..i].iter().map(|n| n.width() + 3).sum();
        area.x + 1 + (prefix.width() + before) as u16
    }

    #[test]
    fn a_click_on_a_panel_row_selects_it() {
        let mut m = connected(148, 42);
        let a = areas(&m);
        // Panel 2 starts at the top (the selection, 2, fits).
        let rows = m.table_rows();
        let orders = rows
            .iter()
            .position(|r| matches!(r, Row::Item(t) if m.schema[*t].name == "orders"))
            .unwrap();
        m.focus_panel(Panel::Saved);
        update(
            &mut m,
            click(a.tables.x + 3, a.tables.y + 1 + orders as u16),
        );
        assert_eq!((m.focus, m.tables.selected), (Panel::Tables, orders));
        // A row past the list's end selects nothing new.
        update(
            &mut m,
            click(a.tables.x + 3, a.tables.y + a.tables.height - 2),
        );
        assert_eq!(m.tables.selected, orders);

        // Panel 3.
        update(&mut m, click(a.saved.x + 3, a.saved.y + 2));
        assert_eq!((m.focus, m.saved.selected), (Panel::Saved, 1));
    }

    /// While panel 2 shows "loading", "couldn't load" or "no
    /// connection", its old rows aren't drawn, so a click selects none.
    #[test]
    fn a_click_selects_no_row_panel_two_doesn_t_draw() {
        use crate::state::app::{Conn, Load};
        for state in ["failed", "idle", "loading"] {
            let mut m = connected(148, 42);
            match state {
                "failed" => m.schema_load = Load::Failed,
                "idle" => {
                    m.schema_load = Load::Idle;
                    m.conn = Conn::None;
                }
                _ => m.schema_load = Load::Loading,
            }
            let before = m.tables.selected;
            let a = areas(&m);
            if state == "loading" {
                // Loading with the old list still in place draws it.
                update(&mut m, click(a.tables.x + 3, a.tables.y + 1 + 4));
                assert_eq!(m.tables.selected, 4, "{state}");
                continue;
            }
            update(&mut m, click(a.tables.x + 3, a.tables.y + 1 + 4));
            assert_eq!(m.tables.selected, before, "{state}");
            assert_eq!(
                hit::at(&m, a.tables.x + 3, a.tables.y + 1 + 4),
                Some(Target::Box(Panel::Tables)),
                "{state}"
            );
        }
    }

    #[test]
    fn a_click_on_a_staged_change_selects_it() {
        let mut m = staged(148, 42, false);
        update(
            &mut m,
            crate::testing::keys::press(crossterm::event::KeyCode::Esc),
        );
        m.focus_panel(Panel::Tables);
        let a = areas(&m);
        // Row 0 is the table's header, then the entries in display order.
        update(&mut m, click(a.pending.x + 3, a.pending.y + 3));
        assert_eq!((m.focus, m.pending.selected), (Panel::Pending, 1));
        // The header selects nothing new.
        update(&mut m, click(a.pending.x + 3, a.pending.y + 1));
        assert_eq!(m.pending.selected, 1);
    }

    #[test]
    fn a_click_on_a_grid_cell_selects_it() {
        let mut m = browsing(148, 42);
        m.focus_panel(Panel::Tables);
        let a = areas(&m);
        let widths = crate::state::browse::cache(&m).widths.clone();
        // Inside the border: the filter line, the names, the types, then
        // rows; each row starts with a 2-column marker.
        let x = a.main.x + 1 + 2 + widths[0] as u16 + 2 + 1;
        update(&mut m, click(x, a.main.y + 1 + 3 + 2));
        assert_eq!(m.focus, Panel::Main);
        assert_eq!((m.browse.row, m.browse.col), (2, 1));
    }

    #[test]
    fn a_click_on_a_results_cell_selects_it() {
        let mut m = ran(148, 42);
        m.focus_panel(Panel::Main);
        let a = areas(&m);
        let (_, results) = layout::query_areas(a.main);
        let tab = m.query.active().unwrap();
        let widths = tab.shown_statement().unwrap().widths.clone();
        let x = results.x + 1 + 1 + widths[0] as u16 + 2 + 1;
        // Inside the border: the names, then rows.
        update(&mut m, click(x, results.y + 1 + 1 + 1));
        let tab = m.query.active().unwrap();
        assert_eq!(m.query.pane, Pane::Results);
        assert_eq!((tab.row, tab.col), (1, 1));
    }

    #[test]
    fn the_wheel_moves_the_list_or_grid_under_the_pointer() {
        let mut m = connected(148, 42);
        let a = areas(&m);
        assert_eq!(m.focus, Panel::Tables);
        update(&mut m, wheel(true, a.saved.x + 3, a.saved.y + 2));
        update(&mut m, wheel(true, a.saved.x + 3, a.saved.y + 2));
        assert_eq!(m.saved.selected, 2);
        assert_eq!(m.focus, Panel::Tables, "the wheel doesn't move the focus");
        update(&mut m, wheel(false, a.tables.x + 3, a.tables.y + 2));
        assert_eq!(m.tables.selected, 1);

        let mut m = browsing(148, 42);
        let a = areas(&m);
        update(&mut m, wheel(true, a.main.x + 10, a.main.y + 8));
        update(&mut m, wheel(true, a.main.x + 10, a.main.y + 8));
        assert_eq!(m.browse.row, 2);
        update(&mut m, wheel(false, a.main.x + 10, a.main.y + 8));
        assert_eq!(m.browse.row, 1);

        let mut m = ran(148, 42);
        let a = areas(&m);
        let (_, results) = layout::query_areas(a.main);
        update(&mut m, wheel(true, results.x + 5, results.y + 3));
        assert_eq!(m.query.active().unwrap().row, 1);

        // The help scrolls under the wheel.
        let mut m = connected(80, 24);
        m.modal = Some(Modal::Help { scroll: 0 });
        update(&mut m, wheel(true, 40, 12));
        assert_eq!(m.modal, Some(Modal::Help { scroll: 1 }));
    }

    #[test]
    fn a_click_on_a_tab_title_switches_to_it() {
        let mut m = connected(148, 42);
        let a = areas(&m);
        let names = [text::TAB_TABLES, text::TAB_VIEWS];
        update(
            &mut m,
            click(tab_x(a.tables, "[2]-", &names, 1) + 1, a.tables.y),
        );
        assert_eq!(m.tables_tab, TablesTab::Views);
        // The active tab again changes nothing.
        update(
            &mut m,
            click(tab_x(a.tables, "[2]-", &names, 1), a.tables.y),
        );
        assert_eq!(m.tables_tab, TablesTab::Views);
        update(
            &mut m,
            click(tab_x(a.tables, "[2]-", &names, 0), a.tables.y),
        );
        assert_eq!(m.tables_tab, TablesTab::Tables);
        let names = [text::TAB_SAVED, text::TAB_HISTORY];
        update(
            &mut m,
            click(tab_x(a.saved, "[3]-", &names, 1) + 2, a.saved.y),
        );
        assert_eq!((m.focus, m.saved_tab), (Panel::Saved, SavedTab::History));

        // The table's tabs in the main view.
        let mut m = browsing(148, 42);
        let a = areas(&m);
        let tabs = text::MAIN_TABS_TABLE;
        update(&mut m, click(tab_x(a.main, "", tabs, 2) + 1, a.main.y));
        assert_eq!(m.main_tab, 2);

        // Query tabs, `+`, and the results' tabs.
        let mut m = querying(148, 42, "SELECT 1", false);
        crate::state::query::new_tab(&mut m);
        let (editor, results) = layout::query_areas(areas(&m).main);
        let titles: Vec<String> = m.query.tabs.iter().map(|t| t.title.clone()).collect();
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        assert_eq!(m.query.active, 1);
        update(
            &mut m,
            click(tab_x(editor, "[Q]-", &titles, 0) + 1, editor.y),
        );
        assert_eq!(m.query.active, 0);
        let mut with_new = titles.clone();
        with_new.push(text::QUERY_TABS_NEW);
        update(&mut m, click(tab_x(editor, "[Q]-", &with_new, 2), editor.y));
        assert_eq!(m.query.tabs.len(), 3, "+ opens a tab");
        update(
            &mut m,
            click(tab_x(results, "", text::RESULT_TABS, 2) + 1, results.y),
        );
        assert_eq!(m.query.active().unwrap().result_tab, ResultTab::Messages);
        assert_eq!(m.query.pane, Pane::Results);
    }
}
