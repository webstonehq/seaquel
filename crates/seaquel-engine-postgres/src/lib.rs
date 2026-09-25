//! PostgreSQL engine for Seaquel.

mod bind;
mod decode;
mod numeric;
mod dialect;
mod driver;
pub mod introspect;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

pub use dialect::PostgresDialect;

pub struct PostgresEngine;

#[seaquel_runtime::async_trait]
impl Engine for PostgresEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::PostgresDriver::connect(config).await?))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&PostgresDialect)
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(PostgresEngine)
}
