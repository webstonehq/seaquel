//! JavaScript's idea of whitespace, for parity with the TS this crate
//! replaces: `/\s/`, `String.prototype.trim` and friends. Rust's
//! `char::is_whitespace` differs (it counts U+0085 and not U+FEFF).

/// Whether `c` matches JavaScript's `/\s/`: the ECMAScript WhiteSpace and
/// LineTerminator characters.
pub(crate) fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2028}'
    ) || matches!(
        c,
        '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    ) || ('\u{2000}'..='\u{200A}').contains(&c)
}

/// `s.trim()` in JavaScript.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// `s.trimEnd()` in JavaScript.
pub(crate) fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_js_set() {
        for c in ['\u{FEFF}', '\u{A0}', '\u{3000}', '\u{2028}', '\u{0B}'] {
            assert!(is_js_space(c), "{c:?}");
        }
        for c in ['\u{85}', '\u{180E}', '\u{200B}', 'a'] {
            assert!(!is_js_space(c), "{c:?}");
        }
        assert_eq!(js_trim("\u{FEFF} a \u{A0}"), "a");
        assert_eq!(js_trim_end("a\u{85}"), "a\u{85}");
    }
}
