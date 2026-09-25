//! PostgreSQL engine for Seaquel.

mod decode;
mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct PostgresEngine;

#[seaquel_runtime::async_trait]
impl Engine for PostgresEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::PostgresDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(PostgresEngine)
}
