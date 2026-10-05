# Engine Plugins

**Date:** 2026-10-05
**Status:** Design. Nothing implemented yet.

## Problem

Core links four database drivers (sqlx's Postgres, MySQL and SQLite, and
tiberius) and runs DuckDB out of process in `seaquel-duckdb`, the DuckDB
helper. So there are two ways an engine runs, two sets of failure modes, and
an in-process driver bug can still take down the app, the TUI or the web
server for every user. The helper showed that the out-of-process path works:
framed protocol, credit windows, cancel ordering, lifecycle rules and a
verified download. This design puts every engine on that path.

## Goal

On every native interface (desktop, CLI, TUI, web server) every engine runs
as a plugin: an executable Core starts and talks to over stdin and stdout.
Core links no database driver. It keeps the dialects, which are pure, and
everything it does today around a connection. The demo is unchanged: DuckDB-WASM
in the page through the DuckDB crate's `browser` driver.

Out of scope: third-party plugins, a public or versioned protocol, Wasm
components, and plugins for anything but engines. The traits and the
protocol should stay clean enough that a later design could add them.

## Decisions

| # | Decision |
|---|---|
| 1 | All five engines are plugins on desktop, CLI, TUI and web. The demo keeps DuckDB-WASM in the page |
| 2 | Postgres, MySQL/MariaDB, MSSQL and SQLite run one plugin process per engine per Core, holding every connection of that engine. DuckDB keeps one process per connection (file exclusivity, crash isolation) |
| 3 | Desktop, CLI and TUI download each plugin on first use through the pinned, verified install the DuckDB helper has today. The web image ships its three plugins |
| 4 | A plugin is locked to the app's version: `hello` must match exactly, and the protocol stays private, free to change in any release |
| 5 | Each engine crate splits by feature into `dialect` (pure, in Core) and `driver` (in the plugin). Dialect calls never cross the pipe |
| 6 | One row format on the native wire for every engine: `Value` batches in a compact binary encoding. DuckDB's Arrow IPC leaves the native wire |
| 7 | "Helper" is renamed "plugin" everywhere |

## Names

| Today | After |
|---|---|
| `seaquel-duckdb` (crate and binary) | `seaquel-plugin-duckdb` |
| — | `seaquel-plugin-postgres`, `-mysql`, `-mssql`, `-sqlite` |
| Release asset `seaquel-duckdb-<triple>[.exe].gz` | `seaquel-plugin-<engine>-<triple>[.exe].gz` |
| `<data_local>/<identifier>/bin/duckdb/<version>/` | `<data_local>/<identifier>/plugins/<version>/`, every engine of one version together |
| `CoreBuilder::duckdb_helper(DuckdbHelper)` | `CoreBuilder::plugins(PluginLocator)` |
| `duckdb_helper_status`, `_asset`, `_install`, `_install_from_file` | `plugin_status(engine)`, `plugin_asset(engine)`, `plugin_install(engine, …)`, `plugin_install_from_file(engine, …)` |
| Core features `engine-duckdb-remote`, `duckdb-helper-install`, `duckdb-helper-testing` | `plugins`, `plugin-install`, `plugin-testing` |
| `SEAQUEL_DUCKDB_HELPER_SIZE`, `_SHA256`, `_REQUIRE_PIN` | `SEAQUEL_PLUGIN_PIN_<ENGINE>` (`<size>:<sha256>`), `SEAQUEL_PLUGIN_REQUIRE_PINS` |
| Tauri `duckdb_helper_offer`, `_install`, `_cancel`, `_install_file` | `plugin_offer`, `plugin_install`, `plugin_cancel`, `plugin_install_file`, each with `{engine}` |
| `seaquel-cli duckdb status\|install` | `seaquel-cli plugins status\|install [ENGINE…]`; `duckdb` stays a hidden alias for one release |
| `HELPER_PROTOCOL` | `PLUGIN_PROTOCOL` |
| `SEAQUEL_TEST_DUCKDB_HELPER` | `SEAQUEL_TEST_PLUGIN_DIR` |
| `tests/HELPER.md` | `tests/PLUGINS.md` |

`ENGINE_NOT_INSTALLED` and `ENGINE_UNAVAILABLE` keep their names and
meanings. The i18n keys `duckdb_install_*` and `duckdb_helper_*` become
`plugin_install_*` with the engine's display name as a parameter.

## What stays in Core

An engine crate does two jobs today, and only one of them is I/O:

- **The dialect** (`dialect.rs`): quoting, `paginate`, `count_query`, the
  CRUD, DDL and data-tab builders. `Workspace::run`, the edits service and
  `db.engine`'s DDL calls use it many times per call, and the demo's wasm32
  Core needs DuckDB's.
- **The driver** (`driver.rs`, `introspect.rs`, `decode.rs`, `bind.rs`, the
  sessions): connecting, running, decoding, introspection, EXPLAIN and
  cancel-on-drop.

Each engine crate gets two features, as DuckDB already has `remote`, `helper`
and `browser`. `dialect` is the default, pure and wasm-clean. `driver` brings
sqlx, tiberius or duckdb-rs, and only the plugin binaries turn it on. Core
depends on all five crates with `dialect` only.

Core registers one `PluginEngine` per engine id. Its dialect is the crate's,
its `open` starts or reuses the plugin process and sends `open`, its
`preflight` is the install check, and DuckDB's keeps `exclusive_file`.

Unchanged in Core: SSH tunnels (the plugin connects to the local port Core
forwarded), `ConnectPolicy` and the web's `check_connect_config` (run before
`open` is sent), `CONNECT_TIMEOUT`, workspace ownership and connection
replacement, `ConnectionLimits`, `RunLimits` and `EditLimits`,
`CONFIRM_REQUIRED`, history, the read-only token check and `lost.rs`. A
plugin knows nothing about workspaces, windows or users; Core assigns every
connection id.

Moved into the plugin, as code that exists today: the statement-logging rules
(`disable_statement_logging`, the log filter), `pg_cancel_backend` and
`KILL QUERY` on a dropped stream, MSSQL's session and `hold_state`, SQLite's
read-only gate and NUL refusal, and DuckDB's restricted mode.

Core still links sqlx's SQLite for `seaquel-storage`, so the rule is "no
database driver in Core", not "no sqlx in Core".

Rejected: dialect calls as RPCs. A run would cross the pipe for each
`paginate` and count, the demo would still need a local dialect, and
planning would stop being pure.

## The protocol

Three crates, extracted from `seaquel-engine-duckdb`'s `wire.rs`, `remote/`
and `helper.rs`:

| Crate | Class | Job |
|---|---|---|
| `seaquel-plugin-wire` | pure | Frames, messages, the row encoding |
| `seaquel-plugin-client` | infra, native | Spawning, the start check, multiplexing calls, `Driver` over the wire. Core's `plugins` feature |
| `seaquel-plugin` | plugin SDK | `serve(engine)` over any `Arc<dyn Engine>`: the reader, the dispatcher, the writer queue, credit, cancel, panics per call |

A plugin's `main` is a few lines:
`seaquel_plugin::serve(seaquel_engine_postgres::engine())`. DuckDB's plugin
serves a `Driver` implemented over `session.rs` with blocking threads, so
`helper.rs` becomes that driver and `serve` replaces its loop.

**Frames** keep today's header, `[len u32 LE][kind u8][call u32 LE][payload]`,
with `MAX_FRAME` (16 MiB) checked from the header before allocating and before
writing, on both sides. Control messages are JSON, internally tagged,
camelCase, `deny_unknown_fields`, with hand-written `Debug`; a parse error
names serde's category and position, never the text.

**Messages** follow `Driver` and `Engine` one for one, each carrying the
`connection` id Core picked: `hello`, `open` (`ConnectConfig` and
`OpenOptions`, the password inside as `open` receives it in process today;
never logged), `close`, `query`, `stream`, `execute`, `transaction`,
`readOnly`, `explainReadOnly`, `listSchemas`, `schemaTables`,
`tableMetadata`, `statistics`, `explain`, `cancel` and `credit`. Replies are
`helloOk`, `opened`, `executed`, `committed`, `done` and `error` (`DbError`,
plus `index` for a transaction). An engine that doesn't implement a call
answers `NOT_SUPPORTED`, as the trait's defaults do.

**Rows** are a `columns` frame, then `batch` frames of `Value` rows: a tag
byte per cell and its payload (lengths as LE integers, text as UTF-8, decimals
as text, JSON as text). Each batch frame is at most 8 MiB and is split by rows;
one row goes alone up to `MAX_FRAME`, past it `RESULT_TOO_LARGE`. `done` ends
the rows. The plugin decodes; Core never sees a driver type.
`query_stream` batches stay 5,000 rows on Core's side.

DuckDB's `decode.rs` runs in its plugin, and its column kinds (`kinds::of`)
stay there. The browser driver keeps reading Arrow IPC in the page with the
same `decode.rs`. Arrow isn't the shared format because SQLite types each
cell, not each column.

**Flow control** is today's. Every call that returns rows has a credit window
of two batch frames, then one per `credit`. Control replies never wait behind
rows; rows wait once 32 MiB is queued. A cancel is posted under the same lock
as later requests, so it reaches the plugin first. Core applies `RowCap` and
the byte budget to decoded rows. Since credit is per call, one stalled stream
(on web, one user's slow browser) can't hold up the process.

**Limits per connection.** DuckDB's 16 concurrent read-only calls become a
per-connection limit the plugin enforces (`TOO_MANY_REQUESTS` as a backstop;
the client queues past it). Pool sizes come from `OpenOptions::max_pool_size`
as today.

## Processes

**Start.** A shared plugin starts on the first `open` for its engine, and
opens that race wait on one start. Before each spawn the start check runs as
it does for the helper: no symlinks from `<identifier>` down, owned by the
user with no group or world write on Unix, `seaquel_runtime::acl` on Windows.
The child gets the cwd, the environment minus `SEAQUEL_*_TEST_*` (and the
AppImage entries), `CREATE_NO_WINDOW` on Windows, and the spawn mutex on Unix.
`hello` must get `helloOk` with the app's version within 5 s, with one 20 s
retry for a file this process hasn't started yet. Another version, a refusal
or EOF is `ENGINE_NOT_INSTALLED`; a silent plugin is `ENGINE_UNAVAILABLE`.

On web the plugin inherits the server's scrubbed environment (no `PG*` or
`MYSQL*`, `HOME=/nonexistent`), so no driver reads `~/.pgpass`.

**Stop.** A shared plugin with no connections exits after 5 minutes idle. A
DuckDB plugin ends with its connection, keeping `closing.rs`'s checkpoint,
detach and file-claim rules. Shutting down closes each plugin's stdin; on
desktop quit each plugin reads EOF and finishes on its own, as the helper does.

**A plugin that dies or breaks the protocol** fails every connection it held
with `CONNECTION_CLOSED` and its message. `Driver::closed()` fires for each, so
`lost.rs` takes each out and announces it once. The next `open` starts a fresh
process. After 3 crashes in 60 s, opens of that engine get
`ENGINE_UNAVAILABLE` for 30 s.

A panic inside one call is caught and fails that call only
(`catch_unwind` per call, as DuckDB's kinds already do). Only an abort, an
OOM kill or a protocol break ends the process. On web that still drops the
engine's connections for every user; that is the price of one process per
engine, accepted in decision 2.

**Reconnecting in the GUI.** A lost DuckDB connection is reconnected only on
the user's click, because the query that killed it may kill the next one.
A lost connection to a shared plugin goes through the quiet reconnect
(`handleConnectionLost`): the crash more likely came from one query on one
connection, and the others did nothing wrong. The failed call is never
retried.

**Wedge.** A plugin whose frames have waited 30 s with none written exits with
code 5, as now. Core's reader always drains into per-call channels, so this
fires only when Core itself stops reading (a TUI stopped with Ctrl+Z); each
connection then reconnects.

## Install and first use

Core's install is the helper's, keyed by engine: `seaquel-http`'s
`release_asset` with private folders, the `.part` file, digest, gzip trailer
and rename, the `.installed` record, the start check after install and the
prune to the two newest version folders (never the running one). One install
per engine at a time per Core, and a second request joins the first, as
`HelperInstalls` does on desktop. Pins are a table compiled in from
`SEAQUEL_PLUGIN_PIN_<ENGINE>`; `SEAQUEL_PLUGIN_REQUIRE_PINS=1` (release
builds) fails the build unless all five are pinned. The first install into
`plugins/` removes the old `bin/duckdb/` folder, best effort; an older app
still installed downloads its helper again.

A connect to an engine without a usable plugin fails at once with
`ENGINE_NOT_INSTALLED`, before anything spawns, so `CONNECT_TIMEOUT` never
covers a download. The interface installs, then connects again.

**Desktop.** The DuckDB dialog becomes the plugin dialog, naming the engine
and its size ("PostgreSQL support for Seaquel 2026.x.y is a separate download
of <size>"), with "Install from a file…" under the pin. Connects that meet it
at once still share one dialog per engine. The prefetch matters more now that
every update needs new plugins: ten seconds after startup the main window
installs, one at a time and without asking, the plugin of every engine the
saved connections use, under today's prefetch rules (main window, connections
loaded, pinned build, not when dismissed this page, not on a metered
connection). "Install Command Line Tool…" no longer installs DuckDB support;
the CLI fetches what it needs. Settings → MCP's DuckDB warning becomes a list
of the plugins the exposed connections need.

**CLI.** `plugins status [ENGINE…]` prints one line per engine
(`postgres installed <path>`, `mysql missing`, `outdated`, `unsafe`) and exits
0 only when every named engine is installed. `plugins install [ENGINE…]`
installs the named engines, all five without names, with `duckdb install`'s
progress lines, signals and `--from FILE --sha256 HEX` (one engine only). The
read commands in a terminal ask "Install PostgreSQL support (<size>)? [Y/n]"
and connect after; without one they fail naming
`seaquel-cli plugins install postgres`. `mcp` rewords the error as it does for
DuckDB today.

**TUI.** The DuckDB install dialog becomes the plugin dialog for any engine,
keeping the pending connect and its typed secrets.

**Web.** The image puts `seaquel-plugin-{postgres,mysql,mssql}` in
`/app/plugins/<version>/`, and `web_core()` points the locator there
(`SEAQUEL_PLUGIN_DIR` overrides it; it joins `rust-env.js`'s allow-list). The
server builds without `plugin-install`, so `release-asset` stays out of it. A
missing or refused plugin stops the server at startup rather than failing the
first connect.

## Testing

**Two paths for engine tests.** Each engine crate's suites (smoke, parity
fixtures, live tests) keep running in process with `driver`, which is fast and
where engine bugs show up. The testkit gains a `through_plugin` mode that runs
the same suites through the built plugin, and CI runs both. Core's tests run
through plugins only, the one path the product uses.

**Finding plugins.** `tests/common/engine.rs` generalizes:
`SEAQUEL_TEST_PLUGIN_DIR`, else the plugins built beside the test binary,
installed into a temp folder laid out as a real install. With neither, a test
panics naming `cargo build -p seaquel-plugin-<engine>`. Under
`SEAQUEL_TEST_REQUIRE_ENGINES=1` a missing plugin fails rather than skips.

**Protocol tests**, beside today's remote-driver tests: many connections in
one process; a stalled stream on one connection while another moves; a crash
dropping every connection and the next open starting a fresh process; the
idle exit; the crash-loop backoff; a panicking call leaving the process up;
frame limits both ways; cancel ordering across connections.

**Benchmark gate.** A 100,000-row SELECT and 1,000 small queries on Postgres,
through the plugin and in process. CI fails past a 1.5× slowdown, to be
adjusted once real numbers exist.

## CI and release

- The `duckdb-helper` job becomes `plugins` (macOS and Windows), building all
  five and running the protocol, install and ACL tests.
- "Native binaries' dependencies" fails if Core's tree has `sqlx-postgres`,
  `sqlx-mysql`, `tiberius` or any `duckdb` crate other than
  `seaquel-engine-duckdb`; if a plugin's tree has another engine's driver; or
  if the server's tree has `release-asset`.
- `check-crate-deps.mjs` replaces `ENGINE_HOSTS` with `PLUGINS`: one engine
  crate with `driver`, `seaquel-plugin`, pure crates, and no dependents.
- Each `publish-tauri` job builds, signs and (on macOS) notarizes five plugins,
  gzips and pins each, builds the app with all five pins, and uploads
  `seaquel-plugin-<engine>-<triple>[.exe].gz`. Only the DuckDB plugin gets
  `disable-library-validation`.
- `check-release` checks 5 plugins × 6 targets against their pin records.
- The Docker image builds the three server plugins in its Rust stage.
- The required checks in branch protection are renamed, and the release
  section of `CLAUDE.md` with them.

## Phases

Each phase ships on its own and leaves every interface working.

1. **Rename.** `seaquel-duckdb` to `seaquel-plugin-duckdb`, with the folder,
   asset names, Tauri commands, CLI subcommand (with its alias), codes, i18n
   keys and docs. No behaviour change beyond names and paths.
2. **Generic protocol.** Extract `seaquel-plugin-wire`,
   `seaquel-plugin-client` and `seaquel-plugin`; add connection ids and
   `Value` batches. DuckDB moves onto it, and Arrow leaves the native wire.
   Add the benchmark.
3. **Install by engine.** Pins, status, install, the dialog, the prefetch,
   the TUI dialog and `plugins`. Only DuckDB is a plugin yet, so this is
   checked end to end with one engine.
4. **SQLite plugin.** The first shared plugin: local, no network, so
   multiplexing, the idle exit and crash handling are tried on the simplest
   engine.
5. **Postgres, MySQL/MariaDB and MSSQL plugins** on desktop, CLI and TUI.
6. **Web.** The image ships its three plugins and `web_core()` uses them;
   the web's limits and connect checks are re-tested through the plugin path.
7. **No drivers in Core.** Split every engine crate into `dialect` and
   `driver`, drop the engine features from Core, and tighten the dependency
   checks.

## Risks

- **First use after each update.** Every update needs new plugins. The
  prefetch hides that when the machine is online at startup; otherwise the
  first connect after an update asks to download, and offline only "Install
  from a file…" helps.
- **Shared fate on web.** One crash drops that engine's connections for every
  user. Per-call panic catching and quiet reconnects shrink it; they don't
  remove it.
- **Overhead.** One more copy of every row and a pipe round trip per call.
  The benchmark gate catches regressions; if wide results miss it, the fix is
  in batching, not the design.
- **Release size and time.** 30 plugin assets per release, each signed and
  notarized on macOS, adds minutes to every macOS job.
- **Scanners.** Five new executables per version, downloaded at run time, so
  a first exec may be scanned. The 20 s retry covers macOS; Windows Defender
  may need the same.
- **No way back without work.** After phase 7 Core can't run an engine
  without its plugin. The `driver` feature stays, so tests still can.
