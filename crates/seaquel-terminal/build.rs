//! The terminal binaries' `--version` reports the desktop app's version,
//! since they ship with it. It's read from `src-tauri/Cargo.toml`, the
//! version the release steps bump, so no crate's own `0.1.0` ever shows.
//! (Moved from `seaquel-cli`'s `build.rs`.)

use std::path::Path;

include!("src/package_version.rs");

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=src/package_version.rs");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", manifest.display()));
    let version = package_version(&text)
        .unwrap_or_else(|| panic!("no [package] version in {}", manifest.display()));
    println!("cargo:rustc-env=SEAQUEL_APP_VERSION={version}");
}
