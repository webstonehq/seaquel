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
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch,
};
pub use tokio_util::sync::CancellationToken;

mod sqlx_driver;

#[doc(hidden)]
pub mod __private {
    pub use async_stream;
    pub use async_trait;
    pub use futures;
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

/// An open connection (or pool) to one database.
#[seaquel_runtime::async_trait]
pub trait Driver: MaybeSend + MaybeSync {
    async fn query(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError>;

    async fn execute(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError>;

    /// Execute multiple statements in a single transaction on one connection.
    ///
    /// Drivers **must** override this to provide real atomicity. The default
    /// returns an error rather than silently running each statement on its
    /// own pooled connection — that was the old behaviour and it let a
    /// mid-batch failure leave earlier writes committed. Loudly failing here
    /// surfaces the gap instead of producing half-written data.
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
        params: Vec<serde_json::Value>,
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

    async fn close(&self) -> Result<(), DbError>;
}

/// A database engine plugin: knows how to open [`Driver`]s for one `driver`
/// value on the wire.
#[seaquel_runtime::async_trait]
pub trait Engine: MaybeSend + MaybeSync {
    /// Stable id. Must equal [`DriverType::as_str`] for the driver it serves.
    fn id(&self) -> &'static str;

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError>;
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

    struct FakeDriver;

    #[seaquel_runtime::async_trait]
    impl Driver for FakeDriver {
        async fn query(
            &self,
            _sql: &str,
            _params: Vec<serde_json::Value>,
        ) -> Result<QueryResult, DbError> {
            Ok(QueryResult {
                columns: vec!["a".into()],
                rows: vec![vec![json!(1)]],
            })
        }

        async fn execute(
            &self,
            _sql: &str,
            _params: Vec<serde_json::Value>,
        ) -> Result<ExecuteResult, DbError> {
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
        assert_eq!(batch.rows, vec![vec![json!(1)]]);
        assert!(batch.is_final);
    }
}
