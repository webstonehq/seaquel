//! `seaquel-cli --version` reports the desktop app's version, since the two
//! ship together. It's read from `src-tauri/Cargo.toml`, the version the
//! release steps bump, so this crate's own `0.1.0` never shows.

use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", manifest.display()));
    let version = package_version(&text)
        .unwrap_or_else(|| panic!("no [package] version in {}", manifest.display()));
    println!("cargo:rustc-env=SEAQUEL_APP_VERSION={version}");
}

/// The `version = "…"` line of the `[package]` table.
fn package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package {
            if let Some(value) = line.strip_prefix("version") {
                let value = value.trim_start().strip_prefix('=')?.trim();
                return value.strip_prefix('"')?.strip_suffix('"');
            }
        }
    }
    None
}
