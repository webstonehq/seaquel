//! MySQL and MariaDB engine for Seaquel. MariaDB connections use the "mysql" driver on the wire.

mod decode;
mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct MysqlEngine;

#[seaquel_runtime::async_trait]
impl Engine for MysqlEngine {
    fn id(&self) -> &'static str {
        "mysql"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::MysqlDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(MysqlEngine)
}
