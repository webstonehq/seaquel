//! Shared helpers for integration tests.
//!
//! Each test binary lives in its own crate, so this module is pulled in via
//! `mod common;` at the top of each test file.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use seaquel_engine::{
    CappedResult, ConnectConfig, DbError, Dialect, Driver, Engine, ExecuteResult, OpenOptions,
    QueryResult, SchemaColumn, SchemaIndex, TransactionError,
};
use seaquel_engine_postgres::PostgresDialect;
use seaquel_server::{
    build_router, web_connect_policy, AppState, Workspaces, WEB_CONNECTION_LIMITS,
};
use seaquel_types::{BatchStatement, Value};
use serde_json::{json, Value as Json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

/// Issue a POST with a JSON body against the router and return `(status, body_json)`.
/// Empty response bodies come back as `Value::Null`.
pub async fn post_json(app: axum::Router, uri: &str, body: Json) -> (StatusCode, Json) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Json::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body was not valid JSON: {e}\nbody={}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, json)
}

// ── A fake "postgres" engine ──

/// What the fake driver saw.
#[derive(Default)]
pub struct Calls {
    pub opened: AtomicUsize,
    pub closed: AtomicUsize,
    pub query: AtomicUsize,
    pub read_only: AtomicUsize,
    pub execute: AtomicUsize,
    pub transaction: AtomicUsize,
    /// `table_metadata` reads (the edits' metadata, Decision 3).
    pub metadata: AtomicUsize,
    /// Queries that were started and are hanging (`hang` in the SQL).
    pub hanging: AtomicUsize,
    /// Hanging queries whose future was dropped (cancelled).
    pub dropped: AtomicUsize,
    /// Each opened config's connection string.
    pub configs: Mutex<Vec<String>>,
    /// Each open's pool size (`OpenOptions::max_pool_size`).
    pub pool_sizes: Mutex<Vec<Option<u32>>>,
}

impl Calls {
    /// Wait until `f` holds, or panic after 5 s.
    pub async fn wait(&self, what: &str, f: impl Fn(&Calls) -> bool) {
        for _ in 0..500 {
            if f(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {what}");
    }
}

/// Counts a drop of the future holding it.
struct DropFlag(Arc<Calls>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeDriver(Arc<Calls>);

fn one_cell(column: &str, value: &str) -> QueryResult {
    QueryResult {
        columns: vec![column.into()],
        rows: vec![vec![Value::Text(value.into())]],
    }
}

impl FakeDriver {
    async fn maybe_hang(&self, sql: &str) {
        if sql.contains("hang") {
            let _flag = DropFlag(self.0.clone());
            self.0.hanging.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
    }
}

#[seaquel_runtime::async_trait]
impl Driver for FakeDriver {
    /// One row, `[sql]`, or hangs until dropped when the SQL says `hang`.
    async fn query(&self, sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        self.0.query.fetch_add(1, Ordering::SeqCst);
        self.maybe_hang(sql).await;
        if sql.contains("big") {
            // Ten rows of 1 MiB each: one 10 MiB batch.
            let cell = "x".repeat(1024 * 1024);
            return Ok(QueryResult {
                columns: vec!["n".into(), "blob".into()],
                rows: (0..10)
                    .map(|i| vec![Value::Int(i), Value::Text(cell.clone())])
                    .collect(),
            });
        }
        if sql.contains("echo-fail") {
            // As Postgres and MySQL do, the message quotes the SQL.
            return Err(DbError::query_error(format!(
                "syntax error at or near \"{sql}\""
            )));
        }
        if sql.contains("fail") {
            return Err(DbError::query_error("the fake query failed"));
        }
        Ok(one_cell("sql", sql))
    }
    /// Three rows affected, or hangs until dropped when the SQL says `hang`.
    async fn execute(&self, sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        self.0.execute.fetch_add(1, Ordering::SeqCst);
        self.maybe_hang(sql).await;
        Ok(ExecuteResult {
            rows_affected: 3,
            last_insert_id: None,
        })
    }
    async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        self.0.transaction.fetch_add(1, Ordering::SeqCst);
        Ok(vec![0; statements.len()])
    }
    async fn close(&self) -> Result<(), DbError> {
        self.0.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    /// Every table has `id integer` (the primary key) and `name text`.
    async fn table_metadata(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.0.metadata.fetch_add(1, Ordering::SeqCst);
        let column = |name: &str, ty: &str, pk: bool| -> SchemaColumn {
            serde_json::from_value(json!({"name": name, "type": ty, "nullable": !pk,
                "isPrimaryKey": pk, "isForeignKey": false}))
            .unwrap()
        };
        Ok((
            vec![column("id", "integer", true), column("name", "text", false)],
            Vec::new(),
        ))
    }
    async fn query_read_only(
        &self,
        sql: &str,
        _params: Vec<Value>,
        max_rows: Option<usize>,
    ) -> Result<CappedResult, DbError> {
        self.0.read_only.fetch_add(1, Ordering::SeqCst);
        self.maybe_hang(sql).await;
        if sql.contains("refuse") {
            return Err(DbError::read_only(
                "cannot execute INSERT in a read-only transaction",
            ));
        }
        if sql.contains("three") {
            let n = max_rows.unwrap_or(3).min(3);
            return Ok(CappedResult {
                columns: vec!["n".into()],
                rows: (1..=n as i64).map(|i| vec![Value::Int(i)]).collect(),
                truncated: n < 3,
            });
        }
        Ok(one_cell("mode", "read_only").into())
    }
}

/// "postgres" with the real Postgres dialect over [`FakeDriver`].
pub struct FakePostgres(pub Arc<Calls>);

#[seaquel_runtime::async_trait]
impl Engine for FakePostgres {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        self.open_with(config, OpenOptions::default()).await
    }
    async fn open_with(
        &self,
        config: &ConnectConfig,
        options: OpenOptions,
    ) -> Result<Arc<dyn Driver>, DbError> {
        self.0
            .pool_sizes
            .lock()
            .unwrap()
            .push(options.max_pool_size);
        self.0.opened.fetch_add(1, Ordering::SeqCst);
        self.0
            .configs
            .lock()
            .unwrap()
            .push(config.connection_string.clone().unwrap_or_default());
        Ok(Arc::new(FakeDriver(self.0.clone())))
    }
    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&PostgresDialect)
    }
}

// ── A test server ──

/// A router over the fake engine under the web connect policy and
/// connection limits, with its own data dir and an LRU of `capacity` workspaces.
pub struct Env {
    pub app: axum::Router,
    pub state: AppState,
    pub calls: Arc<Calls>,
    pub dir: tempfile::TempDir,
}

impl Env {
    pub fn new(capacity: usize) -> Self {
        let calls = Arc::new(Calls::default());
        let core = seaquel_core::Core::builder()
            .engine(Arc::new(FakePostgres(calls.clone())))
            .connect_policy(web_connect_policy())
            .connection_limits(WEB_CONNECTION_LIMITS)
            .run_limits(seaquel_server::WEB_RUN_LIMITS)
            .edit_limits(seaquel_server::WEB_EDIT_LIMITS)
            .executor(Arc::new(seaquel_runtime::TokioExecutor))
            .build();
        Self::with_core(Arc::new(core), capacity, calls)
    }

    pub fn with_core(core: Arc<seaquel_core::Core>, capacity: usize, calls: Arc<Calls>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState {
            core,
            workspaces: Arc::new(Workspaces::with_capacity(dir.path(), capacity)),
            license: Arc::new(seaquel_core::license::server::LicenseServer::new(
                seaquel_core::license::server::ServerConfig::new(dir.path().join("auth.db")),
            )),
            internal_secret: None,
        };
        Self {
            app: build_router(state.clone()),
            state,
            calls,
            dir,
        }
    }

    /// POST `body` to /rpc as `user`.
    pub async fn rpc(&self, user: &str, body: &Json) -> (StatusCode, Json) {
        let req = Request::builder()
            .method("POST")
            .uri("/rpc")
            .header("content-type", "application/json")
            .header("x-seaquel-user", user)
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// A `db` call as `user`: `(status, body)`.
    pub async fn db(&self, user: &str, method: &str, params: Json) -> (StatusCode, Json) {
        self.rpc(user, &db(method, params)).await
    }

    /// Connect `form` as `user`; returns the connection id.
    pub async fn connect(&self, user: &str, form: Json) -> String {
        let (status, body) = self
            .db(
                user,
                "connect",
                json!({"target": {"type": "form", "form": form}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "connect: {body}");
        body["result"]["result"]["connectionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Serve the router on a free loopback port.
    pub async fn serve(&self) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = self.app.clone();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }
}

/// `{"method":"db","params":{"method":…,"params":…}}`.
pub fn db(method: &str, params: Json) -> Json {
    json!({"method": "db", "params": {"method": method, "params": params}})
}

/// A Postgres form the web policy lets through.
pub fn pg_form() -> Json {
    json!({"type": "postgres", "name": "f", "host": "db.example.com", "port": 5432,
           "databaseName": "app", "username": "u"})
}

// ── /rpc/stream clients ──

pub type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open `/rpc/stream` as `user`.
pub async fn open_stream(addr: std::net::SocketAddr, user: &str) -> Ws {
    let mut req = format!("ws://{addr}/rpc/stream")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("x-seaquel-user", user.parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

/// A `start` frame for a `db.queryStream` of `sql` on `connection_id`.
pub fn start(stream_id: &str, connection_id: &str, sql: &str) -> Json {
    start_with(
        stream_id,
        json!({"connectionId": connection_id, "sql": sql}),
    )
}

/// A `start` frame with these queryStream params (`streamId` added).
pub fn start_with(stream_id: &str, mut params: Json) -> Json {
    params["streamId"] = json!(stream_id);
    json!({"op": "start", "streamId": stream_id,
           "request": db("queryStream", params)})
}

/// A `start` frame for a `db.run` of `text` (every statement) on
/// `connection_id` at `page_size`.
pub fn start_run(stream_id: &str, connection_id: &str, text: &str, page_size: u32) -> Json {
    start_run_with(
        stream_id,
        json!({"connectionId": connection_id, "text": text, "target": {"type": "all"},
               "pageSize": page_size}),
    )
}

/// A `start` frame with these run params (`streamId` added).
pub fn start_run_with(stream_id: &str, mut params: Json) -> Json {
    params["streamId"] = json!(stream_id);
    json!({"op": "start", "streamId": stream_id, "request": db("run", params)})
}

/// A `start` frame for a `db.page` of `sql` on `connection_id`.
pub fn start_page(
    stream_id: &str,
    connection_id: &str,
    sql: &str,
    page: u32,
    page_size: u32,
) -> Json {
    json!({"op": "start", "streamId": stream_id, "request": db("page", json!({
        "connectionId": connection_id, "streamId": stream_id,
        "source": {"sql": sql, "params": []}, "page": page, "pageSize": page_size,
    }))})
}

/// A `start` frame for a `db.tablePage` of `public.<table>` with these
/// filters on `connection_id`.
pub fn start_table_page(
    stream_id: &str,
    connection_id: &str,
    table: &str,
    filters: Json,
    page: u32,
    page_size: u32,
) -> Json {
    json!({"op": "start", "streamId": stream_id, "request": db("tablePage", json!({
        "connectionId": connection_id, "streamId": stream_id, "page": page, "pageSize": page_size,
        "query": {"target": {"schema": "public", "table": table}, "filters": filters},
    }))})
}

/// The event types of `frames`.
pub fn event_types(frames: &[Json]) -> Vec<String> {
    frames
        .iter()
        .map(|f| f["event"]["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

pub async fn send(ws: &mut Ws, frame: &Json) {
    ws.send(Message::Text(frame.to_string())).await.unwrap();
}

/// The next JSON frame, or panic after 5 s or when the socket ends.
pub async fn next(ws: &mut Ws) -> Json {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("no frame within 5 s")
            .expect("the socket ended")
            .expect("socket error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

/// Frames until `stream_id` has its `done` or `error`; others go to `other`.
pub async fn until_end(ws: &mut Ws, stream_id: &str, other: &mut Vec<Json>) -> Vec<Json> {
    let mut out = Vec::new();
    loop {
        let frame = next(ws).await;
        if frame["streamId"] == stream_id {
            let kind = frame["event"]["type"].clone();
            out.push(frame);
            if kind == "done" || kind == "error" {
                return out;
            }
        } else {
            other.push(frame);
        }
    }
}

/// Nothing arrives within `ms`.
pub async fn quiet(ws: &mut Ws, ms: u64) {
    if let Ok(Some(Ok(Message::Text(t)))) =
        tokio::time::timeout(Duration::from_millis(ms), ws.next()).await
    {
        panic!("unexpected frame: {t}");
    }
}
