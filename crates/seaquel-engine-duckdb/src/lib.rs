//! DuckDB engine for Seaquel.
//!
//! The drivers share the dialect, the introspection SQL and parsers, and the
//! Arrow decoder:
//!
//! - **`native`** (the default): duckdb-rs, each call on a blocking thread.
//!   Desktop, the CLI and the MCP server. Its DuckDB work (opening, binds,
//!   transactions, the read-only path) is `session.rs`, which hands each
//!   result's Arrow chunks to a sink; the driver's sink decodes them into
//!   `Value`s.
//! - **`browser`** (wasm32 only): DuckDB-WASM in the page, reached through a
//!   bridge object the page passes in ([`browser_engine`]). Result batches
//!   cross as Arrow IPC bytes, read by `ipc.rs` and decoded by the same
//!   `decode.rs`.
//! - **`remote`** and **`helper`** (the DuckDB helper plan): a driver over a
//!   `seaquel-duckdb` child process and the loop that process runs, talking
//!   in `wire.rs`'s frames, the rows as Arrow IPC. The helper's loop is
//!   [`helper::serve`]; the remote driver comes in the plan's Task 3.

#[cfg(not(any(feature = "native", feature = "remote", feature = "browser")))]
compile_error!("seaquel-engine-duckdb needs the `native`, `remote` or `browser` feature");

// The remote driver starts a process, which the browser can't.
#[cfg(all(feature = "remote", target_arch = "wasm32"))]
compile_error!("seaquel-engine-duckdb's `remote` feature is native only (it starts a process)");

mod decode;
mod dialect;
pub mod introspect;

#[cfg(feature = "native")]
mod blocking;
#[cfg(feature = "native")]
mod driver;
#[cfg(feature = "native")]
mod session;

#[cfg(any(feature = "browser", feature = "remote", test))]
mod ipc;

#[cfg(any(feature = "browser", test))]
mod browser;

#[cfg(any(feature = "remote", feature = "helper", test))]
mod wire;

#[cfg(feature = "helper")]
pub mod helper;

#[cfg(feature = "remote")]
pub mod remote;

/// The native typed-cell cases, for the IPC and session comparisons.
#[cfg(all(test, feature = "native"))]
#[path = "../tests/common/cells.rs"]
#[allow(dead_code)]
mod test_cells;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use browser::{browser_engine, DuckDbBridge};

#[cfg(feature = "remote")]
pub use remote::{remote_engine, HelperLocator};

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
