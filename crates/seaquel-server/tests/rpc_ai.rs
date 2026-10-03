//! The assistant on web (phase 6 Task 5): `ai.chat` over `/rpc/stream`, the
//! unary `ai` calls over `/rpc`, the server's limits and egress, and Task
//! 4's contract that a closed (or lagging) socket stops a turn through
//! Core so its reply is stored. Every model call goes to a local mock
//! through a loopback-only client; the key is the fake test key.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use futures::StreamExt;
use seaquel_ai::testing::scripts::{openai_chunk, openai_done, openai_text};
use seaquel_ai::testing::{MockProvider, Reply, SseEnd, TEST_KEY};
use seaquel_core::ai::AiEgress;
use serde_json::{json, Value as Json};
use tokio_tungstenite::tungstenite::Message;

mod common;
use common::{next, open_stream, send, start, until_end, Env, Ws};

const USER: &str = "alice";

struct Ai {
    env: Env,
    mock: MockProvider,
    saved: String,
    provider: String,
    /// Core's id of the open saved connection.
    connection: String,
}

impl Ai {
    async fn new(env: Env) -> Self {
        let mock = MockProvider::start().await;
        lib(&env, "projectEnsureDefault", Json::Null).await;
        let projects = lib(&env, "projectsList", Json::Null).await;
        let project = projects["value"][0]["id"].as_str().unwrap().to_string();
        let (status, body) = env
            .rpc(
                USER,
                &json!({"method": "settings", "params": {"method": "aiProviderCreate", "params":
                    {"provider": {"name": "Mock", "type": "openai-compatible",
                        "baseUrl": format!("{}/v1", mock.url())}}}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let provider = body["result"]["result"]["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let saved = lib(
            &env,
            "connectionCreate",
            json!({"connection": {"projectId": project, "name": "PG", "type": "postgres",
                "host": "db.example.com", "port": 5432, "databaseName": "app", "username": "u",
                "aiShareSchema": false, "aiShareData": true,
                "activeAIProviderId": provider, "activeAIModel": "model-1"}}),
        )
        .await["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (status, body) = env
            .db(
                USER,
                "connect",
                json!({"target": {"type": "saved", "id": saved}, "secrets": {"db": "pw"}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let connection = body["result"]["result"]["connectionId"]
            .as_str()
            .unwrap()
            .to_string();
        Self {
            env,
            mock,
            saved,
            provider,
            connection,
        }
    }

    async fn chat(&self) -> String {
        lib(
            &self.env,
            "chatCreate",
            json!({"chat": {"connectionId": self.saved, "title": "T"}}),
        )
        .await["value"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn start_turn(&self, stream: &str, chat: &str) -> Json {
        json!({"op": "start", "streamId": stream, "request": {"method": "ai", "params": {
            "method": "chat", "params": {
                "streamId": stream, "chatId": chat, "connectionId": self.connection,
                "userMessage": {"id": format!("{stream}-u"), "content": "MARKER_ASK_QUESTION"},
                "assistantMessageId": format!("{stream}-a"), "apiKey": TEST_KEY,
                "providerId": self.provider}}}})
    }

    async fn stored(&self, chat: &str) -> Vec<Json> {
        lib(&self.env, "chatMessagesList", json!({"chatId": chat})).await["value"]["messages"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// The chat's rows once `n` are stored (a stopped turn writes its reply
    /// after the socket is gone), or panic after 10 s.
    async fn stored_eventually(&self, chat: &str, n: usize) -> Vec<Json> {
        for _ in 0..1000 {
            let rows = self.stored(chat).await;
            if rows.len() >= n {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the chat never held {n} rows");
    }

    /// Some text, then the provider holds the stream open.
    fn stall_after_text(&self) {
        self.mock.reply(Reply::Sse {
            events: vec![openai_chunk(
                json!({"choices":[{"index":0,"delta":{"content":"Partial answer"}}]}),
            )],
            piece: 64,
            gap: Duration::ZERO,
            end: SseEnd::Stall,
        });
    }

    async fn wait_for_turns(&self, n: usize) {
        let open = self
            .env
            .state
            .workspaces
            .get(&self.env.state.core, USER)
            .await
            .unwrap();
        for _ in 0..1000 {
            if open.workspace().ai_turn_count() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("never {n} turns running");
    }
}

async fn lib(env: &Env, method: &str, params: Json) -> Json {
    let (status, body) = env.library(USER, Some("win-1"), method, params).await;
    assert_eq!(status, StatusCode::OK, "{method}: {body}");
    body["result"]["result"].clone()
}

/// Frames until `stream` gets an event of `kind`.
async fn until_kind(ws: &mut Ws, stream: &str, kind: &str) -> Json {
    loop {
        let frame = next(ws).await;
        if frame["streamId"] == stream && frame["event"]["type"] == kind {
            return frame;
        }
    }
}

fn ai_body(method: &str, params: Json) -> Json {
    json!({"method": "ai", "params": {"method": method, "params": params}})
}

/// A round with one `run_query` call (it waits for an approval).
fn query_round() -> Vec<String> {
    vec![
        openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1",
            "type":"function","function":{"name":"run_query","arguments":"{\"sql\":\"SELECT 1\"}"}}]}}]}),
        ),
        openai_chunk(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),
        openai_done(),
    ]
}

#[tokio::test]
async fn the_socket_runs_a_turn_and_rpc_answers_its_approval() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let chat = ai.chat().await;
    ai.mock.reply(Reply::sse(query_round()));
    ai.mock.reply(Reply::sse(openai_text("One row.")));
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    let approval = until_kind(&mut ws, "t1", "approvalRequired").await;
    assert_eq!(approval["type"], "ai");
    assert_eq!(approval["event"]["sql"], "SELECT 1");
    let (status, body) = ai
        .env
        .rpc(
            USER,
            &ai_body(
                "respond",
                json!({"streamId": "t1", "callId": "call_1", "decision": "allow"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"method": "ai", "result": {"method": "respond", "result": null}})
    );
    let frames = until_end(&mut ws, "t1", &mut Vec::new()).await;
    let done = frames.last().unwrap();
    assert_eq!(done["event"]["type"], "done", "{done}");
    assert_eq!(done["event"]["messages"][1]["content"], "One row.");
    assert_eq!(ai.env.calls.read_only.load(Ordering::SeqCst), 1);
    // The key reached the provider in the request header and nowhere else.
    assert_eq!(
        ai.mock.requests()[0].header("authorization"),
        Some(format!("Bearer {TEST_KEY}").as_str())
    );
    assert!(!serde_json::to_string(&frames).unwrap().contains(TEST_KEY));

    // Another user's respond can't reach the turn's calls: NOT_FOUND, 404.
    let (status, body) = ai
        .env
        .rpc(
            "bob",
            &ai_body(
                "respond",
                json!({"streamId": "t1", "callId": "call_1", "decision": "allow"}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn a_bad_ai_frame_gets_an_ai_error_and_the_socket_keeps_working() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let chat = ai.chat().await;
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    // Params that don't parse: still an `ai` event.
    send(
        &mut ws,
        &json!({"op": "start", "streamId": "x1", "request": {"method": "ai", "params": {
            "method": "chat", "params": {"streamId": "x1"}}}}),
    )
    .await;
    let frame = next(&mut ws).await;
    assert_eq!(frame["type"], "ai", "{frame}");
    assert_eq!(frame["streamId"], "x1");
    assert_eq!(frame["event"]["type"], "error");
    assert_eq!(frame["event"]["code"], "INVALID_ARGUMENT");
    // A turn whose streamId isn't the frame's.
    let mut frame = ai.start_turn("x2", &chat);
    frame["streamId"] = json!("x3");
    send(&mut ws, &frame).await;
    let refused = next(&mut ws).await;
    assert_eq!(refused["type"], "ai", "{refused}");
    assert_eq!(refused["streamId"], "x3");
    assert_eq!(refused["event"]["code"], "INVALID_ARGUMENT");
    // A unary ai call is no stream.
    send(
        &mut ws,
        &json!({"op": "start", "streamId": "x4", "request": ai_body("respond",
            json!({"streamId": "x4", "callId": "c", "decision": "deny"}))}),
    )
    .await;
    let refused = next(&mut ws).await;
    assert_eq!(refused["event"]["code"], "INVALID_ARGUMENT", "{refused}");
    // The socket still works.
    ai.mock.reply(Reply::sse(openai_text("Fine.")));
    send(&mut ws, &ai.start_turn("x5", &chat)).await;
    let frames = until_end(&mut ws, "x5", &mut Vec::new()).await;
    assert_eq!(frames.last().unwrap()["event"]["type"], "done");
}

/// Task 4's contract: closing the socket mid-turn cancels the turn through
/// Core and lets it finish, so the reply is stored with what streamed.
#[tokio::test]
async fn closing_the_socket_stops_a_turn_and_its_reply_is_stored() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let chat = ai.chat().await;
    ai.stall_after_text();
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    until_kind(&mut ws, "t1", "text").await;
    ws.close(None).await.unwrap();
    let rows = ai.stored_eventually(&chat, 2).await;
    assert_eq!(rows[1]["content"], "Partial answer", "{rows:?}");
    ai.wait_for_turns(0).await;
    assert_eq!(ai.env.state.core.running_stream_count(), 0);
    // The provider saw the client go (the response was dropped).
    assert!(ai.mock.client_gone(Duration::from_secs(5)).await);
}

/// A `respond` for a turn whose socket closed while it waited for an
/// approval answers NOT_FOUND at once, and the reply is stored.
#[tokio::test]
async fn a_respond_after_the_socket_closed_is_not_found() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let chat = ai.chat().await;
    ai.mock.reply(Reply::sse(query_round()));
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    until_kind(&mut ws, "t1", "approvalRequired").await;
    drop(ws);
    ai.wait_for_turns(0).await;
    let answer = tokio::time::timeout(
        Duration::from_secs(5),
        ai.env.rpc(
            USER,
            &ai_body(
                "respond",
                json!({"streamId": "t1", "callId": "call_1", "decision": "allow"}),
            ),
        ),
    )
    .await
    .expect("respond didn't hang");
    assert_eq!(answer.0, StatusCode::NOT_FOUND, "{}", answer.1);
    assert_eq!(answer.1["code"], "NOT_FOUND");
    let rows = ai.stored_eventually(&chat, 2).await;
    assert_eq!(rows[0]["content"], "MARKER_ASK_QUESTION");
    // The approval was never given: no query ran.
    assert_eq!(ai.env.calls.read_only.load(Ordering::SeqCst), 0);
}

/// A socket closed as `EVENTS_LAGGED` stops its turn the same way: the
/// turn ends cancelled and its reply is stored.
#[tokio::test]
async fn a_lagging_socket_stops_its_turn_and_the_reply_is_stored() {
    let ai = Ai::new(Env::with_ai_and_event_bound(4, 16)).await;
    let chat = ai.chat().await;
    ai.stall_after_text();
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    until_kind(&mut ws, "t1", "text").await;
    // Back the outbox up with large batches nobody reads, then flood it
    // with events past the bound.
    for i in 0..4 {
        send(
            &mut ws,
            &start(&format!("b{i}"), &ai.connection, "SELECT big"),
        )
        .await;
    }
    for i in 0..200 {
        let (status, _) = ai
            .env
            .rpc(
                USER,
                &json!({"method": "storage", "params": {"method": "userCredentialsRemoveAllForKey",
                    "params": {"key": format!("k{i}")}}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let close = loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("no close within 10 s");
        match msg {
            Some(Ok(Message::Close(frame))) => break frame.expect("a close code"),
            Some(Ok(_)) => {}
            other => panic!("the socket ended without a close frame: {other:?}"),
        }
    };
    assert!(close.reason.starts_with(seaquel_server::EVENTS_LAGGED));
    let rows = ai.stored_eventually(&chat, 2).await;
    assert_eq!(rows[1]["content"], "Partial answer", "{rows:?}");
    ai.wait_for_turns(0).await;
}

/// A turn is one of a socket's 16 streams.
#[tokio::test]
async fn a_turn_counts_as_one_of_sixteen_streams() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let chat = ai.chat().await;
    ai.stall_after_text();
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    until_kind(&mut ws, "t1", "text").await;
    for i in 0..15 {
        send(
            &mut ws,
            &start(&format!("h{i}"), &ai.connection, "SELECT hang"),
        )
        .await;
    }
    ai.env
        .calls
        .wait("15 hanging queries", |c| {
            c.hanging.load(Ordering::SeqCst) == 15
        })
        .await;
    let other = ai.chat().await;
    send(&mut ws, &ai.start_turn("t2", &other)).await;
    let refused = until_kind(&mut ws, "t2", "error").await;
    assert_eq!(refused["type"], "ai");
    assert_eq!(refused["event"]["code"], "TOO_MANY_STREAMS");
    // Stopping the turn frees its slot.
    send(&mut ws, &json!({"op": "cancel", "streamId": "t1"})).await;
    ai.wait_for_turns(0).await;
    ai.mock.reply(Reply::sse(openai_text("Fine.")));
    for _ in 0..100 {
        send(&mut ws, &ai.start_turn("t3", &other)).await;
        let frame = loop {
            let frame = next(&mut ws).await;
            if frame["streamId"] == "t3" {
                break frame;
            }
        };
        if frame["event"]["code"] != "TOO_MANY_STREAMS" {
            assert_eq!(frame["event"]["type"], "started", "{frame}");
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let frames = until_end(&mut ws, "t3", &mut Vec::new()).await;
    assert_eq!(frames.last().unwrap()["event"]["type"], "done");
}

/// `WEB_AI_LIMITS`: four turns in flight per user; the fifth is
/// `TOO_MANY_REQUESTS` (429).
#[tokio::test]
async fn a_fifth_turn_in_flight_is_too_many_requests() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    for i in 0..4 {
        let chat = ai.chat().await;
        ai.stall_after_text();
        let stream = format!("t{i}");
        send(&mut ws, &ai.start_turn(&stream, &chat)).await;
        until_kind(&mut ws, &stream, "text").await;
    }
    let chat = ai.chat().await;
    send(&mut ws, &ai.start_turn("t4", &chat)).await;
    let refused = until_kind(&mut ws, "t4", "error").await;
    assert_eq!(refused["event"]["code"], "TOO_MANY_REQUESTS");
    assert_eq!(
        seaquel_server::status_for("TOO_MANY_REQUESTS"),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(ai.stored(&chat).await.is_empty(), "nothing stored");
    ws.close(None).await.unwrap();
    ai.wait_for_turns(0).await;
}

/// `SEAQUEL_AI_EGRESS=off`: every model call is `AI_EGRESS_BLOCKED`, 503
/// on `/rpc`, an `ai` error on the socket.
#[tokio::test]
async fn egress_off_blocks_every_model_call() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Off)).await;
    let (status, body) = ai
        .env
        .rpc(
            USER,
            &ai_body(
                "models",
                json!({"providerId": ai.provider, "apiKey": TEST_KEY}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["code"], "AI_EGRESS_BLOCKED");
    let (status, body) = ai
        .env
        .rpc(
            USER,
            &ai_body(
                "generate",
                json!({"connectionId": ai.saved, "request": "x", "apiKey": TEST_KEY,
                    "providerId": ai.provider}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let chat = ai.chat().await;
    let addr = ai.env.serve().await;
    let mut ws = open_stream(addr, USER).await;
    send(&mut ws, &ai.start_turn("t1", &chat)).await;
    let frame = next(&mut ws).await;
    assert_eq!(frame["type"], "ai");
    assert_eq!(frame["event"]["code"], "AI_EGRESS_BLOCKED");
    assert!(ai.mock.requests().is_empty());
}

/// The unary calls answer on `/rpc`; `ai.chat` there is refused.
#[tokio::test]
async fn rpc_serves_the_unary_ai_calls_and_refuses_chat() {
    let ai = Ai::new(Env::with_ai(8, AiEgress::Any)).await;
    ai.mock
        .reply(Reply::json(200, &json!({"data": [{"id": "m-1"}]})));
    let (status, body) = ai
        .env
        .rpc(
            USER,
            &ai_body(
                "models",
                json!({"providerId": ai.provider, "apiKey": TEST_KEY}),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"], json!(["m-1"]));
    let chat = ai.chat().await;
    let turn = ai.start_turn("t1", &chat);
    let (status, body) = ai.env.rpc(USER, &turn["request"]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_ARGUMENT");
    // The provider's refusal keeps its code.
    ai.mock.reply(Reply::json(
        429,
        &json!({"error": {"type": "rate_limit", "message": "slow down"}}),
    ));
    let (status, body) = ai
        .env
        .rpc(USER, &ai_body("test", json!({"providerId": ai.provider})))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "RATE_LIMITED");
}

/// Review I1: `ai.generate`, `ai.models` and `ai.test` each hold a model
/// call for up to 10 minutes, so they count toward the per-user cap on
/// slow calls (`MAX_EDIT_CALLS_PER_USER`, shared with the edit calls): with
/// four stalled at the provider, a fifth is 429 before it reaches it.
#[tokio::test]
async fn a_fifth_slow_ai_call_is_too_many_requests() {
    let ai = std::sync::Arc::new(Ai::new(Env::with_ai(8, AiEgress::Any)).await);
    for _ in 0..4 {
        ai.mock.reply(Reply::Hang);
    }
    let bodies = [
        ai_body("models", json!({"providerId": ai.provider})),
        ai_body("test", json!({"providerId": ai.provider})),
        ai_body(
            "generate",
            json!({"connectionId": ai.saved, "request": "x"}),
        ),
        ai_body("models", json!({"providerId": ai.provider})),
    ];
    let mut stalled = Vec::new();
    for body in bodies {
        let ai = ai.clone();
        stalled.push(tokio::spawn(async move { ai.env.rpc(USER, &body).await }));
    }
    for _ in 0..1000 {
        if ai.mock.requests().len() == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ai.mock.requests().len(), 4, "four calls at the provider");
    for body in [
        ai_body("test", json!({"providerId": ai.provider})),
        ai_body(
            "generate",
            json!({"connectionId": ai.saved, "request": "x"}),
        ),
    ] {
        let (status, refused) = ai.env.rpc(USER, &body).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{refused}");
        assert_eq!(refused["code"], "TOO_MANY_REQUESTS");
    }
    assert_eq!(
        ai.mock.requests().len(),
        4,
        "the refused calls sent nothing"
    );
    // Another user isn't held up.
    let (status, body) = ai
        .env
        .rpc(
            "bob",
            &ai_body("models", json!({"providerId": ai.provider})),
        )
        .await;
    assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    for task in stalled {
        task.abort();
    }
}
