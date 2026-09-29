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
            json!({"connectionId": id, "request": {"method": "columnTypes"}}),
        ),
        (
            "planEdits",
            json!({"connectionId": id, "edits": [{"type": "deleteRow",
                   "target": {"schema": "public", "table": "t"}, "key": [["id", 1]]}]}),
        ),
        (
            "applyChanges",
            json!({"connectionId": id, "confirmed": true, "changes": [
                {"type": "edit", "id": "p1", "edit": {"type": "deleteRow",
                 "target": {"schema": "public", "table": "t"}, "key": [["id", 1]]}},
                {"type": "sql", "id": "p2", "sql": "DELETE FROM t"},
            ]}),
        ),
        (
            "duckdbExtension",
            json!({"connectionId": id, "action": {"type": "list"}}),
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
                json!({"connectionId": id, "request": {"method": "columnTypes"}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["result"]["result"]["kind"], "columnTypes");
    }
    let before = (
        env.calls.query.load(Ordering::SeqCst),
        env.calls.execute.load(Ordering::SeqCst),
        env.calls.transaction.load(Ordering::SeqCst),
        env.calls.metadata.load(Ordering::SeqCst),
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
    // Nothing reached a driver, not even a metadata read.
    assert_eq!(
        before,
        (
            env.calls.query.load(Ordering::SeqCst),
            env.calls.execute.load(Ordering::SeqCst),
            env.calls.transaction.load(Ordering::SeqCst),
            env.calls.metadata.load(Ordering::SeqCst),
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

// ── The edits service ──

/// Plan, apply and extension calls on `/rpc`: one DML-only batch is one
/// transaction, a batch with DDL runs in order, a destructive one asks
/// first (200, an outcome), a key that isn't the primary key is refused in
/// the outcome, and a refused plan gets its code's status.
#[tokio::test]
async fn edits_over_rpc() {
    let env = Env::new(8);
    let c = env.connect("alice", pg_form()).await;
    let target = json!({"schema": "public", "table": "t"});

    let (status, body) = env
        .db(
            "alice",
            "planEdits",
            json!({"connectionId": c, "edits": [
                {"type": "updateCell", "target": target, "key": [["id", 1]], "column": "name", "value": "x"},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let planned = &body["result"]["result"][0];
    assert_eq!(planned["queryType"], "update", "{body}");
    assert_eq!(planned["params"], json!(["x", 1]));
    assert_eq!(env.calls.metadata.load(Ordering::SeqCst), 1);

    // NOT_EDITABLE from a plan: 400.
    let (status, body) = env
        .db(
            "alice",
            "planEdits",
            json!({"connectionId": c, "edits": [
                {"type": "deleteRow", "target": target, "key": [["name", "x"]]},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "NOT_EDITABLE");

    let apply = |changes: Value, confirmed: bool| json!({"connectionId": c, "changes": changes, "confirmed": confirmed});
    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            apply(
                json!([
                    {"type": "edit", "id": "p1", "edit": {"type": "updateCell", "target": target,
                     "key": [["id", 1]], "column": "name", "value": "x"}},
                    {"type": "sql", "id": "p2", "sql": "INSERT INTO t VALUES (2)"},
                ]),
                false,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let outcome = &body["result"]["result"];
    assert_eq!(outcome["outcome"], "applied", "{body}");
    assert_eq!(outcome["mode"], "atomic");
    assert_eq!(outcome["applied"], 2);
    assert_eq!(env.calls.transaction.load(Ordering::SeqCst), 1);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);

    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            apply(
                json!([
                    {"type": "sql", "id": "p1", "sql": "CREATE TABLE u (a int)"},
                    {"type": "sql", "id": "p2", "sql": "INSERT INTO u VALUES (1)"},
                ]),
                false,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let outcome = &body["result"]["result"];
    assert_eq!(outcome["mode"], "inOrder", "{body}");
    assert_eq!(outcome["applied"], 2);
    assert_eq!(outcome["ddl"], true);
    assert_eq!(outcome["results"][0]["rowsAffected"], 3);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 2);

    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            apply(
                json!([{"type": "edit", "id": "d", "edit": {"type": "dropObject", "target": target, "kind": "table"}}]),
                false,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"]["outcome"], "confirmRequired");
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 2);

    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            apply(
                json!([{"type": "edit", "id": "k", "edit": {"type": "deleteRow", "target": target,
                        "key": [["name", "x"]]}}]),
                true,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"]["failed"]["code"], "NOT_EDITABLE");
    assert_eq!(body["result"]["result"]["applied"], 0);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 2);

    // Extensions are DuckDB's, which the web has none of.
    let (status, body) = env
        .db(
            "alice",
            "duckdbExtension",
            json!({"connectionId": c, "action": {"type": "install", "name": "httpfs"}}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "NOT_SUPPORTED");

    // A table page is a stream, not a call.
    let (status, body) = env
        .db(
            "alice",
            "tablePage",
            json!({"connectionId": c, "streamId": "t", "page": 1, "pageSize": 10,
                   "query": {"target": target}}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_ARGUMENT");
}

/// Bob can't plan, apply or run an extension action on alice's connection:
/// 404 `CONNECTION_NOT_FOUND`, and nothing reaches the driver.
#[tokio::test]
async fn another_users_apply_is_connection_not_found() {
    let env = Env::new(8);
    let a = env.connect("alice", pg_form()).await;
    let _b = env.connect("bob", pg_form()).await;
    for (method, params) in calls_on(&a).into_iter().skip(4) {
        let (status, body) = env.db("bob", method, params).await;
        not_found(status, &body, method);
    }
    assert_eq!(env.calls.metadata.load(Ordering::SeqCst), 0);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(env.calls.transaction.load(Ordering::SeqCst), 0);
}

/// The web's edit limits (`WEB_EDIT_LIMITS`): past each one the call is a
/// 400 `INVALID_ARGUMENT` and nothing is read or run.
#[tokio::test]
async fn the_web_edit_limits_apply() {
    let env = Env::new(8);
    let c = env.connect("alice", pg_form()).await;
    let target = json!({"schema": "public", "table": "t"});
    let sql = |i: usize| json!({"type": "sql", "id": format!("p{i}"), "sql": "DELETE FROM t WHERE id = 1"});
    let edit = |table: String| json!({"type": "deleteRow", "target": {"schema": "public", "table": table}, "key": [["id", 1]]});
    let cases = [
        (
            "applyChanges",
            json!({"connectionId": c, "changes": (0..10_001).map(sql).collect::<Vec<_>>()}),
        ),
        (
            "applyChanges",
            json!({"connectionId": c, "changes": [
                {"type": "sql", "id": "big", "sql": format!("DELETE FROM t WHERE a = '{}'", "x".repeat(2 * 1024 * 1024))}]}),
        ),
        (
            "applyChanges",
            json!({"connectionId": c, "changes": [
                {"type": "sql", "id": "v", "sql": "DELETE FROM t WHERE a = $1", "params": ["x".repeat(16 * 1024 * 1024 + 1)]}]}),
        ),
        (
            "planEdits",
            json!({"connectionId": c, "edits": (0..10_001).map(|_| edit("t".into())).collect::<Vec<_>>()}),
        ),
        (
            "planEdits",
            json!({"connectionId": c, "edits": (0..101).map(|i| edit(format!("t{i}"))).collect::<Vec<_>>()}),
        ),
        (
            "planEdits",
            json!({"connectionId": c, "edits": [{"type": "updateCell", "target": target, "key": [["id", 1]],
                   "column": "name", "value": "x".repeat(16 * 1024 * 1024 + 1)}]}),
        ),
    ];
    for (i, (method, params)) in cases.into_iter().enumerate() {
        let (status, body) = env.db("alice", method, params).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case {i}: {body}");
        assert_eq!(body["code"], "INVALID_ARGUMENT", "case {i}: {body}");
    }
    assert_eq!(env.calls.metadata.load(Ordering::SeqCst), 0);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(env.calls.transaction.load(Ordering::SeqCst), 0);
}
