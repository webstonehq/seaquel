//! The data grid's pure parts: a page as `table_page`
//! answered it, cells as the app shows them (`cellText`), the `/` filter
//! over the loaded page, column widths, which columns fit, and the footer's
//! counts. No terminal and no Core.
//!
//! A page holds cells and the SQL Core ran, so `Debug` shows counts only.

use std::fmt;
use std::ops::Range;

use seaquel_core::Value;
use unicode_width::UnicodeWidthStr;

/// The most columns a cell takes on screen; longer text is cut with `…`.
pub const MAX_COLUMN_WIDTH: usize = 40;
/// The least, so a short column's cursor is still visible.
pub const MIN_COLUMN_WIDTH: usize = 4;
/// Between columns.
pub const COLUMN_GAP: usize = 2;

/// One page of a table, as Core answered it.
#[derive(Clone, PartialEq)]
pub struct Page {
    /// The SELECT Core built and ran (the command log shows it).
    pub sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// 1-based.
    pub page: u32,
    pub page_size: u32,
    pub total_rows: u64,
    pub total_pages: u32,
    pub count_estimated: bool,
    pub elapsed_ms: f64,
}

impl fmt::Debug for Page {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Page")
            .field("columns", &self.columns.len())
            .field("rows", &self.rows.len())
            .field("page", &self.page)
            .field("total_rows", &self.total_rows)
            .field("total_pages", &self.total_pages)
            .field("count_estimated", &self.count_estimated)
            .finish_non_exhaustive()
    }
}

impl Page {
    /// A row's value in `column`.
    pub fn value(&self, row: usize, column: &str) -> Option<&Value> {
        let i = self.columns.iter().position(|c| c == column)?;
        self.rows.get(row)?.get(i)
    }

    /// A row as `[column, value]` pairs.
    pub fn row_values(&self, row: usize) -> Vec<(String, Value)> {
        self.rows
            .get(row)
            .map(|cells| {
                self.columns
                    .iter()
                    .cloned()
                    .zip(cells.iter().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether there's a page after this one: the count says so, or (when
    /// estimated) this page came back full.
    pub fn has_next(&self) -> bool {
        self.page < self.total_pages
            || (self.count_estimated && self.rows.len() as u64 >= u64::from(self.page_size))
    }
}

/// `cellText` (`src/lib/values.ts`): bigint and decimal as text, bytes as
/// `\x` hex, JSON compact, NULL as nothing.
pub fn cell_text(v: &Value) -> String {
    seaquel_core::ai::tools::format::cell_text(v)
}

/// A character that would move the cursor, change the terminal, reorder
/// what follows or not show at all: C0 and C1 controls, bidi embeddings,
/// overrides and isolates, and zero-width characters (M5).
fn unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Text as the screen may show it, on one line: a newline as `↵`, a tab as
/// `⇥`, and any other control, bidi or zero-width character as `�`. Used
/// for every cell, name, type, default and DDL line Core hands over.
pub fn clean(text: &str) -> String {
    if !text.chars().any(unsafe_char) {
        return text.to_string();
    }
    text.chars()
        .map(|c| match c {
            '\n' => '↵',
            '\t' => '⇥',
            c if unsafe_char(c) => '\u{FFFD}',
            c => c,
        })
        .collect()
}

/// A cell as the grid draws it: `NULL` for NULL, `cellText` otherwise,
/// [`clean`]ed.
pub fn display(v: &Value) -> String {
    if matches!(v, Value::Null) {
        return "NULL".to_string();
    }
    clean(&cell_text(v))
}

/// Whether a value lines up on the right (numbers, as the design's
/// `total`).
pub fn right_aligned(v: &Value) -> bool {
    matches!(v, Value::Int(_) | Value::Float(_) | Value::Decimal(_))
}

/// The `/` filter: a row matches when any cell's text contains `find`,
/// ignoring case (the prototype's `visRows`, over every column).
pub fn matches(row: &[Value], find: &str) -> bool {
    if find.is_empty() {
        return true;
    }
    let find = find.to_lowercase();
    row.iter()
        .any(|v| !matches!(v, Value::Null) && cell_text(v).to_lowercase().contains(&find))
}

/// The page rows the `/` filter keeps, by index.
pub fn visible(page: &Page, find: &str) -> Vec<usize> {
    (0..page.rows.len())
        .filter(|&i| matches(&page.rows[i], find))
        .collect()
}

/// `48,112`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The footer's counts: `rows 1–100 of 48,112` (`≈48,112` when Core
/// estimated the count), `no rows`, or a page past the end.
pub fn range_text(page: &Page) -> String {
    if page.rows.is_empty() {
        return if page.page > 1 {
            format!("page {} is past the end · p goes back", page.page)
        } else {
            "no rows".to_string()
        };
    }
    let first = u64::from(page.page - 1) * u64::from(page.page_size) + 1;
    let last = first + page.rows.len() as u64 - 1;
    let about = if page.count_estimated { "≈" } else { "" };
    format!(
        "rows {}–{} of {about}{}",
        thousands(first),
        thousands(last),
        thousands(page.total_rows.max(last))
    )
}

/// How long the page took: `18 ms`.
pub fn elapsed_text(ms: f64) -> String {
    format!("{} ms", ms.round() as u64)
}

/// Each column's width: its name, its type and its cells (each at most
/// [`MAX_COLUMN_WIDTH`], at least [`MIN_COLUMN_WIDTH`]).
pub fn column_widths(names: &[String], types: &[String], cells: &[Vec<String>]) -> Vec<usize> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let ty = types.get(i).map_or(0, |t| t.width());
            let widest = cells
                .iter()
                .filter_map(|row| row.get(i))
                .map(|c| c.width())
                .max()
                .unwrap_or(0);
            name.width()
                .max(ty)
                .max(widest)
                .clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
        })
        .collect()
}

/// The columns drawn in `room` columns: from the first, unless the cursor's
/// column wouldn't fit, then ending at the cursor's. Always at least the
/// cursor's column (cut to fit).
pub fn column_window(widths: &[usize], cursor: usize, room: usize) -> Range<usize> {
    if widths.is_empty() {
        return 0..0;
    }
    let cursor = cursor.min(widths.len() - 1);
    let fits = |range: Range<usize>| -> bool {
        let n = range.len();
        widths[range].iter().sum::<usize>() + COLUMN_GAP * n.saturating_sub(1) <= room
    };
    let mut start = 0;
    while start < cursor && !fits(start..cursor + 1) {
        start += 1;
    }
    let mut end = cursor + 1;
    while end < widths.len() && fits(start..end + 1) {
        end += 1;
    }
    start..end
}

/// `text` cut to `width` columns, ending in `…` when it doesn't fit.
pub fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// `text` padded (or cut) to exactly `width` columns, on the right or the
/// left.
pub fn pad(text: &str, width: usize, right: bool) -> String {
    let cut = fit(text, width);
    let gap = " ".repeat(width.saturating_sub(cut.width()));
    if right {
        format!("{gap}{cut}")
    } else {
        format!("{cut}{gap}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(rows: Vec<Vec<Value>>, page: u32, total: u64, estimated: bool) -> Page {
        Page {
            sql: "SELECT".into(),
            columns: vec!["id".into(), "customer".into()],
            page,
            page_size: 100,
            total_rows: total,
            total_pages: (total.div_ceil(100)) as u32,
            count_estimated: estimated,
            elapsed_ms: 18.4,
            rows,
        }
    }

    fn row(id: i64, customer: &str) -> Vec<Value> {
        vec![Value::Int(id), Value::Text(customer.into())]
    }

    #[test]
    fn cells_render_as_the_app_shows_them() {
        assert_eq!(display(&Value::Null), "NULL");
        assert_eq!(
            display(&Value::Int(9_007_199_254_740_993)),
            "9007199254740993"
        );
        assert_eq!(display(&Value::Decimal("4210.00".into())), "4210.00");
        assert_eq!(display(&Value::Bytes(vec![0xde, 0xad])), "\\xdead");
        assert_eq!(
            display(&Value::Json(serde_json::json!({"a": [1, 2]}))),
            "{\"a\":[1,2]}"
        );
        assert_eq!(display(&Value::Text("two\nlines\tx".into())), "two↵lines⇥x");
        assert_eq!(
            display(&Value::Text(
                "\u{1b}]52;c;eA==\u{7}a\u{9b}31m\rb\u{202e}c\u{200b}d".into()
            )),
            "�]52;c;eA==�a�31m�b�c�d"
        );
        assert_eq!(display(&Value::Bool(true)), "true");
        assert!(right_aligned(&Value::Decimal("1".into())));
        assert!(!right_aligned(&Value::Text("1".into())));
    }

    // Prototype `visRows`: case-insensitive, any cell.
    #[test]
    fn the_filter_keeps_rows_whose_cells_contain_the_text() {
        let p = page(
            vec![
                row(48112, "Acme Corp"),
                row(48111, "Initech"),
                row(48110, "acme labs"),
            ],
            1,
            3,
            false,
        );
        assert_eq!(visible(&p, ""), [0, 1, 2]);
        assert_eq!(visible(&p, "ACME"), [0, 2]);
        assert_eq!(visible(&p, "4811"), [0, 1, 2]);
        assert_eq!(visible(&p, "48111"), [1]);
        assert!(visible(&p, "nothing").is_empty());
        assert!(!matches(&[Value::Null], "null"), "NULL isn't text");
    }

    #[test]
    fn the_footer_counts_rows() {
        let rows = (0..100).map(|i| row(i, "x")).collect::<Vec<_>>();
        assert_eq!(
            range_text(&page(rows.clone(), 1, 48_112, false)),
            "rows 1–100 of 48,112"
        );
        assert_eq!(
            range_text(&page(rows.clone(), 3, 48_112, true)),
            "rows 201–300 of ≈48,112"
        );
        assert_eq!(range_text(&page(vec![], 1, 0, false)), "no rows");
        assert_eq!(
            range_text(&page(vec![], 5, 120, false)),
            "page 5 is past the end · p goes back"
        );
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(elapsed_text(18.4), "18 ms");
    }

    #[test]
    fn has_next_follows_the_count_or_a_full_estimated_page() {
        let full: Vec<_> = (0..100).map(|i| row(i, "x")).collect();
        assert!(page(full.clone(), 1, 150, false).has_next());
        assert!(!page(full.clone(), 2, 150, false).has_next());
        assert!(page(full.clone(), 2, 150, true).has_next());
        assert!(!page(vec![row(1, "x")], 2, 150, true).has_next());
    }

    #[test]
    fn widths_fit_names_types_and_cells_within_bounds() {
        let names = vec!["id".to_string(), "customer".into(), "note".into()];
        let types = vec!["int8".to_string(), "text".into(), "text".into()];
        let cells = vec![vec!["48112".into(), "Acme".into(), "x".repeat(200)]];
        assert_eq!(column_widths(&names, &types, &cells), [5, 8, 40]);
    }

    // A cell wider than the screen: the cursor's column is still drawn.
    #[test]
    fn the_window_keeps_the_cursor_s_column_in_view() {
        let widths = [10, 10, 10, 10, 10];
        assert_eq!(column_window(&widths, 0, 34), 0..3);
        assert_eq!(column_window(&widths, 2, 34), 0..3);
        assert_eq!(column_window(&widths, 3, 34), 1..4);
        assert_eq!(column_window(&widths, 4, 34), 2..5);
        assert_eq!(column_window(&widths, 4, 5), 4..5, "one, cut to fit");
        assert_eq!(column_window(&[], 0, 50), 0..0);
    }

    #[test]
    fn text_is_cut_and_padded_by_display_width() {
        assert_eq!(fit("Acme Corp", 6), "Acme …");
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(pad("12", 5, true), "   12");
        assert_eq!(pad("ab", 4, false), "ab  ");
        assert_eq!(pad("abcdef", 4, false), "abc…");
    }

    #[test]
    fn debug_shows_no_cells_or_sql() {
        let mut p = page(vec![row(1, "secret-marker")], 1, 1, false);
        p.sql = "SELECT 'sql-marker'".into();
        let text = format!("{p:?}");
        assert!(!text.contains("secret-marker") && !text.contains("sql-marker"));
    }
}
