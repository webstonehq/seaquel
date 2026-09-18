use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use seaquel_server::{build_router, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn health_endpoint_returns_200_ok() {
    let app = build_router(AppState::default());

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = std::str::from_utf8(&body).unwrap();
    assert_eq!(body_str, "ok");
}
