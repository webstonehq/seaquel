//! Integration test: the `read_only` flag of the WebSocket at /api/db/stream.
//!
//! A fake "postgres" engine answers `query` and `query_read_only` differently,
//! so each test can tell which path the first frame picked. Its read-only
//! query can also hang until dropped, for the cancel-by-close check.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use common::post_json;
use futures::{SinkExt, StreamExt};
use seaquel_engine::{ConnectConfig, DbError, Driver, Engine, ExecuteResult, QueryResult};
use seaquel_server::{build_router, AppState};
use seaquel_types::Value;
use serde_json::{json, Value as Json};
use tokio_tungstenite::tungstenite::Message;

#[derive(Default)]
struct Calls {
    query: AtomicUsize,
    read_only: AtomicUsize,
    /// Set when a hanging read-only query's future is dropped.
    dropped: AtomicBool,
}

/// Sets `dropped` when the future holding it is dropped.
struct DropFlag(Arc<Calls>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.dropped.store(true, Ordering::SeqCst);
    }
}

struct FakeDriver(Arc<Calls>);

fn one_cell(column: &str, value: &str) -> QueryResult {
    QueryResult {
        columns: vec![column.into()],
        rows: vec![vec![Value::Text(value.into())]],
    }
}

#[seaquel_runtime::async_trait]
impl Driver for FakeDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        self.0.query.fetch_add(1, Ordering::SeqCst);
        Ok(one_cell("mode", "read_write"))
    }
    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        Err(DbError::execute_error("not in this test"))
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn query_read_only(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        self.0.read_only.fetch_add(1, Ordering::SeqCst);
        if sql.contains("hang") {
            let _flag = DropFlag(self.0.clone());
            std::future::pending::<()>().await;
        }
        if sql.contains("refuse") {
            return Err(DbError::read_only(
                "cannot execute INSERT in a read-only transaction",
            ));
        }
        Ok(one_cell("mode", "read_only"))
    }
}

struct FakeEngine(Arc<Calls>);

#[seaquel_runtime::async_trait]
impl Engine for FakeEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(FakeDriver(self.0.clone())))
    }
}

/// A served router over the fake engine, one open connection, and its calls.
async fn fake_server() -> (std::net::SocketAddr, String, Arc<Calls>) {
    let calls = Arc::new(Calls::default());
    let core = seaquel_core::Core::builder()
        .engine(Arc::new(FakeEngine(calls.clone())))
        .build();
    let app = build_router(AppState {
        core: Arc::new(core),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_app = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, serve_app).await.unwrap();
    });
    let (status, body) = post_json(app, "/api/db/connect", json!({ "driver": "postgres" })).await;
    assert_eq!(status, StatusCode::OK, "connect failed: {body}");
    let connection_id = body["connection_id"].as_str().unwrap().to_string();
    (addr, connection_id, calls)
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: std::net::SocketAddr, first_frame: Json) -> Ws {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/db/stream"))
        .await
        .unwrap();
    ws.send(Message::Text(first_frame.to_string()))
        .await
        .unwrap();
    ws
}

/// Every JSON frame until the server goes away. The handler returns after
/// the terminal frame without a close handshake, so a reset ends it too.
async fn frames(mut ws: Ws) -> Vec<Json> {
    let mut out = Vec::new();
    while let Some(Ok(msg)) = ws.next().await {
        match msg {
            Message::Text(t) => out.push(serde_json::from_str(&t).unwrap()),
            Message::Close(_) => break,
            _ => {}
        }
    }
    out
}

fn request(connection_id: &str, sql: &str, read_only: Option<bool>) -> Json {
    let mut frame = json!({
        "query_id": uuid::Uuid::new_v4().to_string(),
        "connection_id": connection_id,
        "sql": sql,
        "values": [],
    });
    if let Some(read_only) = read_only {
        frame["read_only"] = json!(read_only);
    }
    frame
}

#[tokio::test]
async fn read_only_frame_runs_the_read_only_query() {
    let (addr, id, calls) = fake_server().await;
    let got = frames(open(addr, request(&id, "SELECT 1", Some(true))).await).await;
    assert_eq!(
        got,
        vec![
            json!({ "type": "batch", "columns": ["mode"], "rows": [["read_only"]], "is_final": true }),
            json!({ "type": "done" }),
        ]
    );
    assert_eq!(calls.read_only.load(Ordering::SeqCst), 1);
    assert_eq!(calls.query.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn frame_without_the_flag_or_with_false_runs_the_normal_query() {
    let (addr, id, calls) = fake_server().await;
    for flag in [None, Some(false)] {
        let got = frames(open(addr, request(&id, "SELECT 1", flag)).await).await;
        assert_eq!(got[0]["rows"], json!([["read_write"]]), "{flag:?}");
        assert_eq!(got.last().unwrap(), &json!({ "type": "done" }), "{flag:?}");
    }
    assert_eq!(calls.read_only.load(Ordering::SeqCst), 0);
    assert_eq!(calls.query.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn read_only_frame_refuses_a_write_before_the_driver() {
    let (addr, id, calls) = fake_server().await;
    let got = frames(open(addr, request(&id, "DELETE FROM t", Some(true))).await).await;
    assert_eq!(
        got,
        vec![json!({
            "type": "error",
            "code": "READ_ONLY",
            "message": "Only read-only SELECT queries are permitted",
        })]
    );
    assert_eq!(calls.read_only.load(Ordering::SeqCst), 0);
    assert_eq!(calls.query.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn read_only_frame_reports_the_databases_refusal() {
    let (addr, id, _calls) = fake_server().await;
    let got = frames(open(addr, request(&id, "SELECT refuse()", Some(true))).await).await;
    assert_eq!(
        got,
        vec![json!({
            "type": "error",
            "code": "READ_ONLY",
            "message": "cannot execute INSERT in a read-only transaction",
        })]
    );
}

/// The web client cancels by closing the socket. A read-only query sends
/// nothing until it finishes, so the server has to notice the close while it
/// waits, not on its next send.
#[tokio::test]
async fn closing_the_socket_drops_a_running_read_only_query() {
    let (addr, id, calls) = fake_server().await;
    let mut ws = open(addr, request(&id, "SELECT hang()", Some(true))).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.read_only.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the read-only query never started");
    assert!(!calls.dropped.load(Ordering::SeqCst));

    ws.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !calls.dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("closing the socket didn't drop the query");
}

/// Dropping the TCP connection without a close frame cancels too.
#[tokio::test]
async fn dropping_the_socket_drops_a_running_read_only_query() {
    let (addr, id, calls) = fake_server().await;
    let ws = open(addr, request(&id, "SELECT hang()", Some(true))).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.read_only.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the read-only query never started");

    drop(ws);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !calls.dropped.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the socket didn't drop the query");
}
