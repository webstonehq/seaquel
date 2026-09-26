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

/// UTF-16 offset to UTF-8 byte offset.
pub fn utf16_to_byte(s: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, c) in s.char_indices() {
        let next = units + c.len_utf16();
        if next > utf16 {
            return byte;
        }
        units = next;
    }
    s.len()
}

/// UTF-8 byte offset to UTF-16 offset. A byte inside a char counts as that
/// char's start; past the end clamps to the string's UTF-16 length.
pub fn byte_to_utf16(s: &str, byte: usize) -> usize {
    s[..floor_char_boundary(s, byte)].encode_utf16().count()
}

/// The UTF-16 length of `s`.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The largest char boundary at or before `byte`, clamped to `s.len()`.
pub fn floor_char_boundary(s: &str, byte: usize) -> usize {
    if byte >= s.len() {
        return s.len();
    }
    let mut b = byte;
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// Converts many byte offsets into one string to UTF-16 offsets in one pass,
/// when they come in ascending order (statement ranges do). An offset before
/// the previous one restarts from the beginning, so any order is correct; only
/// ascending order is linear.
pub struct Utf16Cursor<'a> {
    s: &'a str,
    byte: usize,
    units: usize,
}

impl<'a> Utf16Cursor<'a> {
    pub fn new(s: &'a str) -> Self {
        Utf16Cursor {
            s,
            byte: 0,
            units: 0,
        }
    }

    /// `byte_to_utf16(s, byte)`.
    pub fn to_utf16(&mut self, byte: usize) -> usize {
        let byte = floor_char_boundary(self.s, byte);
        if byte < self.byte {
            self.byte = 0;
            self.units = 0;
        }
        self.units += self.s[self.byte..byte].encode_utf16().count();
        self.byte = byte;
        self.units
    }
}

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

    /// JS's view: the UTF-16 offset of every char boundary.
    fn js_offsets(s: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut units = 0;
        for (b, c) in s.char_indices() {
            out.push((b, units));
            units += c.len_utf16();
        }
        out.push((s.len(), units));
        out
    }

    const MIXED: &str = "SELECT '東京' AS city;\nSELECT '😀' AS face;\nSELECT 3";

    #[test]
    fn spike_round_trip() {
        let s = "SELECT '東京'; SELECT '😀'; SELECT 3";
        // JS: s.indexOf("SELECT 3") === 26; the Rust byte offset is 32.
        let b = utf16_to_byte(s, 26);
        assert_eq!(b, 32);
        assert_eq!(&s[b..], "SELECT 3");
        assert_eq!(byte_to_utf16(s, b), 26);
    }

    #[test]
    fn every_offset_of_the_mixed_string_round_trips() {
        let s = MIXED;
        let len16 = utf16_len(s);
        assert_eq!(len16, s.encode_utf16().count());
        let boundaries = js_offsets(s);
        for &(byte, u16) in &boundaries {
            assert_eq!(utf16_to_byte(s, u16), byte, "utf16 {u16}");
            assert_eq!(byte_to_utf16(s, byte), u16, "byte {byte}");
        }
        // Every UTF-16 offset lands on a char boundary, and converting back
        // gives the offset itself, or the pair's first half for an offset
        // between two surrogates.
        for u16 in 0..=len16 {
            let b = utf16_to_byte(s, u16);
            assert!(s.is_char_boundary(b));
            let back = byte_to_utf16(s, b);
            assert!(
                back == u16 || back + 1 == u16,
                "utf16 {u16} -> {b} -> {back}"
            );
        }
        // Every byte offset, including those inside `東` and `😀`.
        for byte in 0..=s.len() {
            let u = byte_to_utf16(s, byte);
            assert!(u <= len16);
            assert!(s.is_char_boundary(utf16_to_byte(s, u)));
        }
    }

    #[test]
    fn statement_two_starts_at_utf16_21_and_byte_25() {
        let at = MIXED.find("SELECT '😀'").unwrap();
        assert_eq!(at, 25);
        assert_eq!(byte_to_utf16(MIXED, at), 21);
        assert_eq!(utf16_to_byte(MIXED, 21), 25);
    }

    #[test]
    fn inside_a_char_rounds_down() {
        let s = "a😀b東c";
        // UTF-16: a=0, 😀=1..3, b=3, 東=4, c=5. Offset 2 is between the halves.
        assert_eq!(utf16_to_byte(s, 2), 1);
        assert_eq!(utf16_to_byte(s, 3), 5);
        // Bytes 2..=4 are inside the emoji, 7..=8 inside 東.
        for b in 1..5 {
            assert_eq!(byte_to_utf16(s, b), 1);
        }
        assert_eq!(byte_to_utf16(s, 7), 4);
        assert_eq!(byte_to_utf16(s, 8), 4);
        assert_eq!(floor_char_boundary(s, 3), 1);
    }

    #[test]
    fn past_the_end_clamps() {
        for s in ["", "abc", "東京", "😀", MIXED] {
            assert_eq!(utf16_to_byte(s, usize::MAX), s.len());
            assert_eq!(utf16_to_byte(s, utf16_len(s) + 5), s.len());
            assert_eq!(byte_to_utf16(s, usize::MAX), utf16_len(s));
            assert_eq!(byte_to_utf16(s, s.len() + 1), utf16_len(s));
            let mut c = Utf16Cursor::new(s);
            assert_eq!(c.to_utf16(usize::MAX), utf16_len(s));
        }
    }

    #[test]
    fn lone_surrogates_arrive_as_one_unit() {
        // What wasm-bindgen hands over for "SELECT '\uD83D'; SELECT 2" and for
        // a lone surrogate at the end of the buffer.
        let s = "SELECT '\u{FFFD}'; SELECT 2\u{FFFD}";
        assert_eq!(utf16_len(s), 21);
        let at = s.find("SELECT 2").unwrap();
        assert_eq!(byte_to_utf16(s, at), 12);
        assert_eq!(utf16_to_byte(s, 12), at);
        assert_eq!(byte_to_utf16(s, s.len()), 21);
        assert_eq!(utf16_to_byte(s, 20), s.len() - 3);
    }

    #[test]
    fn cursor_matches_the_one_off_conversion_in_any_order() {
        let s = MIXED;
        let mut c = Utf16Cursor::new(s);
        for byte in (0..=s.len() + 2)
            .chain((0..=s.len()).rev())
            .chain([7, 30, 3])
        {
            assert_eq!(c.to_utf16(byte), byte_to_utf16(s, byte), "byte {byte}");
        }
    }

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
