//! `GET /rpc/stream`: one multiplexed WebSocket per browser session. Client
//! frames `{"op":"start"|"cancel","streamId",…}`, server frames `CoreEvent`
//! JSON. See `routes/rpc_stream.rs` for the protocol.

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

mod common;
use common::{
    event_types, next, open_stream, pg_form, quiet, send, start, start_page, start_run,
    start_run_with, start_table_page, start_with, until_end, Env,
};

const MAX_STREAMS: usize = 16;

#[tokio::test]
async fn the_user_header_is_required() {
    let env = Env::new(8);
    let addr = env.serve().await;
    for users in [vec![], vec!["../x"], vec![""], vec!["u1", "u2"]] {
        let mut req = format!("ws://{addr}/rpc/stream")
            .into_client_request()
            .unwrap();
        for user in &users {
            req.headers_mut()
                .append("x-seaquel-user", user.parse().unwrap());
        }
        match tokio_tungstenite::connect_async(req).await {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{users:?}");
                let body: serde_json::Value =
                    serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
                assert_eq!(body["code"], "INVALID_ARGUMENT", "{users:?}");
            }
            other => panic!("{users:?}: expected a refusal, got {other:?}"),
        }
    }
    // Nothing was opened for anyone.
    assert_eq!(env.state.workspaces.opened(), 0);
}

#[tokio::test]
async fn a_stream_sends_its_events_then_done() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(&mut ws, &start("s1", &c, "SELECT 1")).await;
    let got = until_end(&mut ws, "s1", &mut Vec::new()).await;
    assert_eq!(
        got,
        vec![
            json!({"type": "stream", "streamId": "s1", "event":
                   {"type": "batch", "columns": ["sql"], "rows": [["SELECT 1"]], "is_final": true}}),
            json!({"type": "stream", "streamId": "s1", "event": {"type": "done"}}),
        ]
    );

    // A query error is the stream's terminal error.
    send(&mut ws, &start("s2", &c, "SELECT fail()")).await;
    let got = until_end(&mut ws, "s2", &mut Vec::new()).await;
    assert_eq!(
        got,
        vec![json!({"type": "stream", "streamId": "s2", "event":
                    {"type": "error", "code": "QUERY_ERROR", "message": "Query failed: the fake query failed"}})]
    );
    // A finished id can be used again.
    send(&mut ws, &start("s1", &c, "SELECT 3")).await;
    let got = until_end(&mut ws, "s1", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done");
}

#[tokio::test]
async fn read_only_streams_take_the_read_only_path() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    let frame = start_with(
        "r1",
        json!({"connectionId": c, "sql": "SELECT three()", "readOnly": true, "maxRows": 2}),
    );
    send(&mut ws, &frame).await;
    let got = until_end(&mut ws, "r1", &mut Vec::new()).await;
    assert_eq!(
        got[0]["event"],
        json!({"type": "batch", "columns": ["n"], "rows": [[1], [2]], "is_final": true, "truncated": true})
    );
    // A write is refused before the driver.
    let frame = start_with(
        "r2",
        json!({"connectionId": c, "sql": "DELETE FROM t", "readOnly": true}),
    );
    send(&mut ws, &frame).await;
    let got = until_end(&mut ws, "r2", &mut Vec::new()).await;
    assert_eq!(got[0]["event"]["code"], "READ_ONLY");
    // maxRows without readOnly is refused, not ignored.
    let frame = start_with(
        "r3",
        json!({"connectionId": c, "sql": "SELECT 1", "maxRows": 2}),
    );
    send(&mut ws, &frame).await;
    let got = until_end(&mut ws, "r3", &mut Vec::new()).await;
    assert_eq!(got[0]["event"]["code"], "INVALID_OPTIONS");
    assert_eq!(env.calls.read_only.load(Ordering::SeqCst), 1);
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancel_stops_a_stream_with_nothing_more() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(&mut ws, &start("s1", &c, "SELECT hang()")).await;
    env.calls
        .wait("the query to start", |c| {
            c.hanging.load(Ordering::SeqCst) == 1
        })
        .await;
    send(&mut ws, &json!({"op": "cancel", "streamId": "s1"})).await;
    env.calls
        .wait("the query to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
    quiet(&mut ws, 200).await;
    assert_eq!(env.state.core.running_stream_count(), 0);

    // Cancelling an id that isn't running is ignored.
    send(&mut ws, &json!({"op": "cancel", "streamId": "nope"})).await;
    quiet(&mut ws, 100).await;
    // And doesn't hold back a later stream with that id.
    send(&mut ws, &start("nope", &c, "SELECT 1")).await;
    let got = until_end(&mut ws, "nope", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done");
}

/// A stream stopped by something other than this socket (here a `db.cancel`
/// on `/rpc`) ends with a `CANCELLED` error, so the client isn't left waiting.
#[tokio::test]
async fn a_stream_stopped_elsewhere_ends_with_cancelled() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(&mut ws, &start("s1", &c, "SELECT hang()")).await;
    env.calls
        .wait("the query to start", |c| {
            c.hanging.load(Ordering::SeqCst) == 1
        })
        .await;
    let (status, body) = env.db("alice", "cancel", json!({"streamId": "s1"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got = until_end(&mut ws, "s1", &mut Vec::new()).await;
    assert_eq!(
        got,
        vec![json!({"type": "stream", "streamId": "s1", "event":
                    {"type": "error", "code": "CANCELLED", "message": "The query was stopped."}})]
    );
}

#[tokio::test]
async fn streams_are_multiplexed() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c1 = env.connect("alice", pg_form()).await;
    let c2 = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    // A long one, then short ones on both connections, which finish while
    // the long one still runs.
    send(&mut ws, &start("long", &c1, "SELECT hang()")).await;
    env.calls
        .wait("the long query to start", |c| {
            c.hanging.load(Ordering::SeqCst) == 1
        })
        .await;
    send(&mut ws, &start("a", &c1, "SELECT 'a'")).await;
    send(&mut ws, &start("b", &c2, "SELECT 'b'")).await;
    let mut other = Vec::new();
    let a = until_end(&mut ws, "a", &mut other).await;
    let b = until_end(&mut ws, "b", &mut other).await;
    for frame in &other {
        // Only a or b frames can have arrived out of order; never "long".
        assert!(
            frame["streamId"] == "a" || frame["streamId"] == "b",
            "{frame}"
        );
    }
    let rows = |frames: &[serde_json::Value]| {
        frames
            .iter()
            .chain(other.iter())
            .filter(|f| f["event"]["type"] == "batch")
            .map(|f| (f["streamId"].clone(), f["event"]["rows"].clone()))
            .collect::<Vec<_>>()
    };
    assert!(rows(&a).contains(&(json!("a"), json!([["SELECT 'a'"]]))));
    assert!(rows(&b).contains(&(json!("b"), json!([["SELECT 'b'"]]))));
    assert_eq!(env.state.core.running_stream_count(), 1);

    send(&mut ws, &json!({"op": "cancel", "streamId": "long"})).await;
    env.calls
        .wait("the long query to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
}

#[tokio::test]
async fn a_socket_runs_at_most_sixteen_streams() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    for i in 0..MAX_STREAMS {
        send(&mut ws, &start(&format!("s{i}"), &c, "SELECT hang()")).await;
    }
    env.calls
        .wait("16 queries", |c| {
            c.hanging.load(Ordering::SeqCst) == MAX_STREAMS
        })
        .await;
    send(&mut ws, &start("extra", &c, "SELECT 1")).await;
    let frame = next(&mut ws).await;
    assert_eq!(frame["streamId"], "extra");
    assert_eq!(frame["event"]["type"], "error");
    assert_eq!(frame["event"]["code"], "TOO_MANY_STREAMS");
    assert_eq!(env.calls.query.load(Ordering::SeqCst), MAX_STREAMS);

    // Ending one frees its slot.
    send(&mut ws, &json!({"op": "cancel", "streamId": "s0"})).await;
    env.calls
        .wait("s0 to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
    let mut done = false;
    for _ in 0..50 {
        send(&mut ws, &start("extra", &c, "SELECT 1")).await;
        let got = until_end(&mut ws, "extra", &mut Vec::new()).await;
        if got.last().unwrap()["event"]["type"] == "done" {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(done, "the freed slot was never reused");
}

#[tokio::test]
async fn bad_frames_get_an_error_event_and_the_socket_keeps_working() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(&mut ws, &start("busy", &c, "SELECT hang()")).await;
    env.calls
        .wait("the query to start", |c| {
            c.hanging.load(Ordering::SeqCst) == 1
        })
        .await;

    let query = json!({"method": "db", "params": {"method": "query",
                       "params": {"connectionId": c, "sql": "SELECT 1"}}});
    let cases: Vec<(String, &str)> = vec![
        ("not json".into(), ""),
        (r#"{"op":"start","streamId":"x1""#.into(), ""),
        (r#"{"streamId":"x2"}"#.into(), "x2"),
        (r#"{"op":"start"}"#.into(), ""),
        (r#"{"op":"start","streamId":""}"#.into(), ""),
        (r#"{"op":"stop","streamId":"x3"}"#.into(), "x3"),
        (r#"{"op":"start","streamId":"x4"}"#.into(), "x4"),
        (
            r#"{"op":"start","streamId":"x5","request":"text"}"#.into(),
            "x5",
        ),
        // Not a stream.
        (
            json!({"op": "start", "streamId": "x6", "request": query}).to_string(),
            "x6",
        ),
        // `params` before `method`.
        (
            format!(
                r#"{{"op":"start","streamId":"x7","request":{{"params":{{"method":"queryStream","params":{{"connectionId":"{c}","streamId":"x7","sql":"SELECT 1"}}}},"method":"db"}}}}"#
            ),
            "x7",
        ),
        // The request's streamId isn't the frame's.
        (
            {
                let mut f = start("x8", &c, "SELECT 1");
                f["request"]["params"]["params"]["streamId"] = json!("other");
                f.to_string()
            },
            "x8",
        ),
        // Already running on this socket.
        (start("busy", &c, "SELECT 1").to_string(), "busy"),
    ];
    for (text, stream_id) in &cases {
        ws.send(Message::Text(text.clone())).await.unwrap();
        let frame = next(&mut ws).await;
        assert_eq!(frame["type"], "stream", "{text}: {frame}");
        assert_eq!(frame["streamId"], *stream_id, "{text}: {frame}");
        assert_eq!(frame["event"]["type"], "error", "{text}: {frame}");
        assert_eq!(
            frame["event"]["code"], "INVALID_ARGUMENT",
            "{text}: {frame}"
        );
    }
    ws.send(Message::Binary(b"{}".to_vec())).await.unwrap();
    let frame = next(&mut ws).await;
    assert_eq!(frame["event"]["code"], "INVALID_ARGUMENT", "{frame}");

    // Nothing ran, the running stream wasn't disturbed, and the socket
    // still serves.
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 1);
    assert_eq!(env.calls.dropped.load(Ordering::SeqCst), 0);
    send(&mut ws, &start("ok", &c, "SELECT 1")).await;
    let got = until_end(&mut ws, "ok", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done");
}

#[tokio::test]
async fn closing_or_dropping_the_socket_cancels_its_streams() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;

    // A close frame.
    let mut ws = open_stream(addr, "alice").await;
    for i in 0..3 {
        send(&mut ws, &start(&format!("s{i}"), &c, "SELECT hang()")).await;
    }
    env.calls
        .wait("3 queries", |c| c.hanging.load(Ordering::SeqCst) == 3)
        .await;
    ws.close(None).await.unwrap();
    env.calls
        .wait("3 drops", |c| c.dropped.load(Ordering::SeqCst) == 3)
        .await;

    // A lost connection.
    let mut ws = open_stream(addr, "alice").await;
    send(&mut ws, &start("s9", &c, "SELECT hang()")).await;
    env.calls
        .wait("the query", |c| c.hanging.load(Ordering::SeqCst) == 4)
        .await;
    drop(ws);
    env.calls
        .wait("the drop", |c| c.dropped.load(Ordering::SeqCst) == 4)
        .await;
    for _ in 0..100 {
        if env.state.core.running_stream_count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.core.running_stream_count(), 0);
}

#[tokio::test]
async fn a_frame_over_the_limit_closes_the_socket_and_cancels() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;
    send(&mut ws, &start("s1", &c, "SELECT hang()")).await;
    env.calls
        .wait("the query", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;

    let big = start("big", &c, &"x".repeat(9 * 1024 * 1024));
    let _ = ws.send(Message::Text(big.to_string())).await;
    env.calls
        .wait("the drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
    // The socket ends (a close frame, or a reset).
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the socket stayed open");
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 1);
}

/// Streams are scoped per workspace: another user can't use, stream on or
/// cancel alice's connection or stream, from their socket or from `/rpc`.
#[tokio::test]
async fn another_user_cant_reach_a_stream_or_connection() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let _bobs = env.connect("bob", pg_form()).await;
    let mut alice = open_stream(addr, "alice").await;
    let mut bob = open_stream(addr, "bob").await;

    send(&mut alice, &start("s1", &c, "SELECT hang()")).await;
    env.calls
        .wait("alice's query", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;

    // Bob streams on alice's connection.
    send(&mut bob, &start("b1", &c, "SELECT 1")).await;
    let got = until_end(&mut bob, "b1", &mut Vec::new()).await;
    assert_eq!(got[0]["event"]["code"], "CONNECTION_NOT_FOUND");

    // Bob cancels alice's stream id, from his socket and from /rpc; he even
    // starts a stream under the same id.
    send(&mut bob, &json!({"op": "cancel", "streamId": "s1"})).await;
    let (status, _) = env.db("bob", "cancel", json!({"streamId": "s1"})).await;
    assert_eq!(status, StatusCode::OK);
    send(&mut bob, &start("s1", &c, "SELECT 1")).await;
    let got = until_end(&mut bob, "s1", &mut Vec::new()).await;
    assert_eq!(got[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    quiet(&mut alice, 200).await;
    assert_eq!(env.calls.dropped.load(Ordering::SeqCst), 0);
    assert_eq!(env.state.core.running_stream_count(), 1);

    // Closing bob's socket doesn't touch alice's stream.
    bob.close(None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(env.calls.dropped.load(Ordering::SeqCst), 0);

    // Alice's own cancel works.
    send(&mut alice, &json!({"op": "cancel", "streamId": "s1"})).await;
    env.calls
        .wait("alice's drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
}

/// A batch too big for one reasonable frame arrives as several, in order,
/// with the columns on the first and `is_final` on the last only.
#[tokio::test]
async fn a_large_batch_is_split_into_frames() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;
    send(&mut ws, &start("big", &c, "SELECT big()")).await;

    let mut sizes = Vec::new();
    let mut frames = Vec::new();
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = msg else { continue };
        sizes.push(text.len());
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        let end = frame["event"]["type"] != "batch";
        frames.push(frame);
        if end {
            break;
        }
    }
    assert_eq!(frames.last().unwrap()["event"]["type"], "done");
    let batches = &frames[..frames.len() - 1];
    assert!(batches.len() >= 3, "{} batches", batches.len());
    for (i, frame) in batches.iter().enumerate() {
        assert!(
            sizes[i] <= 4 * 1024 * 1024 + 2 * 1024 * 1024,
            "{}",
            sizes[i]
        );
        assert_eq!(frame["event"]["is_final"], i == batches.len() - 1);
        assert_eq!(frame["event"]["columns"].is_array(), i == 0);
    }
    let ns: Vec<i64> = batches
        .iter()
        .flat_map(|f| f["event"]["rows"].as_array().unwrap().clone())
        .map(|row| row[0].as_i64().unwrap())
        .collect();
    assert_eq!(ns, (0..10).collect::<Vec<_>>());
}

/// A user has at most 8 sockets; one more is closed at once with 1013 and
/// a reason, and a slot frees up when a socket closes.
#[tokio::test]
async fn a_user_has_at_most_eight_sockets() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let mut open = Vec::new();
    for _ in 0..8 {
        open.push(open_stream(addr, "alice").await);
    }
    let mut ninth = open_stream(addr, "alice").await;
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ninth.next())
        .await
        .unwrap();
    match msg {
        Some(Ok(Message::Close(Some(frame)))) => {
            assert_eq!(u16::from(frame.code), 1013);
            assert!(
                frame.reason.starts_with("TOO_MANY_SOCKETS"),
                "{}",
                frame.reason
            );
        }
        other => panic!("expected a close, got {other:?}"),
    }
    // Another user isn't affected.
    let mut bob = open_stream(addr, "bob").await;
    quiet(&mut bob, 100).await;

    // Closing one frees a slot.
    let mut first = open.remove(0);
    first.close(None).await.unwrap();
    let mut ok = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut again = open_stream(addr, "alice").await;
        if let Ok(Some(Ok(Message::Close(_)))) =
            tokio::time::timeout(std::time::Duration::from_millis(100), again.next()).await
        {
            continue;
        }
        ok = true;
        break;
    }
    assert!(ok, "the slot never freed up");
}

// ── db.run and db.page ──

/// A run's events arrive as `run` frames with its `streamId`, statement by
/// statement, and end with one `done`; a page likewise.
#[tokio::test]
async fn a_run_streams_statements_over_the_socket() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(
        &mut ws,
        &start_run(
            "r1",
            &c,
            "SELECT 1; INSERT INTO t VALUES (1); SELECT fail()",
            100,
        ),
    )
    .await;
    let got = until_end(&mut ws, "r1", &mut Vec::new()).await;
    assert!(
        got.iter()
            .all(|f| f["type"] == "run" && f["streamId"] == "r1"),
        "{got:?}"
    );
    assert_eq!(
        event_types(&got),
        [
            "statementStart",
            "batch",
            "statementDone",
            "statementStart",
            "statementDone",
            "statementStart",
            "statementError",
            "done"
        ],
        "{got:?}"
    );
    assert_eq!(got[0]["event"]["sql"], "SELECT 1");
    assert_eq!(got[0]["event"]["kind"], "page");
    assert_eq!(got[4]["event"]["rowsAffected"], 3);
    assert_eq!(got[6]["event"]["code"], "QUERY_ERROR");
    assert_eq!(got[7]["event"]["succeeded"], false);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 1);

    // A page of a statement the run sent.
    let source = got[0]["event"]["source"]["sql"]
        .as_str()
        .unwrap()
        .to_string();
    send(&mut ws, &start_page("p1", &c, &source, 2, 10)).await;
    let got = until_end(&mut ws, "p1", &mut Vec::new()).await;
    assert!(
        got.iter()
            .all(|f| f["type"] == "run" && f["streamId"] == "p1"),
        "{got:?}"
    );
    assert_eq!(
        event_types(&got),
        ["statementStart", "batch", "statementDone", "done"],
        "{got:?}"
    );
    assert_eq!(got[0]["event"]["page"], 2);

    // A run with nothing to run is a plain done; a destructive one asks.
    send(&mut ws, &start_run("r2", &c, "-- nothing", 100)).await;
    let got = until_end(&mut ws, "r2", &mut Vec::new()).await;
    assert_eq!(
        got,
        vec![json!({"type": "run", "streamId": "r2", "event":
        {"type": "done", "statements": 0, "succeeded": false}})]
    );
    send(&mut ws, &start_run("r3", &c, "DELETE FROM t", 100)).await;
    let got = until_end(&mut ws, "r3", &mut Vec::new()).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["event"]["code"], "CONFIRM_REQUIRED");
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 1);
}

/// A run's batch too big for one frame arrives as several `run` batch
/// frames, in order, columns on the first and `is_final` on the last only,
/// between the statement's start and its done.
#[tokio::test]
async fn a_run_batch_over_4_mib_is_split_by_rows() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    // Streamed (page size 0), then paged: both kinds carry the big batch.
    for (id, page_size) in [("r0", 0), ("r1", 100)] {
        send(&mut ws, &start_run(id, &c, "SELECT big()", page_size)).await;
        let mut frames = Vec::new();
        loop {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let Message::Text(text) = msg else { continue };
            assert!(text.len() <= 6 * 1024 * 1024, "{}", text.len());
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(frame["type"], "run");
            assert_eq!(frame["streamId"], id);
            let end = frame["event"]["type"] == "done" || frame["event"]["type"] == "error";
            frames.push(frame);
            if end {
                break;
            }
        }
        let types = event_types(&frames);
        assert_eq!(types[0], "statementStart", "{types:?}");
        assert_eq!(
            &types[types.len() - 2..],
            ["statementDone", "done"],
            "{types:?}"
        );
        let batches = &frames[1..frames.len() - 2];
        assert!(batches.len() >= 3, "{types:?}");
        for (i, frame) in batches.iter().enumerate() {
            assert_eq!(frame["event"]["type"], "batch");
            assert_eq!(frame["event"]["is_final"], i == batches.len() - 1, "{i}");
            assert_eq!(frame["event"]["columns"].is_array(), i == 0, "{i}");
        }
        let ns: Vec<i64> = batches
            .iter()
            .flat_map(|f| f["event"]["rows"].as_array().unwrap().clone())
            .map(|row| row[0].as_i64().unwrap())
            .collect();
        assert_eq!(ns, (0..10).collect::<Vec<_>>(), "{id}");
        assert_eq!(frames[frames.len() - 2]["event"]["totalRows"], 10);
    }
}

/// A run takes one of the socket's 16 slots however many statements it
/// has, and a refused run start answers with a `run` error.
#[tokio::test]
async fn a_run_counts_as_one_of_16_streams() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    for i in 0..MAX_STREAMS - 1 {
        send(&mut ws, &start(&format!("s{i}"), &c, "SELECT hang()")).await;
    }
    send(
        &mut ws,
        &start_run("run", &c, "SELECT hang(); SELECT 1; SELECT 2", 100),
    )
    .await;
    env.calls
        .wait("16 queries", |c| {
            c.hanging.load(Ordering::SeqCst) == MAX_STREAMS
        })
        .await;
    assert_eq!(next(&mut ws).await["event"]["type"], "statementStart");

    for frame in [
        start_run("extra-run", &c, "SELECT 1", 100),
        start_page("extra-page", &c, "SELECT 1", 1, 100),
    ] {
        send(&mut ws, &frame).await;
        let got = next(&mut ws).await;
        assert_eq!(got["type"], "run", "{got}");
        assert_eq!(got["streamId"], frame["streamId"], "{got}");
        assert_eq!(got["event"]["type"], "error", "{got}");
        assert_eq!(got["event"]["code"], "TOO_MANY_STREAMS", "{got}");
    }
    send(&mut ws, &start("extra", &c, "SELECT 1")).await;
    let got = next(&mut ws).await;
    assert_eq!(got["type"], "stream", "{got}");
    assert_eq!(got["event"]["code"], "TOO_MANY_STREAMS", "{got}");
    assert_eq!(env.calls.query.load(Ordering::SeqCst), MAX_STREAMS);

    // Cancelling the run frees its slot, and its later statements never run.
    send(&mut ws, &json!({"op": "cancel", "streamId": "run"})).await;
    env.calls
        .wait("the run's statement to be dropped", |c| {
            c.dropped.load(Ordering::SeqCst) == 1
        })
        .await;
    let mut done = false;
    for _ in 0..50 {
        send(&mut ws, &start_run("extra-run", &c, "SELECT 1", 100)).await;
        let got = until_end(&mut ws, "extra-run", &mut Vec::new()).await;
        if got.last().unwrap()["event"]["type"] == "done" {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(done, "the freed slot was never reused");
    // 16 hanging, then the one "SELECT 1" that got through.
    assert_eq!(env.calls.query.load(Ordering::SeqCst), MAX_STREAMS + 1);
}

/// A run or page start whose request's `streamId` isn't the frame's is
/// refused with a `run` error under the frame's id, and nothing runs.
#[tokio::test]
async fn the_start_frame_stream_id_must_match_a_runs() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    let mut run = start_run("x1", &c, "SELECT 1", 100);
    run["request"]["params"]["params"]["streamId"] = json!("other");
    let mut page = start_page("x2", &c, "SELECT 1", 1, 100);
    page["request"]["params"]["params"]["streamId"] = json!("other");
    // A run request that doesn't parse (no target) is refused as a run too.
    let mut bad = start_run("x3", &c, "SELECT 1", 100);
    bad["request"]["params"]["params"]
        .as_object_mut()
        .unwrap()
        .remove("target");
    for frame in [run, page, bad] {
        send(&mut ws, &frame).await;
        let got = next(&mut ws).await;
        assert_eq!(got["type"], "run", "{got}");
        assert_eq!(got["streamId"], frame["streamId"], "{got}");
        assert_eq!(got["event"]["type"], "error", "{got}");
        assert_eq!(got["event"]["code"], "INVALID_ARGUMENT", "{got}");
    }
    quiet(&mut ws, 100).await;
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 0);
}

/// A cancel frame stops a run: the statement in flight is dropped, the rest
/// never run, and nothing more arrives for it.
#[tokio::test]
async fn a_cancel_frame_stops_a_run() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(
        &mut ws,
        &start_run_with(
            "r1",
            json!({"connectionId": c, "text": "SELECT hang(); INSERT INTO t VALUES (1)",
                   "target": {"type": "all"}, "pageSize": 100}),
        ),
    )
    .await;
    assert_eq!(next(&mut ws).await["event"]["type"], "statementStart");
    env.calls
        .wait("the statement", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    send(&mut ws, &json!({"op": "cancel", "streamId": "r1"})).await;
    env.calls
        .wait("the drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
    quiet(&mut ws, 200).await;
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);
    assert_eq!(env.state.core.running_stream_count(), 0);
}

/// A run stopped by something other than this socket (`db.cancel` on
/// `/rpc`) ends with a `run` `CANCELLED` error.
#[tokio::test]
async fn a_run_stopped_elsewhere_ends_with_cancelled() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(&mut ws, &start_run("r1", &c, "SELECT hang(); SELECT 2", 0)).await;
    env.calls
        .wait("the statement", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    let (status, body) = env.db("alice", "cancel", json!({"streamId": "r1"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got = until_end(&mut ws, "r1", &mut Vec::new()).await;
    assert_eq!(
        got.last().unwrap(),
        &json!({"type": "run", "streamId": "r1", "event":
                {"type": "error", "code": "CANCELLED", "message": "The query was stopped."}})
    );
    assert_eq!(event_types(&got), ["statementStart", "error"]);
}

/// Closing or losing the socket cancels its runs.
#[tokio::test]
async fn closing_the_socket_cancels_a_run() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;

    let mut ws = open_stream(addr, "alice").await;
    send(
        &mut ws,
        &start_run("r1", &c, "SELECT hang(); INSERT INTO t VALUES (1)", 100),
    )
    .await;
    env.calls
        .wait("the statement", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    ws.close(None).await.unwrap();
    env.calls
        .wait("the drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;

    let mut ws = open_stream(addr, "alice").await;
    send(&mut ws, &start_page("p1", &c, "SELECT hang()", 1, 100)).await;
    env.calls
        .wait("the page", |c| c.hanging.load(Ordering::SeqCst) == 2)
        .await;
    drop(ws);
    env.calls
        .wait("the page's drop", |c| c.dropped.load(Ordering::SeqCst) == 2)
        .await;
    for _ in 0..100 {
        if env.state.core.running_stream_count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(env.state.core.running_stream_count(), 0);
    assert_eq!(env.calls.execute.load(Ordering::SeqCst), 0);
}

/// Bob can't run or page on alice's connection, nor cancel her run from
/// his socket or `/rpc`, even with the same stream id.
#[tokio::test]
async fn another_users_run_is_connection_not_found() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let _bobs = env.connect("bob", pg_form()).await;
    let mut alice = open_stream(addr, "alice").await;
    let mut bob = open_stream(addr, "bob").await;

    send(&mut alice, &start_run("r1", &c, "SELECT hang()", 100)).await;
    env.calls
        .wait("alice's run", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    assert_eq!(next(&mut alice).await["event"]["type"], "statementStart");

    for frame in [
        start_run("r1", &c, "SELECT 1", 100),
        start_page("b2", &c, "SELECT 1", 1, 100),
    ] {
        send(&mut bob, &frame).await;
        let got = until_end(
            &mut bob,
            frame["streamId"].as_str().unwrap(),
            &mut Vec::new(),
        )
        .await;
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0]["type"], "run");
        assert_eq!(got[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    }
    send(&mut bob, &json!({"op": "cancel", "streamId": "r1"})).await;
    let (status, _) = env.db("bob", "cancel", json!({"streamId": "r1"})).await;
    assert_eq!(status, StatusCode::OK);
    quiet(&mut alice, 200).await;
    assert_eq!(env.calls.dropped.load(Ordering::SeqCst), 0);
    assert_eq!(env.state.core.running_stream_count(), 1);

    send(&mut alice, &json!({"op": "cancel", "streamId": "r1"})).await;
    env.calls
        .wait("alice's drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
}

// ── db.tablePage ──

/// A table page's events arrive as `run` frames with its `streamId`: the
/// built SELECT's start, its batch, its done, then one `done`. A partial
/// page runs no count.
#[tokio::test]
async fn a_table_page_streams_over_the_socket() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    send(
        &mut ws,
        &start_table_page(
            "t1",
            &c,
            "people",
            json!([{"column": "name", "op": "IN", "value": "a, b"}]),
            1,
            10,
        ),
    )
    .await;
    let got = until_end(&mut ws, "t1", &mut Vec::new()).await;
    assert!(
        got.iter()
            .all(|f| f["type"] == "run" && f["streamId"] == "t1"),
        "{got:?}"
    );
    assert_eq!(
        event_types(&got),
        ["statementStart", "batch", "statementDone", "done"],
        "{got:?}"
    );
    let start = &got[0]["event"];
    assert_eq!(start["kind"], "page");
    assert_eq!(start["queryType"], "select");
    assert_eq!(start["table"]["table"], "people");
    let sql = start["source"]["sql"].as_str().unwrap();
    assert!(
        sql.contains(r#""public"."people""#) && sql.contains("IN ($1, $2)"),
        "{sql}"
    );
    assert_eq!(start["source"]["params"], json!(["a", "b"]));
    // The fake answers one row: the paged SQL.
    let paged = got[1]["event"]["rows"][0][0].as_str().unwrap();
    assert!(
        paged.starts_with(sql) && paged.contains("LIMIT 11"),
        "{paged}"
    );
    assert_eq!(got[2]["event"]["totalRows"], 1);
    assert_eq!(got[3]["event"]["succeeded"], true);
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 1, "no count");
}

/// A table page start whose `streamId` isn't the frame's, or whose request
/// doesn't parse, is refused with a `run` error, and nothing runs.
#[tokio::test]
async fn a_bad_table_page_start_is_a_run_error() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    let mut mismatched = start_table_page("x1", &c, "t", json!([]), 1, 10);
    mismatched["request"]["params"]["params"]["streamId"] = json!("other");
    let bad_op = start_table_page(
        "x2",
        &c,
        "t",
        json!([{"column": "a", "op": "eq", "value": "1"}]),
        1,
        10,
    );
    for frame in [mismatched, bad_op] {
        send(&mut ws, &frame).await;
        let got = next(&mut ws).await;
        assert_eq!(got["type"], "run", "{got}");
        assert_eq!(got["streamId"], frame["streamId"], "{got}");
        assert_eq!(got["event"]["type"], "error", "{got}");
        assert_eq!(got["event"]["code"], "INVALID_ARGUMENT", "{got}");
    }
    quiet(&mut ws, 100).await;
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 0);
}

/// A cancel frame stops a table page: its query is dropped and nothing
/// more arrives for it.
#[tokio::test]
async fn a_cancel_frame_stops_a_table_page() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    // The fake hangs on SQL naming `hang`.
    send(
        &mut ws,
        &start_table_page("t1", &c, "hang", json!([]), 1, 10),
    )
    .await;
    assert_eq!(next(&mut ws).await["event"]["type"], "statementStart");
    env.calls
        .wait("the page", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    send(&mut ws, &json!({"op": "cancel", "streamId": "t1"})).await;
    env.calls
        .wait("the drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
    quiet(&mut ws, 200).await;
    assert_eq!(env.state.core.running_stream_count(), 0);
}

/// A table page stopped by something other than this socket (`db.cancel`
/// on `/rpc`) ends with a `run` `CANCELLED` error, and a table page takes
/// one of the socket's 16 slots.
#[tokio::test]
async fn a_table_page_stopped_elsewhere_ends_with_cancelled() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    for i in 0..MAX_STREAMS - 1 {
        send(&mut ws, &start(&format!("s{i}"), &c, "SELECT hang()")).await;
    }
    send(
        &mut ws,
        &start_table_page("t1", &c, "hang", json!([]), 1, 10),
    )
    .await;
    env.calls
        .wait("16 queries", |c| {
            c.hanging.load(Ordering::SeqCst) == MAX_STREAMS
        })
        .await;
    let extra = start_table_page("t2", &c, "t", json!([]), 1, 10);
    send(&mut ws, &extra).await;
    let mut other = Vec::new();
    let got = until_end(&mut ws, "t2", &mut other).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["type"], "run");
    assert_eq!(got[0]["event"]["code"], "TOO_MANY_STREAMS");

    let (status, body) = env.db("alice", "cancel", json!({"streamId": "t1"})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got = until_end(&mut ws, "t1", &mut other).await;
    assert_eq!(
        got.last().unwrap(),
        &json!({"type": "run", "streamId": "t1", "event":
                {"type": "error", "code": "CANCELLED", "message": "The query was stopped."}})
    );
    let mut all = other;
    all.extend(got);
    let mine: Vec<_> = all.into_iter().filter(|f| f["streamId"] == "t1").collect();
    assert_eq!(event_types(&mine), ["statementStart", "error"], "{mine:?}");
}

/// Bob can't page alice's connection, nor cancel her page.
#[tokio::test]
async fn another_users_table_page_is_connection_not_found() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut alice = open_stream(addr, "alice").await;
    let mut bob = open_stream(addr, "bob").await;

    send(
        &mut alice,
        &start_table_page("t1", &c, "hang", json!([]), 1, 10),
    )
    .await;
    env.calls
        .wait("alice's page", |c| c.hanging.load(Ordering::SeqCst) == 1)
        .await;
    send(&mut bob, &start_table_page("t1", &c, "t", json!([]), 1, 10)).await;
    let got = until_end(&mut bob, "t1", &mut Vec::new()).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["type"], "run");
    assert_eq!(got[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    let (status, _) = env.db("bob", "cancel", json!({"streamId": "t1"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(env.calls.dropped.load(Ordering::SeqCst), 0);
    assert_eq!(env.state.core.running_stream_count(), 1);
    send(&mut alice, &json!({"op": "cancel", "streamId": "t1"})).await;
    env.calls
        .wait("alice's drop", |c| c.dropped.load(Ordering::SeqCst) == 1)
        .await;
}

/// The web's table page limits: too many filters, `IN` items, or too long
/// a value is one `INVALID_ARGUMENT` run error, and nothing runs.
#[tokio::test]
async fn the_web_table_page_limits_apply() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    let filter = json!({"column": "name", "op": "=", "value": "x"});
    let in_list = (0..1_001)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    for (id, filters) in [
        ("f", json!(vec![filter; 101])),
        (
            "i",
            json!([{"column": "name", "op": "IN", "value": in_list}]),
        ),
        (
            "v",
            json!([{"column": "name", "op": "=", "value": "x".repeat(64 * 1024 + 1)}]),
        ),
    ] {
        send(&mut ws, &start_table_page(id, &c, "t", filters, 1, 10)).await;
        let got = until_end(&mut ws, id, &mut Vec::new()).await;
        assert_eq!(got.len(), 1, "{id}: {got:?}");
        assert_eq!(got[0]["type"], "run");
        assert_eq!(got[0]["event"]["code"], "INVALID_ARGUMENT", "{id}");
    }
    assert_eq!(env.calls.query.load(Ordering::SeqCst), 0);
}

/// A table page's batch too big for one frame is split by rows like a
/// run's: columns on the first piece, `is_final` on the last.
#[tokio::test]
async fn a_table_page_batch_over_4_mib_is_split_by_rows() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    // The fake answers ten 1 MiB rows for SQL naming `big`.
    send(
        &mut ws,
        &start_table_page("t1", &c, "big", json!([]), 1, 100),
    )
    .await;
    let got = until_end(&mut ws, "t1", &mut Vec::new()).await;
    let types = event_types(&got);
    assert_eq!(types[0], "statementStart", "{types:?}");
    assert_eq!(&types[types.len() - 2..], ["statementDone", "done"]);
    let batches = &got[1..got.len() - 2];
    assert!(batches.len() >= 3, "{types:?}");
    for (i, frame) in batches.iter().enumerate() {
        assert_eq!(frame["type"], "run");
        assert_eq!(frame["event"]["type"], "batch");
        assert_eq!(frame["event"]["is_final"], i == batches.len() - 1, "{i}");
        assert_eq!(frame["event"]["columns"].is_array(), i == 0, "{i}");
    }
    let rows: usize = batches
        .iter()
        .map(|f| f["event"]["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(rows, 10);
}

/// Sixteen table pages fill a socket: the seventeenth start is a `run`
/// `TOO_MANY_STREAMS` error, and nothing more is queried.
#[tokio::test]
async fn sixteen_table_pages_fill_a_socket() {
    let env = Env::new(8);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let mut ws = open_stream(addr, "alice").await;

    for i in 0..MAX_STREAMS {
        send(
            &mut ws,
            &start_table_page(&format!("t{i}"), &c, "hang", json!([]), 1, 10),
        )
        .await;
    }
    env.calls
        .wait("16 pages", |c| {
            c.hanging.load(Ordering::SeqCst) == MAX_STREAMS
        })
        .await;
    send(&mut ws, &start_table_page("t16", &c, "t", json!([]), 1, 10)).await;
    let mut other = Vec::new();
    let got = until_end(&mut ws, "t16", &mut other).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["type"], "run", "{got:?}");
    assert_eq!(got[0]["event"]["type"], "error");
    assert_eq!(got[0]["event"]["code"], "TOO_MANY_STREAMS");
    assert!(
        other.iter().all(|f| f["event"]["type"] == "statementStart"),
        "{other:?}"
    );
    assert_eq!(env.calls.query.load(Ordering::SeqCst), MAX_STREAMS);
    assert_eq!(env.state.core.running_stream_count(), MAX_STREAMS);
}
