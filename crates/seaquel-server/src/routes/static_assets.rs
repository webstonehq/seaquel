//! Serve the embedded SvelteKit frontend.
//!
//! `rust-embed` bakes `build-web/` into the binary for release builds and
//! loads files from disk at runtime in debug builds, so the SvelteKit app can
//! be rebuilt (`npm run build:web`) without a Rust recompile during dev.
//!
//! Behavior:
//! - Known asset path → serve it with the right `Content-Type`.
//! - Unknown path → serve `index.html` so SvelteKit's client-side router can
//!   take over (standard SPA fallback).
//!
//! Mounted as the Axum router's `fallback(get(...))` so API routes take
//! precedence and non-GET requests to unknown paths get a 405 from Axum
//! rather than the SPA shell.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../build-web/"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');

    // Reject path-traversal attempts explicitly. `rust-embed`'s `get()` is
    // defensive in practice — the map is keyed on normalized relative paths
    // baked in at compile time, so `../etc/passwd` won't match anything —
    // but making the rejection explicit here keeps the guarantee local to
    // the handler rather than an implementation detail of an external crate.
    if raw.contains("..") {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }

    // SvelteKit adapter-static generates `index.html` as its fallback entry.
    let lookup = if raw.is_empty() { "index.html" } else { raw };

    if let Some(file) = Assets::get(lookup) {
        return asset_response(lookup, file.data.into_owned());
    }

    // SPA fallback — serve index.html so the client-side router handles the path.
    if let Some(file) = Assets::get("index.html") {
        return asset_response("index.html", file.data.into_owned());
    }

    // No frontend at all (shouldn't happen — build.rs guarantees a placeholder).
    (StatusCode::NOT_FOUND, "frontend not available").into_response()
}

fn asset_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = mime.as_ref().to_string();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .expect("failed to build asset response")
}
