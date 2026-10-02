//! The SQL layer under every query function (phase 8 Decision 3): one set
//! of queries, two executors, chosen by target and never by a Cargo
//! feature, so feature unification can't put both in one build.
//!
//! - **Native** (desktop, web, CLI): [`native`] re-exports sqlx's own
//!   types and functions under these names. The query modules call
//!   `db::query(…)` where they called `sqlx::query(…)`, and nothing else
//!   changes: the same calls reach the same sqlx code.
//! - **wasm32** (the demo): [`mem`] is a wrapper over SQLite compiled to
//!   wasm (`sqlite-wasm-rs`), shaped like the part of sqlx this crate uses:
//!   the same function and type names, the same decode rules, the same
//!   error codes and messages. One connection to an in-memory database;
//!   every call is synchronous behind an `async` signature.
//!
//! Nothing outside this module names `sqlx` or the C API.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod mem;
#[cfg(target_arch = "wasm32")]
pub use mem::*;
