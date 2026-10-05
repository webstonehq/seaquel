//! Answered question 2 of phase 5a: when the LRU evicts a user's workspace,
//! it closes that user's connections, streams and tunnels
//! (`Workspace::close_all`), and each of the user's open `/rpc/stream`
//! sockets gets `connectionClosed` with `WORKSPACE_EVICTED`. The cap is hard.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{next, open_stream, pg_form, quiet, send, start, start_run, until_end, Env};

fn evicted(connection_id: &str) -> Value {
    json!({"type": "connectionClosed", "connectionId": connection_id,
           "code": "WORKSPACE_EVICTED",
           "message": "The server closed this connection to free resources; reconnect to use it again."})
}

async fn wait_evicted(env: &Env, n: usize) {
    for _ in 0..500 {
        if env.state.workspaces.evicted() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "expected {n} evictions, got {}",
        env.state.workspaces.evicted()
    );
}

#[tokio::test]
async fn a_third_user_closes_the_first_users_connections() {
    let env = Env::new(2);
    let addr = env.serve().await;

    let u1a = env.connect("u1", pg_form()).await;
    let u1b = env.connect("u1", pg_form()).await;
    let mut u1_socket = open_stream(addr, "u1").await;
    let mut u1_second_tab = open_stream(addr, "u1").await;
    send(&mut u1_socket, &start("s1", &u1a, "SELECT hang()")).await;
    env.calls
        .wait("u1's query", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    let u2 = env.connect("u2", pg_form()).await;
    let mut u2_socket = open_stream(addr, "u2").await;
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 0);

    // u3 arrives: u1 is the least recently used.
    let _u3 = env.connect("u3", pg_form()).await;
    wait_evicted(&env, 1).await;
    assert!(!env.state.workspaces.contains("u1"));
    assert_eq!(env.state.workspaces.len(), 2);

    // Both of u1's connections are closed, and its stream is stopped.
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 2);
    env.calls
        .wait("u1's query to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;

    // Every u1 socket hears about both connections, and the stream on the
    // first ends with an error: CANCELLED when `close_all`'s stream cancel
    // wins, CONNECTION_CLOSED when its disconnect does.
    for (socket, streams) in [(&mut u1_socket, 1), (&mut u1_second_tab, 0)] {
        let mut closed = Vec::new();
        let mut stream = Vec::new();
        while closed.len() < 2 || stream.len() < streams {
            let frame = next(socket).await;
            if frame["type"] == "connectionClosed" {
                closed.push(frame);
            } else {
                stream.push(frame);
            }
        }
        closed.sort_by_key(|f| f["connectionId"].as_str().unwrap().to_string());
        let mut want = vec![evicted(&u1a), evicted(&u1b)];
        want.sort_by_key(|f| f["connectionId"].as_str().unwrap().to_string());
        assert_eq!(closed, want);
        for frame in stream {
            assert_eq!(frame["streamId"], "s1", "{frame}");
            assert_eq!(frame["event"]["type"], "error", "{frame}");
            let code = frame["event"]["code"].as_str().unwrap();
            assert!(
                code == "CANCELLED" || code == "CONNECTION_CLOSED",
                "{frame}"
            );
        }
    }
    // u2 heard nothing.
    quiet(&mut u2_socket, 200).await;

    // u2's connection still works; u1's old ids are gone (asking reopens
    // u1's workspace, which evicts u3, the least recently used now).
    let (status, body) = env
        .db(
            "u2",
            "query",
            json!({"connectionId": u2, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = env
        .db(
            "u1",
            "query",
            json!({"connectionId": u1a, "sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    wait_evicted(&env, 2).await;
    assert!(!env.state.workspaces.contains("u3"));

    // u1 reconnects, and the same socket hears about that workspace too
    // when it's evicted in turn.
    let again = env.connect("u1", pg_form()).await;
    send(&mut u1_socket, &start("s2", &again, "SELECT 1")).await;
    let got = until_end(&mut u1_socket, "s2", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done");
    let _ = env.connect("u2", pg_form()).await; // touch u2
    let _ = env.connect("u4", pg_form()).await; // evicts u1 again
    wait_evicted(&env, 3).await;
    let frame = next(&mut u1_socket).await;
    assert_eq!(frame, evicted(&again));
}

/// A request that holds the workspace while it's evicted ends cleanly:
/// storage still works, the connections are gone, and connecting says the
/// workspace is closed.
#[tokio::test]
async fn a_request_holding_an_evicted_workspace_ends_cleanly() {
    let env = Env::new(2);
    let c = env.connect("u1", pg_form()).await;
    let held = env
        .state
        .workspaces
        .get(&env.state.core, "u1")
        .await
        .unwrap();

    let _ = env.connect("u2", pg_form()).await;
    let _ = env.connect("u3", pg_form()).await;
    wait_evicted(&env, 1).await;

    let ws = held.workspace();
    let core = Arc::clone(&env.state.core);
    let err = ws.query(&core, &c, "SELECT 1", vec![]).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    let err = ws
        .connect(
            &core,
            seaquel_core::ConnectRequest::form(serde_json::from_value(pg_form()).unwrap()),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "WORKSPACE_CLOSED");
    seaquel_core::storage::app_state::set(ws.storage(), "k", Some("v"))
        .await
        .unwrap();
    assert!(ws.connection_ids(&core).is_empty());
    drop(held);
}

#[tokio::test]
async fn the_cap_is_hard() {
    let env = Env::new(2);
    for i in 0..6 {
        let _ = env.connect(&format!("u{i}"), pg_form()).await;
        assert!(env.state.workspaces.len() <= 2);
    }
    wait_evicted(&env, 4).await;
    // Each evicted user's connection was closed: six opened, four closed.
    assert_eq!(env.calls.opened.load(Ordering::SeqCst), 6);
    for _ in 0..100 {
        if env.calls.closed.load(Ordering::SeqCst) == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(env.calls.closed.load(Ordering::SeqCst), 4);
    assert_eq!(env.state.core.connection_count(), 2);
}

/// Evicting a user ends their running run: the statement in flight is
/// dropped, the later ones never run, and the run ends with an error
/// (`CANCELLED` or `CONNECTION_CLOSED`, whichever of `close_all`'s cancel
/// and disconnect wins), never `done`.
#[tokio::test]
async fn eviction_ends_a_run() {
    let env = Env::new(2);
    let addr = env.serve().await;
    let c = env.connect("u1", pg_form()).await;
    let mut socket = open_stream(addr, "u1").await;
    send(
        &mut socket,
        &start_run("r1", &c, "SELECT hang(); INSERT INTO t VALUES (1)", 100),
    )
    .await;
    env.calls
        .wait("u1's run", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;

    let _ = env.connect("u2", pg_form()).await;
    let _ = env.connect("u3", pg_form()).await;
    wait_evicted(&env, 1).await;
    env.calls
        .wait("the run's statement to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;

    let mut run = Vec::new();
    let got = until_end(&mut socket, "r1", &mut Vec::new()).await;
    run.extend(got);
    assert_eq!(run[0]["event"]["type"], "statementStart", "{run:?}");
    let last = run.last().unwrap();
    assert_eq!(last["type"], "run", "{last}");
    assert_eq!(last["event"]["type"], "error", "{last}");
    let code = last["event"]["code"].as_str().unwrap();
    assert!(code == "CANCELLED" || code == "CONNECTION_CLOSED", "{last}");
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(env.state.core.running_stream_count(), 0);
}

/// A web user's workspace doesn't poll its `meta.db`
/// for other connections' commits (nothing else writes it, and a poll per
/// open user would cost the server for nothing).
#[tokio::test]
async fn a_web_workspace_doesnt_poll_for_external_changes() {
    let env = Env::new(2);
    let held = env
        .state
        .workspaces
        .get(&env.state.core, "u1")
        .await
        .unwrap();
    assert!(!held.workspace().polls_external_changes());
}
