# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Seaquel is a database client built with Tauri 2 + SvelteKit 5 + TypeScript, with a Rust core. It supports PostgreSQL, MySQL/MariaDB, SQLite, MSSQL and DuckDB through the Rust engine crates in `crates/`. It ships as a desktop app, a self-hosted web app (`seaquel-server` behind a Node/SvelteKit server) and a browser demo.

## Development Commands

```bash
# Start development (frontend + Tauri)
npm run tauri dev

# Build production app
npm run tauri build

# Type checking
npm run check

# Type checking (watch mode)
npm run check:watch
```

## Architecture

### Frontend (src/)

- **SvelteKit 5** with static adapter (SSR disabled for Tauri)
- **Svelte 5 runes** (`$state`, `$derived`, `$props`) for reactivity
- **Tailwind CSS v4** for styling
- **bits-ui** for accessible UI components (shadcn-svelte pattern)

### Backend (Rust)

All database logic lives in Rust crates under `crates/`, shared by every interface. See `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` for where this is heading.

- `seaquel-core` — the only entry point interfaces use: engine registry, open connections, streaming and cancellation. `disconnect` cancels the connection's running streams, which end with a `CONNECTION_CLOSED` error event.
- `seaquel-engine` — the `Driver`/`Engine` plugin traits, the pure `Dialect` trait, and the generic DDL/CRUD builders (`ddl.rs`, `crud.rs`) that dialects parameterize. `Driver` has default `NOT_SUPPORTED` introspection methods (`list_schemas`, `schema_tables`, `table_metadata`, `statistics`, `explain`). One crate per engine: `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}`.
- `seaquel-types` — wire types, including the dialect types (`SchemaTable`, `ExplainResult`, `CreateTableDefinition`, …) and `Value`. `npm run types:gen` regenerates `src/lib/types/generated/` from `seaquel-types` and `seaquel-rpc`; never edit those by hand.
- `seaquel-rpc` — `EngineCall`/`EngineRequest`/`EngineResponse` and `dispatch` onto Core, for dialect and introspection calls on one connection. Served as the `db_engine` Tauri command and `POST /api/db/engine`.
- `seaquel-runtime` — `MaybeSend`, `BoxStream`, `Executor`, `#[seaquel_runtime::async_trait]`. Core crates must build for wasm32: no `tokio::spawn`, `Instant` or `SystemTime` (enforced by `crates/clippy.toml`).
- Interfaces: `src-tauri/` (desktop; Tauri commands in `src/db/commands.rs` forward to Core) and `crates/seaquel-server/` (web; axum, loopback-only behind the Node server).
- `npm run crates:check` (`scripts/check-crate-deps.mjs`) enforces which crates may depend on which.
- Engine smoke tests: `cargo test -p seaquel-engine-<name> --test smoke`. Server engines need `SEAQUEL_TEST_<ENGINE>` set to ConnectConfig JSON (see each crate's `tests/smoke.rs`) and the containers from `e2e/test-databases/docker-compose.yml`, seeded with `npm run e2e:db:seed` (which creates `seaquel_test`).
- Desktop plugins still used: `tauri-plugin-store` (legacy JSON import), `tauri-plugin-updater`, `tauri-plugin-keyring`, `tauri-plugin-log` and others in `src-tauri/Cargo.toml`.

### Dialects and engine calls

- **UI code never does dialect work itself.** Introspection, EXPLAIN, statistics, pagination, CRUD and DDL generation all go through `EngineClient` (`src/lib/engine`): `getEngineClient(connection, state)`. Pass the app state so the client reads the live provider connection id on every call (it survives `reconnect()`). Don't cache clients across operations, and don't call `getAdapter(` outside `src/lib/engine/` and `src/lib/db/`.
- **Identifiers:** quote a name with `EngineClient.quoteIdent` and build a table name from a listed schema with `EngineClient.qualifiedTable` (DuckDB lists attached catalogs' schemas as `catalog.schema`, two names). Never hand-build `"${name}"`.
- `getEngineClient` returns `RustEngineClient` (the `db_engine` endpoint) for Postgres, MySQL, MariaDB, SQLite, MSSQL and DuckDB on desktop and web, and `TsEngineClient` (the TypeScript `DatabaseAdapter` plus a provider) for the browser demo, which has no Rust core. `src/lib/db/duckdb.ts` is the only TypeScript adapter left, and it is demo-only (as are `alter-table.ts` and `crud-helpers.ts`; `index.ts` only re-exports the `SqlWithBindings` type from the latter); `getAdapter` throws for every other engine. The rest of `src/lib/db` is engine-independent query-editor code (statement splitting, `{{param}}` substitution, query type detection, visual query parsing).
- **Every engine crate** has `dialect.rs` (the pure `Dialect`), `introspect.rs` (catalog SQL, parsers, EXPLAIN) and `decode.rs` (with `bind.rs` in all but DuckDB) for values. Postgres adds `numeric.rs`, the NUMERIC binary codec. `crates/seaquel-engine-mysql` serves MySQL and MariaDB: a `"mariadb"` connection connects with driver `"mysql"`.
- **SQLite:** cells decode by storage class (`typeof`), not declared type; BLOBs are bytes. SQLite has no `DEFAULT` in `UPDATE`, so Set to default sends the column's default expression from its metadata (`buildSetDefault`'s `columnDefault`). Edits SQLite can't make come back from `alterTable` as `-- …` note lines; the table editor shows them (`splitDdlScript` in `src/lib/utils/ddl-script.ts`).
- **MSSQL:** cells are native: decimal and money are `Decimal`, binary is `Bytes`, dates and times are SQL Server's own text (datetimeoffset as `2024-01-02 03:04:05.5 +01:00`). tiberius hands money over as an f64: it is exact only for |value| < 2^39 ≈ 5.5·10¹¹; above 2^53 units tiberius itself loses bits (up to ~1536 units near the ends of the range), and the maximum reads `922337203685477.5808`, which is out of range when bound back. A `Null` parameter is sent as the `NULL` literal (`inline_nulls`), since no declared type assigns to every column; so a column made only from it is int (`SELECT @P1 INTO`, `UNION`), `COALESCE`/`CASE`/`IIF` with only such NULLs fail (4127, 8133), and so does passing one as an `OUTPUT` argument (179). A `@Pn =` naming a parameter in an `EXEC` argument list is left alone. Query parameters (`{{name}}`) are inlined, strings as `N'…'`, so they work in `TOP`, `CREATE VIEW` and defaults. CRUD binds `@P1…`; EXPLAIN runs `SET SHOWPLAN_XML`/`STATISTICS XML` as separate batches and parses the ShowPlan XML with roxmltree; statistics are `NOT_SUPPORTED`.
- **DuckDB** uses the Rust crate on desktop and web; the demo keeps `duckdb.ts`, whose behaviour shouldn't change. Attached catalogs are listed as `catalog.schema`, with a part holding `.` or `"` double-quoted (`"fx.we""ird".main`, or `"a.b"` for a default-catalog schema named `a.b`); `system` and `temp` are left out. Build `"schema"."table"` from a listed schema with `Dialect::quote_schema` in Rust (the CRUD/DDL builders' `_qs`/`_with` variants take it) and `EngineClient.qualifiedTable` in TypeScript, never by quoting the schema as one name. CRUD binds `?`. Edits DuckDB rejects (constraints in `ADD COLUMN`, column changes while the table keeps an index, dropping or retyping a PRIMARY KEY/UNIQUE column or a column before one) come back from `alterTable` as notes; the rules read `isUnique` and `inUniqueConstraint` (any UNIQUE constraint, composite too), which DuckDB's `table_metadata` reports on `SchemaColumn` and the table editor copies. `{{param}}` values are inlined (desktop, web and demo), skipping comments and quoted names.
- **The MSSQL driver runs on one connection.** A call that doesn't finish (a UI cancel, a dropped request, `RESULT_TOO_LARGE`, a fatal error) closes it, and the next call reconnects. That loses `##global` temp tables, session context, app locks and any transaction opened by hand, which is silently rolled back. Code that changes session state (`SET SHOWPLAN_XML ON`, …) must wrap it in `Session::hold_state()`/`release_state()`; a failure before the matching OFF reconnects, with the same losses.
- **MSSQL statements run through `sp_executesql`**, with or without parameters, so a user's `SET` options and `#temp` tables end with the call and can't leak into introspection or grid edits. `USE` is the exception (it outlives an RPC call), so after a statement mentioning `USE` the driver switches back to the connection's database (`restore_database` in `driver.rs`). Only a parameterless statement that must start a batch (`CREATE VIEW`/`PROCEDURE`/`SCHEMA`, …; `must_start_batch`) and the driver's own BEGIN/COMMIT/ROLLBACK and EXPLAIN `SET … ON/OFF` go as plain batches. A plain EXPLAIN with parameters declares them without values, so its estimates are the generic ones.
- **MySQL/MariaDB TIMESTAMP values are shown as UTC wall-clock time**: sqlx sets the session `time_zone` to `'+00:00'`. Don't add a `timezone` to connection strings; in a zone with DST two instants print the same and a TIMESTAMP key would match the wrong row.
- The parity fixtures in `crates/seaquel-engine-*/tests/fixtures` were recorded from the TS adapters and are frozen; the recorder is gone (a reference copy is in `docs/plans/artifacts/`). Change a fixture only when the Rust behaviour is meant to change, and say why.

### Cell values

Rows and parameters cross the wire in one format (`crates/seaquel-types/src/value.rs`, `src/lib/values.ts`). Values JavaScript holds exactly are plain JSON. Everything else is tagged as `{"$sq": kind, "v": …}` with kind `bigint`, `float` (NaN/±inf), `decimal`, `bytes` (base64) or `json`. The providers decode tags before any UI code sees a row and encode parameters with `encodeParam`, so the UI gets `bigint`, `Uint8Array` and `SqlDecimal` alongside plain values. Use the helpers in `$lib/values` (`cellKey` for comparing cells, `cellText`, `toNumber`, `jsonReplacer` for `JSON.stringify`, `toStorable` for persisted rows) instead of `String()`/`Number()`/bare `JSON.stringify` on cells.

The DuckDB driver reads Arrow chunks itself (`crates/seaquel-engine-duckdb/src/decode.rs`). Temporal types are Text in DuckDB's own format (TIMESTAMPTZ always in UTC, `2024-01-01 12:00:00+00`), STRUCT/MAP are `Json` with sorted keys, and the driver turns on `arrow_lossless_conversion` for its connection.

### State Management

All app state is managed through a single reactive class in `src/lib/hooks/database.svelte.ts`:

- `UseDatabase` class uses Svelte 5 runes for reactivity
- Exposed via Svelte context (`setDatabase`/`useDatabase` pattern)
- Handles: connections, query tabs, schema tabs, query history, saved queries, AI messages

### Key Components (src/lib/components/)

- `query-editor.svelte` - SQL query editor with tab support
- `table-viewer.svelte` - Schema browser with table/column/index details
- `connection-dialog.svelte` - Database connection management
- `sidebar-left.svelte` / `sidebar-right.svelte` - Navigation sidebars
- UI components follow shadcn-svelte structure in `src/lib/components/ui/`

### Data Types (src/lib/types.ts)

Core interfaces: `DatabaseConnection`, `SchemaTable`, `QueryTab`, `QueryResult`, `SavedQuery`, `QueryHistoryItem`

## Configuration Files

- `src-tauri/tauri.conf.json` - Tauri app configuration
- `svelte.config.js` - SvelteKit config with static adapter
- `vite.config.js` - Vite bundler config

## Updating the Demo

The demo is a browser-based version using DuckDB WASM (instead of PostgreSQL) hosted at `seaquel.app/demo`.

From the website repo (`seaquel-app/main`), run:

```bash
npm run demo:update
```

This script removes old demo files, builds the demo with `BUILD_TARGET=demo`, and copies the output to `static/demo/`. Commit and deploy the website changes afterward.

## Releasing a New Version

Version format: `YYYY.month.patch` (e.g., `2026.1.1`)

1. Update version in these files:
   - `src-tauri/Cargo.toml`
   - `src-tauri/tauri.conf.json`
   - `src-tauri/Cargo.lock`
   - `package.json`
   - `package-lock.json`

2. Commit: `Bump version to X.Y.Z`

3. Create and push tag:

   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

4. GitHub Actions builds for macOS (Intel + ARM) and Linux (x86_64 + ARM64), signs binaries, and creates a draft release

5. Review and publish the draft release on GitHub

## Conventions

### Toasts

- For error toasts, always use `errorToast` from `$lib/utils/toast` — never `toast.error(...)` from `svelte-sonner`. `errorToast` renders an `ErrorToast` component that includes a copy button so users can copy the error message.
- For success/info toasts, continue using `toast.success(...)` / `toast.info(...)` from `svelte-sonner`.

## AI behaviour

Never commit any changes to git.

## Tools

You are able to use the Svelte MCP server, where you have access to comprehensive Svelte 5 and SvelteKit documentation. Here's how to use the available tools effectively:

## Available MCP Tools:

### 1. list-sections

Use this FIRST to discover all available documentation sections. Returns a structured list with titles, use_cases, and paths.
When asked about Svelte or SvelteKit topics, ALWAYS use this tool at the start of the chat to find relevant sections.

### 2. get-documentation

Retrieves full documentation content for specific sections. Accepts single or multiple sections.
After calling the list-sections tool, you MUST analyze the returned documentation sections (especially the use_cases field) and then use the get-documentation tool to fetch ALL documentation sections that are relevant for the user's task.

### 3. svelte-autofixer

Analyzes Svelte code and returns issues and suggestions.
You MUST use this tool whenever writing Svelte code before sending it to the user. Keep calling it until no issues or suggestions are returned.

### 4. playground-link

Generates a Svelte Playground link with the provided code.
After completing the code, ask the user if they want a playground link. Only call this tool after user confirmation and NEVER if code was written to files in their project.
