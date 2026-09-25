//! DuckDB engine for Seaquel.

mod blocking;
mod decode;
mod dialect;
mod driver;
pub mod introspect;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

pub use dialect::{parse_dotted, qualified_table, quote_schema, DuckdbDialect};

pub struct DuckdbEngine;

#[seaquel_runtime::async_trait]
impl Engine for DuckdbEngine {
    fn id(&self) -> &'static str {
        "duckdb"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::DuckdbDriver::connect(config).await?))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&DuckdbDialect)
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(DuckdbEngine)
}
