//! `--version` and `helloOk` report the desktop app's version, which the
//! helper ships with and which its client checks. It's read from
//! `src-tauri/Cargo.toml`, as `seaquel-terminal`'s `build.rs` does, with
//! the same parser.

use std::path::Path;

include!("../seaquel-terminal/src/package_version.rs");

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed=../seaquel-terminal/src/package_version.rs");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", manifest.display()));
    let version = package_version(&text)
        .unwrap_or_else(|| panic!("no [package] version in {}", manifest.display()));
    println!("cargo:rustc-env=SEAQUEL_APP_VERSION={version}");
}
