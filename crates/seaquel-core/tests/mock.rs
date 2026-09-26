//! Core's stream lifecycle against mock drivers, so no database is needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::{Core, QueryOptions, StreamEvent};
use seaquel_engine::{
    BoxStream, CancellationToken, CastMap, ConnectConfig, DatabaseStatistics, DbError, Dialect,
    Driver, Engine, ExecuteResult, ExplainResult, QueryResult, RowValues, SchemaColumn,
    SchemaIndex, SchemaTable, SqlWithBindings, StreamBatch, Value,
};
use seaquel_types::{ColumnTypeInfo, CreateTableDefinition};
use serde_json::json;
use tokio::sync::Notify;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);

/// How [`MockDriver::query_stream`] behaves.
#[derive(Clone, Copy)]
enum Mode {
    /// The default (non-streaming) `query_stream`, over a `query()` that never
    /// returns — like a slow MSSQL query.
    HangingQuery,
    /// Yield one batch, then wait for the cancel token, like the sqlx drivers.
    /// `close()` waits until the stream is dropped, like `pool.close()`
    /// waiting for a checked-out connection.
    HoldsConnection,
    /// Yield one batch, then an error, then a batch that must never be seen.
    FailsMidStream,
}

struct MockDriver {
    mode: Mode,
    /// Set when the stream holding the "pooled connection" is dropped.
    released: AtomicBool,
    release: Notify,
}

impl MockDriver {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            released: AtomicBool::new(false),
            release: Notify::new(),
        })
    }
}

/// Marks the driver's connection as released when dropped.
struct Checkout<'a>(&'a MockDriver);

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}

fn batch(n: i64) -> StreamBatch {
    StreamBatch {
        columns: Some(vec!["n".into()]),
        rows: vec![vec![Value::Int(n)]],
        is_final: false,
    }
}

#[seaquel_runtime::async_trait]
impl Driver for MockDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        futures::future::pending().await
    }

    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        unimplemented!()
    }

    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        match self.mode {
            Mode::HangingQuery => {
                // The trait's default body, which can't be called from an override.
                Box::pin(async_stream::try_stream! {
                    let result = self.query(&sql, params).await?;
                    yield StreamBatch {
                        columns: Some(result.columns),
                        rows: result.rows,
                        is_final: true,
                    };
                })
            }
            Mode::HoldsConnection => Box::pin(async_stream::stream! {
                let _checkout = Checkout(self);
                yield Ok(batch(1));
                cancel.cancelled().await;
            }),
            Mode::FailsMidStream => Box::pin(futures::stream::iter([
                Ok(batch(1)),
                Err(DbError::query_error("boom")),
                Ok(batch(2)),
            ])),
        }
    }

    async fn close(&self) -> Result<(), DbError> {
        if let Mode::HoldsConnection = self.mode {
            loop {
                let released = self.release.notified();
                if self.released.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        Ok(())
    }
}

struct MockEngine(Arc<MockDriver>);

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.0.clone())
    }
}

async fn connect(mode: Mode) -> (Core, String) {
    let core = Core::builder()
        .engine(Arc::new(MockEngine(MockDriver::new(mode))))
        .build();
    let config: ConnectConfig = serde_json::from_value(json!({ "driver": "sqlite" })).unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    (core, id)
}

fn summary(events: &[StreamEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            StreamEvent::Batch(_) => "batch",
            StreamEvent::Done => "done",
            StreamEvent::Error { .. } => "error",
        })
        .collect()
}

/// A stream ended by `disconnect` tells the client, which would otherwise wait
/// for a terminal event forever.
fn assert_connection_closed(event: &StreamEvent) {
    match event {
        StreamEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_CLOSED"),
        other => panic!("expected CONNECTION_CLOSED, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_interrupts_a_driver_that_does_not_stream() {
    let (core, id) = connect(Mode::HangingQuery).await;
    let stream = core.query_stream(
        "q1".into(),
        id,
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );

    let (events, ()) = timeout(LIMIT, async {
        // `join!` polls the consumer first, so the query is in flight when
        // the cancel arrives.
        tokio::join!(stream.collect::<Vec<_>>(), async {
            core.cancel_stream("q1");
        })
    })
    .await
    .expect("cancelled stream did not end");

    assert!(events.is_empty(), "{:?}", summary(&events));
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn disconnect_cancels_the_connections_streams() {
    let (core, id) = connect(Mode::HoldsConnection).await;
    let mut stream = core.query_stream(
        "q1".into(),
        id.clone(),
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );

    let (events, disconnected) = timeout(LIMIT, async {
        tokio::join!(
            async {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    events.push(event);
                }
                events
            },
            core.disconnect(&id),
        )
    })
    .await
    .expect("disconnect waited on the running stream");

    disconnected.unwrap();
    assert_eq!(summary(&events), vec!["batch", "error"]);
    assert_connection_closed(&events[1]);
    assert_eq!(core.running_stream_count(), 0);
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn disconnect_leaves_other_connections_streams_running() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    // A second connection, on the same mock engine.
    let config: ConnectConfig = serde_json::from_value(json!({ "driver": "sqlite" })).unwrap();
    let other = core.connect(&config).await.unwrap().connection_id;

    let stream = core.query_stream(
        "q1".into(),
        other,
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );
    core.disconnect(&id).await.unwrap();

    let events = timeout(LIMIT, stream.collect::<Vec<_>>()).await.unwrap();
    assert_eq!(summary(&events), vec!["batch", "error"]);
}

#[tokio::test]
async fn driver_error_mid_stream_ends_with_one_error() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    let events = timeout(
        LIMIT,
        core.query_stream(
            "q1".into(),
            id,
            "SELECT 1".into(),
            vec![],
            QueryOptions::default(),
        )
        .collect::<Vec<_>>(),
    )
    .await
    .unwrap();

    assert_eq!(summary(&events), vec!["batch", "error"]);
    match &events[1] {
        StreamEvent::Error { message, .. } => assert!(message.contains("boom"), "{message}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn a_stream_whose_connection_closed_before_its_first_poll_reports_it() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    let stream = core.query_stream(
        "q1".into(),
        id.clone(),
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );
    core.disconnect(&id).await.unwrap();

    let events = timeout(LIMIT, stream.collect::<Vec<_>>()).await.unwrap();
    assert_eq!(summary(&events), vec!["error"]);
    assert_connection_closed(&events[0]);
    assert_eq!(core.running_stream_count(), 0);
}

// ── Engine, dialect and introspection ──

/// A dialect whose output names the method and its arguments, so a test can
/// see which call reached it.
struct MockDialect;

impl Dialect for MockDialect {
    fn quote_ident(&self, id: &str) -> String {
        format!("[{id}]")
    }
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
        format!("{sql} /* page {limit}@{offset} */")
    }
    fn build_update(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        value: Value,
        _pks: &[String],
        _row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!("update {schema}.{table}.{column}"),
            bind_values: Some(vec![value]),
        }
    }
    fn build_set_default(
        &self,
        schema: &str,
        table: &str,
        column: &str,
        _pks: &[String],
        _row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!("default {schema}.{table}.{column}"),
            bind_values: None,
        }
    }
    fn build_insert(
        &self,
        schema: &str,
        table: &str,
        _values: &[(String, Value)],
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!("insert {schema}.{table}"),
            bind_values: None,
        }
    }
    fn build_delete(
        &self,
        schema: &str,
        table: &str,
        _pks: &[String],
        _row: &RowValues,
        _casts: Option<&CastMap>,
    ) -> SqlWithBindings {
        SqlWithBindings {
            sql: format!("delete {schema}.{table}"),
            bind_values: None,
        }
    }
    fn create_table(&self, def: &CreateTableDefinition) -> String {
        format!("create {}", def.table_name)
    }
    fn alter_table(&self, _from: &CreateTableDefinition, to: &CreateTableDefinition) -> String {
        format!("alter {}", to.table_name)
    }
    fn column_types(&self) -> Vec<ColumnTypeInfo> {
        vec![]
    }
    fn explain_sql(&self, sql: &str, analyze: bool) -> String {
        format!("explain {analyze} {sql}")
    }
}

/// A driver that implements the introspection methods and records each call.
#[derive(Default)]
struct IntrospectingDriver {
    calls: StdMutex<Vec<String>>,
}

impl IntrospectingDriver {
    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[seaquel_runtime::async_trait]
impl Driver for IntrospectingDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        unimplemented!()
    }
    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        unimplemented!()
    }
    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        self.record("list_schemas".into());
        Ok(vec!["public".into(), "sales".into()])
    }
    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        self.record("schema_tables".into());
        Ok(vec![serde_json::from_value(json!({
            "name": "orders", "schema": "sales", "type": "table", "columns": [], "indexes": []
        }))
        .unwrap()])
    }
    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.record(format!("table_metadata {schema}.{table}"));
        Ok((vec![], vec![]))
    }
    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        self.record("statistics".into());
        Ok(serde_json::from_value(json!({
            "overview": { "databaseName": "mock", "totalSize": "0 B", "tableCount": 0, "indexCount": 0 },
            "tableSizes": [],
            "indexUsage": []
        }))
        .unwrap())
    }
    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        self.record(format!("explain {sql} {} {analyze}", params.len()));
        Ok(serde_json::from_value(json!({
            "plan": { "id": "0", "nodeType": "Result", "children": [] },
            "planningTime": 0.5,
            "isAnalyze": analyze
        }))
        .unwrap())
    }
}

/// A "postgres" engine with a dialect, opening one shared introspecting driver.
struct DialectEngine(Arc<IntrospectingDriver>);

#[seaquel_runtime::async_trait]
impl Engine for DialectEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.0.clone())
    }
    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&MockDialect)
    }
}

/// A Core with the dialect engine ("postgres") and the plain mock engine
/// ("sqlite", no dialect, no introspection).
fn two_engine_core() -> (Core, Arc<IntrospectingDriver>) {
    let driver = Arc::new(IntrospectingDriver::default());
    let core = Core::builder()
        .engine(Arc::new(DialectEngine(driver.clone())))
        .engine(Arc::new(MockEngine(MockDriver::new(Mode::FailsMidStream))))
        .build();
    (core, driver)
}

async fn connect_to(core: &Core, driver: &str) -> String {
    let config: ConnectConfig = serde_json::from_value(json!({ "driver": driver })).unwrap();
    core.connect(&config).await.unwrap().connection_id
}

#[tokio::test]
async fn each_connection_keeps_the_engine_that_opened_it() {
    let (core, _) = two_engine_core();
    let pg = connect_to(&core, "postgres").await;
    let lite = connect_to(&core, "sqlite").await;

    assert_eq!(core.engine(&pg).unwrap().id(), "postgres");
    assert_eq!(core.engine(&lite).unwrap().id(), "sqlite");
    assert!(core.engine(&pg).unwrap().dialect().is_some());
    assert!(core.engine(&lite).unwrap().dialect().is_none());
}

#[tokio::test]
async fn engine_of_an_unknown_or_closed_connection_is_not_found() {
    let (core, _) = two_engine_core();
    let err = core.engine("nope").err().unwrap();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");

    let pg = connect_to(&core, "postgres").await;
    core.disconnect(&pg).await.unwrap();
    let err = core.engine(&pg).err().unwrap();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    let err = core.with_dialect(&pg, |d| d.quote_ident("x")).unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
}

#[tokio::test]
async fn with_dialect_runs_against_the_connections_dialect() {
    let (core, _) = two_engine_core();
    let pg = connect_to(&core, "postgres").await;

    let sql = core
        .with_dialect(&pg, |d| d.paginate("SELECT 1", 10, 20))
        .unwrap();
    assert_eq!(sql, "SELECT 1 /* page 10@20 */");

    let row: RowValues = vec![("id".into(), Value::Int(1))];
    let update = core
        .with_dialect(&pg, |d| {
            d.build_update("s", "t", "c", Value::Int(7), &["id".into()], &row, None)
        })
        .unwrap();
    assert_eq!(update.sql, "update s.t.c");
    assert_eq!(update.bind_values, Some(vec![Value::Int(7)]));
}

#[tokio::test]
async fn with_dialect_on_an_engine_without_one_is_not_supported() {
    let (core, _) = two_engine_core();
    let lite = connect_to(&core, "sqlite").await;
    let mut ran = false;
    let err = core
        .with_dialect(&lite, |_| {
            ran = true;
        })
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
    assert!(!ran);
}

#[tokio::test]
async fn introspection_passes_through_to_the_driver() {
    let (core, driver) = two_engine_core();
    let pg = connect_to(&core, "postgres").await;

    assert_eq!(
        core.list_schemas(&pg).await.unwrap(),
        vec!["public", "sales"]
    );
    let tables = core.schema_tables(&pg).await.unwrap();
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].name, "orders");
    let (columns, indexes) = core.table_metadata(&pg, "sales", "orders").await.unwrap();
    assert!(columns.is_empty() && indexes.is_empty());
    let stats = core.statistics(&pg).await.unwrap();
    assert_eq!(stats.overview.database_name, "mock");
    let plan = core
        .explain(&pg, "SELECT $1", vec![Value::Int(1)], true)
        .await
        .unwrap();
    assert!(plan.is_analyze);

    assert_eq!(
        *driver.calls.lock().unwrap(),
        vec![
            "list_schemas",
            "schema_tables",
            "table_metadata sales.orders",
            "statistics",
            "explain SELECT $1 1 true",
        ]
    );
}

#[tokio::test]
async fn introspection_reports_the_drivers_not_supported_and_unknown_connections() {
    let (core, _) = two_engine_core();
    let lite = connect_to(&core, "sqlite").await;

    let codes = |id: String| {
        let core = &core;
        async move {
            vec![
                core.list_schemas(&id).await.unwrap_err().code,
                core.schema_tables(&id).await.unwrap_err().code,
                core.table_metadata(&id, "s", "t").await.unwrap_err().code,
                core.statistics(&id).await.unwrap_err().code,
                core.explain(&id, "SELECT 1", vec![], false)
                    .await
                    .unwrap_err()
                    .code,
            ]
        }
    };
    assert_eq!(codes(lite).await, vec!["NOT_SUPPORTED"; 5]);
    assert_eq!(codes("nope".into()).await, vec!["CONNECTION_NOT_FOUND"; 5]);
}

// ── Read-only queries ──

/// A driver whose `query_read_only` records each call. `query` and
/// `query_stream` panic, so a read-only stream that fell back to them fails.
struct ReadOnlyDriver {
    calls: StdMutex<Vec<(String, usize)>>,
    /// `query_read_only` never returns, holding its "connection" until the
    /// future is dropped. `close()` waits for that, like `pool.close()`.
    hangs: bool,
    released: AtomicBool,
    release: Notify,
}

impl ReadOnlyDriver {
    fn new(hangs: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: StdMutex::default(),
            hangs,
            released: AtomicBool::new(false),
            release: Notify::new(),
        })
    }

    fn calls(&self) -> Vec<(String, usize)> {
        self.calls.lock().unwrap().clone()
    }
}

struct ReadOnlyCheckout<'a>(&'a ReadOnlyDriver);

impl Drop for ReadOnlyCheckout<'_> {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}

#[seaquel_runtime::async_trait]
impl Driver for ReadOnlyDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        panic!("a read-only stream must not call query()")
    }

    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        panic!("a read-only stream must not call execute()")
    }

    fn query_stream<'a>(
        &'a self,
        _sql: String,
        _params: Vec<Value>,
        _cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        panic!("a read-only stream must not call query_stream()")
    }

    async fn query_read_only(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        self.calls
            .lock()
            .unwrap()
            .push((sql.to_string(), params.len()));
        if sql.contains("refuse me") {
            return Err(DbError::read_only(
                "cannot execute INSERT in a read-only transaction",
            ));
        }
        if self.hangs {
            let _checkout = ReadOnlyCheckout(self);
            futures::future::pending::<()>().await;
        }
        Ok(QueryResult {
            columns: vec!["n".into()],
            rows: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        })
    }

    async fn close(&self) -> Result<(), DbError> {
        if self.hangs {
            loop {
                let released = self.release.notified();
                if self.released.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        Ok(())
    }
}

/// An engine with any id, opening one shared read-only driver.
struct ReadOnlyEngine(&'static str, Arc<ReadOnlyDriver>);

#[seaquel_runtime::async_trait]
impl Engine for ReadOnlyEngine {
    fn id(&self) -> &'static str {
        self.0
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.1.clone())
    }
}

async fn connect_read_only(
    engine: &'static str,
    hangs: bool,
) -> (Core, String, Arc<ReadOnlyDriver>) {
    let driver = ReadOnlyDriver::new(hangs);
    let core = Core::builder()
        .engine(Arc::new(ReadOnlyEngine(engine, driver.clone())))
        .build();
    let id = connect_to(&core, engine).await;
    (core, id, driver)
}

fn read_only() -> QueryOptions {
    QueryOptions::default().with_read_only(true)
}

async fn collect_read_only(core: &Core, id: &str, sql: &str) -> Vec<StreamEvent> {
    timeout(
        LIMIT,
        core.query_stream("q1".into(), id.into(), sql.into(), vec![], read_only())
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap()
}

fn assert_error(event: &StreamEvent, code: &str, message: &str) {
    match event {
        StreamEvent::Error {
            code: c,
            message: m,
        } => {
            assert_eq!(c, code);
            assert_eq!(m, message);
        }
        other => panic!("expected {code}, got {other:?}"),
    }
}

#[test]
fn query_options_default_to_read_write() {
    assert!(!QueryOptions::default().read_only);
    assert!(read_only().read_only);
    assert!(!read_only().with_read_only(false).read_only);
}

#[tokio::test]
async fn read_only_runs_the_drivers_read_only_query_as_one_batch() {
    let (core, id, driver) = connect_read_only("postgres", false).await;
    let events = timeout(
        LIMIT,
        core.query_stream(
            "q1".into(),
            id,
            "SELECT n FROM t WHERE n > $1".into(),
            vec![Value::Int(0)],
            read_only(),
        )
        .collect::<Vec<_>>(),
    )
    .await
    .unwrap();

    assert_eq!(summary(&events), vec!["batch", "done"]);
    match &events[0] {
        StreamEvent::Batch(b) => {
            assert_eq!(b.columns, Some(vec!["n".to_string()]));
            assert_eq!(b.rows, vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
            assert!(b.is_final);
        }
        other => panic!("expected a batch, got {other:?}"),
    }
    assert_eq!(
        driver.calls(),
        vec![("SELECT n FROM t WHERE n > $1".to_string(), 1)]
    );
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn read_only_refused_by_the_token_check_never_reaches_the_driver() {
    let (core, id, driver) = connect_read_only("postgres", false).await;
    for sql in [
        "DELETE FROM t",
        "SELECT 1; DROP TABLE t",
        "SELECT pg_advisory_lock(7)",
        "",
    ] {
        let events = collect_read_only(&core, &id, sql).await;
        assert_eq!(summary(&events), vec!["error"], "{sql:?}");
        assert_error(
            &events[0],
            "READ_ONLY",
            seaquel_sql::read_only::READ_ONLY_MESSAGE,
        );
    }
    assert!(driver.calls().is_empty(), "{:?}", driver.calls());
}

#[tokio::test]
async fn read_only_checks_with_the_connections_engine() {
    // SQL Server runs statements that follow each other without a `;`, so
    // only its rules block KILL; MySQL's refuse MariaDB's `/*M!` comments.
    let (core, id, driver) = connect_read_only("mssql", false).await;
    let events = collect_read_only(&core, &id, "SELECT 1 AS k KILL 9999").await;
    assert_error(
        &events[0],
        "READ_ONLY",
        seaquel_sql::read_only::READ_ONLY_MESSAGE,
    );
    assert!(driver.calls().is_empty());

    let (core, id, driver) = connect_read_only("postgres", false).await;
    let events = collect_read_only(&core, &id, "SELECT 1 AS k KILL 9999").await;
    assert_eq!(summary(&events), vec!["batch", "done"]);
    assert_eq!(driver.calls().len(), 1);

    let (core, id, driver) = connect_read_only("mysql", false).await;
    let events = collect_read_only(&core, &id, "SELECT 1 /*M! , 2 */").await;
    assert_error(
        &events[0],
        "READ_ONLY",
        seaquel_sql::read_only::READ_ONLY_MESSAGE,
    );
    let events = collect_read_only(&core, &id, "SELECT 1").await;
    assert_eq!(summary(&events), vec!["batch", "done"]);
    assert_eq!(driver.calls().len(), 1);
}

#[tokio::test]
async fn read_only_passes_the_drivers_refusal_through() {
    let (core, id, _) = connect_read_only("postgres", false).await;
    let events = collect_read_only(&core, &id, "SELECT 'refuse me'").await;
    assert_eq!(summary(&events), vec!["error"]);
    assert_error(
        &events[0],
        "READ_ONLY",
        "cannot execute INSERT in a read-only transaction",
    );
}

#[tokio::test]
async fn read_only_on_a_driver_without_it_is_not_supported() {
    // The plain mock driver implements `query_stream`, not `query_read_only`.
    let (core, id) = connect(Mode::FailsMidStream).await;
    let events = collect_read_only(&core, &id, "SELECT 1").await;
    assert_eq!(summary(&events), vec!["error"]);
    match &events[0] {
        StreamEvent::Error { code, .. } => assert_eq!(code, "NOT_SUPPORTED"),
        other => panic!("expected NOT_SUPPORTED, got {other:?}"),
    }
}

#[tokio::test]
async fn read_only_on_an_unknown_connection_is_not_found() {
    let (core, _, _) = connect_read_only("postgres", false).await;
    let events = collect_read_only(&core, "nope", "SELECT 1").await;
    assert_eq!(summary(&events), vec!["error"]);
    match &events[0] {
        StreamEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_NOT_FOUND"),
        other => panic!("expected CONNECTION_NOT_FOUND, got {other:?}"),
    }
}

#[tokio::test]
async fn read_only_cancelled_before_it_starts_never_reaches_the_driver() {
    let (core, id, driver) = connect_read_only("postgres", false).await;
    let stream = core.query_stream("q1".into(), id, "SELECT 1".into(), vec![], read_only());
    core.cancel_stream("q1");
    let events = timeout(LIMIT, stream.collect::<Vec<_>>()).await.unwrap();
    assert!(events.is_empty(), "{:?}", summary(&events));
    assert!(driver.calls().is_empty());
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn read_only_cancel_drops_the_running_query() {
    let (core, id, driver) = connect_read_only("postgres", true).await;
    let stream = core.query_stream("q1".into(), id, "SELECT 1".into(), vec![], read_only());

    let (events, ()) = timeout(LIMIT, async {
        tokio::join!(stream.collect::<Vec<_>>(), async {
            // Let the driver's future start before cancelling.
            tokio::task::yield_now().await;
            core.cancel_stream("q1");
        })
    })
    .await
    .expect("cancelled read-only stream did not end");

    assert!(events.is_empty(), "{:?}", summary(&events));
    assert_eq!(driver.calls().len(), 1, "the query must have started");
    assert!(
        driver.released.load(Ordering::SeqCst),
        "the driver's future was not dropped"
    );
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn read_only_disconnect_ends_with_connection_closed() {
    let (core, id, driver) = connect_read_only("postgres", true).await;
    let mut stream = core.query_stream(
        "q1".into(),
        id.clone(),
        "SELECT 1".into(),
        vec![],
        read_only(),
    );

    let (events, disconnected) = timeout(LIMIT, async {
        tokio::join!(
            async {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    events.push(event);
                }
                events
            },
            async {
                tokio::task::yield_now().await;
                core.disconnect(&id).await
            },
        )
    })
    .await
    .expect("disconnect waited on the running read-only query");

    disconnected.unwrap();
    assert_eq!(summary(&events), vec!["error"]);
    assert_connection_closed(&events[0]);
    assert_eq!(driver.calls().len(), 1);
    assert!(driver.released.load(Ordering::SeqCst));
    assert_eq!(core.running_stream_count(), 0);
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn read_write_streams_ignore_query_read_only() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    let events = timeout(
        LIMIT,
        core.query_stream(
            "q1".into(),
            id,
            "DELETE FROM t".into(),
            vec![],
            QueryOptions::default(),
        )
        .collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    // The token check doesn't run: the driver's stream does.
    assert_eq!(summary(&events), vec!["batch", "error"]);
}
