//! The `library` group over `/rpc` (phase 5d-1): ownership, statuses, the
//! web limits, the origin header, and `storageChanged` on each of the
//! user's `/rpc/stream` sockets and no one else's.

use axum::http::StatusCode;
use serde_json::{json, Value as Json};

mod common;
use common::{next, open_stream, quiet, Env};

fn draft(project_id: &str, name: &str) -> Json {
    json!({"projectId": project_id, "name": name, "type": "postgres", "host": "db.example.com",
           "port": 5432, "databaseName": "app", "username": "u"})
}

/// A call that must succeed; its `{value, seq}`.
async fn ok(env: &Env, user: &str, method: &str, params: Json) -> Json {
    let (status, body) = env.library(user, Some("tab-1"), method, params).await;
    assert_eq!(status, StatusCode::OK, "{method}: {body}");
    assert_eq!(body["result"]["method"], method, "{body}");
    body["result"]["result"].clone()
}

fn id(v: &Json) -> String {
    v["value"]["id"].as_str().unwrap().to_string()
}

/// What `user` has in the library, without the `seq`s.
async fn everything(env: &Env, user: &str, project: &str) -> Json {
    json!({
        "connections": ok(env, user, "connectionsList", Json::Null).await["value"],
        "projects": ok(env, user, "projectsList", Json::Null).await["value"],
        "queries": ok(env, user, "savedQueriesList", json!({"projectId": project})).await["value"],
        "versions": ok(env, user, "queryVersionsList", json!({"projectId": project})).await["value"],
    })
}

#[tokio::test]
async fn another_users_ids_are_not_found() {
    let env = Env::new(4);
    // Alice's library: a project of her own, a label, a connection, a query.
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    let p = id(&ok(
        &env,
        "alice",
        "projectCreate",
        json!({"project": {"name": "Mine"}}),
    )
    .await);
    let label = id(&ok(
        &env,
        "alice",
        "labelCreate",
        json!({"projectId": p, "label": {"name": "L", "color": "#112233"}}),
    )
    .await);
    let c = id(&ok(
        &env,
        "alice",
        "connectionCreate",
        json!({"connection": draft(&p, "c")}),
    )
    .await);
    let q = id(&ok(
        &env,
        "alice",
        "savedQueryCreate",
        json!({"query": {"projectId": p, "name": "q", "query": "SELECT 1"}}),
    )
    .await);
    ok(
        &env,
        "alice",
        "savedQueryUpdate",
        json!({"id": q, "patch": {"query": "SELECT 2"}}),
    )
    .await;
    ok(&env, "bob", "projectEnsureDefault", Json::Null).await;
    let alice_before = everything(&env, "alice", &p).await;
    let bob_before = everything(&env, "bob", &p).await;

    for (method, params, code) in [
        (
            "connectionUpdate",
            json!({"id": c, "patch": {"name": "x"}}),
            "CONNECTION_NOT_FOUND",
        ),
        ("connectionRemove", json!({"id": c}), "CONNECTION_NOT_FOUND"),
        (
            "connectionCreate",
            json!({"connection": draft(&p, "b")}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "projectUpdate",
            json!({"id": p, "patch": {"name": "x"}}),
            "PROJECT_NOT_FOUND",
        ),
        ("projectRemove", json!({"id": p}), "PROJECT_NOT_FOUND"),
        (
            "labelCreate",
            json!({"projectId": p, "label": {"name": "x", "color": "#000000"}}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "labelUpdate",
            json!({"projectId": p, "labelId": label, "patch": {"name": "x"}}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "labelRemove",
            json!({"projectId": p, "labelId": label}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "savedQueryCreate",
            json!({"query": {"projectId": p, "name": "x", "query": "SELECT 1"}}),
            "PROJECT_NOT_FOUND",
        ),
        (
            "savedQueryUpdate",
            json!({"id": q, "patch": {"query": "x"}}),
            "SAVED_QUERY_NOT_FOUND",
        ),
        (
            "savedQueryRemove",
            json!({"id": q}),
            "SAVED_QUERY_NOT_FOUND",
        ),
    ] {
        let (status, body) = env.library("bob", Some("tab-1"), method, params).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}: {body}");
        assert_eq!(body["code"], code, "{method}: {body}");
    }
    // Reads naming her project see nothing of hers.
    assert_eq!(
        ok(&env, "bob", "savedQueriesList", json!({"projectId": p})).await["value"],
        json!([])
    );
    assert_eq!(
        ok(&env, "bob", "queryVersionsList", json!({"projectId": p})).await["value"],
        json!([])
    );
    assert_eq!(everything(&env, "alice", &p).await, alice_before);
    assert_eq!(everything(&env, "bob", &p).await, bob_before);
}

#[tokio::test]
async fn refusals_have_their_statuses() {
    let env = Env::new(4);
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    let projects = ok(&env, "alice", "projectsList", Json::Null).await;
    let p = projects["value"][0]["id"].as_str().unwrap().to_string();
    ok(
        &env,
        "alice",
        "connectionCreate",
        json!({"connection": draft(&p, "Prod")}),
    )
    .await;

    for (method, params, status, code) in [
        (
            "connectionCreate",
            json!({"connection": draft(&p, " prod ")}),
            StatusCode::CONFLICT,
            "NAME_TAKEN",
        ),
        (
            "projectRemove",
            json!({"id": p}),
            StatusCode::CONFLICT,
            "LAST_PROJECT",
        ),
        (
            "labelRemove",
            json!({"projectId": p, "labelId": "label-nope"}),
            StatusCode::NOT_FOUND,
            "LABEL_NOT_FOUND",
        ),
        (
            "connectionCreate",
            json!({"connection": draft(&p, "x"), "secrets": {"db": "pw"}}),
            StatusCode::NOT_IMPLEMENTED,
            "NOT_SUPPORTED",
        ),
    ] {
        let (got, body) = env.library("alice", None, method, params).await;
        assert_eq!(got, status, "{method}: {body}");
        assert_eq!(body["code"], code, "{method}: {body}");
    }
}

#[tokio::test]
async fn the_web_library_limits_apply() {
    let env = Env::new(4);
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    let p = "default-seaquel";
    let long_name = "n".repeat(1025);
    let long_field = "h".repeat(64 * 1024 + 1);
    let long_query = "x".repeat(2 * 1024 * 1024 + 1);
    let many: Vec<String> = (0..1001).map(|i| format!("t{i}")).collect();
    let mut sqlite = draft(p, "lite");
    sqlite["type"] = json!("sqlite");
    let mut far = draft(p, "far");
    far["host"] = json!(long_field);
    for (method, params, code) in [
        (
            "connectionCreate",
            json!({"connection": draft(p, &long_name)}),
            "INVALID_ARGUMENT",
        ),
        (
            "connectionCreate",
            json!({"connection": far}),
            "INVALID_ARGUMENT",
        ),
        (
            "connectionCreate",
            json!({"connection": sqlite}),
            "ENGINE_NOT_AVAILABLE",
        ),
        (
            "savedQueryCreate",
            json!({"query": {"projectId": p, "name": "big", "query": long_query}}),
            "INVALID_ARGUMENT",
        ),
        (
            "savedQueryCreate",
            json!({"query": {"projectId": p, "name": "tags", "query": "SELECT 1", "tags": many}}),
            "INVALID_ARGUMENT",
        ),
        (
            "projectCreate",
            json!({"project": {"name": long_name}}),
            "INVALID_ARGUMENT",
        ),
    ] {
        let (status, body) = env.library("alice", None, method, params).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method}: {body}");
        assert_eq!(body["code"], code, "{method}: {body}");
    }
    // Nothing was stored.
    assert_eq!(
        ok(&env, "alice", "connectionsList", Json::Null).await["value"],
        json!([])
    );
    assert_eq!(
        ok(&env, "alice", "savedQueriesList", json!({"projectId": p})).await["value"],
        json!([])
    );
}

/// Every one of the writer's sockets gets the event with the writer's
/// origin (the writing tab skips it by that), and no other user's does.
#[tokio::test]
async fn a_write_reaches_every_socket_of_that_user_and_none_of_another() {
    let env = Env::new(4);
    let addr = env.serve().await;
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    ok(&env, "bob", "projectEnsureDefault", Json::Null).await;
    let mut alice_1 = open_stream(addr, "alice").await;
    let mut alice_2 = open_stream(addr, "alice").await;
    let mut bob = open_stream(addr, "bob").await;

    let created = ok(
        &env,
        "alice",
        "connectionCreate",
        json!({"connection": draft("default-seaquel", "canary-name")}),
    )
    .await;
    let conn = id(&created);
    for ws in [&mut alice_1, &mut alice_2] {
        let event = next(ws).await;
        assert_eq!(
            event,
            json!({"type": "storageChanged", "kind": "connection", "scope": "default-seaquel",
                   "ids": [conn], "origin": "tab-1", "seq": created["seq"]})
        );
    }
    quiet(&mut bob, 200).await;

    // A storage-group write too, and a refused write sends nothing.
    let (status, _) = env
        .rpc_from(
            "alice",
            &["tab-2"],
            &json!({"method": "storage", "params": {"method": "userCredentialsSave",
                    "params": {"credential": {"scope": "db", "key": "k", "nonce": "n",
                        "ciphertext": "canary-value", "updatedAt": "t"}}}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = env
        .library(
            "alice",
            Some("tab-2"),
            "connectionCreate",
            json!({"connection": draft("default-seaquel", "canary-name")}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    for ws in [&mut alice_1, &mut alice_2] {
        let event = next(ws).await;
        assert_eq!(event["kind"], "storage", "{event}");
        assert_eq!(event["ids"], json!(["k"]), "{event}");
        assert_eq!(event["origin"], "tab-2", "{event}");
        assert!(!event.to_string().contains("canary"), "{event}");
        quiet(ws, 200).await;
    }
    quiet(&mut bob, 100).await;

    // Bob's write reaches only Bob.
    ok(
        &env,
        "bob",
        "connectionCreate",
        json!({"connection": draft("default-seaquel", "b")}),
    )
    .await;
    assert_eq!(next(&mut bob).await["kind"], "connection");
    quiet(&mut alice_1, 200).await;
}

#[tokio::test]
async fn a_bad_origin_header_is_ignored_not_refused() {
    let env = Env::new(4);
    let addr = env.serve().await;
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    let mut ws = open_stream(addr, "alice").await;
    let long = "x".repeat(65);
    for (i, origins) in [
        vec!["a b"],
        vec![long.as_str()],
        vec!["tab-1", "tab-2"],
        vec!["tab/1"],
        vec![],
    ]
    .into_iter()
    .enumerate()
    {
        let body = json!({"method": "library", "params": {"method": "savedQueryCreate",
            "params": {"query": {"projectId": "default-seaquel", "name": format!("q{i}"),
                                 "query": "SELECT 1"}}}});
        let (status, body) = env.rpc_from("alice", &origins, &body).await;
        assert_eq!(status, StatusCode::OK, "{origins:?}: {body}");
        let event = next(&mut ws).await;
        assert_eq!(event["type"], "storageChanged");
        assert_eq!(event["origin"], Json::Null, "{origins:?}: {event}");
    }
}

// ── Bounded event delivery (phase 5d review, I1) ──

use common::{open_stream_from, send, start, start_run_with};
use std::sync::atomic::Ordering;
use tokio_tungstenite::tungstenite::Message;

/// A socket whose writer is stuck behind 40 MB of results nobody reads: a
/// hanging query, then four 10 MB results. Returns the hanging stream's id.
async fn back_up(env: &Env, ws: &mut common::Ws) -> &'static str {
    let c = env.connect("alice", common::pg_form()).await;
    send(ws, &start("h", &c, "SELECT hang")).await;
    env.calls
        .wait("the hanging query", |calls| {
            calls.hanging.load(Ordering::SeqCst) == 1
        })
        .await;
    for i in 0..4 {
        send(ws, &start(&format!("b{i}"), &c, "SELECT big")).await;
    }
    "h"
}

/// `n` storage writes by alice, each one event on her sockets.
async fn flood(env: &Env, n: usize) {
    for i in 0..n {
        let (status, body) = env
            .rpc(
                "alice",
                &json!({"method": "storage", "params": {"method": "userCredentialsRemoveAllForKey",
                        "params": {"key": format!("k{i}")}}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

/// With events backed up behind a full outbox (and fewer waiting than the
/// bound), the socket still reads client frames: a cancel stops its query.
#[tokio::test]
async fn a_cancel_gets_through_while_events_are_backed_up() {
    let env = Env::new(4);
    let addr = env.serve().await;
    let mut ws = open_stream_from(addr, "alice", None).await;
    let hanging = back_up(&env, &mut ws).await;
    flood(&env, 200).await;
    send(&mut ws, &json!({"op": "cancel", "streamId": hanging})).await;
    env.calls
        .wait("the cancelled query dropped", |calls| {
            calls.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
}

/// Past the bound the socket closes with 1013 `EVENTS_LAGGED` (after what
/// it had queued), and the client can reconnect.
#[tokio::test]
async fn a_socket_that_falls_behind_on_events_closes_as_lagging() {
    let env = Env::with_event_bound(4, 16);
    let addr = env.serve().await;
    let mut ws = open_stream_from(addr, "alice", None).await;
    back_up(&env, &mut ws).await;
    flood(&env, 200).await;
    let close = loop {
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut ws),
        )
        .await
        .expect("no close within 10 s");
        match msg {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => {}
            other => panic!("the socket ended without a close frame: {other:?}"),
        }
    };
    let close = close.expect("a close frame with a code");
    assert_eq!(u16::from(close.code), 1013);
    assert!(
        close.reason.starts_with(seaquel_server::EVENTS_LAGGED),
        "{}",
        close.reason
    );
    // The hanging query was cancelled with the socket.
    env.calls
        .wait("the socket's streams cancelled", |calls| {
            calls.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
    // A new socket works and gets the next event.
    let mut again = open_stream_from(addr, "alice", None).await;
    flood(&env, 1).await;
    assert_eq!(next(&mut again).await["type"], "storageChanged");
}

/// `n` storage writes by alice whose keys (and so event ids) are
/// `key_len` bytes long.
async fn flood_keys(env: &Env, n: usize, key_len: usize) {
    for i in 0..n {
        let key = format!("{i:08}{}", "k".repeat(key_len.saturating_sub(8)));
        let (status, body) = env
            .rpc(
                "alice",
                &json!({"method": "storage", "params": {"method": "userCredentialsRemoveAllForKey",
                        "params": {"key": key}}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
}

/// Phase 5d-1 probe fix (c6): a storage key over 1 KiB is never an event
/// id, so one user's big keys can't make every socket's queue grow by
/// the key's size. The event is a reload of the kind.
#[tokio::test]
async fn an_oversized_key_is_announced_as_a_kind_reload() {
    let env = Env::new(4);
    let addr = env.serve().await;
    let mut ws = open_stream_from(addr, "alice", None).await;
    flood_keys(&env, 1, 1024).await;
    let event = next(&mut ws).await;
    assert_eq!(
        event["ids"].as_array().map(|ids| ids.len()),
        Some(1),
        "{event}"
    );
    flood_keys(&env, 1, 1025).await;
    let event = next(&mut ws).await;
    assert_eq!(event["type"], "storageChanged");
    assert_eq!(event["kind"], "storage");
    assert!(event["ids"].is_null(), "{event}");
    flood_keys(&env, 1, 8 * 1024 * 1024).await;
    let event = next(&mut ws).await;
    assert!(event["ids"].is_null(), "an 8 MiB key isn't an id");
    assert!(event.to_string().len() < 1024, "the event stays small");
}

/// Phase 5d-1 probe fix (c6): a socket's waiting events are bounded by
/// bytes too. Here 200 events of ~1 KiB, far below the 1,024-event bound,
/// pass a 64 KiB byte bound: the socket closes with 1013 `EVENTS_LAGGED`.
#[tokio::test]
async fn a_socket_past_its_event_byte_bound_closes_as_lagging() {
    let env = Env::with_event_byte_bound(4, 64 * 1024);
    let addr = env.serve().await;
    let mut ws = open_stream_from(addr, "alice", None).await;
    back_up(&env, &mut ws).await;
    flood_keys(&env, 200, 1000).await;
    let close = loop {
        let msg = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut ws),
        )
        .await
        .expect("no close within 10 s");
        match msg {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => {}
            other => panic!("the socket ended without a close frame: {other:?}"),
        }
    };
    let close = close.expect("a close frame with a code");
    assert_eq!(u16::from(close.code), 1013);
    assert!(
        close.reason.starts_with(seaquel_server::EVENTS_LAGGED),
        "{}",
        close.reason
    );
}

/// Phase 5d review, M1: a run started on a socket opened from tab `origin`
/// records its history with that origin.
#[tokio::test]
async fn a_runs_history_event_carries_the_sockets_origin() {
    let env = Env::new(4);
    let addr = env.serve().await;
    ok(&env, "alice", "projectEnsureDefault", Json::Null).await;
    let saved = id(&ok(
        &env,
        "alice",
        "connectionCreate",
        json!({"connection": draft("default-seaquel", "saved")}),
    )
    .await);
    let c = env.connect("alice", common::pg_form()).await;
    let mut ws = open_stream_from(addr, "alice", Some("tab-run")).await;
    send(
        &mut ws,
        &start_run_with(
            "r1",
            json!({"connectionId": c, "text": "SELECT 1", "target": {"type": "all"},
                   "pageSize": 10, "history": {"connectionId": saved, "connectionName": "n",
                   "connectionLabels": []}}),
        ),
    )
    .await;
    let history = loop {
        let frame = next(&mut ws).await;
        if frame["type"] == "storageChanged" && frame["kind"] == "history" {
            break frame;
        }
    };
    assert_eq!(history["scope"], json!(saved));
    assert_eq!(history["origin"], "tab-run");
}

/// Phase 5d review, I3: a client that sends refused frames and never reads
/// the refusals is closed once [`MAX_PENDING_REFUSALS`] of them wait,
/// instead of piling up refusals: the session ends (its queries are
/// cancelled) and its place among the user's sockets is freed.
///
/// What the client itself sees isn't asserted: it stopped reading, so the
/// 1008 `TOO_MANY_PENDING` frame waits behind the results it never read,
/// and a client blocked on its own writes may not read it at all. The
/// server has closed its end either way.
#[tokio::test]
async fn a_client_that_sends_refused_frames_without_reading_is_closed() {
    let env = Env::new(4);
    let addr = env.serve().await;
    let mut ws = open_stream_from(addr, "alice", None).await;
    // Back the outbox up so refusals wait, then send many bad frames.
    back_up(&env, &mut ws).await;
    let cap = seaquel_server::MAX_PENDING_REFUSALS;
    for _ in 0..cap * 4 {
        if futures::SinkExt::send(&mut ws, Message::Text("not json".into()))
            .await
            .is_err()
        {
            break;
        }
    }
    // The server ended the session: the socket's queries were cancelled.
    env.calls
        .wait("the socket's streams cancelled", |calls| {
            calls.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
    // Its listener is gone: the user can open the full allowance of
    // sockets again, and each gets events.
    for _ in 0..500 {
        if env.state.workspaces.listener_count("alice") == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.workspaces.listener_count("alice"), 0);
    let mut sockets = Vec::new();
    for _ in 0..seaquel_server::workspaces::MAX_LISTENERS_PER_USER {
        sockets.push(open_stream_from(addr, "alice", None).await);
    }
    flood(&env, 1).await;
    for socket in &mut sockets {
        assert_eq!(next(socket).await["type"], "storageChanged");
    }
    drop(ws);
}
