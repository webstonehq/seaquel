use std::path::Path;

fn main() {
    require_cli_sidecar();
    tauri_build::build()
}

/// `bundle.externalBin` lists `binaries/seaquel-cli`, and tauri-build copies
/// `binaries/seaquel-cli-<target-triple>[.exe]` next to the app on every build
/// (even `cargo check`). When it's missing, tauri-build's own error is a bare
/// "resource path doesn't exist"; say how to make it instead.
fn require_cli_sidecar() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    let exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let rel = format!("binaries/seaquel-cli-{target}{exe}");
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let path = Path::new(&manifest_dir).join(&rel);
    println!("cargo:rerun-if-changed={}", path.display());
    if !path.exists() {
        eprintln!(
            "\nsrc-tauri/{rel} is missing.\n\n\
             It is the seaquel-cli sidecar the app bundles. Build it with:\n\n    \
             npm run cli:build\n\n\
             (`npm run tauri dev` and `npm run tauri build` do this for you; for a cross build, \
             `node scripts/build-cli.mjs --target {target}`.)\n"
        );
        std::process::exit(1);
    }
}
