//! The desktop activation client against a local fake of the license
//! server's `/api/licenses/*`. Nothing here reaches the real server.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use seaquel_license::desktop::{DesktopClient, LicenseError};
use serde_json::{json, Value};

const KEY: &str = "SQ-TEST-9f3a-SECRET";

/// What the fake saw: path, content type and body of each request.
type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

#[derive(Clone)]
struct Fake {
    seen: Seen,
    /// The reply for every request: status and raw body.
    reply: (StatusCode, &'static str),
}

async fn record(
    State(fake): State<Fake>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: String,
) -> Response {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    fake.seen
        .lock()
        .unwrap()
        .push((uri.path().to_string(), content_type, body));
    let (status, text) = fake.reply;
    (status, [("content-type", "application/json")], text).into_response()
}

/// Start a fake answering every `/api/licenses/*` call with `reply`.
async fn fake(reply: (StatusCode, &'static str)) -> (DesktopClient, Seen) {
    let seen: Seen = Arc::default();
    let app = Router::new()
        .route("/api/licenses/{op}", post(record))
        .with_state(Fake {
            seen: seen.clone(),
            reply,
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The fake server runs beside the test; Core's no-spawn rule is for
    // library code.
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (DesktopClient::new(format!("http://{addr}")), seen)
}

const ACTIVE: &str = r#"{"id":"lic_1","status":"active","key":"SQ-TEST-9f3a-SECRET","tier":"business","activation":2,"activation_limit":5,"expires_at":"2027-01-01T00:00:00Z","instance_id":"inst_42","extra":"ignored"}"#;

#[tokio::test]
async fn activate_posts_key_and_instance_name_and_parses_the_license() {
    let (client, seen) = fake((StatusCode::OK, ACTIVE)).await;
    let license = client.activate(KEY, "host__me").await.unwrap();
    assert_eq!(license.id, "lic_1");
    assert_eq!(license.status, "active");
    assert_eq!(license.tier, "business");
    assert_eq!(license.activation, 2);
    assert_eq!(license.activation_limit, 5);
    assert_eq!(license.expires_at.as_deref(), Some("2027-01-01T00:00:00Z"));
    assert_eq!(license.instance_id.as_deref(), Some("inst_42"));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let (path, content_type, body) = &seen[0];
    assert_eq!(path, "/api/licenses/activate");
    assert_eq!(content_type, "application/json");
    assert_eq!(body, &json!({ "key": KEY, "instance_name": "host__me" }));
}

#[tokio::test]
async fn validate_and_deactivate_post_key_and_instance_id() {
    let (client, seen) = fake((StatusCode::OK, ACTIVE)).await;
    client.validate(KEY, "inst_42").await.unwrap();
    client.deactivate(KEY, "inst_42").await.unwrap();
    let seen = seen.lock().unwrap();
    let paths: Vec<_> = seen.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        ["/api/licenses/validate", "/api/licenses/deactivate"]
    );
    for (_, _, body) in seen.iter() {
        assert_eq!(body, &json!({ "key": KEY, "instance_id": "inst_42" }));
    }
}

#[tokio::test]
async fn nullable_fields_may_be_null() {
    let (client, _) = fake((
        StatusCode::OK,
        r#"{"id":"l","status":"expired","key":"k","tier":"individual","activation":0,"activation_limit":1,"expires_at":null,"instance_id":null}"#,
    ))
    .await;
    let license = client.validate(KEY, "i").await.unwrap();
    assert_eq!(license.status, "expired");
    assert_eq!(license.expires_at, None);
    assert_eq!(license.instance_id, None);
}

fn assert_no_key(e: &LicenseError) {
    for shown in [e.to_string(), format!("{e:?}"), e.message.clone()] {
        assert!(!shown.contains(KEY), "{shown}");
    }
}

#[tokio::test]
async fn an_error_status_gives_the_operations_code_with_status_and_body() {
    let body = r#"{"error":"license_not_found"}"#;
    for (op, code, verb) in [
        ("activate", "ACTIVATION_ERROR", "activation"),
        ("validate", "VALIDATION_ERROR", "validation"),
        ("deactivate", "DEACTIVATION_ERROR", "deactivation"),
    ] {
        let (client, _) = fake((StatusCode::NOT_FOUND, body)).await;
        let err = match op {
            "activate" => client.activate(KEY, "n").await,
            "validate" => client.validate(KEY, "i").await,
            _ => client.deactivate(KEY, "i").await,
        }
        .unwrap_err();
        assert_eq!(err.code, code);
        assert_eq!(
            err.message,
            format!("License {verb} failed (404 Not Found): {body}")
        );
        assert_no_key(&err);
    }
}

#[tokio::test]
async fn a_server_error_is_the_operations_code_too() {
    let (client, _) = fake((StatusCode::INTERNAL_SERVER_ERROR, "")).await;
    let err = client.validate(KEY, "i").await.unwrap_err();
    assert_eq!(err.code, "VALIDATION_ERROR");
    assert_eq!(
        err.message,
        "License validation failed (500 Internal Server Error): "
    );
}

#[tokio::test]
async fn a_body_that_isnt_a_license_is_a_parse_error() {
    for body in ["not json", r#"{"id":"l"}"#, r#"{"status":"active"}"#] {
        let (client, _) = fake((StatusCode::OK, body)).await;
        let err = client.activate(KEY, "n").await.unwrap_err();
        assert_eq!(err.code, "PARSE_ERROR", "{body}");
        assert!(
            err.message
                .starts_with("Invalid response from license server: "),
            "{}",
            err.message
        );
    }
}

#[tokio::test]
async fn no_server_is_a_network_error() {
    // Bind and drop, so the port is (almost certainly) closed.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let client = DesktopClient::new(format!("http://127.0.0.1:{port}"));
    let err = client.activate(KEY, "n").await.unwrap_err();
    assert_eq!(err.code, "NETWORK_ERROR");
    assert!(
        err.message
            .starts_with("Failed to connect to license server: "),
        "{}",
        err.message
    );
    assert_no_key(&err);
}

#[tokio::test]
async fn a_trailing_slash_on_the_base_url_is_harmless() {
    let (client, seen) = fake((StatusCode::OK, ACTIVE)).await;
    let with_slash = DesktopClient::new(format!("{}/", client.base_url()));
    with_slash.validate(KEY, "i").await.unwrap();
    assert_eq!(seen.lock().unwrap()[0].0, "/api/licenses/validate");
}

#[test]
fn debug_shows_the_base_url() {
    let client = DesktopClient::new("http://example.invalid");
    assert_eq!(
        format!("{client:?}"),
        r#"DesktopClient { base_url: "http://example.invalid" }"#
    );
}
