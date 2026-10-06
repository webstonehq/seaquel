# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Seaquel is a database client built with Tauri 2 + SvelteKit 5 + TypeScript, with a Rust core. It supports PostgreSQL, MySQL/MariaDB, SQLite, MSSQL and DuckDB through the Rust engine crates in `crates/`. It ships as a desktop app, a self-hosted web app (`seaquel-server` behind a Node/SvelteKit server), a browser demo, and two terminal binaries: `seaquel-cli` (the MCP server) and `seaquel-tui` (a terminal client). No shipped binary but one links DuckDB: the desktop app and both terminal binaries run it in `seaquel-duckdb`, a helper process per DuckDB connection, downloaded once per version on first use into a folder they share (see `seaquel-duckdb` below). The demo runs DuckDB-WASM.

SQLite and DuckDB connections are desktop-only. On web their "connection string" is a path on the server, so the web server refuses both (see `seaquel-server` below), and the web wizard doesn't offer them. The demo has DuckDB-WASM in the page and nothing else, with Core compiled to wasm32 in the page beside it (see "The demo" below). SSH tunnels, shared projects (git) and the TablePlus and DBeaver imports are desktop-only too.

## Development Commands

```bash
# Start development (frontend + Tauri), as app.seaquel.desktop.dev
npm run tauri:dev

# Build production app
npm run tauri build

# Type checking
npm run check

# Type checking (watch mode)
npm run check:watch
```

Use `npm run tauri:dev`, not `npm run tauri dev`. The script passes `--config '{"identifier":"app.seaquel.desktop.dev"}'`, so the dev app keeps its data, its CLI and its DuckDB helper under `app.seaquel.desktop.dev`, where a debug `seaquel-cli` or `seaquel-tui` looks too. A plain `tauri dev` runs as the real `app.seaquel.desktop`: it reads and writes the installed app's `seaquel.db`, and its first DuckDB connect downloads the release helper of its version into the real `bin/duckdb/`. A debug app installs the `seaquel-duckdb` built beside it instead of downloading only when the identifier ends in `.dev` or `SEAQUEL_DATA_DIR` is set (`cli_download::may_use_debug_helper`).

The DuckDB helper's digest is built into release apps. `src-tauri/build.rs` reads `SEAQUEL_DUCKDB_HELPER_SIZE` (bytes, plain decimal, at most 64 MiB) and `SEAQUEL_DUCKDB_HELPER_SHA256` (64 hex digits, no `sha256:` prefix) through `src-tauri/src/helper_pin.rs`, which the app includes too, and compiles them in as one `SEAQUEL_DUCKDB_HELPER_PIN=<size>:<sha256>` read with `option_env!`; nothing reads them at run time. Neither set (every local build and test run) means no pin: the app asks GitHub's release API for the size and digest, as the terminal binaries do, and offers no "Install from a file…" and no prefetch. One without the other, or a value that doesn't parse, fails the build. `SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN=1` (`release.yml` sets it on every app build) fails the build without both; unset, empty or `0` is off, anything else is an error.

Every script that runs Vite, svelte-check or vitest first runs `npm run wasm:build` (`scripts/build-wasm.mjs`), which builds `crates/seaquel-wasm` into `src/lib/wasm/pkg/` (gitignored). It needs the wasm32 target and wasm-bindgen-cli at the exact version in `Cargo.lock`:

`mise install` sets up Node, Rust (with the wasm32 target) and wasm-bindgen-cli from `mise.toml`. mise exports `RUSTUP_TOOLCHAIN` for its Rust version, which overrides `rust-toolchain.toml` (and, in `release.yml`, the toolchain `dtolnay/rust-toolchain` installs), so the target must be listed in `mise.toml`, not only in `rust-toolchain.toml`. Without mise, `rust-toolchain.toml` adds it, or:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version "$(node scripts/build-wasm.mjs --bindgen-version)" --locked
```

When a dependency update bumps wasm-bindgen in `Cargo.lock`, update `"cargo:wasm-bindgen-cli"` in `mise.toml` to the same version.

- If that toolchain is missing the script fails, even when an older `pkg/` exists: a stale module would run old SQL checks, including the AI's read-only check. `SEAQUEL_WASM_PREBUILT=1` uses `pkg/` as it is without building (the Docker image does this); set it only when you know `pkg/` matches the crates.
- `dev` builds the module once at startup. After changing `crates/seaquel-sql` or `crates/seaquel-wasm`, rerun `npm run wasm:build`; Vite then reloads the new module.
- The demo's second module, `crates/seaquel-browser` (Core in the page), is built only where it's used: `predev:demo` and `prebuild:demo` run `npm run wasm:build:browser` (into `src/lib/wasm/browser-pkg/`), and `pretest`/`pretest:watch` run `wasm:build:browser-test` (the `test-hooks` variant vitest loads, into `browser-test-pkg/`). Desktop and web scripts never build it. Each module has its own stamp, and `SEAQUEL_WASM_PREBUILT=1` covers both.

On macOS, a wasm32 build or clippy of `seaquel-storage` or `seaquel-browser` (they compile SQLite's C through sqlite-wasm-rs) needs `CC_wasm32_unknown_unknown`/`AR_wasm32_unknown_unknown` pointing at an LLVM clang and llvm-ar, or Homebrew's llvm (`brew install llvm`) on its default path; Apple's clang has no wasm32 backend. So `npm test`, `dev:demo` and `build:demo` (and the website's `demo:update`) need `brew install llvm` once. `build-wasm.mjs --module browser` finds a clang itself (`CC_wasm32_unknown_unknown`, then Homebrew's llvm, then a `clang` on `PATH` whose `--print-targets` lists wasm32), prints the one it used and stops with the fix named when there is none; plain cargo doesn't look. CI sets `clang-18`/`llvm-ar-18`.

On Linux (and in CI) `seaquel-secrets` links the system libdbus for the Secret Service, so install `libdbus-1-dev` and `pkg-config`.

The terminal binaries, `seaquel-cli` and `seaquel-tui`, and the DuckDB helper, `seaquel-duckdb`, aren't in the app bundle (there is no `bundle.externalBin`). `release.yml` builds, signs and uploads each per target as a release asset (`seaquel-cli-<triple>`, `seaquel-tui-<triple>`, `.exe` on Windows, and `seaquel-duckdb-<triple>[.exe].gz`). Each `publish-tauri` matrix job builds the helper first, without `--gzip`, codesigns it on macOS with the hardened runtime and `src-tauri/macos/duckdb-helper.entitlements.plist` (its one entitlement, `disable-library-validation`, lets DuckDB load its extensions, which DuckDB signs; issue #114) or trusted-signs it on Windows, gzips it with `build-cli.mjs`'s `gzipFile`, so the `.gz` holds the signed file, pins the `.gz`'s size and SHA-256 into the app's build, and uploads only the `.gz` to the draft after the app (see "Releasing"). The app, the CLI and the TUI are signed with the hardened runtime and no entitlements: nothing they load needs one, and the entitlement would let any library load into them. The app downloads the helper on its first DuckDB connect (the dialog, below), and the version-matched `seaquel-cli` when the user asks (`cli_download.rs`, below); nothing installs `seaquel-tui` yet (the README gives a manual download), and the TUI and CLI fetch the helper themselves on first use. All three use one helper per version. `npm run cli:build` (`scripts/build-cli.mjs`) builds the CLI and copies it to `src-tauri/binaries/seaquel-cli-<target-triple>` (gitignored), `npm run tui:build` does the same for the TUI (`--bin seaquel-tui`) and `npm run duckdb-helper:build` for the helper (`--bin seaquel-duckdb`). The script takes `--release`, `--target`, `--bin seaquel-cli|seaquel-tui|seaquel-duckdb` and `--gzip` (also writes `<asset>.gz`), honours `CARGO_TARGET_DIR`, and a host build without `--target` shares `target/debug` with the app. `--release` builds with the root `Cargo.toml`'s `terminal-release` profile (fat LTO, one codegen unit, stripped; output in `target/[<triple>/]terminal-release/`); it keeps `panic = "unwind"`, which the TUI's panic hook and DuckDB's `catch_unwind` need. Without DuckDB (macOS arm64) the TUI is about 18.3 MB (8.6 MB gzipped) and the CLI 15.1 MB (7.2 MB), against 50 and 46 MB with it, and the helper is 34.7 MB (11.5 MB gzipped), downloaded once for all three. The app's release binary went from 101.9 MB (39.1 MB gzip -9) to 60.8 MB (27.0 MB) when DuckDB left it. Neither the app's build nor `cargo check -p seaquel` needs any of these files.

The web app in development is `npm run dev:web:full`: `scripts/with-internal-secret.mjs` generates a `SEAQUEL_INTERNAL_SECRET` and runs `rust:dev` (the Rust service) and `dev:web` (Vite) under it. If you run `rust:dev` and `dev:web` separately, export the same `SEAQUEL_INTERNAL_SECRET` in both shells, or every licensing call is refused and the app shows the 503 page. In production `server.js` generates the secret itself.

## Architecture

### Frontend (src/)

- **SvelteKit 5** with static adapter (SSR disabled for Tauri)
- **Svelte 5 runes** (`$state`, `$derived`, `$props`) for reactivity
- **Tailwind CSS v4** for styling
- **bits-ui** for accessible UI components (shadcn-svelte pattern)

### Backend (Rust)

All database logic lives in Rust crates under `crates/`, shared by every interface. See `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` for where this is heading.

Each crate, interface and GUI area has its own `CLAUDE.md` with the details (loaded when you work in that directory). Read the relevant one before changing code there, and keep details in it rather than here.

- `seaquel-core` — the only entry point interfaces use: engine registry, connections, streaming and cancellation, workspaces (storage + secrets), runs, edits, the library, settings and view state, shared projects, imports, the assistant, and the DuckDB helper's install. Interfaces never name the infrastructure crates; Core re-exports them.
- `seaquel-engine` — the `Driver`/`Engine` plugin traits, the pure `Dialect` trait and the generic DDL/CRUD/SELECT builders. One crate per engine: `seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb}` (MySQL and MariaDB share one).
- `seaquel-types` — wire types, `Value`, the metadata row types and `name_key`. `npm run types:gen` regenerates `src/lib/types/generated/`; never edit those by hand.
- `seaquel-rpc` — the workspace RPC (`CoreRequest`/`CoreResponse` in TS), its groups and dispatch.
- `seaquel-storage` — the metadata SQLite database (`seaquel.db` on desktop, `meta.db` per user on web): schema, migrations, data steps and one typed function per query.
- `seaquel-secrets` — the keychain (`SecretStore`).
- `seaquel-ssh` — SSH tunnels over russh.
- `seaquel-git` — shared projects' git over libgit2, and the projection's file I/O.
- `seaquel-license` — the desktop activation client and the web build's license gate.
- `seaquel-http` — native HTTP: the shared client, the AI egress guard, and release-asset downloads.
- `seaquel-ai` — the assistant's domain: provider wire formats, tool registry, prompt, limits.
- `seaquel-sql` — pure SQL text work (scanner, splitting, checks, params, AST helpers).
- `seaquel-wasm` — wasm-bindgen glue over `seaquel-sql` for the Svelte app.
- `seaquel-browser` — the demo's Core compiled to wasm32.
- `seaquel-runtime` — `Executor`, `MaybeSend`, the Windows ACL rule. Core crates must build for wasm32.
- `seaquel-workspace` — the pure domain crate (`seaquel_core::domain`): run, edit, library, state, shared, import, connection and AI planning and wire types.
- `seaquel-mcp` — the read-only MCP server, used only by `seaquel-cli`.
- `seaquel-duckdb` — the DuckDB helper binary every native interface runs DuckDB in.
- `seaquel-cli` — the `seaquel-cli` binary: `mcp`, `duckdb`, and the read commands.
- `seaquel-terminal` — what the CLI and TUI share.
- `seaquel-tui` — the `seaquel-tui` terminal client.
- Interfaces: `src-tauri/` (desktop), `crates/seaquel-server/` (web, Rust) with the Node side in `src/lib/server/`, and the demo in `src/lib/core/browser/`.

- `npm run crates:check` (`scripts/check-crate-deps.mjs`) enforces which crates may depend on which (`seaquel-ai` and `seaquel-http` are in `DOMAIN_AND_INFRA`, and `WASM_DOMAIN` keeps `seaquel-ai` to pure crates and `seaquel-workspace`). CI also fails if `seaquel-server`'s normal dependencies include the SQLite or DuckDB engines, duckdb, git2, russh, keyring, dbus, libssh2, OpenSSL, or the demo module's `sqlite-wasm-rs`, `arrow-ipc` or `wasm-bindgen` (the "Web server dependencies" step in `ci.yml`, matched by substring), so none of them reaches the Docker image. The "Native binaries' dependencies" step (phase 7a; `scripts/check-native-deps.sh`, which runs locally as written) reads each binary's normal dependencies for every platform (`cargo tree --target all`) and fails if `seaquel-tui`'s include rmcp, git2, libgit2-sys, plist, `seaquel-mcp`, `seaquel-rpc` or `seaquel-license`, if `seaquel-tui`'s, `seaquel-cli`'s or the desktop app's (`seaquel`) include a crate whose name contains `duckdb` other than `seaquel-engine-duckdb` (so `duckdb` and `libduckdb-sys`), or if `seaquel-engine-duckdb --no-default-features --features remote` lists `libduckdb-sys`; the CLI's old reqwest ban went with `duckdb install`. `ENGINE_HOSTS` (`seaquel-duckdb`) is the crate class for a binary that hosts one engine: its engine crate and the pure crates only, and no dependents. "Interfaces build on their own features" runs clippy on `seaquel-cli`, `seaquel-server`, `seaquel-mcp`, `seaquel-terminal` (also with `duckdb-helper-install`), `seaquel-tui` and `seaquel-duckdb` one package at a time, plus the engine crate with only `helper` and only `remote`, Core with only `engine-duckdb-remote` and only `duckdb-helper-install`, and `seaquel-http` with `release-asset`, so a feature only another interface turns on can't hide a missing one.

### Dialects and engine calls

- **UI code never scans or parses SQL itself.** Splitting, statement at cursor, `{{param}}` handling, query type, the destructive and read-only checks, `CREATE TABLE` parsing and the AST helpers all come from `$lib/sql` (`src/lib/sql`), which calls `seaquel-wasm`. Pass the connection's engine; the checks follow its quoting. If the module traps, `callWasm` re-instantiates it and the per-keystroke functions return their "couldn't parse" value, so on the run path (anything that decides what SQL executes) use the `…OrThrow` variants, which fail instead of letting SQL run unchecked. The editor's runs are planned by Core (`db.run`, below) on every interface, the demo included; the wasm calls remain for the editor's own checks (its synchronous destructive prompt, Explain and Visualize at the cursor).
- **UI code never does dialect work itself.** Introspection, EXPLAIN, statistics and DDL generation go through `EngineClient` (`src/lib/engine`): `getEngineClient(connection, state)`. Pagination, counts, CRUD and the data tab's query are Core's (`db.run`/`db.page`, the edits service); `EngineClient` has no `paginate` or `build*` since 5c. Pass the app state so the client reads the live provider connection id on every call (it survives `reconnect()`). Don't cache clients across operations. There is no TypeScript dialect left: `src/lib/db/` (`getAdapter`, `duckdb.ts`, `alter-table.ts`, `crud-helpers.ts`) went in phase 8.
- **Identifiers:** quote a name with `EngineClient.quoteIdent` and build a table name from a listed schema with `EngineClient.qualifiedTable` (DuckDB lists attached catalogs' schemas as `catalog.schema`, two names). Never hand-build `"${name}"`.
- **Model-written SQL runs only read-only.** The assistant's and MCP's tools run in Core (`ai::tools::call`: `query_stream` with `read_only`, `max_rows`, 8 MiB and 60 s; `explain_read_only`). Dashboard widgets, the assistant's dashboard tools' widgets and workflow query nodes run in the page through `executeReadOnly` (`QueryCrud`, then `DatabaseProvider.selectReadOnly` and Core's `query_stream` with `QueryOptions::read_only`, `max_rows` only). The inline prompt only inserts what it generates (`ai.generate`); the user runs it. Core runs the token check (`seaquel_sql::read_only`) and then `Driver::query_read_only`, whose default is `NOT_SUPPORTED`. The `read_only` flag limits what model-written SQL can do, and is no guard against the local user or an API caller. How each engine enforces it is in `crates/seaquel-engine/CLAUDE.md`.
- `getEngineClient` returns `RustEngineClient` (`db.engine` through `CoreClient`) for every engine: Postgres, MySQL, MariaDB, SQLite, MSSQL and DuckDB on desktop, Postgres, MySQL, MariaDB and MSSQL on web, and DuckDB in the demo (over the module).
- Engine-specific rules live in each engine crate's `CLAUDE.md` (`crates/seaquel-engine-{sqlite,mssql,mysql,duckdb}`).

### GUI areas

- `src/lib/core/CLAUDE.md` — `CoreClient` and its transports, how connections are made, lost and reconnected, and DuckDB support (the install dialog).
- `src/lib/hooks/database/CLAUDE.md` — running queries, edits and pending changes, the library and other windows' changes, settings and view state, the assistant, shared projects and imports.
- `src/lib/storage/CLAUDE.md` — metadata storage rules (`getStorage()`, load guards, the storage gate).
- `src/lib/core/browser/CLAUDE.md` — the demo.
- `src/lib/tutorial/CLAUDE.md` — the tutorial's DuckDB-WASM.

### Cell values

Rows and parameters cross the wire in one format (`crates/seaquel-types/src/value.rs`, `src/lib/values.ts`). Values JavaScript holds exactly are plain JSON. Everything else is tagged as `{"$sq": kind, "v": …}` with kind `bigint`, `float` (NaN/±inf), `decimal`, `bytes` (base64) or `json`. The providers decode tags before any UI code sees a row and encode parameters with `encodeParam`, so the UI gets `bigint`, `Uint8Array` and `SqlDecimal` alongside plain values. Use the helpers in `$lib/values` (`cellKey` for comparing cells, `cellText`, `toNumber`, `jsonReplacer` for `JSON.stringify`, `toStorable` for persisted rows) instead of `String()`/`Number()`/bare `JSON.stringify` on cells.

The workspace builds `serde_json` with `float_roundtrip`, so a double read from the wire has the bits it was written with (`floats_cross_the_wire_bit_for_bit` in `seaquel-types`); without it FLOAT's maximum came back one ULP high and no longer matched its own row. `src-tauri` takes `serde_json` from the workspace too.

The DuckDB drivers read Arrow themselves (`crates/seaquel-engine-duckdb/src/decode.rs`): the remote one from the helper's Arrow IPC frames with the column kinds the helper sends beside them (`kinds::of`, from DuckDB's logical types), the browser one from DuckDB-WASM's Arrow IPC bytes. Temporal types are Text in DuckDB's own format (TIMESTAMPTZ always in UTC, `2024-01-01 12:00:00+00`) and STRUCT/MAP are `Json` with sorted keys. The helper turns on `arrow_lossless_conversion` for its connections; the browser driver can't, and its differences are listed under DuckDB above.

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

- `src-tauri/tauri.conf.json` and `src-tauri/macos/duckdb-helper.entitlements.plist` - see `src-tauri/CLAUDE.md` (the CSP and entitlements have rules).
- `svelte.config.js` - SvelteKit config with static adapter
- `vite.config.js` - Vite bundler config

## The demo

The demo (`npm run dev:demo`, `npm run build:demo` into `build-demo/`, hosted at `seaquel.app/demo`) is a static build with no backend: Seaquel Core runs in the page as the `seaquel-browser` module, against DuckDB-WASM. How it starts, stores and recovers is in `src/lib/core/browser/CLAUDE.md`.

### Updating the demo

From the website repo (`seaquel-app/main`), run:

```bash
npm run demo:update
```

It removes `packages/marketing/static/demo`, runs `npm run build:demo` in this repo and copies `build-demo` there as `static/demo`. Commit and deploy the website changes afterward.

`build:demo` builds both modules (`prebuild:demo`), so the machine needs the wasm toolchain from "Development Commands" (Rust, the wasm32 target, the matching wasm-bindgen-cli) and an LLVM clang with the wasm32 backend (`brew install llvm` on a Mac). Without them the build fails and names the fix. `SEAQUEL_WASM_PREBUILT=1` reuses existing `pkg/` and `browser-pkg/`, which may be out of date. The build prints the browser module's size against its 2,000,000-byte brotli budget. DuckDB-WASM comes from jsDelivr at run time (Cloudflare's 25 MiB asset limit is below its 34 MB wasm), so the demo has no offline mode.

## Releasing a New Version

Version format: `YYYY.month.patch` (e.g., `2026.1.1`)

1. Update version in these files:
   - `src-tauri/Cargo.toml`
   - `src-tauri/tauri.conf.json`
   - `Cargo.lock` at the repo root (run `cargo check -p seaquel` after the `Cargo.toml` bump, or edit the `seaquel` entry). `src-tauri` is a workspace member, so the root lockfile is the only one.
   - `package.json`
   - `package-lock.json`

2. Commit: `Bump version to X.Y.Z`

3. Create and push tag:

   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

4. GitHub Actions builds macOS (Intel and ARM), Linux (x86_64 and ARM64) and Windows (x86_64 and ARM64), each on a runner of its own architecture. Each `publish-tauri` job, in order: builds and signs `seaquel-duckdb`, notarizes it on macOS, gzips it, pins it ("Pin the DuckDB helper for the app's build": `scripts/release-pin.mjs export` writes the `.gz`'s size and SHA-256 to `$GITHUB_ENV` and a record, `duckdb-pin-<triple>.json`), builds and signs the CLI and TUI and on macOS notarizes them, builds the app with `tauri-action` under `SEAQUEL_DUCKDB_HELPER_REQUIRE_PIN=1` (so an unpinned app fails its build), checks that the binary carries the pin ("Check the app carries the pin", `release-pin.mjs verify-app`, also the copy inside the macOS bundle), and uploads that target's helper `.gz` to the draft ("Upload the DuckDB helper to the draft", `actions/github-script` with `scripts/upload-helper-asset.mjs`, which re-hashes it against the record; no `gh` in the matrix jobs). On macOS the three `codesign` calls use `--options runtime --timestamp`, and `scripts/notarize-macos.sh` (zip, `xcrun notarytool submit --wait`, fail unless `Accepted`, print `notarytool log` otherwise; the `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` secrets tauri-action notarizes the app with) notarizes the helper before its gzip and pin and the CLI and TUI together before their upload, so a copy downloaded in a browser passes Gatekeeper. They are notarized, not stapled (a bare Mach-O can't be): Gatekeeper fetches the ticket online. Notarizing doesn't change a file's bytes. `tauri-action` creates the draft as a **pre-release** titled "NOT CHECKED: Release <tag>". `publish-cli` adds `seaquel-cli-*` and `seaquel-tui-*`. Every app and terminal binary downloads its helper from the release named `v<its version>`, so a release missing a target's `.gz` leaves DuckDB unusable there; the app's first DuckDB connect fails with `ASSET_NOT_FOUND`.

5. **`check-release`** (`needs` every other job, `if: always()`; `scripts/check-release.mjs`) fails unless every job before it succeeded and, for each of the six targets, there is a pin record, the app is pinned, the draft has its helper with the record's size and SHA-256 and its CLI and TUI, and `latest.json` has the tag's version, every target and only assets that are in the draft with a signature. On a pass it renames the draft "Release <tag>"; on a failure "NOT READY (check-release failed): Release <tag>". Only a draft or pre-release is renamed, and a release that isn't a pre-release fails it, so a re-run after promotion fails and leaves the title alone. A re-run of a failed matrix job rebuilds its helper (a new digest), its app with the new pin and replaces the uploaded `.gz`; then re-run `check-release`.

6. **Publish only a draft titled "Release <tag>"** with `check-release` green. The website's stable update check (`seaquel.app/updates/check/…`, in `seaquel-app`) offers the newest published release that isn't a pre-release and has no pre-release suffix, by version, cached for an hour, and checks nothing else, so promotion is the step that reaches stable users. First publish it as it is, a public pre-release the stable check skips (the beta check offers it to Beta users at once, see "Beta releases"), and run the owner's release checks: the jobs' logs, the digests by hand against each job's "Pin" line, `latest.json`, `codesign -d --entitlements -` showing none on the app, CLI and TUI and `disable-library-validation` on the gunzipped helper, notarization (the app's, and `spctl --assess --type execute -vv` printing "source=Notarized Developer ID" on quarantined browser downloads of the CLI, TUI and gunzipped helper), then installing it over the previous release on macOS, Windows and Linux. Then untick "pre-release" and tick "latest".

7. CI's required checks in branch protection are named "DuckDB through the helper (macos-latest)", "DuckDB through the helper (windows-latest)" and "Desktop app (tests, clippy)" (formerly "DuckDB on both drivers (…)" and "Desktop app compiles"); a rule still naming the old ones leaves PRs waiting on checks that never report.

### Beta releases

The app has a Beta update channel (`updateChannel`, Settings → General → Updates). Deploy the website's beta route before shipping the first app build that has the channel: until then the beta feed answers 404, so Beta users (and any pre-release build with no channel set) get no updates, silently at startup. Tag a beta `vX.Y.Z-beta.N` and bump the same files as above; the workflow, `check-release` and the helper pin are unchanged. Leave a beta a pre-release: never untick "pre-release" or tick "latest". The beta feed (`seaquel.app/updates/check/beta/…`, in `seaquel-app`) serves the newest published release by version, pre-releases included; the stable feed only promoted ones. So step 6's "publish it as it is, a pre-release" now reaches Beta users at once, before stable users. Semver puts `2026.10.0-beta.3` below `2026.10.0`, so beta users move onto the final release by themselves. A pre-release build with no channel set follows Beta. The app never downgrades: a beta user who switches to Stable stays on their build until a newer stable release is out.

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
