//! Decision 11b: the web server never opens SQLite or DuckDB.
//!
//! Their "connection string" is a path on the server, so a signed-in user
//! could open `auth.db` or another user's `meta.db`, and DuckDB's `read_*`,
//! `sqlite_scan` and `COPY TO` reach any file the server can. The server's
//! Core registers only `WEB_ENGINES`, so `db.connect` and `db.test` refuse
//! them (`rpc_db_policy.rs` sends them). This holds in a workspace test
//! build too, where Cargo's feature unification compiles both engines into
//! Core.

use seaquel_server::{AppState, WEB_ENGINES};

#[tokio::test]
async fn the_server_core_holds_only_the_web_engines() {
    let state = AppState::default();
    let ids = state.core.engine_ids();
    assert!(!ids.contains(&"sqlite"), "{ids:?}");
    assert!(!ids.contains(&"duckdb"), "{ids:?}");
    for id in &ids {
        assert!(WEB_ENGINES.contains(id), "{id} isn't a web engine");
    }
    // The server's Cargo features compile all three in, so they're offered.
    assert_eq!(ids, ["mssql", "mysql", "postgres"]);
}
