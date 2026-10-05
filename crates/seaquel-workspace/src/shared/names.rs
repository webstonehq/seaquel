//! File names and the path rules.

use std::collections::HashSet;

use seaquel_types::names::is_js_space;
use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use super::{MAX_FILE_NAME_BYTES, MAX_PATH_BYTES, MAX_STEM_BYTES, SEAQUEL_DIR};

fn nfc_lower_nfc(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    nfc.to_lowercase().nfc().collect()
}

/// Windows' reserved device names: `con`, `prn`, `aux`, `nul`,
/// `com0`–`com9`, `lpt0`–`lpt9`, and `com`/`lpt` with `¹`, `²` or `³`.
pub fn is_reserved_stem(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    if matches!(s.as_str(), "con" | "prn" | "aux" | "nul") {
        return true;
    }
    let rest = match s.strip_prefix("com").or_else(|| s.strip_prefix("lpt")) {
        Some(rest) => rest,
        None => return false,
    };
    let mut chars = rest.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('0'..='9' | '¹' | '²' | '³'), None)
    )
}

/// `s` cut to at most `max` bytes on a character boundary, without a `-`
/// left at the end.
fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end_matches('-')
}

/// A name's file stem: NFC, lower case, NFC again; letters, marks and
/// digits of every script kept (`\p{L}`, `\p{M}`, `\p{N}`); any other run
/// is one `-`, and none at either end; `untitled` when nothing is left;
/// Windows' reserved names get `-file`; at most 250 bytes, cut on a
/// character boundary.
pub fn file_stem(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    let mut gap = false;
    for c in nfc_lower_nfc(name).chars() {
        let keep = matches!(
            c.general_category_group(),
            GeneralCategoryGroup::Letter
                | GeneralCategoryGroup::Mark
                | GeneralCategoryGroup::Number
        );
        if keep {
            if gap && !stem.is_empty() {
                stem.push('-');
            }
            gap = false;
            stem.push(c);
        } else {
            gap = true;
        }
    }
    if stem.is_empty() {
        return "untitled".to_string();
    }
    if is_reserved_stem(&stem) {
        stem.push_str("-file");
    }
    cut(&stem, MAX_STEM_BYTES).to_string()
}

/// Today's `nameToFilename` (the slug path of rows an older release wrote):
/// lower case, everything but `a-z0-9`, whitespace and `-`
/// dropped, runs of whitespace as `-`, runs of `-` as one, one `-` trimmed
/// at each end, `untitled` when nothing is left.
pub fn legacy_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    let kept: String = lower
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || is_js_space(*c) || *c == '-')
        .collect();
    let mut out = String::with_capacity(kept.len());
    let mut space = false;
    for c in kept.chars() {
        if is_js_space(c) {
            space = true;
            continue;
        }
        if space {
            out.push('-');
            space = false;
        }
        out.push(c);
    }
    if space {
        out.push('-');
    }
    let mut collapsed = String::with_capacity(out.len());
    for c in out.chars() {
        if c == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(c);
    }
    let s = collapsed.strip_prefix('-').unwrap_or(&collapsed);
    let s = s.strip_suffix('-').unwrap_or(s);
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s.to_string()
    }
}

/// How paths compare when Core picks a new one (M2): NFC and
/// case-insensitive, as APFS and NTFS compare names.
pub fn path_key(path: &str) -> String {
    nfc_lower_nfc(path)
}

/// The paths a directory already holds, compared by [`path_key`].
#[derive(Clone, Default)]
pub struct TakenPaths(HashSet<String>);

impl std::fmt::Debug for TakenPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TakenPaths")
            .field("count", &self.0.len())
            .finish()
    }
}

impl TakenPaths {
    pub fn from_paths<I: IntoIterator<Item = S>, S: AsRef<str>>(paths: I) -> Self {
        TakenPaths(paths.into_iter().map(|p| path_key(p.as_ref())).collect())
    }

    pub fn insert(&mut self, path: &str) {
        self.0.insert(path_key(path));
    }

    pub fn remove(&mut self, path: &str) {
        self.0.remove(&path_key(path));
    }

    pub fn contains(&self, path: &str) -> bool {
        self.0.contains(&path_key(path))
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The first free `<dir>/<stem><ext>`, then `<stem>-2<ext>`, `-3`, ….
/// `taken` decides what is free; pass one that compares by
/// [`path_key`] ([`TakenPaths`]). The file name stays within 255 bytes: a
/// suffix cuts the stem again on a character boundary.
pub fn free_path(dir: &str, stem: &str, ext: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let fit = |suffix: &str| {
        let room = MAX_FILE_NAME_BYTES.saturating_sub(suffix.len() + ext.len());
        let s = cut(stem, room);
        join(dir, &format!("{s}{suffix}{ext}"))
    };
    let first = fit("");
    if !taken(&first) {
        return first;
    }
    // The scan bounds a directory at 20,000 files, so a free name turns
    // up long before this; the cap only keeps the loop finite.
    let mut last = first;
    for n in 2..=1_000_000u32 {
        last = fit(&format!("-{n}"));
        if !taken(&last) {
            return last;
        }
    }
    last
}

/// Why a path was refused. Names no part of the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathProblem {
    NotUnderSeaquel,
    TooLong,
    EmptyPart,
    DotPart,
    HiddenPart,
    Backslash,
    ControlCharacter,
    ReservedName,
    TrailingDotOrSpace,
}

/// One path component's problem, if any: empty, `.` or `..`, hidden, a
/// backslash, NUL or another control character, a Windows reserved name
/// (also with an extension, `con.sql`), or a trailing dot or space.
pub fn check_component(part: &str) -> Result<(), PathProblem> {
    if part.is_empty() {
        return Err(PathProblem::EmptyPart);
    }
    if part == "." || part == ".." {
        return Err(PathProblem::DotPart);
    }
    if part.starts_with('.') {
        return Err(PathProblem::HiddenPart);
    }
    if part.contains('\\') {
        return Err(PathProblem::Backslash);
    }
    if part.chars().any(char::is_control) {
        return Err(PathProblem::ControlCharacter);
    }
    let base = part.split('.').next().unwrap_or(part);
    if is_reserved_stem(base.trim_end_matches(' ')) {
        return Err(PathProblem::ReservedName);
    }
    if part.ends_with('.') || part.ends_with(' ') {
        return Err(PathProblem::TrailingDotOrSpace);
    }
    Ok(())
}

/// A repo-relative path from a row or a request:
/// `.seaquel/` then `/`-separated components that pass
/// [`check_component`], at most 1,024 bytes.
pub fn check_rel_path(path: &str) -> Result<(), PathProblem> {
    if path.len() > MAX_PATH_BYTES {
        return Err(PathProblem::TooLong);
    }
    let rest = path
        .strip_prefix(SEAQUEL_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .ok_or(PathProblem::NotUnderSeaquel)?;
    rest.split('/').try_for_each(check_component)
}

/// A saved query's folder (`a/b`), as a path below `queries/`: each part
/// passes [`check_component`]. The empty folder is the root.
pub fn check_folder(folder: &str) -> Result<(), PathProblem> {
    if folder.is_empty() {
        return Ok(());
    }
    if folder.len() > MAX_PATH_BYTES {
        return Err(PathProblem::TooLong);
    }
    folder.split('/').try_for_each(check_component)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_stems_are_todays() {
        for (name, stem) in [
            ("Orders", "orders"),
            ("Sales!", "sales"),
            ("a  --  b", "a-b"),
            ("sales_report", "salesreport"),
            ("Straße", "strae"),
            ("Отчёт", "untitled"),
            (" -x- ", "x"),
            ("--x--", "x"),
        ] {
            assert_eq!(legacy_stem(name), stem, "{name:?}");
        }
    }

    #[test]
    fn paths_are_checked() {
        assert!(check_rel_path(".seaquel/projects/team/queries/a.sql").is_ok());
        for bad in [
            "projects/team/queries/a.sql",
            ".seaquel/../x.sql",
            ".seaquel/projects//a.sql",
            ".seaquel/projects/./a.sql",
            ".seaquel/projects/.git/a.sql",
            ".seaquel/projects/a\\b.sql",
            ".seaquel/projects/a\u{0}.sql",
            ".seaquel/projects/con.sql",
            ".seaquel/projects/COM1",
            ".seaquel/projects/a.",
            ".seaquel/projects/a ",
            "/.seaquel/projects/a.sql",
        ] {
            assert!(check_rel_path(bad).is_err(), "{bad:?}");
        }
        assert!(check_rel_path(&format!(".seaquel/{}", "a".repeat(1100))).is_err());
        assert!(check_folder("reports/2024").is_ok());
        assert!(check_folder("../x").is_err());
    }
}
