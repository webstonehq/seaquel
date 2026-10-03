//! The `ai` group (phase 6 Task 5): `ai.chat` is a stream served by
//! `dispatch_stream` only, `respond`, `generate`, `models` and `test` are
//! unary, every params type refuses unknown fields, and `db.connect`
//! records a `savedConnectionId`. Every model call goes to a local mock
//! provider through a loopback-only client; the key is the fake test key.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_ai::testing::scripts::{openai_chunk, openai_done, openai_text};
use seaquel_ai::testing::{LoopbackOnly, MockProvider, Reply, SseEnd, TEST_KEY};
use seaquel_core::ai::native::{Egress, NativeHttp, NativeHttpOptions};
use seaquel_core::ai::AiEgress;
use seaquel_core::{with_plugins, ConnectPolicy, Core, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_stream, dispatch_workspace, parse_request, CoreEvent, Request, RpcError, StreamKind,
    WriteOrigin,
};
use serde_json::{json, Value as Json};

// ── Helpers ──

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    mock: MockProvider,
    dir: tempfile::TempDir,
}

const WINDOW: &str = "win-1";

/// A SQLite-only Core that may call the mock, and a workspace holding a
/// project, a keyless OpenAI-compatible provider at the mock, a saved
/// SQLite connection that shares its data and a chat on it.
async fn env() -> (Env, Ids) {
    let mock = MockProvider::start().await;
    let core = with_plugins(|id| id == "sqlite")
        .connect_policy(ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .ai_http(Arc::new(LoopbackOnly(NativeHttp::new(
            NativeHttpOptions::new(Egress::Any),
        ))))
        .ai_egress(AiEgress::Any)
        .build();
    let dir = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path().join("data")))
        .await
        .unwrap();
    let env = Env {
        core,
        ws,
        mock,
        dir,
    };
    let ids = env.seed().await;
    (env, ids)
}

struct Ids {
    provider: String,
    saved: String,
    chat: String,
}

fn parse(body: &Json) -> Request {
    parse_request(body.to_string().as_bytes()).unwrap()
}

fn ai(method: &str, params: Json) -> Json {
    json!({"method": "ai", "params": {"method": method, "params": params}})
}

impl Env {
    async fn call(&self, body: &Json) -> Result<Json, RpcError> {
        let res = dispatch_workspace(
            &self.core,
            &self.ws,
            parse_request(body.to_string().as_bytes())?,
            WriteOrigin::new(Some(WINDOW)),
        )
        .await?;
        Ok(serde_json::to_value(res).unwrap())
    }

    /// One call of `group`; its `result`.
    async fn group(&self, group: &str, method: &str, params: Json) -> Result<Json, RpcError> {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let res = self
            .call(&json!({"method": group, "params": inner}))
            .await?;
        assert_eq!(res["method"], group, "{res}");
        assert_eq!(res["result"]["method"], method, "{res}");
        Ok(res["result"]["result"].clone())
    }

    async fn seed(&self) -> Ids {
        self.group("library", "projectEnsureDefault", Json::Null)
            .await
            .unwrap();
        let projects = self
            .group("library", "projectsList", Json::Null)
            .await
            .unwrap();
        let project = projects["value"][0]["id"].as_str().unwrap().to_string();
        let provider = self
            .group(
                "settings",
                "aiProviderCreate",
                json!({"provider": {"name": "Mock", "type": "openai-compatible",
                    "baseUrl": format!("{}/v1", self.mock.url())}}),
            )
            .await
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let file = self.dir.path().join("app.db");
        let saved = self
            .group(
                "library",
                "connectionCreate",
                json!({"connection": {"projectId": project, "name": "Lite", "type": "sqlite",
                    "host": "", "port": 0, "username": "",
                    "databaseName": file.display().to_string(), "aiShareData": true,
                    "activeAIProviderId": provider, "activeAIModel": "model-1"}}),
            )
            .await
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let chat = self
            .group(
                "library",
                "chatCreate",
                json!({"chat": {"connectionId": saved, "title": "T"}}),
            )
            .await
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        Ids {
            provider,
            saved,
            chat,
        }
    }

    /// Opens the saved connection; Core's id for it.
    async fn connect(&self, saved: &str) -> String {
        self.group(
            "db",
            "connect",
            json!({"target": {"type": "saved", "id": saved}, "createIfMissing": true}),
        )
        .await
        .unwrap()["connectionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The chat's stored rows (`chatMessagesList`).
    async fn stored(&self, chat: &str) -> Vec<Json> {
        self.group("library", "chatMessagesList", json!({"chatId": chat}))
            .await
            .unwrap()["value"]["messages"]
            .as_array()
            .unwrap()
            .clone()
    }
}

fn chat_params(stream: &str, ids: &Ids, connection: &str) -> Json {
    json!({"streamId": stream, "chatId": ids.chat, "connectionId": connection,
        "userMessage": {"id": format!("{stream}-u"), "content": "How many rows?"},
        "assistantMessageId": format!("{stream}-a"), "apiKey": TEST_KEY,
        "providerId": ids.provider})
}

/// A round with one `run_query` call of `SELECT 1`.
fn query_round() -> Vec<String> {
    vec![
        openai_chunk(
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1",
            "type":"function","function":{"name":"run_query","arguments":"{\"sql\":\"SELECT 1 AS n\"}"}}]}}]}),
        ),
        openai_chunk(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]})),
        openai_done(),
    ]
}

async fn next_event(events: &mut (impl futures::Stream<Item = CoreEvent> + Unpin)) -> Option<Json> {
    tokio::time::timeout(Duration::from_secs(20), events.next())
        .await
        .expect("an event within 20 s")
        .map(|e| serde_json::to_value(e).unwrap())
}

// ── The wire ──

#[tokio::test]
async fn ai_chat_is_a_stream_and_dispatch_workspace_refuses_it() {
    let (env, ids) = env().await;
    let c = env.connect(&ids.saved).await;
    let err = env
        .call(&ai("chat", chat_params("s1", &ids, &c)))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("ai.chat"), "{}", err.message);
    // Nothing ran: no request reached the mock, nothing was stored.
    assert!(env.mock.requests().is_empty());
    assert!(env.stored(&ids.chat).await.is_empty());
}

#[tokio::test]
async fn dispatch_stream_serves_a_turn_as_ai_events() {
    let (env, ids) = env().await;
    let c = env.connect(&ids.saved).await;
    env.mock.reply(Reply::sse(openai_text("Hello there.")));
    let req = parse(&ai("chat", chat_params("s1", &ids, &c)));
    assert_eq!(req.stream_kind(), Some(StreamKind::Ai));
    assert_eq!(req.stream_id(), Some("s1"));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let mut got = Vec::new();
    while let Some(event) = next_event(&mut events).await {
        got.push(event);
    }
    assert!(
        got.iter()
            .all(|e| e["type"] == "ai" && e["streamId"] == "s1"),
        "{got:?}"
    );
    let kinds: Vec<&str> = got
        .iter()
        .map(|e| e["event"]["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"started"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"done"), "{kinds:?}");
    let done = &got.last().unwrap()["event"];
    assert_eq!(done["stop"], "end");
    assert_eq!(done["messages"][1]["content"], "Hello there.");
    // The key went to the provider and nowhere else.
    let sent = env.mock.requests();
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {TEST_KEY}").as_str())
    );
    let text = serde_json::to_string(&got).unwrap();
    assert!(!text.contains(TEST_KEY));
    assert_eq!(env.stored(&ids.chat).await.len(), 2);
}

#[tokio::test]
async fn respond_answers_a_waiting_turn() {
    let (env, ids) = env().await;
    let c = env.connect(&ids.saved).await;
    env.mock.reply(Reply::sse(query_round()));
    env.mock.reply(Reply::sse(openai_text("One row.")));
    let req = parse(&ai("chat", chat_params("s1", &ids, &c)));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let mut kinds = Vec::new();
    while let Some(event) = next_event(&mut events).await {
        let kind = event["event"]["type"].as_str().unwrap().to_string();
        if kind == "approvalRequired" {
            assert_eq!(event["event"]["sql"], "SELECT 1 AS n");
            let call = event["event"]["callId"].as_str().unwrap();
            // Another stream or call is NOT_FOUND, and doesn't answer it.
            let err = env
                .group(
                    "ai",
                    "respond",
                    json!({"streamId": "other", "callId": call, "decision": "allow"}),
                )
                .await
                .unwrap_err();
            assert_eq!(err.code, "NOT_FOUND");
            let answered = env
                .group(
                    "ai",
                    "respond",
                    json!({"streamId": "s1", "callId": call, "decision": "allow"}),
                )
                .await
                .unwrap();
            assert_eq!(answered, Json::Null);
        }
        kinds.push(kind);
    }
    assert!(kinds.contains(&"toolDone".to_string()), "{kinds:?}");
    assert_eq!(kinds.last().unwrap(), "done", "{kinds:?}");
    // After the turn, its calls are gone.
    let err = env
        .group(
            "ai",
            "respond",
            json!({"streamId": "s1", "callId": "call_1", "decision": "deny"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_FOUND");
}

#[tokio::test]
async fn generate_models_and_test_round_trip() {
    let (env, ids) = env().await;
    env.connect(&ids.saved).await;
    env.mock.reply(Reply::json(
        200,
        &json!({"choices": [{"message": {"role": "assistant",
            "content": "Here:\n```sql\nSELECT 2\n```"}}]}),
    ));
    let sql = env
        .group(
            "ai",
            "generate",
            json!({"connectionId": ids.saved, "request": "two", "apiKey": TEST_KEY,
                "providerId": ids.provider}),
        )
        .await
        .unwrap();
    assert_eq!(sql, json!({"sql": "SELECT 2"}));

    env.mock.reply(Reply::json(
        200,
        &json!({"data": [{"id": "m-a"}, {"id": "m-b"}]}),
    ));
    let models = env
        .group(
            "ai",
            "models",
            json!({"providerId": ids.provider, "apiKey": TEST_KEY}),
        )
        .await
        .unwrap();
    assert_eq!(models, json!(["m-a", "m-b"]));

    env.mock.reply(Reply::json(200, &json!({"data": []})));
    let tested = env
        .group("ai", "test", json!({"providerId": ids.provider}))
        .await
        .unwrap();
    assert_eq!(tested, Json::Null);

    env.mock.reply(Reply::json(
        401,
        &json!({"error": {"type": "auth", "message": "bad key"}}),
    ));
    let err = env
        .group("ai", "test", json!({"providerId": ids.provider}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "PROVIDER_ERROR");
    let unknown = env
        .group("ai", "models", json!({"providerId": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, "AI_PROVIDER_NOT_FOUND");
}

#[tokio::test]
async fn every_ai_params_type_refuses_unknown_fields() {
    let (_env, ids) = env().await;
    let mut chat = chat_params("s1", &ids, "c");
    chat["extra"] = json!(1);
    let cases = [
        ai("chat", chat),
        ai(
            "respond",
            json!({"streamId": "s", "callId": "c", "decision": "allow", "extra": 1}),
        ),
        ai(
            "generate",
            json!({"connectionId": "c", "request": "r", "extra": 1}),
        ),
        ai("models", json!({"providerId": "p", "extra": 1})),
        ai("test", json!({"providerId": "p", "extra": 1})),
        ai("nope", json!({})),
    ];
    for body in cases {
        let err = parse_request(body.to_string().as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{body}");
    }
    // A client tool's answer is the other shape `respond` takes.
    let req = parse(&ai(
        "respond",
        json!({"streamId": "s", "callId": "c", "decision": {"result": "{}", "isError": false}}),
    ));
    assert_eq!(req.group(), "ai");
    assert_eq!(req.method(), "respond");
    assert_eq!(req.stream_kind(), None);
}

#[tokio::test]
async fn ai_requests_show_no_key_or_message_in_debug() {
    let (_env, ids) = env().await;
    let mut chat = chat_params("s1", &ids, "c");
    chat["userMessage"]["content"] = json!("MARKER_PROMPT_TEXT");
    for body in [
        ai("chat", chat),
        ai(
            "generate",
            json!({"connectionId": "c", "request": "MARKER_PROMPT_TEXT", "apiKey": TEST_KEY}),
        ),
        ai("models", json!({"providerId": "p", "apiKey": TEST_KEY})),
        ai("test", json!({"providerId": "p", "apiKey": TEST_KEY})),
        ai(
            "respond",
            json!({"streamId": "s", "callId": "c", "decision": {"result": "MARKER_PROMPT_TEXT"}}),
        ),
    ] {
        let req = parse(&body);
        let shown = format!("{req:?} {req:#?}");
        assert!(!shown.contains(TEST_KEY), "{shown}");
        assert!(!shown.contains("MARKER"), "{shown}");
        // Serializing a request back never writes the key either.
        let back = serde_json::to_string(&req).unwrap();
        assert!(!back.contains(TEST_KEY), "{back}");
    }
}

#[tokio::test]
async fn stream_kinds_name_every_stream() {
    let cases = [
        (
            json!({"method": "db", "params": {"method": "queryStream", "params": {
                "connectionId": "c", "streamId": "q", "sql": "SELECT 1"}}}),
            Some(StreamKind::Stream),
            Some("q"),
        ),
        (
            json!({"method": "db", "params": {"method": "run", "params": {
                "connectionId": "c", "streamId": "r", "text": "SELECT 1",
                "target": {"type": "all"}, "pageSize": 10}}}),
            Some(StreamKind::Run),
            Some("r"),
        ),
        (
            json!({"method": "db", "params": {"method": "cancel", "params": {"streamId": "x"}}}),
            None,
            None,
        ),
        (
            ai(
                "respond",
                json!({"streamId": "s", "callId": "c", "decision": "deny"}),
            ),
            None,
            None,
        ),
    ];
    for (body, kind, id) in cases {
        let req = parse(&body);
        assert_eq!(req.stream_kind(), kind, "{body}");
        assert_eq!(req.stream_id(), id, "{body}");
    }
    // A transport that has only the names (a frame it couldn't parse).
    assert_eq!(StreamKind::of("ai", "chat"), Some(StreamKind::Ai));
    assert_eq!(StreamKind::of("db", "tablePage"), Some(StreamKind::Run));
    assert_eq!(
        StreamKind::of("db", "queryStream"),
        Some(StreamKind::Stream)
    );
    assert_eq!(StreamKind::of("ai", "respond"), None);
    // An `ai` refusal is an `ai` error event.
    let refused = CoreEvent::error("s", StreamKind::Ai, "TOO_MANY_STREAMS", "full");
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        json!({"type": "ai", "streamId": "s", "event":
            {"type": "error", "code": "TOO_MANY_STREAMS", "message": "full"}})
    );
    assert!(refused.is_terminal());
    assert_eq!(refused.stream_id(), Some("s"));
}

#[tokio::test]
async fn a_cancelled_turn_still_stores_the_reply_when_polled_to_its_end() {
    let (env, ids) = env().await;
    let c = env.connect(&ids.saved).await;
    // Some text, then the provider stalls.
    env.mock.reply(Reply::Sse {
        events: vec![openai_chunk(
            json!({"choices":[{"index":0,"delta":{"content":"Partial answer"}}]}),
        )],
        piece: 64,
        gap: Duration::ZERO,
        end: SseEnd::Stall,
    });
    let req = parse(&ai("chat", chat_params("s1", &ids, &c)));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    loop {
        let event = next_event(&mut events).await.unwrap();
        if event["event"]["type"] == "text" {
            break;
        }
    }
    env.group("db", "cancel", json!({"streamId": "s1"}))
        .await
        .unwrap();
    // Nothing follows a cancel; the stream ends.
    assert!(next_event(&mut events).await.is_none());
    let stored = env.stored(&ids.chat).await;
    assert_eq!(stored.len(), 2, "{stored:?}");
    assert_eq!(stored[1]["content"], "Partial answer");
}

#[tokio::test]
async fn connect_records_a_saved_connection_id() {
    let (env, ids) = env().await;
    let file = env.dir.path().join("form.db");
    // A form connect naming the chat's saved connection may run its turns.
    let form = env
        .group(
            "db",
            "connect",
            json!({"target": {"type": "form", "form": {"name": "Lite", "type": "sqlite",
                "databaseName": file.display().to_string()}},
                "createIfMissing": true, "savedConnectionId": ids.saved}),
        )
        .await
        .unwrap()["connectionId"]
        .as_str()
        .unwrap()
        .to_string();
    env.mock.reply(Reply::sse(openai_text("Fine.")));
    let req = parse(&ai("chat", chat_params("s1", &ids, &form)));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let mut last = Json::Null;
    while let Some(event) = next_event(&mut events).await {
        last = event;
    }
    assert_eq!(last["event"]["type"], "done", "{last}");

    // A form connect without one can't.
    let other = env
        .group(
            "db",
            "connect",
            json!({"target": {"type": "form", "form": {"name": "Lite", "type": "sqlite",
                "databaseName": file.display().to_string()}}}),
        )
        .await
        .unwrap()["connectionId"]
        .as_str()
        .unwrap()
        .to_string();
    let req = parse(&ai("chat", chat_params("s2", &ids, &other)));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let event = next_event(&mut events).await.unwrap();
    assert_eq!(event["event"]["code"], "CONNECTION_MISMATCH", "{event}");

    // A saved connect naming another saved connection is refused.
    let err = env
        .group(
            "db",
            "connect",
            json!({"target": {"type": "saved", "id": ids.saved}, "createIfMissing": true,
                "savedConnectionId": "conn-other"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

/// Task 7 review I1: a supplied key names the provider it is for, and Core
/// refuses it for any other (the connection's provider changed after the
/// page read the key) before a request goes out; a key with no provider
/// is refused too.
#[tokio::test]
async fn a_supplied_key_for_another_provider_is_refused_before_any_request() {
    let (env, ids) = env().await;
    let c = env.connect(&ids.saved).await;
    let mut params = chat_params("s1", &ids, &c);
    params["providerId"] = json!("prov-other");
    let req = parse(&ai("chat", params));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let event = next_event(&mut events).await.unwrap();
    assert_eq!(event["event"]["type"], "error", "{event}");
    assert_eq!(event["event"]["code"], "AI_PROVIDER_CHANGED", "{event}");
    assert!(next_event(&mut events).await.is_none());

    let err = env
        .call(&ai(
            "generate",
            json!({"connectionId": ids.saved, "request": "x", "apiKey": TEST_KEY,
                "providerId": "prov-other"}),
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code, "AI_PROVIDER_CHANGED");
    let err = env
        .call(&ai(
            "generate",
            json!({"connectionId": ids.saved, "request": "x", "apiKey": TEST_KEY}),
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");

    assert!(env.mock.requests().is_empty());
    assert!(env.stored(&ids.chat).await.is_empty());
}

/// Phase 6 Task 7: a connection the page opened before its row existed
/// (`add` connects the form, then `connectionCreate` answers the id) is
/// recorded for the saved connection afterwards with `db.bindSaved`, once.
#[tokio::test]
async fn bind_saved_records_a_connection_opened_before_its_row() {
    let (env, ids) = env().await;
    let file = env.dir.path().join("form.db");
    let form = env
        .group(
            "db",
            "connect",
            json!({"target": {"type": "form", "form": {"name": "Lite", "type": "sqlite",
                "databaseName": file.display().to_string()}}, "createIfMissing": true}),
        )
        .await
        .unwrap()["connectionId"]
        .as_str()
        .unwrap()
        .to_string();
    let bind = |connection: &str, saved: &str| json!({"connectionId": connection, "savedConnectionId": saved});
    assert_eq!(
        env.group("db", "bindSaved", bind(&form, &ids.saved))
            .await
            .unwrap(),
        Json::Null
    );
    // The same id again is fine; another is refused.
    env.group("db", "bindSaved", bind(&form, &ids.saved))
        .await
        .unwrap();
    let err = env
        .group("db", "bindSaved", bind(&form, "conn-other"))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    let err = env
        .group("db", "bindSaved", bind("sqlite-nope", &ids.saved))
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");

    // The bound connection runs the chat's turns.
    env.mock.reply(Reply::sse(openai_text("Bound.")));
    let req = parse(&ai("chat", chat_params("s1", &ids, &form)));
    let mut events =
        dispatch_stream(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW))).unwrap();
    let mut last = Json::Null;
    while let Some(event) = next_event(&mut events).await {
        last = event;
    }
    assert_eq!(last["event"]["type"], "done", "{last}");
    drop(events);

    // Rebinding a saved target's connection is refused. Connecting the
    // saved row from the same window replaced the bound connection (phase
    // 6 probe F4): it is gone.
    let saved = env.connect(&ids.saved).await;
    let err = env
        .group("db", "bindSaved", bind(&saved, "conn-other"))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    let err = env
        .group("db", "bindSaved", bind(&form, &ids.saved))
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
}

/// Review M1: a stopped turn's last write may wait out a full storage
/// write turn (`WRITE_WAIT`) before it starts, so the transports give it
/// that and 15 s more.
#[test]
fn a_stopped_turn_outwaits_a_storage_write() {
    assert_eq!(
        seaquel_rpc::TURN_STOP_WAIT,
        seaquel_core::storage::WRITE_WAIT + Duration::from_secs(15)
    );
}

/// Review M3: the `secret` group names only the keys it takes; an AI key
/// is refused as Core's to manage.
#[tokio::test]
async fn the_secret_group_lists_no_ai_key() {
    let (env, _) = env().await;
    let secret = |key: &str| ai_free_secret(key);
    let err = env.call(&secret("postgres:c1")).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(!err.message.contains("ai-api-key"), "{}", err.message);
    assert!(err.message.contains("license-key"), "{}", err.message);
    let err = env.call(&secret("ai-api-key:p1")).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("managed by Core"), "{}", err.message);
}

fn ai_free_secret(key: &str) -> Json {
    json!({"method": "secret", "params": {"method": "get", "params": {"key": key}}})
}
