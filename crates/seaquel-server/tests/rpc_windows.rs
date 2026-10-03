//! Phase 6 probe F4: a closed tab's database connections don't stay open
//! until eviction.
//!
//! - A window (the `X-Seaquel-Origin` of `/rpc` and `/rpc/stream`) that
//!   connects a saved connection again replaces its older connection for
//!   it, so reloads don't pile connections up.
//! - When a window's last `/rpc/stream` socket closes and none returns
//!   within the grace period, its connections are closed and the user's
//!   other sockets hear `connectionClosed` with `WINDOW_CLOSED`. A reload
//!   within the grace period keeps them.
//!
//! The fake engine counts opens and closes; the live case counts Postgres
//! backends in `pg_stat_activity` (`SEAQUEL_TEST_POSTGRES`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{next, open_stream_from, pg_form, quiet, Env};

/// Connect `form` as `user` from window `origin`, for saved connection
/// `saved` (as the reconnect form and auto-reconnect do).
async fn connect_from(env: &Env, user: &str, origin: &str, form: Value, saved: &str) -> String {
    let body = common::db(
        "connect",
        json!({"target": {"type": "form", "form": form}, "savedConnectionId": saved}),
    );
    let (status, body) = env.rpc_from(user, &[origin], &body).await;
    assert_eq!(status, StatusCode::OK, "connect: {body}");
    body["result"]["result"]["connectionId"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn close(mut ws: common::Ws) {
    let _ = ws.close(None).await;
}

/// Wait until `f` holds, or panic after 5 s.
async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Five reloads of one tab (connect the same saved connections again from
/// the same window, each with a new socket) leave one connection per saved
/// connection; another tab's stay.
#[tokio::test]
async fn reloads_leave_one_connection_per_saved_connection() {
    let env = Env::new(4);
    let addr = env.serve().await;
    let other = connect_from(&env, "u1", "win-b", pg_form(), "conn-1").await;
    for _ in 0..5 {
        let ws = open_stream_from(addr, "u1", Some("win-a")).await;
        connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
        connect_from(&env, "u1", "win-a", pg_form(), "conn-2").await;
        close(ws).await;
    }
    // win-a's two, win-b's one.
    assert_eq!(env.state.core.connection_count(), 3);
    assert_eq!(env.calls.opened.load(Ordering::SeqCst), 11);
    until("the replaced connections to close", || {
        env.calls.closed.load(Ordering::SeqCst) == 8
    })
    .await;
    let (status, body) = env
        .db(
            "u1",
            "query",
            json!({"connectionId": other, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A window whose socket closes and doesn't come back within the grace
/// period loses its connections; the user's other tab hears about each one
/// (`WINDOW_CLOSED`) and keeps its own.
#[tokio::test]
async fn a_closed_windows_connections_close_after_the_grace_period() {
    let env = Env::with_window_grace(4, Duration::from_millis(300));
    let addr = env.serve().await;
    let a_socket = open_stream_from(addr, "u1", Some("win-a")).await;
    let mut b_socket = open_stream_from(addr, "u1", Some("win-b")).await;
    let a1 = connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
    let a2 = connect_from(&env, "u1", "win-a", pg_form(), "conn-2").await;
    let b = connect_from(&env, "u1", "win-b", pg_form(), "conn-1").await;
    close(a_socket).await;

    // Not at once: a reload gets the grace period.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(env.state.core.connection_count(), 3);
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 0);

    let mut frames = vec![next(&mut b_socket).await, next(&mut b_socket).await];
    frames.sort_by_key(|f| f["connectionId"].as_str().unwrap().to_string());
    let mut want: Vec<Value> = [&a1, &a2]
        .iter()
        .map(|id| {
            json!({"type": "connectionClosed", "connectionId": id, "code": "WINDOW_CLOSED",
                   "message": "The tab that opened this connection was closed."})
        })
        .collect();
    want.sort_by_key(|f| f["connectionId"].as_str().unwrap().to_string());
    assert_eq!(frames, want);
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 2);
    assert_eq!(env.state.core.connection_count(), 1);
    let (status, body) = env
        .db("u1", "query", json!({"connectionId": b, "sql": "SELECT 1"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = env
        .db(
            "u1",
            "query",
            json!({"connectionId": a1, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A window back within the grace period (a reload) keeps its
/// connections, and so does a window with a second socket still open.
#[tokio::test]
async fn a_window_back_within_the_grace_period_keeps_its_connections() {
    let env = Env::with_window_grace(4, Duration::from_millis(300));
    let addr = env.serve().await;
    let first = open_stream_from(addr, "u1", Some("win-a")).await;
    let c = connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
    close(first).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut again = open_stream_from(addr, "u1", Some("win-a")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 0);
    quiet(&mut again, 100).await;

    // Two sockets of one window: closing one keeps the connection.
    let second = open_stream_from(addr, "u1", Some("win-a")).await;
    close(second).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 0);
    let (status, body) = env
        .db("u1", "query", json!({"connectionId": c, "sql": "SELECT 1"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A socket without an origin, and another user's window of the same name,
/// reap nothing.
#[tokio::test]
async fn sockets_reap_only_their_own_users_window() {
    let env = Env::with_window_grace(4, Duration::from_millis(200));
    let addr = env.serve().await;
    let c = connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
    let no_origin = open_stream_from(addr, "u1", None).await;
    close(no_origin).await;
    let other_user = open_stream_from(addr, "u2", Some("win-a")).await;
    close(other_user).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 0);
    let (status, body) = env
        .db("u1", "query", json!({"connectionId": c, "sql": "SELECT 1"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Review I1 (a): a tab whose socket was down past the grace (a sleeping
/// laptop) asks `db.alive` when it is back: its reaped connections aren't
/// alive, another tab's are, another user's never are. Connecting again
/// from that tab gives it a working connection.
#[tokio::test]
async fn a_reaped_tabs_connections_arent_alive_and_reconnect() {
    let env = Env::with_window_grace(4, Duration::from_millis(200));
    let addr = env.serve().await;
    let socket = open_stream_from(addr, "u1", Some("win-a")).await;
    let reaped = connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
    let kept = connect_from(&env, "u1", "win-b", pg_form(), "conn-1").await;
    let theirs = connect_from(&env, "u2", "win-a", pg_form(), "conn-1").await;
    close(socket).await;
    until("the tab's connection to close", || {
        env.calls.closed.load(Ordering::SeqCst) == 1
    })
    .await;

    // Back: the socket opens again, and the page asks.
    let _socket = open_stream_from(addr, "u1", Some("win-a")).await;
    let alive = |ids: Vec<&String>| {
        let env = &env;
        let ids: Vec<String> = ids.into_iter().cloned().collect();
        async move {
            let (status, body) = env
                .rpc_from(
                    "u1",
                    &["win-a"],
                    &common::db("alive", json!({"connectionIds": ids})),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["result"]["result"].clone()
        }
    };
    assert_eq!(alive(vec![&reaped, &kept, &theirs]).await, json!([kept]));
    let again = connect_from(&env, "u1", "win-a", pg_form(), "conn-1").await;
    assert_eq!(alive(vec![&again]).await, json!([again]));

    // Too many ids is refused.
    let many: Vec<String> = (0..=seaquel_rpc::MAX_ALIVE_IDS)
        .map(|i| format!("c{i}"))
        .collect();
    let (status, body) = env
        .rpc_from(
            "u1",
            &["win-a"],
            &common::db("alive", json!({"connectionIds": many})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

// ── Live: Postgres backends ──

fn live_postgres() -> Option<String> {
    match std::env::var("SEAQUEL_TEST_POSTGRES") {
        Ok(raw) => {
            let config: Value = serde_json::from_str(&raw).expect("a ConnectConfig JSON");
            Some(config["connection_string"].as_str()?.to_string())
        }
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_POSTGRES is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => None,
    }
}

/// The probe's measurement, on the server's real Core: five page loads of
/// one tab used to hold 6 more backends each. Now they hold what one load
/// does, and none once the tab is closed past the grace period.
#[tokio::test]
async fn postgres_backends_stay_flat_across_reloads_and_go_with_the_tab() {
    let Some(base) = live_postgres() else { return };
    let app = format!("f4probe{}", std::process::id());
    let sep = if base.contains('?') { '&' } else { '?' };
    let tab = json!({"type": "postgres", "name": "live",
                     "connectionString": format!("{base}{sep}application_name={app}")});
    let observer_form = json!({"type": "postgres", "name": "observer", "connectionString": base});
    let env = Env::with_core_and_grace(
        Arc::new(seaquel_server::web_core(
            seaquel_core::ai::AiEgress::Public,
            None,
        )),
        4,
        Duration::from_millis(500),
    );
    let addr = env.serve().await;
    let observer = env.connect("observer", observer_form).await;
    let backends = || async {
        let (status, body) = env
            .db(
                "observer",
                "query",
                json!({"connectionId": observer,
                       "sql": format!("SELECT count(*)::int FROM pg_stat_activity WHERE application_name = '{app}'")}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["result"]["result"]["rows"][0][0].as_i64().unwrap()
    };
    // What a page load does: connect, then load the schema with several
    // calls at once, which fills the pool.
    let load = |ws_origin: &'static str| {
        let env = &env;
        let tab = tab.clone();
        async move {
            let c = connect_from(env, "alice", ws_origin, tab, "conn-live").await;
            let calls = (0..6).map(|_| {
                env.db(
                    "alice",
                    "query",
                    json!({"connectionId": c, "sql": "SELECT pg_sleep(0.2)"}),
                )
            });
            for (status, body) in futures::future::join_all(calls).await {
                assert_eq!(status, StatusCode::OK, "{body}");
            }
        }
    };

    let ws = open_stream_from(addr, "alice", Some("win-live")).await;
    load("win-live").await;
    let one_load = backends().await;
    assert!(one_load >= 1, "{one_load}");
    close(ws).await;
    for _ in 0..4 {
        let ws = open_stream_from(addr, "alice", Some("win-live")).await;
        load("win-live").await;
        close(ws).await;
    }
    let ws = open_stream_from(addr, "alice", Some("win-live")).await;
    load("win-live").await;
    // Closed backends leave pg_stat_activity a moment after the client goes.
    let mut after = backends().await;
    for _ in 0..50 {
        if after <= one_load {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        after = backends().await;
    }
    assert!(
        after <= one_load,
        "{after} backends after 6 loads, {one_load} after one"
    );

    // The tab goes: past the grace period its backends are gone.
    close(ws).await;
    let mut left = backends().await;
    for _ in 0..50 {
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        left = backends().await;
    }
    assert_eq!(left, 0);
    assert_eq!(env.state.core.connection_count(), 1, "the observer's");
}
