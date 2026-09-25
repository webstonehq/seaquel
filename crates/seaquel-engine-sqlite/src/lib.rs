//! SQLite engine for Seaquel.

mod bind;
mod decode;
mod dialect;
mod driver;
pub mod introspect;
mod timing;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

pub use dialect::SqliteDialect;

pub struct SqliteEngine;

#[seaquel_runtime::async_trait]
impl Engine for SqliteEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::SqliteDriver::connect(config).await?))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&SqliteDialect)
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(SqliteEngine)
}
