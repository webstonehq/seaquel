//! Seaquel Core in the web page (phase 8): the browser demo's WebAssembly
//! module, built by `scripts/build-wasm.mjs --module browser` into
//! `src/lib/wasm/browser-pkg/`. The editor's `seaquel-wasm` is a separate
//! module.
//!
//! It builds one Core with the DuckDB engine over the page's DuckDB-WASM
//! (`seaquel_engine_duckdb::browser_engine`, through a bridge object the page
//! passes in), `ConnectPolicy::Unrestricted`, `WasmExecutor` and no limits
//! (Q6 A), and opens one workspace whose metadata file lives in memory
//! (`WorkspaceSpec::with_image`). The page keeps that file in IndexedDB as a
//! snapshot (`src/lib/core/browser/`).
//!
//! The exports are strings and bytes, like the editor module's:
//!
//! - `open(bridge, image?, onTrap?, fetch?) → Promise<number>`: the commit
//!   counter after the open; `fetch` is the page's fetch bridge for the
//!   assistant's model calls (phase 6, `src/fetch.rs`), without which every
//!   `ai` call answers `NOT_SUPPORTED`;
//! - `call(body) → Promise<string>`: a `CoreResponse`;
//! - `stream(body, onEvent) → Promise<number>`: each `CoreEvent` to
//!   `onEvent`, then the count; `ai.chat` is a stream too, stopped with
//!   `db.cancel` and polled to its end so its reply is stored;
//! - `events(onEvent) → id`, `unsubscribe(id)`;
//! - `snapshot() → Uint8Array`, `commits() → number`;
//! - `ensureDemoConnection() → Promise<string>` (Decision 19).
//!
//! A refused call rejects with its `RpcError`'s JSON **text**; anything else
//! a call throws (a `WebAssembly.RuntimeError`) is a trap. A panic also
//! runs `onTrap` first, since a panic inside an async call leaves its
//! promise pending forever. Every write carries the `demo` origin.
//!
//! **No re-entrancy** (Decision 15): `onEvent` and `onTrap` run while the
//! module is mid-call, so they must only queue work (the transport hands
//! events on in a microtask).
//!
//! The dependencies are wasm32 only, so on native this crate is the log
//! formatter and nothing else.

pub mod log;

#[cfg(target_arch = "wasm32")]
mod fetch;

#[cfg(target_arch = "wasm32")]
mod module;

#[cfg(all(target_arch = "wasm32", feature = "test-hooks"))]
mod test_hooks;
