//! Microsoft SQL Server engine for Seaquel.

mod bind;
mod decode;
mod dialect;
mod driver;
pub mod introspect;
mod session;

#[cfg(test)]
mod live_tests;

pub use dialect::MssqlDialect;
pub use driver::MssqlDriver;
pub use session::ResultSet;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

pub struct MssqlEngine;

#[seaquel_runtime::async_trait]
impl Engine for MssqlEngine {
    fn id(&self) -> &'static str {
        "mssql"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::MssqlDriver::connect(config).await?))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&MssqlDialect)
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(MssqlEngine)
}
