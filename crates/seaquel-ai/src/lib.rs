//! Seaquel's AI domain (phase 6). This crate holds what the assistant, the
//! inline prompt and the MCP server decide, with no I/O of its own: the
//! network is an [`http::HttpClient`] someone else implements
//! (`seaquel-http`'s `NativeHttp`, or the demo's fetch bridge), and the
//! database and storage are Core's.
//!
//! - [`http`], [`sse`] and the providers' [`wire`] (Task 2);
//! - [`tools`]: the registry of tools for the assistant and the MCP server,
//!   their arguments, schemas and renderers (Task 3);
//! - [`prompt`]: the system prompt, the schema context, `@mentions` and the
//!   chat's history (Task 3);
//! - [`sharing`]: whether a connection shares its schema and data, and
//!   [`limits`].
//!
//! It builds for wasm32: no tokio, threads, clock or file system (the
//! `testing` feature's mock server aside, which is native-only and never in
//! a shipped build).

pub mod http;
pub mod limits;
pub mod prompt;
pub mod sharing;
pub mod sse;
pub mod tools;
pub mod wire;

pub use limits::AiLimits;
pub use sharing::{sharing, Sharing};

#[cfg(all(feature = "testing", not(target_arch = "wasm32")))]
pub mod testing;
