//! SQLite engine for Seaquel.

mod bind;
mod decode;
mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct SqliteEngine;

#[seaquel_runtime::async_trait]
impl Engine for SqliteEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::SqliteDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(SqliteEngine)
}
