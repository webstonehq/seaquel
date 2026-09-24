# Rust Core and Plugin Architecture

**Date:** 2026-09-24
**Status:** Draft

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
| 9 | Web auth | Better Auth, signup, team and license gating stay in the SvelteKit/Node layer. They're web-specific |
| 10 | Terminal binaries | One `seaquel` binary: `seaquel <cmd>` for CLI, `seaquel tui`, `seaquel mcp` |
| 11 | Migration | Strangler pattern, one engine or subsystem at a time. The app ships working at every step |

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
| `seaquel-engine` | pure + async traits | `Engine`, `Dialect`, `Connection` traits, `Capabilities`, `EngineRegistry`, `DbError` | `crates/seaquel-db/src/lib.rs`, `db/index.ts` |
| `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}` | plugin | Driver, value decoding, dialect, introspection, EXPLAIN parsing. The `mysql` crate also registers `mariadb` | `crates/seaquel-db/src/*`, `db/{postgres,mysql,sqlite,mssql,duckdb}.ts`, `db/alter-table.ts`, `db/crud-helpers.ts`, `db/parse-create-table.ts` |
| `seaquel-engine-testkit` | dev | Conformance suite every engine must pass | new |
| `seaquel-storage` | infra | App metadata SQLite: schema, migrations, repos, legacy JSON import | `storage/*`, `src/lib/server/storage.ts`, `storage-guard.ts` |
| `seaquel-secrets` | infra | `SecretStore` trait, OS keychain implementation (`keyring` crate) | `services/keyring.ts` |
| `seaquel-ssh` | infra | SSH tunnels (russh) | `src-tauri/src/ssh_tunnel.rs`, `services/ssh-tunnel.ts` |
| `seaquel-git` | infra | git2 operations and credential chain | `src-tauri/src/git.rs`, `services/git.ts` |
| `seaquel-license` | infra | Desktop activation/validation, control-plane client, air-gap bundle verification | `src-tauri/src/license.rs`, `src/lib/server/licensing.ts`, `license-cache.ts`, `airgap/*` |
| `seaquel-workspace` | domain | Connections, projects, labels, saved queries and versions, history, dashboards and versions, workflows, shared repo file format (`.seaquel/` YAML and frontmatter SQL), importers, exporters, connection strings | `hooks/database/*` (core parts), `services/*-parser.ts`, `services/{dbeaver,tableplus}-import.ts`, `utils/{connection-string,export-formats,query-versions,dashboard-versions,cell-type}.ts` |
| `seaquel-ai` | domain | LLM provider clients, tool registry, tool loop, @mention expansion | `services/ai/*`, `services/ai-mentions.ts` |
| `seaquel-core` | orchestrator | `Core`, `Workspace`, execution services, event bus | `hooks/database.svelte.ts` and managers (logic only) |
| `seaquel-rpc` | interface glue | Request/response/event enums and a dispatcher onto `Workspace` | new; replaces `providers/*` and `/api/storage/*` |
| `seaquel-wasm` | interface glue | wasm-bindgen exports of pure crates for the GUI | new |
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

```rust
pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;               // "postgres"
    fn aliases(&self) -> &'static [&'static str]; // ["mariadb"] for mysql
    fn capabilities(&self) -> Capabilities;      // replaces the TS optional methods
    fn dialect(&self) -> &dyn Dialect;
    async fn open(&self, cfg: &ConnectConfig) -> Result<Box<dyn Connection>, DbError>;
}

/// Pure. No I/O, compiles to WASM.
pub trait Dialect: Send + Sync {
    fn quote_ident(&self, id: &str) -> String;
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String;
    fn build_insert(&self, t: &TableRef, values: &Row, casts: &CastLookup) -> SqlWithBindings;
    fn build_update(&self, /* … */) -> SqlWithBindings;
    fn build_delete(&self, /* … */) -> SqlWithBindings;
    fn create_table(&self, def: &TableDefinition) -> Result<String, DbError>;
    fn alter_table(&self, from: &TableDefinition, to: &TableDefinition) -> Result<Vec<String>, DbError>;
    fn parse_create_table(&self, ddl: &str) -> Result<TableDefinition, DbError>;
    fn column_types(&self) -> &'static [ColumnTypeInfo];
    fn sql_dialect(&self) -> Box<dyn sqlparser::dialect::Dialect>;
    fn explain_sql(&self, sql: &str, analyze: bool) -> String;
    fn parse_explain(&self, result: &QueryResult, analyze: bool) -> Result<ExplainResult, DbError>;
}

#[async_trait]
pub trait Connection: Send + Sync {
    async fn query(&self, sql: &str, params: &[Value]) -> Result<QueryResult, DbError>;
    async fn execute(&self, sql: &str, params: &[Value]) -> Result<ExecuteResult, DbError>;
    async fn transaction(&self, stmts: Vec<Statement>) -> Result<(), DbError>;
    fn query_stream(&self, sql: String, params: Vec<Value>, cancel: CancellationToken)
        -> BoxStream<'_, Result<StreamBatch, DbError>>;

    // Introspection: the engine picks the SQL.
    async fn schemas(&self) -> Result<Vec<String>, DbError>;
    async fn tables(&self) -> Result<Vec<SchemaTable>, DbError>;
    async fn columns(&self, t: &TableRef) -> Result<Vec<SchemaColumn>, DbError>;
    async fn indexes(&self, t: &TableRef) -> Result<Vec<SchemaIndex>, DbError>;
    async fn statistics(&self) -> Result<DatabaseStatistics, DbError>; // gated by Capabilities

    async fn close(&self);
}
```

Things this fixes along the way, which are cheaper to do now than after five
engines implement the trait:

- **Typed values.** Cells are plain `serde_json::Value` today. Bigints lose
  precision in the browser, bytea arrives as a number array (base64 for MSSQL),
  and DuckDB lists/structs become Debug strings. `seaquel_types::Value` is an
  enum (`Null`, `Bool`, `Int`, `BigInt`, `Float`, `Decimal`, `Text`, `Bytes`,
  `Json`, `Date`, `Time`, `Timestamp`, `Uuid`, `Array`, `Other { type_name, text }`)
  with one wire encoding. The CLI needs types to format output anyway.
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
| SvelteKit/Node (web) | Better Auth, signup, team, account routes, license gate in `hooks.server.ts`, static serving, loopback proxy | `/api/storage/*`, connection scoping, the licensing and air-gap logic (called through a small Rust endpoint or napi binding; see open questions) |
| `seaquel-server` | axum routing, WS, `X-Seaquel-User` handling | `/api/db/*` handlers (replaced by `/rpc`) |
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

## Testing

"Developed and tested independently" is the point of the split, so each layer
gets a test suite that doesn't need the layers above it.

- **Dialect golden tests** (pure, no database): DDL, ALTER, CRUD SQL, EXPLAIN
  parsing and `parse_create_table` against snapshot files, using `insta`.
- **Parity fixtures during migration.** The TS adapters are pure, so a vitest
  script can record their outputs for a corpus of inputs as JSON. The Rust
  dialect tests assert the same output. This is how we port 5k lines without
  guessing. Delete the fixtures once the TS side is gone.
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

**Phase 1: Postgres engine pilot**
- Port `postgres.ts` and the shared helpers to `seaquel-engine-postgres`,
  checked by parity fixtures and the conformance suite.
- Add introspection and dialect RPC calls.
- In TS, `getAdapter("postgres")` becomes a shim over those calls, so the
  components don't change.
- Introduce the typed `Value` for Postgres.
- Measure the effort and adjust the plan before continuing.

**Phase 2: remaining engines, `seaquel-sql`, `seaquel-wasm`**
- Port MySQL/MariaDB, SQLite, MSSQL and DuckDB the same way.
- Replace node-sql-parser with sqlparser-rs.
- The editor, query builder and tutorial parser switch to `seaquel-wasm`.
- Delete `src/lib/db/*`.

**Phase 3: storage, secrets, infrastructure**
- Move `seaquel-storage` (and migrate the web backend off better-sqlite3),
  `seaquel-secrets`, `seaquel-ssh`, `seaquel-git` and `seaquel-license` into
  their crates.
- Introduce `Workspace` and `seaquel-rpc` for the moved pieces.
- Delete `storage/*` backends, `/api/storage/*` and most `src-tauri` commands.

**Phase 4: `seaquel mcp`**
- The first new interface, and the first real test of Core with a second
  consumer.
- Needs engines, storage and secrets. Nothing else.
- Ship it with the desktop release.

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

## Risks

- **Port size.** About 5k lines of dialect code and 14k lines of state
  managers, plus 1.2k of tutorial parser. Phase 1 is sized to measure the real
  cost before committing to the rest.
- **sqlparser-rs vs node-sql-parser.** The query builder's two-way sync and the
  tutorial's lesson criteria depend on node-sql-parser's AST shape and its
  PostgreSQL dialect quirks. Parity fixtures will surface differences. Some
  lesson criteria may need rewriting rather than porting.
- **The demo.** It runs entirely in the browser on DuckDB-WASM and sql.js. After
  phase 2 there's no TS dialect code for it to use, and after phase 3 no TS
  storage. See open question 1.
- **Keychain compatibility.**
  - Entries written by `tauri-plugin-keyring` must be readable by the `keyring`
    crate under the same service and account names. Check this before phase 3.
  - On macOS, a second binary (`seaquel`) reading items created by the desktop
    app triggers a keychain access prompt unless both are signed by the same
    team with a shared access group.
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

1. **Demo.** Options:
   - (a) Compile Core to WASM with a JS-bridged DuckDB-WASM engine and an
     in-browser storage backend. This needs `?Send` async variants.
   - (b) Run the demo against a hosted, sandboxed `seaquel-server` with
     throwaway DuckDB workspaces. This costs hosting, abuse controls and
     latency.
   - (c) Freeze the demo on the last pre-migration build.

   Recommendation: (c) as a stopgap from phase 2. Spike (a) after phase 3, when
   we know how much of Core is wasm-clean.
2. **Web licensing code.** `licensing.ts`, `license-cache.ts` and `airgap/*`
   move to `seaquel-license`, but `hooks.server.ts` needs their answers on
   every request. Options: call `seaquel-server` over loopback, use a napi-rs
   binding, or move the gate itself into `seaquel-server`. Loopback is simplest
   and matches the existing stream-access check.
3. **License terms for terminal binaries.** Per the 2026-09-18 license split,
   official binaries are free for personal use and paid for commercial use.
   Does `seaquel` check the license key from the keychain, or is it honour
   system?
4. **Bundling the CLI with the desktop app.** Ship `seaquel` inside the app
   bundle with a "Install command-line tool" menu item (as VS Code does), or
   distribute it separately via Homebrew/cargo-dist? Bundling also solves the
   keychain signing question.
5. **Auth in Rust.** Better Auth stays in Node for now. If the Node layer
   shrinks to auth plus static files, is it worth replacing with an axum auth
   layer and dropping Node from the image? That's out of scope here.
6. **Legacy JSON import.** `json-migration.ts` imports plugin-store data from
   before storage v4. Port it to Rust, or drop it and require users on very old
   versions to upgrade through an intermediate release?
