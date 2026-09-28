//! `db` calls on `POST /rpc` with two users: each one's connections are its
//! own. A connection or stream of the other user's is `CONNECTION_NOT_FOUND`
//! (404), the same as an id that doesn't exist, and is left untouched.

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{db, pg_form, Env};

fn not_found(status: StatusCode, body: &Value, what: &str) {
    assert_eq!(status, StatusCode::NOT_FOUND, "{what}: {body}");
    assert_eq!(body["code"], "CONNECTION_NOT_FOUND", "{what}: {body}");
}

/// Every call that takes a connection id, on `id`.
fn calls_on(id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "query",
            json!({"connectionId": id, "sql": "SELECT 1", "params": [1]}),
        ),
        (
            "execute",
            json!({"connectionId": id, "sql": "DELETE FROM t"}),
        ),
        (
            "transaction",
            json!({"connectionId": id, "statements": [{"sql": "UPDATE t SET a = 1", "params": []}]}),
        ),
        (
            "engine",
            json!({"connectionId": id, "request": {"method": "paginate",
                   "params": {"sql": "SELECT * FROM t", "limit": 10, "offset": 20}}}),
        ),
    ]
}

#[tokio::test]
async fn each_user_reaches_only_their_own_connections() {
    let env = Env::new(8);
    let a = env.connect("alice", pg_form()).await;
    let b = env.connect("bob", pg_form()).await;
    assert_ne!(a, b);
    // The web policy let the form through as a URL.
    assert_eq!(
        env.calls.configs.lock().unwrap()[0],
        "postgres://u@db.example.com/app"
    );

    // Each user's own calls work.
    for (user, id) in [("alice", &a), ("bob", &b)] {
        let (status, body) = env
            .db(
                user,
                "query",
                json!({"connectionId": id, "sql": "SELECT 1"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({"method": "db", "result": {"method": "query",
                   "result": {"columns": ["sql"], "rows": [["SELECT 1"]]}}})
        );
        let (status, body) = env
            .db(
                user,
                "execute",
                json!({"connectionId": id, "sql": "DELETE FROM t"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["result"]["result"],
            json!({"rows_affected": 3, "last_insert_id": null})
        );
        let (status, body) = env
            .db(
                user,
                "transaction",
                json!({"connectionId": id, "statements": [{"sql": "UPDATE t SET a = 1", "params": []}]}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, body) = env
            .db(
                user,
                "engine",
                json!({"connectionId": id, "request": {"method": "paginate",
                       "params": {"sql": "SELECT * FROM t", "limit": 10, "offset": 20}}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["result"]["result"],
            json!({"kind": "sql", "data": "SELECT * FROM t\nLIMIT 10 OFFSET 20"})
        );
    }
    let before = (
        env.calls.query.load(Ordering::SeqCst),
        env.calls.execute.load(Ordering::SeqCst),
        env.calls.transaction.load(Ordering::SeqCst),
    );

    // Each user is refused on the other's connection, and on one that
    // doesn't exist, with the same answer.
    for (user, other) in [("alice", &b), ("bob", &a)] {
        for id in [other.as_str(), "no-such-connection"] {
            for (method, params) in calls_on(id) {
                let (status, body) = env.db(user, method, params).await;
                not_found(status, &body, &format!("{user} {method} {id}"));
            }
            let (status, body) = env
                .db(user, "disconnect", json!({"connectionId": id}))
                .await;
            not_found(status, &body, &format!("{user} disconnect {id}"));
        }
    }
    // Nothing reached a driver.
    assert_eq!(
        before,
        (
            env.calls.query.load(Ordering::SeqCst),
            env.calls.execute.load(Ordering::SeqCst),
            env.calls.transaction.load(Ordering::SeqCst),
        )
    );

    // Both connections are still open for their owners.
    for (user, id) in [("alice", &a), ("bob", &b)] {
        let (status, body) = env
            .db(
                user,
                "query",
                json!({"connectionId": id, "sql": "SELECT 2"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{user}: {body}");
    }

    // Disconnect is the owner's; afterwards the id is gone for them too.
    let (status, body) = env
        .db("alice", "disconnect", json!({"connectionId": a}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"method": "db", "result": {"method": "disconnect", "result": null}})
    );
    let (status, body) = env
        .db(
            "alice",
            "query",
            json!({"connectionId": a, "sql": "SELECT 1"}),
        )
        .await;
    not_found(status, &body, "alice after disconnect");
    let (status, body) = env
        .db(
            "bob",
            "query",
            json!({"connectionId": b, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn test_opens_and_closes_without_keeping_a_connection() {
    let env = Env::new(8);
    let (status, body) = env
        .db(
            "alice",
            "test",
            json!({"target": {"type": "form", "form": pg_form()}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"method": "db", "result": {"method": "test", "result": null}})
    );
    assert_eq!(env.calls.opened.load(Ordering::SeqCst), 1);
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 1);
    assert_eq!(env.state.core.connection_count(), 0);
}

/// `db.cancel` names a stream in the caller's own workspace only; any id is
/// accepted, and one that isn't the caller's does nothing (the stream tests
/// check that the other user's stream keeps running).
#[tokio::test]
async fn cancel_answers_for_any_id() {
    let env = Env::new(8);
    let (status, body) = env
        .db("alice", "cancel", json!({"streamId": "whatever"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"method": "db", "result": {"method": "cancel", "result": null}})
    );
}

#[tokio::test]
async fn query_stream_isnt_a_single_call() {
    let env = Env::new(8);
    let a = env.connect("alice", pg_form()).await;
    let (status, body) = env
        .rpc(
            "alice",
            &db(
                "queryStream",
                json!({"connectionId": a, "streamId": "s", "sql": "SELECT 1"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_ARGUMENT");
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 0);
}
