// Shared by `build.rs` (through `include!`) and the version test, so both
// read `src-tauri/Cargo.toml` the same way.

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
