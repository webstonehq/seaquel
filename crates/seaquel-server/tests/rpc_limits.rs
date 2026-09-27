//! The web connection limits on `/rpc`: one user holds at most 16
//! connections (tests and connects in flight included), each a pool of at
//! most 6; past the cap `db.connect` and `db.test` answer 429
//! `TOO_MANY_CONNECTIONS`.

use axum::http::StatusCode;
use seaquel_server::{web_core, WEB_CONNECTION_LIMITS};
use serde_json::json;

mod common;
use common::{pg_form, Env};

#[test]
fn the_server_core_has_the_web_limits() {
    let limits = web_core().connection_limits();
    assert_eq!(limits, WEB_CONNECTION_LIMITS);
    assert_eq!(limits.per_workspace, Some(16));
    assert_eq!(limits.max_pool_size, Some(6));
}

#[tokio::test]
async fn a_user_can_open_sixteen_connections() {
    let env = Env::new(4);
    let mut ids = Vec::new();
    for _ in 0..16 {
        ids.push(env.connect("u1", pg_form()).await);
    }
    let target = json!({"target": {"type": "form", "form": pg_form()}});
    for method in ["connect", "test"] {
        let (status, body) = env.db("u1", method, target.clone()).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{method}: {body}");
        assert_eq!(body["code"], "TOO_MANY_CONNECTIONS", "{method}: {body}");
    }
    // Refused before anything opened; every open got the web pool size.
    let sizes = env.calls.pool_sizes.lock().unwrap().clone();
    assert_eq!(sizes, vec![Some(6); 16]);

    // Another user isn't affected.
    env.connect("u2", pg_form()).await;
    // A disconnect frees a slot.
    let (status, _) = env
        .db("u1", "disconnect", json!({"connectionId": ids[0]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    env.connect("u1", pg_form()).await;
}
