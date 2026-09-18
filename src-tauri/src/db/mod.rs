//! Tauri-facing DB module.
//!
//! All DB driver code lives in the `seaquel-db` crate so it can be shared with
//! the HTTP server. This module holds only the Tauri command wrappers.

pub use seaquel_db::ConnectionManager;

pub mod commands;
