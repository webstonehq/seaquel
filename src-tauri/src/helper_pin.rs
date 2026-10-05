// The DuckDB helper's pinned asset.
// Shared by `build.rs` (through `include!`) and the app, so
// the values the build checks are the values the app reads.
//
// `release.yml` builds and gzips the helper before the app and exports the
// `.gz`'s size and SHA-256 as `SEAQUEL_DUCKDB_HELPER_SIZE` and
// `SEAQUEL_DUCKDB_HELPER_SHA256`. `build.rs` parses them with
// [`pin_from_inputs`] (a value that doesn't parse, or only one of the two,
// fails the build) and hands the checked pin to rustc as
// `SEAQUEL_DUCKDB_HELPER_PIN` (`<size>:<sha256>`, [`pin_text`]), which the
// app reads with `option_env!` and [`compiled_pin`]. So the pin is compiled
// in, never read from the environment at run time. Without the variables
// (debug builds, a local `tauri build`) there is no pin and Core reads the
// release metadata, as before, unless `SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN=1`
// (`release.yml`), which makes a build without a pin fail.

/// The release step's inputs.
#[allow(dead_code)]
const SIZE_VAR: &str = "SEAQUEL_DUCKDB_HELPER_SIZE";
#[allow(dead_code)]
const SHA256_VAR: &str = "SEAQUEL_DUCKDB_HELPER_SHA256";

/// Set to `1` by `release.yml`: a build without a pin then fails.
#[allow(dead_code)]
const REQUIRE_VAR: &str = "SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN";

/// Whether [`REQUIRE_VAR`]'s value lets the build go on: unset, empty or
/// `0` asks nothing, `1` needs a pin, anything else is refused.
#[allow(dead_code)]
pub(crate) fn check_required(require: Option<&str>, pin: Option<&HelperPin>) -> Result<(), String> {
    match (require.map(str::trim).unwrap_or(""), pin) {
        ("" | "0", _) | ("1", Some(_)) => Ok(()),
        ("1", None) => Err(format!(
            "{REQUIRE_VAR}=1 but {SIZE_VAR} and {SHA256_VAR} aren't set: a release build needs the pin"
        )),
        _ => Err(format!("{REQUIRE_VAR} must be 1, 0 or unset")),
    }
}

/// The largest asset Core downloads (`seaquel_http::release_asset::
/// MAX_ASSET_BYTES`; a test keeps the two equal).
#[allow(dead_code)]
const MAX_PINNED_SIZE: u64 = 64 * 1024 * 1024;

/// The helper asset's compressed size and SHA-256 (64 lowercase hex).
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HelperPin {
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

/// The pin from the release step's variables: `Ok(None)` when neither is
/// set (or both are empty), an error naming the variable when one is
/// missing or doesn't parse. Surrounding space is ignored and the digest is
/// kept in lower case.
#[allow(dead_code)]
pub(crate) fn pin_from_inputs(
    size: Option<&str>,
    sha256: Option<&str>,
) -> Result<Option<HelperPin>, String> {
    let size = size.map(str::trim).filter(|v| !v.is_empty());
    let sha256 = sha256.map(str::trim).filter(|v| !v.is_empty());
    let (size, sha256) = match (size, sha256) {
        (None, None) => return Ok(None),
        (Some(_), None) => return Err(format!("{SIZE_VAR} is set but {SHA256_VAR} isn't")),
        (None, Some(_)) => return Err(format!("{SHA256_VAR} is set but {SIZE_VAR} isn't")),
        (Some(size), Some(sha256)) => (size, sha256),
    };
    let size = parse_size(size).ok_or_else(|| {
        format!("{SIZE_VAR} must be a whole number of bytes from 1 to {MAX_PINNED_SIZE}")
    })?;
    let sha256 = parse_sha256(sha256)
        .ok_or_else(|| format!("{SHA256_VAR} must be 64 hex digits (the .gz's SHA-256)"))?;
    Ok(Some(HelperPin { size, sha256 }))
}

#[allow(dead_code)]
fn parse_size(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse()
        .ok()
        .filter(|n| (1..=MAX_PINNED_SIZE).contains(n))
}

#[allow(dead_code)]
fn parse_sha256(text: &str) -> Option<String> {
    (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// `<size>:<sha256>`, what `build.rs` passes to rustc.
#[allow(dead_code)]
pub(crate) fn pin_text(pin: &HelperPin) -> String {
    format!("{}:{}", pin.size, pin.sha256)
}

/// The pin compiled in by `build.rs` (`option_env!`), if any.
///
/// # Panics
///
/// If the text doesn't read back as a pin: `build.rs` only ever
/// writes what [`pin_from_inputs`] accepted, so that is a broken build, not
/// a build without a pin.
#[allow(dead_code)]
pub(crate) fn compiled_pin(text: Option<&str>) -> Option<HelperPin> {
    let text = text?;
    let pin = text.split_once(':').and_then(|(size, sha256)| {
        Some(HelperPin {
            size: parse_size(size)?,
            sha256: parse_sha256(sha256)?,
        })
    });
    Some(pin.expect("the DuckDB helper's compiled-in pin reads back"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn pin(size: u64, sha256: &str) -> HelperPin {
        HelperPin {
            size,
            sha256: sha256.to_string(),
        }
    }

    #[test]
    fn neither_variable_is_no_pin() {
        assert_eq!(pin_from_inputs(None, None), Ok(None));
        assert_eq!(pin_from_inputs(Some(""), Some("  ")), Ok(None));
    }

    #[test]
    fn both_variables_are_a_pin() {
        assert_eq!(
            pin_from_inputs(Some("12051234"), Some(HEX)),
            Ok(Some(pin(12_051_234, HEX)))
        );
        // Upper case is taken and kept lower; surrounding space is trimmed.
        assert_eq!(
            pin_from_inputs(Some(" 1 "), Some(&format!(" {} ", HEX.to_uppercase()))),
            Ok(Some(pin(1, HEX)))
        );
        assert_eq!(
            pin_from_inputs(Some(&MAX_PINNED_SIZE.to_string()), Some(HEX)),
            Ok(Some(pin(MAX_PINNED_SIZE, HEX)))
        );
    }

    #[test]
    fn one_variable_alone_fails() {
        let e = pin_from_inputs(Some("10"), None).unwrap_err();
        assert!(e.contains(SHA256_VAR), "{e}");
        let e = pin_from_inputs(None, Some(HEX)).unwrap_err();
        assert!(e.contains(SIZE_VAR), "{e}");
        let e = pin_from_inputs(Some(""), Some(HEX)).unwrap_err();
        assert!(e.contains(SIZE_VAR), "{e}");
    }

    #[test]
    fn a_size_that_doesn_t_parse_fails() {
        for bad in [
            "0",
            "-1",
            "+5",
            "1e6",
            "12a",
            "1 2",
            "0x10",
            "1.5",
            "99999999999999999999",
        ] {
            let e = pin_from_inputs(Some(bad), Some(HEX)).unwrap_err();
            assert!(e.contains(SIZE_VAR), "{bad}: {e}");
        }
        let past = (MAX_PINNED_SIZE + 1).to_string();
        assert!(pin_from_inputs(Some(&past), Some(HEX)).is_err());
    }

    #[test]
    fn a_digest_that_doesn_t_parse_fails() {
        let short = &HEX[1..];
        let long = format!("{HEX}0");
        let not_hex = format!("{}g", &HEX[1..]);
        let prefixed = format!("sha256:{HEX}");
        for bad in [short, &long, &not_hex, &prefixed, "x"] {
            let e = pin_from_inputs(Some("10"), Some(bad)).unwrap_err();
            assert!(e.contains(SHA256_VAR), "{bad}: {e}");
        }
    }

    #[test]
    fn the_compiled_text_reads_back() {
        let p = pin(4096, HEX);
        assert_eq!(pin_text(&p), format!("4096:{HEX}"));
        assert_eq!(compiled_pin(Some(&pin_text(&p))), Some(p));
        assert_eq!(compiled_pin(None), None);
    }

    /// Compiled text that doesn't read back is a broken build,
    /// not "no pin".
    #[test]
    fn compiled_text_that_doesn_t_read_back_panics() {
        let zero = format!("0:{HEX}");
        for bad in ["", "4096", zero.as_str(), "4096:abc"] {
            let r = std::panic::catch_unwind(|| compiled_pin(Some(bad)));
            assert!(r.is_err(), "{bad:?} was taken quietly");
        }
    }

    /// Review 2: `SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN=1` (set by `release.yml`)
    /// makes a build without a pin fail.
    #[test]
    fn a_required_pin_must_be_there() {
        let p = pin(1, HEX);
        assert_eq!(check_required(Some("1"), Some(&p)), Ok(()));
        let e = check_required(Some("1"), None).unwrap_err();
        assert!(e.contains(REQUIRE_VAR) && e.contains(SIZE_VAR), "{e}");
        for off in [None, Some(""), Some("0")] {
            assert_eq!(check_required(off, None), Ok(()), "{off:?}");
        }
        // Anything else is a mistake, not "off".
        let e = check_required(Some("true"), Some(&p)).unwrap_err();
        assert!(e.contains(REQUIRE_VAR), "{e}");
    }

    #[test]
    fn the_size_cap_is_core_s() {
        assert_eq!(
            MAX_PINNED_SIZE,
            seaquel_http::release_asset::MAX_ASSET_BYTES
        );
    }

    /// Whatever this build compiled in is a pin the app can use (`build.rs`
    /// let it through only after parsing it).
    #[test]
    fn this_build_s_pin_reads_back() {
        if let Some(text) = option_env!("SEAQUEL_DUCKDB_HELPER_PIN") {
            assert!(compiled_pin(Some(text)).is_some(), "{text}");
        }
    }
}
