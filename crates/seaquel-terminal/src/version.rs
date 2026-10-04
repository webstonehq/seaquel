//! The version and terms line both terminal binaries print. Neither checks
//! a license; `--version` and `--help` point to the terms instead (decision
//! 14 of the design doc).

/// The line `--version` and `--help` print about the terms. A macro so it
/// can go into `concat!`.
macro_rules! terms_line {
    () => {
        "Free for personal use; commercial use needs a license. Terms: https://seaquel.app/terms"
    };
}

/// The terms line printed by `--version` and `--help`.
pub const TERMS_LINE: &str = terms_line!();

/// The desktop app's version, which the terminal binaries share (see
/// `build.rs`).
pub const VERSION: &str = env!("SEAQUEL_APP_VERSION");

/// `--version`'s text: the version, then the terms line (clap puts the
/// binary's name first).
pub const VERSION_TEXT: &str = concat!(env!("SEAQUEL_APP_VERSION"), "\n", terms_line!());

#[cfg(test)]
mod tests {
    use super::*;

    include!("package_version.rs");

    #[test]
    fn the_version_is_the_apps() {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/Cargo.toml");
        let text = std::fs::read_to_string(manifest).unwrap();
        let app = package_version(&text).unwrap();
        assert_eq!(VERSION, app);
        assert!(
            !VERSION.starts_with("0.1."),
            "the app's version, not the crate's"
        );
        assert_eq!(VERSION_TEXT, format!("{app}\n{TERMS_LINE}"));
        assert!(TERMS_LINE.contains("https://seaquel.app/terms"));
    }
}
