//! DuckDB engine for Seaquel.

mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct DuckdbEngine;

#[seaquel_runtime::async_trait]
impl Engine for DuckdbEngine {
    fn id(&self) -> &'static str {
        "duckdb"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::DuckdbDriver::connect(config)?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(DuckdbEngine)
}
