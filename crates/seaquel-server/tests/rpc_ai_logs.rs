//! What the server's log holds after assistant calls (phase 6, Decision
//! 16): the request's group and method, codes and counts, and never the
//! key, the user's message, the reply, SQL or the provider's own message.
//! Records go through `startup::format_record`, as on stderr.
//!
//! Its own test binary, since the logger is global.

use std::sync::{Mutex, Once, PoisonError};
use std::time::Duration;

use axum::http::StatusCode;
use seaquel_ai::testing::scripts::{openai_chunk, openai_done, openai_text};
use seaquel_ai::testing::{MockProvider, Reply, TEST_KEY};
use seaquel_core::ai::AiEgress;
use serde_json::{json, Value as Json};

mod common;
use common::{open_stream, send, until_end, Env};

static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    /// What stderr writes, plus every line of Seaquel's own crates at any
    /// level: a debug line must not carry a key either. (Dependencies'
    /// lines below WARN never reach stderr; tungstenite's TRACE quotes
    /// whole frames, the test client's included.)
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        seaquel_server::startup::logs(metadata) || metadata.target().starts_with("seaquel")
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(seaquel_server::startup::format_record(record));
    }

    fn flush(&self) {}
}

fn capture_logs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn records() -> Vec<String> {
    RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

async fn call(env: &Env, group: &str, method: &str, params: Json) -> (StatusCode, Json) {
    env.rpc(
        "alice",
        &json!({"method": group, "params": {"method": method, "params": params}}),
    )
    .await
}

#[tokio::test]
async fn no_key_message_reply_or_sql_reaches_the_log() {
    capture_logs();
    let mock = MockProvider::start().await;
    let env = Env::with_ai(4, AiEgress::Any);
    let marker_ask = "MARKERASK7f3e question";
    let marker_reply = "MARKERREPLY91c answer";
    let marker_sql = "SELECT 'MARKERSQL55d'";
    let marker_provider = "MARKERPROVIDER0b2 refused";

    env.library("alice", None, "projectEnsureDefault", Json::Null)
        .await;
    let (_, projects) = env.library("alice", None, "projectsList", Json::Null).await;
    let project = projects["result"]["result"]["value"][0]["id"].clone();
    let (_, provider) = call(
        &env,
        "settings",
        "aiProviderCreate",
        json!({"provider": {"name": "Mock", "type": "openai-compatible",
            "baseUrl": format!("{}/v1", mock.url())}}),
    )
    .await;
    let provider = provider["result"]["result"]["value"]["id"].clone();
    let (_, saved) = env
        .library(
            "alice",
            None,
            "connectionCreate",
            json!({"connection": {"projectId": project, "name": "PG", "type": "postgres",
                "host": "db.example.com", "port": 5432, "databaseName": "app", "username": "u",
                "aiShareSchema": false, "aiShareData": true,
                "activeAIProviderId": provider, "activeAIModel": "model-1"}}),
        )
        .await;
    let saved = saved["result"]["result"]["value"]["id"].clone();
    let (_, chat) = env
        .library(
            "alice",
            None,
            "chatCreate",
            json!({"chat": {"connectionId": saved, "title": "T"}}),
        )
        .await;
    let chat = chat["result"]["result"]["value"]["id"].clone();
    let (status, connected) = env
        .db(
            "alice",
            "connect",
            json!({"target": {"type": "saved", "id": saved}, "secrets": {"db": "pw"}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{connected}");
    let connection = connected["result"]["result"]["connectionId"].clone();

    // A turn with a query (allowed for the turn) and a reply.
    mock.reply(Reply::sse(vec![
        openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,
            "id":"call_1","type":"function","function":{"name":"run_query",
            "arguments": json!({"sql": marker_sql}).to_string()}}]}}]}),
        ),
        openai_chunk(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),
        openai_done(),
    ]));
    mock.reply(Reply::sse(openai_text(marker_reply)));
    let addr = env.serve().await;
    let mut ws = open_stream(addr, "alice").await;
    send(
        &mut ws,
        &json!({"op": "start", "streamId": "t1", "request": {"method": "ai", "params": {
            "method": "chat", "params": {"streamId": "t1", "chatId": chat,
                "connectionId": connection, "approval": "allowAll",
                "userMessage": {"id": "u1", "content": marker_ask},
                "assistantMessageId": "a1", "apiKey": TEST_KEY,
                "providerId": provider}}}}),
    )
    .await;
    let frames = until_end(&mut ws, "t1", &mut Vec::new()).await;
    assert_eq!(
        frames.last().unwrap()["event"]["type"],
        "done",
        "{frames:?}"
    );

    // A provider refusal with its own message, and the unary calls.
    mock.reply(Reply::json(
        401,
        &json!({"error": {"type": "auth", "message": marker_provider}}),
    ));
    let (status, body) = call(
        &env,
        "ai",
        "models",
        json!({"providerId": provider, "apiKey": TEST_KEY}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    mock.reply(Reply::json(
        200,
        &json!({"choices": [{"message": {"role": "assistant",
            "content": format!("```sql\n{marker_sql}\n```")}}]}),
    ));
    let (status, body) = call(
        &env,
        "ai",
        "generate",
        json!({"connectionId": saved, "request": marker_ask, "apiKey": TEST_KEY,
            "providerId": provider}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let lines = records();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("group=ai") && l.contains("method=models")),
        "the call itself is logged: {lines:#?}"
    );
    for forbidden in [
        TEST_KEY,
        "MARKERASK7f3e",
        "MARKERREPLY91c",
        "MARKERSQL55d",
        "MARKERPROVIDER0b2",
        "/v1/chat/completions",
    ] {
        let hit: Vec<&String> = lines.iter().filter(|l| l.contains(forbidden)).collect();
        assert!(hit.is_empty(), "{forbidden} reached the log: {hit:#?}");
    }
}
