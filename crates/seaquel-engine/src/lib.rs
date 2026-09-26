//! The engine plugin boundary.
//!
//! Each database engine is its own crate (`seaquel-engine-postgres`, …) that
//! implements [`Engine`] and [`Driver`]. Seaquel Core looks engines up in an
//! [`EngineRegistry`] by the `driver` field of [`ConnectConfig`], so nothing
//! outside Core's default-plugin list names an engine crate.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`. The
//! [`impl_sqlx_driver!`] macro is only expanded by native engine crates.

use std::collections::HashMap;
use std::sync::Arc;

pub use seaquel_runtime::{BoxStream, MaybeSend, MaybeSync};
pub use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DatabaseStatistics, DbError, DriverType,
    ExecuteResult, ExpectRows, ExplainResult, QueryResult, SchemaColumn, SchemaIndex, SchemaTable,
    SqlWithBindings, StreamBatch, Value, MAX_SAFE_INTEGER,
};
pub use tokio_util::sync::CancellationToken;

pub mod crud;
pub mod ddl;
mod dialect;
pub mod introspect;
mod sqlx_driver;

pub use dialect::{CastMap, Dialect, RowValues};

#[doc(hidden)]
pub mod __private {
    pub use async_stream;
    pub use async_trait;
    pub use futures;
    pub use log;

    /// `impl_sqlx_driver!`: name the column a cell failed to decode in.
    pub fn in_column(mut e: crate::DbError, column: &str) -> crate::DbError {
        e.message = format!("{} (column \"{column}\")", e.message);
        e
    }
}

/// Cap for non-streaming `query()` results. Streaming `query_stream` is
/// unbounded by design (the UI paginates). This cap exists to prevent a
/// `SELECT * FROM big_table` submitted through the non-streaming path from
/// OOM-killing the process. Override with `SEAQUEL_MAX_QUERY_ROWS`.
pub fn max_query_rows() -> usize {
    use std::sync::OnceLock;
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("SEAQUEL_MAX_QUERY_ROWS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|v: &usize| *v > 0)
            .unwrap_or(100_000)
    })
}

/// The error an engine returns for an operation it hasn't implemented yet.
pub fn not_supported(what: &str) -> DbError {
    DbError {
        message: format!("{what} is not supported by this engine yet"),
        code: "NOT_SUPPORTED".to_string(),
    }
}

/// An open connection (or pool) to one database.
#[seaquel_runtime::async_trait]
pub trait Driver: MaybeSend + MaybeSync {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError>;

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError>;

    /// Execute multiple statements in a single transaction on one connection.
    ///
    /// Drivers **must** override this to provide real atomicity. The default
    /// returns an error rather than silently running each statement on its
    /// own pooled connection — that was the old behaviour and it let a
    /// mid-batch failure leave earlier writes committed. Loudly failing here
    /// surfaces the gap instead of producing half-written data.
    ///
    /// Before COMMIT, every statement's affected rows must pass
    /// [`BatchStatement::check_affected`]; on a shortfall the driver rolls
    /// back and returns that `NO_ROWS_AFFECTED` error.
    async fn transaction(&self, _statements: Vec<BatchStatement>) -> Result<(), DbError> {
        Err(DbError {
            message: "transactions are not supported by this driver".to_string(),
            code: "TRANSACTION_NOT_SUPPORTED".to_string(),
        })
    }

    /// Stream query results in batches.
    ///
    /// Drivers that genuinely stream (the sqlx-based ones) override this and
    /// stop fetching as soon as `cancel` fires. The default runs `query()` and
    /// emits a single terminal batch, so DuckDB and MSSQL get a
    /// correct-but-non-streaming path for free; it can't interrupt `query()`,
    /// so it ignores `cancel` and Core drops the result instead.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        let _ = cancel;
        Box::pin(async_stream::try_stream! {
            let result = self.query(&sql, params).await?;
            yield StreamBatch {
                columns: Some(result.columns),
                rows: result.rows,
                is_final: true,
            };
        })
    }

    /// Run one query that must not change anything: the AI's `run_query`
    /// tool and dashboard widgets (Core's `read_only` query option).
    ///
    /// The default fails with `NOT_SUPPORTED`, so an engine that doesn't
    /// implement it fails closed. Never implement it by calling
    /// [`Driver::query`]. An implementation must meet this contract:
    ///
    /// - **Exactly one statement runs.** Input holding more than one is
    ///   refused before anything runs, or refused by the database as a whole.
    /// - **The database refuses writes**, through the engine's own read-only
    ///   mode (a read-only transaction, session or connection). Where the
    ///   engine has none (SQL Server), everything runs in a transaction that
    ///   is always rolled back, and a query that ends that transaction
    ///   itself is reported.
    /// - **Nothing the query does to its session outlives the call.** The
    ///   connection it ran on is closed, or is one the engine owns for this
    ///   path alone and is left as it was found. A pooled or shared
    ///   connection is never handed back read-only, inside a transaction, or
    ///   with changed settings, locks or prepared statements.
    /// - **Dropping the future cancels the query.** It stops as far as the
    ///   engine allows, and the driver stays usable for the next call.
    ///   Cancellation is drop-only: no `CancellationToken` is passed, so an
    ///   engine whose query runs where dropping the future doesn't reach it
    ///   must interrupt it from a `Drop` guard held by the future. DuckDB
    ///   (the query runs on `spawn_blocking`) calls the clone's interrupt
    ///   handle from that guard; SQL Server runs each call on a connection
    ///   of its own, so a drop at any await drops that connection and the
    ///   server rolls back. The sqlx engines stop when their connection is
    ///   dropped (closed, with `close_on_drop()`).
    /// - **Refusals are `READ_ONLY`.** The engine's read-only refusal (and
    ///   the driver's own, e.g. "one statement at a time") comes back as
    ///   [`DbError::read_only`] with the database's message. Other errors
    ///   keep their usual codes.
    /// - **Rows are capped** like [`Driver::query`]: past
    ///   [`max_query_rows`] it fails with `RESULT_TOO_LARGE`.
    ///
    /// Core runs the AI's token check (`seaquel_sql::read_only`) before
    /// calling this. Drivers must not rely on it: the testkit's read-only
    /// harness calls them directly with SQL the check would refuse.
    async fn query_read_only(
        &self,
        _sql: &str,
        _params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        Err(not_supported("Running read-only queries"))
    }

    async fn close(&self) -> Result<(), DbError>;

    // ── Introspection ──
    //
    // Engines whose dialect still lives in TypeScript leave these alone; the
    // frontend's TypeScript client handles them.

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        Err(not_supported("Listing schemas"))
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        Err(not_supported("Loading the schema"))
    }

    async fn table_metadata(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        Err(not_supported("Loading table metadata"))
    }

    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        Err(not_supported("Database statistics"))
    }

    async fn explain(
        &self,
        _sql: &str,
        _params: Vec<Value>,
        _analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        Err(not_supported("EXPLAIN"))
    }
}

/// A database engine plugin: knows how to open [`Driver`]s for one `driver`
/// value on the wire.
#[seaquel_runtime::async_trait]
pub trait Engine: MaybeSend + MaybeSync {
    /// Stable id. Must equal [`DriverType::as_str`] for the driver it serves.
    fn id(&self) -> &'static str;

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError>;

    /// The engine's SQL dialect, when it has moved to Rust.
    fn dialect(&self) -> Option<&dyn Dialect> {
        None
    }
}

/// The engines compiled into this build, keyed by id.
#[derive(Default, Clone)]
pub struct EngineRegistry {
    engines: HashMap<&'static str, Arc<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an engine.
    ///
    /// # Panics
    ///
    /// If an engine with the same id is already registered. Two engines
    /// claiming one id is a build mistake, not a runtime condition.
    pub fn register(&mut self, engine: Arc<dyn Engine>) {
        let id = engine.id();
        if self.engines.insert(id, engine).is_some() {
            panic!("engine \"{id}\" registered twice");
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Engine>> {
        self.engines.get(id).cloned()
    }

    /// Registered ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<_> = self.engines.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Open a driver with the engine matching `config.driver`.
    pub async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let id = config.driver.as_str();
        let engine = self
            .get(id)
            .ok_or_else(|| DbError::engine_not_available(id))?;
        engine.open(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use futures::StreamExt;
    use serde_json::json;

    #[test]
    fn decode_errors_name_the_column() {
        let e = __private::in_column(DbError::query_error("can't decode a TIME value"), "t");
        assert_eq!(e.code, "QUERY_ERROR");
        assert_eq!(
            e.message,
            "Query failed: can't decode a TIME value (column \"t\")"
        );
    }

    struct FakeDriver;

    #[seaquel_runtime::async_trait]
    impl Driver for FakeDriver {
        async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
            Ok(QueryResult {
                columns: vec!["a".into()],
                rows: vec![vec![Value::Int(1)]],
            })
        }

        async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
            Ok(ExecuteResult {
                rows_affected: 0,
                last_insert_id: None,
            })
        }

        async fn close(&self) -> Result<(), DbError> {
            Ok(())
        }
    }

    struct FakeEngine(&'static str);

    #[seaquel_runtime::async_trait]
    impl Engine for FakeEngine {
        fn id(&self) -> &'static str {
            self.0
        }

        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(FakeDriver))
        }
    }

    fn config(driver: &str) -> ConnectConfig {
        serde_json::from_value(json!({ "driver": driver })).unwrap()
    }

    #[test]
    fn open_dispatches_on_the_driver_field() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        let driver = block_on(registry.open(&config("sqlite"))).unwrap();
        let result = block_on(driver.query("SELECT 1", vec![])).unwrap();
        assert_eq!(result.columns, vec!["a"]);
    }

    #[test]
    fn open_reports_a_missing_engine() {
        let registry = EngineRegistry::new();
        let err = block_on(registry.open(&config("postgres"))).err().unwrap();
        assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
        assert!(err.message.contains("\"postgres\""), "{}", err.message);
    }

    #[test]
    fn ids_are_sorted() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("duckdb")));
        assert_eq!(registry.ids(), vec!["duckdb", "sqlite"]);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn registering_an_id_twice_panics() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("sqlite")));
    }

    #[test]
    fn introspection_defaults_are_not_supported() {
        let d = FakeDriver;
        let codes = [
            block_on(d.list_schemas()).err().unwrap(),
            block_on(d.schema_tables()).err().unwrap(),
            block_on(d.table_metadata("public", "t")).err().unwrap(),
            block_on(d.statistics()).err().unwrap(),
            block_on(d.explain("SELECT 1", vec![], false))
                .err()
                .unwrap(),
        ];
        for err in codes {
            assert_eq!(err.code, "NOT_SUPPORTED");
            assert!(
                err.message.ends_with("is not supported by this engine yet"),
                "{}",
                err.message
            );
        }
    }

    /// Fail closed: a driver that implements `query` but not
    /// `query_read_only` must not run read-only queries through `query`.
    #[test]
    fn read_only_queries_are_not_supported_by_default() {
        let err = block_on(FakeDriver.query_read_only("SELECT 1", vec![]))
            .err()
            .unwrap();
        assert_eq!(err.code, "NOT_SUPPORTED");
        assert_eq!(
            err.message,
            "Running read-only queries is not supported by this engine yet"
        );
    }

    #[test]
    fn engines_have_no_dialect_by_default() {
        assert!(FakeEngine("sqlite").dialect().is_none());
    }

    /// A dialect built from the generic builders, used as a trait object.
    struct TinyDialect;

    impl Dialect for TinyDialect {
        fn quote_ident(&self, id: &str) -> String {
            format!("\"{}\"", id.replace('"', "\"\""))
        }
        fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
            format!("{sql} LIMIT {limit} OFFSET {offset}")
        }
        fn build_update(
            &self,
            schema: &str,
            table: &str,
            column: &str,
            value: Value,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_update(
                schema,
                table,
                column,
                value,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn build_set_default(
            &self,
            schema: &str,
            table: &str,
            column: &str,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_set_default(
                schema,
                table,
                column,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn build_insert(
            &self,
            schema: &str,
            table: &str,
            values: &[(String, Value)],
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_insert(schema, table, values, &qi, casts, &crud::dollar_placeholder)
        }
        fn build_delete(
            &self,
            schema: &str,
            table: &str,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_delete(
                schema,
                table,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn create_table(&self, def: &seaquel_types::CreateTableDefinition) -> String {
            ddl::generate_create_table_ddl(def, &|s| self.quote_ident(s))
        }
        fn alter_table(
            &self,
            from: &seaquel_types::CreateTableDefinition,
            to: &seaquel_types::CreateTableDefinition,
        ) -> String {
            let opts = ddl::AlterTableOptions {
                qualify_drop_index: true,
                ..Default::default()
            };
            ddl::generate_alter_table_sql(from, to, &|s| self.quote_ident(s), opts)
        }
        fn column_types(&self) -> Vec<seaquel_types::ColumnTypeInfo> {
            vec![]
        }
        fn explain_sql(&self, sql: &str, analyze: bool) -> String {
            format!("EXPLAIN {}{sql}", if analyze { "ANALYZE " } else { "" })
        }
    }

    struct EngineWithDialect;

    #[seaquel_runtime::async_trait]
    impl Engine for EngineWithDialect {
        fn id(&self) -> &'static str {
            "postgres"
        }
        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(FakeDriver))
        }
        fn dialect(&self) -> Option<&dyn Dialect> {
            Some(&TinyDialect)
        }
    }

    #[test]
    fn a_dialect_is_usable_through_the_engine() {
        let engine: Arc<dyn Engine> = Arc::new(EngineWithDialect);
        let d = engine.dialect().unwrap();
        let row: RowValues = vec![("id".into(), Value::Int(9))];
        let out = d.build_delete("public", "we\"ird", &["id".to_string()], &row, None);
        assert_eq!(
            out.sql,
            "DELETE FROM \"public\".\"we\"\"ird\" WHERE \"id\" = $1"
        );
        assert_eq!(out.bind_values, Some(vec![Value::Int(9)]));
        assert_eq!(
            d.paginate("SELECT 1", 10, 20),
            "SELECT 1 LIMIT 10 OFFSET 20"
        );
    }

    #[test]
    fn default_query_stream_emits_one_final_batch() {
        let driver = FakeDriver;
        let batches: Vec<_> = block_on(
            driver
                .query_stream("SELECT 1".into(), vec![], CancellationToken::new())
                .collect(),
        );
        assert_eq!(batches.len(), 1);
        let batch = batches.into_iter().next().unwrap().unwrap();
        assert_eq!(batch.columns, Some(vec!["a".to_string()]));
        assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);
        assert!(batch.is_final);
    }
}
