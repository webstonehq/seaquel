//! Tauri-facing DB module.
//!
//! All database work lives in `seaquel-core` so it's shared with the web
//! server and, later, the CLI, TUI and MCP server. This module holds only the
//! Tauri command wrappers.

pub mod commands;
