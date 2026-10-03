//! `ai::tools::call` on its own (phase 6 Task 6's review): it sets no
//! deadline of its own. MCP's `timed` is the call's only deadline (it
//! leaves a pending keychain read out, which a deadline here couldn't see),
//! and `ToolContext::timeout` is only the database's statement timeout.

#![cfg(all(feature = "ai", feature = "ai-native"))]
// Native-only tests: the shared helpers read the system clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod ai_support;
mod common;

use std::sync::Arc;
use std::time::Duration;

use ai_support::*;
use futures::FutureExt;
use seaquel_core::ai::tools::{self, Profile, ToolContext};
use serde_json::json;

#[tokio::test]
async fn a_call_outlasting_its_timeout_isnt_cut_by_core() {
    // A Core with an executor, as the CLI's has: a deadline in `call` would
    // fire on its clock.
    let w = world(Setup {
        conns: vec![Conn::new("conn-1", "sqlite")],
        ..Setup::default()
    })
    .await;
    let id = w.connect("conn-1").await;
    // The query takes longer than the call's timeout.
    *w.db.after_query.lock().unwrap() = Some(Arc::new(|_| {
        tokio::time::sleep(Duration::from_millis(200)).boxed()
    }));
    let ctx = ToolContext {
        profile: Profile::Mcp,
        connection_id: &id,
        connection_name: "Local",
        project_id: "p1",
        project_name: "Main",
        timeout: Duration::from_millis(20),
    };
    let call = tools::parse(
        Profile::Mcp,
        "run_query",
        &json!({ "connection": "Local", "sql": "SELECT 1" }),
    )
    .unwrap();
    let out = tools::call(&w.core, &w.ws, &ctx, &call).await;
    assert!(out.is_ok(), "{:?}", out.err().map(|e| e.to_string()));
    // The timeout still reaches the database.
    let ran = w.db.ran();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].timeout_ms, Some(20));
}
