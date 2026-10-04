//! The browser driver (phase 8): DuckDB-WASM in the page, behind a bridge.
//!
//! - [`binds`]: bound values written into the SQL as literals (Decision 9).
//! - DuckDB-WASM's Arrow IPC bytes are read into rows by [`crate::ipc`]
//!   (Decision 8), which the DuckDB helper's client shares.
//! - `bridge` and `driver` (wasm32 only): the JavaScript bridge and the
//!   `Driver` over it.
//!
//! `binds` is pure and also built for native tests.

#![cfg_attr(
    not(all(feature = "browser", target_arch = "wasm32")),
    allow(dead_code)
)]

pub(crate) mod binds;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod bridge;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
mod driver;

#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use bridge::DuckDbBridge;
#[cfg(all(feature = "browser", target_arch = "wasm32"))]
pub use driver::browser_engine;
