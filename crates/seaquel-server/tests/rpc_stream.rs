//! `GET /rpc/stream`: one multiplexed WebSocket per browser session. Client
//! frames `{"op":"start"|"cancel","streamId",…}`, server frames `CoreEvent`
//! JSON. See `routes/rpc_stream.rs` for the protocol.

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

mod common;
use common::{next, open_stream, pg_form, quiet, send, start, start_with, until_end, Env};

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
