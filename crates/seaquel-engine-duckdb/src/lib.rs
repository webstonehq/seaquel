//! DuckDB engine for Seaquel.
//!
//! Two drivers share the dialect, the introspection SQL and parsers, and the
//! Arrow decoder:
//!
//! - **`native`** (the default): duckdb-rs, each call on a blocking thread.
//!   Desktop, the CLI and the MCP server.
//! - **`browser`** (wasm32 only): DuckDB-WASM in the page, reached through a
//!   bridge object the page passes in ([`browser_engine`]). Result batches
//!   cross as Arrow IPC bytes and are decoded by the same `decode.rs`.

#[cfg(not(any(feature = "native", feature = "browser")))]
compile_error!("seaquel-engine-duckdb needs the `native` or the `browser` feature (or both)");

mod decode;
mod dialect;
pub mod introspect;

#[cfg(feature = "native")]
mod blocking;
#[cfg(feature = "native")]
mod driver;

#[cfg(any(feature = "browser", test))]
mod browser;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use browser::{browser_engine, DuckDbBridge};

pub use dialect::{parse_dotted, qualified_table, quote_schema, DuckdbDialect};

#[cfg(feature = "native")]
pub use native::{engine, DuckdbEngine};

#[cfg(feature = "native")]
mod native {
    use std::sync::Arc;

    use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

    use crate::dialect::DuckdbDialect;
    use crate::driver;

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
}
