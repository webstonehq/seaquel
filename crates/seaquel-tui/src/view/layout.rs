//! Where everything goes (Decision 8). A pure function of the terminal's
//! size and the panel the main view shows, so nothing is cached per size
//! and `update` hit-tests clicks with the same rectangles the view draws.
//!
//! - **Wide (≥ 100 × 30):** a left column of panels 1–4 (a third of the
//!   width, at most 50 columns; 1 is 3 rows; 2, 3 and 4 share the rest,
//!   the one the main view shows taller), the main view on the right, the
//!   command log under it (4 lines), the key bar at the bottom.
//! - **80 × 24 up to wide:** the same with the left column at 28 columns
//!   and the command log at 2 lines.
//! - **Below 80 × 24:** nothing but "Seaquel needs at least 80×24".

use ratatui::layout::Rect;

use crate::state::app::Panel;

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
pub const WIDE_WIDTH: u16 = 100;
pub const WIDE_HEIGHT: u16 = 30;

/// Every box on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub connection: Rect,
    pub tables: Rect,
    pub saved: Rect,
    pub pending: Rect,
    pub main: Rect,
    pub log: Rect,
    pub keybar: Rect,
}

impl Areas {
    /// The panel at a cell, if any.
    pub fn panel_at(&self, x: u16, y: u16) -> Option<Panel> {
        let at = ratatui::layout::Position::new(x, y);
        [
            (self.connection, Panel::Connection),
            (self.tables, Panel::Tables),
            (self.saved, Panel::Saved),
            (self.pending, Panel::Pending),
            (self.main, Panel::Main),
        ]
        .into_iter()
        .find(|(rect, _)| rect.contains(at))
        .map(|(_, panel)| panel)
    }
}

/// The layout of `area` with the main view showing `ctx`; `None` below
/// 80 × 24.
pub fn areas(area: Rect, ctx: Panel) -> Option<Areas> {
    let (w, h) = (area.width, area.height);
    if w < MIN_WIDTH || h < MIN_HEIGHT {
        return None;
    }
    let wide = w >= WIDE_WIDTH && h >= WIDE_HEIGHT;
    let left = if wide { (w / 3).min(50) } else { 28 };
    let log_height = if wide { 6 } else { 4 };
    let (x, y) = (area.x, area.y);
    let body = h - 1;
    let connection = Rect::new(x, y, left, 3);
    // Panels 2–4 share the rest; the one the main view shows gets what the
    // other two leave.
    let rest = body - 3;
    let small = (rest - rest / 2) / 2;
    let big = rest - 2 * small;
    let tall = match ctx {
        Panel::Saved => 1,
        Panel::Pending => 2,
        _ => 0,
    };
    let heights: [u16; 3] = std::array::from_fn(|i| if i == tall { big } else { small });
    let mut top = y + 3;
    let [tables, saved, pending] = heights.map(|height| {
        let rect = Rect::new(x, top, left, height);
        top += height;
        rect
    });
    let right = w - left;
    let main = Rect::new(x + left, y, right, body - log_height);
    let log = Rect::new(x + left, y + body - log_height, right, log_height);
    let keybar = Rect::new(x, y + body, w, 1);
    Some(Areas {
        connection,
        tables,
        saved,
        pending,
        main,
        log,
        keybar,
    })
}

/// The query view inside the main view's box: the editor above (two
/// fifths, at least 6 rows) and the results below.
pub fn query_areas(main: Rect) -> (Rect, Rect) {
    let editor = (main.height * 2 / 5)
        .max(6)
        .min(main.height.saturating_sub(5));
    (
        Rect::new(main.x, main.y, main.width, editor),
        Rect::new(main.x, main.y + editor, main.width, main.height - editor),
    )
}

/// Where the editor draws its text inside its box: within the border, right
/// of the line numbers (as wide as the last line's number, at least 2, and
/// a space).
pub fn editor_text_area(editor: Rect, lines: usize) -> Rect {
    let gutter = gutter_width(lines);
    let inner = Rect::new(
        editor.x + 1,
        editor.y + 1,
        editor.width.saturating_sub(2),
        editor.height.saturating_sub(2),
    );
    Rect::new(
        inner.x + gutter,
        inner.y,
        inner.width.saturating_sub(gutter),
        inner.height,
    )
}

/// The line numbers' column, with its space.
pub fn gutter_width(lines: usize) -> u16 {
    (lines.max(1).to_string().len().max(2) + 1) as u16
}

/// The help dialog's box: centred, 64 columns at most, the full height
/// less a row above and below.
pub fn help_area(area: Rect) -> Rect {
    let width = area.width.min(64);
    let height = area.height.saturating_sub(2).max(area.height.min(3));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// A small dialog's box: centred, `width` × `height` at most.
pub fn dialog_area(area: Rect, width: u16, height: u16) -> Rect {
    let (width, height) = (width.min(area.width), height.min(area.height));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cover(a: &Areas) -> u32 {
        [
            a.connection,
            a.tables,
            a.saved,
            a.pending,
            a.main,
            a.log,
            a.keybar,
        ]
        .iter()
        .map(|r| r.area())
        .sum()
    }

    #[test]
    fn wide_at_148_by_42() {
        let a = areas(Rect::new(0, 0, 148, 42), Panel::Tables).unwrap();
        assert_eq!(a.connection, Rect::new(0, 0, 49, 3));
        assert_eq!(a.keybar, Rect::new(0, 41, 148, 1));
        assert_eq!(a.main, Rect::new(49, 0, 99, 35));
        assert_eq!(a.log, Rect::new(49, 35, 99, 6));
        // 2, 3, 4 share 38 rows; the main view's panel gets the rest.
        assert_eq!(
            (a.tables.height, a.saved.height, a.pending.height),
            (20, 9, 9)
        );
        assert_eq!(a.tables.y, 3);
        assert_eq!(a.saved.y, 23);
        assert_eq!(a.pending.y + a.pending.height, 41);
        assert_eq!(cover(&a), 148 * 42, "every cell belongs to one box");

        let a = areas(Rect::new(0, 0, 148, 42), Panel::Pending).unwrap();
        assert_eq!(
            (a.tables.height, a.saved.height, a.pending.height),
            (9, 9, 20)
        );
    }

    #[test]
    fn squeezed_below_100_by_30() {
        for (w, h) in [(80, 24), (99, 40), (120, 29)] {
            let a = areas(Rect::new(0, 0, w, h), Panel::Saved).unwrap();
            assert_eq!(a.connection.width, 28, "{w}×{h}");
            assert_eq!(a.log.height, 4, "{w}×{h}");
            assert_eq!(cover(&a), u32::from(w) * u32::from(h), "{w}×{h}");
            assert!(a.saved.height > a.tables.height, "{w}×{h}");
        }
        let a = areas(Rect::new(0, 0, 100, 30), Panel::Tables).unwrap();
        assert_eq!((a.connection.width, a.log.height), (33, 6));
    }

    #[test]
    fn too_small_has_no_layout() {
        for (w, h) in [(79, 24), (80, 23), (40, 10)] {
            assert_eq!(areas(Rect::new(0, 0, w, h), Panel::Tables), None, "{w}×{h}");
        }
    }

    #[test]
    fn panel_at_finds_the_box() {
        let a = areas(Rect::new(0, 0, 80, 24), Panel::Tables).unwrap();
        assert_eq!(a.panel_at(0, 0), Some(Panel::Connection));
        assert_eq!(
            a.panel_at(a.tables.x + 1, a.tables.y + 1),
            Some(Panel::Tables)
        );
        assert_eq!(a.panel_at(a.saved.x, a.saved.y), Some(Panel::Saved));
        assert_eq!(a.panel_at(a.pending.x, a.pending.y), Some(Panel::Pending));
        assert_eq!(a.panel_at(a.main.x, a.main.y), Some(Panel::Main));
        assert_eq!(a.panel_at(a.log.x, a.log.y), None);
        assert_eq!(a.panel_at(0, 23), None);
    }

    #[test]
    fn dialogs_are_centred_and_fit() {
        let full = Rect::new(0, 0, 148, 42);
        assert_eq!(help_area(full), Rect::new(42, 1, 64, 40));
        assert_eq!(help_area(Rect::new(0, 0, 80, 24)), Rect::new(8, 1, 64, 22));
        assert_eq!(dialog_area(full, 40, 6), Rect::new(54, 18, 40, 6));
        assert_eq!(
            dialog_area(Rect::new(0, 0, 30, 4), 40, 6),
            Rect::new(0, 0, 30, 4)
        );
    }
}
