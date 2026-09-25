//! Integration test: the server serves the embedded SvelteKit frontend at `/`,
//! falls back to `index.html` for unknown paths (SPA client-side routing), and
//! still routes API traffic correctly.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::post_json;
use http_body_util::BodyExt;
use seaquel_server::{build_router, AppState};
use serde_json::json;
use tower::ServiceExt;

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, bytes.to_vec())
}

/// `index.html` is either the real SvelteKit output or the `build.rs`
/// placeholder, depending on whether the frontend was built. Both contain
/// HTML we can cheaply sanity-check for.
fn looks_like_html(bytes: &[u8]) -> bool {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let lower = text.to_lowercase();
    lower.contains("<!doctype html") || lower.contains("<html")
}

#[tokio::test]
async fn root_serves_index_html() {
    let app = build_router(AppState::default());
    let (status, content_type, body) = get(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type
            .as_deref()
            .unwrap_or("")
            .starts_with("text/html"),
        "unexpected content-type: {content_type:?}"
    );
    assert!(
        looks_like_html(&body),
        "response body does not look like HTML: {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn unknown_path_falls_back_to_index_html() {
    // SvelteKit uses client-side routing. Any path the server doesn't know
    // about must be handed `index.html` so the SPA router can take over.
    let app = build_router(AppState::default());
    let (status, content_type, body) = get(app, "/dashboard/some/deep/route").await;
    assert_eq!(status, StatusCode::OK, "SPA fallback must return 200");
    assert!(
        content_type
            .as_deref()
            .unwrap_or("")
            .starts_with("text/html"),
        "SPA fallback must return HTML, got {content_type:?}"
    );
    assert!(looks_like_html(&body));
}

#[tokio::test]
async fn api_routes_take_precedence_over_static_fallback() {
    // /health is an API route. It must return plain "ok", not the SPA shell.
    let app = build_router(AppState::default());
    let (status, _content_type, body) = get(app, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        &body[..],
        b"ok",
        "API route must not be shadowed by SPA fallback"
    );
}

#[tokio::test]
async fn api_post_still_works_with_static_routes_mounted() {
    // Regression: adding static asset routing must not break the API surface.
    let app = build_router(AppState::default());
    let (status, body) = post_json(
        app,
        "/api/db/query",
        json!({ "connection_id": "does-not-exist", "sql": "SELECT 1", "values": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "CONNECTION_NOT_FOUND");
}
