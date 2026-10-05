//! Tauri's build step, and the DuckDB helper's pinned asset (the desktop
//! DuckDB helper plan, Decision 4): `release.yml` exports the helper
//! `.gz`'s size and SHA-256 as `SEAQUEL_DUCKDB_HELPER_SIZE` and
//! `SEAQUEL_DUCKDB_HELPER_SHA256`; they are checked here and compiled in as
//! `SEAQUEL_DUCKDB_HELPER_PIN`. A value that doesn't parse, or one of the
//! two alone, fails the build, and so does no pin with
//! `SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN=1` (set by `release.yml`). Without
//! them (debug builds, a local `tauri build`) the app has no pin and reads
//! the release metadata.

include!("src/helper_pin.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/helper_pin.rs");
    println!("cargo:rerun-if-env-changed={SIZE_VAR}");
    println!("cargo:rerun-if-env-changed={SHA256_VAR}");
    println!("cargo:rerun-if-env-changed={REQUIRE_VAR}");
    let size = std::env::var(SIZE_VAR).ok();
    let sha256 = std::env::var(SHA256_VAR).ok();
    let pin = pin_from_inputs(size.as_deref(), sha256.as_deref())
        .unwrap_or_else(|e| panic!("the DuckDB helper's pinned asset: {e}"));
    let require = std::env::var(REQUIRE_VAR).ok();
    if let Err(e) = check_required(require.as_deref(), pin.as_ref()) {
        panic!("the DuckDB helper's pinned asset: {e}");
    }
    if let Some(pin) = pin {
        println!(
            "cargo:rustc-env=SEAQUEL_DUCKDB_HELPER_PIN={}",
            pin_text(&pin)
        );
    }
    tauri_build::build()
}
