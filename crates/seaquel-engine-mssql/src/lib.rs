//! Microsoft SQL Server engine for Seaquel.

mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct MssqlEngine;

#[seaquel_runtime::async_trait]
impl Engine for MssqlEngine {
    fn id(&self) -> &'static str {
        "mssql"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::MssqlDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(MssqlEngine)
}
