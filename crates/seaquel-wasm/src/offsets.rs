//! Positions across the boundary. Three units meet here:
//!
//! - JS strings and Monaco count UTF-16 code units.
//! - Rust strings and `seaquel-sql` count UTF-8 bytes.
//! - sqlparser's `Location` is a 1-based line (split on `\n` only) and a
//!   1-based column counted in chars (Unicode scalar values).
//!
//! `東京` is 2 UTF-16 units, 2 chars and 6 bytes; `😀` is 2 units, 1 char and
//! 4 bytes. Every position that crosses the boundary goes through here, and no
//! byte offset or `Location` leaves this crate.
//!
//! Every function is total: an offset past the end clamps to the end, and an
//! offset inside a char (a byte inside a multi-byte char, a UTF-16 offset
//! between the two halves of a surrogate pair) rounds down to the char's
//! start. Nothing here slices at an offset it hasn't checked.
//!
//! Lone surrogates never reach this code: wasm-bindgen encodes a JS string with
//! `TextEncoder`, which turns each unpaired surrogate into U+FFFD. That is one
//! UTF-16 unit too, so a UTF-16 offset computed here is also an offset into the
//! caller's original string.
//!
//! The plain UTF-16/UTF-8 conversions live in [`seaquel_sql::offsets`], so
//! Core converts `db.run`'s cursor with the same function; they're
//! re-exported here. The sqlparser `Location` conversions stay in this crate.

pub use seaquel_sql::offsets::{
    byte_to_utf16, floor_char_boundary, utf16_len, utf16_to_byte, Utf16Cursor,
};

/// A sqlparser char column (1-based) on `line` (1-based, lines split on `\n`
/// as sqlparser's tokenizer counts them) to the UTF-16 column (1-based) the
/// editor shows. A line past the end, or column 0 (sqlparser's "no
/// location"), is returned as it is. A column past the line's end counts
/// the missing chars as one unit each (sqlparser points one past the last
/// char at the end of the input).
pub fn char_column_to_utf16(s: &str, line: u64, column: u64) -> u64 {
    if line == 0 || column == 0 {
        return column;
    }
    let Some(text) = s
        .split('\n')
        .nth((line - 1).try_into().unwrap_or(usize::MAX))
    else {
        return column;
    };
    let wanted = column - 1;
    let mut chars = 0u64;
    let mut units = 0u64;
    for c in text.chars() {
        if chars == wanted {
            break;
        }
        chars += 1;
        units += c.len_utf16() as u64;
    }
    units.saturating_add(wanted - chars).saturating_add(1)
}

/// A sqlparser `Location` to a UTF-16 offset into the whole string: the start
/// of `line` plus the column's UTF-16 width. Clamps to the string's length.
pub fn location_to_utf16(s: &str, line: u64, column: u64) -> usize {
    if line == 0 {
        return 0;
    }
    let mut line_start = 0usize;
    let mut current = 1u64;
    for (byte, c) in s.char_indices() {
        if current == line {
            break;
        }
        if c == '\n' {
            current += 1;
            line_start = byte + 1;
        }
    }
    if current != line {
        return utf16_len(s);
    }
    let rest = &s[line_start..];
    let within: usize = rest
        .chars()
        .take_while(|&c| c != '\n')
        .take(column.saturating_sub(1).try_into().unwrap_or(usize::MAX))
        .map(char::len_utf16)
        .sum();
    byte_to_utf16(s, line_start) + within
}

/// The suffix sqlparser's `Location` prints: ` at Line: L, Column: C`.
const LINE: &str = " at Line: ";
const COLUMN: &str = ", Column: ";

/// Rewrites the `Line: L, Column: C` at the end of a sqlparser message so `C`
/// is the UTF-16 column the editor shows for that position in `sql`, not a
/// char count. sqlparser always puts the location last; a message that
/// doesn't end with exactly that shape (no location, or text after it) is
/// returned unchanged. Hand-parsed from the end, so a `Line: …` inside a
/// quoted token earlier in the message is never touched.
pub fn rewrite_error_location(sql: &str, message: &str) -> String {
    let Some(at) = message.rfind(LINE) else {
        return message.to_string();
    };
    let tail = &message[at + LINE.len()..];
    let Some((line, column)) = tail.split_once(COLUMN) else {
        return message.to_string();
    };
    let (Some(line), Some(column)) = (parse_number(line), parse_number(column)) else {
        return message.to_string();
    };
    let column = char_column_to_utf16(sql, line, column);
    format!("{}{LINE}{line}{COLUMN}{column}", &message[..at])
}

/// A decimal `u64` made only of ASCII digits (no sign, no spaces).
fn parse_number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        let s = "SELECT 1;\nSELECT '😀', x";
        // `x` is at line 2, char column 13; in UTF-16 the emoji counts 2.
        assert_eq!(location_to_utf16(s, 2, 13), utf16_len(s) - 1);
        assert_eq!(location_to_utf16(s, 1, 1), 0);
        assert_eq!(location_to_utf16(s, 2, 1), 10);
        // Past the line's end stops at the line's end; past the last line
        // clamps to the end.
        assert_eq!(location_to_utf16(s, 1, 99), 9);
        assert_eq!(location_to_utf16(s, 9, 1), utf16_len(s));
        assert_eq!(location_to_utf16(s, 0, 0), 0);
        assert_eq!(location_to_utf16("", 1, 1), 0);
    }

    #[test]
    fn char_columns_to_utf16() {
        let s = "SELECT '東京' AS c,\n  '😀😀' x y";
        // Line 1: 東京 are one unit each, so columns match.
        assert_eq!(char_column_to_utf16(s, 1, 12), 12);
        // Line 2: `y` is char column 10, UTF-16 column 12.
        assert_eq!(char_column_to_utf16(s, 2, 10), 12);
        assert_eq!(char_column_to_utf16(s, 2, 1), 1);
        // One past the end of the line, and further.
        assert_eq!(char_column_to_utf16(s, 2, 11), 13);
        assert_eq!(char_column_to_utf16(s, 2, 13), 15);
        // No such line, or no location: unchanged.
        assert_eq!(char_column_to_utf16(s, 3, 5), 5);
        assert_eq!(char_column_to_utf16(s, 0, 0), 0);
        assert_eq!(char_column_to_utf16(s, u64::MAX, u64::MAX), u64::MAX);
        assert_eq!(char_column_to_utf16("😀", 1, u64::MAX), u64::MAX);
    }

    #[test]
    fn rewrites_only_a_trailing_location() {
        let sql = "SELECT '😀😀' x y";
        assert_eq!(
            rewrite_error_location(
                sql,
                "Expected: end of statement, found: y at Line: 1, Column: 15"
            ),
            "Expected: end of statement, found: y at Line: 1, Column: 17"
        );
        // Text after the location, a malformed number, no location: unchanged.
        for msg in [
            "found: y at Line: 1, Column: 16 and more",
            "found: y at Line: 1, Column: -3",
            "found: y at Line: x, Column: 3",
            "found: y at Line: 1, Column: ",
            "found: y at Line: 1 Column: 3",
            "query is nested too deeply to parse",
            "",
        ] {
            assert_eq!(rewrite_error_location(sql, msg), msg);
        }
        // Only the last ` at Line: ` is the location; one quoted in the found
        // token is text.
        let sql = "SELECT 'at Line: 1, Column: 1' 😀 z";
        let msg = "found: 'at Line: 1, Column: 1' at Line: 1, Column: 34";
        assert_eq!(
            rewrite_error_location(sql, msg),
            "found: 'at Line: 1, Column: 1' at Line: 1, Column: 35"
        );
        // A number too large for u64 is left alone.
        let msg = "x at Line: 1, Column: 99999999999999999999999";
        assert_eq!(rewrite_error_location(sql, msg), msg);
    }
}
