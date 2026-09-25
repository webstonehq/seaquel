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
  `column-sources.ts`) add more.

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
| `seaquel-sql` | pure | Statement splitter, `{{param}}` extraction/substitution, comment stripping, read-only validator, sqlparser-rs AST helpers (column sources, visual AST, query builder parse/generate), tutorial lesson checks | `db/sql-parser.ts`, `db/query-params.ts`, `db/query-utils.ts`, `db/column-sources.ts`, `db/sql-ast-parser.ts`, `tutorial/sql-parser.ts`, `tutorial/criteria.ts`, `hooks/query-builder-*.ts`, `services/ai/context.ts` (`validateReadOnlyQuery`) |
| `seaquel-engine` | pure + async traits | `Engine`, `Driver`, `Dialect` traits, generic DDL/CRUD builders, `EngineRegistry`, `DbError` | `crates/seaquel-db/src/lib.rs`, `db/index.ts` |
| `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}` | plugin | Driver, value decoding, dialect, introspection, EXPLAIN parsing. The `mysql` crate also registers `mariadb` | `crates/seaquel-db/src/*`, `db/{postgres,mysql,sqlite,mssql,duckdb}.ts`, `db/alter-table.ts`, `db/crud-helpers.ts`, `db/parse-create-table.ts` |
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
- **Storage.** `seaquel-storage` gets a `StorageBackend` trait. The native
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
- The keychain access group setup on macOS. This is verified in phase 4.
