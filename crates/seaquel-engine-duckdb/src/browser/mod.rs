//! The browser driver (phase 8): DuckDB-WASM in the page, behind a bridge.
//!
//! - [`binds`]: bound values written into the SQL as literals (Decision 9).
//! - [`ipc`]: DuckDB-WASM's Arrow IPC bytes read into rows by the shared
//!   decoder (Decision 8).
//! - `bridge` and `driver` (wasm32 only): the JavaScript bridge and the
//!   `Driver` over it.
//!
//! `binds` and `ipc` are pure and also built for native tests, where
//! `decode_from_ipc_matches_decode_from_duckdb` checks the IPC path against
//! the native driver's decoding.

#![cfg_attr(
    not(all(feature = "browser", target_arch = "wasm32")),
    allow(dead_code)
)]

pub(crate) mod binds;
pub(crate) mod ipc;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod bridge;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod driver;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use bridge::DuckDbBridge;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use driver::browser_engine;

/// The native typed-cell cases, for the IPC comparison.
#[cfg(all(test, feature = "native"))]
#[path = "../../tests/common/cells.rs"]
#[allow(dead_code)]
mod cells;
