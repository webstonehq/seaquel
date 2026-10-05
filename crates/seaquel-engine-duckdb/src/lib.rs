//! DuckDB engine for Seaquel.
//!
//! The drivers share the dialect, the introspection SQL and parsers, and the
//! Arrow decoder:
//!
//! - **`remote`** (the default) and **`helper`** (the DuckDB helper plan): a
//!   driver over a `seaquel-duckdb` child process ([`remote_engine`]) and the
//!   loop that process runs ([`helper::serve`]), talking in `wire.rs`'s
//!   frames, the rows as Arrow IPC with the column kinds beside them
//!   (`kinds.rs`). Every native interface reaches DuckDB this way. The
//!   helper's DuckDB work (opening, binds, transactions, the read-only path)
//!   is `session.rs`, which hands each result's Arrow chunks to a sink.
//! - **`browser`** (wasm32 only): DuckDB-WASM in the page, reached through a
//!   bridge object the page passes in ([`browser_engine`]). Result batches
//!   cross as Arrow IPC bytes, read by `ipc.rs` and decoded by the same
//!   `decode.rs`.
//!
//! There is no in-process native driver: it was deleted in Task 12 of the
//! desktop DuckDB helper plan, once every interface ran DuckDB in the helper.

#[cfg(not(any(feature = "remote", feature = "helper", feature = "browser")))]
compile_error!("seaquel-engine-duckdb needs the `remote`, `helper` or `browser` feature");

// The remote driver starts a process, which the browser can't.
#[cfg(all(feature = "remote", target_arch = "wasm32"))]
compile_error!("seaquel-engine-duckdb's `remote` feature is native only (it starts a process)");

// The helper alone uses only `decode::Kind` (it sends the kinds, its
// clients decode).
#[cfg_attr(not(any(feature = "remote", feature = "browser")), allow(dead_code))]
mod decode;
mod dialect;
pub mod introspect;

#[cfg(feature = "helper")]
mod blocking;
#[cfg(feature = "helper")]
mod session;

#[cfg(any(feature = "browser", feature = "remote", test))]
mod ipc;

#[cfg(any(feature = "browser", test))]
mod browser;

#[cfg(any(feature = "remote", feature = "helper", test))]
mod wire;

#[cfg(feature = "helper")]
pub mod helper;
#[cfg(feature = "helper")]
mod kinds;

#[cfg(feature = "remote")]
pub mod remote;

/// The reference the IPC, session and helper comparisons check against.
#[cfg(all(test, feature = "helper"))]
mod test_reference;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use browser::{browser_engine, DuckDbBridge};

#[cfg(feature = "remote")]
pub use remote::{remote_engine, HelperLocator};

pub use dialect::{parse_dotted, qualified_table, quote_schema, DuckdbDialect};
