# Rust Core and Plugin Architecture

**Date:** 2026-09-24
**Status:** Draft
Phase 0: implemented (see 2026-09-24-rust-core-phase-0-plan.md).
Phase 1: implemented (see 2026-09-25-rust-core-phase-1-plan.md). Its measured
cost and the phase 2 estimate are in "Phase 1 cost" below.
Phase 2: implemented for the engines (see 2026-09-26-rust-core-phase-2-plan.md).
All five engines run in Rust on desktop and web; the demo keeps `duckdb.ts`.
`seaquel-sql`, `seaquel-wasm` and the parser switch moved to phase 2b. Phase
2's measured cost and the phase 2b estimate are in "Phase 2 cost" below.
Phase 2b: implemented (see 2026-09-27-rust-core-phase-2b-plan.md). The editor,
query runner, Visual tab, query builder, tutorial, table editor and the AI's
read-only check call `seaquel-sql` through `seaquel-wasm` on desktop, web and
the demo. node-sql-parser and the TypeScript scanners are gone. Its measured
cost, and the case for doing the two AI read-only Follow-ups before phase 3,
are in "Phase 2b cost" below.
AI safety: implemented (see 2026-09-28-ai-safety-plan.md). Every query the AI
runs, and every dashboard widget, goes through Core's `query_stream` with
`read_only` and a per-engine `Driver::query_read_only`; AI tool calls run on
their chat's connection and can be stopped. Its measured cost is in "AI safety
cost" below.
Phase 3: implemented (see 2026-09-29-rust-core-phase-3-plan.md). Metadata
storage, secrets, SSH tunnels, git and licensing live in `seaquel-storage`,
`seaquel-secrets`, `seaquel-ssh`, `seaquel-git` and `seaquel-license` behind
Core, served over the workspace RPC (`core_call` on desktop, `/rpc` and
`/internal/license/*` on web). No SQL crosses from the webview to its own
database, the legacy JSON import is gone, and Core builds for wasm32 with
the `browser` feature. SQLite and DuckDB connections are off on web
(Decision 11b of that plan). Its measured cost is in "Phase 3 cost" below.
Phase 4: implemented (see 2026-09-30-rust-core-phase-4-plan.md). The desktop
app bundles `seaquel-cli` as a sidecar; `seaquel-cli mcp` is a read-only MCP
server over stdio (`seaquel-mcp`) that opens the app's `seaquel.db` read-only,
reads its keychain entries and connects saved connections through
`Workspace::connect_saved` (`seaquel-workspace`, a port of the GUI's connect
path). Only connections named on its command line are exposed. Core gained
row, byte and time limits on read-only queries and a read-only EXPLAIN. The
GUI still connects through TypeScript, and nothing writes storage from a
second process yet. Its measured cost is in "Phase 4 cost" below.
Phase 5a: implemented (see 2026-10-01-rust-core-phase-5a-plan.md). The
desktop and web GUIs connect, test, query and disconnect through Core's
workspace (`Workspace::connect`/`test` with a saved id or a form, and the
`db` RPC group), over `core_call`/`core_stream`/`core_events` on desktop and
`/rpc` plus a multiplexed `/rpc/stream` WebSocket on web. The TypeScript no
longer builds connect configs, opens tunnels or scopes ids per user; each
connection and stream belongs to the workspace that opened it, and Core
refuses the rest. `/api/db/*` and the `db_*` commands are gone. The recorded
connect quirks are fixed (the v2 fixtures), web connections are capped per
user, and a stopped stream stops on the server. The demo is unchanged. Its
measured cost is in "Phase 5a cost" below.

## Problem

Seaquel is one app with three front ends (desktop, web, demo), and most of what
it knows lives in TypeScript inside the Svelte app. We want more interfaces: a
CLI, a TUI and an MCP server. None of them can reuse that TypeScript.

The split is worst for database engines. Postgres support today is:

- `crates/seaquel-db/src/postgres.rs`: about 30 lines of Rust that open a sqlx
  pool, plus the shared row decoder.
- `src/lib/db/postgres.ts`: 473 lines of TypeScript with the introspection SQL,
  result parsing, EXPLAIN parsing, DDL generation, CRUD SQL and quoting. Shared
  helpers (`alter-table.ts`, `crud-helpers.ts`, `parse-create-table.ts`,
  `column-sources.ts`) add more. (As built: `postgres.ts` went into
  `seaquel-engine-postgres` in phase 1. `alter-table.ts` and `crud-helpers.ts`
  became the generic builders in `seaquel-engine` (`ddl.rs`, `crud.rs`), and
  the TS copies stay only for the demo's `duckdb.ts`. `parse-create-table.ts`
  and `column-sources.ts` went into `seaquel-sql` in phase 2b.)

Most engine bugs land in the TypeScript half. A Rust CLI would only get the
driver half, so a Postgres fix wouldn't reach it.

The same goes for the rest of the app. About 48k lines of non-UI TypeScript sit
in `src/lib`. That includes the 14k-line `hooks/database/` layer, where query
execution, projects, shared repos, dashboards and workflows are mixed in with
tabs and panes. Git, SSH tunnels and license activation live in `src-tauri`, so
not even the web server can use them.

## Goal

Rust is the core language. Everything except interface-specific code lives in
Rust crates: the Svelte GUI, the Tauri shell and the web auth layer are the
only exceptions. Each capability is a plugin crate that can be built and
tested on its own. Seaquel Core registers the plugins and orchestrates them.
Every interface calls Core, so a fix in the Postgres plugin reaches desktop,
web, CLI, TUI and MCP in the same release.

## Decisions

| # | Decision | Choice |
|---|---|---|
| 1 | Core language | Rust. CLI, TUI and MCP server are Rust too |
| 2 | Plugin mechanism | Compile-time crates behind traits, registered explicitly when Core is built. No runtime loading in this design |
| 3 | Engine boundary | One crate per engine, owning driver, dialect and introspection. The GUI never builds engine-specific SQL again |
| 4 | Pure vs I/O code | Pure crates (types, SQL tooling, dialects) must compile to `wasm32-unknown-unknown`. CI enforces this |
| 5 | GUI hot paths | The Svelte app loads pure crates as a WASM module for synchronous, keystroke-rate work (quoting, statement splitting, query builder sync). Everything else goes through IPC/HTTP |
| 6 | One API for GUIs | A single RPC surface (`seaquel-rpc`) served over Tauri IPC and over HTTP/WebSocket. TS types are generated from Rust |
| 7 | Multi-tenancy | Core has a process-wide `Core` and per-user `Workspace`s. Web gets one workspace per user; desktop, CLI, TUI and MCP get one each |
| 8 | GUI state | Tabs, panes and layout stay in the interface. Core persists them as opaque per-interface blobs and never parses them |
| 9 | Web auth | Better Auth, signup, team and account routes stay in the SvelteKit/Node layer. They're web-specific. Replacing Node with an axum auth layer is out of scope |
| 10 | Terminal binaries | One `seaquel` binary: `seaquel <cmd>` for CLI, `seaquel tui`, `seaquel mcp` |
| 11 | Migration | Strangler pattern, one engine or subsystem at a time. The app ships working at every step |
| 12 | Demo | Core compiles to WASM for the browser, with a JS-bridged DuckDB-WASM engine and an in-browser storage backend. The demo becomes a third RPC transport |
| 13 | Web licensing | Licensing and air-gap logic move to `seaquel-license`. Node's gate calls `seaquel-server` over loopback |
| 14 | Terminal binary licensing | Honour system. `seaquel` doesn't check for a license key |
| 15 | CLI distribution | Bundled with the desktop app as a sidecar. Standalone distribution can be added later without design changes |
| 16 | Legacy JSON storage | Dropped. Core doesn't import the pre-SQLite JSON files from versions before 2026.4.5 |

### Why compile-time plugins

"Plugin" here means an independently developed and tested crate behind a
trait, not a `.dylib` loaded at runtime. Rust has no stable ABI, so native
dynamic plugins break on every compiler upgrade. The alternatives (WASM
components via wasmtime, or subprocess plugins over JSON-RPC) cost real
complexity, and nobody outside Webstone is writing Seaquel plugins yet. Keep the
traits clean enough that a WASM-component host could implement them later. If
third-party plugins become a goal, that's a separate design.

## Architecture

```
                 interfaces (thin)
 ┌──────────────┬───────────────┬────────────┬───────────┬───────────┐
 │ src-tauri    │ seaquel-server│ seaquel-cli│seaquel-tui│seaquel-mcp│
 │ (desktop)    │ (web, axum)   │            │ (ratatui) │  (rmcp)   │
 └──────┬───────┴───────┬───────┴─────┬──────┴─────┬─────┴─────┬─────┘
        │   seaquel-rpc │             │            │           │
        └───────┬───────┘             └────────────┼───────────┘
                ▼                                  ▼
 ┌─────────────────────────────── seaquel-core ─────────────────────────────┐
 │ Core (plugin registry, connection pool, tunnels, running queries, events)│
 │ Workspace (per user: storage, secrets, projects, execution services)     │
 └─────────────────────────────────────┬────────────────────────────────────┘
          domain                        │                 infrastructure
 ┌──────────────────┬──────────┐        │   ┌───────────┬─────────┬────────┬────────┬─────────┐
 │ seaquel-workspace│seaquel-ai│        │   │ -storage  │-secrets │ -ssh   │ -git   │-license │
 └──────────────────┴──────────┘        │   └───────────┴─────────┴────────┴────────┴─────────┘
          engines (plugins)             ▼                  pure (wasm-clean)
 ┌──────────┬───────┬────────┬───────┬────────┐   ┌───────────────┬─────────────┬───────────────┐
 │ postgres │ mysql │ sqlite │ mssql │ duckdb │   │ seaquel-types │ seaquel-sql │ engine dialect│
 └────┬─────┴───┬───┴────┬───┴───┬───┴────┬───┘   └───────────────┴─────────────┴───────────────┘
      └─────────┴─ seaquel-engine (traits) + seaquel-engine-testkit
```

The Svelte GUI talks to `seaquel-rpc` (via Tauri IPC on desktop, HTTP/WS on
web) and loads `seaquel-wasm` for pure hot-path functions.

### Crates

| Crate | Kind | Owns | Moves from |
|---|---|---|---|
| `seaquel-types` | pure | Serde DTOs (`SchemaTable`, `QueryResult`, `Value`, `SavedQuery`, …) plus TS codegen (specta or ts-rs) | `src/lib/types/*`, `src/lib/types.ts` |
| `seaquel-sql` | pure | Built in phase 2b. One hand scanner that follows each engine's quoting, with statement splitting, statement at cursor, the row-limit check and count query on it; query type, the destructive-statement check and the source table for inline editing; the AI's read-only check; `{{param}}` extraction and substitution; the `CREATE TABLE` parser for the table editor; sqlparser-rs AST helpers (query builder and tutorial parse, Visual tab AST, column references) | Moved: `db/sql-parser.ts`, `engine/sql-scan.ts`, `db/query-utils.ts`, `db/query-params.ts` (all but `createDefaultParameters`, `coerceValue` and `ParameterSubstitutionError`, now in `src/lib/sql/parameters.ts`), `services/ai/context.ts` (`validateReadOnlyQuery`), `db/parse-create-table.ts`, `db/sql-ast-parser.ts`, `tutorial/sql-parser.ts`, and the SQL half of `db/column-sources.ts` (the primary-key lookup stays in `src/lib/sql`). Stayed in TS: `tutorial/criteria.ts`, the builder hooks (`hooks/query-builder-*.ts`, including `buildSql`), and `describePendingChange` (now `hooks/database/pending-change-description.ts`) |
| `seaquel-engine` | pure + async traits | `Engine`, `Driver`, `Dialect` traits, generic DDL/CRUD builders, `EngineRegistry`, `DbError` | `crates/seaquel-db/src/lib.rs`, `db/index.ts` |
| `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}` | plugin | Driver, value decoding, dialect, introspection, EXPLAIN parsing. The `mysql` crate also registers `mariadb` | `crates/seaquel-db/src/*`, `db/{postgres,mysql,sqlite,mssql,duckdb}.ts`, `db/alter-table.ts`, `db/crud-helpers.ts` (as built: the generic builders are in `seaquel-engine`; `duckdb.ts`, `alter-table.ts` and `crud-helpers.ts` stay for the demo until phase 8; `db/parse-create-table.ts` went to `seaquel-sql`) |
| `seaquel-engine-testkit` | dev | Conformance suite every engine must pass | new |
| `seaquel-storage` | infra | App metadata SQLite: schema, migrations, repos, data dir resolution. Native (sqlx) and browser backends | `storage/*`, `src/lib/server/storage.ts`, `storage-guard.ts` |
| `seaquel-secrets` | infra | `SecretStore` trait, OS keychain implementation (`keyring` crate) | `services/keyring.ts` |
| `seaquel-ssh` | infra | SSH tunnels (russh) | `src-tauri/src/ssh_tunnel.rs`, `services/ssh-tunnel.ts` |
| `seaquel-git` | infra | git2 operations and credential chain | `src-tauri/src/git.rs`, `services/git.ts` |
| `seaquel-license` | infra | Desktop activation/validation, control-plane client, grace-period cache, install id, member binding, air-gap bundle verification | `src-tauri/src/license.rs`, `src/lib/server/{licensing,license-cache,member-license,install}.ts`, `src/lib/server/airgap/*` |
| `seaquel-workspace` | domain | Connections, projects, labels, saved queries and versions, history, dashboards and versions, workflows, shared repo file format (`.seaquel/` YAML and frontmatter SQL), importers, exporters, connection strings | `hooks/database/*` (core parts), `services/*-parser.ts`, `services/{dbeaver,tableplus}-import.ts`, `utils/{connection-string,export-formats,query-versions,dashboard-versions,cell-type}.ts` |
| `seaquel-ai` | domain | LLM provider clients, tool registry, tool loop, @mention expansion | `services/ai/*`, `services/ai-mentions.ts` |
| `seaquel-core` | orchestrator | `Core`, `Workspace`, execution services, event bus | `hooks/database.svelte.ts` and managers (logic only) |
| `seaquel-rpc` | interface glue | Request/response/event enums and a dispatcher onto `Workspace` | new; replaces `providers/*` and `/api/storage/*` |
| `seaquel-wasm` | interface glue | wasm-bindgen exports of pure crates for the GUI | new |
| `seaquel-engine-duckdb-wasm` | plugin (browser only) | `Driver` implemented over `@duckdb/duckdb-wasm` via wasm-bindgen. Reuses the DuckDB `Dialect` | `providers/duckdb-provider.ts` |
| `seaquel-browser` | interface glue | Core built for `wasm32-unknown-unknown` with browser plugins, exporting the RPC dispatcher | new |
| `src-tauri`, `seaquel-server`, `seaquel-cli`, `seaquel-tui`, `seaquel-mcp` | interfaces | Transport, presentation, platform integration | existing plus new |

`seaquel-db` disappears. Its contents split between `seaquel-engine` and the
engine crates.

### Dependency rules

These rules keep plugins independent. A CI script checks them against
`cargo metadata`.

1. Engine crates depend only on `seaquel-engine`, `seaquel-types` and
   `seaquel-sql`. An engine never depends on another engine.
2. Domain and infrastructure crates never name an engine crate. They reach
   engines through `EngineRegistry`.
3. Interfaces depend on `seaquel-core` (and `seaquel-rpc` where they serve
   GUIs). They don't reach into domain crates directly.
4. Pure crates don't depend on tokio networking, sqlx, the filesystem or
   anything that fails `cargo check --target wasm32-unknown-unknown`.

### Plugin kinds

Engines are the main plugin kind, but not the only one. Each kind is a trait in
the crate that consumes it, with implementations registered on the builder.

| Kind | Trait | Implementations |
|---|---|---|
| Database engine | `Engine` | postgres, mysql/mariadb, sqlite, mssql, duckdb |
| Connection importer | `Importer` | DBeaver, TablePlus |
| Result exporter | `Exporter` | CSV, JSON, SQL inserts, Markdown (whatever `export-formats.ts` has today) |
| AI provider | `LlmProvider` | Anthropic, OpenAI-compatible |
| Secret store | `SecretStore` | OS keychain (desktop, CLI, TUI, MCP), per-request (web) |

```rust
let core = Core::builder()
    .engine(seaquel_engine_postgres::engine())
    .engine(seaquel_engine_mysql::engine())
    .importer(seaquel_workspace::import::dbeaver())
    .llm_provider(seaquel_ai::anthropic())
    .build();

// or, for binaries that want everything enabled by Cargo features:
let core = seaquel_core::with_default_plugins().build();
```

Each engine is a Cargo feature of `seaquel-core` (`engine-postgres`, …), all on
by default. The web image can drop DuckDB, and a slim CLI build can ship
without MSSQL.

(As built in phase 3: the web image drops both SQLite and DuckDB, because a
file engine on a server lets any signed-in user read and write the server's
files. Features alone didn't hold that rule, since Cargo unifies features
across a workspace build, so `seaquel_core::with_plugins(|id| …)` registers
engines by id and the server allows only Postgres, MySQL and MSSQL.)

## The engine plugin

The TS `DatabaseAdapter` returns SQL strings and parses rows the caller
fetched. The Rust engine does its own I/O and returns typed results. Callers
ask for tables, not for the query that lists tables.

The traits as shipped in phases 0 and 1 (`crates/seaquel-engine`). The first
draft called the connection trait `Connection` and gave `Engine` an
`aliases()` list and a `Capabilities` struct; neither exists. MariaDB is its own
engine id served by the mysql crate, and missing features are `NOT_SUPPORTED`
errors from default methods.

```rust
#[seaquel_runtime::async_trait]
pub trait Engine: MaybeSend + MaybeSync {
    fn id(&self) -> &'static str;                // "postgres"
    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError>;
    fn dialect(&self) -> Option<&dyn Dialect> { None } // Some once the dialect is in Rust
}

/// Pure. No I/O, compiles to WASM.
pub trait Dialect: MaybeSend + MaybeSync {
    fn quote_ident(&self, id: &str) -> String;
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String;
    fn build_update(&self, schema: &str, table: &str, column: &str, value: Value,
                    pks: &[String], row: &RowValues, casts: Option<&CastMap>) -> SqlWithBindings;
    fn build_set_default(&self, /* … */) -> SqlWithBindings;
    fn build_insert(&self, schema: &str, table: &str, values: &[(String, Value)],
                    casts: Option<&CastMap>) -> SqlWithBindings;
    fn build_delete(&self, /* … */) -> SqlWithBindings;
    fn create_table(&self, def: &CreateTableDefinition) -> String;
    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String;
    fn column_types(&self) -> Vec<ColumnTypeInfo>;
    fn explain_sql(&self, sql: &str, analyze: bool) -> String;
    // Phase 2b adds parse_create_table and sql_dialect with seaquel-sql.
}

#[seaquel_runtime::async_trait]
pub trait Driver: MaybeSend + MaybeSync {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError>;
    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError>;
    async fn transaction(&self, stmts: Vec<BatchStatement>) -> Result<(), DbError>; // default: TRANSACTION_NOT_SUPPORTED
    fn query_stream(&self, sql: String, params: Vec<Value>, cancel: CancellationToken)
        -> BoxStream<'_, Result<StreamBatch, DbError>>;
    async fn close(&self) -> Result<(), DbError>;

    // Introspection: the engine picks the SQL. Defaults return NOT_SUPPORTED.
    async fn list_schemas(&self) -> Result<Vec<String>, DbError>;
    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError>;
    async fn table_metadata(&self, schema: &str, table: &str)
        -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError>;
    async fn statistics(&self) -> Result<DatabaseStatistics, DbError>;
    async fn explain(&self, sql: &str, params: Vec<Value>, analyze: bool) -> Result<ExplainResult, DbError>;
}
```

EXPLAIN parsing sits on the driver, next to the query it runs, not on the
dialect. The generic builders in `seaquel-engine` (`ddl.rs`, `crud.rs`) are
ports of `alter-table.ts` and the parameterized half of `crud-helpers.ts`; a
dialect passes them its quote function, placeholder style and options.

Things this fixes along the way, which are cheaper to do now than after five
engines implement the trait:

- **Typed values.** Cells are plain `serde_json::Value` today. Bigints lose
  precision in the browser, bytea arrives as a number array (base64 for MSSQL),
  and DuckDB lists/structs become Debug strings. `seaquel_types::Value` is an
  enum with one wire encoding. The CLI needs types to format output anyway.
  Phase 1 shipped a smaller enum than first sketched: `Null`, `Bool`, `Int`
  (i64), `Float`, `Decimal` (exact text), `Text`, `Bytes`, `Json`, `Array`.
  Dates, times, UUIDs and other types stay `Text` as before. On the wire, values
  JavaScript holds exactly are plain JSON, and the rest are tagged
  `{"$sq": kind, "v": …}` (`bigint`, `float` for NaN/±inf, `decimal`, `bytes`
  as base64, `json`). The TS providers decode tags into `bigint`, `Uint8Array`
  and `SqlDecimal` before the UI sees a row. The full table is in the phase 1
  plan under "Value wire format"; the code is `seaquel-types/src/value.rs` and
  `src/lib/values.ts`. All five engines produce `Value` now. Postgres decoded
  natively in phase 1, and the other four since phase 2.
- **Parameter binding.** `impl_sqlx_driver!` binds every number as `f64`, so
  large integer keys get corrupted in `WHERE pk = $1`. Typed `Value` fixes that.
- **Cancellation.** Replace the `AtomicBool` map on `ConnectionManager` with a
  `CancellationToken` passed into `query_stream`. Engines that can cancel
  server-side (`pg_cancel_backend`, MSSQL attention) do so. Others stop at the
  next batch as today.
- **Connection lifecycle.** `ConnectionManager` has no connect/disconnect;
  `src-tauri` and `seaquel-server` each re-implement it. That moves into Core.
- **MSSQL transactions and DuckDB blocking.** MSSQL returns
  `TRANSACTION_NOT_SUPPORTED`, and DuckDB runs blocking calls on the async
  runtime. The conformance suite will flag both; fix them in the port.
  (Both were fixed in phase 2: MSSQL has transactions, and DuckDB runs its
  calls in `spawn_blocking`.)

## Core, workspaces and state

```rust
pub struct Core { /* registry, open connections, tunnels, running queries, event bus */ }
pub struct Workspace { /* storage, SecretStore, owner id, services below */ }

impl Core {
    pub async fn workspace(&self, spec: WorkspaceSpec) -> Result<Workspace, CoreError>;
}

impl Workspace {
    pub fn connections(&self) -> &ConnectionService;  // CRUD, test, connect (tunnel → open → schema load)
    pub fn queries(&self) -> &QueryService;           // execute, stream, cancel, paginate, pending changes
    pub fn projects(&self) -> &ProjectService;        // projects, labels, saved queries, versions, history
    pub fn shared_repos(&self) -> &SharedRepoService; // clone/pull/push, .seaquel/ file format
    pub fn dashboards(&self) -> &DashboardService;
    pub fn workflows(&self) -> &WorkflowService;      // node graph model + execution
    pub fn ai(&self) -> &AiService;
    pub fn ui_state(&self, interface: &str) -> &UiStateStore; // opaque blobs
    pub fn events(&self) -> broadcast::Receiver<CoreEvent>;
}
```

- **Desktop, CLI, TUI, MCP** open one workspace backed by
  `${dataDir}/seaquel.db` and the OS keychain. They share the data dir, so a
  connection saved in the GUI shows up in `seaquel conn list`.
- **Web** opens a workspace per user, backed by
  `${DATA_DIR}/users/<id>/meta.db`. Connection ownership is checked in Core, so
  the `userId:rustId` rewriting in `shared/connection-scope.js` and
  `routes/api/db/[...path]` goes away.
- **Events** carry connection status, query progress, AI tokens, repo status
  and "data changed" notices. The GUI subscribes over Tauri channels or the
  WebSocket, and the TUI subscribes directly.
- **GUI state.** Tabs, panes, open dashboards and the query builder canvas
  layout are interface concerns. The `tabs` and `project_state` tables become a
  `ui_state(interface, project_id, blob)` table. The desktop and web GUIs share
  the `"gui"` key, and the TUI gets its own.

### Storage ownership

Core becomes the only thing that writes app storage. Today there are three
backends (Tauri IPC SQLite, `/api/storage/*` with better-sqlite3, sql.js in
localStorage) and client-side write queues. After the move:

- `seaquel-storage` uses sqlx SQLite in WAL mode with a busy timeout. Several
  processes can open the same file (the desktop app and `seaquel mcp` running in
  Claude Desktop).
- Migrations are numbered SQL files embedded with `sqlx::migrate!`. The
  inline-column-migration logic in `schema.ts` becomes migration 0001, which
  must be a no-op on existing v4 databases.
- `storage-guard.ts` and the generic `/api/storage/{query,exec,transaction}`
  routes are deleted. The web client can't send arbitrary SQL to its metadata DB
  anymore, which is a security improvement.
- Cross-process change detection: Core polls `PRAGMA data_version` and emits
  `CoreEvent::StorageChanged` so a running GUI picks up a query saved from the
  CLI.
- Data dir resolution moves from `get_data_dir` in `src-tauri` into
  `seaquel-storage`. It honours `SEAQUEL_DATA_DIR` first, then computes the
  platform app-data path for `app.seaquel.desktop` (or `app.seaquel.desktop.dev`
  in dev builds) with the `directories` crate. It has to produce exactly the path
  Tauri's `app_data_dir` gives today, so the CLI finds the desktop app's data
  without Tauri. A unit test pins this per platform.
- **No legacy JSON import.** Versions before 2026.4.5 kept data in
  `tauri-plugin-store` JSON files, and `json-migration.ts` imports them on first
  launch. Core doesn't port this. If Core finds the legacy files and no
  `seaquel.db`, it refuses to start and names the fix: install any release from
  2026.4.5 up to the last pre-Core release, launch it once, then upgrade.
  `json-migration.ts`, `legacy.ts`, `tauri-storage.ts`, `web-storage.ts` and
  the `tauri-plugin-store` dependency are deleted in phase 3.

As built in phase 3 (details in that plan's Decisions 2–6 and findings):

- **No migration 0001 for the inline upgrades.** They became a frozen
  baseline (`schema.rs`) that runs on every open, creates missing tables
  before adding columns (which also opens `v2026.4.5-beta.1` files again)
  and upgrades every file any release wrote. Numbered SQL migrations run
  after it and start empty. They must be expand-only, because releases
  2026.4.5–2026.9.x open files a newer build changed and the migrator
  ignores versions it doesn't know (`set_ignore_missing(true)`).
- **Data steps for data cleanups.** A rewrite that needs Rust logic is a
  data step (`data_steps.rs`, recorded in `_seaquel_data_steps`), not a SQL
  migration: the first, stripping passwords from stored connection strings,
  was quadratic as SQL and couldn't have been fixed after shipping, since
  sqlx checksums migration files.
- **`data_version` polling and `StorageChanged` are deferred to phase 4**,
  with serialising opens across processes (sqlx's migrate lock is a no-op on
  SQLite). No second process writes the file until `seaquel mcp` exists.
- **No `StorageBackend` trait yet.** `seaquel-storage` is sqlx only. Phase
  8's spike decides whether the browser needs a trait or the same queries on
  another executor.
- **Data dir** is `seaquel_storage::data_dir`, with the `dirs` crate rather
  than `directories`. The desktop and web files didn't move.

As built in phase 4:

- **The MCP server doesn't write storage.** It opens `seaquel.db` with
  `StorageOptions { read_only: true }`: no baseline, migrations or data
  steps, and no journal-mode change. A file with any of them pending is
  refused with `STORAGE_NEEDS_UPGRADE` ("Open the Seaquel app once…"), a
  missing one with `STORAGE_NOT_FOUND`. So there is no query history from
  MCP and no saved-query edits.
- **`data_version` polling and `StorageChanged` are deferred again**, to the
  first phase with a second writer (phase 5 or phase 7's `seaquel conn add`).
  A read-only reader needs neither: it re-reads the rows it needs, such as the
  sharing flags, on every call.
- **Opens are serialised.** A pending migration runs under `BEGIN IMMEDIATE`
  on the migrator's own connection and is re-checked under the lock, so two
  processes or two pools opening one file apply it once. This fixed the race
  phase 3 left on web.

### Secrets

`SecretStore` has two implementations:

- **OS keychain** via the `keyring` crate, for desktop, CLI, TUI and MCP. It
  keeps today's key names (`db:<id>`, `ssh:<id>`, `ssh-key:<id>`,
  `license-key`, `ai-api-key:<id>`) so nothing needs re-entering. The desktop
  app stops calling `tauri-plugin-keyring` from JS.
- **Per-request**, for web. The vault's design is that the server never holds
  the key: Argon2id and AES-GCM run in the browser. That stays, as
  web-interface code. The browser unlocks the vault and sends decrypted secrets
  with the RPC call that needs them (connect, AI request), as it already does
  for database passwords. Core keeps them in memory for the connection's
  lifetime and never persists them.

(As built in phase 3: the desktop's `KeychainStore` reads the plugin's
entries unchanged, same crate version and service. The web workspace has no
`SecretStore`, so secret calls there answer `NOT_SUPPORTED`, and TS still
passes the password to `db_connect` on both targets; `ConnectionConnect {
id, secrets }` is phase 5.)

## The RPC surface for GUIs

**Status after phase 1.** `seaquel-rpc` exists, earlier than planned and
narrower. It is connection-scoped rather than workspace-scoped: an `EngineCall
{ connection_id, request: EngineRequest }` covers the dialect and
introspection calls (list schemas, tables, table metadata, statistics,
EXPLAIN, column types, paginate, the four CRUD builders, create and alter
table) and returns an `EngineResponse`. `dispatch` runs it against `Core`.
Desktop exposes it as the `db_engine` Tauri command and web as
`POST /api/db/engine`, whose top-level `connection_id` lets the Node proxy scope
it like the other `/api/db/*` routes. Queries, streaming and storage still use
the phase 0 commands. In TS, `EngineClient` (`src/lib/engine`) has a Rust
implementation over this endpoint and a TS one over the old adapters, picked
per engine. Since phase 2 every engine uses the Rust one on desktop and web,
and the TS one serves only the demo's DuckDB. The workspace-level `Request`
below is still the target.

The CLI, TUI and MCP server link `seaquel-core` directly. The Svelte GUI can't,
so `seaquel-rpc` defines the calls once:

```rust
#[derive(Deserialize, specta::Type)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    ConnectionsList,
    ConnectionConnect { id: ConnectionId, secrets: Option<Secrets> },
    SchemaTables { connection: ConnectionId },
    QueryStream { connection: ConnectionId, sql: String, params: Vec<Value>, query_id: QueryId },
    QueryCancel { query_id: QueryId },
    SavedQueriesSave { /* … */ },
    // …
}

pub async fn dispatch(ws: &Workspace, req: Request, sink: EventSink) -> Result<Response, RpcError>;
```

- **Desktop** exposes two Tauri commands: `core_call(request)` and
  `core_stream(request, channel)`. The existing 24 `db_*`, `git_*` and
  `ssh_*` commands are removed once nothing calls them.
- **Web** has `seaquel-server` serve `POST /rpc` and `GET /rpc/stream`
  (WebSocket). The Node layer keeps authenticating and proxies with a trusted
  `X-Seaquel-User` header over loopback. `seaquel-server` only binds loopback
  already. That header is its only notion of identity, so the loopback-only
  bind becomes a documented security requirement.
- **TypeScript** gets generated `Request`/`Response`/`CoreEvent` types and one
  `CoreClient` with two transports (Tauri, HTTP). It replaces
  `providers/unified-tauri-provider.ts`, `providers/http-provider.ts`,
  `providers/wire.ts` and the storage backends.

### What stays synchronous in the browser

Some of today's TS calls run at keystroke rate. Tauri IPC would be fine for
them, but web round trips add tens of milliseconds:

- statement-at-cursor splitting
- `{{param}}` detection in the editor
- identifier quoting while building filters
- query builder canvas ↔ SQL sync
- Monaco completion ranking

These call `seaquel-wasm`, which is `seaquel-sql` plus each engine's `Dialect`
compiled with wasm-bindgen. It's the same Rust code, so the rule still holds.
Schema data for completions comes from Core and is cached in the GUI.

(As built in phase 2b: `seaquel-wasm` has `seaquel-sql` only. Statement at
cursor, `{{param}}` handling and the builder's parse run in it. Identifier
quoting and pagination still use TS mirrors of the Rust dialects, because the
engine crates can't build for wasm32 until their drivers sit behind a Cargo
feature. Completion ranking is still `monaco-sql-languages`.)

## Interfaces after the move

| Interface | Keeps | Loses to Core |
|---|---|---|
| Svelte GUI (desktop and web) | Components, tabs/panes/layout, Monaco, xyflow canvases, charts, theming, i18n, toasts, dialogs, shortcuts, the web vault's crypto | `db/*`, `providers/*`, `storage/*`, and the logic in `hooks/database/*`, `services/*`, `utils/*` as listed in the crate table |
| `src-tauri` | Windows, menus, updater, deep-link registration, drag-drop, clipboard images, `open_path`, logging, the two RPC commands | `db/commands.rs`, `git.rs`, `ssh_tunnel.rs`, `license.rs`, `read_dbeaver_config`, `read_tableplus_config`, `get_username` |
| SvelteKit/Node (web) | Better Auth, signup, team, account routes, the gate in `hooks.server.ts` (now a loopback call), static serving, loopback proxy | `/api/storage/*`, connection scoping, licensing and air-gap logic |
| `seaquel-server` | axum routing, WS, `X-Seaquel-User` handling, `/internal/license/*` | `/api/db/*` handlers (replaced by `/rpc`) |
| `seaquel` CLI | `clap` commands, table/CSV/JSON output, exit codes | — |
| `seaquel tui` | ratatui views | — |
| `seaquel mcp` | rmcp server, tool schemas, stdio/HTTP transport | — |

### MCP tool set (first cut)

The in-app AI assistant already has a tool registry (`run_query`,
`create_dashboard`, `add_widget`, `get_dashboard`, `update_widget`,
`remove_widget`). Moving it into `seaquel-ai` lets both the assistant and the
MCP server use one registry, so each tool is written once.

- `list_connections`, `list_schemas`, `list_tables`, `describe_table`
- `run_query`: read-only by default, checked by the same validator the
  assistant uses. Write access needs a per-connection opt-in stored in the
  workspace, not a model-controlled flag.
- `explain_query`
- `list_saved_queries`, `run_saved_query`
- the dashboard tools, once dashboards are in Core

### CLI (first cut)

```
seaquel conn list | test <name> | add … | import dbeaver|tableplus
seaquel query -c <conn> [-f file.sql | "<sql>"] [--format table|csv|json] [--param k=v]
seaquel schema -c <conn> [<table>]
seaquel saved list | run <name>
seaquel export -c <conn> "<sql>" --format csv > out.csv
seaquel tui
seaquel mcp [--connection <name>…]
```

## Web licensing over loopback

`hooks.server.ts` needs a license answer on every request, and that logic
moves to `seaquel-license`. Node asks `seaquel-server` over loopback, the same
way `server.js` already asks `/api/account/stream-access` before proxying a
WebSocket.

- `seaquel-server` serves `/internal/license/*`:
  - `gate` (per user: ok, needs membership, suspended, revalidate)
  - `signup-check` and `register-install`
  - `bind-member` and `unbind-member`
  - `members` (the team list proxy)
  - air-gap bundle `status`, `upload` and `clear`
- The Node proxy forwards only `/rpc` and `/rpc/stream` to Rust. `/internal/*`
  is reachable from inside the container only, and `seaquel-server` rejects it
  if the request didn't come from loopback, as a second check.
- **Tables.** The license tables (install id, `member_license`, the
  grace-period cache, `airgap_bundle`) stay in `auth.db`, so no data moves.
  Rust owns those tables and their migrations, and Better Auth keeps its own.
  Both processes open the file in WAL mode.
- **Caching.** The soft/hard TTL ladder from the 2026-05-19 design runs in Rust.
  Node keeps a few seconds of in-memory cache per user to absorb request bursts.
- **Behaviour.** The control-plane contract (`X-Install-Id`, `X-License-Key`)
  and the air-gap Ed25519 verification don't change. The existing vitest
  suites for `licensing`, `license-cache`, `airgap/*` and `signup` become
  parity fixtures for the Rust port, the same way the dialect fixtures work.

As built in phase 3:

- **Node keeps the license migrations.** It still applies 006–012 through
  `auth.ts`; they're frozen history. Rust opens `auth.db` with sqlx and owns
  every read and write of the license tables, and answers `NOT_READY` (503)
  until Node has created them. The migrations move into Rust when the next
  one is needed.
- **Loopback isn't enough on its own.** A query running inside the Rust
  process (DuckDB's httpfs, when a build has it) also connects from
  loopback, so `/internal/*` also needs a per-boot secret: `server.js`
  generates `SEAQUEL_INTERNAL_SECRET`, gives it to both processes, and Node
  sends it as `X-Seaquel-Internal`. So Node and Rust must share a host; a
  split deployment fails closed with a 503 page.
- **Node's burst cache** is 5 seconds per user, with a generation counter so
  an answer that started before an invalidation isn't cached.
- **Session purging stays in Node** and runs after Rust's transaction has
  committed, so it can no longer roll back with it; a failed purge finishes
  when the bundle is uploaded again.
- **The Rust client's TLS and proxy behaviour** differ from Node's fetch:
  it adds `NODE_EXTRA_CA_CERTS` to its roots explicitly and honours
  `HTTP(S)_PROXY`/`NO_PROXY`, which Node 24's fetch ignored.

## Terminal binaries: licensing and distribution

**Licensing.** `seaquel`, `seaquel tui` and `seaquel mcp` don't check for a
license. The terms from the 2026-09-18 license split still apply (free for
personal use, paid for commercial use); it's just not enforced. `seaquel
--version` and `--help` print one line pointing to `seaquel.app/terms`. The CLI
crates don't depend on `seaquel-license`. The desktop app's license checks don't
change.

**Bundling.** The desktop app ships `seaquel` as a Tauri sidecar
(`bundle.externalBin`):

- `release.yml` builds `seaquel` for each target triple before `tauri-action`
  runs. Each target is signed with the same identity as the app.
- On **macOS**, the app menu gets "Install Command Line Tool…", which symlinks
  `/usr/local/bin/seaquel` to the binary inside the app bundle. The admin prompt
  appears only when needed.
- On **Linux**, the `.deb` and `.rpm` packages install `/usr/bin/seaquel`. The
  AppImage offers the same menu item, targeting `~/.local/bin`.
- On **Windows**, the installer adds the binary's directory to `PATH`.
- The CLI updates with the app through the existing updater.
- Signing both binaries with the same team identity is what lets `seaquel` read
  keychain items created by the app. The macOS keychain access group still
  needs setting up; see Risks.

Standalone distribution later (Homebrew, cargo-dist, `ghcr.io`) needs no design
change. The binary already finds its data dir without Tauri (see Storage
ownership).

As built in phase 4:

- **The binary is `seaquel-cli`, everywhere.** tauri-build refuses a sidecar
  named like the Cargo package, and the GUI already owns `seaquel` on every
  platform: `Contents/MacOS/seaquel` on a case-insensitive APFS, `/usr/bin/seaquel`
  from deb and rpm, and `Seaquel.exe` on Windows. The `PATH` links, docs and MCP
  snippets all say `seaquel-cli`.
- **Windows `PATH` is deferred.** The installer doesn't change it; the settings
  panel shows the full path, which is all an MCP host needs. NSIS and WiX hooks
  are a follow-up.
- **A keychain prompt instead of an access group.** keyring 3.6 uses the
  legacy file keychain, where each item trusts only the app that created it,
  and access groups exist only in the data protection keychain, which would
  orphan every saved password and needs a provisioning profile a bare binary
  can't carry. So on macOS the first read of each item shows an "Allow /
  Always Allow / Deny" prompt. The settings panel and README say to choose
  "Always Allow"; the MCP call's timeout pauses while a prompt is open, and a
  denial fails with `SECRET_UNREADABLE` naming the connection. Checking that
  "Always Allow" sticks on a signed build is a manual check.
- **The bundler signs the sidecar.** tauri-bundler 2.9.4 codesigns sidecars
  with the hardened runtime before the app and then notarizes the app, and
  on Windows signs them with `signCommand`. `release.yml` builds the sidecar
  per target in its own step. The first draft release still has to confirm it
  (`codesign -dv --verbose=4`, `signtool verify /pa`).
- **The CLI doesn't enable `git` or either `license-*` feature**, and
  `--version` and `--help` print the terms line.

## The demo: Core in the browser

The demo runs entirely in the browser on DuckDB-WASM and sql.js. It stays that
way, but the logic comes from Core compiled to `wasm32-unknown-unknown`. It then
behaves like the desktop app, and every engine and domain fix reaches it.

```
Svelte GUI ──CoreClient (in-page transport)──▶ seaquel-browser (WASM)
                                                 ├─ seaquel-core, -workspace, -ai
                                                 ├─ seaquel-engine-duckdb-wasm ──▶ @duckdb/duckdb-wasm (JS)
                                                 └─ seaquel-storage (browser backend) ──▶ SQLite in the page
```

- **Transport.** `CoreClient` gets a third transport that calls the WASM
  `dispatch` directly. There's no network, and the same `Request`/`Response`
  types are used.
- **Engine.** `seaquel-engine-duckdb-wasm` implements `Connection` by calling
  DuckDB-WASM through wasm-bindgen. Its `Dialect` is the DuckDB engine crate's,
  built with its `driver` feature off so the native `duckdb` crate isn't pulled
  in. The demo's sample data seeding (`src/lib/demo/*`) stays as TS content
  loaded through Core.
- **Storage.** `seaquel-storage` gets a `StorageBackend` trait (not yet: as
  of phase 3 it is sqlx only, and the spike below decides). The native
  backend uses sqlx. The browser backend is either rusqlite on
  `sqlite-wasm-rs`, or a wasm-bindgen bridge to the sql.js the demo uses today.
  Spike rusqlite first, since it keeps all of storage in Rust. Persistence stays
  in the browser: IndexedDB rather than today's base64 in localStorage, which
  has a size cap.
- **Excluded in the browser build.** SSH, git, keychain, licensing, and every
  native engine. These are Cargo features, off in `seaquel-browser`. AI works
  if the user brings a key: `LlmProvider` uses `reqwest`'s WASM fetch backend.

### Constraints this puts on Core from phase 0

Retrofitting WASM support into an async codebase later is painful, so these
rules apply from the start. CI builds `seaquel-core` for
`wasm32-unknown-unknown` with the `browser` feature set from phase 3 onward.

- **`Send` bounds.** JS-backed futures aren't `Send`. Async traits use
  `#[cfg_attr(not(target_arch = "wasm32"), async_trait)]` and
  `#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]`, wrapped in one
  crate-local macro so nobody writes it by hand.
- **No direct `tokio::spawn`.** Core spawns through a small `Executor` it's
  given: tokio on native, `wasm_bindgen_futures::spawn_local` in the browser.
  `tokio::sync` primitives are fine, since they don't need a runtime.
- **Time.** Timers and clocks go through the executor as well, and
  `std::time::Instant` isn't used in Core. It panics on wasm32.
- **Filesystem.** Shared-repo file I/O and exports go through a trait, because
  the browser has no filesystem. Git and shared repos are off in the browser
  anyway, but exports need a download-based implementation.

## Testing

"Developed and tested independently" is the point of the split, so each layer
gets a test suite that doesn't need the layers above it.

- **Dialect golden tests** (pure, no database): DDL, ALTER, CRUD SQL, EXPLAIN
  parsing and `parse_create_table` against snapshot files, using `insta`.
- **Parity fixtures during migration.** The TS adapters are pure, so a vitest
  script can record their outputs for a corpus of inputs as JSON. The Rust
  dialect tests assert the same output. This is how we port 5k lines without
  guessing. Phase 1 kept the Postgres fixtures after deleting the TS adapter,
  as frozen regression tests (no `insta`; plain JSON files read with
  `include_str!`). Phase 2 did the same for the other four engines.
- **Engine conformance** (`seaquel-engine-testkit`): a function per behaviour
  (introspection shape, round-trip of every column type in the fixture schema,
  streaming and cancel, transactions, CRUD against a real row, EXPLAIN on a
  real plan). Each engine crate runs the whole suite. The fixtures are the
  existing `e2e/test-databases` schemas and seed data: postgres:16, mysql:8,
  mariadb:11 and mssql:2022 in Docker, and the SQLite and DuckDB fixture files.
- **Core tests** use SQLite and DuckDB in-memory, a temp storage DB and a fake
  `SecretStore`.
- **Interface tests**:
  - `seaquel-server` keeps its router tests, moved to `/rpc`.
  - The CLI uses `assert_cmd`.
  - MCP uses an rmcp client against the stdio server.
- **CI.** Nothing runs tests in CI today. Add a workflow running:
  - `cargo test` for pure and core crates
  - the engine matrix with service containers
  - `cargo check --target wasm32-unknown-unknown` on pure crates
  - the dependency-rule script
  - `svelte-check`
  - `vitest`

## Migration plan

Each phase ships. Between phases the GUI calls a mix of old and new paths, and
that's fine.

**Phase 0: groundwork**
- CI workflow above.
- Split `seaquel-db` into `seaquel-engine` plus one crate per engine, with the
  current behaviour unchanged.
- `seaquel-types` with TS codegen wired into `npm run check`.
- Move connect/disconnect into Core, and switch to `CancellationToken`.
- Set up the async-trait macro, the `Executor` abstraction and the no-`Instant`
  lint so new Core code is WASM-ready from the first line.

**Phase 1: Postgres engine pilot**
- Port `postgres.ts` and the shared helpers to `seaquel-engine-postgres`,
  checked by parity fixtures and the conformance suite.
- Add introspection and dialect RPC calls.
- In TS, `getAdapter("postgres")` becomes a shim over those calls, so the
  components don't change. (As built: the call sites moved to an async
  `EngineClient` instead, and `getAdapter("postgres")` now throws.)
- Introduce the typed `Value` for Postgres.
- Measure the effort and adjust the plan before continuing.

**Phase 2: remaining engines**
- Port MySQL/MariaDB, SQLite, MSSQL and DuckDB the same way.
- (As built: `duckdb.ts` stays for the demo, so the demo wasn't pinned. The
  SQL tooling below was split off as phase 2b.)

**Phase 2b: `seaquel-sql`, `seaquel-wasm`**
- Replace node-sql-parser with sqlparser-rs.
- The editor, query builder and tutorial parser switch to `seaquel-wasm`.
- Delete `src/lib/db/*`, except the demo's `duckdb.ts` and the two helpers it
  uses (`alter-table.ts`, `crud-helpers.ts`), which go in phase 8.
- (As built: `db/index.ts` stays too, since it holds the `DatabaseAdapter`
  interface and `getAdapter`. The TS that moved is listed in the `seaquel-sql`
  row of the crate table.)

**Phase 3: storage, secrets, infrastructure**
- Move `seaquel-storage` (and migrate the web backend off better-sqlite3),
  `seaquel-secrets`, `seaquel-ssh`, `seaquel-git` and `seaquel-license` into
  their crates.
- Introduce `Workspace` and `seaquel-rpc` for the moved pieces.
- Delete `storage/*` backends, `/api/storage/*` and most `src-tauri` commands.
- Drop the legacy JSON import and `tauri-plugin-store`.
- Web licensing moves behind `/internal/license/*`, and `hooks.server.ts` and
  the signup/team routes call it.
- Add the `wasm32` CI build of Core with the `browser` feature set.

**Phase 4: `seaquel mcp`**
- The first new interface, and the first real test of Core with a second
  consumer.
- Needs engines, storage and secrets. Nothing else.
- Ship it as the bundled sidecar, along with the "Install Command Line Tool"
  menu item. `seaquel mcp` is the only subcommand at this point.

**Phase 5: domain services**
- Move query execution, pending changes, projects, saved queries, versions,
  history, shared repos, dashboards and workflows into `seaquel-workspace` and
  `seaquel-core`.
- `hooks/database/*` shrinks to view models over `CoreClient`.
- This is the largest phase. Do it one manager at a time.

**Phase 6: `seaquel-ai`**
- LLM calls move out of the webview into Rust.
- The assistant and MCP share the tool registry.

**Phase 7: CLI, then TUI**
- By this point they're mostly presentation code.
- They ship in the same sidecar binary as `seaquel mcp`.

**Phase 8: demo on Core**
- Build `seaquel-browser`, `seaquel-engine-duckdb-wasm` and the browser storage
  backend, plus the in-page `CoreClient` transport.
- Unpin the demo. `npm run demo:update` in the website repo builds it the same
  way as before, with the WASM bundle included.
- This can start any time after phase 5, in parallel with phases 6 and 7.

## Phase 1 cost

Source: `2026-09-25-phase-1-effort.md`, plus line counts measured against a
copy of the tree taken before phase 1 started. Times are implementer wall-clock
as logged, including review fixes where the log says so. They don't include
writing the plan, the research before it, the review passes themselves, or the
owner's checkpoint reviews. Read them as relative sizes, not calendar time.

### Time per part

| Part | Tasks | Time |
|---|---|---|
| A. Contracts (dialect types, `Value`) | 1–2 | ~40 min |
| B. Parity fixtures | 3 | ~30 min |
| C. Rust dialect, introspection, native values | 4–7 | ~2.5 h (Task 7 alone ~70 min, half of it review fixes) |
| D. Core and `seaquel-rpc` | 8–9 | ~45 min |
| E. Frontend (`EngineClient`, value-aware UI, call sites, delete adapter) | 10–13 | ~2.5 h |
| F. CI, docs | 14–16 | not logged |
| **Total logged** | | **~7 h** |

The work that made values exact (Tasks 2, 7 and 11) took about 2.6 h, over a
third of the total, and produced the worst bug of the phase.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~3,140 | ~400 (net ~+2,750) |
| Rust, tests (test files, testkit, inline `#[cfg(test)]`) | ~4,150 | ~30 |
| TypeScript/Svelte, production | ~1,470 | ~1,250 (net ~+220) |
| TypeScript, tests | ~1,930 | — |
| Generated TS types | ~440 | — |

The fixture recorder (~1,160 lines of TS) was written in Task 3 and deleted in
Task 13, so it's in neither column.

Where the production Rust went:

- **Postgres-specific, ~1,060:** `dialect.rs` ~160, `introspect.rs` ~440,
  `numeric.rs` ~270, `bind.rs` ~170, driver and lib ~70. `decode.rs` shrank by
  ~50 in the rewrite.
- **Generic and reusable, ~640:** `seaquel-engine` `ddl.rs` ~360, `crud.rs`
  ~170, `dialect.rs` 65, trait defaults ~45.
- **Types, ~660:** `seaquel-types` dialect types ~380, `Value` ~260.
- **RPC and interfaces, ~245:** `seaquel-rpc` 210, server route and error
  mapping, Tauri command.
- **Core ~85, other engines' value plumbing ~60** (MySQL and SQLite binders;
  MSSQL and DuckDB binders got shorter).

On the TS side, `postgres.ts` (473) went, the four dialect type files lost
~265 lines to generated re-exports, and the call sites lost roughly 350 lines
of adapter and provider plumbing. New code: `src/lib/engine` ~620, `values.ts`
~240, and small helpers (`latest-debounced.ts`, `dashboard-serialize.ts`).

So 473 lines of TS dialect became about 600 lines of Rust dialect and
introspection. The rest of the Rust is either one-time (types, `Value`,
builders, RPC, Core) or had no TS counterpart (the NUMERIC codec, the binder).

### Parity fixtures

110 recorded cases plus 11 hand-written bug-fix cases. 108 of them ran as
parity checks against the port: 78 dialect cases and 30 introspection cases.
**None caught a real difference.** Both ports passed on their first green run.
The dialect and parsers were mechanical to port.

Recording the fixtures against real Postgres was still worth its 30 minutes:
it found bug fix 5 (statistics fail on any table name that needs quoting).
Every other bug of the phase came from live tests or code review, not from
fixtures (see the phase 1 plan's execution notes).

### What was harder than expected

- **Values across every engine.** A typed `Value` meant touching all five
  drivers and binders, both providers, 15 UI files and persisted workflows and
  dashboards. It was planned as Postgres work and turned out to be
  cross-cutting.
- **Bugs that reviews found, not tests:**
  - sqlx's per-connection statement cache reuses the first prepare's parameter
    types, so once `Int` and `Float` bound differently, a float could be
    written as the bits of an INT8 (Postgres) or read back as 0 (MySQL). Fixed
    with `persistent(false)` in `impl_sqlx_driver!`.
  - One bigint cell aborted the whole project save, because saved workflows and
    dashboards were stringified without a replacer.
  - Engine clients captured a connection id that `reconnect()` replaces.
  - Index columns were first returned quoted, which the DDL generator would
    have quoted again.
- **Old decoder bugs surfaced by the round-trip test:** a 30-digit NUMERIC
  panicked, NaN became null, one NULL element nulled a whole array,
  DATE[]/JSON[]/BYTEA[] came back as binary garbage, and every UUID cell was
  NULL.
- **sqlx gaps.** `PgNumeric` is private and neither decimal crate keeps
  NaN/Infinity and display scale, so NUMERIC got a hand-written binary codec.
- **UI sites.** DDL preview became async (debounce, stale replies, snapshotting
  the definition to keep deep tracking), and Postgres now needs a live
  connection to preview.

### What phase 2 reuses

- `ddl.rs` and `crud.rs`, including the option branches Postgres doesn't use
  (`useModifyColumn`, `supportsDropColumn: false`, `?` placeholders), already
  unit-tested.
- The `Dialect` trait, the `Driver` introspection defaults, and the
  `introspection = { … }` hook in `impl_sqlx_driver!`.
- `EngineClient`, `RustEngineClient`, `TsEngineClient` and the
  `getEngineClient` switch. Moving an engine to Rust on the frontend means
  adding it to `RUST_ENGINES`; no call site changes.
- `seaquel-rpc` and both endpoints, unchanged per engine.
- The `Value` format, the provider decoding and every UI site. These already
  work for all engines.
- The fixture recorder pattern. The recorder itself was deleted before any
  commit, so phase 2 rewrites it, ideally once and parameterized by adapter.
- The testkit: `run_smoke`'s Int/Float/Text alternation and array checks run
  for every engine. `run_introspection` is Postgres-specific SQL and needs a
  per-engine scratch schema.

### Phase 2 estimate (engine ports only)

In phase 1 the Postgres-specific tasks (3, 5, 6, 7 and 13) took ~2.8 h for a
473-line adapter. The shared work (A, D, most of E) doesn't repeat. Per engine,
expect fixtures, dialect, introspection, native values, flipping the switch
and deleting the adapter, plus about as much again in review fixes as Task 7
needed.

| Engine | TS adapter | Specific work | Estimate |
|---|---|---|---|
| MySQL/MariaDB | `mysql.ts` 721 | Largest adapter. Record fixtures against both servers. Native decoding for unsigned BIGINT, DECIMAL, BIT, BLOB, JSON. `useModifyColumn` path. sqlx, so the macro and binder already exist | 4–6 h |
| SQLite | `sqlite.ts` 604 | PRAGMA-based introspection; the per-table row-count loop in `TsEngineClient.statistics` and the EXPLAIN timing move into Rust. sqlx | 3–4 h |
| MSSQL | `mssql.ts` 448 | Port the inline CRUD builders (`buildInline*`, `formatLiteralValue`, `formatMssqlBinary`) that phase 1 left in TS. Native tiberius decoding (bytes arrive as base64 today). Not the sqlx macro, so introspection goes on the driver directly. Transactions are still unsupported | 4–5 h |
| DuckDB | `duckdb.ts` 525 | The driver blocks the async runtime; the blocking-work hook carried over from phase 0 is still missing. Lists and structs are Debug strings today and should become `Array`/`Json`. Reuses MSSQL's inline builders. Deleting `duckdb.ts` removes the demo's dialect, which triggers the demo pin | 5–7 h |

About **16–22 h** of implementer time for the four engines, plus a manual GUI
pass per engine (30–60 min of a person's time each). Phase 1's own manual
checks are still outstanding and should be done first, since phase 2 builds on
the same UI paths.

Not in this estimate: the rest of phase 2, now phase 2b (`seaquel-sql` on
sqlparser-rs, `parse_create_table`, `seaquel-wasm`, the editor, query builder
and tutorial switch). Phase 1 measured nothing about those, and matching node-sql-parser's
AST shape is the bigger risk. Plan them separately.

### Engine order

Keep the order from the migration plan: MySQL/MariaDB, SQLite, MSSQL, DuckDB.

- MySQL is closest to what phase 1 built: sqlx, parameterized builders,
  `information_schema`, and its binder and statement-cache fix already landed.
  It's also the largest adapter, so it's the best test of the per-engine
  estimate.
- SQLite is the same shape and smaller.
- MSSQL before DuckDB, because MSSQL brings the inline builders DuckDB also
  needs.
- DuckDB last: it needs the blocking hook, and its port ends TS dialect support
  for the demo.

Before the first engine, rewrite the fixture recorder once, parameterized by
adapter, so each engine only adds inputs.

## Phase 2 cost

Source: `2026-09-26-phase-2-effort.md` and the phase 2 plan's execution notes,
plus line counts measured against a copy of the tree taken before phase 2
started. Times are framed as in phase 1: implementer wall-clock as logged,
review fixes included (they have their own lines in the log), but not the plan,
the research, the review passes themselves or the owner's checkpoints.

### Time per engine

| Part | Tasks | Estimate | Logged |
|---|---|---|---|
| MySQL/MariaDB | 4–7 | 4–6 h | ~5.9 h |
| SQLite | 8–10 | 3–4 h | ~4.1 h |
| MSSQL | 11–14 | 4–5 h | ~9.7 h |
| DuckDB | 15–17 | 5–7 h | ~10.6 h |
| **Engines** | | **16–22 h** | **~30.3 h** |
| Shared groundwork (fixes, recorder, testkit) | 1–3 | — | ~1.4 h |
| UI dialect snippets, 0-row edits | 18 | — | ~5.1 h |
| Delete adapters, CI, docs | 19–21 | — | ~3 h |
| **Total logged** | | | **~39.8 h** |

MySQL and SQLite landed at or just over the top of their ranges. MSSQL and
DuckDB took about twice their estimates, and together account for two thirds of
the engine time. The estimate scaled with the size of each TS adapter, and that
turned out to predict nothing: `mssql.ts` was the smallest adapter and cost the
most. What cost time was the driver underneath.

The shared rows weren't in the estimate at all, which covered engine ports
only. Task 18 was added to the plan from research findings and review
follow-ups, and grew as reviews added the UNIQUE checkbox and catalog-qualified
names.

Review fixes logged on their own lines add up to ~9.3 h, about a quarter of the
total: MySQL ~50 min, SQLite ~50 min, MSSQL ~2.7 h, DuckDB ~3.2 h and shared
~1.8 h. Tasks 2, 3, 6 and 11 folded their review fixes into the task's time, so
the real share is higher. Phase 1's rule of thumb, "about as much again in
review fixes as Task 7 needed", was about right for MySQL and SQLite and too
low for the other two.

Fixture recording took ~7.9 h including its review rounds (MySQL 75 min, SQLite
40 min, MSSQL ~2.6 h, DuckDB ~3.4 h), against 30 minutes in phase 1. For MSSQL
and DuckDB, recording turned into designing the fixes: the expected outputs of
the new bug fixes came from small models of the fixed generators (in
`docs/plans/artifacts`), and every expected ALTER was run against a real
server.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~9,400 | ~880 (net ~+8,550) |
| Rust, tests (test files, testkit, inline `#[cfg(test)]`) | ~15,700 | ~300 |
| Parity fixtures (JSON) | ~57,100 | — |
| TypeScript/Svelte, production | ~2,020 | ~2,360 (net ~−340) |
| TypeScript, tests | ~1,810 | ~190 |
| Generated TS types | 60 | 4 |

The fixture recorder and the four corpora (about 4,200 lines of TS at their
peak) were written in Tasks 2, 4, 8, 12 and 17 and deleted in Tasks 19 and 21,
so they're in neither column; reference copies are in `docs/plans/artifacts`. A
workspace-wide `cargo fmt` in Task 19 also reflowed ~300 lines in `src-tauri`,
which aren't counted.

Where the production Rust went:

- **MySQL, ~1,820:** `dialect.rs` ~210, `introspect.rs` ~1,270 (three EXPLAIN
  formats across two servers), native decode and bind ~280.
- **SQLite, ~1,260:** `dialect.rs` ~360, `introspect.rs` ~690, driver ~210.
- **MSSQL, ~2,200:** `dialect.rs` ~380, `introspect.rs` ~380, `session.rs`
  ~380, decode and bind ~460, driver ~660.
- **DuckDB, ~2,540:** `dialect.rs` ~370, `introspect.rs` ~730, the Arrow
  decoder ~720, `blocking.rs` ~160, driver ~590.
- **Shared, ~1,350:** `seaquel-engine` `ddl.rs` (the T-SQL and DuckDB rules,
  UNIQUE changes, drop order), the introspection helpers the engines had each
  copied, and `crud.rs` variants.
- **Other, ~260:** `seaquel-types` (`expectRows`, collation, UNIQUE flags),
  RPC, server and the Postgres side of the dedupe.

The three deleted adapters plus `duckdb.ts` come to 2,298 lines of TS dialect.
Their Rust dialect and introspection files come to about 4,400, roughly twice
the size, against 1.3 times for Postgres. The difference is mostly EXPLAIN:
MySQL alone parses v1 JSON, v2 JSON, ANALYZE text and MariaDB's JSON. The MSSQL
session and the DuckDB decoder had no TS counterpart.

Of the fixture JSON, about 19,600 lines are `bugfixes.json`, 13,300 recorded
EXPLAIN inputs and 11,300 ALTER cases.

### Bugs found

51 numbered bug fixes, each with its own test and documented exception to
parity. By where they were found:

| Engine | Plan (research before phase 2) | Recording fixtures | Review |
|---|---|---|---|
| MySQL/MariaDB | 8 | 2 | — (7 and 8 reworked) |
| SQLite | 8 | 3 | — |
| MSSQL | 9 | 5 | 1 |
| DuckDB | 7 | 8 | — (6, 10 and 11 reworked) |
| **Total** | **32** | **18** | **1** |

SQLite's fix 11 (Set to default) was noticed while recording and made a fix by
the Tasks 8–9 review; it's counted under recording.

**The parity fixtures caught no port bugs**, as in phase 1: 0 of 652 recorded
cases (MySQL 145, MariaDB 70, SQLite 161, MSSQL 130, DuckDB 146). Recording
them against real servers is what found the 18 fixes in the middle column.

The bugs outside the numbered lists came from live tests and reviews. The
effort log names each one, but it doesn't always say which, so these counts are
approximate. A bug that a live test caught during a review follow-up is counted
under review.

| Engine | Live tests | Review |
|---|---|---|
| MySQL/MariaDB | ~4 | ~5 |
| SQLite | ~2 | ~5 |
| MSSQL | ~4, plus 1 while recording | ~9 |
| DuckDB | ~6 | ~8 |

Live tests found value bugs: TINYINT(1) holding 5 read as `true`, TIMESTAMP
keys that strict mode rejected on both servers, a varbinary key sent as base64
that matched no row, `東京` read back as `??`, TIMETZ losing its offset, BC
timestamps shown as the year 2044. Reviews found the session, concurrency and
quoting bugs. The driver and UI bugs fixed outside any engine's list (0-row
edits reported as success, identifier quoting in the data tab, schema tab,
workflow nodes, command palette and completions, the MSSQL count wrapper) are
in the plan's execution notes.

### What was harder than expected

- **MSSQL session and batch semantics.** tiberius sends every query through
  `sp_executesql`, where CREATE VIEW is a syntax error and SHOWPLAN returns
  nothing. Only RPC calls report affected rows, and `USE` outlives the call
  while `SET` and `#temp` don't. The batch policy was settled in the Task 13
  review, after the first version let a user's `SET` outlive the query. A
  caller dropped between BEGIN and its reply left tiberius and the server
  disagreeing about the transaction, and every later request failed with 3989;
  the fix was poison-and-reconnect, which took Task 11 and both its review
  rounds of live drop tests. There's also no NULL parameter type that fits
  every column, and money arrives as an f64.
- **DuckDB's Rust bindings.** duckdb-rs panicked on TIME_NS and dumped LIST and
  STRUCT cells as the Debug string of the whole column chunk. The driver now
  decodes Arrow itself (Task 16, 2.5 h alone), and needs
  `arrow_lossless_conversion` so TIMETZ and HUGEINT survive. Cancellation
  needed a check between prepare and execute, because DuckDB clears its
  interrupt flag at each one.
- **Catalog-qualified names.** DuckDB fix 6 lists an attached catalog's schemas
  as `catalog.schema`, quote-aware. That reached `ddl.rs` (`quote_schema`,
  `AlterTableRules`), `EngineClient.qualifiedTable` and every UI file that
  builds a table name, over three review rounds.
- **Engine DDL rules.** DuckDB refuses most column changes while any index
  exists, and SQL Server drops a column's collation and refuses DROP INDEX on a
  constraint's index. Finding these took live runs of every ALTER case, not the
  fixtures.
- **Reviews that found real bugs.** Task 14 bound MSSQL `{{params}}` as `@Pn`,
  which broke `TOP {{n}}`, `CREATE VIEW` and a user's own `@p1`. The review
  reverted it to inlining, and the next one found that the inliner substituted
  inside comments and bracketed names and wrote `1-{{p}}` with `-1` as `1--1`,
  a comment. DuckDB's inliner didn't know `$tag$…$tag$` or `E'…'` strings.
  SQLite's first Set to default copied another column's value when the default
  was a bare word. None of these had a failing test until the review wrote one.

### Phase 2b estimate (`seaquel-sql`, `seaquel-wasm`, the parser switch)

What it replaces, in TS today:

- **Scanners, ~1,300 lines, no parser:** `db/sql-parser.ts` (statement
  splitting, statement at cursor), `db/query-params.ts`, `db/query-utils.ts`,
  `engine/sql-scan.ts`. They have 42 unit tests between them, and phase 2's
  reviews kept finding quoting bugs in exactly this kind of code.
- **node-sql-parser consumers, ~3,900 lines:** `tutorial/sql-parser.ts` (1,213)
  and `criteria.ts`, `db/sql-ast-parser.ts` (the visual AST),
  `db/column-sources.ts` and the query builder (`hooks/query-builder-*.ts`,
  ~1,650, counted whole). They have no unit tests beyond a 53-line query
  builder test.
- **`db/parse-create-table.ts`, 306 lines,** hand-written.

| Part | Estimate |
|---|---|
| Record a TS baseline (a SQL corpus through the scanners, the visual AST, column sources, every lesson's criteria and query builder round trips) | 3–5 h |
| Scanners and `parse_create_table` in `seaquel-sql` | 4–6 h |
| sqlparser-rs AST helpers: visual AST, column sources, tutorial parser, query builder parse | 10–16 h |
| `seaquel-wasm`: wasm-bindgen, the Vite build on desktop, web and demo, async init | 3–5 h |
| Switch the call sites, delete the TS | 2–4 h |
| Review fixes, at phase 2's rate of about a quarter | 5–9 h |
| **Total** | **~27–45 h** |

That's wider than the phase 2 estimate on purpose. Phase 2's straight ports
(MySQL, SQLite) came in on estimate, and the parts that sat on a library with
its own behaviour (tiberius, duckdb-rs) doubled. Most of 2b sits on
sqlparser-rs, and nothing has been measured about it yet.

Risks:

- **AST shape.** The query builder's two-way sync and the tutorial's lesson
  criteria read node-sql-parser's AST. sqlparser-rs has a different tree, and
  some criteria may need rewriting rather than porting.
- **No baseline.** Phase 2 had a working adapter per engine to record. Here
  most of the code has no tests, so the baseline has to be built from a
  hand-picked corpus, and whatever it misses won't be compared.
- **Offsets.** Monaco positions are UTF-16 offsets and Rust strings are UTF-8.
  Statement at cursor, and anything else that returns a position, needs
  converting at the boundary. A query with `東京` before the cursor is enough to
  show it wrong.
- **Dialect coverage.** node-sql-parser is used in PostgreSQL mode for the
  tutorial and per engine elsewhere. sqlparser-rs has MsSql, DuckDb and SQLite
  dialects, but they accept different things than node-sql-parser's, so some
  queries that parse today may not, and the other way round.
- **WASM in the build.** Nothing in the repo loads WASM yet. The call sites are
  synchronous at keystroke rate, so the module has to be initialized before the
  editor mounts, in three builds, without adding much to startup.

**Do a spike first**, of about 4–6 h. Write one passing query per tutorial
challenge (the lessons store criteria, not solutions) and 30–50 query builder
round trips, parse them with sqlparser-rs in each dialect and map them to the
tutorial's `ParsedQuery` and the visual AST, and count how many differ from
what node-sql-parser gives. In the same spike, load a stub `seaquel-wasm` into
the desktop, web and demo builds and measure size and init time. If most
queries map cleanly, 2b is near the low end. If the lesson criteria have to be
rewritten, plan for the high end and consider keeping node-sql-parser for the
tutorial.

## Phase 2b cost

Source: `2026-09-27-phase-2b-effort.md` and the phase 2b plan's execution
notes, plus line counts measured against a copy of the tree taken before phase
2b started. Times are framed as in phases 1 and 2: agent wall time as logged,
review fixes included (they have their own lines in the log), but not the plan,
the review passes themselves or the owner's checkpoints. Tasks 3–7 ran in
parallel, so the calendar time was shorter than the sum.

### Time per task

| Part | Tasks | Estimate | First pass | Review fixes, follow-ups | Logged |
|---|---|---|---|---|---|
| Spike | — | 4–6 h | ~1.3 h | — | ~1.3 h |
| Crates | 1 | 0.5–1 h | ~0.5 h | — | ~0.5 h |
| TS baseline and models | 2 | 1.5–2.5 h | ~0.5 h | ~1.6 h | ~2.1 h |
| Scanner and statement checks | 3 | 1.5–3 h | ~0.8 h | ~0.6 h | ~1.4 h |
| `{{param}}` substitution | 4 | 1.5–3 h | ~0.7 h | ~1.5 h | ~2.2 h |
| `parse_create_table` | 5 | 1–1.5 h | ~0.6 h | ~0.4 h | ~1 h |
| AST helpers | 6 | 2.5–3.5 h | ~1.1 h | ~0.6 h | ~1.7 h |
| Build and load the module | 7 | 1.5–2.5 h | ~0.9 h | ~0.8 h | ~1.7 h |
| Exports, offsets, `src/lib/sql` | 8 | 1.5–2 h | ~0.8 h | ~0.4 h | ~1.2 h |
| Switch the call sites | 9–10 | 2–3 h | ~1.4 h | ~0.6 h | ~2 h |
| CI, docs | 11–12 | 0.75–1.25 h | ~0.7 h | — | ~0.7 h |
| Measure | 13 | 0.25 h | ~0.4 h | — | ~0.4 h |
| Review fixes (the plan's own row) | | 3.5–5.5 h | | | |
| **Total, Tasks 1–13** | | **~18–29 h** | **~8.4 h** | **~6.5 h** | **~14.9 h** |

With the spike, ~16.2 h, against the design doc's 27–45 h plus a 4–6 h spike.
The whole phase came in under the low end of both estimates.

The two halves went opposite ways. First passes took about 8.4 h against 14.5–23.5
h in the plan's task rows: every port matched its fixtures on the first
run, and the spike had already ported most of the AST code. Review fixes and
follow-ups took ~6.5 h against 3.5–5.5 h, over the top of the range, and made
up over 40% of the logged time, against a floor of a quarter in phase 2. Tasks
2 and 4 account for half of it: the read-only check (fix 14) and `{{param}}`
substitution (fix 13) went through three review rounds each, and Task
4's reviews took twice as long as its port.

The ~1.6 h under Task 2 is spread over five log lines: the owner's checkpoint
fixes, the Task 2 review and re-review, and a model update after the Task 3 and
4 reviews. Task 7's column includes the 2 MB stack (~0.2 h), which came out of
Task 6's measurements rather than a review.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~7,120 | 0 |
| Rust, tests (test files, inline `#[cfg(test)]`) | ~3,500 | — |
| Fixtures (JSON) | ~129,400 | — |
| TypeScript/Svelte, production | ~1,110 | ~3,810 (net ~−2,700) |
| TypeScript, tests | ~2,190 | ~500 |
| Generated TS types | ~390 | — |
| Build scripts (JS) | ~390 | — |

Measured as in phase 2 (`crates/`, `src/`, `src-tauri/src`, `scripts/` and the
root config files, generated paraglide output and `src/lib/wasm/pkg` left out).
`pending-change-description.ts` moved from `src/lib/db` to `hooks/database`
unchanged and is in neither column. The spike (~1,980 lines of Rust and ~1,430
of TS/JS) and the fixture recorder with its models (~5,400 lines of TS/JS when
deleted) were deleted in Task 12, so they aren't counted either; reference
copies are in `docs/plans/artifacts`. node-sql-parser left `package.json`.

Where the production Rust went (`seaquel-sql` ~6,510, `seaquel-wasm` ~610):

- **Scanner and statement checks, ~1,930:** `scan.rs` ~665, `statements.rs`
  ~560, `read_only.rs` ~250, `js_word.rs` ~350 (mostly the generated Unicode
  tables of fix 19), `engine.rs` and `js_ws.rs` ~110. The TS it replaces
  (`sql-parser.ts`, `query-utils.ts`, `sql-scan.ts` and the read-only check)
  was about 590 lines. The growth is fixes 10–14, 18 and 19: per-engine
  quoting, comment, number and word rules, and the destructive and read-only
  checks on tokens.
- **`{{param}}` substitution, ~1,070** (`params.rs`), for 715 lines of
  `query-params.ts`. Most of the extra is fix 13's value rules.
- **`parse_create_table`, ~1,060**, for 306 lines of TS. About a third of it is
  the backtracking matcher with JavaScript regex semantics, with its step
  budget.
- **AST helpers, ~2,420:** tutorial and builder `ParsedQuery` ~960, the visual
  AST ~900, the depth guard and parse helpers ~400, column references ~110. The
  TS was 1,926 lines (`tutorial/sql-parser.ts`, `sql-ast-parser.ts`,
  `column-sources.ts`), so this part was close to one to one.
- **`seaquel-wasm`, ~610:** 16 exports and the `{ok}`/`{error}` envelope ~410,
  UTF-16 offsets ~180, the stack-size build script 22.

About 3,510 lines of TS files were deleted, close to the plan's ~3,500. The
Rust that replaced them is twice the size, as in phase 2. The AST code ported
at about one to one; the scanners grew to three times their size, because the
TS scanners ignored the engine, and fixing that is most of what phase 2b's bug
fixes are.

Of the fixture JSON, about 29,400 lines are `split.json`, 17,000
`statements.json`, 15,600 `params.json` and 13,600 `bugfixes.json` (1,585 model
cases, each naming its fix).

### Bugs found

19 numbered bug fixes, each with fixture cases and a reason in
`bugfixes.json`. By where they were first found:

| Area | Spike | Plan research | Recording (Task 2) | Review |
|---|---|---|---|---|
| Visual tab | 1–8 | — | 15 | — |
| Query builder | 9 | — | — | — |
| Scanner and checks | 10 | 11–14 | — | 18, 19 |
| Table editor | — | — | 16, 17 | — |
| **Total** | **10** | **4** | **3** | **2** |

That table undercounts recording and review, because both widened fixes that
already had a number. Recording found that a plain function call hid the whole
query from the Visual tab (fix 3) and that placeholders printed as `""` (fix 6),
so those two are half spike, half recording. Reviews widened four fixes:

- **Fix 10:** MySQL executable comments, `--` ending at `\r`, MySQL's `--`
  rule and dollar tags of any length (Task 2, 3 and 4 reviews).
- **Fix 11:** EXPLAIN ANALYZE, `DROP c` without `COLUMN`, data-modifying CTEs,
  `MERGE … THEN DELETE`, the other DROP kinds and SQL Server without `;`, each
  of which ran with no confirmation (Task 2 review and re-review).
- **Fix 13:** the forced-inline contexts (Task 2 checkpoint and review), then
  the four value rules and the spacing rule after two live breakouts (Task 4's
  two review rounds).
- **Fix 14:** the read-only bypasses confirmed live on MySQL, MariaDB and SQL
  Server, about 40 blocked functions, and the NBSP and combining-mark keyword
  tricks (Task 2 review and re-review, Task 3 review).

By count of rules added, the reviews found more than any other source,
including every bypass of the new checks.

**The parity fixtures caught no port bugs**, as in phases 1 and 2. Every Rust
port and the TS wrapper matched every recorded case on the first run, except
the planned fixes and a few pinned exceptions.

Outside the numbered list:

- **Older than 2b:** AI dashboard widgets ran the model's query with no
  read-only check (Tasks 9/10 review, fixed); the AI tool checks a query
  against one connection and may run it on another (found in Task 9, not
  fixed, a Follow-up); the Visual panel dropped SQL it couldn't parse (plan
  research, fixed in Task 10); the runner resolved column sources against the
  active connection instead of the one running the query (fixed in Task 9; the
  log doesn't say whether the implementer or the review found it).
- **In the new code, before it shipped:** `has_where` was quadratic (Task 3,
  implementer); `destructive_reason` still was (1.2 s on 690 KB, Task 3
  review); the CREATE TABLE matcher was quadratic on long whitespace (62 s on
  50k spaces, Task 5 review); operator chains overflowed the stack inside
  sqlparser (Task 6, implementer); `callWasm` only recovered from a
  `RuntimeError`, not V8's `RangeError` (Task 7 review); and the first fallback
  contract failed open on the run path (Tasks 9/10 review).

### Module size and init time

The module the three builds ship, measured again on the final build:

| | Raw | gzip -9 | brotli 11 |
|---|---|---|---|
| `seaquel_wasm_bg.wasm` | 1,710,767 B (1.71 MB) | 634 KB | 484 KB |
| The spike's stub, for comparison | 1.56 MB | 561 KB | 428 KB |

Compressed with Node's zlib, as `build-wasm.mjs` prints them. The growth
over the stub is our own code (the scanner, checks, substitution,
the CREATE TABLE parser and the Unicode tables); sqlparser is about the same
1.08 MB inside it. Desktop, web and demo ship the same file.

Dropping node-sql-parser and the TS it served took 2.65 MB raw and 516 KB
gzipped out of the JS (27.78 → 25.13 MB raw, 6.23 → 5.72 MB gzip; the Task 8
build against the final one, all builds the same). Net of the module, the app
is ~118 KB gzipped heavier than before phase 2b, where the spike expected
~140 KB.

Init time, with the spike's Playwright script adapted to the shipped module:
from the start of the `.wasm` fetch to the instance being ready, five cold
loads each, headless, from localhost. The desktop static build is served with
Tauri's CSP.

| Target | Chromium | WebKit |
|---|---|---|
| Desktop static build | 3.7–6.8 ms (median 4.2) | 22–42 ms (median 23) |
| Web (adapter-node) | 4.0–7.5 ms (median 4.2) | 21–26 ms (median 22) |
| Demo (`/demo` base path) | 3.7–5.0 ms (median 3.9) | 22–29 ms (median 26) |
| Spike stub, for comparison | 3.4–7 ms | 16–27 ms |

So the full module loads about as fast as the stub did; WebKit is still five
times slower than Chromium and still under 45 ms. The demo editor check passed
in both browsers: typing 43 characters and Run at cursor after `東京😀` ran the
second statement. The web build still sends the `.wasm` uncompressed
(1,711,067 bytes over the wire; `precompress` is a Follow-up).

### What was harder than expected

- **Security review rounds, not the ports.** The ports of fixes 13 and 14
  matched their models on the first run, and the models were wrong. The first
  read-only model was bypassed live on three servers in the Task 2 review
  (`/*! DELETE … */` deleted rows on MySQL and MariaDB, `SELECT 1. INTO t`
  created a table, `SELECT 1 AS k KILL 9999` ran on SQL Server), then again
  through a digit glued to `INTO`, an NBSP after `--` and a combining mark
  after `INTO`. The check ended up scanning each input under two sql_modes and
  two word rules and refusing if either reading refuses, and the plan now says
  plainly that it isn't a sandbox. Substitution went the same way: after the
  port matched, one review broke out of a `$$…$$` string on Postgres, DuckDB
  and MySQL, and the next, a randomized pass of 7 million checks, merged a
  value into the literal before it (`E'a'{{p}}`) on the DuckDB CLI. In both
  cases the fixture pipeline did its job, and the model it pinned was the weak
  point.
- **The CREATE TABLE matcher.** The TS was a dozen regexes, and its output
  depends on regex details, so matching it meant writing a small backtracking
  matcher with JavaScript's semantics. The review found it 45 times slower than
  V8 on long whitespace (62 s for 50k spaces). Possessive `\s+`, a run cache
  and a step budget fixed it, and replacing the matcher with a parser is a
  Follow-up.
- **Nesting and stack limits.** sqlparser's recursion limit doesn't cover
  left-deep chains, and 20,000 `AND`s overflowed its own stack. The fix was a
  depth guard counted from the tokens, an iterative printer, a cap of 2,000 and
  a 2 MB module stack. Natively, sqlparser still overflows a 1 MB stack on
  nested FROM subqueries from depth 25, so the AST helpers need a bigger thread
  stack before Core, the CLI or MCP call them.
- **Trap recovery in wasm-bindgen 0.2.128.** `initSync` returns early once a
  module is set, and the reset-state flag generates a call to an export that
  doesn't exist. The build script now patches the glue, and `callWasm` has to
  handle three different errors from a trap. The fallback answers that made
  sense per keystroke failed open on the run path, which took another review
  round to find.
- **Agents sharing one crate.** Tasks 3–6 edited `seaquel-sql` at the same
  time. `cargo fmt -p` reformatted other agents' files, a check-list run failed
  twice on another task's half-written module, parallel lib tests aborted twice
  with SIGABRT while another agent was mid-edit, and a concurrent vite build
  broke paraglide's output. None of it caused a wrong result, only reruns, and
  the log doesn't time them.

### Follow-ups and phase 3

The Follow-ups don't change phase 3's scope. Storage, secrets, SSH, git and
licensing don't touch `seaquel-sql`. Two items touch phase 3's edges: the web
server should precompress or compress the `.wasm`, which fits with phase 3's
changes to the Node server, and the wasm32 CI build of Core has a working
template now (`build-wasm.mjs`, the CI toolchain steps, the stack-size check).

The two high-priority AI items should come before phase 3, as a short phase of
their own:

- **Read-only enforced in the database.** With "allow all queries" on, fix 14's
  token check is the only thing between the model and a write. Phase 2b showed
  twice that a blocklist can be bypassed, and it can't see inside user-defined
  functions. The engines can enforce it: `BEGIN READ ONLY` on Postgres, `START
  TRANSACTION READ ONLY` on MySQL/MariaDB, `PRAGMA query_only` on SQLite, a
  read-only connection on DuckDB. SQL Server has no equivalent, so it keeps the
  token check and should say that a read-only login is the real guard. This is
  engine and Core work (one connection per call, a read-only flag through
  `seaquel-rpc`), and phase 2 showed that session semantics are where engine
  work runs long, so budget 4–8 h with live tests on every engine. AI dashboard
  widgets need the same session.
- **Bind the AI tool call to its connection.** The tool checks a query under
  one connection's rules and runs it on whatever is active when it executes.
  Passing the connection id with the call and refusing if it's gone is under an
  hour of TS.

Neither depends on phase 3, and phase 3 doesn't touch these paths, so doing
them first costs nothing in rework. Waiting for phase 6 (AI in Rust) would
leave the gap open for several phases. Neither closes network egress
(DuckDB's httpfs); that needs httpfs off for the AI's connection.

For estimating phase 3: first passes ran at about half of phase 2b's estimate
and review fixes over it. Where code decides what SQL runs, or holds secrets as
phase 3's will, plan review fixes at 40% rather than a quarter.

## AI safety cost

Source: `2026-09-28-ai-safety-effort.md` and the AI safety plan's execution
notes, plus line counts measured against the phase 2b commit (`dd63c70`); the
whole phase is in the working tree on top of it. Times are framed as in the
earlier phases: agent wall time as logged, review fixes included, but not the
plan, the probes behind it, the review passes themselves or the owner's
checkpoints. Tasks 2–7 ran in parallel after Task 1, so the calendar time was
much shorter than the sum.

### Time per task

| Task | Estimate | First pass | Review fixes | Logged |
|---|---|---|---|---|
| 1. The contract | 1–1.5 h | ~0.3 h | — | ~0.3 h |
| 2. Postgres, MySQL, MariaDB | 1–1.5 h | ~0.6 h | ~0.25 h | ~0.8 h |
| 3. SQLite | 1–1.5 h | ~0.9 h | ~0.5 h | ~1.4 h |
| 4. DuckDB | 0.75–1 h | ~0.8 h | ~0.9 h | ~1.8 h |
| 5. SQL Server | 0.75–1.25 h | ~2 h, together | | ~2 h |
| 6. Transports, providers, demo | 1–1.5 h | ~0.3 h | — | ~0.3 h |
| 7. AI tools, dashboards, binding | 1.5–2 h | ~0.7 h | — | ~0.7 h |
| 8. Docs, note and measure | 0.5–0.75 h | ~0.5 h | — | ~0.5 h |
| Review fixes (the plan's own row) | 3–4.5 h | | | |
| **Total** | **~10.5–15.5 h** | | | **~7.8 h** |

Tasks 1–7 took ~7.3 h against 7–10.25 h in the plan's task rows alone. The
design doc had budgeted 4–8 h for enforcing read-only in the database and
under an hour for binding the tool call, 5–9 h in all, and the phase landed
inside that. The plan expected ~7–10 h logged if first passes ran at half
their estimate, as in phase 2b; they did, apart from SQL Server.

The log keeps review fixes on their own lines for Tasks 2–4 (~1.7 h, a quarter
of Tasks 1–7). Task 5's ~2 h covers its first pass, the coordinator's switch
to a connection per call and two review rounds, with no split; if half of it
was review work, fixes were about 37% of the total, close to the 40% the plan
budgeted. Tasks 1, 6 and 7 have no review-fix lines. The binding itself, the
second Follow-up, fit in Task 7's ~0.7 h, as the design doc expected.

SQL Server alone ran over: about twice the top of its range, and the only task
where the plan's mechanism was replaced rather than adjusted.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~1,600 | ~100 |
| Rust, the testkit's read-only harness | ~570 | — |
| Rust, tests (test files, inline `#[cfg(test)]`) | ~4,960 | ~340 |
| TypeScript/Svelte, production | ~740 | ~170 |
| TypeScript, tests | ~1,350 | ~20 |
| `server.js` | 9 | — |

Measured with `git diff --numstat` against `dd63c70` plus the new files, with
`Cargo.lock`, the docs and the message files left out (two new keys, in six
languages). About 330 of the test lines moved: the DuckDB typed-cell cases and
the seeded copy went from `values.rs` and `smoke.rs` into `tests/common` so
the read-only tests could use them.

Where the production Rust went: SQLite ~490 (`read_only.rs` ~280, the gate,
the authorizer and the only `unsafe` in the crate; `driver.rs` ~205, the NUL
refusal wrapper and the progress handler), SQL Server ~420 (`driver.rs` ~320,
`session.rs` ~70, `bind.rs` 30), DuckDB ~165, Postgres ~135, MySQL ~65, Core
~90, `seaquel-engine` ~115 (`Driver::query_read_only`, `fetch_capped` and the
macro's `read_only` arm), `seaquel-sql` ~65 (the DuckDB block list), the two
transports ~50. Of the ~1,520 non-blank production lines, ~550 are comments,
most of them recording what a probe showed and why the mechanism is shaped the
way it is.

Tests outweigh the code three to one, because every attack is a live case with
its own check from a normal session that nothing changed.

### Bugs found

By who found them first. "Implementer" means a live probe or a failing test
while building the task; "review" means one of the review rounds after it.

| Area | Implementer | Review |
|---|---|---|
| Postgres | — | 1 |
| MySQL/MariaDB | 1 | 2 |
| SQLite | 2 | 2 |
| DuckDB | 2 | 7 |
| SQL Server | 1 | 4 |
| Transports, demo, widgets | 4 | — |
| **Total** | **10** | **16** |

Found by the implementers:

- **MySQL:** a procedure that switches the session read-write before its
  INSERT wrote under the session setting the plan chose (Task 2).
- **SQLite:** a NUL byte in SQL text makes sqlx loop forever, on every path,
  the editor's included (older than this phase); a dropped query kept running
  and holding its locks (Task 3).
- **DuckDB:** `checkpoint()` runs inside a read-only transaction; a plain
  SELECT autoinstalls known extensions (Task 4; the second is a Follow-up).
- **SQL Server:** `SET CONTEXT_INFO`, session context, app locks and global
  cursors survive ROLLBACK and `sp_executesql`, so the held session couldn't
  be reused (Task 5, the coordinator's probe).
- **Transports and widgets:** a web Stop never reached the WebSocket handler;
  `server.js` flushed a buffered first frame after the browser had gone (older);
  DuckDB-WASM can't interrupt a prepared statement (a Follow-up); the widget
  editor's preview and the version diff still ran widget SQL through
  `executeRaw` (Tasks 6 and 7).

Found in review, with the critical ones first:

- **SQLite (critical):** `sqlite3_stmt_readonly` is true for process-wide
  PRAGMAs, and `PRAGMA hard_heap_limit = 200000` from the read-only path
  crashed the process; `soft_heap_limit` and `temp_store_directory` changed
  every connection. Fixed with an authorizer. The review also widened the NUL
  hang to every entry point. The authorizer then made FTS5 tables unreadable
  until its internal PRAGMAs were allowed; the log doesn't say who caught that.
- **DuckDB (critical):** on one shared clone, `enable_profiling()` outlived
  the rollback and rewrote its file after every later call; `enable_logging()`
  is global and puts the user's editor SQL, `CREATE SECRET` included, in
  `duckdb_logs`; `query('…')` and `json_execute_serialized_sql` hide SQL from
  the check; the re-review's sweep of `duckdb_functions()` found that
  `mysql_execute('my', 'CREATE TABLE …')` created a table from the read-only
  path, that `start_ui_server()` started an HTTP server serving the database,
  and that `load_aws_credentials` returns credentials. Also: without a `LIMIT`
  a huge result took 15 s to reach the row cap.
- **SQL Server (critical):** a blocked call could hold one of the four slots
  forever (now a 60 s timeout); the first escape detection reported the AI's
  most common mistakes (conversion, date, `UNION` errors) as escapes, since
  those roll the transaction back even with `XACT_ABORT OFF`; `XACT_ABORT` and
  the lock timeout came from the server's defaults; an escape followed by a
  result over the row cap went unreported.
- **MySQL/MariaDB:** `transaction_read_only` is `tx_read_only` on MariaDB 10.x
  and MySQL before 5.7.20; two more functions to block.
- **Postgres:** after an error or the row cap, `ROLLBACK` first read the rest
  of the result (4.6 s against 0.17 s on 20M rows).

The final review of the whole phase found nothing to fix in behaviour. It
added two Follow-ups (no row cap on the demo's read-only path; "Allow all"
ticked on another connection's card carries over) and four fixes to comments,
docs and log levels, made in Task 8.

As in phase 2b, the reviews found every way around the new mechanisms. The
plan's probes covered the attacks one would think of first, writes, DDL, two
statements, files, and each mechanism passed them on its first run. The
reviews went after what sits next to a query: process and database-wide state,
connection state that outlives a rollback, and SQL run by an extension on
another server.

### What was harder than expected

- **SQL Server.** The plan reused the held session under `hold_state`, and a
  probe showed session state that no rollback clears, so every call now opens
  its own connection, with a cap and a timeout. Detecting an escape needed a
  two-deep transaction, implicit transactions, the transaction id, a `#temp`
  marker and a TRY/CATCH around a nested `sp_executesql`, because SQL Server
  stops raising 266 in that mode and rolls a transaction back by itself on
  ordinary errors. Each of those came from a live result, not the docs.
- **DuckDB's surface.** A read-only transaction there limits writes to the
  database, not what a function can do. Logging, profiling, checkpoints, the
  UI server and the scanners for other databases are all SELECTs. The fix is a
  name list in the token check, the kind of blocklist the phase set out to
  stop relying on, checked against ~1,400 function names. MotherDuck's and
  DuckLake's weren't probed and are left for the owner to decide.
- **SQLite's "read-only".** `sqlite3_stmt_readonly` means the statement
  doesn't write the database file, not that it has no effects. The gate the
  plan chose over our own scanner still needed an authorizer, and then an
  allowlist that FTS5 could live with.
- **Stop.** Carrying the AI's `AbortSignal` down to the database took changes
  on every layer: the WebSocket handler, `server.js`, the approval promise and
  a demo that can't cancel at all.

What went to plan: the Core contract and its harness (~20 min), the
connection binding, and Postgres, where the review's one fix was about speed.

### Follow-ups and phase 3

None of the AI safety Follow-ups block phase 3. One fits it: a separate,
read-only login for AI queries is the only full fix on SQL Server and closes
the file-write and `SET GLOBAL` gaps elsewhere, and it needs per-connection
credentials, which phase 3's secrets work touches. The MotherDuck `md_*`
question is the owner's call and small either way.

For estimating: first passes again ran at about half their estimate, except
where the plan's mechanism turned out wrong. Review fixes stayed near 40%
where code decides what SQL runs, so keep that rate.

## Phase 3 cost

Source: `2026-09-29-phase-3-effort.md` and the phase 3 plan's execution
notes, plus line counts measured against the AI safety commit (`fecb1b4`);
Part 1 is the `Phase 3a` commit and Part 2 is in the working tree on top of
it. Times are framed as in the earlier phases: agent wall time as logged,
review fixes included, but not the plan, the review passes themselves or the
owner's checkpoints. Tasks 1–2, 3–5 and 10–13 ran partly in parallel, so the
calendar time was shorter than the sum.

### Time per task

| Task | Estimate | First pass | Review fixes | Logged |
|---|---|---|---|---|
| 1. Crates, rules, keychain check | 0.75–1 h | ~0.3 h | — | ~0.3 h |
| 2. Freeze the TS storage baseline | 1–1.5 h | ~1.1 h | — | ~1.1 h |
| 3. Storage: open, data dir, baseline | 1.5–2 h | ~0.7 h | ~0.4 h | ~1.1 h |
| 4. Storage: the typed queries | 3–4 h | ~1.4 h | ~0.6 h | ~2 h |
| 5. `seaquel-secrets` | 0.75–1 h | ~0.4 h | ~0.25 h | ~0.7 h |
| 6. `Workspace`, `Request`, `core_call` | 1.5–2 h | ~0.8 h | ~0.4 h | ~1.3 h |
| 7. Web `/rpc`, `/api/rpc` | 1.5–2 h | ~0.75 h | ~0.6 h | ~1.3 h |
| 8. TS `StorageClient`, deletions | 3–4 h | ~1.25 h | — | ~1.25 h |
| 9. Legacy refusal screen | 0.75–1 h | ~0.7 h | ~1 h | ~1.7 h |
| **Part 1** | ~14–18.5 h | ~7.4 h | ~3.25 h | ~10.7 h |
| 10. `seaquel-ssh`, tunnels in Core | 1.5–2 h | ~1.25 h | ~0.75 h | ~2 h |
| 11. `seaquel-git` | 1.5–2 h | ~0.9 h | ~0.5 h | ~1.4 h |
| 12. `seaquel_license::desktop` | 0.5 h | ~0.75 h | ~0.3 h | ~1.1 h |
| 13. `seaquel_license::server`, `/internal` | 4–5 h | ~2.5 h | ~1 h | ~3.5 h |
| 14. `browser`, CI, Docker (+ Decision 11b) | 1–1.5 h | ~1.25 h | ~1 h | ~2.25 h |
| 15. Docs, measure, checks | 0.75–1 h | ~0.75 h | — | ~0.75 h |
| **Part 2** | 9.25–12 h | ~7.4 h | ~3.6 h | ~11 h |
| Review fixes (the plan's own row) | 9–12 h | | | |
| **Total** | **~33–43 h** | **~14.8 h** | **~6.8 h** | **~21.7 h** |

The plan expected ~12–15 h of first passes and 5–7 h of fixes, about
**17–22 h logged**, if first passes ran at half their estimate as in phase 2b
and the AI safety phase. First passes came in at ~14.8 h, just inside;
fixes at ~6.8 h, near the top; the total at ~21.7 h, the top of the range
and about half the plan's 33–43 h. Review fixes were about 31% of the logged
time, under the 40% the plan budgeted for code that holds secrets.

The two parts cost the same, ~11 h each, though Part 2's estimate was
two-thirds of Part 1's. Part 1's first passes ran at about half their
estimate, as expected:
the typed storage port (Task 4) and the call-site switch (Task 8), the two
largest rows, took ~1.4 h and ~1.25 h against 3–4 h each, because the
frozen fixtures made both mechanical. Part 2's first passes took ~7.4 h
against 9.25–12 h, much closer to the estimate. Task 12 took 1.5 times its
0.5 h estimate before review; Task 14 took up to twice its estimate, since
it absorbed Decision 11b (SQLite and DuckDB off on web),
which wasn't in the plan; and Task 13's review round added the
`/internal` secret, the CA and proxy handling and the 503 page. Task 9's
review took longer than its first pass, because it found that a failed load
followed by a save wiped the collection, and the fix touched every store
that replaces a whole collection.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~12,560 | ~1,620 |
| Rust, tests (test files, inline `#[cfg(test)]`) | ~10,850 | ~30 |
| Fixtures (storage schemas and repo cases, JSON and SQL) | ~14,600 | — |
| TypeScript/Svelte/JS, production | ~3,220 | ~4,220 (net ~−1,000) |
| TypeScript, tests | ~3,830 | ~2,590 |
| Generated TS types | ~530 | — |
| Build and dev scripts (JS/TS) | ~300 | ~15 |

Measured with `git diff -U0` against `fecb1b4` plus the untracked files,
with `Cargo.lock`, `package-lock.json`, the docs and the message files left
out; inline test modules counted from their `#[cfg(test)]` line. The
~1,810-line storage recorder is kept in `docs/plans/artifacts` and isn't
counted. The Rust removed is mostly `src-tauri`'s `ssh_tunnel.rs` (455),
`git.rs` (890) and `license.rs` (171).

Where the production Rust went: the five new crates have ~8,450 lines
(`seaquel-storage` ~3,440, of which the frozen baseline is ~660;
`seaquel-license` ~3,260, of which the server side is ~2,900;
`seaquel-git` ~970; `seaquel-ssh` ~460; `seaquel-secrets` ~320).
`seaquel-types` grew by ~1,040 (the storage row types are ~840),
`seaquel-rpc` by ~1,230 (the workspace RPC is ~830), `seaquel-server` by
~1,210 (the workspace LRU, `/rpc`, `/internal/license/*`, `web_config.rs`
and the startup checks), Core by ~410 and `src-tauri/src/lib.rs` by ~280 (the lazy workspace
and `core_call`, with their tests).

The TypeScript that went: the storage backends, the JSON import and
`MigrationManager` (~2,230 lines with the storage routes), and the Node
licensing and air-gap code (~1,390 lines, plus ~2,040 lines of its tests,
now ported to Rust). The Rust that replaced the licensing code is about
twice its size, as the ports in phases 2 and 2b were: canonical JSON and the
bundle checks had to reproduce JavaScript's number, string, base64 and sort
semantics by hand.

The Rust tests are about the size of the new production Rust (~10,850
against ~10,940 net), and the storage fixtures add ~14,600 lines.

### Bugs found

By who found them first, counted from the effort log. "Implementer" means a
fixture, live test or probe while building the task, including the fix
round; "review" means a review round after it. The bracketed number is how
many were older than phase 3, in released code.

| Area | Implementer | Review |
|---|---|---|
| Storage and data safety | 6 [6] | 3 [1] |
| Web server and Node proxy | — | 3 [3] |
| Web engines (Decision 11b) | 2 [1] | 4 [3] |
| SSH | 2 [2] | 3 [2] |
| Git | 2 [2] | 4 [3] |
| Licensing | 2 [2] | 9 [1] |
| **Total** | **14 [13]** | **26 [13]** |

The serious ones were almost all older than phase 3, and moving the code is
what exposed them:

- **Web: any server file was reachable** (Task 13 review, confirmed by a
  probe). A signed-in web user could open a SQLite or DuckDB connection on
  any server path: `auth.db` (sessions, license keys, emails), every other
  user's `meta.db`, any readable file through DuckDB's `read_*`,
  `sqlite_scan` and `ATTACH`, and writes through `COPY TO`. Fixed by
  Decision 11b. The Task 14 review then found the same class through
  connection options: Postgres and MySQL TLS key and certificate paths,
  Unix sockets, and the operator's `PG*` variables and `~/.pgpass`, which
  sqlx reads.
- **Web: `/api/db` path traversal** (Task 7 review). The proxy built its
  upstream URL from a decoded path, so `/api/db/x%2F..%2Fquery` skipped the
  per-user connection check, and an encoded `..` could reach any Rust route.
- **Data loss after a failed load** (Task 9 and its fix round). A save after
  a failed load replaced the collection with the empty one in memory:
  projects, shared repos, project state, saved queries, a connection's
  history and an AI chat's messages. `Vault.setup` wrote a new vault over the
  old one when the read failed, orphaning every stored credential.
- **`v2026.4.5-beta.1` files didn't open** (Task 2 recording). The upgrade
  added columns before creating tables. Broken since `v2026.4.8`, and each
  launch left the file half-upgraded.
- **Git** (Task 11 and its review). The commit after resolving a conflict
  had one parent and left the merge open, so the next push was rejected and
  the next pull conflicted again; a push the remote refused over HTTP or SSH
  reported success; a wrong password could loop through the same
  credentials; a commit went through with conflicts still in the index; the
  conflict list was empty after a restart mid-merge.
- **SSH** (plan research and the Task 10 review). Closing a tunnel left the
  session and its forwards up; the trust-on-first-use retry was a second
  connection that recorded whatever key it met; a tunnel stayed open after a
  connection was switched off or failed to connect.
- **Licensing TLS** (Task 12 review). `reqwest::Client::new()` panics when
  the OS certificate store holds only invalid certificates, and the desktop
  built its client at startup; now built lazily with a fallback. The Task 13
  review found the port had dropped `NODE_EXTRA_CA_CERTS`, which Node's fetch
  honoured, and that a `tenant-info` answer without a tenant id wiped the
  stored tenant, as the TS did.

In the new code, before it shipped: the first password-stripping migration
was quadratic (280 s on the review's input) and truncated at NUL bytes; the
check for a corrupt file could change the file it was checking; Cargo's feature
unification gave `/rpc` on web real SSH tunnels and put SQLite and DuckDB
back into the server in workspace builds; `/internal/*` accepted in-process
loopback requests; a gate call racing an invalidation cached the stale
answer; and the web build's DuckDB-WASM assets leaked into the desktop and demo builds
(75 MB each).

As in phase 2b and the AI safety phase, the reviews found more than the
implementers. What changed is where: most of what the reviews found here is
older than the phase, around the code being moved rather than in the
ports.

### Size of the web image

The Rust stage of the Docker build went from 157 s to 58 s cold (dependency
build 133 s to 35 s), and the stripped server binary from 47.6 MB to 14.4 MB,
no longer linking libstdc++ (DuckDB). The runtime stage drops `libssl3`. The
web client grew by 75.2 MB for the DuckDB-WASM assets the tutorial now loads
from the image; the full image is 432 MB.

### What was harder than expected

- **Cargo feature unification.** The plan kept infrastructure out of the web
  build with features. `cargo test --workspace` compiles every feature into
  every crate, so `/rpc` on web had real SSH tunnels and the server had
  SQLite and DuckDB in test builds. Twice the fix was a check in code that
  holds whatever Cargo compiled in: `dispatch_workspace` refuses SSH, git and
  license calls, and the server registers engines by id.
- **Byte-exact JSON.** Stored JSON columns had to stay byte-identical, which
  ruled out `serde_json::Value` (it sorts keys) and made the RPC depend on
  `method` coming before `params`, and the desktop's `core_call` take raw
  bytes. Canonical JSON for air-gap bundles had to match JavaScript's
  `JSON.stringify`, `atob`, `TextDecoder`, `parseInt` and UTF-16 sort order
  by hand.
- **The web's trust boundary.** The plan assumed loopback was enough for
  `/internal/*`, that the Rust service's environment was harmless, and that
  a connection string was just a string. Each was wrong on a server: a
  query inside the process is loopback, sqlx reads `PG*` and `~/.pgpass`,
  and a file engine or a TLS key path reads the server's files.
- **The test SSH server.** linuxserver/openssh-server ships forwarding off
  and OpenSSH 10's per-source penalties refused the parallel tests after one
  wrong password, so the container needed its own init script, and CI runs
  it as a step rather than a service.

What went to plan: the storage port, which matched all 76 recorded cases
with their stored rows exact; the keychain, whose entries read back unchanged; and the
wasm32 build of Core, which needed one `uuid` feature and one `cfg`.

### Follow-ups and phase 4

Phase 4 (`seaquel mcp`) is the first second process to write the metadata
file, and phase 3 left it two jobs: `data_version` polling with
`StorageChanged`, and serialising opens across processes and pools, since
sqlx's migrate lock is a no-op on SQLite. Both are in its scope already. It
also verifies the macOS keychain access group on a signed build (see
Risks). Nothing else in the Follow-ups blocks it.

For estimating phase 4: first passes ran at about half their estimate where
the plan's mechanism held (Part 1) and close to the estimate where it
didn't (Part 2). Budget 40% for review fixes on anything a network client
or another process can reach, and put a probe of the trust boundary in the
plan itself rather than leaving it to review.

## Phase 4 cost

Source: `2026-09-30-phase-4-effort.md` and the phase 4 plan's execution
notes, plus line counts measured against the phase 3 commit (`5d555e0`);
phase 4 is in the working tree on top of it. Times are agent wall time as
logged, review and probe fixes included, but not the plan, the review passes
themselves or the owner's checkpoints. Tasks 1–2, 3–5 and 7–9 ran partly in
parallel. The probe's two runs aren't in the effort log; their times come
from the timestamps of their scratch files.

### Time per task

| Task | Estimate | First pass | Fixes | Logged |
|---|---|---|---|---|
| 1. Crates, rules, pins | 0.5–0.75 h | ~0.6 h | — | ~0.6 h |
| 2. Freeze the TS connect-config baseline | 1–1.5 h | ~0.8 h | — | ~0.8 h |
| 3. `seaquel-workspace`, `connect_saved` | 2–3 h | ~1.25 h | ~0.7 h | ~1.9 h |
| 4. Read-only storage, migration lock | 1–1.5 h | ~1.1 h | — | ~1.1 h |
| 5. `max_rows` (Core, 5 engines, in-app AI) | 2–3 h | ~1.7 h | — | ~1.7 h |
| 6. `seaquel-mcp`, `seaquel-cli mcp` | 3–4 h | ~1.8 h | ~2 h | ~3.8 h |
| 7. Trust-boundary probe (two runs) | 1–1.5 h | ~0.9 h | ~4.9 h | ~5.8 h |
| 8. Sidecar, release, menu, PATH | 2–3 h | ~1.8 h | — | ~1.8 h |
| 9. MCP settings panel | 1–1.5 h | ~0.75 h | — | ~0.75 h |
| 10. Docs, measure, checks | 0.75–1 h | ~0.4 h | — | ~0.4 h |
| Review fixes (the plan's own row) | 6–8 h | | | |
| **Total** | **~21–29 h** | **~11.1 h** | **~7.6 h** | **~18.7 h** |

Task 8's first pass includes the ~0.6 h rerun after the disk filled up
(below). Task 6's fixes are the review round on the MCP layer (~1.6 h) and
two small follow-ons: EXPLAIN and timeout wiring, and the `-32700` id. Task
7's fixes are the five the probe led to: the SQL Server `\r` comment
(~0.5 h), restricted DuckDB (~0.7 h), read-only EXPLAIN with a server-side
timeout and cancel (~2 h), DuckDB json linked statically (~0.6 h) and a
byte budget in the drivers (~1.2 h).

The plan expected about **13–17 h logged**. It came in at ~18.7 h: above that,
and about two-thirds of the plan's 21–29 h. First passes took ~11.1 h against
14.25–20.75 h, between 53% and 78% of their estimates, less of a saving than
phase 3's Part 1 because most tasks here were new mechanisms rather than
ports. Fixes took ~7.6 h, the top of the 6–8 h the plan budgeted, and 41% of
the logged time, right on the 40% the phase 3 notes advised. What the plan
didn't foresee is where they came from: the probe, estimated at 1–1.5 h,
cost ~5.8 h once its fixes are counted, more than any other task.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~7,260 | ~290 |
| Rust, tests (test files, inline `#[cfg(test)]`) | ~7,350 | ~60 |
| Fixtures (107 connect-config cases, `seaquel-sql` fixture updates, test migrations) | ~6,050 | — |
| TypeScript/Svelte, production | ~540 | ~45 |
| TypeScript/JS, tests | ~480 | ~35 |
| Generated TS types | ~25 | — |
| Build and dev scripts (`build-cli.mjs`, `tauri.mjs`) | ~220 | — |

Measured with `git diff -U0` against `5d555e0` plus the untracked files, with
`Cargo.lock`, `package-lock.json`, config files, the docs and the message
files left out; inline test modules counted from their `#[cfg(test)]` line.
The ~650-line connect-config recorder in `docs/plans/artifacts` isn't counted.

Where the production Rust went: `seaquel-mcp` ~2,520 (with ~2,280 lines of
tests), `seaquel-workspace` ~950, `src-tauri` ~710 (`cli_install.rs`,
`cli_info.rs`, the menu), Core ~700 (`connect_saved`, the tunnel ownership,
`QueryOptions`, `explain_read_only`), `seaquel-storage` ~410, `seaquel-cli`
~380, `seaquel-engine` ~330 (`RowCap`, `ReadOnlyOptions`, the trait
methods), the testkit ~230 and ~880 across the five engines. The tests
outweigh the production code, mostly the MCP server's and the engines' live
read-only tests.

### Bugs found

By who found them first, counted from the effort log. "Implementer" means a
fixture, live test or check while building the task; "review" a review round
after it; "probe" Task 7's attack on the running server, or the fixes it
led to. The bracketed number is how many were older than phase 4.

| Area | Implementer | Review | Probe |
|---|---|---|---|
| Connecting saved connections | 3 [3] | 3 | — |
| Read-only queries (Core, engines, `seaquel-sql`) | 1 | — | 5 [3] |
| MCP server | — | 9 | — |
| Packaging | 1 | — | — |
| **Total** | **5 [3]** | **12** | **5 [3]** |

The serious ones:

- **SQL Server `\r` comments hid a write from the read-only check** (probe).
  `SELECT 1 AS a -- x\rDELETE … COMMIT COMMIT` passed, because the scanner
  ended `--` at a lone `\r` only on Postgres and DuckDB, and SQL Server does
  too. It affected the in-app AI as well as MCP. The check now also reads
  every input with `\r` ending comments, on every engine.
- **EXPLAIN could write** (probe). The editor's `explain` isn't meant for
  untrusted SQL: Postgres folds immutable functions while planning, MariaDB
  evaluates `NEXTVAL` in a derived table or a primary-key lookup, SQLite's
  EXPLAIN runs every statement and DuckDB's all but the first. `explain_read_only` plans
  one statement inside the engine's read-only transaction or session.
- **DuckDB could read any file the user can** (probe): `read_csv`, a path as
  a table, `glob`, `ATTACH`. The MCP server's instances are now restricted.
  The in-app AI keeps the gap, documented.
- **A timed-out query kept running on the server** (probe; an AI safety
  follow-up). Dropping a sqlx stream doesn't cancel a Postgres or MySQL
  statement. Now the database's own timeout is set, and a drop sends
  `pg_cancel_backend` or `KILL QUERY` from a fresh connection.
- **No byte limit** (review, then probe). A result was bounded only by rows,
  so 1,000 rows of 1 MB took 1.2 GB of memory. The MCP layer now cuts cells
  and results, and the drivers stop fetching at a byte budget (peak RSS
  121 MB on SQLite and 82 MB on Postgres for the same query).
- **Passwords with `%` or `+` failed to reconnect** (Task 2 recording). The
  URL password setter leaves them unencoded, so `50%off` became an invalid
  escape. Fixed in the app too.
- **SQL Server `verify-full` accepted any certificate** (Task 2 recording):
  `trust_cert` was on for every mode but `require`. Fixed in the app too.
- **A failed keychain read connected without a password** (Task 3 review).
  The TS treats a failed read as no secret; `connect_saved` refuses with
  `SECRET_UNREADABLE` instead.

The MCP review's nine were in new code: no output byte limits, non-JSON
input dropped silently, saved queries listed for connections that don't
share their schema, a keychain prompt counted against the call timeout, a
process that hung after SIGTERM, a `{{name}}` missing from the stored
definitions not treated as a parameter, `sqlx::query` logging SQL at the default level, the DuckDB
lockdown not wired in, and a `-32700` reply without `"id": null`.

The probe found fewer bugs than the reviews but the worst ones, and three of
its five were older than the phase. Writing it into the plan, as phase 3
advised, paid off.

### What was harder than expected

- **The trust boundary needed the database, not just the check.** Decision 7
  first relied on the read-only check, the engines' read-only modes and
  dropping a stream. The probe showed each wasn't enough for a model with
  direct access: EXPLAIN, a server that keeps running, a DuckDB instance
  that reads files, and memory bounded only by rows. Each fix went into the
  drivers.
- **DuckDB extensions.** Locking the instance meant no autoload, so JSON
  functions stopped working until `json` was linked in. `icu` can't be:
  duckdb-rs's `icu` feature needs a DuckDB source checkout the crates.io
  package leaves out, so it would mean a git dependency and cmake on every
  runner. Time zones stay broken on the MCP server.
- **Two connect paths in the GUI.** The TS connects through autoReconnect or
  through the reconnect tab's rebuild, and some rows (every shared import)
  only ever connect through the second. The fixtures had to record which
  path the GUI takes per row.
- **rmcp's edges.** It drops a non-JSON line without a reply and offers no
  hook, so seaquel-mcp has its own line transport; its default server info
  reports rmcp's own name; tokio's blocking stdin read kept the process
  alive after a signal.
- **Disk and load.** Each parallel agent kept its own target directory
  (up to 56 GB each with DuckDB), and during Task 8 the disk filled up, so
  its first pass couldn't build or test anything and was rerun on a clean
  directory. Load averages of 25–34 made one wasm parity test exceed
  vitest's 5 s timeout.

What went to plan: the read-only storage open and the migration lock; `max_rows`
on all five engines, live, first time; the Rust port of the connect path,
which matched all 107 recorded cases except the five the two fixes changed
(the `url` crate matched WHATWG `new URL` on every case); and the sidecar in
the bundle, which printed its version from inside `Seaquel.app` on the first
local bundle build.

### What this means for phase 5

- **The GUI can move onto `connect_saved`.** The port, the fixtures and
  `HostKeyPolicy::Trust` exist. The quirks the fixtures kept (the README's
  list: MySQL `verify-*` unmapped, the rebuild dropping parameters, DuckDB
  keeping `?params` in the path, …) become decisions then, as does the
  MSSQL TLS server-name override for tunnelled connections.
- **The first second writer brings `StorageChanged`.** Phase 4 avoided it by
  not writing.
- **Estimating.** First passes ran at 53–78% of their estimates, not half:
  budget new mechanisms near the estimate. Keep a probe in the plan, and
  budget its fixes separately at about three times the probe itself; here
  they were the largest single cost. Share one target directory between
  agents, or clean them, before starting parallel work with DuckDB.

## Phase 5a cost

Source: `2026-10-01-phase-5a-effort.md` and the phase 5a plan's execution
notes, plus line counts measured against the phase 4 commit (`5d12ac6`);
phase 5a is in the working tree on top of it. Times are agent wall time as
logged, review and probe fixes included, but not the plan, the review passes
themselves or the owner's checkpoints. Tasks 4 and 5 ran partly in
parallel, and the probe fixes split into a Rust and a TS agent. The probe
run itself has no entry in the effort log; its scratch files span about ten
minutes, so it's counted at ~0.25 h with its setup.

### Time per task

| Task | Estimate | First pass | Fixes | Logged |
|---|---|---|---|---|
| 1. Form-path baseline, v2 fixtures | 1.5–2 h | ~1.25 h | ~0.4 h | ~1.7 h |
| 2. Core `connect`/`test`, one builder, ownership | 4–5.5 h | ~3 h | — | ~3 h |
| 3. `seaquel-rpc` `db` group, `CoreEvent` | 1.5–2 h | ~1.25 h | ~1.75 h | ~3 h |
| 4. Desktop transport | 1.5–2 h | ~1.25 h | ~0.4 h | ~1.7 h |
| 5. Web transport | 3–4 h | ~2.5 h | ~1.25 h | ~3.75 h |
| 6. TS onto Core | 3–4 h | ~2.5 h | ~1.75 h | ~4.25 h |
| 7. Trust-boundary probe | 1–1.5 h | ~0.25 h | ~3.5 h | ~3.75 h |
| 8. Docs, measure, checks | 0.75–1 h | ~0.4 h | — | ~0.4 h |
| Probe fixes (the plan's row) | 3–4.5 h | | | |
| Review fixes (the plan's row) | 6–8 h | | | |
| **Total** | **~26–35 h** | **~12.4 h** | **~9.05 h** | **~21.5 h** |

Task 1's fixes are the round after the owner settled Decision 6's open
choices (A, B and I), not a review. Task 3's are two review rounds, and
most of them landed in Core: `ConnectPolicy` replaced a Cargo feature gate
that feature unification defeated, and the tunnel refusal moved before any
secret read. Task 6's are two review rounds, mostly on the connection
string. Task 7's are the probe's fixes (~2.5 h, with the TS origin work
in its own agent) and their review round (~1 h).

The plan expected about **18–25 h logged**. It came in at ~21.5 h, in the
middle of that range and about two-thirds of the plan's 26–35 h. First
passes took ~12.4 h against 16.25–22 h, 56–76% of their estimates, about
the same band as phase 4. Fixes took ~9.05 h, 42% of the logged time: review
fixes ~5.2 h against 6–8 h, and probe fixes ~3.5 h, inside the 3–4.5 h the
plan set at three times the probe's estimate. The probe itself took far less
than its 1–1.5 h, since the endpoints were few and scripted; its fixes did
not.

### Lines

| | Added | Removed |
|---|---|---|
| Rust, production | ~5,170 | ~1,670 |
| Rust, tests (test files, inline `#[cfg(test)]`) | ~6,810 | ~2,330 |
| Fixtures (160 v2 connect-config cases, 53 recorded form cases) | ~9,320 | — |
| TypeScript/Svelte/JS, production | ~2,410 | ~2,050 |
| TypeScript/JS, tests | ~2,400 | ~910 |
| Generated TS types | ~175 | ~11 |

Measured with `git diff -U0` against `5d12ac6` plus the untracked files,
with `Cargo.lock`, config files, the docs and the message files left out;
inline test modules counted from their `#[cfg(test)]` line. The recorder
and the v2 checker in `docs/plans/artifacts` aren't counted.

Where the production Rust went: Core ~1,460 (`connect`/`test`, ownership,
`ConnectPolicy`, `ConnectionLimits`, events, early cancel), `seaquel-workspace`
~1,010 (the one builder, the form mapping), `seaquel-server` ~880
(`/rpc/stream`, eviction, the per-user event hub, status codes),
`seaquel-rpc` ~470 (the `db` group), `src-tauri` ~320 (`core_stream`,
`core_events`, per-webview sinks), and ~540 across the Postgres and MySQL
drivers and `seaquel-engine` for the server-side cancel. On the TS side,
`src/lib/core` (~780) and `CoreProvider` replaced the two providers, the
TS tunnel code and `/api/db` (~1,300 lines removed there), so TS production
code grew by only ~360 lines net.

### Bugs found

By who found them first, counted from the effort log (a review finding
that bundles several small ones counts once per item). The bracketed
number is how many were older than phase 5a. The two origin findings
touched both old routes and the new stream proxy, so they aren't split.

| Area | Implementer | Review | Probe |
|---|---|---|---|
| Connect semantics (fixtures, live checks) | 6 [6] | — | — |
| Core API and policy | — | 6 | — |
| Desktop transport | — | 2 | — |
| Web transport and proxy | — | 6 | 1 [1] |
| TS client and connection string | — | 12 | — |
| Engines (cancel, logging) | — | 1 | 2 [2] |
| Web origins | — | 1 | 2 |
| **Total** | **6 [6]** | **28** | **5 [3+]** |

The serious ones:

- **SQL in the logs** (probe). sqlx logs every statement at DEBUG and any
  statement slower than a second at WARN, whole. On web that put users'
  SQL, and anything in it, in the server log. Statement logging is now off
  on every sqlx pool and connection, and the log filters drop the target.
- **No per-user connection cap** (probe). One web user could open as many
  pools as the databases allowed. `ConnectionLimits` now caps a workspace
  at 16 connections with pools of 6, counting connects in flight.
- **A stopped stream kept running on the server** (probe). Cancel, disconnect
  and eviction dropped the stream but not the Postgres or MySQL statement.
  The drivers now send `pg_cancel_backend` or `KILL QUERY`, guarded by the
  statement's prefix; the review found the MySQL guard compared text MySQL
  reports differently, so it never fired for bound parameters.
- **Dev origins trusted in production, and no Origin check on most `/api`
  mutations** (probe). Fixed with the Origin gate; its review then found the
  Host fallback could be reached through DNS rebinding, so it now applies
  only to `localhost` and IP-address hosts.
- **The feature gate on `connect` didn't hold** (Task 3 review). Cargo unifies
  features, so a web build with tests could connect over SSH. `ConnectPolicy`
  has no default and is checked in code.
- **A stale connection string** (Task 6 review). Core obeys a stored string,
  so a string the old GUI rebuilt from the fields would win over a later
  field edit. Strings now live only while visible, and old rows lose them.
- **The string migration's save wiped AI sharing flags** (Task 6 second
  review). The load mapping dropped `aiShareSchema`/`aiShareData`.
- **The recorded form path lost data** (Task 1): a password typed after
  pasting a string never reached the driver, an empty username dropped the
  password, port 0 went out as `:0`, and a pasted `mssql://` URL connected
  unencrypted. All fixed by the one builder.

### What was harder than expected

- **Tenancy is more than ownership checks.** Ownership held on the first
  probe: no cross-user access by any route. What the probe found was
  everything around it: logs, caps, cancel and origins. A multi-user server
  needs those checked as deliberately as the ids.
- **WebSocket lifecycle.** Re-authorising a long-lived socket, telling a lost
  session (1008) from a gate outage or a socket cap (1013) so the client
  reconnects only when it should, splitting large batches, and closing
  after 12 h each came from review, not the plan.
- **Channel ordering on desktop.** A Tauri command's reply can overtake its
  channel messages, so "the stream ended with no terminal event" wasn't
  decidable until `core_stream` returned the count it sent.
- **The connection string.** Moving connects to Core made the stored string
  authoritative, which exposed every place the old GUI had rebuilt it. The
  visibility rule and the one-time migration took two review rounds.

What went to plan: the one builder, which passed all 160 v2 cases on its
first run; ownership, which the probe couldn't break; the `db` RPC group,
whose 16 tests passed first time; and the live stream path, where a 50,000-row
stream and a `pg_sleep` cancel worked through the new client on the first run.

### What this means for phase 5b onwards

- **Query execution in Core (5b)** can build on `db.queryStream` and the
  per-workspace stream registry; nothing in the transports needs to change.
- **`CONNECTION_CLOSED` and `TUNNEL_CLOSED` are still reserved.** Core can't
  see a lost connection or a dropped tunnel yet; the GUI finds out on the
  next call.
- **Estimating.** First passes ran at 56–76% of their estimates again. Keep
  the probe and its separate fix budget, and expect a multi-user feature's
  review to find lifecycle and limits issues the plan didn't list.

## Risks

- **Port size.** About 5k lines of dialect code and 14k lines of state
  managers, plus 1.2k of tutorial parser. Phase 1 is sized to measure the real
  cost before committing to the rest.
- **sqlparser-rs vs node-sql-parser.** The query builder's two-way sync and the
  tutorial's lesson criteria depend on node-sql-parser's AST shape and its
  PostgreSQL dialect quirks. Parity fixtures will surface differences. Some
  lesson criteria may need rewriting rather than porting.
- **The demo gap.** The browser build of Core needs the domain services, so it
  can't ship before phase 5. The plan was to delete the TS dialect code in
  phase 2 and pin the demo to the last build before it. Phase 2 kept
  `duckdb.ts` for the demo instead, so the demo still gets new features, at
  the cost of maintaining one TS dialect next to the Rust one until phase 8.
  DuckDB bugs fixed in Rust are listed as demo follow-ups in the phase 2 plan.
- **WASM size and startup.** Core, sqlparser-rs and SQLite compiled to WASM,
  plus DuckDB-WASM, is a heavy page. Budget it in phase 8 and use `wasm-opt`
  and lazy loading of the AI and dashboard modules if needed.
- **Keychain compatibility.**
  - Entries written by `tauri-plugin-keyring` must be readable by the `keyring`
    crate under the same service and account names. Check this before phase 3.
  - On macOS, the bundled `seaquel` is signed by the same team as the app. Items
    also need a shared keychain access group, or the first read from the CLI
    still shows an access prompt. Verify this on a signed build in phase 4,
    because that's the first time a second binary reads the keychain.
- **AI on web.** Moving LLM calls server-side means the tenant container makes
  outbound calls to `api.anthropic.com` or a custom base URL. Air-gapped
  installs need that to be off or configurable. It also means API keys transit
  the server per request, same as database passwords today.
- **Concurrent writers.** The desktop app and `seaquel mcp` writing the same
  SQLite file is new. WAL and busy timeouts handle correctness. Stale GUI state
  is handled by `StorageChanged` events, but only for data Core knows was
  written by another process.
- **Build times and binary size.** Bundled DuckDB, vendored libgit2 and OpenSSL
  are slow to compile and large. Feature flags per engine help, and CI needs
  good caching.

## Open questions

None at the moment. The six questions from the first draft are resolved as
decisions 9 and 12–16 above.

Two choices are left to the spikes and don't block the plan:

- The browser storage backend: rusqlite on `sqlite-wasm-rs`, or a bridge to
  sql.js. This is decided in phase 8.
- The keychain access group setup on macOS. Settled in phase 4: no access
  group; the CLI accepts one prompt per item (see "Terminal binaries").
