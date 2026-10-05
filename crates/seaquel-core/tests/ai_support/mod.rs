//! What the assistant's tests share (phase 6 Task 4): a database engine
//! that answers from a script, a client that records the requests Core
//! builds and sends them to the mock provider (never anywhere else), and a
//! workspace with a project, saved connections, AI settings and a chat.
#![allow(dead_code)]

#[path = "../common/duckdb.rs"]
mod duckdb_helper;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::StreamExt;
use seaquel_ai::http::{HttpClient, HttpError, HttpErrorKind, HttpRequest, HttpResponse, Method};
use seaquel_ai::testing::{LoopbackOnly, MockProvider, Reply, SseEnd, TEST_KEY};
use seaquel_ai::wire::{Decoder, ProviderKind, RoundEvent, StopReason};
use seaquel_core::ai::native::{Egress, NativeHttp, NativeHttpOptions};
use seaquel_core::ai::{AiDecision, AiEgress, AiEvent, AiLimits, ChatParams, ChatUserMessage};
use seaquel_core::{ConnectRequest, Core, StateLimits, Workspace, WorkspaceSpec};
use seaquel_engine::{
    CappedResult, ConnectConfig, DbError, Driver, Engine, ExecuteResult, ExplainResult,
    QueryResult, ReadOnlyOptions, SchemaColumn, SchemaIndex, SchemaTable, Value,
};
use serde_json::{json, Value as Json};

use crate::common::{insert_rows, TestStore};

pub const T0: &str = "2026-01-01T00:00:00.000Z";

// ── The database ──

/// One query the scripted database ran, as the fixtures write it.
#[derive(Clone, Debug, PartialEq)]
pub struct Ran {
    /// The saved connection whose open connection ran it.
    pub saved: String,
    pub sql: String,
    pub max_rows: Option<usize>,
    pub max_bytes: Option<usize>,
    pub timeout_ms: Option<u64>,
}

type Hook = Arc<dyn Fn(&Ran) -> BoxFuture<'static, ()> + Send + Sync>;

/// What every connection of the scripted engines answers.
#[derive(Default)]
pub struct Db {
    pub schema: Mutex<Vec<SchemaTable>>,
    /// Query answers by SQL: `{columns, rows, truncated?}` or `{error:
    /// "CODE: message"}`; anything else answers `{"columns":["n"],"rows":[[1]]}`.
    pub answers: Mutex<BTreeMap<String, Json>>,
    pub explain: Mutex<Option<ExplainResult>>,
    pub ran: Mutex<Vec<Ran>>,
    /// Runs after each query is recorded (the "data sharing turned off
    /// after the first query" case).
    pub after_query: Mutex<Option<Hook>>,
    /// Queries hang until the call is dropped.
    pub hang: Mutex<bool>,
}

impl Db {
    pub fn ran(&self) -> Vec<Ran> {
        self.ran.lock().unwrap().clone()
    }
}

struct ScriptedDriver {
    db: Arc<Db>,
    saved: String,
}

fn not_supported(what: &str) -> DbError {
    DbError {
        code: "NOT_SUPPORTED".into(),
        message: format!("{what} isn't scripted"),
    }
}

#[seaquel_runtime::async_trait]
impl Driver for ScriptedDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        Err(not_supported("query"))
    }

    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        Err(not_supported("execute"))
    }

    async fn query_read_only_with(
        &self,
        sql: &str,
        _params: Vec<Value>,
        options: ReadOnlyOptions,
    ) -> Result<CappedResult, DbError> {
        let ran = Ran {
            saved: self.saved.clone(),
            sql: sql.to_string(),
            max_rows: options.max_rows,
            max_bytes: options.max_bytes,
            timeout_ms: options.timeout.map(|t| t.as_millis() as u64),
        };
        self.db.ran.lock().unwrap().push(ran.clone());
        let hook = self.db.after_query.lock().unwrap().clone();
        if let Some(hook) = hook {
            hook(&ran).await;
        }
        if *self.db.hang.lock().unwrap() {
            futures::future::pending::<()>().await;
        }
        let answer = self
            .db
            .answers
            .lock()
            .unwrap()
            .get(sql)
            .cloned()
            .unwrap_or_else(|| json!({"columns": ["n"], "rows": [[1]]}));
        if let Some(e) = answer["error"].as_str() {
            let (code, message) = e.split_once(": ").unwrap();
            return Err(DbError {
                code: code.into(),
                message: message.into(),
            });
        }
        let columns: Vec<String> = serde_json::from_value(answer["columns"].clone()).unwrap();
        let mut rows: Vec<Vec<Value>> = answer["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|c| Value::from_wire(c.clone()).unwrap())
                    .collect()
            })
            .collect();
        let mut truncated = answer["truncated"] == json!(true);
        if let Some(max) = options.max_rows {
            if rows.len() > max {
                rows.truncate(max);
                truncated = true;
            }
        }
        Ok(CappedResult {
            columns,
            rows,
            truncated,
        })
    }

    async fn explain_read_only(
        &self,
        _sql: &str,
        _params: Vec<Value>,
        _timeout: Option<Duration>,
    ) -> Result<ExplainResult, DbError> {
        self.db
            .explain
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| not_supported("explain"))
    }

    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        let mut out: Vec<String> = Vec::new();
        for t in self.db.schema.lock().unwrap().iter() {
            if !out.contains(&t.schema) {
                out.push(t.schema.clone());
            }
        }
        Ok(out)
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        Ok(self.db.schema.lock().unwrap().clone())
    }

    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.db
            .schema
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.schema == schema && t.name == table)
            .map(|t| (t.columns.clone(), t.indexes.clone()))
            .ok_or_else(|| not_supported("that table"))
    }
}

/// An engine whose connections answer from `db`. The saved connection a
/// connection was opened for is found by its id in the config (the seeded
/// rows put it in the database name).
pub struct ScriptedEngine {
    pub id: &'static str,
    pub db: Arc<Db>,
    pub saved_ids: Vec<String>,
}

#[seaquel_runtime::async_trait]
impl Engine for ScriptedEngine {
    fn id(&self) -> &'static str {
        self.id
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let fields = [
            config.connection_string.clone(),
            config.database.clone(),
            config.path.clone(),
        ];
        let saved = self
            .saved_ids
            .iter()
            .find(|id| fields.iter().flatten().any(|f| f.contains(id.as_str())))
            .cloned()
            .unwrap_or_default();
        Ok(Arc::new(ScriptedDriver {
            db: self.db.clone(),
            saved,
        }))
    }
}

// ── The provider ──

/// What a scripted response does: go to the mock, or fail to connect.
#[derive(Clone, Debug)]
pub enum Action {
    Forward,
    NetworkError,
}

/// One request as Core built it, before it was sent: the URL Core chose
/// (a real provider's, which this client never reaches), the headers with
/// the test key as `<key>`, and the body.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub method: &'static str,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Json,
}

/// Records each request, then sends it to the mock: the URL's origin (and
/// an OpenAI-compatible base's path up to `/v1`) is replaced by the mock's,
/// so `https://api.anthropic.com/v1/messages` goes to `<mock>/v1/messages`.
/// What reaches the network goes through [`LoopbackOnly`].
pub struct Redirect {
    pub mock: MockProvider,
    pub inner: LoopbackOnly<NativeHttp>,
    pub sent: Mutex<Vec<Sent>>,
    pub actions: Mutex<VecDeque<Action>>,
}

impl Redirect {
    pub fn new(mock: MockProvider) -> Arc<Self> {
        Arc::new(Self {
            mock,
            inner: LoopbackOnly(NativeHttp::new(NativeHttpOptions::new(Egress::Any))),
            sent: Mutex::default(),
            actions: Mutex::default(),
        })
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    /// Queue a scripted response: the mock's reply, or a network error.
    pub fn script(&self, action: Action, reply: Option<Reply>) {
        self.actions.lock().unwrap().push_back(action);
        if let Some(reply) = reply {
            self.mock.reply(reply);
        }
    }

    pub fn unused(&self) -> usize {
        self.actions.lock().unwrap().len()
    }
}

/// `url` with its origin swapped for the mock's.
fn to_mock(url: &str, mock: &str) -> String {
    for origin in [
        "https://api.anthropic.com",
        "https://api.openai.com",
        "http://localhost:11434",
    ] {
        if let Some(rest) = url.strip_prefix(origin) {
            return format!("{mock}{rest}");
        }
    }
    url.to_string()
}

#[seaquel_runtime::async_trait]
impl HttpClient for Redirect {
    async fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let headers = req
            .headers
            .iter()
            .map(|(n, v)| (n.clone(), v.expose().replace(TEST_KEY, "<key>")))
            .collect();
        self.sent.lock().unwrap().push(Sent {
            method: req.method.as_str(),
            url: req.url.clone(),
            headers,
            body: match req.method {
                Method::Get => Json::Null,
                Method::Post => serde_json::from_slice(&req.body).unwrap_or(Json::Null),
            },
        });
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Action::Forward);
        match action {
            Action::NetworkError => Err(HttpError::new(
                HttpErrorKind::Connect,
                "connection refused (scripted)",
            )),
            Action::Forward => {
                let mut req = req;
                req.url = to_mock(&req.url, self.mock.url());
                self.inner.send(req).await
            }
        }
    }
}

/// An SSE reply in 7-byte pieces, the recorder's split.
pub fn sse(body: &str, end: SseEnd) -> Reply {
    Reply::Sse {
        events: vec![body.to_string()],
        piece: 7,
        gap: Duration::ZERO,
        end,
    }
}

/// `changes.json`'s `*` rule 1 on a scripted round: a `run_query` call's
/// `query` argument is sent as `sql`. A round that decodes cleanly and has
/// such a call is sent again in the same provider's shape with the
/// argument renamed (text joined into one delta, no usage); any other body
/// goes as recorded.
pub fn renamed_round(body: &str, kind: ProviderKind) -> String {
    let mut decoder = Decoder::new(kind);
    let mut events = Vec::new();
    if decoder.feed(body.as_bytes(), &mut events).is_err() || decoder.finish(&mut events).is_err() {
        return body.to_string();
    }
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut stop = StopReason::End;
    let mut renamed = false;
    for e in events {
        match e {
            RoundEvent::Text(t) => text.push_str(&t),
            RoundEvent::ToolCall(mut c) => {
                if c.name == "run_query" {
                    if let Some(map) = c.input.as_object_mut() {
                        if let Some(v) = map.remove("query") {
                            map.insert("sql".into(), v);
                            renamed = true;
                        }
                    }
                }
                calls.push(c);
            }
            RoundEvent::Stop(s) => stop = s,
            RoundEvent::Usage(_) => {}
        }
    }
    if !renamed {
        return body.to_string();
    }
    use seaquel_ai::testing::scripts::{anthropic_event, openai_chunk, openai_done};
    let mut out = String::new();
    match kind {
        ProviderKind::Anthropic => {
            out.push_str(&anthropic_event(
                "message_start",
                json!({"type":"message_start","message":{"id":"msg","role":"assistant","content":[]}}),
            ));
            let mut index = 0;
            if !text.is_empty() {
                out.push_str(&anthropic_event("content_block_start", json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}})));
                out.push_str(&anthropic_event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}})));
                out.push_str(&anthropic_event(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":index}),
                ));
                index += 1;
            }
            for c in &calls {
                out.push_str(&anthropic_event("content_block_start", json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":c.id,"name":c.name,"input":{}}})));
                out.push_str(&anthropic_event("content_block_delta", json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":c.input.to_string()}})));
                out.push_str(&anthropic_event(
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":index}),
                ));
                index += 1;
            }
            let reason = match stop {
                StopReason::End => "end_turn",
                StopReason::ToolUse => "tool_use",
                StopReason::MaxTokens => "max_tokens",
            };
            out.push_str(&anthropic_event(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":reason}}),
            ));
            out.push_str(&anthropic_event(
                "message_stop",
                json!({"type":"message_stop"}),
            ));
        }
        ProviderKind::OpenAiCompatible => {
            if !text.is_empty() {
                out.push_str(&openai_chunk(
                    json!({"choices":[{"index":0,"delta":{"role":"assistant","content":text}}]}),
                ));
            }
            for (i, c) in calls.iter().enumerate() {
                out.push_str(&openai_chunk(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":i,"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.input.to_string()}}]}}]})));
            }
            let reason = match stop {
                StopReason::End => "stop",
                StopReason::ToolUse => "tool_calls",
                StopReason::MaxTokens => "length",
            };
            out.push_str(&openai_chunk(
                json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]}),
            ));
            out.push_str(&openai_done());
        }
    }
    out
}

// ── The workspace ──

/// One saved connection to seed.
#[derive(Clone)]
pub struct Conn {
    pub id: String,
    pub name: String,
    pub ty: String,
    pub share_schema: Option<bool>,
    pub share_data: Option<bool>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl Conn {
    pub fn new(id: &str, ty: &str) -> Self {
        Self {
            id: id.into(),
            name: "Local".into(),
            ty: ty.into(),
            share_schema: None,
            share_data: None,
            provider: Some("prov-1".into()),
            model: Some("model-1".into()),
        }
    }
}

/// What to build a world from.
pub struct Setup {
    pub conns: Vec<Conn>,
    /// `aiSettings.providers`.
    pub providers: Vec<Json>,
    pub share_schema_globally: bool,
    pub share_data_globally: bool,
    pub enabled: bool,
    /// The keychain's `ai-api-key:prov-1`; `None`: no store at all (web).
    pub key: Option<Option<String>>,
    pub egress: Option<AiEgress>,
    pub http: bool,
    pub limits: AiLimits,
    pub state_limits: StateLimits,
    pub executor: Option<Arc<dyn seaquel_runtime::Executor>>,
    /// Register the scripted engines (else the real ones).
    pub scripted: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            conns: vec![Conn::new("conn-1", "postgres")],
            providers: vec![json!({"id": "prov-1", "name": "Anthropic", "type": "anthropic"})],
            share_schema_globally: true,
            share_data_globally: false,
            enabled: true,
            key: Some(Some(TEST_KEY.to_string())),
            egress: Some(AiEgress::Any),
            http: true,
            limits: AiLimits::default(),
            state_limits: StateLimits::default(),
            executor: None,
            scripted: true,
        }
    }
}

/// Whether there is a DuckDB helper to run (`common/duckdb.rs`); without
/// one a real DuckDB case is skipped, or fails under
/// `SEAQUEL_TEST_REQUIRE_ENGINES`.
pub fn duckdb_helper_built() -> bool {
    duckdb_helper::built_helper().is_some()
}

pub struct World {
    pub dir: tempfile::TempDir,
    /// The DuckDB helper's install (`common/duckdb.rs`), when a real
    /// DuckDB connection asked for one and there is a helper.
    pub duckdb_helper: Option<tempfile::TempDir>,
    pub core: Core,
    pub ws: Arc<Workspace>,
    pub db: Arc<Db>,
    pub mock: MockProvider,
    pub http: Arc<Redirect>,
    pub store: Option<Arc<TestStore>>,
}

pub async fn world(setup: Setup) -> World {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Db::default());
    let mock = MockProvider::start().await;
    let http = Redirect::new(mock.clone());
    let saved_ids: Vec<String> = setup.conns.iter().map(|c| c.id.clone()).collect();
    let mut helper = None;
    let mut builder = if setup.scripted {
        let mut b = Core::builder();
        for id in ["postgres", "mysql", "sqlite", "mssql", "duckdb"] {
            b = b.engine(Arc::new(ScriptedEngine {
                id,
                db: db.clone(),
                saved_ids: saved_ids.clone(),
            }));
        }
        b
    } else if setup.conns.iter().any(|c| c.ty == "duckdb") {
        let (plugins, dir) = duckdb_helper::default_plugins();
        helper = dir;
        plugins
    } else {
        seaquel_core::with_default_plugins()
    };
    builder = builder
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(
            setup
                .executor
                .clone()
                .unwrap_or_else(|| Arc::new(seaquel_runtime::TokioExecutor)),
        )
        .ai_limits(setup.limits)
        .state_limits(setup.state_limits);
    if setup.http {
        builder = builder.ai_http(http.clone());
    }
    if let Some(egress) = setup.egress {
        builder = builder.ai_egress(egress);
    }
    let core = builder.build();
    let store = setup.key.as_ref().map(|key| {
        let store = TestStore::new();
        if let Some(key) = key {
            store.put("ai-api-key:prov-1", key);
        }
        store
    });
    let spec = WorkspaceSpec::new(dir.path());
    let spec = match &store {
        Some(store) => spec.with_secrets(store.clone()),
        None => spec,
    };
    let ws = core.open_workspace(spec).await.unwrap();
    insert_rows(
        ws.storage(),
        "projects",
        &[json!({"id": "p1", "name": "Main", "created_at": T0, "updated_at": T0})],
    )
    .await;
    for c in &setup.conns {
        let (host, database) = match c.ty.as_str() {
            "sqlite" | "duckdb" if !setup.scripted => ("".to_string(), ":memory:".to_string()),
            "sqlite" | "duckdb" => ("".to_string(), format!("/tmp/{}.db", c.id)),
            _ => ("h".to_string(), c.id.clone()),
        };
        insert_rows(
            ws.storage(),
            "connections",
            &[json!({
                "id": c.id, "project_id": "p1", "name": c.name, "type": c.ty,
                "host": host, "port": 5432, "database_name": database, "username": "u",
                "ai_share_schema": c.share_schema, "ai_share_data": c.share_data,
                "active_ai_provider_id": c.provider, "active_ai_model": c.model,
            })],
        )
        .await;
    }
    let settings = json!({
        "enabled": setup.enabled,
        "providers": setup.providers,
        "shareSchemaGlobally": setup.share_schema_globally,
        "shareDataGlobally": setup.share_data_globally,
    });
    insert_rows(
        ws.storage(),
        "app_state",
        &[json!({"key": "aiSettings", "value": settings.to_string()})],
    )
    .await;
    World {
        dir,
        duckdb_helper: helper,
        core,
        ws,
        db,
        mock,
        http,
        store,
    }
}

impl World {
    /// Open `saved` and answer Core's connection id.
    pub async fn connect(&self, saved: &str) -> String {
        let secrets = seaquel_core::SuppliedSecrets {
            db: Some("db-password".into()),
            ssh: None,
            ssh_key: None,
        };
        self.ws
            .connect(
                &self.core,
                ConnectRequest::saved(saved).with_secrets(secrets),
            )
            .await
            .unwrap()
    }

    /// A chat on `saved`, made directly in storage.
    pub async fn chat(&self, id: &str, saved: &str) {
        insert_rows(
            self.ws.storage(),
            "ai_chats",
            &[json!({"id": id, "connection_id": saved, "title": "T", "created_at": T0, "updated_at": T0})],
        )
        .await;
    }

    /// Stored messages, oldest first.
    pub async fn messages(&self, chat: &str) -> Vec<seaquel_types::storage::PersistedAIMessage> {
        seaquel_core::storage::ai_chats::load_messages(self.ws.storage(), chat)
            .await
            .unwrap()
    }

    /// A row already in the chat (history), at `T0` plus `n` seconds.
    pub async fn seed_message(&self, chat: &str, id: &str, role: &str, content: &str, n: u32) {
        insert_rows(
            self.ws.storage(),
            "ai_messages",
            &[
                json!({"id": id, "chat_id": chat, "role": role, "content": content,
                     "timestamp": format!("2026-01-01T00:00:{n:02}.000Z")}),
            ],
        )
        .await;
    }
}

/// The turn's request with defaults: chat `chat-1`, user message `u1`,
/// reply `a1`, ask before queries, no client tools, no key.
pub fn params(stream: &str, connection: &str, content: &str) -> ChatParams {
    ChatParams {
        stream_id: stream.into(),
        chat_id: "chat-1".into(),
        connection_id: connection.into(),
        user_message: ChatUserMessage {
            id: format!("{stream}-u"),
            content: content.into(),
        },
        assistant_message_id: format!("{stream}-a"),
        approval: Default::default(),
        client_tools: false,
        api_key: None,
        provider_id: None,
    }
}

/// What a test answers when the turn waits.
pub enum Answer {
    Decide(AiDecision),
    /// Cancel the turn (`Workspace::cancel`) and keep reading it.
    Cancel,
    /// Leave it waiting (the test does something else).
    Nothing,
}

/// Runs a turn to its end, answering each `approvalRequired` and
/// `clientTool` with `answer`; `on_text` may cancel after a text event.
pub async fn run_turn(
    w: &World,
    p: ChatParams,
    mut answer: impl FnMut(&AiEvent) -> Answer,
) -> Vec<AiEvent> {
    let stream_id = p.stream_id.clone();
    let mut stream =
        w.ws.ai_chat(&w.core, p, seaquel_core::WriteOrigin::new(Some("win-test")));
    let mut events = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(deadline);
    loop {
        let next = tokio::select! {
            e = stream.next() => e,
            () = &mut deadline => panic!("the turn didn't end: {events:?}"),
        };
        let Some(event) = next else { break };
        let reply = answer(&event);
        let call = match &event {
            AiEvent::ApprovalRequired { call_id, .. } | AiEvent::ClientTool { call_id, .. } => {
                Some(call_id.clone())
            }
            _ => None,
        };
        events.push(event);
        match reply {
            Answer::Decide(d) => {
                let call = call.expect("a decision answers a waiting event");
                w.ws.ai_respond(&stream_id, &call, d).unwrap();
            }
            Answer::Cancel => w.ws.cancel(&w.core, &stream_id),
            Answer::Nothing => {}
        }
    }
    events
}

/// The text a turn's `text` events add up to.
pub fn text_of(events: &[AiEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AiEvent::Text { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

pub fn terminal(events: &[AiEvent]) -> Option<&AiEvent> {
    events.iter().find(|e| e.is_terminal())
}

/// The events a workspace subscription holds now, without waiting.
pub fn drain(
    events: &mut seaquel_engine::BoxStream<'static, seaquel_core::WorkspaceEvent>,
) -> Vec<seaquel_core::WorkspaceEvent> {
    use futures::FutureExt;
    let mut out = Vec::new();
    while let Some(Some(e)) = events.next().now_or_never() {
        out.push(e);
    }
    out
}
