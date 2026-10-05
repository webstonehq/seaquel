# Rust Core and Plugin Architecture

**Date:** 2026-09-24
**Status:** Implemented through phase 8, phase 7a and the DuckDB helper.
Phase 7b (the CLI's commands) isn't planned yet. CLAUDE.md is the detailed
reference for how each part works today; this file keeps the shape of the
design and the decisions that still constrain new code.

## Phases

- **Phase 0, groundwork.** CI, `seaquel-db` split into `seaquel-engine` and
  one crate per engine, `seaquel-types` with TypeScript codegen,
  connect/disconnect in Core, `CancellationToken`, the `Executor` and the
  wasm-ready async-trait macro.
- **Phase 1, Postgres pilot.** The Postgres dialect, introspection and
  EXPLAIN moved to Rust, with the typed `Value` and `EngineClient` in the GUI.
- **Phase 2, the other engines.** MySQL/MariaDB, SQLite, MSSQL and DuckDB in
  Rust on desktop and web.
- **Phase 2b, SQL tooling.** `seaquel-sql` and `seaquel-wasm` replaced
  node-sql-parser and the TypeScript scanners.
- **AI safety.** Model-written SQL runs only through Core's read-only path.
- **Phase 3, infrastructure.** Storage, secrets, SSH, git and licensing in
  Rust behind Core and the workspace RPC; the legacy JSON import dropped;
  SQLite and DuckDB off on web.
- **Phase 4, MCP.** `seaquel-cli mcp`, a read-only MCP server over stdio.
- **Phase 5a, connections.** The GUIs connect, test, query and disconnect
  through the workspace (`db` group); web streams over `/rpc/stream`.
- **Phase 5b, runs.** The editor's runs, paging and history are `db.run` and
  `db.page`.
- **Phase 5c, edits.** Grid edits, pending changes and the data tab are
  planned and run by Core.
- **Phase 5d-1, the library.** Connections, projects, labels and saved
  queries are written through Core, with `StorageChanged` events.
- **Phase 5d-2, the rest of storage.** Dashboards, workflows, chats,
  settings and per-window view state.
- **Phase 5e, shared projects.** The `.seaquel` projection, sync and the
  TablePlus and DBeaver imports moved into Core (desktop only).
- **Phase 8, the demo.** Core runs in the page as a second WebAssembly
  module, `seaquel-browser`.
- **Phase 6, the assistant.** `seaquel-ai` and `seaquel-http`; Core runs
  every turn, tool call and model request.
- **Phase 7a, the TUI.** `seaquel-tui`, a second process that writes the
  app's storage, and `data_version` polling.
- **DuckDB helper.** The terminal binaries run DuckDB in `seaquel-duckdb`,
  downloaded on first use.
- **Desktop DuckDB helper.** The desktop app runs DuckDB in the same helper,
  and the in-process driver is gone.
- **Phase 7b, CLI commands.** Not planned yet (see "Open work").

## Problem

Seaquel had three front ends (desktop, web, demo), and most of what it knew
lived in TypeScript inside the Svelte app. A CLI, a TUI and an MCP server
couldn't reuse any of it. Engines were the worst case: a Postgres connection
was 30 lines of Rust that opened a pool and about 500 lines of TypeScript
that held the introspection SQL, EXPLAIN parsing, DDL, CRUD and quoting, so a
Rust CLI would have had only the driver half, and Postgres fixes would never
have reached it.

## Goal

Rust is the core language. Everything except interface code lives in Rust
crates: the Svelte GUI, the Tauri shell and the web auth layer are the only
exceptions. Each capability is a crate that can be built and tested on its
own, and Seaquel Core registers and orchestrates them. Every interface calls
Core, so a fix in the Postgres crate reaches desktop, web, the demo, the TUI
and the MCP server in the same release.

## Decisions

| # | Decision | As it stands |
|---|---|---|
| 1 | Core language | Rust. The CLI, TUI and MCP server are Rust too |
| 2 | Plugin mechanism | Compile-time crates behind traits, registered on `CoreBuilder` (`engine`, `with_plugins`, `duckdb_helper`). No runtime loading |
| 3 | Engine boundary | One crate per engine owns its driver, dialect and introspection. The GUI never builds engine-specific SQL |
| 4 | Pure vs I/O code | Pure crates (types, SQL tooling, dialects, the domain crates) build for `wasm32-unknown-unknown`, and so does Core with its `browser` feature. CI enforces both |
| 5 | GUI hot paths | Keystroke-rate work (statement at cursor, `{{param}}` handling, the builder's parse) calls `seaquel-wasm` synchronously. Everything else goes through the RPC |
| 6 | One API for GUIs | `seaquel-rpc` is served over Tauri IPC, HTTP plus one WebSocket per page, and the demo's in-page module. TypeScript types are generated from Rust |
| 7 | Multi-tenancy | One process-wide `Core`, a `Workspace` per user. Web keeps an LRU of per-user workspaces; desktop, TUI and MCP open one each |
| 8 | GUI state | Tabs, panes and layout belong to the interface. Core stores them as opaque per-window blobs and never parses them; the TUI keeps its own state file |
| 9 | Web auth | Better Auth, signup, team and account routes stay in SvelteKit/Node |
| 10 | Terminal binaries | Two: `seaquel-cli` (`mcp`, `duckdb`) and `seaquel-tui`, sharing `seaquel-terminal`. Neither can be named `seaquel`, which the app owns on every platform |
| 11 | Migration | Strangler pattern, one subsystem at a time, shipping at every step. Done |
| 12 | Demo | Core compiled to wasm32 in the page, with DuckDB-WASM through the DuckDB crate's `browser` driver and storage as in-memory SQLite saved to IndexedDB |
| 13 | Web licensing | `seaquel-license` behind `/internal/license/*`, reachable from loopback with a per-boot secret only |
| 14 | Terminal licensing | Honour system. The terminal binaries check no license and print the terms line |
| 15 | CLI distribution | Release assets. The app downloads the version-matched `seaquel-cli` and the DuckDB helper on request; the TUI is a manual download |
| 16 | Legacy JSON storage | Dropped. Core refuses pre-2026.4.5 JSON files and names the fix |

### Rules that keep holding

- **Policies have no defaults.** `ConnectPolicy`, the `Executor`,
  `LocalFiles`, `ai_http` and `ai_egress` must be passed to `CoreBuilder`.
  A Core built without one refuses the calls that need it, whatever Cargo
  features were unified into the build.
- **Features aren't a security boundary.** Cargo unifies features across a
  workspace build, so anything the web server must not do (file engines,
  SSH, git, licensing calls from `/rpc`, local files, the OS proxy) is also
  refused in code.
- **Storage schema changes are expand-only.** Older releases open files a
  newer build changed, so new columns are nullable, a new migration is a new
  file (sqlx checksums the old ones), rewrites that need Rust are data
  steps, and the frozen baseline is never edited.
- **One writer connection per storage.** Every write goes through
  `Storage::write` on a dedicated connection, so `PRAGMA data_version` on it
  moves only for other connections' commits, which is how a second process
  (the TUI) is noticed.
- **Every stored write emits `StorageChanged`**, ids and a change sequence,
  never a value. GUIs refetch and apply by sequence.
- **No secret leaves its owner.** Keychain entries are read by Core on
  desktop; on web the browser's vault sends a secret with the call that
  needs it, and the server holds it for that call only. Secrets never reach
  a log, an event, a stored row or an error.
- **Model-written SQL runs read-only**, through each engine's
  `query_read_only` after `seaquel-sql`'s token check.
- **Wasm32 constraints in Core.** No `tokio::spawn`, `Instant` or
  `SystemTime` (enforced by `crates/clippy.toml`); time and spawning go
  through the `Executor`; async traits use `seaquel_runtime::async_trait`,
  which drops `Send` on wasm32.

### Why compile-time plugins

"Plugin" means an independently developed and tested crate behind a trait,
not a library loaded at runtime. Rust has no stable ABI, so native dynamic
plugins break on every compiler upgrade, and the alternatives (WASM
components, subprocess plugins) cost real complexity while nobody outside
Webstone writes Seaquel plugins. If third-party plugins become a goal, that
is a separate design; the traits should stay clean enough for it.

DuckDB is the one engine that runs out of process on native platforms, and
that doesn't contradict the above. The helper is first-party, released with
the binaries that start it, and refused unless its version matches theirs,
so there is no protocol to promise anyone. It was chosen for binary size
(DuckDB was about two thirds of each terminal binary) and for safety: in
process, two connections could open one file and silently lose committed
writes, and an abort in DuckDB took the whole app down. It sits behind the
same `Engine` and `Driver` traits, and nothing in Core knows the driver is
remote. The cost was what subprocess plugins were expected to cost: a
framed protocol with credit windows and cancel ordering, process lifecycle
rules, and a verified download.

## Architecture

```
                 interfaces (thin)
 ┌──────────────┬───────────────┬────────────┬───────────┬────────────────┐
 │ src-tauri    │ seaquel-server│ seaquel-cli│seaquel-tui│ seaquel-browser│
 │ (desktop)    │ (web, axum)   │ (+ -mcp)   │ (ratatui) │ (demo, wasm32) │
 └──────┬───────┴───────┬───────┴─────┬──────┴─────┬─────┴────────┬───────┘
        │   seaquel-rpc │             └── seaquel-terminal        │
        └───────┬───────┘                    │          seaquel-rpc
                ▼                            ▼                    ▼
 ┌─────────────────────────────── seaquel-core ─────────────────────────────┐
 │ Core (engine registry, connections, tunnels, running streams, policies)  │
 │ Workspace (per user: storage, secrets, library, runs, edits, AI, events) │
 └─────────────────────────────────────┬────────────────────────────────────┘
          domain                        │                 infrastructure
 ┌──────────────────┬──────────┐        │   ┌────────┬────────┬─────┬─────┬────────┬──────┐
 │ seaquel-workspace│seaquel-ai│        │   │storage │secrets │ ssh │ git │license │ http │
 └──────────────────┴──────────┘        │   └────────┴────────┴─────┴─────┴────────┴──────┘
          engines (plugins)             ▼                  pure (wasm-clean)
 ┌──────────┬───────┬────────┬───────┬────────┐   ┌───────────────┬─────────────┬───────────────┐
 │ postgres │ mysql │ sqlite │ mssql │ duckdb │   │ seaquel-types │ seaquel-sql │ seaquel-runtime│
 └────┬─────┴───┬───┴────┬───┴───┬───┴────┬───┘   └───────────────┴─────────────┴───────────────┘
      └─────────┴─ seaquel-engine (traits) + seaquel-engine-testkit
                                          duckdb ──▶ seaquel-duckdb (helper process)
```

The Svelte GUI talks to `seaquel-rpc` through `CoreClient` (Tauri IPC on
desktop, HTTP and a WebSocket on web, the in-page module in the demo) and
loads `seaquel-wasm` for the synchronous editor checks.

### Crates

| Crate | Kind | Owns |
|---|---|---|
| `seaquel-types` | pure | Wire types, `Value`, the stored row types, `name_key` |
| `seaquel-runtime` | pure | `Executor`, `MaybeSend`, `BoxStream`, the async-trait macro, Windows ACL checks |
| `seaquel-sql` | pure | The per-engine scanner, splitting, statement at cursor, query type, the destructive and read-only checks, `{{param}}` substitution, the `CREATE TABLE` parser, sqlparser-rs helpers |
| `seaquel-engine` | pure + async traits | `Engine`, `Driver`, `Dialect`, the generic DDL, CRUD and table-select builders, `DbError` |
| `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}` | plugin | Driver, decoding, dialect, introspection, EXPLAIN. `mysql` also serves MariaDB; `duckdb` has the `remote`, `helper` and `browser` drivers |
| `seaquel-engine-testkit` | dev | The conformance suite each engine runs |
| `seaquel-workspace` | domain (pure) | The planners: connections, runs, edits, library, state, shared projects, imports, and the AI wire types |
| `seaquel-ai` | domain (pure) | Provider wire formats, the tool registry, prompts, limits, sharing |
| `seaquel-storage` | infra | The metadata SQLite file: baseline, migrations, data steps, one function per query, sqlx natively and `sqlite-wasm-rs` on wasm32 |
| `seaquel-secrets` | infra | `SecretStore`, the OS keychain, `SecretWait` |
| `seaquel-ssh` | infra | Tunnels over russh and the known_hosts check |
| `seaquel-git` | infra | libgit2 operations and the shared projection's file I/O |
| `seaquel-license` | infra | Desktop activation and the web server's license gate |
| `seaquel-http` | infra | The native HTTP client, the egress guard, verified release downloads |
| `seaquel-core` | orchestrator | `Core`, `Workspace` and every service above them |
| `seaquel-rpc` | interface glue | The workspace RPC and its dispatchers |
| `seaquel-wasm` | interface glue | wasm-bindgen exports of `seaquel-sql` for the editor |
| `seaquel-browser` | interface | Core in the demo's page |
| `seaquel-terminal`, `seaquel-mcp` | interface libraries | What the terminal binaries share; the MCP server |
| `src-tauri`, `seaquel-server`, `seaquel-cli`, `seaquel-tui`, `seaquel-duckdb` | binaries | The interfaces and the DuckDB helper |

### Dependency rules

`npm run crates:check` (`scripts/check-crate-deps.mjs`) checks these against
`cargo metadata`:

1. Engine crates depend only on `seaquel-engine`, the pure crates and their
   own drivers. No engine depends on another.
2. Domain and infrastructure crates never name an engine crate. They reach
   engines through Core's registry.
3. Interfaces depend on `seaquel-core` (and `seaquel-rpc` where they serve
   GUIs), and reach domain and infrastructure crates only through Core's
   re-exports.
4. Pure crates don't depend on tokio networking, sqlx, the file system or
   anything that fails a wasm32 build. `seaquel-ai` may use only pure crates
   and `seaquel-workspace`.
5. The DuckDB helper may use only its engine crate and the pure crates, and
   nothing depends on it.

CI also checks the web server's and the terminal binaries' dependency trees
for crates they must not link (DuckDB, git2, russh, keyring, OpenSSL and
others).

### Plugin kinds

Engines are the plugin kind: each implements `Engine` (open, an optional
`preflight` and `exclusive_file`, the dialect) and `Driver` (query, execute,
transaction, `query_stream`, the read-only calls, introspection with
`NOT_SUPPORTED` defaults, and `closed()` for drivers that can tell their
connection was lost). Each engine is a Cargo feature of Core, and
`with_plugins(|id| …)` registers a subset by id. The other seams are traits
an interface passes in rather than plugins: `SecretStore`, `Executor`, the
AI `HttpClient`, and DuckDB-WASM's bridge object in the demo.

## The engine plugin

The engine does its own I/O and returns typed results: callers ask for
tables, not for the query that lists tables. EXPLAIN parsing sits on the
driver, next to the query it runs. The generic builders in `seaquel-engine`
(`ddl.rs`, `crud.rs`, `select.rs`) take a dialect's quote function,
placeholder style and options, so each engine writes only what differs.

Values cross the wire in one format (`seaquel-types/src/value.rs`): plain
JSON for what JavaScript holds exactly, and `{"$sq": kind, "v": …}` for big
integers, non-finite floats, decimals, bytes and JSON. Parameters bind with
their real types, cancellation is a token passed into `query_stream`, and a
dropped stream stops on the server where the engine can do that.

## Core, workspaces and state

`Core` is process-wide: the engine registry, open connections, SSH tunnels,
running streams and the policies it was built with. A `Workspace` is one
user's data: its storage file, an optional secret store, and the services
that run on them (connect, runs, edits, the library, settings and view
state, shared projects, imports, the assistant). Connections and streams
belong to the workspace that opened them, and since phase 6 to the window
that opened them as well; Core refuses any other caller with the same answer
as an unknown id.

- **Desktop, TUI and MCP** open `<data_dir>/seaquel.db` with the OS
  keychain. The MCP server opens it read-only; the TUI opens it as a second
  writing process that does no schema work.
- **Web** opens `DATA_DIR/users/<id>/meta.db` per user, with no secret
  store, from an LRU of workspaces.
- **The demo** opens an in-memory file in the page and saves a snapshot of
  it to IndexedDB after each call that committed.

### Storage ownership

Core is the only thing that writes app storage. The webview never sends SQL
to its own metadata database; a new query is a new `seaquel-storage`
function and RPC method. Writes queue on one writer connection behind a
mutex and run in `BEGIN IMMEDIATE` transactions. Opens are serialised across
processes, and a pending migration runs once under a lock. A read-only or
second-process open refuses a file that still needs schema work
(`STORAGE_NEEDS_UPGRADE`) until the app has opened it once.

### Secrets

The OS keychain (desktop, CLI, TUI) keeps the key names older releases used
(`db:<id>`, `ssh:<id>`, `ssh-key:<id>`, `license-key`, `ai-api-key:<id>`), so
nothing had to be re-entered. On macOS each item trusts only the binary that
created it, so a terminal binary reading an app item, or the app reading one
the TUI saved, asks once ("Always Allow"); a keychain access group would
have orphaned every saved password. Web has no store: the vault decrypts in
the browser and sends the secret with the call that needs it. The demo keeps
an AI key in page memory for the session.

## The RPC surface

`seaquel-rpc` defines one adjacently tagged `Request`/`Response` per group
(`storage`, `library`, `settings`, `ui`, `shared`, `imports`, `db`, `ai`,
plus the desktop-only `secret`, `ssh`, `git` and `license`) and the streams
(`db.queryStream`, `db.run`, `db.page`, `db.tablePage`, `ai.chat`), which
yield `CoreEvent`s. Every variant exists in every build, so the generated
TypeScript doesn't depend on features. The web server serves `/rpc` and
`/rpc/stream` on loopback only, behind Node, which authenticates and sets
the trusted `X-Seaquel-User` header; that header is the server's only
identity, which is why binding to loopback is a security requirement.

## Web licensing

Node's request gate asks `seaquel-server` over loopback
(`/internal/license/*`), with a per-boot secret that `server.js` generates,
because a query running inside the Rust process could also connect from
loopback. The license tables stay in `auth.db`; Node still applies their
historical migrations and Rust owns every read and write. The control-plane
contract (`X-Install-Id`, `X-License-Key`), the soft and hard TTLs and the
Ed25519 air-gap bundle check live in `seaquel-license`.

## Terminal binaries and distribution

The app bundles no terminal binary. `release.yml` builds, signs and
notarizes `seaquel-cli`, `seaquel-tui` and the gzipped `seaquel-duckdb` per
target and uploads them to the release. The app's "Install Command Line
Tool…" downloads the CLI and the helper that match its version, checks size
and SHA-256, and links the CLI onto `PATH` (not on Windows). The terminal
binaries download the helper themselves on first use. The app's release
build carries the helper's size and SHA-256, so it needs no GitHub API call.

## Testing

Each layer has suites that don't need the layers above it: pure planner
tests against recorded fixtures (frozen when the TypeScript they came from
was deleted), engine conformance and smoke tests against the
`e2e/test-databases` containers, Core tests on SQLite, DuckDB and temp
storage with fake secret stores and mock providers, and interface tests for
the server's routes, the CLI's stdio and the TUI's rendered screens. CI runs
them with the wasm32 builds, the dependency rules, `svelte-check` and
vitest.

## Open work

- **Phase 7b, the CLI's commands:** `conn list|test|add|import`, `query`,
  `schema`, `saved`, `export`, `ask`. Connection create and edit in the TUI
  follow `conn add`. Keychain writes from these go through
  `connectionCreate`/`connectionUpdate`, as the TUI's password save does.
- **The app installing the TUI** as it installs the CLI.
- **MCP write and dashboard tools**, behind a per-connection opt-in stored
  in the workspace, never a model-controlled flag.
- **A read-only database login for AI queries**, the only full guard on SQL
  Server, where a rolled-back transaction still keeps what a query committed.
- **Supply chain:** a signed `SHA256SUMS` for the downloads, and pinned
  digests for the terminal binaries' helper download and the app's CLI
  download (moving `cli_download.rs` onto `seaquel-http`'s `release_asset`).
- **OS sandboxing of a restricted DuckDB helper** (landlock and seccomp,
  `sandbox_init`), possible now that DuckDB has its own process.
- **Assistant:** more providers, prompt caching, a per-connection schema
  cache with columns in the schema context, token usage per chat, a cap on
  the referenced-context section, and a graceful shutdown for
  `seaquel-server` so a SIGTERM doesn't lose replies in flight.
- **Cross-process events:** the MCP server re-reading its exposed
  connections on `external`, and a change journal if one coarse `external`
  event ever proves too expensive.
- **Smaller items:** a structured fingerprint on `UNKNOWN_HOST_KEY`,
  `connectionCreate` binding the open connection itself (so `db.bindSaved`
  goes), Windows `PATH` for the CLI, creating a new DuckDB file from the
  wizard, and stripping the app binary.

## Risks

- **DuckDB versions.** The demo's DuckDB-WASM 1.32 is DuckDB 1.4.3, behind
  the native 1.5, and the type differences its tests pin stay until
  DuckDB-WASM ships a newer build.
- **Demo storage.** One SQLite file in memory, saved whole to IndexedDB:
  a crash between a commit and its save loses that change, two tabs each
  start from the file as they found it and the last writer wins, and WebKit
  can drop a save still running when a tab closes. OPFS in a worker would
  fix the first two at the cost of moving Core off the main thread.
- **WASM size.** The demo's module is held to 2,000,000 bytes brotli by the
  build; the assistant took it to about 82% of that.
- **AI egress on web.** Behind an HTTP proxy, a name the server can't
  resolve goes to the proxy, which then decides what it may reach; an
  operator who needs private targets blocked must block them there.
- **Concurrent writers.** Another process's commit is seen within the poll
  interval as one coarse `external` event, and the GUI reloads every list on
  it. Another connection's `wal_checkpoint(TRUNCATE)` counts as one change
  too.
- **Keychain prompts on macOS** are one per item and binary, and have to be
  checked by hand on a signed build.
- **Build times and binary size.** DuckDB, vendored libgit2 and the
  release profile's fat LTO are slow to compile; CI depends on caching.
