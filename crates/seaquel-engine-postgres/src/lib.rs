//! PostgreSQL engine for Seaquel.

mod bind;
mod decode;
mod dialect;
mod driver;
pub mod introspect;
mod numeric;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine, OpenOptions};

pub use dialect::PostgresDialect;

pub struct PostgresEngine;

#[seaquel_runtime::async_trait]
impl Engine for PostgresEngine {
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
        Ok(Arc::new(
            driver::PostgresDriver::connect(config, options).await?,
        ))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&PostgresDialect)
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(PostgresEngine)
}
