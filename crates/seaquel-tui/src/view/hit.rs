//! What the pointer is on (probe F3): the boxes, rows, cells and title
//! tabs, worked out from the same layout, windows and widths the view draws
//! with, so a click lands on what the user sees. Pure: `update` calls it.

use ratatui::layout::{Position, Rect};
use unicode_width::UnicodeWidthStr;

use crate::state::app::{Model, Panel, SavedTab};
use crate::state::query::{Pane, ResultTab};
use crate::state::{browse, grid, text};
use crate::view::layout;

/// The thing under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A row of panel 2's or 3's list (an index into the list it shows).
    Row(Panel, usize),
    /// Panel 4: a staged entry (its place in the list), or `None` on a
    /// table's header or the hint.
    Pending(Option<usize>),
    /// A tab in a box's title: panel 2's or 3's, or the main view's.
    Tab(Panel, usize),
    /// A query tab's title, or the `+` after them.
    QueryTab(usize),
    NewQueryTab,
    /// One of the results box's tabs.
    ResultTab(usize),
    /// The Data grid: a row (into `browse::rows`), and the column when the
    /// pointer is on one.
    GridCell(usize, Option<usize>),
    /// The query's results grid: a row and maybe a column.
    ResultCell(usize, Option<usize>),
    /// The query editor.
    Editor,
    /// A box, nowhere more particular.
    Box(Panel),
}

/// What's at `(x, y)`, if anything: `None` off every box (the command log,
/// the key bar) and below 80 × 24.
pub fn at(model: &Model, x: u16, y: u16) -> Option<Target> {
    let areas = layout::areas(Rect::new(0, 0, model.size.0, model.size.1), model.ctx)?;
    let panel = areas.panel_at(x, y)?;
    let area = match panel {
        Panel::Connection => return Some(Target::Box(panel)),
        Panel::Tables => areas.tables,
        Panel::Saved => areas.saved,
        Panel::Pending => areas.pending,
        Panel::Main => return Some(main(model, areas.main, x, y)),
    };
    let names: &[&str] = match panel {
        Panel::Tables => &[text::TAB_TABLES, text::TAB_VIEWS],
        Panel::Saved => &[text::TAB_SAVED, text::TAB_HISTORY],
        _ => &[text::PANEL_PENDING],
    };
    if y == area.y {
        let prefix = format!("[{}]-", panel.number());
        return Some(match tab_at(area, &prefix, names, x) {
            Some(i) if panel != Panel::Pending => Target::Tab(panel, i),
            _ => Target::Box(panel),
        });
    }
    let Some(line) = body_line(area, y) else {
        return Some(Target::Box(panel));
    };
    if panel == Panel::Pending {
        return Some(Target::Pending(super::pending::entry_at(model, area, line)));
    }
    let list = model.list(panel).copied().unwrap_or_default();
    let len = match (panel, model.saved_tab) {
        (Panel::Tables, _) if super::panels::tables_placeholder(model).is_some() => 0,
        (Panel::Tables, _) => model.table_rows().len(),
        (_, SavedTab::Saved) => model.saved_rows().len(),
        (_, SavedTab::History) => model.history_items.len(),
    };
    let height = usize::from(area.height.saturating_sub(2));
    let shown = super::panels::window(len, list.selected, height);
    Some(if line < shown.len() {
        Target::Row(panel, shown.start + line)
    } else {
        Target::Box(panel)
    })
}

/// The main view: the query's boxes, or the table's tabs and grid.
fn main(model: &Model, area: Rect, x: u16, y: u16) -> Target {
    let inside = |r: Rect| r.contains(Position::new(x, y));
    if model.query.shown && !model.query.tabs.is_empty() {
        let (editor, results) = layout::query_areas(area);
        if inside(editor) {
            if y != editor.y {
                return Target::Editor;
            }
            let mut names: Vec<String> = model
                .query
                .tabs
                .iter()
                .map(|t| grid::clean(&t.title))
                .collect();
            names.push(text::QUERY_TABS_NEW.to_string());
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            return match tab_at(editor, "[Q]-", &names, x) {
                Some(i) if i + 1 == names.len() => Target::NewQueryTab,
                Some(i) => Target::QueryTab(i),
                None => Target::Editor,
            };
        }
        if y == results.y {
            if let Some(i) = tab_at(results, "", text::RESULT_TABS, x) {
                return Target::ResultTab(i);
            }
        }
        let shows_results = model
            .query
            .active()
            .is_some_and(|t| t.result_tab == ResultTab::Results);
        let inner = inset(results);
        if shows_results && inside(inner) {
            if let Some((row, col)) = super::query::result_cell_at(model, inner, x, y) {
                return Target::ResultCell(row, col);
            }
        }
        return Target::Box(Panel::Main);
    }
    if y == area.y {
        if let Some(i) = tab_at(area, "", model.main_tabs(), x) {
            return Target::Tab(Panel::Main, i);
        }
    }
    if browse::shows_grid(model) {
        if let Some((row, col)) = super::grid::cell_at(model, inset(area), x, y) {
            return Target::GridCell(row, col);
        }
    }
    Target::Box(Panel::Main)
}

/// Inside a box's border.
fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// The body line (from 0) at `y` of a bordered box, if `y` is inside it.
fn body_line(area: Rect, y: u16) -> Option<usize> {
    let line = y.checked_sub(area.y + 1)?;
    (line < area.height.saturating_sub(2)).then_some(usize::from(line))
}

/// The tab of a title `prefix` + `names` joined by ` - ` (as the view
/// draws it, from the column after the corner) at column `x`.
fn tab_at(area: Rect, prefix: &str, names: &[&str], x: u16) -> Option<usize> {
    let mut start = usize::from(area.x) + 1 + prefix.width();
    let x = usize::from(x);
    for (i, name) in names.iter().enumerate() {
        let end = start + name.width();
        if (start..end).contains(&x) {
            return Some(i);
        }
        start = end + 3;
    }
    None
}

/// Whether the pointer is over the query's editor or results (for the
/// pane a click there picks).
pub fn query_pane(target: Target) -> Option<Pane> {
    match target {
        Target::Editor | Target::QueryTab(_) | Target::NewQueryTab => Some(Pane::Editor),
        Target::ResultTab(_) | Target::ResultCell(..) => Some(Pane::Results),
        _ => None,
    }
}
