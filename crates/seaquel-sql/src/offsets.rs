//! UTF-16 and UTF-8 offsets. JS strings and Monaco count UTF-16 code units;
//! Rust strings and this crate count UTF-8 bytes. `東京` is 2 UTF-16 units
//! and 6 bytes; `😀` is 2 units and 4 bytes.
//!
//! Every function is total: an offset past the end clamps to the end, and an
//! offset inside a char (a byte inside a multi-byte char, a UTF-16 offset
//! between the two halves of a surrogate pair) rounds down to the char's
//! start. Nothing here slices at an offset it hasn't checked.
//!
//! `seaquel-wasm` re-exports these for the editor, and Core converts `db.run`'s
//! cursor with [`utf16_to_byte`], so both pick the same statement for the same
//! offset. A lone surrogate never arrives: wasm-bindgen's `TextEncoder` and
//! the client's `toWellFormed()` both turn it into U+FFFD, which is one
//! UTF-16 unit too, so the offsets still line up with the caller's string.

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
}
