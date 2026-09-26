//! SQL text work for the Svelte app, compiled to WebAssembly by
//! `seaquel-wasm`. Pure: no I/O, builds for `wasm32-unknown-unknown`.
//!
//! - [`scan`]: the hand scanner (ported from `src/lib/engine/sql-scan.ts`,
//!   deleted in phase 2b), statement splitting, statement at cursor, row-limit
//!   detection.
//! - [`statements`]: query type, destructive-statement check, source table for
//!   inline editing.
//! - [`read_only`]: the AI's read-only check.
//! - [`params`]: `{{param}}` extraction and substitution.
//! - [`create_table`]: `CREATE TABLE` text to a `CreateTableDefinition`.
//! - [`ast`]: sqlparser-rs, only where an AST is needed (the query builder and
//!   tutorial `ParsedQuery`, the Visual tab, column sources).
//!
//! Every offset in this crate's API is a UTF-8 byte offset into the input,
//! `end` exclusive. `seaquel-wasm` converts to UTF-16 at the boundary.
//!
//! Nothing here may panic on user input: in the module a panic is a trap.

pub mod ast;
pub mod create_table;
mod engine;
mod js_word;
mod js_ws;
pub mod params;
pub mod read_only;
pub mod scan;
pub mod statements;

pub use engine::{SqlEngine, UnknownEngine};
