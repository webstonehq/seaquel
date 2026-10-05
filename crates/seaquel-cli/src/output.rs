//! What the read commands print on stdout: `--format`, the aligned table and
//! the JSON cells.
//!
//! JSON keeps what JSON holds exactly (null, booleans, integers within
//! ±2^53, finite floats, text) and writes every other cell as the GUI's
//! text for it (`cell_text`: bigint and decimal as strings, bytes as `\x`
//! hex, JSON as text), never cut. The table cuts a cell at
//! [`MAX_CELL_WIDTH`] display columns and shows newlines and tabs as `↵`
//! and `→`, so a row stays on one line.

use std::io::IsTerminal;

use seaquel_core::ai::tools::format::cell_text;
use seaquel_types::{Value, MAX_SAFE_INTEGER};
use serde::Serialize;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::FormatArg;

/// The widest a table cell gets, in display columns, its `…` included.
pub const MAX_CELL_WIDTH: usize = 60;

/// Between two columns.
const GAP: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
}

impl Format {
    /// The flag, else table on a terminal and JSON otherwise.
    pub fn pick(arg: Option<FormatArg>) -> Self {
        match arg {
            Some(FormatArg::Table) => Format::Table,
            Some(FormatArg::Json) => Format::Json,
            None if std::io::stdout().is_terminal() => Format::Table,
            None => Format::Json,
        }
    }
}

/// A cell as JSON: what JSON holds exactly stays JSON, the rest is the
/// GUI's text for it. Never cut.
pub fn json_cell(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int(i) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => (*i).into(),
        Value::Float(f) if f.is_finite() => (*f).into(),
        Value::Text(s) => s.as_str().into(),
        other => cell_text(other).into(),
    }
}

/// `\n` as `↵`, `\t` as `→`, other control characters as `�`.
pub fn table_text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' => '↵',
            '\t' => '→',
            c if c.is_control() => '\u{fffd}',
            c => c,
        })
        .collect()
}

/// [`table_text`], cut to [`MAX_CELL_WIDTH`] display columns with `…`.
fn table_cell(s: &str) -> String {
    let text = table_text(s);
    if text.width() <= MAX_CELL_WIDTH {
        return text;
    }
    let mut cut = String::new();
    let mut width = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        // One column stays for the `…`.
        if width + w > MAX_CELL_WIDTH - 1 {
            break;
        }
        cut.push(c);
        width += w;
    }
    cut.push('…');
    cut
}

/// Columns separated by two spaces, a rule of `─` under the header, each
/// cell cut to [`MAX_CELL_WIDTH`] display columns with `…`. Trailing spaces
/// trimmed. Every line ends with `\n`.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let header: Vec<String> = header.iter().map(|h| table_cell(h)).collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|c| table_cell(c)).collect())
        .collect();
    let mut widths: Vec<usize> = header.iter().map(|h| h.width()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            match widths.get_mut(i) {
                Some(w) => *w = (*w).max(cell.width()),
                None => widths.push(cell.width()),
            }
        }
    }

    let mut out = String::new();
    let mut line = |cells: &[String]| {
        let mut text = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                text.push_str(GAP);
            }
            text.push_str(cell);
            let pad = widths[i].saturating_sub(cell.width());
            text.extend(std::iter::repeat_n(' ', pad));
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    line(&header);
    let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
    line(&rule);
    for row in &rows {
        line(row);
    }
    out
}

/// `value` as pretty-printed JSON, ending with a newline: the lists and
/// `schema`'s answers.
pub fn json<T: Serialize + ?Sized>(value: &T) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into());
    text.push('\n');
    text
}

/// Writes `text` to stdout. A closed pipe (`… | head`) isn't an error:
/// the reader has what it wanted.
pub fn print(text: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_cells_keep_what_json_holds_exactly() {
        assert_eq!(json_cell(&Value::Int(42)), serde_json::json!(42));
        assert_eq!(
            json_cell(&Value::Int(MAX_SAFE_INTEGER)),
            serde_json::json!(MAX_SAFE_INTEGER)
        );
        assert_eq!(
            json_cell(&Value::Int(1 << 60)),
            serde_json::json!("1152921504606846976")
        );
        assert_eq!(
            json_cell(&Value::Int(-(1 << 60))),
            serde_json::json!("-1152921504606846976")
        );
        assert_eq!(json_cell(&Value::Null), serde_json::Value::Null);
        assert_eq!(json_cell(&Value::Bool(true)), serde_json::json!(true));
        assert_eq!(json_cell(&Value::Float(1.5)), serde_json::json!(1.5));
        assert_eq!(json_cell(&Value::Float(f64::NAN)), serde_json::json!("NaN"));
        assert_eq!(
            json_cell(&Value::Decimal("12.50".into())),
            serde_json::json!("12.50")
        );
        assert_eq!(
            json_cell(&Value::Bytes(vec![0xde, 0xad])),
            serde_json::json!("\\xdead")
        );
        assert_eq!(
            json_cell(&Value::Json(serde_json::json!({"a": 1}))),
            serde_json::json!("{\"a\":1}")
        );
    }

    #[test]
    fn a_table_aligns_by_display_width_and_cuts_long_cells() {
        let t = table(
            &["id", "name"],
            &[
                vec!["1".into(), "日本".into()],
                vec!["22".into(), "x".repeat(80)],
            ],
        );
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "id  name");
        assert_eq!(lines[1], format!("──  {}", "─".repeat(MAX_CELL_WIDTH)));
        assert_eq!(lines[2], "1   日本");
        assert!(lines[3].ends_with('…'));
        assert_eq!(lines[3].width(), 4 + MAX_CELL_WIDTH);
    }

    /// A wide character that would cross the limit is left out whole.
    #[test]
    fn a_cut_never_splits_a_wide_character() {
        let cell = table_cell(&"日".repeat(40));
        assert_eq!(cell, format!("{}…", "日".repeat(29)));
        assert!(cell.width() <= MAX_CELL_WIDTH);
    }

    #[test]
    fn table_text_shows_newlines_and_tabs() {
        assert_eq!(table_text("a\nb\tc"), "a↵b→c");
        assert_eq!(table_text("a\rb\u{1b}"), "a\u{fffd}b\u{fffd}");
    }

    #[test]
    fn a_table_with_no_rows_is_its_header_and_rule() {
        assert_eq!(table(&["a", "bc"], &[]), "a  bc\n─  ──\n");
    }
}
