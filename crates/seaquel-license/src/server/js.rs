//! The few JavaScript behaviours the TypeScript relied on, reproduced where
//! they decide bytes or answers: `parseInt`, `Date.prototype.toISOString`,
//! `Number.prototype.toString` for integers, and UTF-16 string lengths.

/// `parseInt(text, radix)` for radix 10 or 16: leading whitespace, an
/// optional sign, (radix 16) an optional `0x`, then the longest run of
/// digits. `None` where JavaScript gives `NaN`. Saturates instead of
/// growing past `i64`.
pub(crate) fn parse_int(text: &str, radix: u32) -> Option<i64> {
    let s = text.trim_start_matches(is_js_whitespace);
    let (negative, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = if radix == 16 && (s.starts_with("0x") || s.starts_with("0X")) {
        &s[2..]
    } else {
        s
    };
    let digits: Vec<u32> = s.chars().map_while(|c| c.to_digit(radix)).collect();
    if digits.is_empty() {
        return None;
    }
    let mut n: i64 = 0;
    for d in digits {
        n = n.saturating_mul(radix as i64).saturating_add(d as i64);
    }
    Some(if negative { -n } else { n })
}

/// JavaScript's `WhiteSpace` and `LineTerminator`, which `parseInt` and
/// `String.prototype.trim` skip.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{A}' | '\u{B}' | '\u{C}' | '\u{D}' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `new Date(ms).toISOString()`, or `None` where it throws (outside
/// ±8.64e15 ms, or not finite). Years outside 0..=9999 use the extended
/// `±YYYYYY` form.
pub(crate) fn iso_string(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    // `new Date(x)` truncates toward zero (TimeClip).
    let ms = ms.trunc() as i64;
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss, mss) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    let year = if (0..=9999).contains(&y) {
        format!("{y:04}")
    } else if y < 0 {
        format!("-{:06}", -y)
    } else {
        format!("+{y:06}")
    };
    Some(format!(
        "{year}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{mss:03}Z"
    ))
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A JavaScript number as `JSON.stringify` writes it, for integers in the
/// range where that's a plain integer: an `i64` JSON number there, a float
/// beyond (never reached by a real bundle).
pub(crate) fn number_value(x: f64) -> serde_json::Value {
    if x.fract() == 0.0 && x.abs() < 9.007_199_254_740_992e15 {
        // -0 prints as 0 in JavaScript.
        serde_json::Value::from(x as i64)
    } else {
        serde_json::Number::from_f64(x).map_or(serde_json::Value::Null, serde_json::Value::Number)
    }
}

/// `x` as an `i64` for a SQLite INTEGER column, saturating.
pub(crate) fn to_i64(x: f64) -> i64 {
    x as i64
}

/// The UTF-16 length, as JavaScript's `String.prototype.length`.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(-n)` in UTF-16 units. A cut through a surrogate pair keeps the
/// lone half as U+FFFD, the way it would reach the browser's JSON anyway.
pub(crate) fn utf16_tail(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    let start = units.len().saturating_sub(n);
    String::from_utf16_lossy(&units[start..])
}

/// Sort keys as `Array.prototype.sort` does by default: by UTF-16 code
/// units.
pub(crate) fn cmp_utf16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_matches_javascript() {
        assert_eq!(parse_int(" 12abc", 10), Some(12));
        assert_eq!(parse_int("1e3", 10), Some(1));
        assert_eq!(parse_int("+5", 10), Some(5));
        assert_eq!(parse_int("0x10", 10), Some(0));
        assert_eq!(parse_int("abc", 10), None);
        assert_eq!(parse_int("", 10), None);
        assert_eq!(parse_int("-", 10), None);
        assert_eq!(parse_int(" 1", 16), Some(1));
        assert_eq!(parse_int("-1", 16), Some(-1));
        assert_eq!(parse_int("+f", 16), Some(15));
        assert_eq!(parse_int("1z", 16), Some(1));
        assert_eq!(parse_int("0x", 16), None);
        assert_eq!(parse_int("g1", 16), None);
        assert_eq!(parse_int("ff", 16), Some(255));
        assert_eq!(parse_int("0X", 16), None);
        assert_eq!(parse_int("99999999999999999999999", 10), Some(i64::MAX));
    }

    #[test]
    fn iso_string_matches_to_iso_string() {
        // Expected values from Node 24's `new Date(s * 1000).toISOString()`.
        for (s, want) in [
            (0_i64, "1970-01-01T00:00:00.000Z"),
            (1_699_999_999, "2023-11-14T22:13:19.000Z"),
            (1_700_000_000, "2023-11-14T22:13:20.000Z"),
            (253_402_300_799, "9999-12-31T23:59:59.000Z"),
            (253_402_300_800, "+010000-01-01T00:00:00.000Z"),
            (-62_167_219_200, "0000-01-01T00:00:00.000Z"),
            (-62_167_219_201, "-000001-12-31T23:59:59.000Z"),
            (8_640_000_000_000, "+275760-09-13T00:00:00.000Z"),
            (951_782_400, "2000-02-29T00:00:00.000Z"),
            (4_107_542_400, "2100-03-01T00:00:00.000Z"),
        ] {
            assert_eq!(iso_string(s as f64 * 1000.0).as_deref(), Some(want), "{s}");
        }
        assert_eq!(iso_string(8.64e15 + 1000.0), None);
        assert_eq!(iso_string(f64::NAN), None);
    }

    #[test]
    fn utf16_helpers() {
        assert_eq!(utf16_len("a😀"), 3);
        assert_eq!(utf16_tail("abcdef", 4), "cdef");
        assert_eq!(utf16_tail("ab", 4), "ab");
        assert_eq!(cmp_utf16("😀", "\u{FFFF}"), std::cmp::Ordering::Less);
        assert_eq!(js_trim("\u{FEFF} x \u{A0}"), "x");
    }

    #[test]
    fn number_value_prints_integers_as_javascript_does() {
        assert_eq!(number_value(3.0).to_string(), "3");
        assert_eq!(number_value(-0.0).to_string(), "0");
        assert_eq!(number_value(1_700_000_000.0).to_string(), "1700000000");
    }
}
