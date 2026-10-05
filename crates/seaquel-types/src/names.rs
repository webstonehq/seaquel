//! How names are compared (phase 5d), shared by Core's checks
//! (`seaquel_workspace::library`, which re-exports it) and by storage, which
//! stores each connection's, project's and saved query's key in a
//! `name_key` column and backfills it in a data
//! step. One function for both, so the stored keys and new writes agree by
//! construction.
//!
//! **Changing [`name_key`] changes stored data.** Every stored key was made
//! by it. A change (a new Unicode version in `unicase` or
//! `unicode-normalization` counts) needs a data step that recomputes the
//! column; until then the duplicate check compares new keys with old ones.

/// JavaScript's `String.prototype.trim` set: WhiteSpace and LineTerminator.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `s.trim()` as JavaScript trims.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// The key names are compared by: trimmed, NFC-normalised and fully
/// Unicode case-folded, so `Straße` equals `STRASSE`, and a name typed in
/// NFD equals its NFC form. The fold runs between two normalisations, since
/// folding can leave text that isn't NFC.
pub fn name_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let nfc: String = js_trim(name).nfc().collect();
    let folded = unicase::UniCase::unicode(nfc.as_str()).to_folded_case();
    folded.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_fold_and_normalise() {
        assert_eq!(name_key(" ärger db "), name_key("Ärger DB"));
        assert_eq!(name_key("STRASSE"), name_key("Straße"));
        assert_eq!(name_key("Cafe\u{301}"), name_key("Café"));
        assert_eq!(name_key("\u{feff}x\u{3000}"), name_key("X"));
        assert_ne!(name_key("a"), name_key("b"));
    }
}
