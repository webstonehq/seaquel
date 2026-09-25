# Rust Core Phase 0 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Lay the groundwork for the Rust core. Split `seaquel-db` into an engine trait crate plus one crate per engine, add a minimal `seaquel-core` that owns connections and cancellation, generate the TypeScript wire types from Rust, make new Core code WASM-ready, and add CI. The app's behaviour doesn't change.

**Architecture:** Phase 0 of `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md`.

- New crates:
  - Pure, building for wasm32: `seaquel-macros`, `seaquel-runtime`, `seaquel-types`, `seaquel-engine`.
  - Native engines: `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}`, plus `seaquel-engine-testkit`.
  - `seaquel-core`: registers the engines, owns open connections, and keeps a `CancellationToken` per running stream.
- The Tauri app and `seaquel-server` become thin wrappers over `Core`. Their IPC and HTTP contracts stay byte-for-byte the same.
- `seaquel-db` stays compiling until the end, as a shim that re-exports `seaquel-types`. It's deleted in Task 16.

**Tech Stack:**
- Rust 2021: sqlx 0.8, tiberius 0.12, duckdb 1.1, tokio-util `CancellationToken`, async-trait, async-stream.
- ts-rs 12 for TypeScript generation.
- clippy `disallowed-types`/`disallowed-methods` for the WASM rules.
- GitHub Actions with service containers.
- SvelteKit 5 and vitest on the frontend.

---

## Ground rules for whoever executes this

- **No git writes.** The repo owner forbids `git add`, `git commit`, `git mv`, `git stash`, branches and worktrees. Use plain `mv`/`cp`/`rm`. Each task ends with a **Checkpoint**: stop, summarise what changed and the verification output, and let the user review and commit. Read-only git (`git status`, `git diff`) is fine.
- Run everything from the repo root: `/Users/m/projects/github/webstonehq/seaquel`.
- **Copy, don't move, from `seaquel-db`.** Engine code is copied out of `crates/seaquel-db/` and the old crate stays untouched (apart from Task 3's re-export) until Task 16 deletes it. That keeps the Tauri app and server compiling at every checkpoint.
- Never edit `src/lib/components/ui/*`.
- Error toasts use `errorToast` from `$lib/utils/toast`. This plan shouldn't touch toasts at all.
- Most of this code was compiled and tested in a scratch workspace while the plan was written: the macros, runtime, types, engine, sqlite engine, testkit, Core and the dependency checker. If something doesn't compile, suspect a typo in transcription before redesigning.

### Baseline (2026-09-24, before Task 1)

| Check | Result |
|---|---|
| `cargo test -p seaquel-db -p seaquel-server` | 25 passed |
| `cargo clippy -p seaquel-db -p seaquel-server --all-targets` | 14 warnings (13 `redundant_closure`, 1 `useless_conversion`) |
| `npx vitest run` | 185 passed |
| `npx oxlint --type-aware --type-check --deny-warnings` | clean |
| `npm run check` (svelte-check) | **19 errors** in 2 chart files. Task 1 fixes them |

### Crate map after Phase 0

```
crates/
  clippy.toml                  # WASM rules for everything under crates/
  seaquel-macros/              # proc macro: #[seaquel_runtime::async_trait]
  seaquel-runtime/             # MaybeSend/MaybeSync, BoxStream/BoxFuture, Executor
  seaquel-types/               # wire DTOs + ts-rs codegen
  seaquel-engine/              # Driver, Engine, EngineRegistry, impl_sqlx_driver!
  seaquel-engine-testkit/      # smoke suite every engine runs
  seaquel-engine-postgres/
  seaquel-engine-mysql/        # MySQL and MariaDB
  seaquel-engine-sqlite/
  seaquel-engine-mssql/
  seaquel-engine-duckdb/
  seaquel-core/                # Core: registry, connections, stream cancellation
  seaquel-server/              # web interface (existing)
  seaquel-server/clippy.toml   # opts the interface out of the WASM rules
src-tauri/                     # desktop interface (existing)
scripts/check-crate-deps.mjs   # dependency rules from the design doc
src/lib/types/generated/       # ts-rs output, committed
.cargo/config.toml             # TS_RS_EXPORT_DIR
.github/workflows/ci.yml
```

---

### Task 1: Clear the svelte-check baseline

CI will run `npm run check`. It fails today with 19 errors unrelated to this work, so fix them first.

**Files:**
- Modify: `src/lib/components/charts/query-chart.svelte` (11 errors, around lines 94–167)
- Modify: `src/lib/components/workflow/nodes/chart-node.svelte` (8 errors, around lines 194–243)

**Step 1: Reproduce**

Run: `npx svelte-check --tsconfig ./tsconfig.json --output machine 2>&1 | grep ERROR`
Expected: 19 errors of two kinds:
- `Type 'boolean' is not assignable to type 'Snippet<[{ context: ChartState<…> }]>'`
- `Property 'visibleSeries' | 'getBarsProps' | 'getSplineProps' | 'getAreaProps' does not exist on type '{ context: ChartState<…>; facet: Facet; }'`

**Step 2: Find the API change**

`package.json` asks for `layerchart@^2.0.0-next.44`, but 2.5.0 is installed. The chart components' snippet props changed shape between those versions. Read the installed types to see the new shape:

```bash
ls node_modules/layerchart/dist/components/charts/
grep -n "visibleSeries\|getBarsProps\|getSplineProps\|getAreaProps" -r node_modules/layerchart/dist --include=*.d.ts | head -20
```

Work out:
- where `visibleSeries` and the `get*Props` helpers live now (probably under `context`)
- what the boolean props on lines 94/115/137/146/167 and 194/217/234/243 have become (a snippet, or a differently named boolean)

Use @superpowers:systematic-debugging if the answer isn't obvious from the types.

**Step 3: Update both components**

Change the snippet destructuring and props to the 2.5.0 API. Keep the rendered output identical: same series, same bars/lines/areas, same legend/tooltip behaviour. Run the Svelte MCP `svelte-autofixer` on each file until it reports no issues.

**Step 4: Verify**

- Run: `npm run check`. Expected: `0 ERRORS`.
- Run: `npx vitest run`. Expected: 185 passed.
- Manual: run `npm run tauri:dev` and check two things:
  - Run a query and switch the result to a chart. Bar, line and area each render with a legend and tooltip.
  - Open a workflow with a chart node. It renders.

**Step 5: Checkpoint.** Suggested commit message: `Fix chart components for layerchart 2.5`.

---

### Task 2: Pin the wire format before moving anything

The TypeScript frontend depends on the exact JSON shape of the Rust types. Lock it with tests against the current `seaquel-db` crate, so later tasks can't change it by accident.

**Files:**
- Create: `crates/seaquel-db/tests/wire_format.rs`

**Step 1: Write the tests**

```rust
//! Pins the JSON shape of every wire type. The TypeScript frontend (wire.ts,
//! both providers) depends on these exact shapes; a failure here means the
//! frontend breaks too.

use seaquel_db::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch,
};
use serde_json::{from_value, json, to_value};

#[test]
fn query_result_is_columnar() {
    let r = QueryResult {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![json!(1), json!("x")]],
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "columns": ["a", "b"], "rows": [[1, "x"]] })
    );
}

#[test]
fn stream_batch_keeps_snake_case_fields() {
    let b = StreamBatch {
        columns: None,
        rows: vec![],
        is_final: true,
    };
    assert_eq!(
        to_value(&b).unwrap(),
        json!({ "columns": null, "rows": [], "is_final": true })
    );
}

#[test]
fn execute_result_shape() {
    let r = ExecuteResult {
        rows_affected: 3,
        last_insert_id: None,
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "rows_affected": 3, "last_insert_id": null })
    );
}

#[test]
fn connect_result_shape() {
    let r = ConnectResult {
        connection_id: "sqlite-1".into(),
    };
    assert_eq!(to_value(&r).unwrap(), json!({ "connection_id": "sqlite-1" }));
}

#[test]
fn db_error_shape_and_display() {
    let e = DbError::query_error("boom");
    assert_eq!(
        to_value(&e).unwrap(),
        json!({ "message": "Query failed: boom", "code": "QUERY_ERROR" })
    );
    assert_eq!(e.to_string(), "QUERY_ERROR: Query failed: boom");
}

#[test]
fn connect_config_accepts_every_field() {
    let c: ConnectConfig = from_value(json!({
        "driver": "mssql",
        "connection_string": "cs",
        "host": "db.local",
        "port": 1433,
        "database": "master",
        "username": "sa",
        "password": "pw",
        "encrypt": true,
        "trust_cert": false,
        "path": "/tmp/x.duckdb",
        "create_if_missing": true
    }))
    .unwrap();
    assert!(matches!(c.driver, DriverType::Mssql));
    assert_eq!(c.connection_string.as_deref(), Some("cs"));
    assert_eq!(c.host.as_deref(), Some("db.local"));
    assert_eq!(c.port, Some(1433));
    assert_eq!(c.database.as_deref(), Some("master"));
    assert_eq!(c.username.as_deref(), Some("sa"));
    assert_eq!(c.password.as_deref(), Some("pw"));
    assert_eq!(c.encrypt, Some(true));
    assert_eq!(c.trust_cert, Some(false));
    assert_eq!(c.path.as_deref(), Some("/tmp/x.duckdb"));
    assert_eq!(c.create_if_missing, Some(true));
}

#[test]
fn connect_config_optional_fields_default_to_none() {
    let c: ConnectConfig = from_value(json!({ "driver": "duckdb" })).unwrap();
    assert!(matches!(c.driver, DriverType::Duckdb));
    assert!(c.connection_string.is_none());
    assert!(c.path.is_none());
    assert!(c.create_if_missing.is_none());
}

#[test]
fn driver_type_is_lowercase_on_the_wire() {
    for wire in ["postgres", "mysql", "sqlite", "mssql", "duckdb"] {
        assert!(from_value::<DriverType>(json!(wire)).is_ok(), "{wire}");
    }
    assert!(from_value::<DriverType>(json!("Postgres")).is_err());
    assert!(from_value::<DriverType>(json!("mariadb")).is_err());
}

#[test]
fn batch_statement_params_default_to_empty() {
    let s: BatchStatement = from_value(json!({ "sql": "DELETE FROM t" })).unwrap();
    assert_eq!(s.sql, "DELETE FROM t");
    assert!(s.params.is_empty());
}
```

**Step 2: Run them. They should pass, because they describe current behaviour.**

Run: `cargo test -p seaquel-db --test wire_format`
Expected: `test result: ok. 9 passed`. If any test fails, the test is wrong: fix it to match what the code does today, not the other way round.

**Step 3: Checkpoint.** Suggested commit: `Pin the DB wire format with tests`.

---

### Task 3: Create `seaquel-types`

Move the wire DTOs into a pure crate. Add `StreamEvent`, which is currently duplicated in `src-tauri/src/db/commands.rs` and `crates/seaquel-server/src/routes/db/stream.rs`, plus `DriverType::as_str` and `DbError::engine_not_available`. `seaquel-db` re-exports the types, so nothing else changes yet.

**Files:**
- Create: `crates/seaquel-types/Cargo.toml`
- Create: `crates/seaquel-types/src/lib.rs`
- Move: `crates/seaquel-db/tests/wire_format.rs` → `crates/seaquel-types/tests/wire_format.rs`, then extend it
- Modify: `Cargo.toml` (workspace members and `ts-rs`)
- Modify: `crates/seaquel-db/Cargo.toml` and `crates/seaquel-db/src/lib.rs` (replace the DTO definitions with a re-export)

**Step 1: Workspace manifest**

In the root `Cargo.toml`:
- Add `"crates/seaquel-types",` to `members`, before `"crates/seaquel-db"`.
- Add to `[workspace.dependencies]`:

```toml
ts-rs = "12"
```

**Step 2: Crate manifest** — `crates/seaquel-types/Cargo.toml`:

```toml
[package]
name = "seaquel-types"
version = "0.1.0"
edition = "2021"
description = "Wire types shared by every Seaquel interface. Build with --features ts to regenerate the TypeScript bindings."

[features]
# Generates src/lib/types/generated/*.ts when running `cargo test --features ts`.
# Run it through `npm run types:gen`.
ts = ["dep:ts-rs"]

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
ts-rs = { workspace = true, optional = true }
```

**Step 3: Move the test and add the new behaviour (red)**

```bash
mkdir -p crates/seaquel-types/tests
mv crates/seaquel-db/tests/wire_format.rs crates/seaquel-types/tests/wire_format.rs
rmdir crates/seaquel-db/tests
```

Replace the whole file with the final version below. The changes from Task 2:
- imports `seaquel_types` (with `StreamEvent`)
- uses `assert_eq!` on `DriverType`
- checks `as_str`
- adds three new tests

```rust
//! Pins the JSON shape of every wire type. The TypeScript frontend (wire.ts,
//! both providers) depends on these exact shapes; a failure here means the
//! frontend breaks too.

use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch, StreamEvent,
};
use serde_json::{from_value, json, to_value};

#[test]
fn query_result_is_columnar() {
    let r = QueryResult {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![json!(1), json!("x")]],
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "columns": ["a", "b"], "rows": [[1, "x"]] })
    );
}

#[test]
fn stream_batch_keeps_snake_case_fields() {
    let b = StreamBatch {
        columns: None,
        rows: vec![],
        is_final: true,
    };
    assert_eq!(
        to_value(&b).unwrap(),
        json!({ "columns": null, "rows": [], "is_final": true })
    );
}

#[test]
fn execute_result_shape() {
    let r = ExecuteResult {
        rows_affected: 3,
        last_insert_id: None,
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "rows_affected": 3, "last_insert_id": null })
    );
}

#[test]
fn connect_result_shape() {
    let r = ConnectResult {
        connection_id: "sqlite-1".into(),
    };
    assert_eq!(to_value(&r).unwrap(), json!({ "connection_id": "sqlite-1" }));
}

#[test]
fn db_error_shape_and_display() {
    let e = DbError::query_error("boom");
    assert_eq!(
        to_value(&e).unwrap(),
        json!({ "message": "Query failed: boom", "code": "QUERY_ERROR" })
    );
    assert_eq!(e.to_string(), "QUERY_ERROR: Query failed: boom");
}

#[test]
fn connect_config_accepts_every_field() {
    let c: ConnectConfig = from_value(json!({
        "driver": "mssql",
        "connection_string": "cs",
        "host": "db.local",
        "port": 1433,
        "database": "master",
        "username": "sa",
        "password": "pw",
        "encrypt": true,
        "trust_cert": false,
        "path": "/tmp/x.duckdb",
        "create_if_missing": true
    }))
    .unwrap();
    assert_eq!(c.driver, DriverType::Mssql);
    assert_eq!(c.connection_string.as_deref(), Some("cs"));
    assert_eq!(c.host.as_deref(), Some("db.local"));
    assert_eq!(c.port, Some(1433));
    assert_eq!(c.database.as_deref(), Some("master"));
    assert_eq!(c.username.as_deref(), Some("sa"));
    assert_eq!(c.password.as_deref(), Some("pw"));
    assert_eq!(c.encrypt, Some(true));
    assert_eq!(c.trust_cert, Some(false));
    assert_eq!(c.path.as_deref(), Some("/tmp/x.duckdb"));
    assert_eq!(c.create_if_missing, Some(true));
}

#[test]
fn connect_config_optional_fields_default_to_none() {
    let c: ConnectConfig = from_value(json!({ "driver": "duckdb" })).unwrap();
    assert_eq!(c.driver, DriverType::Duckdb);
    assert!(c.connection_string.is_none());
    assert!(c.path.is_none());
    assert!(c.create_if_missing.is_none());
}

#[test]
fn driver_type_is_lowercase_on_the_wire() {
    for (wire, expected) in [
        ("postgres", DriverType::Postgres),
        ("mysql", DriverType::Mysql),
        ("sqlite", DriverType::Sqlite),
        ("mssql", DriverType::Mssql),
        ("duckdb", DriverType::Duckdb),
    ] {
        assert_eq!(from_value::<DriverType>(json!(wire)).unwrap(), expected);
        assert_eq!(expected.as_str(), wire);
    }
    assert!(from_value::<DriverType>(json!("Postgres")).is_err());
    assert!(from_value::<DriverType>(json!("mariadb")).is_err());
}

#[test]
fn batch_statement_params_default_to_empty() {
    let s: BatchStatement = from_value(json!({ "sql": "DELETE FROM t" })).unwrap();
    assert_eq!(s.sql, "DELETE FROM t");
    assert!(s.params.is_empty());
}

#[test]
fn stream_event_batch_is_flattened() {
    let ev = StreamEvent::Batch(StreamBatch {
        columns: Some(vec!["n".into()]),
        rows: vec![vec![json!(1)]],
        is_final: false,
    });
    assert_eq!(
        to_value(&ev).unwrap(),
        json!({ "type": "batch", "columns": ["n"], "rows": [[1]], "is_final": false })
    );
}

#[test]
fn stream_event_done_and_error() {
    assert_eq!(to_value(StreamEvent::Done).unwrap(), json!({ "type": "done" }));
    assert_eq!(
        to_value(StreamEvent::from(DbError::connection_not_found("x"))).unwrap(),
        json!({
            "type": "error",
            "message": "Connection not found: x",
            "code": "CONNECTION_NOT_FOUND"
        })
    );
}

#[test]
fn engine_not_available_error() {
    let e = DbError::engine_not_available("mssql");
    assert_eq!(e.code, "ENGINE_NOT_AVAILABLE");
    assert!(e.message.contains("\"mssql\""), "{}", e.message);
}
```

Create `crates/seaquel-types/src/lib.rs` containing only `//! placeholder` for now.

Run: `cargo test -p seaquel-types --test wire_format`
Expected: compile errors (`unresolved imports`). That's the red step.

**Step 4: Implement** — `crates/seaquel-types/src/lib.rs`:

```rust
//! Wire types shared by every Seaquel interface.
//!
//! These cross process boundaries (Tauri IPC, HTTP, WebSocket), so their serde
//! shape is a contract with the TypeScript frontend. `tests/wire_format.rs`
//! pins the JSON, and `npm run types:gen` regenerates
//! `src/lib/types/generated/` from these definitions.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`.

use serde::{Deserialize, Serialize};

/// Columnar result format for all drivers
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryResult {
    pub columns: Vec<String>,
    #[cfg_attr(feature = "ts", ts(type = "unknown[][]"))]
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// A batch of rows emitted by a streaming query.
/// `columns` is Some on the first batch (so the frontend can render headers)
/// and None on subsequent batches. `is_final` marks the terminal batch, which
/// may carry zero rows.
#[derive(Debug, Serialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StreamBatch {
    pub columns: Option<Vec<String>>,
    #[cfg_attr(feature = "ts", ts(type = "unknown[][]"))]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub is_final: bool,
}

/// Result of a write operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ExecuteResult {
    // ts-rs maps u64/i64 to `bigint`, but serde_json sends plain JSON numbers.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rows_affected: u64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_insert_id: Option<i64>,
}

/// Result of a connect operation
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectResult {
    pub connection_id: String,
}

/// Unified error type for all drivers
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DbError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DbError {}

impl DbError {
    pub fn connection_not_found(id: &str) -> Self {
        Self {
            message: format!("Connection not found: {}", id),
            code: "CONNECTION_NOT_FOUND".to_string(),
        }
    }

    pub fn connection_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Failed to connect: {}", msg),
            code: "CONNECTION_ERROR".to_string(),
        }
    }

    pub fn query_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Query failed: {}", msg),
            code: "QUERY_ERROR".to_string(),
        }
    }

    pub fn execute_error(msg: impl std::fmt::Display) -> Self {
        Self {
            message: format!("Execute failed: {}", msg),
            code: "EXECUTE_ERROR".to_string(),
        }
    }

    pub fn result_too_large(cap: usize) -> Self {
        Self {
            message: format!(
                "Result exceeds the {cap}-row cap for non-streaming queries. Use query_stream for large results, or add LIMIT {cap} to the query."
            ),
            code: "RESULT_TOO_LARGE".to_string(),
        }
    }

    /// The build doesn't include an engine for this driver (e.g. a slim CLI
    /// built without the `engine-mssql` feature).
    pub fn engine_not_available(driver: &str) -> Self {
        Self {
            message: format!("Database engine \"{}\" is not available in this build", driver),
            code: "ENGINE_NOT_AVAILABLE".to_string(),
        }
    }
}

/// Driver type discriminant
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DriverType {
    Postgres,
    Mysql,
    Sqlite,
    Mssql,
    Duckdb,
}

impl DriverType {
    /// The wire name, which is also the id of the engine that handles it.
    pub fn as_str(self) -> &'static str {
        match self {
            DriverType::Postgres => "postgres",
            DriverType::Mysql => "mysql",
            DriverType::Sqlite => "sqlite",
            DriverType::Mssql => "mssql",
            DriverType::Duckdb => "duckdb",
        }
    }
}

/// Connection configuration — superset of all driver needs
#[derive(Debug, Deserialize, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ConnectConfig {
    pub driver: DriverType,
    /// Connection string for sqlx-based drivers (postgres, mysql, sqlite)
    pub connection_string: Option<String>,
    /// Individual fields for MSSQL
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub encrypt: Option<bool>,
    pub trust_cert: Option<bool>,
    /// File path for DuckDB
    pub path: Option<String>,
    /// SQLite only: create the database file (and its directory) if it doesn't
    /// exist. Off by default so a mistyped path fails instead of silently
    /// opening a new, empty database.
    pub create_if_missing: Option<bool>,
}

/// A single statement in a batch/transaction
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BatchStatement {
    pub sql: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
    pub params: Vec<serde_json::Value>,
}

/// Events delivered to a client for one streaming query, over a Tauri channel
/// or a WebSocket. A stream is zero or more `Batch` events followed by exactly
/// one `Done` or `Error` — or nothing more at all if the client cancelled.
///
/// `Batch` flattens the `StreamBatch` fields onto the event:
/// `{"type":"batch","columns":…,"rows":…,"is_final":…}`.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum StreamEvent {
    Batch(StreamBatch),
    Done,
    Error { message: String, code: String },
}

impl From<DbError> for StreamEvent {
    fn from(err: DbError) -> Self {
        StreamEvent::Error {
            message: err.message,
            code: err.code,
        }
    }
}
```

**Step 5: Run the tests**

Run: `cargo test -p seaquel-types`
Expected: `test result: ok. 12 passed` for `wire_format`.

**Step 6: Point `seaquel-db` at the new types**

- `crates/seaquel-db/Cargo.toml`: add `seaquel-types = { path = "../seaquel-types" }` to `[dependencies]`.
- `crates/seaquel-db/src/lib.rs`:
  - Delete the definitions of `QueryResult`, `StreamBatch`, `ExecuteResult`, `ConnectResult`, `DbError` (including its `Display`, `Error` and constructor `impl`s), `DriverType`, `ConnectConfig` and `BatchStatement`.
  - Keep `max_query_rows`, the `Driver` trait, the macro, `ConnectionManager` and `open`.
  - Add this near the top:

```rust
pub use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch,
};
```

Remove any `use serde::{Deserialize, Serialize};` that becomes unused.

**Step 7: Verify nothing else noticed**

Run: `cargo test -p seaquel-db -p seaquel-server -p seaquel-types && cargo check -p seaquel`
Expected: all tests pass (seaquel-db 7, server 18, types 12), and the Tauri crate checks cleanly. If `cargo check -p seaquel` complains that `../build` is missing, run `mkdir -p build` first.

**Step 8: Pure-crate check**

```bash
rustup target add wasm32-unknown-unknown
cargo check --target wasm32-unknown-unknown -p seaquel-types
```
Expected: `Finished`.

**Step 9: Checkpoint.** Suggested commit: `Move wire types into seaquel-types`.

---

### Task 4: Generate the TypeScript wire types

**Files:**
- Create: `.cargo/config.toml`
- Create (generated): `src/lib/types/generated/*.ts`
- Modify: `package.json` (scripts)
- Modify: `.oxlintrc.json`, `.oxfmtrc.json` (ignore generated files)
- Modify: `src/lib/providers/wire.ts`

**Step 1: Tell ts-rs where to write** — `.cargo/config.toml`:

```toml
# ts-rs writes TypeScript bindings here when seaquel-types is tested with
# `--features ts` (see `npm run types:gen`). `relative` resolves against the
# repo root, the directory that contains `.cargo/`.
[env]
TS_RS_EXPORT_DIR = { value = "src/lib/types/generated", relative = true }
```

**Step 2: npm scripts.** In `package.json` `scripts`:
- Add `"types:gen": "cargo test -p seaquel-types --features ts --lib export_bindings --quiet",`
- Change `check` to `"npm run types:gen && svelte-kit sync && svelte-check --tsconfig ./tsconfig.json"`

**Step 3: Generate**

Run: `npm run types:gen && ls src/lib/types/generated`
Expected: 9 tests pass, and these files appear: `BatchStatement.ts ConnectConfig.ts ConnectResult.ts DbError.ts DriverType.ts ExecuteResult.ts QueryResult.ts StreamBatch.ts StreamEvent.ts`.

Spot-check two of them:

```
StreamEvent.ts:   export type StreamEvent = { "type": "batch" } & StreamBatch | { "type": "done" } | { "type": "error", message: string, code: string, };
ExecuteResult.ts: export type ExecuteResult = { rows_affected: number, last_insert_id: number | null, };
```

**Step 4: Keep linters off generated code.** Add `"src/lib/types/generated"` to `ignorePatterns` in both `.oxlintrc.json` and `.oxfmtrc.json`.

**Step 5: Use the generated types in `wire.ts`.** In `src/lib/providers/wire.ts`, replace the block from `import type { ConnectionConfig } from "./types";` through the end of the `DbStreamEvent` type with:

```ts
import type { ConnectConfig } from "$lib/types/generated/ConnectConfig";
import type { DbError } from "$lib/types/generated/DbError";
import type { ConnectionConfig } from "./types";

// -------- Wire types --------
// Generated from the Rust `seaquel-types` crate by `npm run types:gen`. Change
// the Rust side, then regenerate; don't edit these by hand.

export type { DbError };
export type { ConnectResult as DbConnectResult } from "$lib/types/generated/ConnectResult";
export type { QueryResult as DbQueryResult } from "$lib/types/generated/QueryResult";
export type { ExecuteResult as DbExecuteResult } from "$lib/types/generated/ExecuteResult";
/**
 * Internally tagged stream event. For `batch`, the `StreamBatch` fields are
 * flattened onto the event: `{type:"batch", columns, rows, is_final}`.
 */
export type { StreamEvent as DbStreamEvent } from "$lib/types/generated/StreamEvent";
```

Also update the file's header comment to say the wire shapes come from `seaquel-types`, and change `toRustConfig`'s return type from `Record<string, unknown>` to `ConnectConfig`.

**Step 6: Verify**

- Run: `npm run check`. Expected: `0 ERRORS`. If `toRustConfig` fails to type-check, it's catching a real mismatch between the frontend and Rust. Fix the TS to match `ConnectConfig`, not the other way round.
- Run: `npx vitest run`. Expected: 185 passed.
- Run: `npx oxlint --type-aware --type-check --deny-warnings`. Expected: clean.

**Step 7: Checkpoint.** Suggested commit: `Generate TypeScript wire types from seaquel-types`.

---

### Task 5: `seaquel-macros`, `seaquel-runtime` and the WASM lint rules

**Files:**
- Create: `crates/seaquel-macros/{Cargo.toml,src/lib.rs}`
- Create: `crates/seaquel-runtime/{Cargo.toml,src/lib.rs}`
- Create: `crates/clippy.toml`
- Modify: `Cargo.toml` (members)

**Step 1: Workspace members.** Add `"crates/seaquel-macros",` and `"crates/seaquel-runtime",` at the top of `members`.

**Step 2: `crates/seaquel-macros/Cargo.toml`**

```toml
[package]
name = "seaquel-macros"
version = "0.1.0"
edition = "2021"
description = "Proc macros for Seaquel crates. Use them through seaquel-runtime, not directly."

[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote = "1"
```

**Step 3: `crates/seaquel-macros/src/lib.rs`**

```rust
//! Proc macros re-exported by `seaquel-runtime`. Depend on `seaquel-runtime`
//! and write `#[seaquel_runtime::async_trait]`; don't use this crate directly.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// `async_trait` that drops the `Send` bound on wasm32.
///
/// Futures backed by JavaScript aren't `Send`, so the browser build of Core
/// needs `#[async_trait(?Send)]` while native builds need the default. Writing
/// both `cfg_attr` lines by hand on every trait and impl is easy to get wrong,
/// so every Seaquel async trait uses this attribute instead.
#[proc_macro_attribute]
pub fn async_trait(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return quote! {
            compile_error!("#[seaquel_runtime::async_trait] takes no arguments");
        }
        .into();
    }
    let item = TokenStream2::from(item);
    quote! {
        #[cfg_attr(
            not(target_arch = "wasm32"),
            ::seaquel_runtime::__private::async_trait::async_trait
        )]
        #[cfg_attr(
            target_arch = "wasm32",
            ::seaquel_runtime::__private::async_trait::async_trait(?Send)
        )]
        #item
    }
    .into()
}
```

**Step 4: `crates/seaquel-runtime/Cargo.toml`**

```toml
[package]
name = "seaquel-runtime"
version = "0.1.0"
edition = "2021"
description = "Platform seams for Seaquel Core: Send bounds, boxed streams and futures, and the executor. Compiles for native and wasm32."

[features]
# Native executor backed by tokio. Interfaces enable it; pure crates don't.
tokio = ["dep:tokio"]

[dependencies]
seaquel-macros = { path = "../seaquel-macros" }
async-trait = { workspace = true }
futures = { workspace = true }
# Not the workspace tokio: that one enables "net", which doesn't build for wasm32.
tokio = { version = "1", default-features = false, features = ["rt", "time"], optional = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen-futures = "0.4"
gloo-timers = { version = "0.3", features = ["futures"] }
js-sys = "0.3"

[dev-dependencies]
tokio = { version = "1", features = ["rt", "time", "macros", "sync"] }
```

**Step 5: Write the tests first.** Put this at the bottom of `crates/seaquel-runtime/src/lib.rs`, with nothing else in the file yet:

```rust
#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tokio_executor_spawns_and_sleeps() {
        let exec = TokioExecutor;
        let (tx, rx) = tokio::sync::oneshot::channel();
        exec.spawn(Box::pin(async move {
            let _ = tx.send(42);
        }));
        exec.sleep(Duration::from_millis(1)).await;
        assert_eq!(rx.await.unwrap(), 42);
    }

    #[test]
    fn tokio_executor_reports_time_after_2020() {
        // 2020-01-01T00:00:00Z
        assert!(TokioExecutor.unix_time() > Duration::from_secs(1_577_836_800));
    }

    #[seaquel_runtime::async_trait]
    trait Greeter: MaybeSend + MaybeSync {
        async fn greet(&self) -> &'static str;
    }

    struct Hello;

    #[seaquel_runtime::async_trait]
    impl Greeter for Hello {
        async fn greet(&self) -> &'static str {
            "hello"
        }
    }

    #[tokio::test]
    async fn async_trait_wrapper_produces_send_trait_objects_on_native() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Greeter>();
        let g: Box<dyn Greeter> = Box::new(Hello);
        assert_eq!(g.greet().await, "hello");
    }
}
```

Run: `cargo test -p seaquel-runtime --features tokio`
Expected: compile errors (`TokioExecutor`, `MaybeSend`, … not found).

**Step 6: Implement.** Put this above the tests module:

```rust
//! Platform seams for Seaquel Core.
//!
//! Core has to build for native targets and for `wasm32-unknown-unknown` (the
//! browser demo). The differences live here, so the rest of Core never writes
//! `cfg(target_arch = "wasm32")`:
//!
//! - [`MaybeSend`] / [`MaybeSync`]: `Send`/`Sync` on native, no bound on wasm32.
//! - [`BoxStream`] / [`BoxFuture`]: `Send` boxes on native, local boxes on wasm32.
//! - [`async_trait`]: `async_trait` on native, `async_trait(?Send)` on wasm32.
//! - [`Executor`]: spawning, sleeping and wall-clock time. Core must not call
//!   `tokio::spawn`, `std::time::Instant` or `SystemTime` directly; clippy
//!   enforces that through `crates/clippy.toml`.

// Lets `#[seaquel_runtime::async_trait]` expand to `::seaquel_runtime::…` paths
// inside this crate too.
extern crate self as seaquel_runtime;

use std::time::Duration;

pub use seaquel_macros::async_trait;

#[doc(hidden)]
pub mod __private {
    pub use async_trait;
}

#[cfg(not(target_arch = "wasm32"))]
mod bounds {
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}

    pub trait MaybeSync: Sync {}
    impl<T: Sync + ?Sized> MaybeSync for T {}

    pub type BoxStream<'a, T> = futures::stream::BoxStream<'a, T>;
    pub type BoxFuture<'a, T> = futures::future::BoxFuture<'a, T>;
}

#[cfg(target_arch = "wasm32")]
mod bounds {
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}

    pub trait MaybeSync {}
    impl<T: ?Sized> MaybeSync for T {}

    pub type BoxStream<'a, T> = futures::stream::LocalBoxStream<'a, T>;
    pub type BoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;
}

pub use bounds::{BoxFuture, BoxStream, MaybeSend, MaybeSync};

/// Everything Core needs from the async runtime. Interfaces pass one in:
/// [`TokioExecutor`] on native, [`WasmExecutor`] in the browser.
pub trait Executor: MaybeSend + MaybeSync {
    /// Run `future` in the background. It must not outlive the process.
    fn spawn(&self, future: BoxFuture<'static, ()>);

    /// Resolve after `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;

    /// Wall-clock time since the Unix epoch.
    fn unix_time(&self) -> Duration;
}

#[cfg(feature = "tokio")]
mod tokio_executor {
    use super::{BoxFuture, Duration, Executor};

    /// Native executor. Must be used from inside a tokio runtime.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct TokioExecutor;

    impl Executor for TokioExecutor {
        // The one sanctioned `tokio::spawn` in Core's crates.
        #[allow(clippy::disallowed_methods)]
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            tokio::spawn(future);
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
            Box::pin(tokio::time::sleep(duration))
        }

        // The one sanctioned `SystemTime` in Core's crates.
        #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
        fn unix_time(&self) -> Duration {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
        }
    }
}

#[cfg(feature = "tokio")]
pub use tokio_executor::TokioExecutor;

#[cfg(target_arch = "wasm32")]
mod wasm_executor {
    use super::{BoxFuture, Duration, Executor};

    /// Browser executor: the page's microtask queue and `setTimeout`.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct WasmExecutor;

    impl Executor for WasmExecutor {
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            wasm_bindgen_futures::spawn_local(future);
        }

        fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
            Box::pin(gloo_timers::future::sleep(duration))
        }

        fn unix_time(&self) -> Duration {
            Duration::from_secs_f64(js_sys::Date::now() / 1000.0)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_executor::WasmExecutor;
```

**Step 7: Run**

Run: `cargo test -p seaquel-runtime --features tokio`
Expected: `test result: ok. 3 passed`.

**Step 8: The lint rules** — `crates/clippy.toml`:

```toml
# Applies to every crate under crates/ (clippy uses the nearest clippy.toml
# above a crate). Core crates must also build for wasm32 and run on the
# `Executor` from seaquel-runtime, so they may not reach for the platform
# directly. Interface crates that legitimately do (seaquel-server) carry their
# own clippy.toml to opt out.
disallowed-types = [
    { path = "std::time::Instant", reason = "panics on wasm32; take time from seaquel_runtime::Executor" },
    { path = "std::time::SystemTime", reason = "panics on wasm32; use seaquel_runtime::Executor::unix_time" },
]
# `allow-invalid`: crates without tokio in their graph (the pure ones) would
# otherwise warn that the path is unreachable.
disallowed-methods = [
    { path = "tokio::spawn", allow-invalid = true, reason = "use seaquel_runtime::Executor::spawn" },
    { path = "tokio::task::spawn", allow-invalid = true, reason = "use seaquel_runtime::Executor::spawn" },
    { path = "tokio::task::spawn_local", allow-invalid = true, reason = "use seaquel_runtime::Executor::spawn" },
    { path = "std::thread::spawn", reason = "not available on wasm32; use seaquel_runtime::Executor::spawn" },
    { path = "std::time::SystemTime::now", reason = "panics on wasm32; use seaquel_runtime::Executor::unix_time" },
]
```

`seaquel-server` lives under `crates/` and uses `tokio::spawn` in its tests. Give it an opt-out now, so clippy stays green. Create `crates/seaquel-server/clippy.toml`:

```toml
# seaquel-server is an interface, not Core: it owns its tokio runtime and may
# spawn tasks and read the clock. This file shadows crates/clippy.toml.
```

**Step 9: Prove the rule fires, then remove the canary**

```bash
echo 'pub fn _canary() { let _ = std::time::Instant::now(); }' >> crates/seaquel-runtime/src/lib.rs
cargo clippy -p seaquel-runtime -- -D warnings 2>&1 | grep "disallowed type"
```
Expected: `error: use of a disallowed type 'std::time::Instant'`. Now delete that last line from `crates/seaquel-runtime/src/lib.rs`.

**Step 10: Clean on both targets**

```bash
cargo clippy -p seaquel-macros -p seaquel-runtime --all-targets --features seaquel-runtime/tokio -- -D warnings
cargo clippy --target wasm32-unknown-unknown -p seaquel-runtime -p seaquel-types -- -D warnings
```
Expected: both `Finished` with no warnings.

**Step 11: Checkpoint.** Suggested commit: `Add seaquel-runtime and the wasm lint rules`.

---

### Task 6: `seaquel-engine`: Driver, Engine, EngineRegistry, `impl_sqlx_driver!`

The `Driver` trait moves here and gains a `cancel: CancellationToken` parameter on `query_stream`. `Engine` and `EngineRegistry` are new: they're how Core finds an engine without naming its crate.

**Files:**
- Create: `crates/seaquel-engine/{Cargo.toml,src/lib.rs,src/sqlx_driver.rs}`
- Modify: `Cargo.toml` (members)

**Step 1:** Add `"crates/seaquel-engine",` to `members` after `seaquel-types`.

**Step 2: `crates/seaquel-engine/Cargo.toml`**

```toml
[package]
name = "seaquel-engine"
version = "0.1.0"
edition = "2021"
description = "The Seaquel engine plugin boundary: Driver and Engine traits plus the engine registry. Pure; builds for wasm32."

[dependencies]
seaquel-runtime = { path = "../seaquel-runtime" }
seaquel-types = { path = "../seaquel-types" }
async-stream = { workspace = true }
async-trait = { workspace = true }
futures = { workspace = true }
serde_json = { workspace = true }
# Not the workspace tokio-util: that one enables "compat", which pulls tokio "net".
tokio-util = { version = "0.7", default-features = false }
```

**Step 3: Tests first.** Create `crates/seaquel-engine/src/lib.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use futures::StreamExt;
    use serde_json::json;

    struct FakeDriver;

    #[seaquel_runtime::async_trait]
    impl Driver for FakeDriver {
        async fn query(
            &self,
            _sql: &str,
            _params: Vec<serde_json::Value>,
        ) -> Result<QueryResult, DbError> {
            Ok(QueryResult {
                columns: vec!["a".into()],
                rows: vec![vec![json!(1)]],
            })
        }

        async fn execute(
            &self,
            _sql: &str,
            _params: Vec<serde_json::Value>,
        ) -> Result<ExecuteResult, DbError> {
            Ok(ExecuteResult {
                rows_affected: 0,
                last_insert_id: None,
            })
        }

        async fn close(&self) -> Result<(), DbError> {
            Ok(())
        }
    }

    struct FakeEngine(&'static str);

    #[seaquel_runtime::async_trait]
    impl Engine for FakeEngine {
        fn id(&self) -> &'static str {
            self.0
        }

        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(FakeDriver))
        }
    }

    fn config(driver: &str) -> ConnectConfig {
        serde_json::from_value(json!({ "driver": driver })).unwrap()
    }

    #[test]
    fn open_dispatches_on_the_driver_field() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        let driver = block_on(registry.open(&config("sqlite"))).unwrap();
        let result = block_on(driver.query("SELECT 1", vec![])).unwrap();
        assert_eq!(result.columns, vec!["a"]);
    }

    #[test]
    fn open_reports_a_missing_engine() {
        let registry = EngineRegistry::new();
        let err = block_on(registry.open(&config("postgres"))).err().unwrap();
        assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
        assert!(err.message.contains("\"postgres\""), "{}", err.message);
    }

    #[test]
    fn ids_are_sorted() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("duckdb")));
        assert_eq!(registry.ids(), vec!["duckdb", "sqlite"]);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn registering_an_id_twice_panics() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("sqlite")));
    }

    #[test]
    fn default_query_stream_emits_one_final_batch() {
        let driver = FakeDriver;
        let batches: Vec<_> = block_on(
            driver
                .query_stream("SELECT 1".into(), vec![], CancellationToken::new())
                .collect(),
        );
        assert_eq!(batches.len(), 1);
        let batch = batches.into_iter().next().unwrap().unwrap();
        assert_eq!(batch.columns, Some(vec!["a".to_string()]));
        assert_eq!(batch.rows, vec![vec![json!(1)]]);
        assert!(batch.is_final);
    }
}
```

Run: `cargo test -p seaquel-engine`
Expected: compile errors (`Driver`, `Engine`, … not found).

**Step 4: Implement.** Put this above the tests in `crates/seaquel-engine/src/lib.rs`:

```rust
//! The engine plugin boundary.
//!
//! Each database engine is its own crate (`seaquel-engine-postgres`, …) that
//! implements [`Engine`] and [`Driver`]. Seaquel Core looks engines up in an
//! [`EngineRegistry`] by the `driver` field of [`ConnectConfig`], so nothing
//! outside Core's default-plugin list names an engine crate.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`. The
//! [`impl_sqlx_driver!`] macro is only expanded by native engine crates.

use std::collections::HashMap;
use std::sync::Arc;

pub use seaquel_runtime::{BoxStream, MaybeSend, MaybeSync};
pub use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch,
};
pub use tokio_util::sync::CancellationToken;

mod sqlx_driver;

#[doc(hidden)]
pub mod __private {
    pub use async_stream;
    pub use async_trait;
    pub use futures;
}

/// Cap for non-streaming `query()` results. Streaming `query_stream` is
/// unbounded by design (the UI paginates). This cap exists to prevent a
/// `SELECT * FROM big_table` submitted through the non-streaming path from
/// OOM-killing the process. Override with `SEAQUEL_MAX_QUERY_ROWS`.
pub fn max_query_rows() -> usize {
    use std::sync::OnceLock;
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("SEAQUEL_MAX_QUERY_ROWS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|v: &usize| *v > 0)
            .unwrap_or(100_000)
    })
}

/// An open connection (or pool) to one database.
#[seaquel_runtime::async_trait]
pub trait Driver: MaybeSend + MaybeSync {
    async fn query(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError>;

    async fn execute(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError>;

    /// Execute multiple statements in a single transaction on one connection.
    ///
    /// Drivers **must** override this to provide real atomicity. The default
    /// returns an error rather than silently running each statement on its
    /// own pooled connection — that was the old behaviour and it let a
    /// mid-batch failure leave earlier writes committed. Loudly failing here
    /// surfaces the gap instead of producing half-written data.
    async fn transaction(&self, _statements: Vec<BatchStatement>) -> Result<(), DbError> {
        Err(DbError {
            message: "transactions are not supported by this driver".to_string(),
            code: "TRANSACTION_NOT_SUPPORTED".to_string(),
        })
    }

    /// Stream query results in batches.
    ///
    /// Drivers that genuinely stream (the sqlx-based ones) override this and
    /// stop fetching as soon as `cancel` fires. The default runs `query()` and
    /// emits a single terminal batch, so DuckDB and MSSQL get a
    /// correct-but-non-streaming path for free; it can't interrupt `query()`,
    /// so it ignores `cancel` and Core drops the result instead.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<serde_json::Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        let _ = cancel;
        Box::pin(async_stream::try_stream! {
            let result = self.query(&sql, params).await?;
            yield StreamBatch {
                columns: Some(result.columns),
                rows: result.rows,
                is_final: true,
            };
        })
    }

    async fn close(&self) -> Result<(), DbError>;
}

/// A database engine plugin: knows how to open [`Driver`]s for one `driver`
/// value on the wire.
#[seaquel_runtime::async_trait]
pub trait Engine: MaybeSend + MaybeSync {
    /// Stable id. Must equal [`DriverType::as_str`] for the driver it serves.
    fn id(&self) -> &'static str;

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError>;
}

/// The engines compiled into this build, keyed by id.
#[derive(Default, Clone)]
pub struct EngineRegistry {
    engines: HashMap<&'static str, Arc<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an engine.
    ///
    /// # Panics
    ///
    /// If an engine with the same id is already registered. Two engines
    /// claiming one id is a build mistake, not a runtime condition.
    pub fn register(&mut self, engine: Arc<dyn Engine>) {
        let id = engine.id();
        if self.engines.insert(id, engine).is_some() {
            panic!("engine \"{id}\" registered twice");
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Engine>> {
        self.engines.get(id).cloned()
    }

    /// Registered ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<_> = self.engines.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Open a driver with the engine matching `config.driver`.
    pub async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let id = config.driver.as_str();
        let engine = self
            .get(id)
            .ok_or_else(|| DbError::engine_not_available(id))?;
        engine.open(config).await
    }
}
```

**Step 5: The shared sqlx macro** — `crates/seaquel-engine/src/sqlx_driver.rs`.

This is the `impl_sqlx_driver!` macro from `crates/seaquel-db/src/lib.rs` with four changes:
- `#[macro_export]`
- every Seaquel path goes through `$crate`
- redundant closures are removed
- `query_stream` takes `cancel` and wraps the fetch in `take_until`

```rust
//! The `Driver` implementation shared by the sqlx-based engines.

/// Generates the `Driver` impl shared by the sqlx-based engines (Postgres,
/// MySQL, SQLite). They differ only in the sqlx database type, the arguments
/// type, the decode function, and how `last_insert_id` is read from the
/// execute result.
///
/// Native only. Expand it in a module that defines `$driver_name` with a
/// `pool: sqlx::Pool<$db>` field. The calling crate must depend on `sqlx` and
/// `serde_json`: those paths in the expansion resolve in the caller. Everything
/// else goes through `$crate`, so callers don't need async-trait, async-stream
/// or futures.
#[macro_export]
macro_rules! impl_sqlx_driver {
    (
        $driver_name:ident,
        $db:ty,
        $args:ty,
        decode_fn = $decode_fn:path,
        last_insert_id = $last_insert_id:expr
    ) => {
        fn bind_params<'q>(
            mut query: sqlx::query::Query<'q, $db, $args>,
            values: &'q [serde_json::Value],
        ) -> sqlx::query::Query<'q, $db, $args> {
            use serde_json::Value as JsonValue;
            for value in values {
                if value.is_null() {
                    query = query.bind(None::<JsonValue>);
                } else if let Some(b) = value.as_bool() {
                    // Bind JS booleans as native `bool`. sqlx encodes this as
                    // BOOLEAN for Postgres and as TINYINT 0/1 for MySQL/SQLite.
                    // Without this branch the fallback serializes to JSON text
                    // ("true"/"false") and MySQL rejects it for tinyint columns.
                    query = query.bind(b);
                } else if value.is_string() {
                    query = query.bind(value.as_str().unwrap().to_owned());
                } else if let Some(number) = value.as_number() {
                    query = query.bind(number.as_f64().unwrap_or_default());
                } else {
                    query = query.bind(value.clone());
                }
            }
            query
        }

        #[$crate::__private::async_trait::async_trait]
        impl $crate::Driver for $driver_name {
            async fn query(
                &self,
                sql: &str,
                params: Vec<serde_json::Value>,
            ) -> Result<$crate::QueryResult, $crate::DbError> {
                use sqlx::{Column, Row};
                use $crate::__private::futures::StreamExt;

                let query = sqlx::query(sql);
                let query = bind_params(query, &params);

                // Stream rows (rather than fetch_all) so we can bail out as
                // soon as the per-query cap is hit. Without this, a `SELECT *
                // FROM big_table` loads everything into RAM before we can
                // reject it.
                let cap = $crate::max_query_rows();
                let mut stream = query.fetch(&self.pool);

                let mut columns: Vec<String> = Vec::new();
                let mut result_rows: Vec<Vec<serde_json::Value>> = Vec::new();

                while let Some(row_result) = stream.next().await {
                    let row = row_result.map_err($crate::DbError::query_error)?;

                    if columns.is_empty() {
                        columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    if result_rows.len() >= cap {
                        return Err($crate::DbError::result_too_large(cap));
                    }
                    let mut values = Vec::with_capacity(columns.len());
                    for i in 0..row.columns().len() {
                        let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                        values.push($decode_fn(v)?);
                    }
                    result_rows.push(values);
                }

                Ok($crate::QueryResult {
                    columns,
                    rows: result_rows,
                })
            }

            fn query_stream<'a>(
                &'a self,
                sql: String,
                params: Vec<serde_json::Value>,
                cancel: $crate::CancellationToken,
            ) -> $crate::BoxStream<'a, Result<$crate::StreamBatch, $crate::DbError>> {
                Box::pin($crate::__private::async_stream::try_stream! {
                    use sqlx::{Column, Row};
                    use $crate::__private::futures::StreamExt;

                    const BATCH_SIZE: usize = 5000;

                    let sqlx_query = sqlx::query(&sql);
                    let sqlx_query = bind_params(sqlx_query, &params);

                    // `take_until` ends the row stream as soon as `cancel`
                    // fires, even in the middle of a batch. Dropping the fetch
                    // releases the pooled connection.
                    let mut stream = std::pin::pin!(sqlx_query
                        .fetch(&self.pool)
                        .take_until(cancel.cancelled()));

                    let mut buffer: Vec<Vec<serde_json::Value>> = Vec::with_capacity(BATCH_SIZE);
                    let mut captured_columns: Option<Vec<String>> = None;
                    let mut first_batch = true;

                    while let Some(row_result) = stream.next().await {
                        let row = row_result.map_err($crate::DbError::query_error)?;

                        if captured_columns.is_none() {
                            captured_columns = Some(
                                row.columns().iter().map(|c| c.name().to_string()).collect(),
                            );
                        }
                        let col_count = row.columns().len();
                        let mut values = Vec::with_capacity(col_count);
                        for i in 0..col_count {
                            let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                            values.push($decode_fn(v)?);
                        }
                        buffer.push(values);

                        if buffer.len() >= BATCH_SIZE {
                            let batch_cols = if first_batch { captured_columns.clone() } else { None };
                            first_batch = false;
                            yield $crate::StreamBatch {
                                columns: batch_cols,
                                rows: std::mem::take(&mut buffer),
                                is_final: false,
                            };
                        }
                    }

                    // Terminal batch — empty buffer is fine, but still needs to carry
                    // the columns if no row ever arrived (empty result set).
                    let final_cols = if first_batch {
                        Some(captured_columns.unwrap_or_default())
                    } else {
                        None
                    };
                    yield $crate::StreamBatch {
                        columns: final_cols,
                        rows: buffer,
                        is_final: true,
                    };
                })
            }

            async fn execute(
                &self,
                sql: &str,
                params: Vec<serde_json::Value>,
            ) -> Result<$crate::ExecuteResult, $crate::DbError> {
                use sqlx::Executor;

                let query = sqlx::query(sql);
                let query = bind_params(query, &params);

                let result = self
                    .pool
                    .execute(query)
                    .await
                    .map_err($crate::DbError::execute_error)?;

                Ok($crate::ExecuteResult {
                    rows_affected: result.rows_affected(),
                    last_insert_id: ($last_insert_id)(&result),
                })
            }

            async fn transaction(&self, statements: Vec<$crate::BatchStatement>) -> Result<(), $crate::DbError> {
                use sqlx::{Acquire, Executor};

                let mut conn = self
                    .pool
                    .acquire()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                let mut tx = conn
                    .begin()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                for stmt in &statements {
                    let query = sqlx::query(&stmt.sql);
                    let query = bind_params(query, &stmt.params);
                    tx.execute(query)
                        .await
                        .map_err($crate::DbError::execute_error)?;
                }

                tx.commit()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                Ok(())
            }

            async fn close(&self) -> Result<(), $crate::DbError> {
                self.pool.close().await;
                Ok(())
            }
        }
    };
}
```

**Step 6: Run**

- Run: `cargo test -p seaquel-engine`. Expected: `test result: ok. 5 passed`.
- Run: `cargo clippy -p seaquel-engine --all-targets -- -D warnings && cargo clippy --target wasm32-unknown-unknown -p seaquel-engine -- -D warnings`. Expected: clean on both.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine with Driver, Engine and EngineRegistry`.

---

### Task 7: `seaquel-engine-testkit`

A smoke suite every engine runs from `tests/smoke.rs`. Engines that need a server read a `ConnectConfig` JSON from an environment variable and skip when it's unset, unless `SEAQUEL_TEST_REQUIRE_ENGINES` is set (CI sets it, so a missing service can't pass silently).

**Files:**
- Create: `crates/seaquel-engine-testkit/{Cargo.toml,src/lib.rs}`
- Modify: `Cargo.toml` (members)

**Step 1:** Add `"crates/seaquel-engine-testkit",` to `members` after `seaquel-engine`.

**Step 2: `crates/seaquel-engine-testkit/Cargo.toml`**

```toml
[package]
name = "seaquel-engine-testkit"
version = "0.1.0"
edition = "2021"
description = "Behaviour every Seaquel engine must have. Engine crates run it from their tests."
publish = false

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
futures = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
```

**Step 3: `crates/seaquel-engine-testkit/src/lib.rs`**

```rust
//! Behaviour every Seaquel engine must have.
//!
//! Engine crates call [`run_smoke`] from `tests/smoke.rs`. Engines that need a
//! server (Postgres, MySQL, MariaDB, MSSQL) read their connection from an
//! environment variable holding `ConnectConfig` JSON (see [`config_from_env`])
//! and skip when it's unset, so `cargo test` works without Docker. CI sets the
//! variables, runs the servers as service containers, and sets
//! [`REQUIRE_ENGINES`] so a missing variable fails instead of skipping.
//!
//! This is the seed of the conformance suite planned for phase 1.

use futures::{FutureExt, StreamExt};
use seaquel_engine::{
    BatchStatement, CancellationToken, ConnectConfig, Driver, Engine, StreamBatch,
};
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;

/// When set, [`config_from_env`] panics on a missing variable instead of
/// skipping the test.
pub const REQUIRE_ENGINES: &str = "SEAQUEL_TEST_REQUIRE_ENGINES";

/// How an engine's SQL differs in ways the smoke suite cares about.
pub struct SmokeSpec {
    /// Placeholder for the 1-based parameter `n`.
    pub placeholder: fn(usize) -> String,
    /// Whether `Driver::transaction` is implemented. Engines that don't must
    /// return `TRANSACTION_NOT_SUPPORTED`.
    pub supports_transactions: bool,
}

impl SmokeSpec {
    /// MySQL, MariaDB, SQLite, DuckDB.
    pub const QUESTION_MARK: SmokeSpec = SmokeSpec {
        placeholder: |_| "?".to_string(),
        supports_transactions: true,
    };
    /// Postgres.
    pub const DOLLAR: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("${n}"),
        supports_transactions: true,
    };
    /// MSSQL. Its driver doesn't implement transactions yet.
    pub const AT_P: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("@P{n}"),
        supports_transactions: false,
    };
}

/// Read a `ConnectConfig` from `var` (JSON). `None` when unset, so the caller
/// can skip, unless [`REQUIRE_ENGINES`] is set.
///
/// # Panics
///
/// If the variable is set but isn't valid `ConnectConfig` JSON, or if it's
/// unset while [`REQUIRE_ENGINES`] is set.
pub fn config_from_env(var: &str) -> Option<ConnectConfig> {
    match std::env::var(var) {
        Ok(raw) => Some(
            serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("{var} is not valid ConnectConfig JSON: {e}")),
        ),
        Err(_) if std::env::var_os(REQUIRE_ENGINES).is_some() => {
            panic!("{var} is not set, and {REQUIRE_ENGINES} requires every engine to run")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

/// 10,000 rows in one column `n` (0..=9999), using only SQL every engine
/// accepts. Larger than the sqlx drivers' 5,000-row batch, so it exercises
/// multi-batch streaming.
pub const TEN_THOUSAND_ROWS: &str = "SELECT a.d + 10 * b.d + 100 * c.d + 1000 * e.d AS n \
     FROM (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) a \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) b \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) c \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) e";

/// Open, query, write with parameters, run transactions and stream, against a
/// scratch table that is dropped afterwards even if an assertion fails.
pub async fn run_smoke(engine: &dyn Engine, config: &ConnectConfig, spec: &SmokeSpec) {
    assert_eq!(engine.id(), config.driver.as_str(), "engine id must match its driver");
    let driver = engine.open(config).await.expect("open");

    let table = format!("seaquel_smoke_{}", uuid::Uuid::new_v4().simple());
    let outcome = AssertUnwindSafe(smoke_body(&*driver, &table, spec))
        .catch_unwind()
        .await;
    let _ = driver.execute(&format!("DROP TABLE {table}"), vec![]).await;
    driver.close().await.expect("close");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn smoke_body(driver: &dyn Driver, table: &str, spec: &SmokeSpec) {
    let p = spec.placeholder;

    // Plain query.
    let one = driver.query("SELECT 1 AS one", vec![]).await.expect("SELECT 1");
    assert_eq!(one.columns, vec!["one"]);
    assert_eq!(one.rows.len(), 1);
    assert_eq!(as_i64(&one.rows[0][0]), 1);

    // DDL and parameterised writes.
    driver
        .execute(
            &format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, label VARCHAR(50))"),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let inserted = driver
        .execute(
            &format!("INSERT INTO {table} (id, label) VALUES ({}, {})", p(1), p(2)),
            vec![json!(1), json!("one")],
        )
        .await
        .expect("INSERT");
    assert_eq!(inserted.rows_affected, 1);

    let row = driver
        .query(
            &format!("SELECT id, label FROM {table} WHERE id = {}", p(1)),
            vec![json!(1)],
        )
        .await
        .expect("SELECT by id");
    assert_eq!(row.columns, vec!["id", "label"]);
    assert_eq!(row.rows.len(), 1);
    assert_eq!(as_i64(&row.rows[0][0]), 1);
    assert_eq!(row.rows[0][1], json!("one"));

    // Transactions: all or nothing.
    let insert = |id: i64, label: &str| BatchStatement {
        sql: format!("INSERT INTO {table} (id, label) VALUES ({}, {})", p(1), p(2)),
        params: vec![json!(id), json!(label)],
    };
    let ok = driver.transaction(vec![insert(2, "two"), insert(3, "three")]).await;
    if spec.supports_transactions {
        ok.expect("transaction");
        assert_eq!(count(driver, table).await, 3);
        let dup = driver.transaction(vec![insert(4, "four"), insert(1, "dup")]).await;
        assert!(dup.is_err(), "duplicate key must fail the transaction");
        assert_eq!(count(driver, table).await, 3, "failed transaction must roll back");
    } else {
        assert_eq!(ok.expect_err("transaction").code, "TRANSACTION_NOT_SUPPORTED");
    }

    // Streaming.
    let batches: Vec<StreamBatch> = driver
        .query_stream(TEN_THOUSAND_ROWS.to_string(), vec![], CancellationToken::new())
        .map(|b| b.expect("stream batch"))
        .collect()
        .await;
    assert!(!batches.is_empty());
    assert_eq!(batches[0].columns, Some(vec!["n".to_string()]));
    assert!(batches[1..].iter().all(|b| b.columns.is_none()));
    assert!(batches.last().unwrap().is_final);
    assert_eq!(batches.iter().filter(|b| b.is_final).count(), 1);
    assert_eq!(batches.iter().map(|b| b.rows.len()).sum::<usize>(), 10_000);
}

async fn count(driver: &dyn Driver, table: &str) -> i64 {
    let r = driver
        .query(&format!("SELECT COUNT(*) AS c FROM {table}"), vec![])
        .await
        .expect("COUNT");
    as_i64(&r.rows[0][0])
}

/// Engines decode integers differently (JSON number, or a string for
/// NUMERIC/DECIMAL). Accept either.
fn as_i64(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("expected an integer, got {v}"))
}
```

**Step 4:** Run `cargo clippy -p seaquel-engine-testkit -- -D warnings`. Expected: clean. The suite is exercised by the engine tasks that follow.

**Step 5: Checkpoint.** Suggested commit: `Add the engine smoke testkit`.

---

### Task 8: `seaquel-engine-sqlite`

This task sets the pattern that Tasks 9–12 repeat for the other engines.

**Files:**
- Create: `crates/seaquel-engine-sqlite/{Cargo.toml,src/lib.rs,tests/smoke.rs}`
- Create by copying:
  - `crates/seaquel-db/src/sqlite.rs` → `crates/seaquel-engine-sqlite/src/driver.rs`
  - `crates/seaquel-db/src/decode/sqlite.rs` → `crates/seaquel-engine-sqlite/src/decode.rs`
- Modify: `Cargo.toml` (members)

**Step 1:** Add `"crates/seaquel-engine-sqlite",` to `members`. Then copy the files:

```bash
mkdir -p crates/seaquel-engine-sqlite/src crates/seaquel-engine-sqlite/tests
cp crates/seaquel-db/src/sqlite.rs crates/seaquel-engine-sqlite/src/driver.rs
cp crates/seaquel-db/src/decode/sqlite.rs crates/seaquel-engine-sqlite/src/decode.rs
```

**Step 2: `crates/seaquel-engine-sqlite/Cargo.toml`**

```toml
[package]
name = "seaquel-engine-sqlite"
version = "0.1.0"
edition = "2021"
description = "SQLite engine for Seaquel."

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
seaquel-runtime = { path = "../seaquel-runtime" }
serde_json = { workspace = true }
sqlx = { workspace = true }
time = { workspace = true }

[dev-dependencies]
seaquel-engine-testkit = { path = "../seaquel-engine-testkit" }
serde_json = { workspace = true }
tokio = { version = "1", features = ["macros", "rt"] }
uuid = { workspace = true }
```

**Step 3: The smoke test (red)** — `crates/seaquel-engine-sqlite/tests/smoke.rs`:

```rust
use seaquel_engine_testkit::{run_smoke, SmokeSpec};

#[tokio::test]
async fn smoke() {
    let path = std::env::temp_dir().join(format!("seaquel-smoke-{}.sqlite", uuid::Uuid::new_v4()));
    let config = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite:{}", path.display()),
        "create_if_missing": true
    }))
    .unwrap();

    run_smoke(&*seaquel_engine_sqlite::engine(), &config, &SmokeSpec::QUESTION_MARK).await;

    let _ = std::fs::remove_file(&path);
}
```

Run: `cargo test -p seaquel-engine-sqlite --test smoke`
Expected: compile errors (no `src/lib.rs` yet).

**Step 4: Adapt the copied files**

In `src/driver.rs`:

| Find | Replace with |
|---|---|
| `use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` | `use seaquel_engine::{ConnectConfig, DbError};` |
| `super::impl_sqlx_driver!(` | `seaquel_engine::impl_sqlx_driver!(` |
| `decode_fn = super::decode::sqlite::to_json,` | `decode_fn = crate::decode::to_json,` |
| `.map_err(\|e\| DbError::connection_error(e))` | `.map_err(DbError::connection_error)` |

In `src/decode.rs`: replace `use crate::DbError;` with `use seaquel_engine::DbError;`.

**Step 5: `crates/seaquel-engine-sqlite/src/lib.rs`**

```rust
//! SQLite engine for Seaquel.

mod decode;
mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct SqliteEngine;

#[seaquel_runtime::async_trait]
impl Engine for SqliteEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::SqliteDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(SqliteEngine)
}
```

**Step 6: Run**

- Run: `cargo test -p seaquel-engine-sqlite`. Expected: the 4 unit tests carried over in `driver.rs` pass, and `smoke` passes.
- Run: `cargo clippy --fix --allow-dirty -p seaquel-engine-sqlite --all-targets && cargo clippy -p seaquel-engine-sqlite --all-targets -- -D warnings`. Expected: clean.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine-sqlite`.

---

### Task 9: `seaquel-engine-postgres`

The same pattern as Task 8.

**Step 1:** Add `"crates/seaquel-engine-postgres",` to `members`.

```bash
mkdir -p crates/seaquel-engine-postgres/src crates/seaquel-engine-postgres/tests
cp crates/seaquel-db/src/postgres.rs crates/seaquel-engine-postgres/src/driver.rs
cp crates/seaquel-db/src/decode/postgres.rs crates/seaquel-engine-postgres/src/decode.rs
```

**Step 2: `crates/seaquel-engine-postgres/Cargo.toml`**

```toml
[package]
name = "seaquel-engine-postgres"
version = "0.1.0"
edition = "2021"
description = "PostgreSQL engine for Seaquel."

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
seaquel-runtime = { path = "../seaquel-runtime" }
rust_decimal = { workspace = true }
serde_json = { workspace = true }
sqlx = { workspace = true }
time = { workspace = true }

[dev-dependencies]
seaquel-engine-testkit = { path = "../seaquel-engine-testkit" }
tokio = { version = "1", features = ["macros", "rt"] }
```

**Step 3: `tests/smoke.rs`**

```rust
use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
/// {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}
#[tokio::test]
async fn smoke() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run_smoke(&*seaquel_engine_postgres::engine(), &config, &SmokeSpec::DOLLAR).await;
}
```

**Step 4: Adapt the copied files**

In `src/driver.rs`:

| Find | Replace with |
|---|---|
| `use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` | `use seaquel_engine::{ConnectConfig, DbError};` |
| `super::impl_sqlx_driver!(` | `seaquel_engine::impl_sqlx_driver!(` |
| `decode_fn = super::decode::postgres::to_json,` | `decode_fn = crate::decode::to_json,` |
| `.map_err(\|e\| DbError::connection_error(e))` | `.map_err(DbError::connection_error)` |

In `src/decode.rs`: replace `use crate::DbError;` with `use seaquel_engine::DbError;`.

**Step 5: `src/lib.rs`**

```rust
//! PostgreSQL engine for Seaquel.

mod decode;
mod driver;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, DbError, Driver, Engine};

pub struct PostgresEngine;

#[seaquel_runtime::async_trait]
impl Engine for PostgresEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(driver::PostgresDriver::connect(config).await?))
    }
}

pub fn engine() -> Arc<dyn Engine> {
    Arc::new(PostgresEngine)
}
```

**Step 6: Run against a real server**

```bash
npm run e2e:db:up          # in another terminal; wait until postgres is healthy
npm run e2e:db:seed -- postgresql   # creates the seaquel_test database
SEAQUEL_TEST_POSTGRES='{"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}' \
  cargo test -p seaquel-engine-postgres --test smoke
```
Expected: `smoke ... ok`. Without the variable the test prints `skipping` and passes.

Then run: `cargo clippy --fix --allow-dirty -p seaquel-engine-postgres --all-targets && cargo clippy -p seaquel-engine-postgres --all-targets -- -D warnings`.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine-postgres`.

---

### Task 10: `seaquel-engine-mysql` (MySQL and MariaDB)

**Step 1:** Add `"crates/seaquel-engine-mysql",` to `members`.

```bash
mkdir -p crates/seaquel-engine-mysql/src crates/seaquel-engine-mysql/tests
cp crates/seaquel-db/src/mysql.rs crates/seaquel-engine-mysql/src/driver.rs
cp crates/seaquel-db/src/decode/mysql.rs crates/seaquel-engine-mysql/src/decode.rs
```

**Step 2: `Cargo.toml`.** This is identical to Postgres's, with `name = "seaquel-engine-mysql"` and `description = "MySQL and MariaDB engine for Seaquel."`.

**Step 3: `tests/smoke.rs`**

```rust
use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// SEAQUEL_TEST_MYSQL, e.g.
/// {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}
#[tokio::test]
async fn mysql() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MYSQL") else {
        return;
    };
    run_smoke(&*seaquel_engine_mysql::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}

/// SEAQUEL_TEST_MARIADB. MariaDB uses the mysql driver, e.g.
/// {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}
#[tokio::test]
async fn mariadb() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MARIADB") else {
        return;
    };
    run_smoke(&*seaquel_engine_mysql::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}
```

**Step 4: Adapt.** Use the same table as Task 9, with `super::decode::mysql::to_json` as the decode path to replace. Leave the `last_insert_id = |r: &sqlx::mysql::MySqlQueryResult| …` line as it is.

**Step 5: `src/lib.rs`.** This is the same as Postgres with these substitutions:
- the doc line becomes `//! MySQL and MariaDB engine for Seaquel. MariaDB connections use the "mysql" driver on the wire.`
- `PostgresEngine` becomes `MysqlEngine`
- the id `"postgres"` becomes `"mysql"`
- `driver::PostgresDriver` becomes `driver::MysqlDriver`

**Step 6: Run**

```bash
npm run e2e:db:seed -- mysql mariadb
SEAQUEL_TEST_MYSQL='{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}' \
SEAQUEL_TEST_MARIADB='{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}' \
  cargo test -p seaquel-engine-mysql --test smoke
```
Expected: `mysql ... ok` and `mariadb ... ok`. Then run clippy `--fix` followed by `-D warnings`, as in Task 9.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine-mysql`.

---

### Task 11: `seaquel-engine-mssql`

MSSQL isn't sqlx-based. It keeps its own `Driver` impl and uses the default (non-streaming) `query_stream`.

**Step 1:** Add `"crates/seaquel-engine-mssql",` to `members`.

```bash
mkdir -p crates/seaquel-engine-mssql/src crates/seaquel-engine-mssql/tests
cp crates/seaquel-db/src/mssql.rs crates/seaquel-engine-mssql/src/driver.rs
```

**Step 2: `Cargo.toml`**

```toml
[package]
name = "seaquel-engine-mssql"
version = "0.1.0"
edition = "2021"
description = "Microsoft SQL Server engine for Seaquel."

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
seaquel-runtime = { path = "../seaquel-runtime" }
base64 = { workspace = true }
futures = { workspace = true }
log = { workspace = true }
serde_json = { workspace = true }
tiberius = { workspace = true }
tokio = { workspace = true, features = ["time"] }
tokio-util = { workspace = true }
# tiberius pulls chrono without default features; to_rfc3339 (driver.rs) needs "alloc".
# Inside the old seaquel-db crate it arrived by accident, via duckdb → arrow.
chrono = { version = "0.4", default-features = false, features = ["alloc"] }

[dev-dependencies]
seaquel-engine-testkit = { path = "../seaquel-engine-testkit" }
tokio = { version = "1", features = ["macros", "rt"] }
```

**Step 3: `tests/smoke.rs`**

```rust
use seaquel_engine_testkit::{config_from_env, run_smoke, SmokeSpec};

/// SEAQUEL_TEST_MSSQL, e.g.
/// {"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa",
///  "password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}
#[tokio::test]
async fn smoke() {
    let Some(config) = config_from_env("SEAQUEL_TEST_MSSQL") else {
        return;
    };
    run_smoke(&*seaquel_engine_mssql::engine(), &config, &SmokeSpec::AT_P).await;
}
```

**Step 4: Adapt `src/driver.rs`**

| Find | Replace with |
|---|---|
| `use async_trait::async_trait;` | *(delete the line)* |
| `use super::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` | `use seaquel_engine::{ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` |
| `super::max_query_rows()` | `seaquel_engine::max_query_rows()` |
| `#[async_trait]` (above `impl Driver for MssqlDriver`) | `#[seaquel_runtime::async_trait]` |

**Step 5: `src/lib.rs`.** Use the Postgres template with these changes:
- drop `mod decode;`, because MSSQL has no decode module
- the doc line becomes `//! Microsoft SQL Server engine for Seaquel.`
- the engine is `MssqlEngine` with id `"mssql"`
- `open` returns `Ok(Arc::new(driver::MssqlDriver::connect(config).await?))`

**Step 6: Run**

```bash
SEAQUEL_TEST_MSSQL='{"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa","password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}' \
  cargo test -p seaquel-engine-mssql --test smoke
```
Expected: `smoke ... ok`. The suite asserts `TRANSACTION_NOT_SUPPORTED` for MSSQL, which documents today's gap. Fixing it is phase 1. On Apple Silicon the SQL Server image runs under Rosetta and is slow on first boot.

Then run clippy `--fix` and `-D warnings`.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine-mssql`.

---

### Task 12: `seaquel-engine-duckdb`

**Step 1:** Add `"crates/seaquel-engine-duckdb",` to `members`.

```bash
mkdir -p crates/seaquel-engine-duckdb/src crates/seaquel-engine-duckdb/tests
cp crates/seaquel-db/src/duckdb.rs crates/seaquel-engine-duckdb/src/driver.rs
```

**Step 2: `Cargo.toml`**

```toml
[package]
name = "seaquel-engine-duckdb"
version = "0.1.0"
edition = "2021"
description = "DuckDB engine for Seaquel (native; the browser demo gets its own engine in phase 8)."

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
seaquel-runtime = { path = "../seaquel-runtime" }
base64 = { workspace = true }
duckdb = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
seaquel-engine-testkit = { path = "../seaquel-engine-testkit" }
serde_json = { workspace = true }
tokio = { version = "1", features = ["macros", "rt"] }
```

**Step 3: `tests/smoke.rs`**

```rust
use seaquel_engine_testkit::{run_smoke, SmokeSpec};

#[tokio::test]
async fn smoke() {
    let config = serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": ":memory:"
    }))
    .unwrap();
    run_smoke(&*seaquel_engine_duckdb::engine(), &config, &SmokeSpec::QUESTION_MARK).await;
}
```

**Step 4: Adapt `src/driver.rs`**

| Find | Replace with |
|---|---|
| `use async_trait::async_trait;` | *(delete the line)* |
| `use super::{BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` | `use seaquel_engine::{BatchStatement, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};` |
| `super::max_query_rows()` | `seaquel_engine::max_query_rows()` |
| `#[async_trait]` (above `impl Driver for DuckdbDriver`) | `#[seaquel_runtime::async_trait]` |

**Step 5: `src/lib.rs`.** Use the Postgres template with these changes:
- drop `mod decode;`, because DuckDB has no decode module
- the doc line becomes `//! DuckDB engine for Seaquel.`
- the engine is `DuckdbEngine` with id `"duckdb"`
- `open` returns `Ok(Arc::new(driver::DuckdbDriver::connect(config)?))`. DuckDB's `connect` is synchronous, so there's no `.await`.

**Step 6: Run**

- Run: `cargo test -p seaquel-engine-duckdb`. Expected: the 3 carried-over unit tests plus `smoke` pass.
- Run clippy `--fix` and `-D warnings`.

**Step 7: Checkpoint.** Suggested commit: `Add seaquel-engine-duckdb`.

---

### Task 13: `seaquel-core`

Core owns what `ConnectionManager` owned, plus the connect/disconnect logic that `src-tauri` and `seaquel-server` each duplicate today. It also owns stream cancellation through `CancellationToken`.

**Files:**
- Create: `crates/seaquel-core/{Cargo.toml,src/lib.rs,tests/core.rs}`
- Modify: `Cargo.toml` (members)

**Step 1:** Add `"crates/seaquel-core",` to `members`, before `seaquel-server`.

**Step 2: `crates/seaquel-core/Cargo.toml`**

```toml
[package]
name = "seaquel-core"
version = "0.1.0"
edition = "2021"
description = "Seaquel Core: registers plugins and owns connections and running queries. Every interface goes through it."

[features]
default = ["engine-postgres", "engine-mysql", "engine-sqlite", "engine-mssql", "engine-duckdb"]
engine-postgres = ["dep:seaquel-engine-postgres"]
engine-mysql = ["dep:seaquel-engine-mysql"]
engine-sqlite = ["dep:seaquel-engine-sqlite"]
engine-mssql = ["dep:seaquel-engine-mssql"]
engine-duckdb = ["dep:seaquel-engine-duckdb"]

[dependencies]
seaquel-engine = { path = "../seaquel-engine" }
seaquel-runtime = { path = "../seaquel-runtime" }
seaquel-types = { path = "../seaquel-types" }
seaquel-engine-postgres = { path = "../seaquel-engine-postgres", optional = true }
seaquel-engine-mysql = { path = "../seaquel-engine-mysql", optional = true }
seaquel-engine-sqlite = { path = "../seaquel-engine-sqlite", optional = true }
seaquel-engine-mssql = { path = "../seaquel-engine-mssql", optional = true }
seaquel-engine-duckdb = { path = "../seaquel-engine-duckdb", optional = true }
async-stream = { workspace = true }
futures = { workspace = true }
log = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
tokio = { version = "1", features = ["macros", "rt"] }
uuid = { workspace = true }
```

**Step 3: Tests first** — `crates/seaquel-core/tests/core.rs`:

```rust
#![cfg(feature = "engine-sqlite")]

use futures::StreamExt;
use seaquel_core::{Core, StreamEvent};
use seaquel_engine::{BatchStatement, ConnectConfig};
use serde_json::json;
use std::path::PathBuf;

/// A SQLite file in the temp dir, removed on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("seaquel-core-{}.sqlite", uuid::Uuid::new_v4())))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", self.0.display()),
            "create_if_missing": true
        }))
        .unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn core() -> Core {
    seaquel_core::with_default_plugins().build()
}

/// Connect and create `nums(n)` holding 1..=rows.
async fn connect_with_rows(core: &Core, db: &TempDb, rows: u32) -> String {
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    core.execute(&id, "CREATE TABLE nums (n INTEGER)", vec![]).await.unwrap();
    core.execute(
        &id,
        &format!(
            "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<{rows}) \
             INSERT INTO nums SELECT n FROM seq"
        ),
        vec![],
    )
    .await
    .unwrap();
    id
}

fn row_count(events: &[StreamEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            StreamEvent::Batch(b) => b.rows.len(),
            _ => 0,
        })
        .sum()
}

#[tokio::test]
async fn connect_query_disconnect() {
    let core = core();
    let db = TempDb::new();
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    assert!(id.starts_with("sqlite-"), "{id}");
    assert_eq!(core.connection_count(), 1);

    let r = core.query(&id, "SELECT 1 AS one", vec![]).await.unwrap();
    assert_eq!(r.columns, vec!["one"]);
    assert_eq!(r.rows, vec![vec![json!(1)]]);

    core.disconnect(&id).await.unwrap();
    assert_eq!(core.connection_count(), 0);
    let err = core.query(&id, "SELECT 1", vec![]).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");

    // Idempotent.
    core.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn test_does_not_register_a_connection() {
    let core = core();
    let db = TempDb::new();
    core.test(&db.config()).await.unwrap();
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn connect_without_the_engine_fails() {
    let core = Core::builder().build();
    let db = TempDb::new();
    let err = core.connect(&db.config()).await.unwrap_err();
    assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
}

#[tokio::test]
async fn transaction_is_all_or_nothing() {
    let core = core();
    let db = TempDb::new();
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    core.execute(&id, "CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![]).await.unwrap();

    let insert = |v: i64| BatchStatement {
        sql: "INSERT INTO t (id) VALUES (?)".into(),
        params: vec![json!(v)],
    };
    assert!(core.transaction(&id, vec![insert(1), insert(1)]).await.is_err());

    let r = core.query(&id, "SELECT COUNT(*) AS c FROM t", vec![]).await.unwrap();
    assert_eq!(r.rows, vec![vec![json!(0)]]);
}

#[tokio::test]
async fn stream_emits_batches_then_done() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 12_345).await;

    let events: Vec<StreamEvent> = core
        .query_stream("q1".into(), id, "SELECT n FROM nums ORDER BY n".into(), vec![])
        .collect()
        .await;

    assert_eq!(row_count(&events), 12_345);
    let batches = events.iter().filter(|e| matches!(e, StreamEvent::Batch(_))).count();
    assert!(batches >= 2, "expected multi-batch streaming, got {batches}");
    assert!(matches!(events.last(), Some(StreamEvent::Done)));
    assert_eq!(events.iter().filter(|e| matches!(e, StreamEvent::Done)).count(), 1);
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn stream_on_an_unknown_connection_emits_one_error() {
    let core = core();
    let events: Vec<StreamEvent> = core
        .query_stream("q1".into(), "nope".into(), "SELECT 1".into(), vec![])
        .collect()
        .await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        StreamEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_NOT_FOUND"),
        other => panic!("expected an error event, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_ends_the_stream_without_a_terminal_event() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 200_000).await;

    let mut stream = core.query_stream("q1".into(), id, "SELECT n FROM nums".into(), vec![]);
    let first = stream.next().await.unwrap();
    assert!(matches!(first, StreamEvent::Batch(_)));

    core.cancel_stream("q1");
    let rest: Vec<StreamEvent> = stream.collect().await;

    assert!(
        rest.iter().all(|e| matches!(e, StreamEvent::Batch(_))),
        "no Done or Error after cancel: {rest:?}"
    );
    let total = row_count(&[first]) + row_count(&rest);
    assert!(total < 200_000, "cancel should stop fetching, got all {total} rows");
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn dropping_a_stream_unregisters_it() {
    let core = core();
    let stream = core.query_stream("q1".into(), "nope".into(), "SELECT 1".into(), vec![]);
    assert_eq!(core.running_stream_count(), 1);
    drop(stream);
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn a_reused_query_id_cancels_the_newest_stream() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 10).await;

    let older = core.query_stream("q".into(), id.clone(), "SELECT n FROM nums".into(), vec![]);
    let newer = core.query_stream("q".into(), id, "SELECT n FROM nums".into(), vec![]);
    drop(older);
    assert_eq!(core.running_stream_count(), 1, "dropping the older stream must not unregister the newer one");

    core.cancel_stream("q");
    let events: Vec<StreamEvent> = newer.collect().await;
    assert!(events.is_empty(), "cancelled before it started: {events:?}");
}
```

Create `crates/seaquel-core/src/lib.rs` as `//! placeholder`, then run: `cargo test -p seaquel-core`
Expected: compile errors.

**Step 4: Implement** — `crates/seaquel-core/src/lib.rs`:

```rust
//! Seaquel Core.
//!
//! Interfaces (the Tauri app, `seaquel-server`, and later the CLI, TUI and MCP
//! server) do database work only through [`Core`]. It owns the engine
//! registry, the open connections, and the cancellation tokens of running
//! streams. It grows into the full Core from
//! `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` over the
//! following phases.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use futures::StreamExt;
use log::{debug, info};
use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, ConnectConfig, ConnectResult, DbError, Driver,
    Engine, EngineRegistry, ExecuteResult, QueryResult,
};
pub use seaquel_types::StreamEvent;

type StreamTokens = Mutex<HashMap<String, (u64, CancellationToken)>>;

pub struct Core {
    engines: EngineRegistry,
    /// `Arc` (not `Box`) so a handle can be cloned out of the map and the lock
    /// released before awaiting the driver. A long-running stream must never
    /// hold this lock, or it would block every other caller — disconnect,
    /// new queries, other connections — until it finished.
    connections: RwLock<HashMap<String, Arc<dyn Driver>>>,
    /// Cancellation tokens of running streams, keyed by the client's query id.
    /// The `u64` tells apart two streams that were given the same query id.
    streams: StreamTokens,
    next_stream: AtomicU64,
}

#[derive(Default)]
pub struct CoreBuilder {
    engines: EngineRegistry,
}

impl CoreBuilder {
    pub fn engine(mut self, engine: Arc<dyn Engine>) -> Self {
        self.engines.register(engine);
        self
    }

    pub fn build(self) -> Core {
        Core {
            engines: self.engines,
            connections: RwLock::default(),
            streams: Mutex::default(),
            next_stream: AtomicU64::new(0),
        }
    }
}

/// A builder with every plugin this build's Cargo features enable.
pub fn with_default_plugins() -> CoreBuilder {
    #[allow(unused_mut)]
    let mut builder = Core::builder();
    #[cfg(feature = "engine-postgres")]
    {
        builder = builder.engine(seaquel_engine_postgres::engine());
    }
    #[cfg(feature = "engine-mysql")]
    {
        builder = builder.engine(seaquel_engine_mysql::engine());
    }
    #[cfg(feature = "engine-sqlite")]
    {
        builder = builder.engine(seaquel_engine_sqlite::engine());
    }
    #[cfg(feature = "engine-mssql")]
    {
        builder = builder.engine(seaquel_engine_mssql::engine());
    }
    #[cfg(feature = "engine-duckdb")]
    {
        builder = builder.engine(seaquel_engine_duckdb::engine());
    }
    builder
}

fn sql_keyword(sql: &str) -> String {
    sql.split_whitespace().next().unwrap_or("?").to_uppercase()
}

impl Core {
    pub fn builder() -> CoreBuilder {
        CoreBuilder::default()
    }

    /// Ids of the engines in this build, sorted.
    pub fn engine_ids(&self) -> Vec<&'static str> {
        self.engines.ids()
    }

    pub async fn connect(&self, config: &ConnectConfig) -> Result<ConnectResult, DbError> {
        let driver_name = config.driver.as_str();
        info!(activity = "db.connect", driver = driver_name; "Connecting");

        let driver = self.engines.open(config).await?;
        let connection_id = format!("{}-{}", driver_name, uuid::Uuid::new_v4());
        self.connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(connection_id.clone(), driver);

        info!(activity = "db.connect", driver = driver_name, connection_id = connection_id.as_str(); "Connected");
        Ok(ConnectResult { connection_id })
    }

    /// Open and close a connection without registering it ("Test connection").
    pub async fn test(&self, config: &ConnectConfig) -> Result<(), DbError> {
        debug!(activity = "db.test", driver = config.driver.as_str(); "Testing connection");
        let driver = self.engines.open(config).await?;
        driver.close().await
    }

    /// Close a connection. Idempotent: an unknown id succeeds.
    pub async fn disconnect(&self, connection_id: &str) -> Result<(), DbError> {
        info!(activity = "db.disconnect", connection_id = connection_id; "Disconnecting");
        let driver = self
            .connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id);
        if let Some(driver) = driver {
            // `close()` waits for in-flight queries to return their pooled
            // connections. A stream holding an `Arc` clone drops it when done.
            driver.close().await?;
        }
        Ok(())
    }

    pub fn connection_count(&self) -> usize {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn driver(&self, connection_id: &str) -> Result<Arc<dyn Driver>, DbError> {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .cloned()
            .ok_or_else(|| DbError::connection_not_found(connection_id))
    }

    pub async fn query(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.query", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query");
        self.driver(connection_id)?.query(sql, params).await
    }

    pub async fn execute(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.execute", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Execute");
        self.driver(connection_id)?.execute(sql, params).await
    }

    pub async fn transaction(
        &self,
        connection_id: &str,
        statements: Vec<BatchStatement>,
    ) -> Result<(), DbError> {
        debug!(activity = "db.transaction", connection_id = connection_id, statements = statements.len(); "Executing transaction");
        self.driver(connection_id)?.transaction(statements).await
    }

    /// Run a query and deliver its results as client events: zero or more
    /// `Batch` events, then exactly one `Done` or `Error`.
    ///
    /// After [`Core::cancel_stream`] with the same `query_id`, or when the
    /// returned stream is dropped, the driver stops fetching and the stream
    /// ends with no terminal event.
    pub fn query_stream(
        &self,
        query_id: String,
        connection_id: String,
        sql: String,
        params: Vec<serde_json::Value>,
    ) -> BoxStream<'_, StreamEvent> {
        let keyword = sql_keyword(&sql);
        debug!(activity = "db.query_stream", query_id = query_id.as_str(), connection_id = connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query stream");

        // Registered now, not on first poll, so a cancel that arrives before
        // the stream starts still counts.
        let (token, guard) = self.register_stream(query_id);
        Box::pin(async_stream::stream! {
            let _guard = guard;
            let driver = match self.driver(&connection_id) {
                Ok(driver) => driver,
                Err(e) => {
                    yield StreamEvent::from(e);
                    return;
                }
            };
            let mut batches = driver.query_stream(sql, params, token.clone());
            while let Some(item) = batches.next().await {
                if token.is_cancelled() {
                    return;
                }
                match item {
                    Ok(batch) => yield StreamEvent::Batch(batch),
                    Err(e) => {
                        yield StreamEvent::from(e);
                        return;
                    }
                }
            }
            if !token.is_cancelled() {
                yield StreamEvent::Done;
            }
        })
    }

    /// Cancel a running stream. Unknown or finished query ids are ignored.
    pub fn cancel_stream(&self, query_id: &str) {
        debug!(activity = "db.cancel_stream", query_id = query_id; "Cancel stream");
        if let Some((_, token)) = self
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(query_id)
        {
            token.cancel();
        }
    }

    pub fn running_stream_count(&self) -> usize {
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn register_stream(&self, query_id: String) -> (CancellationToken, StreamGuard<'_>) {
        let token = CancellationToken::new();
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(query_id.clone(), (id, token.clone()));
        let guard = StreamGuard {
            streams: &self.streams,
            query_id,
            id,
        };
        (token, guard)
    }
}

/// Removes a stream's cancellation token when the stream finishes or is
/// dropped. It only removes its own entry: if a newer stream reused the query
/// id, that one stays cancellable.
struct StreamGuard<'a> {
    streams: &'a StreamTokens,
    query_id: String,
    id: u64,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if streams.get(&self.query_id).is_some_and(|(id, _)| *id == self.id) {
            streams.remove(&self.query_id);
        }
    }
}
```

**Step 5: Run**

- Run: `cargo test -p seaquel-core`. Expected: `test result: ok. 9 passed`.
- Run it five times to check the cancel test isn't flaky: `for i in 1 2 3 4 5; do cargo test -q -p seaquel-core --test core; done`. Expected: 9 passed every time.
- Run: `cargo clippy -p seaquel-core --all-targets -- -D warnings`. Expected: clean.

**Step 6: Checkpoint.** Suggested commit: `Add seaquel-core with connection and stream management`.

---

### Task 14: `seaquel-server` on Core

The HTTP and WebSocket contract doesn't change. The 18 existing integration tests are the guard, and they must pass unmodified.

**Files:**
- Modify: `crates/seaquel-server/Cargo.toml`
- Modify: `crates/seaquel-server/src/lib.rs`
- Modify: `crates/seaquel-server/src/error.rs`
- Modify: `crates/seaquel-server/src/routes/db/{connect,disconnect,query,execute,transaction,test,stream}.rs`

**Step 1: Record the baseline.** Run `cargo test -p seaquel-server` and confirm 18 pass.

**Step 2: Dependencies.** In `crates/seaquel-server/Cargo.toml`, replace `seaquel-db = { path = "../seaquel-db" }` with:

```toml
seaquel-core = { path = "../seaquel-core" }
seaquel-types = { path = "../seaquel-types" }
```

**Step 3: `AppState`.** In `src/lib.rs`, replace `use seaquel_db::ConnectionManager;` and the `AppState` struct and impl with:

```rust
use seaquel_core::Core;
```

```rust
/// Application state shared across request handlers.
#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            core: Arc::new(seaquel_core::with_default_plugins().build()),
        }
    }
}
```

Keep the `Default` impl and `build_router` as they are.

**Step 4: `error.rs`.** Change `use seaquel_db::DbError;` to `use seaquel_types::DbError;`, and in the module doc change `seaquel_db::DbError` to `seaquel_types::DbError`.

**Step 5: Handlers.** Each handler keeps its request struct and becomes a call into Core. Core now does the logging these handlers did, so remove their `debug!`/`info!` calls and any imports that become unused.

`connect.rs`:
```rust
use axum::{extract::State, Json};
use seaquel_types::{ConnectConfig, ConnectResult};

use crate::{error::ApiError, AppState};

pub async fn connect(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<Json<ConnectResult>, ApiError> {
    Ok(Json(state.core.connect(&config).await?))
}
```

`disconnect.rs`: the body becomes `state.core.disconnect(&req.connection_id).await?; Ok(())`. Keep the doc comment about idempotency.

`query.rs`: `use seaquel_types::QueryResult;`, and the body becomes `Ok(Json(state.core.query(&req.connection_id, &req.sql, req.values).await?))`.

`execute.rs`: `use seaquel_types::ExecuteResult;`, and the body becomes `Ok(Json(state.core.execute(&req.connection_id, &req.sql, req.values).await?))`.

`transaction.rs`: `use seaquel_types::BatchStatement;`, and the body becomes `state.core.transaction(&req.connection_id, req.statements).await?; Ok(())`.

`test.rs`: it needs state now, because Core owns the engine registry:
```rust
use axum::{extract::State, Json};
use seaquel_types::ConnectConfig;

use crate::{error::ApiError, AppState};

/// Validate that the given config can open a connection. Opens the driver,
/// closes it immediately, returns `()`. Mirrors the Tauri `db_test` command.
/// Probes never leave a connection behind.
pub async fn test(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<(), ApiError> {
    state.core.test(&config).await?;
    Ok(())
}
```

**Step 6: `stream.rs`** — replace the whole file:

```rust
//! WebSocket streaming endpoint. The Tauri counterpart is `db_query_stream`
//! in `src-tauri/src/db/commands.rs`; both forward `Core::query_stream`.
//!
//! Protocol:
//!   1. Client opens WS.
//!   2. Client sends one Text frame: `{"query_id","connection_id","sql","values"}`.
//!   3. Server sends `StreamEvent` frames: `{"type":"batch", ...StreamBatch fields}`
//!      (the last one has `"is_final": true`), then a terminal `{"type":"done"}`
//!      or `{"type":"error","message","code"}`.
//!   4. Server closes the socket.
//!
//! Cancellation: the client closes the WS. The next send fails, the event
//! stream is dropped, and Core stops the driver's fetch.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::StreamExt;
use log::warn;
use seaquel_core::StreamEvent;
use seaquel_types::DbError;
use serde::Deserialize;

use crate::AppState;

#[derive(Debug, Deserialize)]
struct StreamRequest {
    query_id: String,
    connection_id: String,
    sql: String,
    #[serde(default)]
    values: Vec<serde_json::Value>,
}

pub async fn stream(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let req = match socket.recv().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<StreamRequest>(&t) {
            Ok(r) => r,
            Err(e) => {
                warn!("stream request parse failed: {e}");
                send_error(&mut socket, DbError::query_error(format!("malformed request: {e}"))).await;
                return;
            }
        },
        Some(Ok(other)) => {
            warn!("stream first frame was not text: {other:?}");
            send_error(
                &mut socket,
                DbError::query_error(format!("expected Text frame, got {other:?}")),
            )
            .await;
            return;
        }
        Some(Err(e)) => {
            warn!("ws recv error before request: {e}");
            return;
        }
        None => return, // client closed immediately
    };

    let mut events = state
        .core
        .query_stream(req.query_id, req.connection_id, req.sql, req.values);
    while let Some(event) = events.next().await {
        // A batch carries arbitrary row data from a driver, so serializing it
        // can fail. Report that as the terminal error instead of skipping it.
        let json = match serde_json::to_string(&event) {
            Ok(json) => json,
            Err(e) => {
                warn!("failed to serialize stream event: {e}");
                send_error(&mut socket, DbError::query_error(format!("failed to serialize result: {e}"))).await;
                return;
            }
        };
        if socket.send(Message::Text(json.into())).await.is_err() {
            // Peer went away. Dropping `events` stops the fetch.
            return;
        }
    }
}

async fn send_error(socket: &mut WebSocket, err: DbError) {
    if let Ok(json) = serde_json::to_string(&StreamEvent::from(err)) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}
```

This changes one behaviour on purpose. If a batch failed to serialize, the old code stopped and then sent `done`, which falsely signalled success. It now sends an `error` event.

**Step 7: Verify**

- Run: `cargo test -p seaquel-server`. Expected: 18 passed, with no test files changed.
- Run: `cargo clippy --fix --allow-dirty -p seaquel-server --all-targets && cargo clippy -p seaquel-server --all-targets -- -D warnings`. Expected: clean. The `useless_conversion` warning from the baseline is in `tests/db_stream.rs`; `--fix` handles it.
- Manual: in one terminal run `npm run dev:web:full`. Then:
  - Sign in, connect to a SQLite database, and run a query that returns more than 5,000 rows. It streams.
  - Run a long query and cancel it. It stops.
  - Test a connection with a wrong path. It reports `FILE_NOT_FOUND`.

**Step 8: Checkpoint.** Suggested commit: `Serve the web DB API through seaquel-core`.

---

### Task 15: The Tauri app on Core

The IPC commands keep their names, arguments and results, so the frontend doesn't change.

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/db/mod.rs`
- Modify: `src-tauri/src/db/commands.rs` (full rewrite)
- Modify: `src-tauri/src/lib.rs:456`

**Step 1: Dependencies.** In `src-tauri/Cargo.toml`:
- Replace `seaquel-db = { path = "../crates/seaquel-db" }` with:

```toml
seaquel-core = { path = "../crates/seaquel-core" }
seaquel-types = { path = "../crates/seaquel-types" }
```

- Remove the dependencies that only the old in-tree drivers used, which now live in the engine crates: `sqlx`, `rust_decimal`, `duckdb`, `uuid`, `async-stream`, `tokio-util` and `base64`.
- Before deleting each one, confirm it's unused: `grep -rn "<crate_name>::" src-tauri/src` must print nothing. Use the underscore form of the name, e.g. `rust_decimal::`, `tokio_util::`, `async_stream::`.
- Keep `futures`, `tokio`, `async-trait` and `time`. The SSH tunnel and logging code use them.

**Step 2: `src-tauri/src/db/mod.rs`**

```rust
//! Tauri-facing DB module.
//!
//! All database work lives in `seaquel-core` so it's shared with the web
//! server and, later, the CLI, TUI and MCP server. This module holds only the
//! Tauri command wrappers.

pub mod commands;
```

**Step 3: `src-tauri/src/db/commands.rs`** — replace the whole file:

```rust
use futures::StreamExt;
use seaquel_core::{Core, StreamEvent};
use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, ExecuteResult, QueryResult,
};
use tauri::{command, ipc::Channel, State};

#[command]
pub async fn db_connect(
    config: ConnectConfig,
    core: State<'_, Core>,
) -> Result<ConnectResult, DbError> {
    core.connect(&config).await
}

#[command]
pub async fn db_query(
    connection_id: String,
    sql: String,
    values: Vec<serde_json::Value>,
    core: State<'_, Core>,
) -> Result<QueryResult, DbError> {
    core.query(&connection_id, &sql, values).await
}

/// Streams results through `on_event`: `batch` events, then `done` or
/// `error`. A cancelled stream just stops. Errors arrive as events rather
/// than as a rejected invoke, so the frontend has one termination path.
#[command]
pub async fn db_query_stream(
    query_id: String,
    connection_id: String,
    sql: String,
    values: Vec<serde_json::Value>,
    on_event: Channel<StreamEvent>,
    core: State<'_, Core>,
) -> Result<(), DbError> {
    let mut events = core.query_stream(query_id, connection_id, sql, values);
    while let Some(event) = events.next().await {
        // Err means the webview dropped the channel. Stop; dropping `events`
        // stops the driver's fetch and releases its connection.
        if on_event.send(event).is_err() {
            break;
        }
    }
    Ok(())
}

#[command]
pub async fn db_cancel_stream(query_id: String, core: State<'_, Core>) -> Result<(), DbError> {
    core.cancel_stream(&query_id);
    Ok(())
}

#[command]
pub async fn db_execute(
    connection_id: String,
    sql: String,
    values: Vec<serde_json::Value>,
    core: State<'_, Core>,
) -> Result<ExecuteResult, DbError> {
    core.execute(&connection_id, &sql, values).await
}

#[command]
pub async fn db_transaction(
    connection_id: String,
    statements: Vec<BatchStatement>,
    core: State<'_, Core>,
) -> Result<(), DbError> {
    core.transaction(&connection_id, statements).await
}

#[command]
pub async fn db_disconnect(connection_id: String, core: State<'_, Core>) -> Result<(), DbError> {
    core.disconnect(&connection_id).await
}

#[command]
pub async fn db_test(config: ConnectConfig, core: State<'_, Core>) -> Result<(), DbError> {
    core.test(&config).await
}
```

This changes one behaviour on purpose. A stream on an unknown connection used to reject the `invoke`. It now sends an `error` event, which is what the web server always did. `unified-tauri-provider.ts` handles both paths and produces the same `"CODE: message"` string, so the UI shows the same error.

**Step 4: Register Core.** In `src-tauri/src/lib.rs`, replace `.manage(db::ConnectionManager::new())` with:

```rust
        .manage(seaquel_core::with_default_plugins().build())
```

**Step 5: Verify**

- Run: `mkdir -p build && cargo check -p seaquel`. Expected: `Finished` with no warnings from `src/db/`. If an "unresolved import"/"can't find crate" error names one of the removed dependencies, something still uses it: put that one back.
- Run: `npm run tauri:dev`, then exercise every DB command. The app's own metadata storage also goes through these commands (`src/lib/storage/tauri-sqlite.ts`), so the app starting at all exercises connect, query and transaction.
  1. The app starts, and saved connections and tabs load.
  2. Connect to Postgres (`npm run e2e:db:up`) and run a query.
  3. Stream a large result (`SELECT generate_series(1, 200000)`), then run it again and click Cancel. It stops.
  4. Edit a cell in the data grid and save. That path goes through `db_transaction`.
  5. "Test connection" with a bad password shows an error.
  6. Disconnect, then reconnect.
  7. Open a DuckDB file and a SQLite file.

**Step 6: Checkpoint.** Suggested commit: `Run the desktop app's DB commands through seaquel-core`.

---

### Task 16: Delete `seaquel-db` and fix the Docker build

**Files:**
- Delete: `crates/seaquel-db/`
- Modify: `Cargo.toml` (members)
- Modify: `Dockerfile` (Rust stage)

**Step 1: Confirm nothing uses it**

Run: `grep -rn "seaquel.db\b\|seaquel_db" --include=*.rs --include=*.toml --include=Dockerfile . | grep -v "^./target" | grep -v "^./node_modules"`
Expected: only `Cargo.toml`'s members line and the `Dockerfile` lines. Anything else still needs moving.

**Step 2: Delete**

```bash
rm -rf crates/seaquel-db
```
Remove `"crates/seaquel-db",` from `members`.

**Step 3: Dockerfile.** The Rust stage hard-codes `crates/seaquel-db` in its dependency-caching trick. It would also need a manual stub for every new crate. Replace everything from `FROM rust:1-bookworm AS rust-builder` down to (and including) the `RUN find crates -name '*.rs' -exec touch {} + … strip target/release/seaquel-server` line with a cargo-chef build. cargo-chef derives the dependency layer from the manifests automatically, and `docker-publish.yml`'s `type=gha` layer cache keeps working.

```dockerfile
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked
WORKDIR /build

# src-tauri is a workspace member, so cargo needs its targets to exist just to
# load the workspace. The web image never builds it, and .dockerignore keeps
# its source out, so stub it.
FROM chef AS rust-planner
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY src-tauri/Cargo.toml src-tauri/build.rs src-tauri/
RUN mkdir -p src-tauri/src && echo "" > src-tauri/src/lib.rs && echo "fn main(){}" > src-tauri/src/main.rs
RUN cargo chef prepare --recipe-path recipe.json

# The cook layer depends only on recipe.json, so it's rebuilt when a manifest
# or Cargo.lock changes, not on every source edit.
FROM chef AS rust-builder
COPY --from=rust-planner /build/recipe.json recipe.json
RUN cargo chef cook --release -p seaquel-server --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY src-tauri/Cargo.toml src-tauri/build.rs src-tauri/
RUN mkdir -p src-tauri/src && echo "" > src-tauri/src/lib.rs && echo "fn main(){}" > src-tauri/src/main.rs
RUN cargo build --release -p seaquel-server \
 && strip target/release/seaquel-server
```

The runtime stage's `COPY --from=rust-builder /build/target/release/seaquel-server ./seaquel-server` stays as it is.

**Step 4: Verify**

- Run: `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`. Expected: everything passes, and the server-engine smoke tests print `skipping`.
- Run: `cargo check -p seaquel`. Expected: `Finished`.
- Run: `docker build --target rust-builder -t seaquel-rust-check .`. Expected: success. This takes several minutes, because bundled DuckDB and libgit2 compile from source.
- Run: `docker build -t seaquel-local . && docker run --rm -p 8787:8787 seaquel-local`. Expected: `curl -s localhost:8787/health` prints `ok`. Stop the container afterwards.

**Step 5: Checkpoint.** Suggested commit: `Remove seaquel-db and build the server image with cargo-chef`.

---

### Task 17: Enforce the crate dependency rules

**Files:**
- Create: `scripts/check-crate-deps.mjs`
- Create: `scripts/check-crate-deps.test.mjs`
- Modify: `package.json` (script)

**Step 1: Tests first** — `scripts/check-crate-deps.test.mjs`:

```js
import { describe, expect, it } from "vitest";
import { checkCrateDeps } from "./check-crate-deps.mjs";

/** A `cargo metadata` package; string deps are normal dependencies. */
const pkg = (name, ...deps) => ({
  name,
  dependencies: deps.map((d) => (typeof d === "string" ? { name: d, kind: null } : d)),
});

describe("checkCrateDeps", () => {
  it("accepts the phase 0 workspace", () => {
    const packages = [
      pkg("seaquel-macros"),
      pkg("seaquel-runtime", "seaquel-macros"),
      pkg("seaquel-types"),
      pkg("seaquel-engine", "seaquel-runtime", "seaquel-types"),
      pkg("seaquel-engine-testkit", "seaquel-engine"),
      pkg("seaquel-engine-sqlite", "seaquel-engine", "seaquel-runtime", {
        name: "seaquel-engine-testkit",
        kind: "dev",
      }),
      pkg("seaquel-core", "seaquel-engine", "seaquel-types", "seaquel-engine-sqlite"),
      pkg("seaquel-server", "seaquel-core", "seaquel-types"),
      pkg("seaquel", "seaquel-core", "seaquel-types"),
    ];
    expect(checkCrateDeps(packages)).toEqual([]);
  });

  it("rejects an engine depending on another engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-postgres", "seaquel-engine-mysql"),
      pkg("seaquel-engine-mysql"),
    ]);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/^seaquel-engine-postgres -> seaquel-engine-mysql/);
  });

  it("rejects a pure crate depending on a native one", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine", "seaquel-engine-sqlite"),
      pkg("seaquel-engine-sqlite"),
    ]);
    expect(errors[0]).toMatch(/pure crates may only depend on other pure crates/);
  });

  it("rejects an interface bypassing Core", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-server", "seaquel-engine-postgres"),
      pkg("seaquel-engine-postgres"),
    ]);
    expect(errors[0]).toMatch(/interfaces reach everything through seaquel-core/);
  });

  it("rejects the testkit naming an engine", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-testkit", "seaquel-engine-sqlite"),
      pkg("seaquel-engine-sqlite"),
    ]);
    expect(errors[0]).toMatch(/EngineRegistry/);
  });

  it("ignores dev-dependencies and third-party crates", () => {
    const errors = checkCrateDeps([
      pkg("seaquel-engine-sqlite", "sqlx", { name: "seaquel-engine-testkit", kind: "dev" }),
      pkg("seaquel-engine-testkit"),
    ]);
    expect(errors).toEqual([]);
  });

  it("requires every crate to be classified", () => {
    expect(checkCrateDeps([pkg("seaquel-storage")])).toEqual([
      "seaquel-storage: unclassified crate. Add it to scripts/check-crate-deps.mjs.",
    ]);
  });
});
```

Run: `npx vitest run scripts/check-crate-deps.test.mjs`
Expected: FAIL (module not found).

**Step 2: Implement** — `scripts/check-crate-deps.mjs`:

```js
#!/usr/bin/env node
/**
 * Enforces the crate dependency rules from
 * docs/plans/2026-09-24-rust-core-plugin-architecture-design.md
 * ("Dependency rules"). Run from the repo root:
 *
 *   node scripts/check-crate-deps.mjs
 *
 * Only normal and build dependencies count; dev-dependencies (tests) are free.
 * Every workspace crate must be classified below, so a new crate fails this
 * check until someone decides which rules apply to it.
 */

import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

/** Must build for wasm32. May only depend on each other. */
const PURE = new Set(["seaquel-macros", "seaquel-runtime", "seaquel-types", "seaquel-engine"]);

/** Registers the default plugins, so it's the one crate allowed to name engines. */
const CORE = new Set(["seaquel-core"]);

/** Thin shells over Core. `seaquel` is the Tauri app in src-tauri/. */
const INTERFACES = new Set(["seaquel", "seaquel-server"]);

/** Engine-agnostic test support. */
const TESTKIT = new Set(["seaquel-engine-testkit"]);

/**
 * Domain and infrastructure crates (seaquel-storage, seaquel-workspace, …)
 * arrive in later phases. List them here as they're created.
 */
const DOMAIN_AND_INFRA = new Set([]);

const ENGINE_MAY_USE = new Set(["seaquel-engine", "seaquel-runtime", "seaquel-types", "seaquel-sql"]);
const INTERFACE_MAY_USE = new Set(["seaquel-core", "seaquel-runtime", "seaquel-types", "seaquel-rpc"]);

const isEngine = (name) => name.startsWith("seaquel-engine-") && !TESTKIT.has(name);

function classify(name) {
  if (PURE.has(name)) return "pure";
  if (CORE.has(name)) return "core";
  if (INTERFACES.has(name)) return "interface";
  if (TESTKIT.has(name)) return "testkit";
  if (isEngine(name)) return "engine";
  if (DOMAIN_AND_INFRA.has(name)) return "domain";
  return null;
}

/**
 * @param {{ name: string, dependencies: { name: string, kind: string | null }[] }[]} packages
 *   `packages` from `cargo metadata --no-deps`.
 * @returns {string[]} One message per violation. Empty means OK.
 */
export function checkCrateDeps(packages) {
  const workspace = new Set(packages.map((p) => p.name));
  const errors = [];

  for (const pkg of packages) {
    const kind = classify(pkg.name);
    if (!kind) {
      errors.push(`${pkg.name}: unclassified crate. Add it to scripts/check-crate-deps.mjs.`);
      continue;
    }
    const deps = pkg.dependencies
      .filter((d) => d.kind !== "dev" && workspace.has(d.name))
      .map((d) => d.name);
    const forbid = (allowed, why) => {
      for (const dep of deps) {
        if (!allowed(dep)) errors.push(`${pkg.name} -> ${dep}: ${why}`);
      }
    };

    switch (kind) {
      case "pure":
        forbid((d) => PURE.has(d), "pure crates may only depend on other pure crates");
        break;
      case "engine":
        forbid(
          (d) => ENGINE_MAY_USE.has(d),
          "engine crates may only depend on seaquel-engine, seaquel-runtime, seaquel-types and seaquel-sql",
        );
        break;
      case "interface":
        forbid((d) => INTERFACE_MAY_USE.has(d), "interfaces reach everything through seaquel-core");
        break;
      case "core":
        break;
      case "testkit":
      case "domain":
        forbid((d) => !isEngine(d), "reach engines through EngineRegistry, never by crate name");
        break;
    }
  }
  return errors;
}

const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  const metadata = execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const { packages } = JSON.parse(metadata);
  const errors = checkCrateDeps(packages);
  if (errors.length > 0) {
    console.error(errors.join("\n"));
    process.exit(1);
  }
  console.log(`crate dependency rules: ${packages.length} crates OK`);
}
```

**Step 3: npm script.** Add `"crates:check": "node scripts/check-crate-deps.mjs",` to `package.json`.

**Step 4: Verify**

- Run: `npx vitest run scripts/check-crate-deps.test.mjs`. Expected: 7 passed.
- Run: `npm run crates:check`. Expected: `crate dependency rules: 13 crates OK`. That's 12 crates under `crates/` plus `src-tauri`.
- Run: `npx oxlint --type-aware --type-check --deny-warnings`. Expected: clean.

**Step 5: Checkpoint.** Suggested commit: `Check crate dependency rules`.

---

### Task 18: CI

Nothing runs tests in CI today. Add one workflow with four jobs: Rust, engines, desktop compile, and frontend.

**Files:**
- Create: `.github/workflows/ci.yml`

**Step 1: `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  rust:
    name: Rust (lint, tests, wasm, crate rules, generated types)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: 24

      - name: Crate dependency rules
        run: node scripts/check-crate-deps.mjs

      - name: Clippy
        run: cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings

      - name: Tests
        run: cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio

      - name: Pure crates build for wasm32
        run: cargo clippy --target wasm32-unknown-unknown -p seaquel-types -p seaquel-runtime -p seaquel-engine -- -D warnings

      - name: Generated TypeScript types are up to date
        run: |
          cargo test -p seaquel-types --features ts --lib export_bindings --quiet
          if [ -n "$(git status --porcelain -- src/lib/types/generated)" ]; then
            git status --porcelain -- src/lib/types/generated
            echo "::error::src/lib/types/generated is stale. Run 'npm run types:gen' and commit the result."
            exit 1
          fi

  engines:
    name: Engine smoke tests
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_HOST_AUTH_METHOD: trust
          POSTGRES_DB: seaquel_test
        ports: ["5432:5432"]
        options: >-
          --health-cmd "pg_isready -U postgres"
          --health-interval 5s --health-timeout 5s --health-retries 20
      mysql:
        image: mysql:8
        env:
          MYSQL_ALLOW_EMPTY_PASSWORD: "yes"
          MYSQL_DATABASE: seaquel_test
        ports: ["3306:3306"]
        options: >-
          --health-cmd "mysqladmin ping -h 127.0.0.1"
          --health-interval 5s --health-timeout 5s --health-retries 20
      mariadb:
        image: mariadb:11
        env:
          MARIADB_ALLOW_EMPTY_ROOT_PASSWORD: "yes"
          MARIADB_DATABASE: seaquel_test
        ports: ["3307:3306"]
        options: >-
          --health-cmd "healthcheck.sh --connect --innodb_initialized"
          --health-interval 5s --health-timeout 5s --health-retries 20
      mssql:
        image: mcr.microsoft.com/mssql/server:2022-latest
        env:
          ACCEPT_EULA: "Y"
          MSSQL_SA_PASSWORD: Seaquel_Test_123!
          MSSQL_PID: Developer
        ports: ["1433:1433"]
        options: >-
          --health-cmd "/opt/mssql-tools18/bin/sqlcmd -S localhost -U sa -P Seaquel_Test_123! -C -Q 'SELECT 1'"
          --health-interval 10s --health-timeout 10s --health-retries 30 --health-start-period 30s
    env:
      SEAQUEL_TEST_REQUIRE_ENGINES: "1"
      SEAQUEL_TEST_POSTGRES: '{"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}'
      SEAQUEL_TEST_MYSQL: '{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}'
      SEAQUEL_TEST_MARIADB: '{"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}'
      SEAQUEL_TEST_MSSQL: '{"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa","password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}'
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Smoke tests
        run: >-
          cargo test --test smoke
          -p seaquel-engine-postgres -p seaquel-engine-mysql -p seaquel-engine-mssql
          -p seaquel-engine-sqlite -p seaquel-engine-duckdb

  desktop:
    name: Desktop app compiles
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Tauri system dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
      - name: Placeholder frontend
        # tauri::generate_context! requires frontendDist (../build) to exist.
        run: mkdir -p build && touch build/index.html
      - run: cargo check -p seaquel

  frontend:
    name: Frontend (types, lint, tests)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      # `npm run check` regenerates the wire types with cargo first.
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - uses: actions/setup-node@v4
        with:
          node-version: 24
          cache: npm
      - run: npm ci
      - name: Compile paraglide messages
        # Normally generated by the Vite plugin; svelte-check imports them.
        run: npx paraglide-js compile --project ./project.inlang --outdir ./src/lib/paraglide --strategy localStorage cookie globalVariable baseLocale
      - run: npm run check
      - name: Lint
        run: npx oxlint --type-aware --type-check --deny-warnings
      - run: npx vitest run
```

**Step 2: Check it locally as far as possible**

- Run: `npx --yes @action-validator/cli .github/workflows/ci.yml` if it's available, or paste the file into an online YAML linter. Expected: valid.
- Run every `run:` command from the `rust` and `frontend` jobs locally, in order. Each must pass.
- Run the engines job locally with `npm run e2e:db:up` and the same environment variables. For local Postgres and MySQL, first run `npm run e2e:db:seed -- postgresql mysql mariadb`, which creates `seaquel_test`.

**Step 3: Checkpoint.** Tell the user CI only proves itself on GitHub. After they push, all four jobs must go green. The fragile spots:
- The MSSQL service health check: its quoting inside `options`.
- `paraglide-js compile` flags: if svelte-check reports missing `$lib/paraglide/*` modules, the step's output layout is off.

Suggested commit: `Add CI workflow`.

---

### Task 19: Docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` (status line only)

**Step 1: `CLAUDE.md`.** Replace the `### Backend (src-tauri/)` section with:

```markdown
### Backend (Rust)

All database logic lives in Rust crates under `crates/`, shared by every interface. See `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` for where this is heading.

- `seaquel-core` — the only entry point interfaces use: engine registry, open connections, streaming and cancellation.
- `seaquel-engine` — the `Driver`/`Engine` plugin traits. One crate per engine: `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}`.
- `seaquel-types` — wire types. `npm run types:gen` regenerates `src/lib/types/generated/`; never edit those by hand.
- `seaquel-runtime` — `MaybeSend`, `BoxStream`, `Executor`, `#[seaquel_runtime::async_trait]`. Core crates must build for wasm32: no `tokio::spawn`, `Instant` or `SystemTime` (enforced by `crates/clippy.toml`).
- Interfaces: `src-tauri/` (desktop; Tauri commands in `src/db/commands.rs` forward to Core) and `crates/seaquel-server/` (web; axum, loopback-only behind the Node server).
- `node scripts/check-crate-deps.mjs` (`npm run crates:check`) enforces which crates may depend on which.
- Engine smoke tests: `cargo test -p seaquel-engine-<name> --test smoke`. Server engines need `SEAQUEL_TEST_<ENGINE>` set to ConnectConfig JSON (see each crate's `tests/smoke.rs`) and `npm run e2e:db:up`.
```

Also fix the stale `tauri-plugin-sql` mention in "Project Overview": PostgreSQL, MySQL/MariaDB, SQLite, MSSQL and DuckDB are supported through the Rust engine crates.

**Step 2: Design doc.** Under `**Status:** Draft`, add a line: `Phase 0: implemented (see 2026-09-24-rust-core-phase-0-plan.md).` Only add it if every task above is done.

**Step 3: Checkpoint.** Suggested commit: `Document the Rust core layout`.

---

### Task 20: Final verification

Run everything once more from a clean state and report the results. Don't claim anything that wasn't observed (@superpowers:verification-before-completion).

```bash
cargo clean -p seaquel-core   # force Core and its dependants to rebuild
node scripts/check-crate-deps.mjs
cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings
cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio
cargo clippy --target wasm32-unknown-unknown -p seaquel-types -p seaquel-runtime -p seaquel-engine -- -D warnings
cargo check -p seaquel
npm run check
npx oxlint --type-aware --type-check --deny-warnings
npx vitest run
```

Expected:
- every command exits 0
- `vitest` reports 192 tests (185 + 7)
- `svelte-check` reports 0 errors

With Docker up, also run the engine smoke tests with all four `SEAQUEL_TEST_*` variables and `SEAQUEL_TEST_REQUIRE_ENGINES=1`.

Then run the desktop checks from Task 15 Step 5 and the web checks from Task 14 Step 7 one last time.

Report to the user:
- each command and its result
- the two intentional behaviour changes: stream errors on an unknown connection are now events in Tauri, and serialization failures are now errors on the web
- that CI is unproven until pushed

---

## Execution notes (2026-09-24)

The plan was executed task by task with a review after each. Where the result departs from the code above, the repo is authoritative:

- **Task 1.** layerchart 2.5 also changed `seriesLayout`'s default to `auto`, which stacks multi-series bar and area charts. The components set `seriesLayout="overlap"` to keep the old look. Bar, line and area charts never had a legend, so none was added.
- **Tasks 11–12.** The MSSQL and DuckDB crates have no `decode` module. MSSQL also needs a direct `chrono` dependency with the `alloc` feature, which `seaquel-db` only got through duckdb's feature unification.
- **Task 13, after review.** Core changed in three ways:
  - `disconnect` now cancels the connection's running streams before closing the driver. Before this, a Postgres disconnect waited for a running stream to finish (3.3 s for 2M rows), or hung if the caller held the stream without polling it.
  - Streams cut off by a disconnect end with a `CONNECTION_CLOSED` error event. Streams the client cancelled still end silently. The desktop frontend only finishes on a terminal event, so a silent end would have hung the UI.
  - `query_stream` wraps the driver stream in `take_until`, so cancel is prompt for MSSQL too. It doesn't help DuckDB: its driver blocks the thread rather than awaiting, so a DuckDB query still runs to completion before Core drops the result. Fixing that is phase 1 work.
  - New tests are in `crates/seaquel-core/tests/mock.rs`.
- **Task 14.** `uuid` moved to the server's dev-dependencies, and the unused `thiserror` was dropped.
- **Task 15.** `src-tauri/src/lib.rs` adds `.level_for("seaquel_core", Trace)`, because Core now emits the DB debug logs under its own target. The unused `thiserror` was dropped from `src-tauri`.
- **Manual checks outstanding.** The GUI checks (Task 1 Step 4, Task 14 Step 7, Task 15 Step 5) and the first CI run on GitHub still need a person. Add one more check: disconnect during a large stream and confirm the UI shows `CONNECTION_CLOSED`.
- **Carry into phase 1** (from the final review):
  - DuckDB needs a blocking-work hook. `Executor` has only `spawn`, `sleep` and `unix_time`, and there's no wasm equivalent.
  - Core doesn't take an `Executor` yet, and no interface enables `seaquel-runtime/tokio`.
  - Core won't build for wasm32 as-is: `uuid` v4 needs getrandom's js backend, and the default features pull in native engines.
  - The design doc's trait sketch uses the names `Connection` and `aliases()`. Phase 0 shipped `Driver`, `Arc<dyn Driver>` and `close() -> Result`.
  - Core's stream map is keyed by the client's query id alone. Key it by (connection, query id) once `seaquel-rpc` scopes calls per tenant.
  - `ENGINE_NOT_AVAILABLE` maps to HTTP 500 in `seaquel-server`. Use 400 or 501 once slim builds exist.
  - CI has no `cargo fmt --check`, and the new crates aren't rustfmt-clean.
  - layerchart's range in `package.json` is still `^2.0.0-next.44`, while the charts now need 2.5 APIs. Bump it together with a lockfile refresh.
  - `src/lib/storage/tauri-sqlite.ts` still declares its own copies of the wire types.
