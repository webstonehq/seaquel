//! Ensure `build-web/` exists with at least a placeholder `index.html`.
//!
//! `rust-embed` requires its `folder = "..."` path to exist at compile time.
//! On a fresh clone, the SvelteKit output hasn't been produced yet — without
//! this shim, `cargo build -p seaquel-server` fails before the developer has
//! a chance to run `npm run build:web`. The placeholder explains the fix.
//!
//! In practice:
//! - Dev: `npm run build:web` overwrites the placeholder with the real app.
//! - Dockerfile (Phase 3d): builds the frontend before the Rust step, so the
//!   placeholder is replaced with the actual bundle before this build.rs even
//!   runs in the container.

use std::fs;
use std::path::Path;

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>seaquel-server</title>
  <style>
    body { font-family: -apple-system, system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem; line-height: 1.5; }
    code { background: #f4f4f4; padding: 0.1em 0.3em; border-radius: 3px; }
  </style>
</head>
<body>
  <h1>seaquel-server</h1>
  <p>The backend is running, but the frontend has not been built yet.</p>
  <p>Run <code>npm run build:web</code> at the repository root, then rebuild the server.</p>
</body>
</html>
"#;

fn main() {
    // Repo-root-relative path — seaquel-server's Cargo.toml lives in
    // crates/seaquel-server/, so `../../build-web` points at the top-level
    // build-web/ directory.
    let build_web = Path::new("../../build-web");

    if !build_web.exists() {
        fs::create_dir_all(build_web).expect("failed to create build-web/ placeholder directory");
    }

    let index = build_web.join("index.html");
    if !index.exists() {
        fs::write(&index, PLACEHOLDER_HTML).expect("failed to write placeholder index.html");
    }

    // If the frontend is rebuilt (index.html changes), redo this build step
    // so `rust-embed` picks up the new contents.
    println!("cargo:rerun-if-changed=../../build-web/index.html");
}
