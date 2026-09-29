//! The web connection limits on `/rpc`: one user holds at most 16
//! connections (tests and connects in flight included), each a pool of at
//! most 6; past the cap `db.connect` and `db.test` answer 429
//! `TOO_MANY_CONNECTIONS`.

use axum::http::StatusCode;
use seaquel_server::{web_core, WEB_CONNECTION_LIMITS, WEB_EDIT_LIMITS, WEB_LIBRARY_LIMITS};
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

#[test]
fn the_server_core_has_the_web_edit_limits() {
    let limits = web_core().edit_limits();
    assert_eq!(limits, WEB_EDIT_LIMITS);
    assert_eq!(limits.max_changes, Some(10_000));
    assert_eq!(limits.max_tables, Some(100));
    assert_eq!(limits.max_sql_bytes, Some(2 * 1024 * 1024));
    assert_eq!(limits.max_value_bytes, Some(16 * 1024 * 1024));
    assert_eq!(limits.max_filters, Some(100));
    assert_eq!(limits.max_in_values, Some(1_000));
    assert_eq!(limits.max_filter_value_bytes, Some(64 * 1024));
}

#[tokio::test]
async fn the_server_core_has_the_web_library_limits() {
    let limits = web_core().library_limits();
    assert_eq!(limits, WEB_LIBRARY_LIMITS);
    assert_eq!(limits.max_name_bytes, Some(1024));
    assert_eq!(limits.max_field_bytes, Some(64 * 1024));
    assert_eq!(limits.max_query_bytes, Some(2 * 1024 * 1024));
    assert_eq!(limits.max_list_items, Some(1_000));
    assert_eq!(limits.max_connections, Some(10_000));
    assert_eq!(limits.max_projects, Some(1_000));
    assert_eq!(limits.max_saved_queries, Some(50_000));
    // Phase 5d-1 probe fix: 8 versions of a query of `max_query_bytes`.
    assert_eq!(limits.max_version_bytes, Some(16 * 1024 * 1024));
    // The test server's Core has them too.
    assert_eq!(Env::new(1).state.core.library_limits(), WEB_LIBRARY_LIMITS);
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

/// Send `body` to `/rpc` as `user` from its own task; its status when it ends.
fn spawn_rpc(env: &Env, user: &'static str, body: String) -> tokio::task::JoinHandle<StatusCode> {
    let app = env.app.clone();
    tokio::spawn(async move {
        use tower::ServiceExt;
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/rpc")
            .header("content-type", "application/json")
            .header("x-seaquel-user", user)
            .body(axum::body::Body::from(body))
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    })
}

/// Wait until `n` of `tasks` have ended; their statuses.
async fn ended(tasks: &[tokio::task::JoinHandle<StatusCode>], n: usize) {
    for _ in 0..500 {
        if tasks.iter().filter(|t| t.is_finished()).count() >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {n} calls to end");
}

/// An apply of one typed change padded to about 900 KB, which hangs in the
/// fake driver when `hang` is true.
fn padded_apply(c: &str, hang: bool) -> String {
    let pad = "x".repeat(900 * 1024);
    let sql = format!(
        "UPDATE t SET a = 1 WHERE {} = 1 /* {pad} */",
        if hang { "hang" } else { "b" }
    );
    common::db(
        "applyChanges",
        json!({"connectionId": c, "confirmed": true,
               "changes": [{"type": "sql", "id": "c1", "sql": sql, "params": []}]}),
    )
    .to_string()
}

/// Probe review, M4: every `applyChanges`, `planEdits` and
/// `duckdbExtension` counts, whatever its size: a user runs at most 4 at
/// once, and the rest are 429 `TOO_MANY_REQUESTS`. Other calls and other
/// users aren't held up, and every slot is released when a call ends or is
/// dropped.
#[tokio::test]
async fn a_user_runs_at_most_four_edit_calls_at_once() {
    use std::sync::atomic::Ordering;
    assert_eq!(seaquel_server::MAX_EDIT_CALLS_PER_USER, 4);
    let env = Env::new(4);
    let c = env.connect("u1", pg_form()).await;
    let tasks: Vec<_> = (0..200)
        .map(|_| spawn_rpc(&env, "u1", padded_apply(&c, true)))
        .collect();
    ended(&tasks, 196).await;
    env.calls
        .wait("four hanging applies", |c| {
            c.hanging.load(Ordering::SeqCst) == 4
        })
        .await;
    let mut refused = 0;
    let mut held = Vec::new();
    for t in tasks {
        if t.is_finished() {
            assert_eq!(t.await.unwrap(), StatusCode::TOO_MANY_REQUESTS);
            refused += 1;
        } else {
            held.push(t);
        }
    }
    assert_eq!((refused, held.len()), (196, 4));
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 4);

    // A small apply is refused too; a small query, another user's apply
    // and the GUI's parallel storage loads run.
    let small = common::db(
        "applyChanges",
        json!({"connectionId": c, "confirmed": true,
               "changes": [{"type": "sql", "id": "c1", "sql": "UPDATE t SET a = 1 WHERE b = 1", "params": []}]}),
    );
    let (status, body) = env.rpc("u1", &small).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "TOO_MANY_REQUESTS", "{body}");
    let (status, body) = env
        .db("u1", "query", json!({"connectionId": c, "sql": "SELECT 1"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c2 = env.connect("u2", pg_form()).await;
    let (status, body) = env
        .rpc(
            "u2",
            &serde_json::from_str(&padded_apply(&c2, false)).unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let loads: Vec<_> = (0..100)
        .map(|_| {
            spawn_rpc(
                &env,
                "u1",
                json!({"method": "library", "params": {"method": "connectionsList"}}).to_string(),
            )
        })
        .collect();
    for t in loads {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }

    // Dropping the held applies frees their slots.
    for h in held {
        h.abort();
    }
    env.calls
        .wait("the held applies dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 4
        })
        .await;
    let (status, body) = env.rpc("u1", &small).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(env.state.workspaces.users_with_calls_in_flight(), 0);
}

/// Probe review, M4: a user's calls hold at most 40 MiB of bodies at once
/// (a lone call always runs); past it a call is 429 `TOO_MANY_REQUESTS`
/// before it's parsed.
#[tokio::test]
async fn a_user_holds_at_most_40_mib_of_bodies_at_once() {
    use std::sync::atomic::Ordering;
    let budget = seaquel_server::MAX_IN_FLIGHT_BYTES_PER_USER;
    assert_eq!(budget, 40 * 1024 * 1024);
    let env = Env::new(4);
    let c = env.connect("u1", pg_form()).await;
    let pad = "x".repeat(900 * 1024);
    let body = common::db(
        "query",
        json!({"connectionId": c, "sql": format!("SELECT hang /* {pad} */")}),
    )
    .to_string();
    let fits = budget / body.len();
    let tasks: Vec<_> = (0..fits + 5)
        .map(|_| spawn_rpc(&env, "u1", body.clone()))
        .collect();
    ended(&tasks, 5).await;
    env.calls
        .wait("the queries that fit hanging", |c| {
            c.hanging.load(Ordering::SeqCst) == fits
        })
        .await;
    let mut held = Vec::new();
    for t in tasks {
        if t.is_finished() {
            assert_eq!(t.await.unwrap(), StatusCode::TOO_MANY_REQUESTS);
        } else {
            held.push(t);
        }
    }
    assert_eq!(held.len(), fits);
    for h in held {
        h.abort();
    }
    env.calls
        .wait("the held queries dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == fits
        })
        .await;
    // With the budget full, a call under 64 KiB still runs, and counts
    // while it does; one of 64 KiB doesn't.
    let fill = "x".repeat(budget - 1024);
    let fill = spawn_rpc(
        &env,
        "u1",
        common::db(
            "query",
            json!({"connectionId": c, "sql": format!("SELECT hang /* {fill} */")}),
        )
        .to_string(),
    );
    env.calls
        .wait("the filling query hanging", |c| {
            c.hanging.load(Ordering::SeqCst) == fits + 1
        })
        .await;
    let (status, body) = env
        .rpc(
            "u1",
            &json!({"method": "library", "params": {"method": "connectionsList"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let near = |len: usize| {
        let sql = "SELECT 1 /*  */";
        let base = common::db("query", json!({"connectionId": c, "sql": sql}))
            .to_string()
            .len();
        let pad = "x".repeat(len - base);
        common::db(
            "query",
            json!({"connectionId": c, "sql": format!("SELECT 1 /* {pad} */")}),
        )
    };
    let small = near(seaquel_server::SMALL_CALL_BYTES - 1);
    assert_eq!(
        small.to_string().len(),
        seaquel_server::SMALL_CALL_BYTES - 1
    );
    let (status, body) = env.rpc("u1", &small).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = env.rpc("u1", &near(seaquel_server::SMALL_CALL_BYTES)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    fill.abort();
    env.calls
        .wait("the filling query dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == fits + 1
        })
        .await;
    assert_eq!(env.state.workspaces.users_with_calls_in_flight(), 0);

    // A lone call larger than the budget still runs.
    let huge = "x".repeat(budget + 1);
    let (status, body) = env
        .db(
            "u1",
            "query",
            json!({"connectionId": c, "sql": format!("SELECT 1 /* {huge} */")}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", body["code"]);
}
