# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Seaquel is a database client built with Tauri 2 + SvelteKit 5 + TypeScript, with a Rust core. It supports PostgreSQL, MySQL/MariaDB, SQLite, MSSQL and DuckDB through the Rust engine crates in `crates/`. It ships as a desktop app, a self-hosted web app (`seaquel-server` behind a Node/SvelteKit server) and a browser demo.

SQLite and DuckDB connections are desktop-only. On web their "connection string" is a path on the server, so the web server refuses both (see `seaquel-server` below), and the web wizard doesn't offer them. The demo has DuckDB-WASM in the page and nothing else. SSH tunnels and shared projects (git) are desktop-only too.

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

Every script that runs Vite, svelte-check or vitest first runs `npm run wasm:build` (`scripts/build-wasm.mjs`), which builds `crates/seaquel-wasm` into `src/lib/wasm/pkg/` (gitignored). It needs the wasm32 target and wasm-bindgen-cli at the exact version in `Cargo.lock`:

`mise install` sets up Node, Rust and wasm-bindgen-cli from `mise.toml`, and `rust-toolchain.toml` adds the wasm32 target. Without mise:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version "$(node scripts/build-wasm.mjs --bindgen-version)" --locked
```

When a dependency update bumps wasm-bindgen in `Cargo.lock`, update `"cargo:wasm-bindgen-cli"` in `mise.toml` to the same version.

- If that toolchain is missing the script fails, even when an older `pkg/` exists: a stale module would run old SQL checks, including the AI's read-only check. `SEAQUEL_WASM_PREBUILT=1` uses `pkg/` as it is without building (the Docker image does this); set it only when you know `pkg/` matches the crates.
- `dev` builds the module once at startup. After changing `crates/seaquel-sql` or `crates/seaquel-wasm`, rerun `npm run wasm:build`; Vite then reloads the new module.

On Linux (and in CI) `seaquel-secrets` links the system libdbus for the Secret Service, so install `libdbus-1-dev` and `pkg-config`.

The desktop app bundles the `seaquel-cli` binary as a Tauri sidecar (`bundle.externalBin`). `npm run cli:build` (`scripts/build-cli.mjs`) builds it and copies it to `src-tauri/binaries/seaquel-cli-<target-triple>` (gitignored); it takes `--release` and `--target`, honours `CARGO_TARGET_DIR`, and a host build without `--target` shares `target/debug` with the app. tauri-build refuses to build the app while that file is missing, so **`cargo check -p seaquel` needs it too**: run `npm run cli:build` once after cloning (`src-tauri/build.rs` stops with that hint; CI touches an empty placeholder). `npm run tauri` goes through `scripts/tauri.mjs`, which for `dev` builds the sidecar before Tauri starts (otherwise `tauri dev`'s ~180 s wait for Vite can run out on a cold build) and then sets `SEAQUEL_CLI_PREBUILT=1`, which makes `beforeDevCommand`/`beforeBuildCommand`'s `build-cli.mjs` only check that the file exists. `release.yml` builds it per target in its own step the same way.

The web app in development is `npm run dev:web:full`: `scripts/with-internal-secret.mjs` generates a `SEAQUEL_INTERNAL_SECRET` and runs `rust:dev` (the Rust service) and `dev:web` (Vite) under it. If you run `rust:dev` and `dev:web` separately, export the same `SEAQUEL_INTERNAL_SECRET` in both shells, or every licensing call is refused and the app shows the 503 page. In production `server.js` generates the secret itself.

## Architecture

### Frontend (src/)

- **SvelteKit 5** with static adapter (SSR disabled for Tauri)
- **Svelte 5 runes** (`$state`, `$derived`, `$props`) for reactivity
- **Tailwind CSS v4** for styling
- **bits-ui** for accessible UI components (shadcn-svelte pattern)

### Backend (Rust)

All database logic lives in Rust crates under `crates/`, shared by every interface. See `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` for where this is heading.

- `seaquel-core` — the only entry point interfaces use: engine registry, open connections, streaming and cancellation. `disconnect` cancels the connection's running streams, which end with a `CONNECTION_CLOSED` error event. `Core::open_workspace(WorkspaceSpec)` opens a `Workspace` (`workspace.rs`): one data dir's metadata `Storage` plus an optional `SecretStore`, behind Core's `storage`/`secrets` features. Core owns SSH tunnels (`ssh_open`/`ssh_close`; dropping Core closes them) and re-exports the other infrastructure crates, so interfaces never name them directly.
  - **Features.** The engines (`engine-postgres`, `engine-mysql`, `engine-sqlite`, `engine-mssql`, `engine-duckdb`, all default) plus `storage`, `secrets`, `ssh`, `git`, `license-desktop`, `license-server` and `workspace`, off by default. `src-tauri` enables `storage`, `secrets`, `ssh`, `git` and `license-desktop`; `seaquel-server` enables `storage`, `license-server` and the Postgres, MySQL and MSSQL engines; `seaquel-cli` and `seaquel-mcp` enable `storage`, `secrets`, `ssh` and `workspace`, and no `git` or `license-*` (the CLI checks no license). `browser` is the wasm32 build for phase 8 and enables nothing; combining it with an engine or infrastructure feature is a `compile_error!`.
  - **Re-exports**, since interfaces may not name these crates: `storage`, `secrets` (behind their features), `sql` (`seaquel-sql`, always) and `domain` (`seaquel-workspace`, behind `workspace`; not `workspace`, which is Core's own `Workspace` module).
  - **`Workspace::connect_saved(core, id, options)`** (storage, secrets, ssh and workspace together) connects a saved connection the way the desktop app does: loads the row, reads its secrets from the workspace's store, opens its SSH tunnel and calls `Core::connect`, returning Core's connection id. `options` is a `HostKeyPolicy` or a `ConnectSavedOptions`: `HostKeyPolicy::KnownOnly` accepts only a host already in known_hosts and never writes it (`UNKNOWN_HOST_KEY` says to connect once in the app), `Trust(fingerprint)` is for the GUI later; `ConnectSavedOptions::new(policy).restricted(true)` sets `ConnectConfig::restricted`, which opens DuckDB locked down (no file access beyond its database, no extension installs or loads, configuration locked; other engines ignore it). The tunnel belongs to the connection: `disconnect`, a failed connect, dropping Core or dropping the future closes it. A store read that fails gives `SECRET_UNREADABLE` before anything opens, and connect errors have every saved secret replaced by `<redacted>`. Only the MCP server uses it; the GUI keeps its TypeScript connect path until phase 5, so the two are kept in step by the frozen fixtures in `crates/seaquel-workspace/tests/fixtures/connect-config`.
  - **`QueryOptions`** for `query_stream`: `read_only` (above), and, only with it, `max_rows` (return that many rows and mark the final `StreamBatch` `truncated` instead of failing with `RESULT_TOO_LARGE`), `max_bytes` (stop, `truncated`, once the kept rows' decoded cells reach it; one cell is never split) and `timeout` (enforced by the database, ending with `TIMEOUT`; a backstop for the caller dropping the stream, which stays the cancel). Any of the three without `read_only` ends the stream with `INVALID_OPTIONS`. Of the three, only `max_rows` crosses the Tauri and WebSocket transports (the in-app AI's `run_query` passes 1,000, `RUN_QUERY_MAX_ROWS`, and its tool result tells the model when rows were cut).
  - **`Core::explain_read_only(connection_id, sql, params, timeout)`** is the MCP server's EXPLAIN: the read-only token check, then exactly one statement (split on `;`; SQL Server counts statements from the ShowPlan XML), then `Driver::explain_read_only`. Refusals are `READ_ONLY`. The editor's `explain` is unchanged and isn't safe for model-written SQL: planning runs user code on Postgres and MariaDB, and SQLite and DuckDB run every statement.
  - **Features aren't a security boundary.** Cargo unifies features across a build, so `cargo test --workspace` compiles every engine and every infrastructure crate into `seaquel-server` too. Anything the web build must not do is refused in code as well: `with_plugins(|id| …)` registers only the engines it allows, and `dispatch_workspace` refuses SSH, git and license calls.
- `seaquel-engine` — the `Driver`/`Engine` plugin traits, the pure `Dialect` trait, and the generic DDL/CRUD builders (`ddl.rs`, `crud.rs`) that dialects parameterize. `Driver` has default `NOT_SUPPORTED` introspection methods (`list_schemas`, `schema_tables`, `table_metadata`, `statistics`, `explain`). One crate per engine: `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}`. Read-only queries go through `query_read_only_with(sql, params, ReadOnlyOptions { max_rows, max_bytes, timeout })`, which returns a `CappedResult { columns, rows, truncated }`; `RowCap` (`fail`, `truncate`, `read_only(max_rows, max_bytes)`) decides per row whether to keep it, truncate or fail with `RESULT_TOO_LARGE`, and `max_rows` never goes past `max_query_rows()` (100,000, `SEAQUEL_MAX_QUERY_ROWS`). `explain_read_only` defaults to `NOT_SUPPORTED`, so an engine without it fails closed.
- `seaquel-types` — wire types, including the dialect types (`SchemaTable`, `ExplainResult`, `CreateTableDefinition`, …), `Value` and the metadata row types (`storage.rs`, the `Persisted*` types). `npm run types:gen` regenerates `src/lib/types/generated/` from `seaquel-types`, `seaquel-rpc`, `seaquel-sql` and `seaquel-wasm`; never edit those by hand.
- `seaquel-rpc` — `EngineCall`/`EngineRequest`/`EngineResponse` and `dispatch` onto Core, for dialect and introspection calls on one connection. Served as the `db_engine` Tauri command and `POST /api/db/engine`. `workspace.rs` holds the workspace RPC: `Request`/`Response` (`CoreRequest`/`CoreResponse` in TS) with the `storage`, `secret`, `ssh`, `git` and `license` groups, and `dispatch_workspace`. Only the desktop serves the last four, each through its own function that needs no storage: `dispatch_secret` (the keychain), `dispatch_ssh` (Core's tunnels), `dispatch_git` and `dispatch_license` (the activation client). `dispatch_workspace` answers `NOT_SUPPORTED` for SSH, git and license whatever the features, so `/rpc` on web can never open a tunnel, touch a repo or call the license server. Every variant exists in every build, so the generated TS doesn't depend on features. Both levels are adjacently tagged (`{"method", "params"}`, responses `{"method", "result"}`), and `method` must come before `params`: stored JSON columns are `RawValue` and stay byte-identical, so parse bodies with `parse_request` from the raw bytes, never through a `serde_json::Value`. A method with no params leaves `params` out (`{}` is refused). Errors are `RpcError { code, message }`. It logs the method name only, never params.
- `seaquel-storage` — the metadata SQLite database: open, schema and one typed function per query (`queries/`); the only place SQL for it lives. `data_dir(identifier)` reads `SEAQUEL_DATA_DIR` (an empty value is ignored), else the platform data dir plus the Tauri identifier. The file is `<data_dir>/seaquel.db` on desktop and `DATA_DIR/users/<id>/meta.db` on web. `Storage::open` runs the frozen baseline (`schema.rs`, which upgrades every released file), then numbered expand-only SQL migrations (`migrations/`, rules in its README), then Rust data steps (`data_steps.rs`, recorded in `_seaquel_data_steps`). Schema changes are new migration files, never baseline edits. A pending migration runs under a `BEGIN IMMEDIATE` lock held on the migrator's own connection (each migration is a savepoint inside it), so two pools or processes opening one file apply it once. `StorageOptions { read_only: true }` (for `seaquel-cli`) opens with `SQLITE_OPEN_READONLY`, never sets the journal mode or creates the file, and runs nothing: if the baseline (`schema::is_current`), a migration or a data step has work to do it fails with `STORAGE_NEEDS_UPGRADE`, a missing file with `STORAGE_NOT_FOUND`. Fixtures in `crates/seaquel-storage/tests/fixtures` (each release's schema, the repo cases) are frozen. Codes that block the app: `LEGACY_STORAGE` (only pre-2026.4.5 JSON files, no `seaquel.db`), `STORAGE_CORRUPT`, `NO_DATA_DIR`; anything else is `STORAGE_ERROR`.
- `seaquel-secrets` — `SecretStore` (`get`/`set`/`delete`), `KeychainStore` and `MemoryStore` for tests. `validate_key` allows only `db:<id>`, `ssh:<id>`, `ssh-key:<id>`, `ai-api-key:<id>` and `license-key`. The keychain service is `DESKTOP_SERVICE` (`app.seaquel.desktop`) in dev builds too, since changing it orphans saved passwords. Errors name the key, never the value. The web workspace has no store, so secret calls there are `NOT_SUPPORTED`.
- `seaquel-ssh` — SSH tunnels over russh 0.48: a local port forwarded through a bastion, password or key-file auth, and the host-key check against known_hosts. An unknown key fails with `UNKNOWN_HOST_KEY` and its `SHA256:…` fingerprint; the retry passes that fingerprint as `trustHostKey`, and only a key with exactly that fingerprint is recorded. `HOST_KEY_MISMATCH` is never accepted. A key of an algorithm known_hosts doesn't hold for the host counts as unknown, not a mismatch (russh compares only keys of the same algorithm). Closing a tunnel (or dropping it) aborts every forward and ends the SSH session. The error codes are the ones the TS host-key prompt matches on; keep them. Live tests need `SEAQUEL_TEST_SSH` (below).
- `seaquel-git` — shared projects' git over libgit2 (vendored): `Git`, with every call async over `spawn_blocking`. The home dir for default SSH keys is a field (`Git::from_env()` on desktop), so tests never read `~/.ssh`. The credential chain is agent, then the given key or the default keys, then user/password; it never hands out the same credential twice and gives up after 4, so a wrong password can't loop. A commit after a conflicted pull gets MERGE_HEAD as its second parent, and commit refuses while files are conflicted. A push the remote refuses fails with `PUSH_ERROR`; a non-fast-forward says `PUSH_REJECTED_NON_FAST_FORWARD`, which the TS matches exactly to mark the repo "behind".
- `seaquel-license` — two parts that share only the HTTP client (`http.rs`: rustls with the OS and webpki roots, built lazily so a broken OS certificate store can't panic at startup). `desktop` is the activation client (`DesktopClient`: activate, validate, deactivate); the 12-hour revalidation stays in `license.svelte.ts`. `server` is the web build's gate (`LicenseServer`): the install id, the control-plane client, the soft/hard TTL ladder (`SEAQUEL_LICENSE_SOFT_TTL`/`SEAQUEL_LICENSE_GRACE_TTL`, 24 h and 14 d), member licenses and air-gap bundles (Ed25519; canonical JSON byte-identical to what seaquel-app signs). It reads and writes the license tables in `auth.db` with sqlx, but Node keeps applying migrations 006–012 through `auth.ts`: until they have run, every call answers `NOT_READY` (503). The control-plane client adds the PEMs in `NODE_EXTRA_CA_CERTS` as roots and honours `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY`. License keys never reach a log line, an error or `Debug`.
- `seaquel-sql` — pure SQL text work, no I/O: one hand scanner that follows each engine's quoting (`scan.rs`), with statement splitting, statement at cursor and the row-limit check on top; `statements.rs` (query type, the destructive-statement check, the source table for inline editing); `read_only.rs` (the AI's read-only check); `params.rs` (`{{param}}` substitution); `create_table.rs` (the table editor's SQL pane); and `ast/`, sqlparser-rs used only where an AST is needed (query builder and tutorial `ParsedQuery`, the Visual tab, column sources). It works in UTF-8 byte offsets. Parity fixtures in `crates/seaquel-sql/tests/fixtures` are frozen (see its README). Nothing in it may panic on user input: in the browser a panic is a trap.
- `seaquel-wasm` — wasm-bindgen glue over `seaquel-sql`, loaded by the Svelte app on desktop, web and the demo. Strings and JSON in and out, and every position that crosses is a UTF-16 offset (`offsets.rs`). The root `src/routes/+layout.ts` awaits `initSeaquelWasm` before anything renders, so calls are synchronous. Linked with a 2 MB stack (`build.rs`).
- `seaquel-runtime` — `MaybeSend`, `BoxStream`, `Executor`, `#[seaquel_runtime::async_trait]`. Core crates must build for wasm32: no `tokio::spawn`, `Instant` or `SystemTime` (enforced by `crates/clippy.toml`).
- `seaquel-workspace` — the domain crate, reached as `seaquel_core::domain`; it may not name an engine crate. Today only `connections` (and `connection_string`): a port of how `connection-manager.svelte.ts`, `connection-string.ts` and `wire.ts` turn a saved row plus keychain secrets into a `ConnectConfig` (`read_secrets` → `tunnel_config` → `build_config`), with the TypeScript's quirks kept. The 107 cases in `tests/fixtures/connect-config` were recorded from the TS and are frozen (README; the recorder is `docs/plans/artifacts/2026-09-30-freeze-connect-config.mjs.txt`). A change to how the GUI connects must change both, and the fixture only when the behaviour is meant to change.
- `seaquel-mcp` — the MCP server (rmcp 3.4.1), an interface library that only `seaquel-cli` may depend on (`INTERFACE_LIBS` in `check-crate-deps.mjs`). Eight tools, all read-only: `list_connections`, `list_schemas`, `list_tables`, `describe_table`, `run_query`, `explain_query`, `list_saved_queries`, `run_saved_query`. Errors are tool results with `isError: true` and `CODE: message`, never protocol errors. Its rules:
  - **stdout carries JSON-RPC only.** Nothing may print to it; logs go to stderr. It has its own line transport (`transport.rs`) so a line that isn't JSON gets `-32700` with `"id": null` instead of rmcp's silence.
  - **Only the connections named with `--connection`/`--project`** (id, else exact case-sensitive name) are exposed, resolved once at startup (`exposed.rs`); with neither, none. No tool takes a connection string, host, path or driver. A connection's AI schema and data sharing flags (after the global default in `app_state`'s `aiSettings`: schema on, data off) gate the tools, re-read on every call.
  - **No writes**, to databases or to Seaquel's own files: queries go through `query_stream` with `read_only`, EXPLAIN through `explain_read_only`, storage is opened read-only, and known_hosts is never written. A write opt-in is a follow-up with its own review.
  - **DuckDB opens restricted** (`ConnectSavedOptions::restricted`). The workspace `duckdb` dependency links `json` statically so the JSON functions work there; `icu` can't be linked from crates.io, so time zones and TIMESTAMPTZ arithmetic fail on the MCP server (pinned in `seaquel-engine-duckdb/tests/restricted.rs`).
  - **Limits:** `max_rows` default 100, at most 1,000 (`tools/query.rs`); an 8 MB fetch budget (`MAX_FETCH_BYTES`, Core's `max_bytes`); a cell's text cut at 64 KB (`format::MAX_CELL_BYTES`, as a `{"truncated": true, "bytes", "text"}` object) and a result stopped before its JSON passes 4 MB (`MAX_RESULT_BYTES`); a 60 s timeout per call (`DEFAULT_CALL_TIMEOUT`, not counting a pending keychain prompt, `secret_wait.rs`). Past it the call's stream is dropped, which cancels it, and Core's `timeout` also stops the statement on the server (Postgres `pg_cancel_backend`, MySQL/MariaDB `KILL QUERY`, from a fresh connection).
  - Cells render like `$lib/values` `cellText` (`format.rs`): bigint and decimal as strings, bytes as `\x` hex, JSON through a `JSON.stringify` port; `null` stays `null`.
- `seaquel-cli` — the `seaquel-cli` binary (clap), with one subcommand, `mcp [--connection …] [--project …] [--log-level …]`. It opens `seaquel.db` read-only with `KeychainStore` (service `app.seaquel.desktop` in every build) and `~/.ssh/known_hosts`; the data dir is `data_dir("app.seaquel.desktop")`, `.dev` in a debug build, or `SEAQUEL_DATA_DIR`. `--version` prints the desktop app's version (`build.rs` reads `src-tauri/Cargo.toml`) and the terms line; it checks no license. The binary can't be named `seaquel`: the app owns that name on every platform. Debug builds only (`cfg!(debug_assertions)`) read the test hooks `SEAQUEL_CLI_TEST_SECRETS` (a JSON file loaded into a `MemoryStore`), `SEAQUEL_CLI_TEST_KNOWN_HOSTS` and `SEAQUEL_CLI_TEST_CALL_TIMEOUT_MS`; a release build ignores them. SIGTERM and SIGINT close connections and tunnels like stdin closing does. `tests/stdio.rs` runs the built binary and checks that stdout carries only JSON-RPC.
- Interfaces:
  - `src-tauri/` (desktop). Its commands are `core_call`; `cli_info` and `install_cli` (`cli_info.rs`, for the MCP settings panel); the `db_*` set in `src/db/commands.rs` (`db_connect`, `db_query`, `db_query_stream`, `db_cancel_stream`, `db_execute`, `db_transaction`, `db_disconnect`, `db_engine`, `db_test`), which forward to Core until phase 5; and interface code (`copy_image_to_clipboard`, `open_path`, `get_data_dir`, `read_log_file`, `clear_log_file`, `get_username`, `install_update`, `check_for_update_command`, `read_dbeaver_config`, `read_tableplus_config`). There are no `git_*`, `ssh_*` or license commands: `core_call` serves storage, secrets, SSH, git and licensing. It takes the request as the raw bytes of a `Uint8Array` (`invoke("core_call", bytes)`); an object is refused. Storage opens lazily on the first storage call (`DesktopWorkspace`): `LEGACY_STORAGE`/`STORAGE_CORRUPT`/`NO_DATA_DIR` are kept and returned to every storage call, other failures retry on the next call, and secret, SSH, git and license calls work either way.
    - **The command line tool.** On macOS, and on Linux only when run as an AppImage (`cli_install::available`), the app menu has "Install Command Line Tool…" (`cli_install.rs`): macOS symlinks `/usr/local/bin/seaquel-cli` to `Contents/MacOS/seaquel-cli`, asking for admin rights through `osascript` only when needed and building that command from fixed paths only; the AppImage copies the sidecar into the data dir and links `~/.local/bin/seaquel-cli` to the copy. deb and rpm already install `/usr/bin/seaquel-cli`; Windows gets no item and no `PATH` change. Settings → MCP (`src/lib/components/settings/mcp/`, desktop only) shows the binary and whether it's on `PATH` (`cli_info`), the same install (`install_cli`), connection and project checkboxes, and the Claude Desktop JSON and `claude mcp add` line (`mcp-snippets.ts`, POSIX or PowerShell quoting, connections named by id).
  - `crates/seaquel-server/` (web; axum). `POST /rpc` takes the user from `X-Seaquel-User` and serves that user's workspace from an LRU (`workspaces.rs`, 1,024 open, 2 connections each). It trusts that header, so **binding to loopback only is a security requirement** (`main.rs`): it refuses a non-loopback `BIND_ADDR` unless `SEAQUEL_ALLOW_NON_LOOPBACK=1`, which is never for deployments.
    - **`/internal/license/*`** (`routes/internal_license.rs`) is licensing for Node's hooks and routes, through `src/lib/server/license-client.ts`. It refuses (403) a peer that isn't loopback and any request without the per-boot secret in `X-Seaquel-Internal`, compared in constant time. Loopback alone isn't enough, since a query running inside the process (DuckDB's httpfs, in a build that has it) also arrives from loopback. `server.js` generates `SEAQUEL_INTERNAL_SECRET` and hands it to both processes; Rust removes it from its own environment at startup. Neither `server.js` nor any SvelteKit route forwards `/internal/*`, so it also needs Node and Rust on one host: a non-loopback `SEAQUEL_RUST_URL` fails closed. When the license service can't be reached, Node answers 503 (`api-gate.ts`: `{code: "license_service_unavailable"}` on `/api/*`, an operator page otherwise); `/health` skips the gate.
    - **Engines.** `web_core()` builds Core with `with_plugins(|id| WEB_ENGINES.contains(&id))`, `WEB_ENGINES = ["postgres", "mysql", "mssql"]`, so SQLite and DuckDB fail with `ENGINE_NOT_AVAILABLE` (400) whatever Cargo compiled in (`tests/web_engines.rs`). `web_config.rs` refuses, on connect and test, connection options that name a file or socket on the server: Postgres and MySQL TLS certificate and key paths, `passfile`, MySQL `socket`, socket hosts and URLs without a host (`CONNECTION_OPTION_NOT_ALLOWED`, 400). So client certificates and socket connections are desktop-only.
    - **Environment.** `server.js` starts Rust with an allow-list (`shared/rust-env.js`: the variables the Rust crates read, the proxy and CA variables, `PATH`/`TZ`/`LANG` and temp dirs) and `HOME=/nonexistent`, because sqlx takes defaults from `PG*` variables and `~/.pgpass`. The binary also scrubs `PG*`/`MYSQL*` and sets that `HOME` itself at startup (`startup.rs`), which covers `npm run rust:dev`. A new env var the Rust service reads goes into the allow-list; its vitest scans the crates for them.
  - Node: `/api/rpc` requires a session, sets `X-Seaquel-User` from `locals.user.id` (dropping any the browser sent) and forwards the body bytes untouched. `/api/db/[...path]` forwards only an exact path allow-list (`FORWARDED_PATHS`); add a path there deliberately, never by pattern. It also refuses a `connect`/`test` whose driver isn't in `WEB_DRIVERS`, with the same body Rust sends. `hooks.server.ts` caches each user's gate answer for 5 s.
- **The web tutorial runs DuckDB-WASM in the page**, never on the server: the web build serves its own copy of the bundles (`duckdb-local-bundles.ts`, about 75 MB of assets, so an air-gapped install has a tutorial), the demo loads them from jsDelivr, and desktop uses SQLite through Tauri. The branch in `duckdb-bundles.ts` is on `import.meta.env.VITE_BUILD_TARGET` itself so Rollup drops the assets from the desktop and demo builds; keep it that way.
- `npm run crates:check` (`scripts/check-crate-deps.mjs`) enforces which crates may depend on which. CI also fails if `seaquel-server`'s normal dependencies include the SQLite or DuckDB engines, duckdb, git2, russh, keyring, dbus, libssh2 or OpenSSL (the "Web server dependencies" step in `ci.yml`, matched by substring), so none of them reaches the Docker image.
- Engine smoke tests: `cargo test -p seaquel-engine-<name> --test smoke`. Server engines need `SEAQUEL_TEST_<ENGINE>` set to ConnectConfig JSON (see each crate's `tests/smoke.rs`) and the containers from `e2e/test-databases/docker-compose.yml`, seeded with `npm run e2e:db:seed` (which creates `seaquel_test`). `SEAQUEL_TEST_REQUIRE_ENGINES=1` turns a missing variable into a failure instead of a skip.
- SSH tests (`crates/seaquel-ssh/tests/tunnel.rs`, `crates/seaquel-core/tests/ssh.rs`) use the compose file's `ssh` service (OpenSSH on `127.0.0.1:2222`, user `seaquel`, the fixture keys in `e2e/test-databases/ssh/`) and `SEAQUEL_TEST_SSH='{"host":"127.0.0.1","port":2222,"remote_host":"postgres","remote_port":5432}'`. The image keeps its config in an anonymous volume, so recreate it with `-V` after changing `ssh/init`.
- Desktop plugins still used: `tauri-plugin-updater`, `tauri-plugin-log` and others in `src-tauri/Cargo.toml`. Storage and the keychain go through `core_call`, not plugins.

### Dialects and engine calls

- **UI code never scans or parses SQL itself.** Splitting, statement at cursor, `{{param}}` handling, query type, the destructive and read-only checks, `CREATE TABLE` parsing and the AST helpers all come from `$lib/sql` (`src/lib/sql`), which calls `seaquel-wasm`. Pass the connection's engine; the checks follow its quoting. If the module traps, `callWasm` re-instantiates it and the per-keystroke functions return their "couldn't parse" value, so on the run path (anything that decides what SQL executes) use the `…OrThrow` variants, which fail instead of letting SQL run unchecked.
- **UI code never does dialect work itself.** Introspection, EXPLAIN, statistics, pagination, CRUD and DDL generation all go through `EngineClient` (`src/lib/engine`): `getEngineClient(connection, state)`. Pass the app state so the client reads the live provider connection id on every call (it survives `reconnect()`). Don't cache clients across operations, and don't call `getAdapter(` outside `src/lib/engine/` and `src/lib/db/`.
- **Identifiers:** quote a name with `EngineClient.quoteIdent` and build a table name from a listed schema with `EngineClient.qualifiedTable` (DuckDB lists attached catalogs' schemas as `catalog.schema`, two names). Never hand-build `"${name}"`.
- **AI and dashboard SQL runs only through `executeReadOnly`** (`QueryCrud`, then `DatabaseProvider.selectReadOnly` and Core's `query_stream` with `QueryOptions::read_only`), never `executeRaw`. Core runs the token check (`seaquel_sql::read_only`) and then `Driver::query_read_only`, whose default is `NOT_SUPPORTED`. The client sets the `read_only` flag: it limits what the AI's SQL can do, and is no guard against the local user or an API caller. Per engine:
  - Postgres: `BEGIN READ ONLY` on a pooled connection, which is closed afterwards, not returned.
  - MySQL/MariaDB: `SET SESSION TRANSACTION READ ONLY`, then `START TRANSACTION READ ONLY`, on a pooled connection that is then closed.
  - SQLite: a private read-only connection with `PRAGMA query_only = ON`, an authorizer that refuses most PRAGMAs and `ATTACH` (`read_only::deny_settings`), and a gate that prepares the SQL with SQLite's parser and allows one statement for which `sqlite3_stmt_readonly` is true (`read_only::check`). `SqliteDriver` refuses SQL holding a NUL byte on every entry point: sqlx 0.8.6 loops forever on it.
  - DuckDB: a clone of the connection per call, `BEGIN TRANSACTION READ ONLY` and `SELECT * FROM query(?) LIMIT <cap + 1>` with the SQL bound. The token check also refuses DuckDB functions that act outside the query (`BLOCKED_FUNCTIONS_DUCKDB`: `enable_logging`, `checkpoint`, `query`, `mysql_execute`, …). For the in-app AI, files and URLs stay readable: its instance is the editor's, whose file functions need external access. Only the MCP server's own instances are restricted.
  - A lone `\r` ends a `--` comment in the scanner on SQL Server, Postgres and DuckDB, as those databases do. The read-only check (`read_only_error`) also reads every input a second time with a lone `\r` ending `--` and `#` comments, on every engine, and refuses if either reading does; so on MySQL, MariaDB and SQLite, code-looking text after a `\r` inside a comment is refused.
  - MSSQL has no read-only mode. Each call gets its own connection (`READ_ONLY_CONNECTIONS`, 4 at once; `READ_ONLY_TIMEOUT`, 60 s), never the held session. The SQL runs two transactions deep inside TRY/CATCH and is always rolled back; a query that ends the transaction comes back as `READ_ONLY`, but what it committed stays, and `NEXT VALUE FOR` advances. Only a read-only login closes that.
- `getEngineClient` returns `RustEngineClient` (the `db_engine` endpoint) for Postgres, MySQL, MariaDB, SQLite, MSSQL and DuckDB on desktop (Postgres, MySQL, MariaDB and MSSQL on web), and `TsEngineClient` (the TypeScript `DatabaseAdapter` plus a provider) for the browser demo, which has no Rust core. `src/lib/db/duckdb.ts` is the only TypeScript adapter left, and it is demo-only (as are `alter-table.ts` and `crud-helpers.ts`; `index.ts` only re-exports the `SqlWithBindings` type from the latter); `getAdapter` throws for every other engine.
- **Every engine crate** has `dialect.rs` (the pure `Dialect`), `introspect.rs` (catalog SQL, parsers, EXPLAIN) and `decode.rs` (with `bind.rs` in all but DuckDB) for values. Postgres adds `numeric.rs`, the NUMERIC binary codec. `crates/seaquel-engine-mysql` serves MySQL and MariaDB: a `"mariadb"` connection connects with driver `"mysql"`.
- **SQLite:** cells decode by storage class (`typeof`), not declared type; BLOBs are bytes. SQLite has no `DEFAULT` in `UPDATE`, so Set to default sends the column's default expression from its metadata (`buildSetDefault`'s `columnDefault`). Edits SQLite can't make come back from `alterTable` as `-- …` note lines; the table editor shows them (`splitDdlScript` in `src/lib/utils/ddl-script.ts`).
- **MSSQL:** cells are native: decimal and money are `Decimal`, binary is `Bytes`, dates and times are SQL Server's own text (datetimeoffset as `2024-01-02 03:04:05.5 +01:00`). tiberius hands money over as an f64: it is exact only for |value| < 2^39 ≈ 5.5·10¹¹; above 2^53 units tiberius itself loses bits (up to ~1536 units near the ends of the range), and the maximum reads `922337203685477.5808`, which is out of range when bound back. A `Null` parameter is sent as the `NULL` literal (`inline_nulls`), since no declared type assigns to every column; so a column made only from it is int (`SELECT @P1 INTO`, `UNION`), `COALESCE`/`CASE`/`IIF` with only such NULLs fail (4127, 8133), and so does passing one as an `OUTPUT` argument (179). A `@Pn =` naming a parameter in an `EXEC` argument list is left alone. Query parameters (`{{name}}`) are inlined, strings as `N'…'`, so they work in `TOP`, `CREATE VIEW` and defaults. CRUD binds `@P1…`; EXPLAIN runs `SET SHOWPLAN_XML`/`STATISTICS XML` as separate batches and parses the ShowPlan XML with roxmltree; statistics are `NOT_SUPPORTED`.
- **DuckDB** uses the Rust crate on desktop (web has no DuckDB connections); the demo keeps `duckdb.ts`, whose behaviour shouldn't change. Attached catalogs are listed as `catalog.schema`, with a part holding `.` or `"` double-quoted (`"fx.we""ird".main`, or `"a.b"` for a default-catalog schema named `a.b`); `system` and `temp` are left out. Build `"schema"."table"` from a listed schema with `Dialect::quote_schema` in Rust (the CRUD/DDL builders' `_qs`/`_with` variants take it) and `EngineClient.qualifiedTable` in TypeScript, never by quoting the schema as one name. CRUD binds `?`. Edits DuckDB rejects (constraints in `ADD COLUMN`, column changes while the table keeps an index, dropping or retyping a PRIMARY KEY/UNIQUE column or a column before one) come back from `alterTable` as notes; the rules read `isUnique` and `inUniqueConstraint` (any UNIQUE constraint, composite too), which DuckDB's `table_metadata` reports on `SchemaColumn` and the table editor copies. `{{param}}` values are inlined (desktop, web and demo), skipping comments and quoted names.
- **The MSSQL driver runs on one connection**, except `query_read_only`, which opens its own per call (above). A call that doesn't finish (a UI cancel, a dropped request, `RESULT_TOO_LARGE`, a fatal error) closes it, and the next call reconnects. That loses `##global` temp tables, session context, app locks and any transaction opened by hand, which is silently rolled back. Code that changes session state (`SET SHOWPLAN_XML ON`, …) must wrap it in `Session::hold_state()`/`release_state()`; a failure before the matching OFF reconnects, with the same losses.
- **MSSQL statements run through `sp_executesql`**, with or without parameters, so a user's `SET` options and `#temp` tables end with the call and can't leak into introspection or grid edits. `USE` is the exception (it outlives an RPC call), so after a statement mentioning `USE` the driver switches back to the connection's database (`restore_database` in `driver.rs`). Only a parameterless statement that must start a batch (`CREATE VIEW`/`PROCEDURE`/`SCHEMA`, …; `must_start_batch`) and the driver's own BEGIN/COMMIT/ROLLBACK and EXPLAIN `SET … ON/OFF` go as plain batches. A plain EXPLAIN with parameters declares them without values, so its estimates are the generic ones.
- **MySQL/MariaDB TIMESTAMP values are shown as UTC wall-clock time**: sqlx sets the session `time_zone` to `'+00:00'`. Don't add a `timezone` to connection strings; in a zone with DST two instants print the same and a TIMESTAMP key would match the wrong row.
- The parity fixtures in `crates/seaquel-engine-*/tests/fixtures` were recorded from the TS adapters and are frozen; the recorder is gone (a reference copy is in `docs/plans/artifacts/`). Change a fixture only when the Rust behaviour is meant to change, and say why.

### Cell values

Rows and parameters cross the wire in one format (`crates/seaquel-types/src/value.rs`, `src/lib/values.ts`). Values JavaScript holds exactly are plain JSON. Everything else is tagged as `{"$sq": kind, "v": …}` with kind `bigint`, `float` (NaN/±inf), `decimal`, `bytes` (base64) or `json`. The providers decode tags before any UI code sees a row and encode parameters with `encodeParam`, so the UI gets `bigint`, `Uint8Array` and `SqlDecimal` alongside plain values. Use the helpers in `$lib/values` (`cellKey` for comparing cells, `cellText`, `toNumber`, `jsonReplacer` for `JSON.stringify`, `toStorable` for persisted rows) instead of `String()`/`Number()`/bare `JSON.stringify` on cells.

The DuckDB driver reads Arrow chunks itself (`crates/seaquel-engine-duckdb/src/decode.rs`). Temporal types are Text in DuckDB's own format (TIMESTAMPTZ always in UTC, `2024-01-01 12:00:00+00`), STRUCT/MAP are `Json` with sorted keys, and the driver turns on `arrow_lossless_conversion` for its connection.

### Metadata storage

- **Read and write metadata only through `getStorage()`** (`$lib/storage`), which returns a `StorageClient`: `RustStorageClient` (`core_call` on desktop, `/api/rpc` on web) or, in the demo only, the sql.js repositories (`sqljs-client.ts`). UI code never sends SQL to the metadata database; a new query is a new `seaquel-storage` function and `StorageRequest` variant. `RustStorageClient` puts every write through one queue so writes land in the order issued (`PersistenceManager`'s debounced saves rely on it); reads skip it. Every method is classified in `STORAGE_METHOD_KIND`.
- **Never save a collection whose load failed** (`load-guard.ts`). A store that replaces a whole collection or record on save marks a failed load, and its saves refuse until a load succeeds: interactive saves throw `NotLoadedError` (show it with `errorToast`), background saves call `skipUnloadedSave` and return. `PersistenceManager` tracks this per key. New replace-all saves need the same guard.
- **The storage gate** (`storage-gate.svelte.ts`): `storageGate.check()` is the first storage call, awaited by `UseDatabase.initializeApp` and the `(app)` layout. `LEGACY_STORAGE`, `STORAGE_CORRUPT` and `NO_DATA_DIR` set `storageGate.blocked`, and `(app)/+layout.svelte` shows `storage-error-screen.svelte` instead of the app. Other errors are retried twice, then logged, and the app starts.

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

- `src-tauri/tauri.conf.json` - Tauri app configuration. Its CSP has `'wasm-unsafe-eval'` in `script-src` because WebKit and Chromium won't compile `seaquel-wasm` without it. The token allows compiling WebAssembly and nothing else (`eval()` and `new Function` stay blocked); don't swap it for `'unsafe-eval'`. The web build sends no CSP.
- `svelte.config.js` - SvelteKit config with static adapter
- `vite.config.js` - Vite bundler config

## Updating the Demo

The demo is a browser-based version using DuckDB WASM (instead of PostgreSQL) hosted at `seaquel.app/demo`.

From the website repo (`seaquel-app/main`), run:

```bash
npm run demo:update
```

This script removes old demo files, builds the demo with `BUILD_TARGET=demo`, and copies the output to `static/demo/`. Commit and deploy the website changes afterward.

The build runs `npm run wasm:build` in this repo, so the machine needs the wasm toolchain from "Development Commands" (Rust, the wasm32 target and the matching wasm-bindgen-cli). Without it the build fails; `SEAQUEL_WASM_PREBUILT=1` reuses an existing `src/lib/wasm/pkg/`, which may be out of date.

## Releasing a New Version

Version format: `YYYY.month.patch` (e.g., `2026.1.1`)

1. Update version in these files:
   - `src-tauri/Cargo.toml`
   - `src-tauri/tauri.conf.json`
   - `Cargo.lock` at the repo root (run `cargo check -p seaquel` after the `Cargo.toml` bump, which needs the sidecar from `npm run cli:build`, or edit the `seaquel` entry). `src-tauri` is a workspace member, so the root lockfile is the only one.
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
