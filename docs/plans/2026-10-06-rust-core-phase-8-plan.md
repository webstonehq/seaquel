# Phase 8 Implementation Plan: the demo on Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** executed 2026-10-02 (Tasks 1–8, both checkpoints); owner's manual checks pending. Planned with the owner's answers to Q1–Q9 (2026-10-02). Surveyed on 2026-10-01 at HEAD `8d73887` (Phase 5e), with 5e's doc edits uncommitted (the design doc and the 5e plan). Line numbers are as of that tree. Spikes ran the same day (see "Spikes"); their code lives only in the session scratchpad and isn't kept.

**Goal:** The demo at `seaquel.app/demo` runs Seaquel Core in the page. The Rust Core, compiled to `wasm32-unknown-unknown`, plans and runs queries, applies edits, and stores the library, settings and view state, as it already does on desktop and web. It runs against DuckDB-WASM through a JavaScript bridge, and keeps its metadata in a SQLite file that the browser stores. The demo's TypeScript twins are deleted: `TsLibrary`, `TsState`, `TsSettings`, `TsUi`, `TsQueryRunner`, `TsEditService`, `TsEngineClient`, the sql.js repositories, `duckdb.ts`, `alter-table.ts` and `crud-helpers.ts`. `npm run demo:update` in the website repo builds the demo as it does today, with the new module included.

**After this phase, nothing the demo decides is decided in TypeScript either.** Every engine and domain fix reaches desktop, web and the demo in the same release.

**Architecture:**
- **`seaquel-storage`** gets one set of queries and two executors. A small `db` facade, shaped like the part of sqlx the crate uses, sits under every query function. On native targets it is sqlx, unchanged. On wasm32 it is SQLite compiled to wasm (`sqlite-wasm-rs`), called through a thin safe wrapper, with the whole file in memory. The page keeps a snapshot of that file in IndexedDB.
- **`seaquel-core`'s `browser` feature** admits `storage` and `workspace`. It still refuses engines, secrets, SSH, git, licensing and imports.
- **`seaquel-engine-duckdb`** splits into a `native` driver (duckdb-rs, the default) and a `browser` driver. Both share `dialect.rs`, `introspect.rs` and the Arrow decoder. The browser driver calls DuckDB-WASM through a bridge object that the page passes in. Result batches cross as Arrow IPC bytes and are decoded in Rust by the same `decode.rs`.
- **`seaquel-browser`** (new interface crate) builds Core with that storage and engine, and exports `open`, `call`, `stream`, `events` and `snapshot` through wasm-bindgen. It is a second module next to `seaquel-wasm`.
- **TypeScript:** an in-page `CoreClient` transport (`BrowserCoreClient`, plus `browserCoreTransport` for `RustStorageClient`). It loads the module, keeps the IndexedDB snapshot, deletes the old `localStorage` file and recovers from a trap. Every demo seam switches to its Core implementation, and the twins are deleted. `DuckDBProvider` stays only for the tutorial (Q5).

**Tech stack:** Rust (`seaquel-storage`, `seaquel-core`, `seaquel-engine-duckdb`, `seaquel-rpc`, the new `seaquel-browser`), wasm-bindgen 0.2.128, `sqlite-wasm-rs` 0.5, `arrow-ipc` 58 (the version duckdb-rs already uses), TypeScript/Svelte 5, vitest (Node) with the real module and DuckDB-WASM's Node build, and Playwright for the probe.

**Inputs:**
- The design doc: Decision 12, "The demo: Core in the browser", "Constraints this puts on Core from phase 0", phase 8, the "As built" notes in phase 3 (no `StorageBackend` trait yet; phase 8's spike decides), "Risks" (the demo gap, WASM size and startup) and "Open questions" (the browser storage backend). Also every phase cost section, for the estimate.
- The 5b–5e plans' "for phase 8" notes: the demo's gaps (5c), the trailing `--` in `duckdb.ts`'s paginate (5b), the twins' sizes (5d), and "5e adds none".
- The 5d and 5e effort logs. Review fixes ran at 64–72% of first passes, probe fixes at 1.5–2 times their budget, and from 5e's Task 4 on about half of each row was builds.
- A read-only survey of Core's `browser` build, `seaquel-storage`, the DuckDB engine, the demo's seams and twins, DuckDB-WASM in the page, the build scripts, CI and the website's `demo:update`. Spikes on each open technical question follow.

**Naming.** "The module" is `seaquel-browser`'s wasm output. "The editor module" is `seaquel-wasm`. "The bridge" is the TypeScript object that drives DuckDB-WASM for the browser driver. "The snapshot" is the serialized metadata file the page keeps in IndexedDB. "The old file" is the sql.js database in `localStorage["seaquel_db"]`, which phase 8 deletes unread (Q2).

---

## What the code shows

### Core and the `browser` feature

1. **`browser` builds Core with nothing in it.** `crates/seaquel-core/src/lib.rs:30-52` is a `compile_error!` if `browser` is combined with any engine, `storage`, `secrets`, `ssh`, `git`, `license-*` or `workspace`. CI checks `cargo clippy --target wasm32-unknown-unknown -p seaquel-core -p seaquel-rpc --no-default-features --features seaquel-core/browser` (`.github/workflows/ci.yml:59-60`). In that build `db.connect`, `run`, `page`, `tablePage` and `applyChanges` answer `NOT_SUPPORTED` (`crates/seaquel-rpc/src/db.rs:632-640`). The library, settings and ui groups are behind `storage`.
2. **The async groundwork from phase 0 holds.** `seaquel_runtime::{MaybeSend, MaybeSync, BoxStream}` drop `Send` on wasm32 (`crates/seaquel-runtime/src/lib.rs:27-49`). `WasmExecutor` spawns with `spawn_local`, sleeps with `gloo-timers` and reads `performance.now()` through the global object, so it also works in a worker (`:111-157`). The `#[async_trait]` macro switches to `?Send` (`crates/seaquel-macros/src/lib.rs:14-35`). `crates/clippy.toml` forbids `Instant`, `SystemTime` and `tokio::spawn`. `spawn_blocking` isn't on the list.
3. **What `storage` adds to Core** is pure apart from one `std::fs::metadata` (`upgrade.rs:411`, the VACUUM size check): `library.rs` (1,401 lines), `state.rs` (2,113), `upgrade.rs` (431) and `projection.rs` (79). Core doesn't name sqlx anywhere. It reaches the database only through `seaquel_storage`'s functions and `Storage`/`WriteTx`/`Reader`.
4. **The RPC entry points** are `seaquel_rpc::parse_request` (`workspace.rs:384`), `dispatch_workspace` (`:486`), `dispatch_stream` (`db.rs:660`, which borrows Core and the workspace for the stream's life) and `workspace_events` (`db.rs:767`). All three transports would call them the same way.

### Storage

5. **`seaquel-storage` is sqlx all the way down.** The crate is about 9,400 lines, with 319 sqlx call sites: 190 `sqlx::query`, 37 `query_as` (tuples only, no derives), 22 `query_scalar`, 144 `.execute`, 53 `.fetch_all`, 36 `.fetch_optional`, 17 `.fetch_one`, 493 `.bind`, 31 `.get`/`try_get`, and `SqliteRow` in every row mapper (`queries/codec.rs` centralises the column readers). Migrations are `sqlx::migrate!("./migrations")` (`open.rs:22-23`), recorded in `_sqlx_migrations` with sqlx's checksums.
6. **The write transaction leans on tokio:** `tokio::sync::Mutex` for the write turn (`open.rs:93`, `:228`), `tokio::time::timeout` for `WRITE_WAIT` (`write.rs:178`, `:231`), and `Handle::try_current` for the drop-time `ROLLBACK` (`write.rs:127`). Opening reads the file system: the legacy JSON check, the corrupt check, `create_dir_all` (`open.rs:265-305`). The workspace's `tokio` dependency carries `net` (root `Cargo.toml`), and so does every crate that inherits it.
7. **The demo has its own copy of the schema.** `src/lib/storage/schema.ts` (549 lines, `CURRENT_STORAGE_VERSION = 4`) creates the tables the TypeScript repositories and twins use, including the 5d-2 `windows`/`window_state` tables without `write_seq`. The sql.js file is the same SQLite format as `seaquel.db` (spike S6).

### The DuckDB engine

8. **`seaquel-engine-duckdb` has no features** (`Cargo.toml`). `duckdb` and `tokio` are unconditional. `lib.rs` declares `blocking`, `decode` and `driver` beside `dialect` and `introspect`. The native parts are `blocking.rs` (161), `decode.rs` (824, Arrow from `duckdb::arrow`, i.e. arrow-rs 58.4) and `driver.rs` (1,118, `spawn_blocking` at `:246`, `:275`, `:751`, `:853`, `:975`). `dialect.rs` (468) and `introspect.rs` (775: catalog SQL and parsers over `QueryResult`) import only `seaquel_engine`, `seaquel_types` and `serde_json`. The driver's introspection methods (`driver.rs:875-990`) are about 120 lines of glue around `introspect`.
9. **Versions differ.** The native crate is duckdb-rs 1.10505.0, a DuckDB 1.5 release. DuckDB-WASM 1.32.0 reports `v1.4.3` (spike S5).
10. **The crate rules** (`scripts/check-crate-deps.mjs`): engine crates may use only `seaquel-engine`, `-runtime`, `-types` and `-sql`, and never another engine.

### The demo's TypeScript twins and seams

11. **The demo is a fallthrough.** `isDemo()` is `!isTauri() && !isWeb()` (`src/lib/utils/environment.ts:45-47`), so a desktop build opened in a plain browser counts as the demo. `VITE_IS_DEMO` is defined (`vite.config.js:67-69`) and read nowhere.
12. **Ten seam sites pick the twin with an implicit else:** `storage/db.ts:14`; `library/index.ts:49`, `:104`, `:151` (library, ui, settings); `query-runner/index.ts:28`; `edit-service/index.ts:36`; `engine/index.ts:44`, `:60`; `providers/index.ts:25`; and `providers/provider-registry.ts:30`. `getCoreClient` (`src/lib/core/index.ts:19-23`) hands the demo an `HttpCoreClient` that nothing calls. `RustStorageClient` picks its transport per call (`storage/rust-client.ts:95-154`: `tauriCoreTransport` or `httpCoreTransport`), so a third transport fits there.
13. **Eight explicit `isDemo()` branches:**
    - `features/index.ts:66` turns off new and edited connections, SSH, MSSQL, file export, the updater, the type selector and shared projects. The AI assistant stays on.
    - The "Demo" badge (`sidebar-manage.svelte:121`).
    - `demo-connection` can't be removed or edited (`sidebar/manage/connections.svelte:310`, `connection-manager.svelte.ts:868`).
    - No `ChangeFeed`/`LibrarySync` (`hooks/database.svelte.ts:342`) and no `listenForCoreEvents` (`:404`).
    - The demo's own start (`routes/(app)/+layout.svelte:115`, `src/lib/demo/init.ts:17`).
14. **The twins**, in production lines:
    - `ts-library.ts` 1,502, `ts-state.ts` 760, `ts-settings.ts` 658, `ts-ui.ts` 270;
    - `ts-runner.ts` 566 and `ts-service.ts` 606;
    - `engine/ts-engine-client.ts` 162;
    - `db/duckdb.ts` 527, `alter-table.ts` 221, `crud-helpers.ts` 102, and `db/index.ts` 155 (`getAdapter`);
    - the sql.js storage: `sqljs-client.ts` 60, `web-sqlite.ts` 126, `schema.ts` 549, `repos/` (19 files, about 1,300), `create-repo.ts` 244, `repository.ts`, `sqlite-types.ts` and `sql.js.d.ts`.

    That is about **7,900 lines**. `DuckDBProvider` (361) also serves the tutorial on web and in the demo (`providers/index.ts:42-54`, `tutorial/database.ts:285-302`).
15. **The demo's connection is TypeScript's.** `ConnectionManager.addDemoConnection` (`connection-manager.svelte.ts:927-1000`) stores the fixed `demo-connection` row through `TsLibrary.putDemoConnection` (`ts-library.ts:879-890`). That call stores the row once, then only bumps `last_connected`, so the user's labels stay. No Rust code knows the id, although CLAUDE.md says Core has it fixed. The engine client is built inline (`:984-989`). DuckDB is `:memory:` and is seeded from `src/lib/demo/sample-data.ts` on every load through `provider.execute` (`init.ts:16-56`). Edits to the sample tables are lost on reload.
16. **Tests that use the twins.**
    - Replays of the recorded fixtures: `ts-runner-fixtures.test.ts` (95 run cases), `ts-library.test.ts` (the library set), `library-replay.svelte.test.ts` (133 steps through the view models), `state-replay.svelte.test.ts` (4,488 lines, 112 cases and 377 steps), `client.test.ts`'s sql.js half (the storage repo fixtures) and `ts-service-duckdb.svelte.test.ts` (the DuckDB edit cases).
    - Behaviour tests that use the twins as a stand-in database: `dashboard-review`, `library-persistence`, `window-state` and `stores/settings-across-tabs`.
    - Each replay names its twin-only exemptions: `TS_ONLY`, `SKIPPED`, `EXEMPT` and `keyless()`.

### DuckDB-WASM in the page

17. **`DuckDBProvider`** loads `@duckdb/duckdb-wasm` 1.32.0 from jsDelivr (`duckdb-bundles.ts:25-30`) through a blob worker with a 30 s start timeout (`:49-85`, `duckdb-provider.ts:180-215`). Its behaviour:
    - Every connection shares one in-memory database.
    - `select` and `execute` ignore their parameters (`:239-253`, `:309-320`), so values are inlined before they reach it.
    - Rows are Arrow's `toJSON()`: unscaled decimals and timestamps as float milliseconds (spike S4).
    - `selectStream` sends one final batch (`:255-282`).
    - A cancel only rejects the promise; the query keeps running (`:97-101`, `:140-145`).
    - The read-only path opens a fresh connection with `BEGIN TRANSACTION READ ONLY` and `SELECT * FROM query(?)` (`:103-146`). Without `maxRows` it has no row cap.
18. **The web build** serves its own DuckDB bundles for the tutorial (`duckdb-local-bundles.ts`, about 75 MB, gated on `VITE_BUILD_TARGET === "web"` so Rollup drops them elsewhere). The demo uses jsDelivr. Nothing sets COOP/COEP, so the COI bundle is never chosen.

### Persistence today

19. **The old file.** `WebSqliteDatabase` re-exports the whole file as base64 into `localStorage["seaquel_db"]` after every `execute` and `transaction` (`web-sqlite.ts:11-15`, `:38-50`, `:71-84`). That costs O(file) per write and is capped by the roughly 5 MB quota. A blob that doesn't open silently starts an empty database (`:104-122`). After one load and one reload the file is 303 KB (404,140 base64 characters). Nothing else uses IndexedDB. Two tabs each keep their own copy, and the last writer wins.

### Build, CI and the website

20. **`scripts/build-wasm.mjs`** (374 lines) runs cargo with `--profile wasm-release` (`opt-level = "z"`, LTO, `panic = "abort"`, strip; root `Cargo.toml:110-119`). It then runs `wasm-bindgen --target web`, `wasm-opt -Oz` from npm `binaryen` 130.0.0, a glue patch (`__seaquel_reinstantiate`) and a 2 MiB stack check. It prints the sizes and keeps a stamp. Every `dev`, `build`, `check` and `test` script runs it first. `src/routes/+layout.ts:15-23` awaits the editor module before anything renders.
21. **CI** builds the pure crates and Core's `browser` set for wasm32 (`ci.yml:53-60`) and runs `npm run build:demo` (`:258-260`). Nothing deploys the demo.
22. **The website.** `demo:update` is one inline command in `packages/marketing/package.json:15`:

    ```sh
    (rm -fr ./static/demo && cd ../../../../seaquel/ && npm run build:demo && cp -r ./build-demo ../seaquel-app/main/packages/marketing/static/demo)
    ```

    It copies whatever `build-demo` holds and sets no environment of its own. The site runs on Cloudflare Workers static assets (`adapter-cloudflare`, `wrangler.jsonc`) and sends no COOP/COEP. **`static/demo` is committed and dates from 2026-09-23 (`8a6ed59`).** That is before phase 1. It has no editor module, so the next `demo:update` ships phases 1–8 to the demo at once.

### Sizes and load times today

Measured on a fresh `vite build --mode demo` of this tree in a scratch copy (`SEAQUEL_WASM_PREBUILT=1`). Compressed sizes are Node zlib (gzip -9, brotli 11), as `build-wasm.mjs` prints them.

| | Raw | brotli |
|---|---|---|
| `build-demo` on disk | 28 MB, 314 files | |
| JavaScript, 255 files (Monaco's workers included, most loaded lazily) | 25.39 MB | 4.29 MB |
| `seaquel_wasm_bg.wasm` (the editor module) | 1.73 MB | 487 KB |
| `sql-wasm-browser.wasm` (sql.js) | 658 KB | 279 KB |
| DuckDB-WASM `duckdb-eh.wasm` + worker, from jsDelivr | 34.2 MB + 0.77 MB | 5.92 MB + 0.17 MB (brotli 9) |

Load times (Playwright, headless, served from localhost with no compression, DuckDB from jsDelivr), measured from navigation until the "Demo database loaded with sample data" toast:

| | Cold | Reload |
|---|---|---|
| Chromium | 0.88–0.90 s | 0.13 s |
| Firefox | 1.44–1.49 s | 0.30 s |
| WebKit | 1.89–1.92 s | 0.25 s |

A cold load served 17.5 MB from localhost.

### Bugs and gaps the survey found

"Seen" means reproduced in the spikes; "by reading" means not reproduced.

1. **Every reload toasts an error** (seen, all three browsers): "Couldn't save the dashboard: There's already a dashboard called "E-Commerce Overview" in this project." `createDemoDashboard` (`src/lib/demo/sample-dashboard.ts:236-259`) creates the sample dashboard on every load without looking. Before 5d-2's name check, each reload added another copy: the live demo's file holds two after one reload (spike S6). **Fixed in Task 1.**
2. **Every reload opens another query tab** (seen: "Query 2" after one reload). The cause wasn't traced. Task 1 traces and fixes it with bug 1.
3. **The demo's AI can't use Anthropic** (by reading). `NoopKeyringService` returns no key (`services/keyring.ts:171-227`), `TsSettings` refuses keys (`ts-settings.ts:337`), and the browser `fetch` sends no `anthropic-dangerous-direct-browser-access` header (`services/ai/providers.ts:95`, `:183`). Only a keyless OpenAI-compatible endpoint can work. See Q7.
4. **A corrupt old file is replaced silently** (by reading, `web-sqlite.ts:116-119`). Moot after phase 8: the old file is deleted unread (Q2).
5. **CLAUDE.md is wrong about `demo-connection`** (item 15). Task 8 corrects it.
6. **Dead code:** `getDemoConnectionConfig` (`init.ts:61-72`); `DuckDBProvider.executeRaw`, `getDb` and `getConnection`; `VITE_IS_DEMO`.
7. **Known demo gaps that Core closes** (5b, 5c):
   - no key check (`NOT_EDITABLE`);
   - a count before every data-tab page;
   - filter values inlined as literals;
   - a trailing `--` swallowing the page's `LIMIT` (`duckdb.ts:488`);
   - an empty page without column names;
   - no substitution budget;
   - a cancel that leaves the query running.
8. **A Monaco error at load** (seen, all three, headless): `TypeError: … reading 'parentNode'` in `registerEditorContainer`. It is older than this phase and wasn't traced. The probe checks it in a headed browser.

---

## Spikes (2026-10-01)

Each spike ran in the session scratchpad (`…/scratchpad/p8-spikes/`) with `CARGO_TARGET_DIR=…/scratchpad/p5a/target`, against a copy of the crates where code had to change. None of it is in the tree.

**S1. sqlx can't run in the browser.** `cargo check -p seaquel-storage --target wasm32-unknown-unknown` fails in three places:
- `mio` ("This wasm target is unsupported"; the workspace's tokio has `net`);
- `ring` (its C build can't target wasm32);
- `getrandom` (no `js` feature).

Fixing those wouldn't be enough: sqlx-sqlite runs each connection on a `std::thread` (`sqlx-sqlite-0.8.6/src/connection/worker.rs:106`), which panics on `wasm32-unknown-unknown`. So storage needs a second executor under the same queries.

**S2. SQLite runs in all three browsers.** rusqlite 0.40 with `bundled` and `serialize` builds for wasm32 on `sqlite-wasm-rs` 0.5.5 (SQLite 3.53.0). In Chromium, Firefox and WebKit it:
- opened a sql.js-made file through `sqlite3_deserialize`;
- read `Straße 東京` back exactly;
- kept foreign keys on;
- wrote, serialized and stored the file in IndexedDB, and after a reload reopened it with the row count one higher.

Timings: init 2–16 ms, opening a 20 KB file 0–7 ms, and serializing plus storing a 9.46 MB file 5–27 ms. Size after wasm-bindgen: 1.07 MB raw / 396 KB brotli, or 939 KB / 363 KB after `wasm-opt -Oz`. sql.js is 658 KB / 279 KB.

Two catches:
- **sqlite-wasm-rs compiles SQLite's C with `clang --target=wasm32-unknown-unknown`, and Apple's clang has no wasm32 backend.** The build failed until `CC_wasm32_unknown_unknown` pointed at an LLVM clang (from nix here; `brew install llvm` on a Mac). Since 0.5 the crate has no precompiled option; 0.4 had one. Ubuntu's clang, as on the CI runners, has the backend. See Q9.
- **rusqlite can't join this workspace.** rusqlite 0.40 needs libsqlite3-sys 0.38, while sqlx 0.8.6 and `seaquel-engine-sqlite` (`=0.30.1`) need 0.30. Both declare `links = "sqlite3"`, and Cargo refuses the pair even when rusqlite is optional and wasm32-only, because it resolves `links` across every target. rusqlite 0.32 shares libsqlite3-sys 0.30 but has no wasm32 support. `sqlite-wasm-rs` alone declares `links = "wsqlite3"` and resolves next to sqlx (checked with `cargo metadata`). So the browser executor calls sqlite-wasm-rs's C API itself (Decision 3); all 260 functions are there, `sqlite3_serialize`/`deserialize` included.

**S3. Core with `workspace` builds and runs in the page.** The only change was dropping `workspace` from the `compile_error!` list, plus a `native` feature on the DuckDB crate. Core (`browser` + `workspace`) and `seaquel-rpc` (`workspace`) built for wasm32 with one warning: `Outcome.elapsed_ms`/`row_count` are unread without `storage`.

A spike crate supplied a DuckDB `Engine` whose driver calls JavaScript through wasm-bindgen (`JsFuture` over a promise), plus `call`/`stream` exports over `parse_request`, `dispatch_workspace` and `dispatch_stream`. With a real DuckDB-WASM in Chromium, Firefox and WebKit:
- `db.connect` with a form answered a Core connection id.
- `db.run` over three statements (a `CREATE TABLE … AS` of 1,000 rows, a `SELECT … WHERE id >= {{min}}` and a typed row) gave the right events: `{{min}}` inlined for DuckDB, page 1 of 10 with the count probe (`totalRows: 990`), and `done`.
- `db.page` for page 3 returned rows 210–309.
- `DROP TABLE t` without `confirmed` ended with `CONFIRM_REQUIRED`.
- Instantiating plus building Core took 10–11 ms in Chromium, 18 ms in Firefox and 102 ms in WebKit. The run took 49, 5 and 32 ms (Firefox's timer is coarse).

`MaybeSend` was enough: nothing needed `Send`, and every borrow of Core and the workspace across a stream held.

Size, without storage: 4.34 MB from cargo, 3.66 MB after wasm-bindgen (1.06 MB gzip, **756 KB brotli**). `wasm-opt -Oz` made it smaller raw (3.30 MB) but larger compressed (1.12 MB gzip, 809 KB brotli). The editor module ships `-Oz`; for this module that pass costs about 7%.

**S4. DuckDB-WASM can stop a query, but not through prepared statements.** In all three browsers:
- `conn.send(sql)` followed by `cancelSent()` 300 ms later stopped `SELECT sum(range) FROM range(30000000000)` with "query was canceled" within about 0–100 ms of the cancel. The same connection ran the next query in 2–7 ms.
- `prepare(…).query(9007199254740993n, …)` failed with "Do not know how to serialize a BigInt" (each browser's wording): DuckDB-WASM sends prepared parameters as JSON. So the browser driver can't bind a `bigint` (or, by the same path, bytes) as a parameter.
- Arrow-JS values lose information: `DECIMAL(10,2)` 123.45 comes as `DecimalBigNum` "12345" (unscaled), `TIMESTAMP` as the float `1704164645123.456`, and `INTERVAL` as an `Int32Array`.

**S5. Rust can decode DuckDB-WASM's own Arrow bytes.** `AsyncDuckDB.runQuery(conn, sql)` returns Arrow IPC in file format (`ARROW1`). `arrow_ipc::reader::FileReader` decoded it in the page exactly: `Decimal128(10, 2)=123.45`, `Timestamp(µs)=2024-01-02T03:04:05.123456`, lists, structs, UUID as text.

Two differences from native DuckDB:
- A TIMETZ loses its offset.
- A 39-digit HUGEINT comes as `Decimal128(38, 0)` and is wrong. `SET arrow_lossless_conversion = true` is accepted and changes nothing on v1.4.3.

The decoder with `arrow-cast`'s display is 839 KB raw / **183 KB brotli**, an upper bound for `decode.rs` over arrow-rs.

**S6. Today's demo data opens with the native Core, both versions.** Two sql.js files were dumped from `localStorage` after a cold load and one reload: one from today's demo build, one from the live demo (`static/demo` of 2026-09-23). Copied as `seaquel.db`, each was opened by `Core::open_workspace`. The baseline, migrations `0001`–`0005` and all four data steps ran. `connectionsList` (today's: `demo-connection` with its `prod` label), `projectsList` and `dashboardsList` answered. The live demo's file has no connection row and two sample dashboards with the same name; both stayed (a name check applies on create). So the old file could have become the new file's starting image. The owner chose to start fresh instead (Q2 C), so this result isn't used.

**S7. Today's demo** (the measurements above, and bugs 1, 2 and 8). The reload in Playwright only worked once the static server resolved `/demo/manage` to `manage.html` as Cloudflare does, which is worth knowing for the probe.

---

## Answered questions (2026-10-02)

The owner answered all nine on 2026-10-02: every one with the recommended option except Q2. Each keeps the options that were weighed, so later changes start from them. The decisions below follow the answers.

### Q1. Where the demo keeps its data, and two tabs

Today the whole sql.js file goes into `localStorage` as base64 after every write. Core can't use `localStorage`, since it needs synchronous file access. Options were:
- **A:** SQLite in memory on the main thread, with a snapshot in IndexedDB after every committing call. A crash between a commit and its snapshot loses that one change. Two tabs each load the file when they open, and the last tab to write wins.
- **B:** OPFS in a dedicated worker. Real durability, but Core moves into a worker, the transport becomes `postMessage`, and a second tab can't open the file. About +3–4 h.
- **C:** A with Web Locks, so that only one tab writes. About +1 h.

**Answer (owner): A.** Decisions 2 and 6.

### Q2. The old demo data

Visitors have demo data in `localStorage["seaquel_db"]`. S6 showed both the current and the live demo's files open with Core. Options were:
- **A:** bring it over once as the starting image (about 0.5 h).
- **B:** start fresh and leave the key unread.
- **C:** start fresh and delete the key.

**Answer (owner): C.** Nothing is imported. On the first start of the new demo the page deletes `localStorage["seaquel_db"]` and any other `seaquel_db.*` key an earlier build wrote, and starts with an empty metadata file. Visitors lose their earlier demo saved queries, history and dashboards; the release notes say so. Decision 6.

### Q3. Size budget and load time

The module without storage is 756 KB brotli (S3). SQLite adds about 363–396 KB (S2), Arrow decoding up to 183 KB (S5), and storage plus Core's library and state code an estimated 200–400 KB. sql.js (279 KB) and the twins go. Net: about +1.0–1.4 MB brotli. Options were:
- **A:** a 2.0 MB brotli budget for the module, checked by the build. A cold load may be at most 400 ms slower than Task 1's baseline in each browser. `wasm-opt` chosen per module by brotli size.
- **B:** merge the editor module into the module (saves about 480 KB brotli; a trap in a keystroke function would also reset Core). About +1.5 h.
- **C:** no budget, measure only.

**Answer (owner): A.** Decision 21; Task 7 measures it.

### Q4. Offline, and where DuckDB-WASM comes from

Options were:
- **A:** jsDelivr for DuckDB as today, no offline mode.
- **B:** self-host DuckDB-WASM in the demo (34–39 MB per file, over Cloudflare's 25 MiB asset limit).
- **C:** B plus a service worker. About +2 h.

**Answer (owner): A.**

### Q5. The tutorial

The tutorial runs on `DuckDBProvider` on web and at `/demo/learn`. Options were:
- **A:** leave it as it is; `DuckDBProvider` stays, trimmed to what the tutorial uses, and the web build doesn't load the module.
- **B:** the demo's tutorial on the demo's Core. About +1 h.
- **C:** both on an in-page Core; the web build loads the module for the tutorial. About +2 h.

**Answer (owner): A.** Decision 24.

### Q6. The demo's behaviour differences

With Core in the page the demo behaves as desktop does: an edit without a primary key is `NOT_EDITABLE`; data-tab totals can be estimates and filters are bound; a cancelled query stops; empty results show their columns; `{{param}}` values meet Core's 32 MiB substitution budget; grid writes queue and apply as on desktop. Options were:
- **A:** Core's behaviour, with no limits beyond desktop's.
- **B:** Core's behaviour with the web's limits.
- **C:** keep some twin behaviour.

**Answer (owner): A.** No `RunLimits`, `EditLimits`, `LibraryLimits` or `StateLimits` (Decision 13).

### Q7. The AI assistant in the demo

It is on in the demo, but no Anthropic key can be stored or sent (bug 3). Options were:
- **A:** turn it off in the demo; a phase 6 item.
- **B:** leave it as it is.
- **C:** a session-only key in the module. About 1–1.5 h.

**Answer (owner): A.** `features.aiAssistant` is false in the demo (Task 6).

### Q8. Browser support

Options were:
- **A:** desktop Chrome/Edge, Firefox and Safari, current and previous major version, tested. Mobile browsers load the demo but aren't tested. A browser without WebAssembly or IndexedDB gets a page saying so; private windows work and keep nothing after they close.
- **B:** A plus iOS Safari on a device. About +0.5 h.
- **C:** whatever the three Playwright engines run.

**Answer (owner): A.**

### Q9. The C toolchain for SQLite in wasm

`sqlite-wasm-rs` needs an LLVM clang with the wasm32 backend (S2). Apple's clang has none, and `mise` has no LLVM package. Options were:
- **A:** build the module only where it's used (`build:demo`, `dev:demo`, `test`, CI's demo and test steps) and find clang automatically (`CC_wasm32_unknown_unknown`, then Homebrew's `llvm`, then `clang --print-targets`), stopping with the fix named when none is found.
- **B:** build it in every `wasm:build`.
- **C:** commit a prebuilt `libsqlite3.a` for wasm32.
- **D:** a bridge to sql.js.

**Answer (owner): A.** Decision 21. `npm test` and `demo:update` need `brew install llvm` once on a Mac.

---

## Decisions (2026-10-02)

Settled with the answers above. Numbered from 1: phase 8 is its own phase.

### Scope and placement

#### 1. Scope

- The demo runs Core in the page: library, settings, view state, history, runs, pages, edits, the data tab, DuckDB extensions, introspection, EXPLAIN, statistics, and the AI's and dashboards' read-only queries.
- The twins and the sql.js storage are deleted (item 14), `getAdapter` and `TsEngineClient` with them, and `sql.js` leaves `package.json`.
- Not in scope:
  - the tutorial (Q5);
  - the AI itself (Q7, phase 6);
  - offline use (Q4);
  - new demo features: new connections, file engines and imports stay off (`features/index.ts:66`);
  - the web and desktop GUIs, apart from the storage facade under them.

#### 2. One page, one Core, on the main thread

The module is instantiated once per page, before the app renders: a demo-only branch next to the editor module in `src/routes/+layout.ts`. It builds one `Core` and opens one `Workspace`. Core's work per call is small; DuckDB, the heavy part, stays in its own worker (Q1 A).

### Storage

#### 3. One set of queries, two executors

- `seaquel-storage` gets a `db` module whose API is the subset of sqlx the crate uses: `query`/`query_as`/`query_scalar` with `.bind`, `.execute`/`.fetch_all`/`.fetch_optional`/`.fetch_one` on a connection or `Reader`, rows read by column name, `rows_affected`, and errors that keep SQLite's message and code (the `"no transaction is active"` test in `write.rs` and `classify`'s corrupt check depend on them).
- **On native targets it is a zero-cost re-export over sqlx.** The 319 call sites change their path, not their behaviour, and desktop and web storage is byte for byte what it was. The storage fixtures, Core's replays and one full live run pin that.
- **On wasm32 it is a wrapper over `sqlite-wasm-rs`'s C API** (open, prepare, bind, step, column, finalize, changes, errmsg, serialize, deserialize), about 400–600 lines with the only `unsafe` in the crate. It is synchronous behind `async` signatures. rusqlite isn't possible (S2), and diesel would mean rewriting every query.
- The choice is by `target_arch`, not a Cargo feature, so feature unification can never put both in one build.
- Not a trait object: no call pays for dynamic dispatch, and the design doc's `StorageBackend` trait isn't needed.

#### 4. The browser's storage open

- `StorageOptions::in_memory(image: Option<Vec<u8>>)` opens SQLite's memory database and, given an image, deserializes it. Then it runs the frozen baseline, the numbered migrations and the data steps as `Storage::open` does.
- **Migrations are recorded exactly as sqlx records them**: `_sqlx_migrations` with the same version, description, SHA-384 checksum and `success`. A file made in the browser and one made on desktop are interchangeable. Task 2 checks it both ways: a module-made file opens natively with nothing pending, and S6's files open in the module.
- No WAL, busy timeout or `max_page_count` (one connection in memory). `STORAGE_CORRUPT` comes from SQLite's answer to the image, and the legacy JSON check doesn't exist there.
- `Storage::snapshot() -> Vec<u8>` is `sqlite3_serialize`.
- A commit counter tells the page whether anything committed since the last snapshot, so a write that emits no event (a data step, a refill) is still kept.

#### 5. The write turn without tokio

The write mutex becomes `async-lock`'s (or `futures::lock`'s), which works on both targets. `WRITE_WAIT` races the executor's `sleep`; storage gets the executor from Core at open. On wasm32 a dropped `WriteTx` sends its `ROLLBACK` synchronously in `Drop`. Native keeps `Handle::spawn`, and its behaviour is unchanged. The `net` feature stops reaching `seaquel-storage` (its tokio dependency stops inheriting the workspace's features).

#### 6. The snapshot, and the old file deleted (Q1 A, Q2 C)

The transport stores the snapshot in IndexedDB (`seaquel-demo`/`files`/`meta.db`). That happens after each call or stream that left the commit counter higher (coalesced in one macrotask), on `pagehide` and on `visibilitychange` to hidden. One write is in flight at a time, and the newest snapshot wins.

At open, a snapshot in IndexedDB is the image; otherwise there is none and Core starts an empty file. The old file is never read.

**The old keys go.** Before opening, the transport removes `localStorage["seaquel_db"]` and every key starting `seaquel_db.`. It does this on every start, not once: a stale cached copy of an older demo build in another tab can write the key again, and the next start removes it. Removing a missing key is a no-op, so this costs nothing. Other keys (`seaquel-theme-cache`, the paraglide locale) stay.

If IndexedDB can't be opened (blocked storage, an old private mode), Core runs in memory and the page shows once that nothing will be kept. A snapshot that doesn't open is moved to `meta.db.unreadable`, Core starts empty, and the page says so.

### The engine

#### 7. The browser driver lives in `seaquel-engine-duckdb`

- The crate gets `native` (default: duckdb-rs, tokio, `blocking`, `driver`) and `browser` (wasm32: wasm-bindgen, js-sys, `arrow-ipc`).
- `dialect.rs`, `introspect.rs` and `decode.rs` serve both.
- The design doc named a separate `seaquel-engine-duckdb-wasm`. One crate keeps the rule that an engine never depends on another and lets the two drivers share the decoder without a third crate.
- The engine id stays `duckdb`, so saved rows, fixtures and the GUI's routing don't change.

#### 8. Values cross as Arrow bytes

- The bridge hands Rust the IPC bytes DuckDB-WASM produces: `runQuery`'s file, and per-batch chunks for streams (`startPendingQuery`/`fetchQueryResults`, to be confirmed in Task 4's first test).
- Rust decodes them with `arrow-ipc` and the same `decode.rs`, made generic over arrow-rs's arrays (duckdb-rs re-exports arrow 58.4; the browser driver pins `arrow-ipc`, `arrow-array` and `arrow-schema` to that version).
- Cells are therefore what the desktop shows: TIMESTAMPTZ in UTC text, STRUCT/MAP as sorted JSON.
- The two differences S5 found (TIMETZ without its offset; HUGEINT past 38 digits) are named in the value tests and in CLAUDE.md. Converting cells in JavaScript was rejected: S4 showed unscaled decimals and float timestamps.

#### 9. Parameters are written as typed literals

DuckDB-WASM can't bind a `bigint` (S4). So the browser driver writes every bound `?` as a typed literal, using `seaquel_sql::params`' DuckDB value writer (fix 13's rules from 2b, which `{{param}}` already uses for DuckDB) and DuckDB's scanner to find each `?` outside strings, comments and quoted names. Literal forms:
- `NULL`; `TRUE`/`FALSE`;
- integers as written;
- floats with `'nan'`/`'inf'` casts;
- decimals as `CAST('…' AS DECIMAL(p, s))` from their text;
- text as an escaped `'…'`;
- bytes as `from_hex('…')`;
- JSON as `'…'::JSON`;
- arrays as list literals.

The edit fixtures' DuckDB cases and a bind round trip per value kind pin it. Run and page statements have no binds on DuckDB already (S3: `"params": []`).

#### 10. Streams stop on the server

`query_stream` uses a pending query and fetches batch by batch, so a page or a 100,000-row stream crosses in pieces. Dropping the stream cancels it (`cancelPendingQuery`, or `cancelSent` for `send`), and S4 showed the query stops. With Decision 9 every statement is unprepared, so every statement can be cancelled. Each Core connection is one DuckDB-WASM connection on the page's single database. `close` closes it.

#### 11. The rest of `Driver`, as the native driver does it

- `query`, `execute` (rows affected from DuckDB's `Count` column).
- `transaction`: `BEGIN`/`COMMIT`, the `TRANSACTION_OPEN` probe by two `txid_current()` calls, `expect_rows`.
- `query_read_only_with`: a fresh connection per call, `BEGIN TRANSACTION READ ONLY`, `SELECT * FROM query('<the SQL as a literal>') LIMIT <cap + 1>`, then `ROLLBACK` and close; `max_bytes` checked while decoding.
- `explain` and `explain_read_only`, `list_schemas`, `schema_tables`, `table_metadata`, `statistics`: through `introspect`, as `driver.rs:875-990` does.
- The connect config's path is ignored: every connection opens on the page's one database, as `DuckDBProvider` does.
- `restricted` is refused with `NOT_SUPPORTED` (the browser has no MCP server).

#### 12. The bridge is the page's

DuckDB-WASM is imported, its bundle chosen (`duckdb-bundles.ts`, jsDelivr) and its worker started (`startWithin`) by TypeScript, which passes Rust a bridge object at `open`. Its methods: `connect() → id`, `runQuery(id, sql) → Uint8Array`, `startPending(id, sql)`, `fetchChunk(id) → Uint8Array | null`, `cancel(id)` and `close(id)`. In Rust it is a wasm-bindgen `extern` type. It has no snippet files, and there is only one place that knows DuckDB-WASM's API. ~~The tutorial's `DuckDBProvider` uses the same `AsyncDuckDB` instance (Q5 A).~~ **Amended in Task 6's review:** the tutorial's `DuckDBProvider` has an `AsyncDuckDB` instance of its own (`tutorialDuckDb`), as before phase 8, apart from the demo's (`pageDuckDb`). Sharing one catalog showed Learn's tables in the demo connection, let a visitor's tables break the lessons, and let the tutorial's sandbox drop the demo schema. The second instance costs a worker and its memory; the worker script and the wasm come from the browser's cache.

### The module and the transport

#### 13. `seaquel-browser`

- A new interface crate: `cdylib`, wasm32-only dependencies on `seaquel-core` (`browser`, `storage`, `workspace`), `seaquel-rpc` (`storage`, `workspace`) and `seaquel-engine-duckdb` (`browser`).
- Because the dependencies are target-specific, `cargo test --workspace` on native builds it empty, and `browser` is never unified into a native Core.
- Exports (all strings or bytes, like the editor module):
  - `open(bridge, image?)`;
  - `call(body: Uint8Array) → Promise<string>`;
  - `stream(body, onEvent) → Promise<number>`;
  - `events(onEvent) → unsubscribe id`;
  - `snapshot() → Uint8Array`;
  - `commits() → number`;
  - `__test_trap` (debug feature only).
- Core is built with `ConnectPolicy::Unrestricted`, `WasmExecutor` and no limits (Q6 A), and with the DuckDB engine only.
- The write origin is `demo`, the demo's window id (`window-id.ts:40`).

#### 14. `browser` admits `storage` and `workspace`

The `compile_error!` list drops them and keeps the engines, `secrets`, `ssh`, `git`, `license-*` and `imports`. CI's wasm32 line becomes `-p seaquel-core -p seaquel-rpc -p seaquel-storage -p seaquel-engine-duckdb -p seaquel-browser`, with `--no-default-features` and the browser features.

#### 15. The in-page transport

- `BrowserCoreClient` implements `CoreClient`:
  - `call` → `call`;
  - `stream` → `stream`, which yields events through a queue and sends `db.cancel` on abort or when the loop is left;
  - `events` → `events`;
  - `onResubscribed` fires `initial` at open and again after a trap recovery.
- `browserCoreTransport` serves `RustStorageClient`.
- **Nothing calls into the module synchronously from inside a callback the module made.** Events and stream items are handed to TypeScript in a microtask, because a re-entrant call would hit wasm-bindgen's borrow checks or a `RefCell` already borrowed in Core.
- The branch is `import.meta.env.VITE_BUILD_TARGET === "demo"`, evaluated by Rollup as the DuckDB bundles' branch is, so desktop and web don't bundle the module or the transport.

#### 16. A trap is survivable

The module is built with `panic = "abort"` like the editor module, so a Rust panic leaves the instance unusable. The transport catches the trap on any export, instantiates a new module and reopens from the last stored snapshot. It answers the failed call with `CORE_RESTARTED`, fires `onResubscribed`, and marks the demo connection disconnected so `ConnectionManager` reconnects it. DuckDB's data survives, since it lives in DuckDB's worker. What's lost is the failed call and anything committed after the last stored snapshot, which with Decision 6 is that call.

#### 17. One code path for events

The demo turns on `ChangeFeed`, `LibrarySync` and `listenForCoreEvents` like desktop and web (`hooks/database.svelte.ts:342`, `:404`). Its own writes carry the `demo` origin and are skipped. Core's own writes (an upgrade, a refill) refetch as anywhere else.

#### 18. Logs

The module logs WARN and above to `console`, through a small `log` implementation. It follows the same rules as the server: activities, ids, counts and codes, never SQL, values, names or file contents.

### The demo's start

#### 19. The demo connection belongs to Core

`Workspace::ensure_demo_connection()` (behind `browser`) stores the `demo-connection` row with its fixed id once, in a `WriteTx` (Core's checks, `name_key`, a `connection` event), and afterwards only sets `lastConnected`. That replaces `putDemoConnection`, keeps the user's labels and AI flags, and makes CLAUDE.md's claim true.

The page then connects it as a saved target (`db.connect {target: {type: "saved", id: "demo-connection"}}`) and seeds the sample tables through `db.execute`, one statement at a time, as `init.ts` does now. The sample SQL stays TypeScript content, as the design doc said.

#### 20. The sample dashboard is created only when missing

That is Task 1's fix (bug 1). It checks `dashboardsList` by name before creating, and goes through Core from Task 6 on.

### Build and CI

#### 21. `build-wasm.mjs` builds two modules (Q9 A)

- `node scripts/build-wasm.mjs --module browser` builds `seaquel-browser` with the same profile, wasm-bindgen version, glue patch, stack check and stamp, into `src/lib/wasm/browser-pkg/` (gitignored).
- `wasm-opt` runs only if it makes the brotli size smaller (S3).
- It checks Q3's budget and finds a wasm-capable clang (Q9).
- `predev:demo`, `prebuild:demo` and `pretest` build it. The desktop and web scripts don't.
- `SEAQUEL_WASM_PREBUILT=1` covers both modules.

#### 22. CI

- The wasm32 clippy line of Decision 14.
- The frontend job builds the module before `vitest` (Ubuntu's clang) and keeps `build:demo`.
- The "Web server dependencies" step also fails if `sqlite-wasm-rs`, `arrow-ipc` or `wasm-bindgen` reach `seaquel-server`.

### Everything else

#### 23. Old releases and the old file

- No older release reads the snapshot (it's new), and the old file is never read (Q2 C).
- An older demo build still cached in another tab can write `seaquel_db` again; the next start of the new demo deletes it again (Decision 6). Its edits are lost, as Q2 C accepts.
- The website's next `demo:update` ships every phase since 2026-09-23 (item 22). The probe checks a visitor who had the live demo's data: the key goes, and the new demo starts clean.

#### 24. What stays in TypeScript

- `DuckDBProvider`, trimmed for the tutorial (Q5 A).
- The DuckDB bundle helpers.
- The sample data and dashboard content.
- The demo's feature flags.
- `$lib/sql` (the editor module), unchanged.

---

## The split

**Two slices in one plan, 8a and 8b, each with a checkpoint.**

- **8a: Core runs in the page (Tasks 1–5).** It covers the storage facade (which touches desktop and web), the browser storage, Core's `browser` set, the browser driver, `seaquel-browser`, the transport and the Node test harness. The demo still runs on its twins, so 8a can land and be checked, with one full live run for the facade, before any visitor sees it. Task 1's fixes can ship as a patch.
- **8b: the demo on Core (Tasks 6–8).** The seams flip, the twin-backed tests move onto the real module, the twins go, then the probe and the final checkpoint.
- **Why not one slice:** 8a's facade is the first change to desktop and web storage since 5e and needs its own full live run. 8b's deletion is only safe once the module has passed the same replays the twins pass. **Why not two plans:** 8b has no design of its own; it is the flip.

---

## The wire and the API

The RPC doesn't change. The demo sends the same `CoreRequest`s and receives the same `CoreEvent`s as desktop and web. New code is under the transport and under storage.

### Storage

```rust
// crates/seaquel-storage/src/db.rs: one API, per target
#[cfg(not(target_arch = "wasm32"))] pub use sqlx_impl::*;   // re-exports over sqlx
#[cfg(target_arch = "wasm32")]      pub use wasm_impl::*;   // over sqlite-wasm-rs
pub fn query(sql: &str) -> Query<'_>;            // .bind(v) .execute(r) .fetch_all(r) .fetch_optional(r) .fetch_one(r)
pub fn query_as<T: FromRow>(sql: &str) -> QueryAs<'_, T>;
pub fn query_scalar<T: Decode>(sql: &str) -> QueryScalar<'_, T>;
pub struct Row; impl Row { pub fn try_get<T: Decode>(&self, column: &str) -> Result<T, DbErr>; }
pub struct Done; impl Done { pub fn rows_affected(&self) -> u64; }

// crates/seaquel-storage/src/open.rs
impl StorageOptions { pub fn in_memory(image: Option<Vec<u8>>) -> Self; }   // wasm32 only
impl Storage {
    pub fn snapshot(&self) -> Result<Vec<u8>, StorageError>;               // wasm32 only
    pub fn commits(&self) -> u64;                                          // wasm32 only
}
```

### Core and the engine

```rust
// crates/seaquel-core: `browser` admits `storage` and `workspace`
#[cfg(feature = "browser")]
impl Workspace { pub async fn ensure_demo_connection(&self, core: &Core) -> Result<Seqd<PersistedConnection>, CoreError>; }

// crates/seaquel-engine-duckdb, feature `browser`
#[wasm_bindgen] extern "C" {
    pub type DuckDbBridge;
    #[wasm_bindgen(method, catch)] async fn connect(this: &DuckDbBridge) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = runQuery)] async fn run_query(this: &DuckDbBridge, conn: u32, sql: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = startPending)] async fn start_pending(this: &DuckDbBridge, conn: u32, sql: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch, js_name = fetchChunk)] async fn fetch_chunk(this: &DuckDbBridge, conn: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch)] async fn cancel(this: &DuckDbBridge, conn: u32) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch)] async fn close(this: &DuckDbBridge, conn: u32) -> Result<JsValue, JsValue>;
}
pub fn browser_engine(bridge: DuckDbBridge) -> Arc<dyn Engine>;     // id "duckdb", DuckdbDialect
pub(crate) fn decode_batches(ipc: &[u8], cap: RowCap) -> Result<(Vec<String>, Vec<Vec<Value>>, bool), DbError>;
pub(crate) fn inline_binds(sql: &str, params: &[Value]) -> Result<String, DbError>;   // Decision 9
```

### The module

```rust
// crates/seaquel-browser/src/lib.rs (wasm32)
#[wasm_bindgen] pub async fn open(bridge: DuckDbBridge, image: Option<Vec<u8>>) -> Result<JsValue, JsValue>; // {ok} | {error: RpcError}
#[wasm_bindgen] pub async fn call(body: Vec<u8>) -> Result<String, JsValue>;                     // CoreResponse | RpcError JSON
#[wasm_bindgen] pub async fn stream(body: Vec<u8>, on_event: js_sys::Function) -> Result<u32, JsValue>;
#[wasm_bindgen] pub fn events(on_event: js_sys::Function) -> u32;   #[wasm_bindgen] pub fn unsubscribe(id: u32);
#[wasm_bindgen] pub fn snapshot() -> Result<Vec<u8>, JsValue>;      #[wasm_bindgen] pub fn commits() -> f64;
```

### TypeScript

```ts
// src/lib/core/browser/ (demo only)
export class BrowserCoreClient implements CoreClient { /* call, stream, events, onResubscribed, onEventsUnavailable */ }
export const browserCoreTransport: CoreTransport;          // RustStorageClient's third transport
export function openBrowserCore(): Promise<void>;          // delete the old keys, instantiate, bridge, image (IndexedDB snapshot or none), open
export interface DuckDbBridge { connect(): Promise<number>; runQuery(c: number, sql: string): Promise<Uint8Array>;
  startPending(c: number, sql: string): Promise<void>; fetchChunk(c: number): Promise<Uint8Array | null>;
  cancel(c: number): Promise<boolean>; close(c: number): Promise<void>; }
```

New codes: `CORE_RESTARTED` (Decision 16), `STORAGE_UNAVAILABLE` (IndexedDB can't be used; a notice, not an error the user acts on). Both are demo only.

---

## Parity

The fixtures and replays from phases 2–5 pin every rule the demo will get. What's new is the browser storage executor, the browser driver, the transport, and the demo's own start. The rule: **no twin is deleted until the replay that runs through it today passes through the real module, with its twin-only exemptions removed.**

### Recorded first (Task 1)

- **`crates/seaquel-browser/tests/fixtures/demo-baseline/`**: a Playwright recorder (its script kept in `docs/plans/artifacts/`) runs today's demo build in Chromium and stores what a visitor sees, normalised:
  - a cold load: connections, projects, dashboards, tabs, the sidebar's history and saved queries;
  - a reload;
  - a saved query, then a reload;
  - a run with `{{param}}`, an error and a destructive statement;
  - a grid edit with pending changes on and off;
  - a data-tab filter;
  - the extensions list.

  Each expected difference after phase 8 goes in `changes.json` with its decision: bugs 1, 2 and 7, Decision 9's bound filters, Q6 and Q7 (the assistant hidden). The old file isn't recorded, since nothing reads it (Q2 C). Task 1's bug fixes land first, so the baseline records the fixed reload.

### Replays moved onto the module (Tasks 5–6)

| Replay today, through the twins | After |
|---|---|
| `state-replay.svelte.test.ts` (112 cases, 377 steps; `EXEMPT` 2, `keyless()`) | the same file through `BrowserCoreClient` over the module in vitest (Node). `EXEMPT` and `keyless()` go; secret steps stay out (no store in the browser, as on web) |
| `library-replay.svelte.test.ts` (133 steps; `SKIPPED` 2, `EXEMPT` 2) | the same, through the module; an entry left in `SKIPPED` or `EXEMPT` needs a reason that isn't the twin (`add/keychain-failure#0` is a keychain case and stays) |
| `ts-library.test.ts` | deleted: Core's own replay covers the library rules, and `library-replay` covers the seam |
| `client.test.ts`, the sql.js half (the storage repo fixtures) | `RustStorageClient` over `browserCoreTransport`, the same expectations as its Tauri and HTTP halves |
| `dashboard-review`, `library-persistence`, `window-state`, `settings-across-tabs` (twins as a stand-in database) | the same tests over the module |
| `ts-runner-fixtures.test.ts` (95 run cases, `TS_ONLY` 1) | deleted with `TsQueryRunner`: Core's `run.rs` replay covers them; the module's run path is checked live by the engine suite |
| `ts-service-duckdb.svelte.test.ts` (DuckDB edit cases) | `engine-duckdb-browser.test.ts`: `plan-duckdb`, the DuckDB `apply` cases and `table-page-duckdb` through the module against real DuckDB-WASM (Node build), **including the attached-catalog page the twin skipped** |
| `ts-runner-duckdb.test.ts`, `duckdb-read-only.test.ts` | the same cases through Core's `db.run`, `db.queryStream` (`read_only`, `max_rows`) and the read-only path, live |

### New checks

- **Storage executor parity.**
  - Every module-made file opens natively with nothing pending and the same rows.
  - `seaquel-storage`'s wrapper has `wasm-bindgen-test` unit tests under Node: binds of every column type, NULLs, UTF-8 and a lone surrogate (5d-2's CESU-8 case), errors and their codes, `ROLLBACK` after `SQLITE_FULL`-like failures, serialize and deserialize.
- **Values.** The native crate's typed-cell cases (`seaquel-engine-duckdb/tests/common`) run against DuckDB-WASM through the module, compared with the native expectation. S5's two differences are listed by name, and any other difference fails.
- **Binds** (Decision 9): one round trip per value kind, a `?` inside a string, a comment and a quoted name, and a value holding `'`, `\` and a NUL.

### `changes.json`

`crates/seaquel-browser/tests/fixtures/demo-baseline/changes.json` lists each intended difference from the baseline with its decision. A difference the probe finds that isn't listed is a finding, not an entry to add.

---

## Ground rules

5e's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules;
- parallel agents own their files and make small, re-read edits to shared ones;
- tests never touch the real keychain, data dir, `~/.ssh` or home;
- no secrets, names, hosts, strings, SQL or values in `Debug`, errors, logs or events;
- the full check list, with npm through mise;
- one shared `CARGO_TARGET_DIR` (`/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`);
- effort log: `docs/plans/2026-10-06-phase-8-effort.md`.

Added for phase 8:
- **Never run the website repo's scripts.** The checkpoint reproduces `demo:update`'s copy into a scratch directory and serves it under `/demo`.
- **A wasm-capable clang** for any task that builds the module (Q9). Agents on this machine use LLVM 21 from the nix store or Homebrew's `llvm`, through `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown`.
- **Browsers:** Playwright 1.63 from the npx cache (`~/.npm/_npx/705bc6b22212b352/node_modules/playwright`), with Chromium, Firefox and WebKit installed. The probe also uses one headed run.

### Constraints the executors must obey

- **Native storage behaviour doesn't change.** The facade's native side re-exports sqlx. Any diff in the storage fixtures, Core's replays or the live run is a bug in the port.
- **No `tokio`, threads, `Instant`, `SystemTime` or file system** in anything the module links. clippy's wasm32 line enforces the first four; the review checks the fifth.
- **The module never calls back into itself synchronously**, and TypeScript never calls an export from inside a callback the module is running (Decision 15).
- **No twin is deleted before its replay passes on the module** (Parity).
- **Desktop and web don't bundle the module, the bridge or the transport.** Task 6 checks the built output for the module's file name.

### Things a task could quietly skip

Reviews check each by name:
- a storage call site still naming `sqlx::` directly, so the wasm32 build breaks only later;
- `_sqlx_migrations` rows written differently in the browser (checksum, description), so a file stops being portable;
- a write that commits without moving the commit counter, so it's never snapshotted;
- the snapshot taken before the commit it should include, or two snapshot writes racing so an older one lands last;
- the old keys left in `localStorage` (`seaquel_db` and `seaquel_db.*`), or another `seaquel-*` key removed with them;
- a stream that isn't cancelled on DuckDB when it's dropped (the `cancel` call missing on one path: page, table page, read-only);
- a bound value written as a literal without the DuckDB scanner's string, comment and quoted-name rules;
- `decode.rs` forked for the browser instead of made generic;
- a re-entrant call from an event handler into the module;
- trap recovery that reopens a fresh database instead of the last snapshot, or forgets to mark the connection disconnected;
- the module or the transport reaching the desktop or web bundle;
- a twin-backed test deleted rather than moved;
- an exemption kept in a replay after the module passes without it;
- (GUI) a seam still choosing the twin for the demo, or `isDemo()` gating something the Core path now serves.

---

## Order and estimates

Sized from logged first passes of the nearest tasks in 5d and 5e (effort logs; design doc "Phase 5d cost" and "Phase 5e cost"). Review fixes are budgeted at about 70% of first passes (5d: 64–70%, 5e: 72%), and probe fixes at 2–3.5 h (5d: 3.9 h and 3.4 h, 5e: 3.5 h). Builds and browser runs are inside each row. From 5e's Task 4 on they were about half of the wall time, and this phase adds a second target and three browsers.

| # | Task | First pass | Nearest logged task (first pass) | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | Reload fixes (dashboard toast, extra tab), baseline recorder | 0.4–0.6 h | 5e T1 (0.55 h), 5d-2 T1 (0.25 h) | — | 2, 4 |
| 2 | Storage: `db` facade, the 319 sites, the wasm32 executor, migrator rows, in-memory open, snapshot | 2.5–3.5 h | phase 3 T4, the typed port (1.4 h + 0.6 h fixes), 5e T3 (0.9 h); the wrapper is new | — | 1, 4 |
| 3 | Core: `browser` with `storage`/`workspace`, the write turn, `ensure_demo_connection`, CI line | 1–1.5 h | 5d-2 T4 (1.5 h); S3 showed Core needs little | 2 | 4 |
| 4 | Engine: `native`/`browser`, decoder over arrow-rs, the browser driver, binds as literals, streams and cancel | 2.5–3.5 h | 5c T3 (0.8 h), AI safety T4 DuckDB (0.8 h); phase 2's DuckDB port was 10.6 h, most of it the decoder reused here | — | 1, 2, 3 |
| 5 | `seaquel-browser`, `build-wasm.mjs`, the transport, snapshot store, old keys deleted, trap recovery, the Node harness and the live engine suite | 2.2–3.1 h | 2b T7 (0.9 h + 0.8 h), 5a T4 (1.25 h) and T5 (2.5 h) | 3, 4 | — |
| | **Checkpoint 8a** (in Task 8's row) | | | | |
| 6 | GUI onto Core in the demo, replays moved, twins deleted, `DuckDBProvider` trimmed | 2–3 h | 5d-1 T6 (1.9 h), 5e T7 (1.1 h) | 5 | — |
| 7 | Probe | 0.6–1 h | 5e T8 (0.65 h), 5d-1 T7 (0.65 h) | 6 | — |
| 8 | Docs, measurement, both checkpoints, the `demo:update` dry run | 0.8–1.3 h wall | 5d-2 T8 (2.25 h wall, 0.6 h work), 5e T9 (0.4 h) | all | — |
| | **First passes** | **12–17.5 h** | 5e: 14.35 h | | |
| | Review fixes (~70% of Tasks 1–7) | 7.8–11.3 h | 5e: 10.1 h | | |
| | Probe fixes | 2–3.5 h | 5e: 3.5 h | | |
| | Owner answers (all in; Q2 C needs no old-file work) | 0–0.2 h | 5e: 0.2 h | | |
| | **Total** | **~21.8–32.5 h** | 5e: 28.2 h | | |

**Expect about 27.5 h:** 8a about 18.5 h, 8b about 9 h. Re-checked after the answers (2026-10-02): eight answers are the options the rows were sized for. Q2 C drops the old-file import and its tests (about 0.5 h with their review share) and adds only the key removal. Q7 A is one flag in Task 6.

The riskiest parts:
- **Task 2:** a wide mechanical port under shipped desktop and web code, and the first `unsafe` FFI in storage. Expect the review to find error-mapping and lifetime bugs in the wrapper and a call site whose sqlx behaviour the facade doesn't copy.
- **Task 4:** DuckDB 1.4 against 1.5 (introspection SQL, EXPLAIN JSON), the IPC chunk API, and literal binds. The live suite is the only real check.
- **Task 5:** the trap, re-entrancy and snapshot ordering. They only show in a browser.
- **The probe:** the first time three browsers, a long session and a large metadata file meet the module.

Cut if time runs short:
- trap recovery beyond "reload the page" (Decision 16 becomes a follow-up, with a message instead);
- streaming in chunks (Decision 10 falls back to one batch and `cancelSent` for unbound statements).

The answers to Q1–Q9 are scope, not cuts.

---

## Task 1: Reload fixes and the baseline

Two bugs every visitor of today's demo sees on reload (S7, seen in all three browsers), fixed first so they can ship as a patch:
- **Bug 1, the error toast.** Each reload shows "Couldn't save the dashboard: There's already a dashboard called "E-Commerce Overview" in this project.", because `createDemoDashboard` (`src/lib/demo/sample-dashboard.ts:236-259`) creates the sample dashboard on every load.
- **Bug 2, the extra tab.** Each reload opens another query tab ("Query 2" after one reload).

**Files:** `src/lib/demo/sample-dashboard.ts` (create only when `dashboardsList` has no dashboard with that name; open the existing one otherwise); the code that opens the extra tab, once traced (likely the demo start in `routes/(app)/+layout.svelte:113-134` or the view-state restore in `TsUi`); the recorder in `docs/plans/artifacts/2026-10-06-record-demo-baseline.mjs.txt`, run against a scratch build; `crates/seaquel-browser/tests/fixtures/demo-baseline/{README.md,baseline.json,changes.json}`.

**Tests first** (they fail today):
- `the sample dashboard isn't created twice` (a reload with the dashboard stored: no create, no toast);
- `a reload restores the tabs it had and opens no new one`.
- The recorder runs twice and gives byte-identical output after normalising ids, times and the DuckDB connection id.

**Run:** `mise exec -- npx vitest run src/lib/demo src/lib/hooks/database`; `npm run check` 0/0; svelte-autofixer on changed `.svelte` files; the recorder against `build:demo` in a scratch copy, with both fixes in.

**Review:**
- Each step of "Recorded first" has a case, the reload steps included.
- After a reload: no error toast, one sample dashboard, the same tabs.
- The README names the build commit, the browser version, the normalising rules and what can't be recorded (DuckDB timings, toasts' order).

**Things this task could quietly skip:**
- bug 2's cause (a fix that hides the tab without finding why it opens);
- the sample dashboard renamed by the visitor: a new one is then created once, which is acceptable, but the test should say so;
- a step after a reload.

### Notes from Task 1 (as built)

- **Bug 2's cause.** `ConnectionManager.addDemoConnection` ended with an unconditional `this.onCreateInitialTab()` (`connection-manager.svelte.ts`, then line 1005), which is `queryTabs.add()` plus the query view. The demo's start awaits `db.whenReady()` first, and `whenReady` waits for `projects.initialize()`, which has already restored the project's tabs from the window's view state. So each load added one more query tab after the restored ones and made it active ("Query 2", then "Query 3"). Seen in the built demo: the restored tabs are on screen for about 0.5 s before "Query 2" appears. The fix (now lines 1004–1010) opens a tab only when the project has no query tab, which is what `reconnect` already did (`:752-757`). A first load, and a visitor who closed every query tab, still get "Query 1".
- **Bug 1.** `createDemoDashboard` (`sample-dashboard.ts:265-299`, `sameName` at `:238`) reads the project's dashboards with `getLibrary().listDashboards` and looks for `DEMO_DASHBOARD_NAME` (exact name). Found: it only runs the widgets again (their rows aren't stored, and DuckDB was seeded again) and **opens no tab**, so the restored tabs, including which one is active, stay as the visitor left them. The plan said "open the existing one"; that would have re-opened a dashboard tab the visitor had closed and moved the active tab, the same class of bug as bug 2. Not found (first load, or renamed): created, widgets added, tab opened, as before. Names compare trimmed and lowercased, which is Core's `name_key` for this ASCII name, so a sample renamed only in case or spacing (" e-commerce overview ") is found rather than created and refused. A sample renamed to another name gets a new one once; the tests say both. It works in the active project, so a reload with another project active creates one sample there, and a failed `listDashboards` ends in the demo start's generic "Failed to initialize demo database" toast (both in the docstring). It takes `db.state.activeProjectId`, so the layout's call is unchanged.
- **Tests (seen failing first):** `src/lib/demo/sample-dashboard.svelte.test.ts` "isn't created twice" failed on the toast (`expected [ "Couldn't save the dashboard: …" ] to deeply equal []`); `src/lib/hooks/database/connection-manager-demo.svelte.test.ts` "a reload restores the tabs it had and opens no new one" failed with `[ 'Query 1', 'Query 2' ]`. Review fix: "renamed only in case or spacing" failed on the toast with the exact comparison, then passed. The other three cases in those files pass before and after (first load, renamed sample, first load's one tab). Both files run on the twins (`TsLibrary` over sql.js), like `dashboard-review`; Task 6 moves them with the rest.
- **The baseline** is `crates/seaquel-browser/tests/fixtures/demo-baseline/` (`baseline.json` 13 steps; `README.md`; `changes.json`, 7 entries). The directory is only fixtures until Task 5 adds the crate; `crates:check` and Cargo ignore it. Recorded with Playwright 1.63's Chromium (153.0.8010.12) from the npx cache, against `build:demo` of 8d73887 plus both fixes, built in `scratchpad/p8-t1/app` (an rsync of the tree with `node_modules` linked, `SEAQUEL_WASM_PREBUILT=1`). Steps: cold load, reload, saved query, reload, a `{{param}}` run, an error, `DELETE … WHERE id = 1` (not destructive by design, so not prompted; queued, applied, counted), a grid edit with pending changes on and off, a data-tab filter, the extensions list, a final reload, and last a destructive `DELETE FROM demo.order_items` with pending changes on again: the editor's prompt, then the sheet's apply confirmation listing it. Two runs of the final recorder were byte-identical (`cmp`), as runs of each earlier revision were; adding the last step left the first twelve byte for byte unchanged. A run against the unfixed build differs only in the reload steps' tabs and toasts.
- **Found while recording** (not fixed, older than phase 8):
  - A run whose only statement is deferred shows nothing new: the result pane keeps the previous run's result.
  - Today's demo loses its query history on a reload: `addDemoConnection` resets the connection's history to `[]` (`initializeConnectionMaps`) and never loads it, so `final reload` shows `History 0`.
  - In headless Chromium the first click on "Pending Changes" after a grid edit doesn't open the sheet; the second does. The recorder clicks until it opens.
  - `features.aiAssistant` is read nowhere, so Task 6's Q7 flag has to be wired, not just flipped. `changes.json` says so.
  - Page errors over the run: bug 8's `parentNode` error and Monaco's `Missing requestHandler or method: doCompletionWithEntities`.

## Task 2: Storage, one set of queries and two executors

**Files:**
- `crates/seaquel-storage/src/db.rs` (new: the facade; `sqlx_impl` re-exports, `wasm_impl` over `sqlite-wasm-rs`);
- every `queries/*.rs`, `codec.rs`, `open.rs`, `write.rs`, `data_steps.rs` and `lib.rs`: paths through `db`, the write turn and the timeout per Decision 5, `in_memory`, `snapshot` and `commits`, and a migrator for wasm32 that writes sqlx's rows;
- `Cargo.toml`: `sqlite-wasm-rs` and `async-lock` for wasm32, and tokio without `net`.

**Tests first:**
- `the_native_facade_changes_nothing`: the whole existing storage suite, the frozen fixtures and `tests/baseline.rs`, unchanged.
- In wasm32 under Node (`wasm-bindgen-test`):
  - `binds_and_reads_every_column_kind`;
  - `a_lone_surrogate_reads_as_5d2_does`;
  - `errors_keep_sqlite_code_and_message`;
  - `rollback_after_a_failed_statement_leaves_the_connection_usable`;
  - `serialize_then_deserialize_round_trips`;
  - `migrations_record_sqlx_checksums`;
  - `the_commit_counter_moves_on_every_commit_and_only_then`.
- Native: `a_file_made_in_wasm_opens_with_nothing_pending` (the wasm test writes its snapshot to a fixture during the run; a native test opens it).

**Run:** `cargo test -p seaquel-storage`; `cargo clippy --target wasm32-unknown-unknown -p seaquel-storage -- -D warnings`; the wasm tests with `wasm-bindgen-test-runner` under Node; CI clippy.

**Review:**
- No `sqlx::` outside `db.rs`'s native half.
- The wrapper's `unsafe` (null checks, finalize on every path, `SQLITE_TRANSIENT` for text and blobs, freeing serialized memory with `sqlite3_free`).
- Error codes match sqlx's where storage reads them.
- The native diff is paths only.

**Things this task could quietly skip:**
- `query_as` tuple decoding of `Option` columns;
- `fetch_optional` on a statement that returns more than one row;
- a `RawValue` JSON column read byte for byte (5d's rule) on the wasm side;
- `PRAGMA foreign_keys = ON` at open;
- `secure_delete` and `VACUUM` (the string-secrets upgrade calls them) working on a memory database.

### Notes from Task 2 (as built)

2026-10-02, ~2.3 h wall (about 23:55–02:15), roughly a third of it builds and three native/wasm test runs on the shared target.

- **The facade.** `crates/seaquel-storage/src/db/mod.rs` picks the executor by `target_arch`. Natively, `db/native.rs` re-exports sqlx under the names the crate already used (`query`, `query_as`, `query_scalar`, `Row`, `SqliteConnection`, `SqlitePool`, `SqliteRow`, `Error`, `SqliteExecutor`, `MigrateError`, `Migrator`, the connect options) plus three aliases (`PoolConnection`, `Transaction`, `SqliteQuery`), `embedded_migrator()` (the `sqlx::migrate!` call) and `cell()` (the old `codec::is_one` body moved, for the flag codecs). Every `queries/*.rs`, `schema.rs`, `data_steps.rs`, `lib.rs`, `error.rs`, `write.rs` and `open.rs` change is the path (`sqlx::` → `db::`) and rustfmt's reflow; nothing outside `db/native.rs` names sqlx. Two call sites changed shape without changing behaviour: `window_state`'s `query_scalar::<_, Option<i64>>` turbofish is a type annotation (the in-memory `query_scalar` has one type parameter), and the flag codecs read through `db::cell`. `StorageError::Sqlx`/`Migrate` keep their names and hold `db::Error`/`db::MigrateError` (sqlx's natively; re-exported as `seaquel_storage::{DbError, MigrateError}`).
- **The in-memory executor** (`db/mem/mod.rs`, ~1,100 lines with `ffi.rs`): sqlx's API shape over one connection. It copies sqlx 0.8.6 where storage can see it, read from sqlx-sqlite's source: trimmed text, statements prepared one at a time after the previous ran; values consumed in order across statements, `?NNN`/`$NNN` by number, missing values NULL and extra ones ignored (`SqliteArguments::bind`, without its `expect` panic); `rows_affected` summing `sqlite3_changes` after each statement; `fetch_optional`/`fetch_one` stopping at the first row so later statements never run; `try_get` checking the cell's storage class with sqlx's `compatible` rules (`String` only from TEXT, `f64` only from REAL, integers and `bool` from INTEGER, bytes from BLOB or TEXT) and decoding with SQLite's own conversions on a `sqlite3_value_dup` copy; last duplicate column name wins; text that isn't UTF-8 is a decode error. Errors are `Error::Database(SqliteError { extended code, message })` with sqlx's `Display` (`error returned from database: (code: N) …`), `code()` as text and `kind()`, so `StorageError::code`'s `STORAGE_FULL` check (low byte 13), `classify`'s corrupt check (11, 26), the busy check (5) and `write.rs`'s "no transaction is active" match read it unchanged.
- **The pool** is the one connection behind an async mutex (`src/lock.rs`: `futures::lock::Mutex` on wasm32, tokio's natively): `acquire` holds it, the next caller waits, as on a sqlx pool of one; in the browser a wait past 30 s (sqlx's acquire timeout) is `PoolTimedOut`, racing `WasmExecutor::sleep`. A `PoolConnection` or `Transaction` dropped inside a transaction runs `ROLLBACK` synchronously. **This is stricter than the native pools of 2–4**: a read through `&storage` while the same task holds a `WriteTx` waits (then times out) instead of reading the other connection. CLAUDE.md already forbids it; Task 5's replays through the module will show any Core path that does it.
- **FFI** (`db/mem/ffi.rs`): the crate's only `unsafe`, 38 blocks, each with a `// SAFETY:` comment (clippy's `undocumented_unsafe_blocks` passes on wasm32). NULL checks on every pointer SQLite returns; statements finalized and values freed in `Drop`; `SQLITE_TRANSIENT` for text and blobs (an empty slice binds empty, not NULL); `sqlite3_serialize`'s buffer freed with `sqlite3_free` after the copy; `sqlite3_deserialize` with `FREEONCLOSE | RESIZEABLE` from `sqlite3_malloc64`; lengths past `int` are errors; a NUL in SQL ends the text instead of looping (sqlx 0.8.6 loops). `Db` is `Send` (so the pool's mutex is `Sync`) and not `Sync`; `Value` is neither. On wasm32 that rests on a single-threaded build (`-DSQLITE_THREADSAFE=0`, and a `compile_error!` on `atomics`). The same file is compiled natively for its tests over `libsqlite3-sys` 0.30.1 (a native dev-dependency, the version sqlx links), so `cargo test -p seaquel-storage --lib` runs the wrapper's six tests natively too.
- **The commit counter** is SQLite's commit hook, which only marks a commit under way; the statement that ends it counts it once it succeeded (and a finalize that commits a write stopped at a row). It counts every committed write transaction, including one that changed nothing (the open's baseline `BEGIN IMMEDIATE … COMMIT`), so it can run ahead of real changes, never behind. Task 5 should take `commits()` after `open` as its starting point.
- **The browser's open** (`src/open_mem.rs`, wasm32 only): `Storage::open(path, StorageOptions::in_memory(image))` keeps Core's call as it is; `path` only names the file in errors. A non-SQLite header is `Corrupt { untouched: true }`; a desktop file's WAL header (bytes 18–19 = 2) is rewritten to 1 before `deserialize`; a new file gets `page_size = 4096` (the wasm build's default is 8192); `PRAGMA foreign_keys = ON`; a probe read classifies SQLite's 26/11 as `STORAGE_CORRUPT`; then the baseline, the migrations and the data steps as natively. `snapshot()` is refused (busy, `STORAGE_ERROR`) while someone holds the connection or a transaction is open, so it never holds uncommitted rows. `debug_rows` is the wasm tests' plain-SQL read, compiled only with the crate's `test-hooks` feature (turned on by its own wasm32 dev-dependency on itself), so it isn't in the module. `StorageOptions` gained a wasm32-only `image: Option<Image>` (its `Debug` prints the length); `read_only`, `max_bytes` and pool sizes are ignored there.
- **The migrator.** `build.rs` now also writes `$OUT_DIR/migrations.rs` from `migrations/` with sqlx's naming rules; `src/migrations.rs` holds the list and the SHA-384 checksum, and `migrations_match_sqlx` checks it natively against `sqlx::migrate!`. `open_mem.rs` applies them as sqlx's `run_direct` and SQLite `apply` do: the same `CREATE TABLE _sqlx_migrations` text, the dirty check, `VersionMismatch`, each file in `SAVEPOINT _sqlx_savepoint_1` with its row (`TRUE`, `-1`), then `execution_time` in nanoseconds from `WasmExecutor::monotonic`, all under one `BEGIN IMMEDIATE` taken only when something is pending; unknown applied versions are ignored.
- **The write turn** (Decision 5): `Storage::turn` (`write.rs`) is `tokio::time::timeout` natively, unchanged, and a race with `WasmExecutor::sleep` on wasm32 (storage uses the executor type directly rather than one passed in by Core; Task 3 can plumb Core's if its test needs to). A dropped `WriteTx` rolls back synchronously on wasm32; native keeps `Handle::spawn`. tokio is a native-only dependency (without the workspace's `net`), and the wasm32 build has none: `cargo tree --target wasm32-unknown-unknown -p seaquel-storage -e normal` lists no tokio. `dirs`, `sqlx` and the `data_dir` module are native-only; `sha2` is a wasm32 dependency and a native dev-dependency.
- **Fixtures.** `tests/fixtures/sqljs/` holds S6's two sql.js files (sample content only; the live demo's with the WAL header the spike's native open left), and `tests/fixtures/wasm-made/meta.db` the file `a_snapshot_reopens_with_nothing_pending` wrote with `SEAQUEL_RECORD_WASM_FIXTURE=1` (Node's `fs` through `process.getBuiltinModule`). `tests/wasm_made.rs` checks every recorded migration's description and checksum against sqlx's, opens the file read-only (nothing pending), then writable. A migration or data step added later makes the read-only open report it; the test then says to regenerate the fixture instead of failing, since the checksums it recorded are still checked. Both READMEs say how.
- **Tests seen failing first.** `migrations_match_sqlx` (no generated list) and `a_file_made_in_wasm_opens_with_nothing_pending` (no fixture) failed before their code. The six wrapper tests and the seven `tests/wasm.rs` tests were written before the executor and the open, but their first run was a compile failure on test plumbing, not on behaviour, and the next run passed except `a_snapshot_reopens_with_nothing_pending`, which found that the open's empty baseline transaction counts as a commit (the assertion now compares the reopened file byte for byte instead). To show the wrapper tests bite, mutations were run: making `f64` accept INTEGER or stopping `fetch_optional` from stopping fails `binds_and_reads_every_column_kind`. Counting a commit whose statement failed is not caught: SQLite checks deferred foreign keys before the hook, so no test can make a hooked commit fail; the guard stays.
- **Review fixes** (2026-10-02, ~0.4 h). The plan's "no tokio in the module" holds: the mutex moved to `futures::lock` on wasm32 behind `src/lock.rs`. The S6 file's `-shm`/`-wal` side files (left by a native read in place) are gone, `tests/fixtures/.gitignore` keeps them out, and the README says to copy a fixture to a temp dir before opening it natively. The nine native-only test files carry `#![cfg(not(target_arch = "wasm32"))]`, so `cargo test` and `cargo clippy --target wasm32-unknown-unknown -p seaquel-storage --all-targets` both work. A new case in `the_commit_counter_moves_on_every_commit_and_only_then` (an autocommit `INSERT … RETURNING` stopped at its first row) failed with the finalize's `settle` removed and passes with it. The VACUUM test checks `freelist_count` goes to 0 instead of an assertion that couldn't fail. `tests/wasm_made.rs` fails on a stale fixture when `CI` or `SEAQUEL_STRICT_FIXTURES` is set. FFI comments: the hook's pointer is valid until `sqlite3_close`; `Sync` on `Db` and both impls on `Value` are dropped.
- **For Task 3.** Core's browser build can call `Storage::open` as it does; `storage.snapshot()`/`commits()` are wasm32-only. `upgrade.rs`'s `std::fs::metadata` for the VACUUM size check needs a wasm32 path (VACUUM, `secure_delete` and `checkpoint` all work in memory: `secure_delete_vacuum_and_checkpoint_work_in_memory`). CI's wasm32 line for storage (Decision 14) needs `CC_wasm32_unknown_unknown` only where a runner's default clang lacks the wasm32 backend (Ubuntu's has it); it isn't in `ci.yml` yet.
- **For Task 5.** The wasm tests run with `cargo test --target wasm32-unknown-unknown -p seaquel-storage --all-targets` and `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner` (0.2.128, from mise); on this machine clang is `/nix/store/cbsa0j0sqa44ls1wr2bgvbcx87k50j8h-clang-21.1.8/bin/clang` with `llvm-ar` from `/nix/store/329vgmwkd4vp36lfih4l0l6z6n43q3fr-llvm-21.1.8/bin/` (Homebrew's `llvm` isn't installed). A snapshot taken while a call holds the connection fails; coalesce and retry after the call, as Decision 6 already plans. The pool's 30 s acquire timeout and the write turn's 30 s `WRITE_WAIT` both use the page's `setTimeout`.

## Task 3: Core in the browser

**Files:**
- `crates/seaquel-core/src/lib.rs` (the `compile_error!` list) and `workspace.rs` (the browser's `WorkspaceSpec` with an image; storage given the executor);
- `upgrade.rs` (the VACUUM size check without `std::fs` on wasm32);
- a new `demo.rs` (`ensure_demo_connection`, behind `browser`);
- `Cargo.toml`;
- `crates/seaquel-rpc/Cargo.toml` if needed;
- `.github/workflows/ci.yml` (Decision 14's line).

**Tests first** (native, with `browser`'s code paths compiled for tests where they're target-independent):
- `ensure_demo_connection_creates_once_and_keeps_labels`;
- `ensure_demo_connection_on_a_file_that_has_the_row`;
- `the_write_turn_times_out_through_the_executor`.

**Run:** `cargo test -p seaquel-core --features seaquel-runtime/tokio`; the wasm32 clippy line; `cargo test -p seaquel-rpc`.

**Review:** `browser` still refuses every engine and the infrastructure; no Core path the module links reads the file system or a clock outside the executor; the MCP and server tests are unchanged.

**Things this task could quietly skip:** `refill_name_keys` and `refill_list_meta` on the browser open; the string-secrets upgrade on a workspace with no store (it must behave as web does); the `Outcome` warning S3 found.

### Notes from Task 3 (as built)

2026-10-02, ~0.7 h wall, about a third of it builds on the shared target.

- **`browser` admits `storage` and `workspace`** (`crates/seaquel-core/src/lib.rs:30-57`). It still refuses all five engine features, `engine-duckdb` included (that one is the native driver; Task 5's module registers `browser_engine(bridge)` itself through `CoreBuilder::engine`), plus `secrets`, `ssh`, `git`, both `license-*` features and, new to the list, `imports`. Each refused feature was checked by hand (`cargo check -p seaquel-core --no-default-features --features browser,storage,workspace,<f>`, 11 features, each a `compile_error!`). On wasm32 `open_workspace` allows `clippy::arc_with_non_send_sync` (`lib.rs:750`): nothing is `Send` in the page.
- **The write turn runs on Core's executor** (Decision 5). `Storage::with_executor` (`crates/seaquel-storage/src/open.rs:276`, a `Clock` field whose `Debug` prints `<executor>`) makes `turn` (`write.rs:251`) race the lock against the executor's `sleep` (`race`, `write.rs:286`; the lock is polled first, so a free lock wins over an ended wait). Without one it is tokio's `timeout` natively and `WasmExecutor` on wasm32, as Task 2 left it. `Workspace::open` takes Core's executor and hands it to storage (`workspace.rs:207-224`). Desktop, web and CLI now wait on `TokioExecutor::sleep` instead of `tokio::time::timeout`: the same timer and the same 30 s. `futures` moved from storage's wasm32 dependencies to its common ones.
- **The browser's spec:** `WorkspaceSpec::with_image(Option<Vec<u8>>)` (`workspace.rs:91`, `storage` on wasm32 only) sets `StorageOptions::in_memory(image)`. The data dir and file name then only name the file in errors.
- **`Workspace::ensure_demo_connection(core, origin)`** (`crates/seaquel-core/src/demo.rs:64`; `DEMO_CONNECTION_ID` re-exported) is compiled with `storage` and either `browser` or this crate's own tests. The plan's signature had no `origin`; it takes the page's like every library write, so the page skips its own event (Decision 17). In one `WriteTx` it does one of two things:
  - **The row is stored:** a patch of only `connected` through `update_connection_in`, so the visitor's labels, AI flags, name and project stay. One `connection` event.
  - **It isn't:** the row goes into `default-seaquel`, else the first project by rowid, else a default project made in the same transaction (then a `project` event before the `connection` event). The draft is the TypeScript twin's: "Demo Database", `duckdb`, `browser`, port 0, `demo`, no user, `prod`, connected. It is created through `insert_connection_in`, with Core's checks, `name_key` and `renameIfTaken`.

  It checks the `duckdb` engine is registered (`ENGINE_NOT_AVAILABLE` otherwise). It has no RPC method yet: Task 5/6 add the demo-only call in `seaquel-browser`.
- **`upgrade.rs`'s size check** (`file_bytes`, `:403`/`:423`): `std::fs::metadata` natively, unchanged; on wasm32 the length of `Storage::snapshot()`, 0 if it can't be taken. A browser file is made fresh and never has strings to strip, so this runs only in theory. The string-secrets upgrade on a store-less workspace already behaves as web does (`keychain_step` without `secrets` is `NoStore`: the row is stripped and listed). `refill_name_keys` and `refill_list_meta` run on the browser open, since `open_workspace` calls them under `storage`.
- **S3's `Outcome` warning:** `elapsed_ms` and `row_count` are only read when a run writes history, so they allow `dead_code` without `storage` (`run.rs:69-72`). `browser,workspace` without storage now passes clippy too.
- **One storage fix for the wasm32 Core line:** `Row::len` in the in-memory executor (`db/mem/mod.rs:452`) is read only by `debug_rows` (`test-hooks`) and the tests. Built as Core's dependency (no `--all-targets`), it was dead code and failed `-D warnings`. It is now allowed outside those builds.
- **CI** (`.github/workflows/ci.yml:56-74`), Decision 14:
  - Core and the RPC for wasm32 twice: the bare `browser` set, and `browser,storage,workspace` with `seaquel-rpc/storage,workspace`;
  - `-p seaquel-storage --all-targets`;
  - `-p seaquel-engine-duckdb --no-default-features --features seaquel-engine-duckdb/browser`, as Task 4 asked.

  The "Web server dependencies" step also bans `sqlite-wasm-rs`, `arrow-ipc` and `wasm-bindgen` (Decision 22). None is in `seaquel-server`'s tree today. The steps set no `CC_wasm32_unknown_unknown`, because Ubuntu's clang has the backend; a comment says what to set elsewhere. A `seaquel-browser` line waits for Task 5.
- **`cargo tree --target wasm32-unknown-unknown -p seaquel-core --no-default-features --features browser,storage,workspace -e normal`** lists 100 crates, with no `mio`, `ring`, `sqlx`, `libsqlite3-sys` or `socket2`. **It does list `tokio` and `tokio-util`.** `seaquel-engine`'s `CancellationToken` comes from `tokio-util`, which turns on tokio's `sync` feature only: no `rt`, `time` or `net`. This predates phase 8 (the bare `browser` build had it too) and builds for wasm32, but it breaks the letter of "no tokio in the module". Task 5 should measure what it costs in the module. Replacing it is a `seaquel-engine` change (a small token over `futures`/`event-listener`) that no task owns yet.
- **Tests seen failing first:**
  - `ensure_demo_connection_creates_once_and_keeps_labels`, `ensure_demo_connection_on_a_file_that_has_the_row` (S6's `demo-2026-10-01.db`, copied to a temp dir) and `ensure_demo_connection_goes_to_the_first_project_without_a_default_one` (added beside the two listed). These are unit tests in `demo.rs`, since `browser` can't be on in a native test build. All three failed against a stub that answered `NOT_SUPPORTED`.
  - `the_write_turn_times_out_through_the_executor` (`tests/write_turn.rs`) uses an executor whose `sleep` ends at once. It failed after 5 s ("the write turn waited on tokio's clock instead of the executor's") before the plumbing. Its first run after the plumbing also failed, on the test itself: a dropped `WriteTx` rolls back on a task, which an instant wait races. It now commits the held transaction.
- **For Task 5:**
  - Build Core with `ConnectPolicy::Unrestricted`, `WasmExecutor` and `.engine(browser_engine(bridge))`, then open `WorkspaceSpec::new("/demo").with_image(image)`.
  - Call `ensure_demo_connection(&core, &WriteOrigin::new(Some("demo")))` from the demo-only export, before `db.connect` saved.
  - The module's Cargo features are `seaquel-core` `browser,storage,workspace` (no defaults) and `seaquel-rpc` `storage,workspace`.

## Task 4: The DuckDB engine in the browser

**Files:**
- `crates/seaquel-engine-duckdb/Cargo.toml` (`native`, `browser`);
- `lib.rs`;
- `decode.rs` (generic over arrow-rs arrays, no `duckdb::` types in signatures);
- `browser/{mod.rs,bridge.rs,driver.rs,binds.rs,ipc.rs}` (new);
- `introspect.rs` (a shared helper that turns `query` results into the driver's introspection answers, used by both drivers);
- `scripts/check-crate-deps.mjs` if a new dependency needs listing.

**Tests first:**
- Native: the whole existing suite, unchanged; `binds_inline_every_value_kind` and `binds_skip_question_marks_in_strings_comments_and_quoted_names` (pure, so they run natively); `decode_from_ipc_matches_decode_from_duckdb` (native DuckDB writes IPC; both paths give the same cells).
- First, check in a browser test that `startPendingQuery`/`fetchQueryResults` give IPC stream chunks and that `cancelPendingQuery` stops a long query. If not, fall back to Decision 10's cut and record it.

**Run:** `cargo test -p seaquel-engine-duckdb`; `cargo clippy --target wasm32-unknown-unknown -p seaquel-engine-duckdb --no-default-features --features browser -- -D warnings`.

**Review:**
- `decode.rs` isn't forked.
- Every stream path cancels on drop.
- The read-only path rolls back and closes on every outcome.
- `restricted` is refused.
- The literal writer is `seaquel-sql`'s, not a new one.

**Things this task could quietly skip:**
- `max_bytes` in the browser's read-only path;
- an empty result keeping its column names (the IPC schema has them);
- the `TRANSACTION_OPEN` probe;
- `Count`-column `rows_affected` for `UPDATE` and `DELETE` alike;
- attached catalogs (`catalog.schema`) in introspection.

### Notes from Task 4 (as built)

- **Layout.** `crates/seaquel-engine-duckdb` has features `native` (default: `duckdb`, `tokio`, `blocking.rs`, `driver.rs`) and `browser` (`arrow-ipc`, `arrow-buffer`, `futures`, `js-sys`, `wasm-bindgen`, `wasm-bindgen-futures`). `src/browser/` holds `binds.rs` and `ipc.rs` (pure; compiled for `browser` and for native tests) and `bridge.rs` and `driver.rs` (only `browser` on wasm32). The crate exports `browser_engine(DuckDbBridge) -> Arc<dyn Engine>` (id `duckdb`, `DuckdbDialect`) and `DuckDbBridge` there; `engine()` and `DuckdbEngine` are unchanged behind `native`. `arrow-array` and `arrow-schema` 58.4 are direct dependencies of both builds, the same crates duckdb-rs re-exports, so the native driver hands the decoder the same types. Core's `engine-duckdb` feature keeps the default (native); `seaquel-browser` must depend on the crate with `default-features = false, features = ["browser"]` and register `browser_engine` itself.
- **`decode.rs` isn't forked.** It imports `arrow_array`/`arrow_schema` instead of `duckdb::arrow`. The walk over DuckDB logical types moved to `driver.rs` (`kind_of`, native only, unchanged). The browser gets `Kind::of_field(field, decimal38_is_hugeint)`, which reads Arrow extension metadata: `arrow.uuid`, `arrow.json`, `arrow.bool8`, `arrow.opaque` with a `type_name`, and `duckdb.*`. `decode_from_ipc_matches_decode_from_duckdb` writes native DuckDB 1.5's Arrow (with `arrow_lossless_conversion`, as the driver opens it) as IPC file and stream, reads it back through `browser/ipc.rs`, and compares against the native driver on all 119 typed-cell cases plus a 5,000-row result, an empty result and an ENUM. A hand-made check showed 46 of the 246 comparisons depend on the metadata.
- **The introspection calls** (`list_schemas`, `schema_tables`, `table_metadata`, `statistics`, `explain`) moved verbatim into `introspect::calls`, over `&dyn Driver`. Both drivers delegate to them.
- **The bridge (Decision 12, changed).** Its methods are `connect`, `runQuery`, `startPending` (header bytes, or `null` while the query runs), **`pollPending`** (new: the poll loop is Rust's, so dropping a call stops polling), `fetchChunk` (bytes; empty at the end; `null` for "not yet"), `cancel` and `close`. They are bound as plain functions returning a `Promise`, not `async` externs: an `async` extern only calls JavaScript when first polled, and the cancel, `ROLLBACK` and close a `Drop` sends must reach DuckDB-WASM's worker before the next call's statement. **Each bridge method must post its request before it returns its promise** (`AsyncDuckDB`'s methods do, unless OPFS file handling is configured). The reference bridge for Task 5's `duckdb-bridge.ts` is `scratchpad/p8t4/bridge.cjs`, 10 lines over `connectInternal`, `runQuery`, `startPendingQuery(c, sql, true)`, `pollPendingQuery`, `fetchQueryResults`, `cancelPendingQuery` (caught to `false`) and `disconnect`.
- **Every statement is a pending query**, with `allowStreamResult` on, so every call can be cancelled. `runQuery` is used for the `ROLLBACK` a `Drop` sends and for the ENUM re-read (below). In a multi-statement call, DuckDB-WASM runs every statement before the last inside `startPendingQuery`, in one worker step, so those can't be cancelled; only the last can. `CancelOnDrop` is armed before the statement is sent. The first live run found it armed only after the header arrived, so a drop during execution sent no cancel; the test that saw it checks the bridge's call log. Calls take turns on the connection through a `futures::lock::Mutex`. A stream releases its turn before it yields its final batch, so a consumer that stops at `is_final` without polling again doesn't block the connection (review: the concurrency test deadlocked on this).
- **IPC.** DuckDB-WASM's pending query sends the schema message alone, then one chunk per fetch, and **no end-of-stream marker**. `arrow-ipc`'s `StreamDecoder` holds a message with an empty body (the schema) back until more bytes arrive, so the first live run found every result without columns. `IpcStream::start` now reads the schema with `StreamReader`, and `finish` feeds an end marker when the decoder is mid-message. Pinned natively by `a_stream_cut_anywhere_without_its_end_marker_reads_whole` (7-byte chunks, no marker, an empty result). The file-format reader (`decode_batches`) is test-only.
- **Binds (Decision 9)** are `seaquel_sql::params::duckdb_bind_literal`, a new function next to the `{{param}}` writer. The forms:
  - `NULL`, `TRUE`, `FALSE`;
  - integers as digits, negatives as `(-5)`;
  - floats as `CAST('<Rust {:?}>' AS DOUBLE)`, with `'nan'`, `'inf'` and `'-inf'`;
  - plain decimals as `CAST('<normalized>' AS DECIMAL(w, s))`, with the native driver's width and scale. Over 38 digits, or text that isn't plain digits, they become a string, as the native text bind does;
  - text as a standard string with `'` doubled. A NUL becomes `chr(0)` between the parts;
  - bytes as `from_hex`;
  - JSON as `CAST('…' AS JSON)`;
  - arrays as list literals. The native driver refuses arrays.

  `browser::binds::inline_binds` finds placeholders only in code, between DuckDB's strings, comments and quoted names as `seaquel_sql::scan` reads them. It handles `?`, plus `?N` and `$N` (introspection binds `$1`/`$2`), with mixing and count checks. Each literal gets a space on both sides, so it can't merge with a string before it, make an `E'…'` string or start a `--`. **SQL holding a NUL is refused on every path**, the read-only path included: DuckDB-WASM passes SQL as a C string and silently ran `SELECT 1 AS x\0; SELECT 2` as its first statement. With no values, the SQL passes unchanged.
- **Review fixes:** `OwnConnection::finish` marks itself finished only after the close has gone out, so a drop during its ROLLBACK lets `Drop` send cancel, ROLLBACK and close. `close` sets `closed` right after the close is sent; `Bridge::close` sends at call time. `introspect::calls` exists only when a driver does, and the crate is a `compile_error!` with neither feature. Host clippy is clean for native, browser-only (lib) and both; the browser-only lib tests (21) pass on the host.
- **Driver behaviour**, as the native driver except where noted:
  - `execute` reads `rows_affected` from a lone `Count` column, which DuckDB returns for INSERT, UPDATE, DELETE and `CREATE TABLE … AS`. It reads other results to the end without decoding them. A SELECT whose single column is named `Count` would report its value, where the native driver reports 0.
  - `transaction`: binds checked first, then the `txid_current()` probe (`TRANSACTION_OPEN`), `BEGIN`, `expect_rows`, `COMMIT`, and `ROLLBACK` on failure. A `TransactionGuard` sends cancel and `ROLLBACK` if the call is dropped.
  - The read-only path and `explain_read_only` each open their own connection (`connect` is guarded, so a connection that arrives after a drop is closed). Each runs `BEGIN TRANSACTION READ ONLY`, then `SELECT * FROM query('<literal>') LIMIT <cap + 1>`, with `max_rows` and `max_bytes` checked while decoding. `ROLLBACK` and close always follow, or cancel, `ROLLBACK` and close when the call is dropped. DuckDB's `LINE 1: SELECT * FROM query(` pointer is cut from errors.
  - `restricted` is `NOT_SUPPORTED`. The connect path and `duckdb_config` are ignored. `arrow_lossless_conversion` isn't set: it is global, and the tutorial's arrow-js reader shares the instance.
- **DuckDB-WASM 1.4.3 against native 1.5.** The live run decodes every native typed-cell case through the browser driver. The suite pins these 14 cases as known differences, and any other difference fails:
  - JSON reads as Text (no `arrow.json` metadata), inside lists and STRUCTs too;
  - BIT reads as Bytes (DuckDB's padded form);
  - TIMETZ loses its offset (S5);
  - TIME_NS can't be exported (`Unsupported Arrow type TIME_NS`), also inside STRUCTs and UNIONs;
  - GEOMETRY doesn't exist without the spatial extension;
  - `-7::BIGINT = -7::BIGNUM` is false in 1.4.3, so a negative BIGNUM doesn't bind back;
  - **ENUM** (review fix): DuckDB-WASM's pending results carry no dictionary batch (`runQuery` files do; arrow-js reads the pending stream as nulls). When the header has a dictionary column and the statement is exactly one SELECT (`seaquel_sql::statements::query_type`, one statement by the splitter), the driver cancels the pending query and runs it again with `runQuery` (`ipc::read_file`). That re-read can't be cancelled, holds the whole result before the row and byte caps apply (as natively), and runs a SELECT's side effects (`nextval`) a second time. Anything else (`INSERT … RETURNING`, `WITH …`, DuckDB's `FROM t`, several statements) has already run, so its ENUM column fails with `UNSUPPORTED_TYPE` ("cast it to VARCHAR"), followed by Arrow's error text in parentheses.

  **HUGEINT matches**: 1.4.3 sends it as a bare `Decimal128(38, 0)` and ignores `arrow_lossless_conversion`, global or not, so the browser reads that carrier as HUGEINT. 39-digit values come out right; S5's "wrong" was arrow-js. The cost is that a real `DECIMAL(38, 0)` reads as an Int when it fits. UHUGEINT and BIGNUM arrive as `arrow.opaque` and decode.
- **Tests seen failing first:** `binds_inline_every_value_kind`, `binds_skip_question_marks_in_strings_comments_and_quoted_names`, `binds_never_join_their_neighbours`, `hostile_text_stays_inside_its_literal`, `counts_and_mixing_are_checked`, `nul_in_the_sql_is_refused` and `decode_from_ipc_matches_decode_from_duckdb`, all against stubs. The live suite's first runs failed on the schema hold-back, the late-armed cancel and the ENUM error before each fix. `a_stream_cut_anywhere_without_its_end_marker_reads_whole` was written after the fix it pins.
- **The live suite** (`scratchpad/p8t4/run.test.cjs`, `node --test`; harness crate `scratchpad/p8t4/harness`, built by `build.sh` with wasm-bindgen `--target nodejs`) runs the driver in wasm32 against DuckDB-WASM 1.32's Node build. It passes 35 of 35 tests:
  - the first check: pending queries give IPC stream chunks, and `cancelPendingQuery` stops `range(30000000000)` in about 100 ms;
  - values, binds (a round trip per kind, `?` in strings, comments and quoted names, hostile text, NUL);
  - empty results, 5,000-row batches, `RESULT_TOO_LARGE`, `rows_affected`;
  - cancel for each of query, execute, stream and read-only (by drop, from the bridge log), a stream's token, a half-read stream, and a dropped CTAS that leaves no table;
  - transactions: commit, index on failure, `NO_ROWS_AFFECTED`, `TRANSACTION_OPEN`, and a dropped transaction rolled back;
  - the read-only path (refusals, `max_rows`, `max_bytes`, rollback and close on every outcome);
  - EXPLAIN in its three forms; introspection with an attached catalog; `restricted`; close;
  - review additions: ENUM through query, stream and the read-only path (caps included), an ENUM from `INSERT … RETURNING`, a read-only call dropped during its ROLLBACK, a dropped connect, concurrent calls (queries, executes, read-only calls and three streams), a multi-statement execute, and 12 misbehaving bridges (missing methods, synchronous throws, a non-number or negative id, non-bytes, a rejection that isn't an `Error`, garbage and truncated IPC, a throwing cancel and close): errors every time, no trap.
- **For Task 3:** Decision 14's wasm32 line must include `-p seaquel-engine-duckdb --no-default-features --features seaquel-engine-duckdb/browser`, which passes clippy today. Core's `browser` must not enable `engine-duckdb` (that feature is the native driver). The module registers `browser_engine` itself.
- **For Task 5:**
  - The bridge contract above, with `pollPending` and `runQuery` (the ENUM re-read). Port `bridge.cjs` and `run.test.cjs` into `engine-duckdb-browser.test.ts` through the module, and add the DuckDB edit fixtures, which Task 4 didn't run (they need Core).
  - ENUM: a SELECT re-reads through `runQuery`; anything else shows the `UNSUPPORTED_TYPE` message.
  - Sizes weren't measured here: the browser build adds `arrow-ipc`, `flatbuffers` and the decoder.

## Task 5: The module, the transport and the Node harness

**Files:**
- `crates/seaquel-browser/{Cargo.toml,build.rs,src/lib.rs,src/log.rs}` (new; build.rs sets the 2 MiB stack);
- `scripts/build-wasm.mjs` (`--module browser`, clang detection, the budget, `wasm-opt` by brotli size);
- `package.json` scripts (Q9 A);
- `.gitignore` (`src/lib/wasm/browser-pkg/`);
- `src/lib/core/browser/{index.ts,client.ts,transport.ts,snapshot-store.ts,old-keys.ts,duckdb-bridge.ts}` (new);
- `src/lib/core/index.ts` and `storage/rust-client.ts` (the third transport, demo branch);
- `src/lib/core/browser/*.test.ts` and `src/lib/engine/engine-duckdb-browser.test.ts`.

**Tests first:**
- vitest in Node, loading the module from bytes with DuckDB-WASM's Node build:
  - `call_and_stream_round_trip`;
  - `events_are_delivered_after_the_call_returns` (no re-entrancy);
  - `aborting_a_stream_cancels_it_in_duckdb` (a long `range` sum stops);
  - `a_snapshot_is_stored_after_a_committing_call_and_not_otherwise`;
  - `the_newest_snapshot_wins_when_writes_overlap`;
  - `the_old_keys_are_deleted_and_never_read` (`seaquel_db` and a `seaquel_db.x` set before start are gone, other keys stay, and the metadata file starts empty);
  - `an_unreadable_snapshot_is_kept_aside`;
  - `indexeddb_unavailable_runs_in_memory_with_a_notice`;
  - `a_trap_reopens_from_the_last_snapshot` (`__test_trap`).
- The engine suite: values, binds, cancel, transactions, the read-only path, EXPLAIN, introspection, and the DuckDB edit fixtures (Parity).

**Run:** `node scripts/build-wasm.mjs --module browser`; `mise exec -- npx vitest run src/lib/core src/lib/engine src/lib/storage`; `npm run check` 0/0; `npm run build` and `npm run build:web` (neither contains `seaquel_browser`).

**Review:**
- Re-entrancy (Decision 15).
- Snapshot ordering (Decision 6).
- The bridge's errors become `DbError`s with DuckDB's message and no SQL in logs.
- The transport's `stream` ends with exactly one `done` or `error` on every path, `CANCELLED` included.
- The budget check really fails the build.

**Things this task could quietly skip:**
- `onResubscribed` after a trap;
- a `pagehide` flush while a snapshot write is in flight;
- the stamp covering the second module;
- `SEAQUEL_WASM_PREBUILT` for both modules;
- the web and desktop bundles checked for the module's file name.

### Notes from Task 5 (as built)

2026-10-02, ~0.85 h wall (about 01:25–02:15), roughly a third of it module builds, the full vitest run and the three app builds.

- **The crate.** `crates/seaquel-browser/` (`Cargo.toml`, `build.rs` with the 2 MiB stack, `src/lib.rs`, `src/module.rs`, `src/log.rs`, `src/test_hooks.rs`). Every dependency but `log` is wasm32-only, so natively the crate is the log formatter and its 3 tests. Core is built as Task 3 said: `ConnectPolicy::Unrestricted`, `WasmExecutor`, `.engine(browser_engine(bridge))`, `WorkspaceSpec::new("/demo").with_image(image)`; features `seaquel-core` `browser,storage,workspace` (no defaults), `seaquel-rpc` `storage,workspace`, `seaquel-engine-duckdb` `browser` (no defaults). Every write carries the `demo` origin.
- **Exports**: `open(bridge, image?, onTrap?) → commits`, `call(body) → CoreResponse JSON`, `stream(body, onEvent) → count`, `events(onEvent) → id`, `unsubscribe(id)`, `snapshot()`, `commits()` and `ensureDemoConnection()` (Decision 19, the demo-only call Task 3 left for here). A refusal rejects with the `RpcError`'s JSON **text**; anything else thrown is a trap. A second `open` closes the first Core (`close_all`) and replaces it. The plan's `{ok} | {error}` shape for `open` became "resolve with the commit counter or reject with the error", which is what the transport needs for its baseline.
- **Traps.** A panic inside an async export aborts in wasm-bindgen-futures' task queue: the export's promise never settles and the `RuntimeError` is uncaught. So the module installs a panic hook that calls the page's `onTrap` first (location only, no message, logged as `browser.trap`). The transport restarts on that, on any non-string throw from an export, and on a `RuntimeError` the window's `error`/`unhandledrejection` events carry (a stack overflow in an async task, which no hook sees). An instance generation makes sure one trap restarts once.
- **Re-instantiating this glue isn't the editor module's patch.** Its closures (promise callbacks, timers) call back through the glue's one `wasm` variable, and their destructors run on whatever `wasm` is at finalization, so after a swap a late DuckDB answer meant for the dead instance would run the new instance's code with the old one's pointers. `build-wasm.mjs`'s `patchBrowserGlue` stamps each closure with the generation it was made in; once `__seaquel_reinstantiate` moves the generation on, an old closure does nothing and its destructor never runs. The patch fails the build if the glue's shape changes (one match each, no `makeClosure`, no exported class).
- **Logs** (Decision 18): WARN and above to `console.warn`/`console.error`, formatted as the server's are (`log.rs`: 1 KiB messages, 128-byte values, logfmt quoting, control characters escaped).
- **`build-wasm.mjs`.** `--module editor` (the default, unchanged) and `--module browser`, plus `--test-hooks` for the variant vitest loads (`src/lib/wasm/browser-test-pkg/`, same `wasm-release` profile, no wasm-opt, no budget). Each module has its own pkg directory and stamp (the stamp also covers the module and its features). `SEAQUEL_WASM_PREBUILT=1` checks whichever module was asked for. clang: `CC_wasm32_unknown_unknown`, then Homebrew's llvm (`brew --prefix llvm`, `/opt/homebrew/opt/llvm`, `/usr/local/opt/llvm`), then a `clang` on PATH whose `--print-targets` lists wasm32; the archiver: `AR_wasm32_unknown_unknown`, then the `llvm-ar` beside that clang, `llvm-ar`, `llvm-ar-<major>`. Each failure names the fix (`brew install llvm`). It prints the clang it used. `wasm-opt` is kept only when it lowers the brotli size; the budget (`BROWSER_BUDGET_BYTES`, 2,000,000 bytes brotli 11) is checked before the pkg directory is replaced, so an oversized module never lands. `SEAQUEL_WASM_BUDGET_BYTES` overrides it, to see the check fail.
- **Sizes** (release, wasm-bindgen 0.2.128): cargo 7,078 KB; after wasm-bindgen 6,245 KB raw, 2,013 KB gzip -9, **1,473 KB brotli 11 (1,508,765 of 2,000,000 bytes)**; JS glue 26 KB. `wasm-opt -Oz` gives 1,531 KB brotli, so it's skipped. After the review fixes (serde_json's `float_roundtrip` parser): 6,262 KB raw, 2,023 KB gzip, **1,481 KB brotli (1,516,561 bytes)**. The budget check failed a build with 3 MB of random bytes linked in (4,501,494 bytes brotli, exit 1, pkg untouched) and with the budget lowered to 1,000,000. **tokio and tokio-util** (`sync` only, through `seaquel-engine`'s `CancellationToken`): 5.9 KB of the module's 6.19 MB of named function bodies (tokio 3.2 KB, tokio-util 2.7 KB, about 0.1%), measured on an unstripped build; LTO inlines some of it elsewhere, so call it under 10 KB raw and a couple of KB brotli.
- **Scripts.** `wasm:build:browser` and `wasm:build:browser-test`; `predev:demo` and `prebuild:demo` build the shipping module, `pretest` and `pretest:watch` the test one. Desktop and web scripts don't. `build:demo` builds the module but nothing imports it yet, so `build-demo` is as it was: none of `build`, `build-web` or `build-demo` holds a `seaquel_browser` file or reference.
- **The transport** (`src/lib/core/browser/`):
  - `transport.ts`, `BrowserCore`: calls and streams under the trap guard; events, stream items and a stream's end go through one mailbox and reach TypeScript in a later microtask, in order (Decision 15); the snapshot after each call or stream that moved the commit counter, one macrotask later, one save in flight, a refused (busy) snapshot retried when the next call ends, the counter read right before the snapshot; `flushNow` saves at once even with a save in flight (the store keeps call order, as IndexedDB does); the restart (every call in flight fails with `CORE_RESTARTED`, the dead instance's DuckDB connections closed through the bridge's `closeAll`, the newest saved snapshot reopened).
  - `client.ts`, `browserCoreClient(core)`: the `CoreClient`. It remembers the connection ids `db.connect` gave out; after a restart each gets a `connectionClosed` with code `CORE_RESTARTED`, then `onResubscribed({initial: false})`. `onEventsUnavailable` never fires.
  - `index.ts`, `openBrowserCore({bridge, module?, store?, indexedDB?, localStorage?, window?, document?})`: deletes the old keys, opens the store (or runs in memory with `STORAGE_UNAVAILABLE`), opens on the snapshot (one that's `STORAGE_CORRUPT` goes to `meta.db.unreadable` and Core starts empty, with that notice), listens for `pagehide`, `visibilitychange` (on `document`) and traps. `useBrowserCore(opened)` registers the client and the storage transport. The demo's own module loads behind `import.meta.env.VITE_BUILD_TARGET === "demo"`; `src/lib/wasm/browser-pkg.d.ts` types that import for `npm run check` on machines that never build it.
  - `snapshot-store.ts` (IndexedDB `seaquel-demo`/`files`/`meta.db`), `old-keys.ts` (`seaquel_db` and `seaquel_db.*`, removed unread on every start), `duckdb-bridge.ts` (Task 4's reference bridge, plus `closeAll` and a `note` hook for tests), `testing/node.ts` (loads the test module from its bytes, boots DuckDB-WASM's Node build).
  - `$lib/core`'s `getCoreClient` and `RustStorageClient`'s default transport take the browser ones only in a demo build and only once `useBrowserCore` set them. Neither file imports `src/lib/core/browser/`.
- **Crate rules.** `seaquel-browser` is a new `browser-interface` class in `check-crate-deps.mjs`: an interface's crates plus `seaquel-engine-duckdb` and no other engine (2 new cases in its test).
- **CI.** Workflow-wide `CC_wasm32_unknown_unknown: clang-18` and `AR_wasm32_unknown_unknown: llvm-ar-18` (ubuntu-latest has clang 16–18; `llvm-ar-18` comes with clang-18's llvm-18), so no wasm32 build falls back to GNU `ar`. A "browser module builds for wasm32" step clippies `seaquel-browser` with and without `test-hooks`. The frontend job runs `npm run wasm:build:browser-test` before `npx vitest run`.
- **Task 3's review items.** `demo.rs` has `two_concurrent_calls_store_one_row` (`tokio::join!` of two calls: one row, no rename; the events are a `project`, then a `connection` from each call, since the second call's `lastConnected` update is a write of its own; "one connection event" read literally would need the second call coalesced into the first, which nothing asks for) and `without_the_duckdb_engine_it_is_engine_not_available` (nothing written, no event). Both passed against the existing code; with `check_engine` commented out the second fails.
- **The engine suite** (`src/lib/engine/engine-duckdb-browser.test.ts`, 44 tests, ~7 s) runs Task 4's 35 through Core's RPC in the page: `db.query`, `db.execute`, `db.transaction`, `db.queryStream` (`readOnly`, `maxRows`, `maxBytes`), `db.engine`, `db.cancel`. What the wire can't reach goes through the test build's hooks: calls and streams dropped after a delay (as Core drops one), a second Core over a misbehaving or slow bridge (`__test_side_*`), a `restricted` connect and Core's read-only EXPLAIN. Two changes from Task 4's suite: `db.transaction`'s wire carries no failing index, so the index is checked through the apply fixtures' `failedAt`; and the typed-cell cases come from a new fixture, `crates/seaquel-engine-duckdb/tests/fixtures/cells.json`, written from `tests/common/cells.rs` by `tests/cells_fixture.rs` (which fails when it's stale; `SEAQUEL_RECORD_CELLS=1` rewrites it; scratch table names are numbered so it's stable). New: the 9 DuckDB edit cases live (3 plan, 4 apply, 2 table pages, the attached-catalog page the twin skipped included), each with its table set up in DuckDB-WASM and its rows checked after.
- **Found by the suite** (both pinned, neither fixed here):
  - **The JSON wire lost the last bit of some floats, on every interface.** FLOAT's maximum reads as `3.4028234663852886e38`; sent back as that JSON number, serde_json (without its `float_roundtrip` feature) parsed it one ULP high, so `? = 3.4028235e38::FLOAT` was false. The native suite never crosses JSON, so it never saw this. Fixed in the review round (below): `float_roundtrip` is on for the workspace.
  - DuckDB-WASM 1.4.3 prefixes a statement that fails inside a transaction with `Execute failed: ` (from DuckDB-WASM, not either driver). The apply case pins the rest of the message.
- **Tests seen failing first:**
  - `log.rs`'s 3 tests against a stub;
  - `browser-core.test.ts`'s 9 (`call_and_stream_round_trip`, `events_are_delivered_after_the_call_returns`, `aborting_a_stream_cancels_it_in_duckdb`, `a_snapshot_is_stored_after_a_committing_call_and_not_otherwise`, `the_newest_snapshot_wins_when_writes_overlap`, `the_old_keys_are_deleted_and_never_read`, `an_unreadable_snapshot_is_kept_aside`, `indexeddb_unavailable_runs_in_memory_with_a_notice`, `a_trap_reopens_from_the_last_snapshot`) against stubs, with the module and DuckDB loading in `beforeAll`;
  - the crate rule's new case; `the_cells_fixture_matches_the_cases` (no fixture);
  - the engine suite's first run: 4 of 44 (the FLOAT wire bit above; two of my own assumptions, a `NO_ROWS_AFFECTED` message the GUI words and a STRUCT column read as text; then the `Execute failed: ` prefix).
  - Mutations: with the mailbox delivering synchronously, `events_are_delivered_after_the_call_returns` fails (it first passed anyway, because the module calls `onEvent` from its task queue rather than inside an export; the test now also counts time inside the module's callbacks); with a second save allowed beside the first, `the_newest_snapshot_wins_when_writes_overlap` fails.
  - `snapshot-store.test.ts` (3, over a small IndexedDB fake) was written after the store and passed first time.
  - The first trap run showed the "sync" kind was itself an async export; `__test_trap` is now a plain export that panics before returning for `"sync"` and returns a promise that panics in a task for `"async"`. The async one's uncaught `RuntimeError` was at first filtered suite-wide in `vite.config.js`; the review round replaced that with a per-occurrence counter (below).
- **Runs:** `node scripts/build-wasm.mjs --module browser` (sizes above); `npx vitest run src/lib/core src/lib/engine src/lib/storage` 17 files, 489 tests; `CI=1 npx vitest run` 120 files, 2,061 tests; `npm run check` 0/0; oxlint clean; `npm run build`, `build:web`, `build:demo` pass with no `seaquel_browser` in any output; `cargo test -p seaquel-core --features seaquel-runtime/tokio` 378 tests; rustfmt; the workspace clippy and every wasm32 clippy line in `ci.yml`, the new `seaquel-browser` ones included; `npm run crates:check` 25 crates OK.
- **For Task 6:**
  - The demo's start: boot DuckDB-WASM as the tutorial does, `openBrowserCore({ bridge: makeDuckDbBridge(db) })`, `useBrowserCore(opened)`, then `opened.core.ensureDemoConnection()` and `db.connect` saved. Import `$lib/core/browser` only inside a `VITE_BUILD_TARGET === "demo"` branch (as `duckdb-bundles.ts` does), and grep the three outputs again: from then on the demo bundles the module, and desktop and web must still not.
  - `opened.notices` (`STORAGE_UNAVAILABLE`, `STORAGE_CORRUPT`) carry English text; showing them needs en.json keys through the translator.
  - A restart's `connectionClosed` (`CORE_RESTARTED`) reaches `listenForCoreEvents` once Decision 17 turns it on in the demo; the connection then reconnects like any closed one.
  - The replays can open Core the way `browser-core.test.ts` does (`loadTestModule`, `bootDuckDb`, `openBrowserCore` with a fake store, `localStorage: null`, `window: null`); each test file gets its own module instance.
  - The in-memory pool is a pool of one (Task 2): a Core path that reads `&storage` while holding a `WriteTx` waits 30 s, then fails. Nothing in this task's calls hit it; the state and library replays are the real test.
  - A `CORE_FAILED` start leaves the module's last instance trapped; in the page that is the end until a reload (the tests re-instantiate before reusing the module).

**Review fixes** (2026-10-02, ~0.5 h, about half of it module rebuilds, the live workspace run and the full vitest run):

- **A save in flight during a restart** (Important 1). `restart` now awaits every pending save (`Promise.allSettled`) before it picks the image. Each save carries the generation and a sequence number taken with its snapshot: only a save of the current generation moves `storedCommits`; `lastImage` is the image of the highest sequence that landed, whichever generation took it. Test `a_save_in_flight_during_a_restart_is_neither_lost_nor_counted_against_the_new_instance` (hold A's save, trap, release, write B, pagehide; a new Core on the store has A and B) failed first. Mutation: without the await it fails again. The generation tag alone doesn't change that test's outcome (with the await, no old save can land after the reopen), so it stays as a second guard.
- **A panic during `open`** (Important 2).
  - `openInstance` races `module.open` against the panic hook: during an open, `onTrap` fails that open instead of scheduling a restart. A non-string throw from `open` is a trap too.
  - `openRecovering` serves the first open and a restart's reopen alike. A trapped open on an image, or a `STORAGE_CORRUPT` one, moves the image aside with a `STORAGE_CORRUPT` notice (now raised by `BrowserCore.notices`) and retries on a new file. A trapped open with no image retries on a fresh instance.
  - Every fresh instance counts toward the cap, `MAX_RESTARTS` 3 per `RESTART_WINDOW_MS` (60 s). Past it Core is fatal: `BrowserCore.open` rejects, and every call fails with `CORE_FAILED`.
  - `trapped()` never rejects (`restart` catches and goes fatal), and `run` checks `fatal` after waiting for a restart, so nothing becomes an unhandled rejection.
  - The test build's `maybe_panic_on_open` panics inside the open's task for an image starting `SEAQUEL_TEST_PANIC`, or while `globalThis.__seaquelTestPanicOpens > 0` (counted down).
  - Tests: `a_panic_during_the_first_open_restarts_it_instead_of_hanging`, `a_snapshot_that_makes_core_panic_is_kept_aside_and_core_starts_empty`, and `past three restarts a minute every call fails with CORE_FAILED, never hangs` (a first start whose every open panics: 4 traps, then `CORE_FAILED`; a running Core whose reopen keeps panicking: 3 traps, then every call `CORE_FAILED`). Before the fix the first hung (20 s timeout), and its trapped instance timed out the tests after it.
- **The trap filter** (Important 3). `onUnhandledError` runs in vitest's main process, which can't see a counter the test worker sets, so the opt-in moved into the worker: `testing/node.ts` wraps `queueMicrotask` (what the glue schedules the module's task queue with) and swallows a `RuntimeError` only while `globalThis.__seaquelExpectedTraps` is above 0, counting it down. Each trap test sets it and asserts it's back to 0; `vite.config.js` is back as it was. Mutation: a test that traps with no counter set made the run exit 1 ("Unhandled Errors: RuntimeError: unreachable").
- **Minor.**
  - A snapshot IndexedDB can't read, or that isn't bytes, is left alone: Core runs in memory with a `STORAGE_UNAVAILABLE` notice and stores nothing over it (`a_snapshot_the_store_can_t_read_runs_in_memory_and_is_never_overwritten`).
  - `BROWSER_MODULE_EXPORTS` (each export with its arity) `satisfies Record<keyof BrowserModule, number>`, so `npm run check` catches a key added to or dropped from the interface; a test checks the built module against it.
  - `testModuleMissing` throws when the test package's `.stamp` is older than any source the module is built from (the crates it links, `Cargo.lock`, `build-wasm.mjs`). `build-wasm.mjs` now touches the stamp on an up-to-date run too, or an edit that leaves the module unchanged would read as stale forever. Seen: a touched source failed the suite until a rebuild.
  - `noticeTrap` restarts only when the stack names `seaquel_browser` (a test). A non-panic trap outside any call whose stack doesn't reach such a frame (V8 keeps 10 frames) goes unnoticed; the panic hook still covers every panic.
  - `BROWSER_REINIT_JS`'s comment says the dead instance's heap slots leak, a bounded amount per trap.
  - `caught()` logs the refusal's code only.
  - CLAUDE.md: wasm32 builds of `seaquel-storage`/`seaquel-browser` on macOS need the two env vars, or Homebrew's llvm.
- **`float_roundtrip`** (the coordinator's decision). The workspace's serde_json has it, and `src-tauri` now takes serde_json from the workspace. `seaquel-types`' `floats_cross_the_wire_bit_for_bit` (9 doubles, FLOAT's maximum first) failed first: "3.4028234663852886e38 came back as 3.402823466385289e38". The engine suite's FLOAT pin is gone and both cases pass. `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio --no-fail-fast`, with every live engine and the SSH server required (CI's `SEAQUEL_TEST_*` and `SEAQUEL_TEST_REQUIRE_ENGINES=1`): 2,011 passed, 0 failed, 3 ignored (the two keychain tests and an MCP doc test, ignored before too), 192 binaries. That covers every engine's parity, dialect, smoke and live suites and the run, edits, library, state and shared replays. No frozen fixture changed, and no recorded test changed its result. `cargo check -p seaquel` passes. The module grew 7.6 KB brotli.
- **Re-review follow-ups** (~0.2 h):
  - **A stuck save can't hold a restart.** `restart` races its wait for saves in flight against `SAVE_WAIT_MS` (3 s; `saveWaitMs` in tests). Past it, Core reopens on the newest snapshot that landed and starts saving again without waiting on the stuck one (`this.saving` cleared; a save landing after a newer one started no longer clears that one's `saving`). If the stuck save lands later, its counter is the dead instance's and moves nothing. `store.load()` at startup gets `LOAD_TIMEOUT_MS` (3 s; `loadTimeoutMs`); past it Core runs in memory with `STORAGE_UNAVAILABLE` and never writes over the stored snapshot. Tests `a_save_that_never_finishes_doesn_t_stop_a_restart` (5 saved writes, then A's save never answers; trap; calls work at once on P1–P5; B; A's save lands late; C; a new Core on the store has P1–P5, B and C) and `a_snapshot_read_that_never_answers_runs_in_memory_and_writes_nothing` both hung (15 s timeout) first. Mutation: dropping the generation guard on `storedCommits` fails the first. (Without the five earlier writes it didn't: the new instance's counter caught up with the old one's, so the late save never outranked it.) The `saving` identity check is defensive; no test sees it, since the store keeps call order either way.
  - **Test globals.** `browser-core.test.ts` has an `afterEach` that asserts `__seaquelExpectedTraps` and `__seaquelTestPanicOpens` are 0 or unset, then unsets both, so a failed test can't swallow a later test's real trap. No other file sets them.
  - **Set-aside behaviour.** A stray trap during a reopen sets the snapshot aside like a corrupt one, and a second set-aside replaces the earlier `meta.db.unreadable`; Decision 6 accepts this (only the newest unreadable copy is kept).
- **Runs after the fixes:** both modules built (release within budget); `CI=1 npx vitest run` 120 files, 2,068 tests; `npm run check` 0/0; oxlint clean; `build`, `build:web` and `build:demo` pass with no `seaquel_browser` in any output; Core 378; rustfmt, the workspace clippy and every wasm32 line clean; crate rules 25 OK.


### Checkpoint 8a

The full check list with one live run. The demo still runs on its twins, so `build:demo` must give Task 1's baseline unchanged. The owner can release Task 1 and the facade.

### Checkpoint 8a (as run)

Run on 2026-10-02 (about 02:57–03:12) over Tasks 1–5 and their review fixes, all uncommitted, one step at a time on the shared `scratchpad/p5a/target`, npm and cargo through `mise exec`. Nothing failed and nothing was fixed.

| Check | Result |
|---|---|
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio --no-fail-fast`, live (the `ci.yml` engine env, `SEAQUEL_TEST_REQUIRE_ENGINES=1`, `SEAQUEL_TEST_SSH`; the compose databases and the SSH server were already up, seeded with `npm run e2e:db:seed -- postgresql mysql mariadb sqlserver duckdb`) | pass: 2,011 passed, 0 failed, 3 ignored (the two keychain tests and the MCP doc test, as before), in 192 test targets, doc-tests included. 4 min 49 s wall (02:58:39–03:03:28), 9 s of it compiling. The same counts as Task 5's review run |
| `npm run crates:check` | pass: 25 crates |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with the bare `seaquel-core/browser` | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `browser,storage,workspace` and `seaquel-rpc/storage,workspace` | pass |
| wasm32 clippy, `seaquel-storage --all-targets` | pass |
| wasm32 clippy, `seaquel-engine-duckdb` with only `browser` | pass |
| wasm32 clippy, `seaquel-browser`, with and without `test-hooks` | pass, both |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 (`sqlite-wasm-rs`, `arrow-ipc` and `wasm-bindgen` included) |
| `npm run cli:build`, then `cargo test -p seaquel --lib` | pass: 46 |
| `npm run types:gen` twice | pass: the 232 generated files didn't change on either run, and match the tree |
| `node scripts/build-wasm.mjs --module browser` | pass, within budget (below) |
| `npm run check` | pass: 0 errors, 0 warnings (4,751 files) |
| `npx oxlint --type-aware --type-check --deny-warnings` | pass: exit 0, no diagnostics (oxlint 1.85 prints no summary when clean) |
| `CI=1 npx vitest run`, after `npm run wasm:build:browser-test` as CI does | pass: 2,070 tests in 120 files, no unhandled errors |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` as in 5e |
| `npm run build:demo` | pass |
| `seaquel_browser` in the outputs | none: no file named for it and no reference to it (or to `seaquel-browser` or `browser-pkg`) in `build` (314 files), `build-web` (609) or `build-demo` (314) |

**The direct wasm32 lines need CI's compiler variables here.** Run bare on this Mac, the three lines that compile SQLite's C (Core with `storage`, `seaquel-storage`, `seaquel-browser`) stop in `sqlite-wasm-rs`'s build script with `unable to create target: 'No available targets are compatible with triple "wasm32-unknown-unknown"'`: plain `cargo` takes Apple's `clang` from `PATH`. `ci.yml` sets `CC_wasm32_unknown_unknown`/`AR_wasm32_unknown_unknown` workflow-wide, and CLAUDE.md says to on macOS, so they ran with Homebrew's (`/opt/homebrew/opt/llvm/bin/clang`, 23.1.2, and its `llvm-ar`) and passed. `build-wasm.mjs` finds that clang itself.

**The module.** `build-wasm.mjs --module browser` found `browser-pkg/` up to date with the sources (cargo finished from cache in 3.4 s; the package was built at 02:44 after Task 5's re-review): cargo output 7,096.8 KB, `seaquel_browser_bg.wasm` 6,262.4 KB raw, 2,022.7 KB gzip -9, **1,481.0 KB brotli 11 (1,516,561 of 2,000,000 bytes, 76% of the budget)**, JS glue 25.8 KB. The same as after Task 5's review fixes. The test variant is 1,492.4 KB brotli.

**The demo baseline.** The tree was rsynced into `scratchpad/p8-cp8a/app` (without `.git`, `node_modules`, `target`, the build outputs and `.svelte-kit`), with `node_modules` linked to the repo's, as in Task 1. `SEAQUEL_WASM_PREBUILT=1 npm run build:demo` there used the existing `pkg/` and `browser-pkg/` and passed; its `build-demo` has no `seaquel_browser` either. The recorder (copied from `docs/plans/artifacts/2026-10-06-record-demo-baseline.mjs.txt`, Playwright 1.63's Chromium from the npx cache) recorded all 13 steps in about 1.5 minutes, and **`cmp` against `crates/seaquel-browser/tests/fixtures/demo-baseline/baseline.json` found them byte-identical** (63,098 bytes each, SHA-1 `0a87fdf1…`). So Tasks 2–5 changed nothing a demo visitor sees, and Task 1's two reload fixes are what the baseline holds.

No container was started or stopped (the four databases and the SSH server were up before the run); every process started here has ended.

**Not run:** the release workflow, a signed build, `cargo check -p seaquel` on its own (the `--lib` test builds it), and the probe (Task 7).

## Task 6: The demo on Core

**Files:**
- the ten seam sites (item 12) and `src/lib/core/index.ts`;
- `routes/(app)/+layout.svelte` and `+layout.ts` (open the module in the demo);
- `src/lib/demo/init.ts` (Decision 19: `ensure_demo_connection` through a demo-only call, then `db.connect` saved, then `db.execute` seeding), and `connection-manager.svelte.ts` (`addDemoConnection` through Core; `isDemoLibrary` goes);
- `hooks/database.svelte.ts` (Decision 17);
- `features/index.ts` (`aiAssistant` false in the demo, Q7 A);
- `providers/duckdb-provider.ts`, trimmed;
- deletions per item 14, the twin-only tests, `sql.js` in `package.json`;
- the moved replays (Parity table);
- `storage/db.ts` (`demoDatabase` goes).

**Tests first:**
- Every moved replay passes over the module with its twin-only exemptions removed. Run them before deleting anything, with both the twin and the module, and record the differences.
- `the demo starts on an empty store`, `the demo starts clean with an old file in localStorage and deletes it`, `a reload keeps a saved query and makes no second dashboard or tab` (vitest over the module, a fake DuckDB bridge where DuckDB isn't the point).

**Run:** `mise exec -- npx vitest run`; `npm run check` 0/0; svelte-autofixer on changed `.svelte` files; all three builds; `rg -l "ts-library|ts-runner|ts-service|TsEngineClient|getAdapter|sql\.js|sqljs" src` returns only the trimmed provider's comment, if anything.

**Review:**
- No seam picks a twin.
- The tutorial still runs in the web build and at `/demo/learn` (Q5).
- No test was deleted that Parity says moves.
- `isDemo()` gates only demo product choices (features, the badge, `demo-connection`'s actions).

**Things this task could quietly skip:**
- the extensions tab's demo path (now `db.duckdbExtension`);
- workflow query nodes in the demo;
- dashboard widgets' read-only runs;
- `window-id.ts`'s `demo` id as the origin;
- the storage gate's demo path;
- `beforeunload`'s `db.flush()` (now the snapshot flush).

### Notes from Task 6 (as built)

2026-10-02, ~0.5 h wall (about 03:10–03:40), with three test moves run in parallel by forked agents; about a third of it vitest runs, the three app builds and the recorder.

- **The seams.** Every one of item 12's ten sites now picks Core unconditionally, the demo included: `storage/db.ts` (`demoDatabase` and the lazy sql.js client gone), `library/index.ts` (`getLibrary`/`getUi`/`getSettings`; `DemoLibrary`, `isDemoLibrary` and the three lazy twins gone), `query-runner/index.ts`, `edit-service/index.ts`, `engine/index.ts` (`RustEngineClient` for every engine; `usesRustEngine` went, so `create-table-view.svelte` needs a connection for every dialect), `providers/index.ts` and `provider-registry.ts` (`CoreProvider` for every type; `getOrCreateDuckDB` gone). `getCoreClient` and `RustStorageClient` still take the browser client and transport only in a demo build once `useBrowserCore` ran (Task 5).
- **The demo's start** (Decisions 2, 19):
  - `src/routes/+layout.ts` opens Core before anything renders, behind `import.meta.env.VITE_BUILD_TARGET === "demo"` (`$lib/demo/core`'s `openDemoCore`, once per page; a failure shows the root layout's load-error page with the `RpcError` code, e.g. `CORE_FAILED`).
  - `$lib/demo/core.ts` starts DuckDB-WASM at the same time but doesn't wait for it: the bridge is `makeDuckDbBridge(lazyDuckDb(pageDuckDb))`, whose `connect` waits for DuckDB and whose other methods (which name a connection, so only exist after a connect) call it directly, keeping the bridge's post-before-return contract.
  - `$lib/providers/duckdb-wasm.ts` (`pageDuckDb`) is the demo's DuckDB-WASM. As first built it was shared with the tutorial's `DuckDBProvider`; review fix I1 gave the tutorial its own instance. It uses DuckDB's `VoidLogger`, not `ConsoleLogger` (its log lines carry SQL).
  - `$lib/demo/init.ts` (`startDemo(page, core, provider)`): `whenReady`, Core's `ensureDemoConnection`, `db.connect {saved: demo-connection}`, the sample SQL through `db.execute` one statement at a time (a failure logged by index and code, never SQL), `ConnectionManager.addDemoConnection(stored, coreId)`, then `createDemoDashboard`. `getDemoConnectionConfig` (dead, bug 6) went.
  - `routes/(app)/+layout.svelte` calls it behind the same build-time branch, shows Core's notices once as warning toasts (`demo_notice_storage_unavailable`, `demo_notice_storage_corrupt`, translated into all five locales), and now adds the reason to the "Failed to initialize demo database" toast.
  - `addDemoConnection(stored, providerConnectionId)` shows Core's row (`applyOwn`) instead of building one. A row the page already listed keeps its maps (`ensureConnectionMapsExist`), so its history and chats, loaded with the list, stay: **Task 1's finding (history lost on every reload) is fixed**. The engine client is `getEngineClient`'s, the extra-tab guard is kept, and `TsEngineClient` and `isDemoLibrary` went.
- **Decision 17.** `hooks/database.svelte.ts` builds `ChangeFeed`/`LibrarySync` and calls `listenForCoreEvents` in every build. The demo's own writes carry `demo` (the window id, `pageOrigin()`), so they are skipped; a restart's `connectionClosed` (`CORE_RESTARTED`) reaches `handleConnectionClosed`. `UseDatabase` is now exported (the start test builds a page from it).
- **Q7 A.** `features.aiAssistant` is `!demo`, read through `aiSettingsStore.available` (the flag and the user's setting): the header's toggle, the command palette's Toggle AI, the editor's inline prompt, the manage page's right panel and Settings' AI group and sections. The Features section hides the AI switch when the flag is off. `changes.json`'s `q7-ai-hidden` entry now says all of this, as Task 1 asked.
- **`DuckDBProvider`** (361 → 104 lines) keeps only `connect`, `disconnect`, `select` and `execute` for the tutorial (`TutorialProvider`); `selectReadOnlyOn`, `selectStream`, `selectReadOnly`, `test`, `executeRaw`, `getDb` and `getConnection` went. The tutorial still runs on it on web and at `/demo/learn` (not opened in a browser here; Task 7).
- **A bridge bug found by the start test.** DuckDB-WASM whose worker is gone (`terminate()`, a dead worker) answers every request with `undefined` at once; the driver read that as "not yet" and polled in a microtask loop that never yielded (the test process ran out of memory, 4 GB in ~13 s). The bridge now rejects `undefined` ("DuckDB isn't running") and maps `cancel` to `true`/`false` (`duckdb-bridge.test.ts`, 2 tests, both seen failing first). In the page a worker only dies on a crash, so this is a hang the probe could have met, not a common path.
- **Deleted** (item 14): `ts-library.ts` 1,502, `ts-state.ts` 760, `ts-settings.ts` 658, `ts-ui.ts` 270, `ts-runner.ts` 566, `ts-service.ts` 606, `ts-engine-client.ts` 162, `db/` (`index.ts` 155, `duckdb.ts` 527, `alter-table.ts` 221, `crud-helpers.ts` 102), and the sql.js storage (`sqljs-client.ts` 60, `web-sqlite.ts` 126, `schema.ts` 549, `create-repo.ts` 244, `repository.ts` 21, `sqlite-types.ts` 12, `sql.js.d.ts` 39, `repos/` 19 files 1,259): **37 files, 7,819 lines**. Twin-only tests: `ts-library.test.ts`, `ts-runner-fixtures.test.ts`, `ts-runner.test.ts`, `ts-runner-duckdb.test.ts`, `ts-service-duckdb.svelte.test.ts`, `ts-engine-client.test.ts`, `crud-helpers.test.ts`, `duckdb-read-only.test.ts`, `repos/project-state-repo.test.ts`: **9 files, 2,635 lines**. `sql.js` left `package.json` (`npm uninstall`). The spec's `rg` now matches only the two edit-fixture imports in `fixtures-replay.svelte.test.ts` (`plan-mysql.json`/`plan-mssql.json`, where the pattern's `.` matches `sql.json`'s dot).
- **Moved onto the module** (Parity), each run on the twin first, then on the module, differences recorded. The harness is `src/lib/core/browser/testing/meta.ts` (`openModuleCore`: a `RustStorageClient` per window origin through a new test-build export `__test_call_as(body, origin)`, and `query`/`execute` on the metadata file through `node:sqlite`'s `serialize`/`deserialize`, Core reopening on a written file).
  - `state-replay` (forked): 112 cases, 377 steps on both. `keyless()` went; the three AI-key steps now check Core's `NOT_SUPPORTED` (no secret store, as on web) and that nothing is written (`SECRET_STEPS`). `project_state` and `tabs` are compared now (the twin wrote no mirror; Core's matches the recording and `changes.json`), and `mirrorRows`/`projectStateRepo` went. `EXEMPT` keeps its two GUI entries (they fail without them on Core too) and gains three for `old-data/project-state-canvas-view`: the harness can only seed before Core opens, and the open's baseline rewrites `active_view` `canvas` to `workflow`, where the recording and Rust's replay seed after the open. Closing that needs a test-only raw write into the open Core. The dump drops what Rust's replay drops (`connectionStringSecretsUpgraded`, the 5d/5e columns). The beta-era file is built from the frozen `v2026.4.5-beta.1.sql`. 4,488 → 3,920 lines (the never-compared "Core calls derived from storage calls" machinery went).
  - `library-replay` (forked): 104 cases, 133 steps on the twin. On the module 18 cases are refused at their first step with `ENGINE_NOT_AVAILABLE` (Postgres, MySQL, MariaDB, SQL Server and SQLite rows: the module's Core registers only DuckDB, Decision 13); `OTHER_ENGINES` checks each is refused and writes nothing. 113 steps compared in full plus those 18 refusals. `SKIPPED`/`EXEMPT` are unchanged (none was there for the twin). `fixture-support.ts` lost sql.js.
  - `window-state` 31, `library-persistence` 21, `settings-across-tabs` 18 (forked): no differences beyond the seeded connections' type moving from `postgres` to `duckdb` (the same engine rule; 7 tests failed with `ENGINE_NOT_AVAILABLE` until then).
  - `client.test.ts`'s sql.js half (forked): `RustStorageClient` over the module. Of 76 recorded repo cases the 12 that call a method the client still has run for real; the 64 that call only retired repositories are no longer replayed in TypeScript (`repos.rs` replays them natively). The reload test runs the real page path (`openBrowserCore` over a store). 172 → 108 tests.
  - `dashboard-review` (6) and `sample-dashboard` (4): no differences. `connection-manager-demo` (Task 1's two tests): the module version passes; the twin version can't run any more, since `addDemoConnection` takes Core's row.
  - The DuckDB twins' live suites, re-run at Checkpoint 8a's copy (224 tests in 10 files, all passing) and then moved: `ts-service-duckdb`'s 3 plan, 4 apply and 1 table-page cases match the engine suite's (which adds the attached-catalog page the twin skipped); its two twin-only checks (values inlined into the plan's SQL, filters inlined) are `changes.json`'s `q6-edit-sql` and `q6-data-tab`. `ts-runner-duckdb` (3) and `duckdb-read-only` (23) became `src/lib/demo/duckdb-on-core.test.ts` (25), over the real page path (`openBrowserCore`, `CoreProvider`, `CoreQueryRunner`, DuckDB-WASM's Node build). Differences, all Core's behaviour: `FROM t` and the CTE that deletes are refused by Core's token check (`READ_ONLY`) where DuckDB's `query()` accepted the first and its binder refused the second (`QUERY_ERROR`); BIGINTs arrive as numbers (Decision 8); the twin's "a row limit that isn't a count" check has no Core counterpart (the caller's `maxRows` is a constant); an aborted read-only query now stops in DuckDB. `SET threads = 1` became `SET default_order = 'asc'` (the twin's was scripted, not run).
- **Tests first.** The three listed tests are `src/lib/demo/start.svelte.test.ts` (a whole `UseDatabase` page over the module and a fresh DuckDB per load, one snapshot store across loads), plus `a reload keeps the demo connection's query history`. They were written after `startDemo`'s first draft and passed on their first complete run, so each was checked by mutation instead: maps reset on every load, a query tab on every load, the old keys kept, the sample dashboard created on every load, and connecting before `ensureDemoConnection` each fail them. `duckdb-bridge.test.ts` failed first (2 of 2).
- **The baseline** (recorder against `build:demo` of this tree, Chromium; `scratchpad/p8-t6/{recorded.json,diff.txt,baseline-diff-summary.md}`; the frozen `baseline.json` untouched): 2 of 13 steps identical, `pageErrors` identical. Seen as `changes.json` says: `q7-ai-hidden` in all seven steps, `bug-7-page-sql`, `q6-edit-sql` (`?` placeholders and the "Values: 1: 'Jonson' 2: 1" line), `q6-data-tab` and bugs 1 and 2 (no difference). **Not as listed or not listed:**
  - `decision-8-cells`: `created_at` shows as `Jan 15, 2024, <time>` (the grid formats the timestamp text, as on desktop), not the raw `2024-01-15 10:30:00` the entry predicted.
  - Run errors gain a `Query failed: ` prefix (`DbError::query` in `seaquel-types`, the same on desktop).
  - `final reload` shows `History 4` instead of `History 0`, and the last step `History 6` instead of 2: history survives a reload now. The applied grid edit is recorded with its `?` placeholders.
- **Runs:** `CI=1 npx vitest run` 114 files, 1,852 tests (Checkpoint 8a: 120 files, 2,070; −9 deleted files, +3 new, −64 retired repo cases); `npm run check` 0/0 (4,711 files); oxlint clean; oxfmt clean on `src`; the autofixer found nothing caused by this task in the 8 changed `.svelte` files (only older suggestions); `npm run build`, `build:web` and `build:demo` pass. `seaquel_browser`: `build` 0 files and 0 references; `build-web` 0 files, and one comment naming `ensureDemoConnection` in an unminified server chunk (the `addDemoConnection` doc comment, not the module); `build-demo` has `seaquel_browser_bg.*.wasm` (6.4 MB raw) and 4 referencing files. No `sql-wasm` in any output. `cargo test -p seaquel-core --features seaquel-runtime/tokio` 378; `seaquel-browser` 3; rustfmt; wasm32 clippy for `seaquel-browser` with and without `test-hooks`.
- **For Task 7:**
  - The two unlisted baseline differences and `decision-8-cells`' wording.
  - ~~A desktop or dev build opened in a plain browser still reads as the demo~~ (review fix I2: it now shows the unsupported-build page).
  - The bridge's `undefined` fix (a crashed DuckDB worker), `lazyDuckDb` when DuckDB's CDN is unreachable (the connect fails with the reason; the toast says it), and the tutorial at `/demo/learn` (on its own DuckDB instance since review fix I1).
  - `beforeunload`'s `db.flush()` then `pagehide`'s snapshot: a write right before closing the tab.
  - Each tab keeps its own Core: with Decision 17 on, a second tab's writes never reach the first (no shared events), as Q1 A accepts.
  - The state replay's `old-data/project-state-canvas-view` exemption (harness only).

**Review fixes** (2026-10-02, ~0.4 h, about half of it the full vitest run, the three builds and the recorder):

- **I1, the tutorial's own DuckDB.** `providers/duckdb-start.ts` (`startDuckDb`, a new instance per call) and `duckdb-wasm.ts`'s `pageDuckDb` (the demo's) and `tutorialDuckDb` (the tutorial's), each started once. `DuckDBProvider` uses `tutorialDuckDb`. Decision 12 is amended. Test `src/lib/tutorial/database.test.ts` (the demo's Core over one Node DuckDB, the tutorial seeded by `executeQuery`): the demo connection's `schemaTables` shows only `demo.customers` and `listSchemas` no tutorial schema; the tutorial's `information_schema` shows no `demo.*` table. Seen failing first: the demo connection listed the six tutorial tables (`main.categories`, …).
- **I2, a plain browser.** `isDemo()` is the build constant (`VITE_BUILD_TARGET === "demo"` or `VITE_IS_DEMO`); `isSupportedBuild()` is desktop app, web build or demo build. The root `load` returns `unsupportedBuild: true` for anything else without loading the editor module or opening Core, and the root layout then renders only `unsupported-build.svelte` (`unsupported_build_title`/`unsupported_build_message`, in all six locales): "This build runs inside the Seaquel app. For the browser demo run `npm run dev:demo`." Nothing below the root layout mounts, so no `UseDatabase`, `CoreClient` or socket exists. Tests `src/lib/utils/environment.test.ts` (4) and `src/routes/layout.test.ts` (2, the load and the server-rendered layout, with `getCoreClient` and `initSeaquelWasm` spied): 5 of 6 failed first (one passed already: `VITE_IS_DEMO` alone).
- **`changes.json`** corrected (`decision-8-cells`, `bug-7-page-sql`'s `Query failed: ` prefix, new `history-survives-reload`), with a Corrections note in the README; the history follow-up is listed under "Follow-ups". The re-run recording's diff against `baseline.json` is identical to Task 6's first one and now matches `changes.json` entry for entry (`scratchpad/p8-t6/{recorded-review.json,diff-review.txt}`).
- **M1.** library-replay reads the recorded `postgres` as `duckdb` in `add/labels-and-ai-flags`, `add/other-project`, `add/duplicate-name-other-project`, `add-then-update-then-remove` and `update/type`'s seed (`AS_DUCKDB`, case and `changes.json` entry alike); the first four replay whole, and `update/type`'s one step (its change to MySQL) stays a checked refusal. 119 steps compared in full (was 113), now asserted exactly; `OTHER_ENGINES` keeps the 13 engine, SSH and secret cases plus `update/type`.
- **M2.** `old-data/project-state-canvas-view` moved from three `EXEMPT` entries to `SKIPPED_CASES`, whose reason names Rust's replay (`seaquel-core/tests/state.rs`); 111 cases and 375 steps, asserted.
- **M3.** `extensions-duckdb-tabs.svelte.ts`'s comment says Core's `db.duckdbExtension` on desktop and in the demo.
- **M4.** `tutorial/database.ts` and `getDuckDBProvider` branch on the build constant (`web` or `demo`), so the desktop build has no DuckDB-WASM: `build` has 297 files (was 299) and no `AsyncDuckDB`, `getJsDelivrBundles`, `duckdb-eh` or `*duckdb*` file.
- **M5** is a probe item under Task 7.
- **Runs:** `CI=1 npx vitest run` 117 files, 1,859 tests; `npm run check` 0/0 (4,716 files); oxlint and oxfmt clean; the autofixer clean on the root layout and `unsupported-build.svelte`; `build`, `build:web` and `build:demo` pass. `seaquel_browser`: none in `build` or `build-web`, in `build-demo` only. No `sql-wasm` anywhere.

## Task 7: Probe

A separate agent, on a `build:demo` served under `/demo` as Cloudflare serves it (`manage.html` for `/demo/manage`, S7), in Chromium, Firefox and WebKit through Playwright, plus one headed Chromium run. It records evidence for each:

- **Size and load.**
  - The module's raw, gzip and brotli sizes against Q3's budget.
  - Cold and reload times to the seeded state, five runs each, against Task 1's baseline.
  - Memory after load.
- **Old data.** A visitor with the live demo's data in `localStorage` (the probe makes it by loading the website's `static/demo` build first, from the same origin): the new demo starts clean, `seaquel_db` is gone after the first start, and it comes back from no path in the new build. A snapshot the probe corrupts by hand is kept aside with a notice.
- **Persistence.**
  - Ten writes, then a reload: all there.
  - A write, then closing the tab at once: there or not, and which.
  - A metadata file grown to 50 MB (history and dashboard versions): snapshot time per write and the frame it blocks.
  - Two tabs writing: the result matches Q1's answer.
- **Hostile and odd input.**
  - A 100,000-row result and a 10 MB cell: memory, and the time the page is blocked.
  - A long query cancelled from the pagination bar's Cancel: DuckDB stops.
  - `{{p}}` used 300,000 times: Core's budget refuses it.
  - SQL holding NUL, lone surrogates and `?` in odd places through the grid's writes.
  - DuckDB's `read_csv('https://…')` from the AI's path (a known gap, CLAUDE.md).
- **Failure.**
  - A trap (debug build's `__test_trap`), then the app keeps working from the last snapshot.
  - IndexedDB blocked (a Firefox profile with storage off) and a private window.
  - DuckDB-WASM's CDN unreachable (offline): the page says what failed.
- **Leaks.** The console carries no SQL, values or names at any level.
- **Closing the tab right after a write** (Task 6's review, M5): `beforeunload`'s `db.flush()` puts the view state's save on the write queue, and `pagehide`'s `flushNow` takes the snapshot. Check whether the snapshot can be taken before that queued save commits (so the last view-state change is lost), in all three browsers. Not fixed in Task 6.

Probe fixes are budgeted separately.

### Probe fixes (as built)

2026-10-02, ~1.4 h wall, more than half of it rebuilding the probe's two demo copies (`scratchpad/p8-probe/app`, `app-th`, re-synced from the tree; `app-th` with the test-hooks module and the probe's `__probeOpened` line) and running the probe scripts before and after. Before and after outputs: `scratchpad/p8-probe/fixes/{before,after}/`.

1. **The last view-state change on close or reload** (Important). Chromium runs no task between `beforeunload` and `pagehide`, so the save `beforeunload`'s `db.flush()` queued hadn't committed when `pagehide` took the snapshot.
   - Fix: the demo saves at `pagehide`, as web does (the layout's `savesOnPageHide`). The active project's pending view state goes, synchronously, into a `localStorage` journal, `seaquel.demo.pendingViewState` (`$lib/storage/view-state-journal.ts`). The journal holds the `ui.windowStateSave` request itself, capped at 60 KiB like a keepalive body; `RustStorageClient.saveWindowStateKeepalive` writes it in a demo build. `openBrowserCore` replays it through Core right after Core opens, before the page loads its view state, and forgets it either way; Core keeps it only if its `rev` is newer. Only that key is written; the old `seaquel_db*` keys are still deleted unread (Q2 C).
   - Tests:
     - `view-state-journal.test.ts` (5): 3 failed first against a stub.
     - `the_view_state_journal_is_replayed_after_the_open_and_only_a_newer_rev_is_kept` (module): failed first with `[5, 'stored']`.
     - `keepalive.test.ts`'s demo case: failed first (it called `fetch`).
   - Probe `t-m5d.mjs`, active tab switched then the tab closed or reloaded, kept of 10:

     | Run | Before | After |
     |---|---|---|
     | Chromium close | the probe's 1 of 10 (this rerun crashed the browser target) | 10 |
     | Chromium reload | 0 | 10 |
     | WebKit close | 10 | 10 |
     | Firefox close | 10 | 10 |

     `t-m5.mjs` (a new query tab, then close) after: WebKit 10 of 10, Chromium 10 of 10.
   - **Two tabs:** the journal is one key per demo origin (window id `demo`), and `rev` (each tab's own count) decides, so it's Q1's last writer wins: a closing tab's journal with older content but a higher `rev` can replace what another tab saved. Accepted under Q1 A.
2. **WebKit and a save in flight at close.** The probe's WebKit loss was view state too: a new query tab, then close (`closeAfterNewTab`, 1 of 5 kept). The journal covers it (10 of 10 above), so view state no longer depends on an IndexedDB write finishing at unload. Other writes made in the last moment before closing (a saved query, a dashboard edit) still depend on it. In WebKit, an IndexedDB transaction still running when the page unloads isn't guaranteed to commit, and the snapshot of such a write can be lost. This is a known limit; nothing large goes into `localStorage` to work around it.
3. **Reconnect after a trap.** `handleConnectionClosed` reconnects a connection closed with `CORE_RESTARTED` at once (`autoReconnect`, a saved-target connect) and shows an error only if that fails. Every build has this, but only the demo emits the code. Tests: two new `connection-manager` cases; the reconnect case failed first. Probe `t-trap3.mjs` after: a run straight after the trap, with no click, succeeds, and the sample tables are there.
4. **`noticeTrap` in WebKit.** WebKit's stacks name no module. The glue patch now records each `RuntimeError` thrown through one of this module's closures, with the instance generation it came from. The new export `__seaquel_isCurrentTrap(e)` answers by object identity and only for the live instance, so a trap already restarted from isn't restarted again. The stack-name check is gone. Another module's trap (the editor module's, DuckDB's) is never ours. A trap recorded under an older generation isn't prevented either, so it still shows as uncaught in the console; that is cosmetic and on purpose (a comment says so), since acting on it would restart Core twice. Tests: `noticeTrap attributes this module's trap without a URL in its stack (WebKit), once` failed first (`[false]`); one restart, not two. Probe `t-trapstack.mjs`: the WebKit error event is now `prevented` (before: `false`; Chromium and Firefox `true` both times).
5. **A DuckDB worker killed from outside.** It answers nothing at all (only `db.terminate()` makes it answer `undefined`).
   - The bridge now has a liveness check. While any request has waited `checkAfterMs` (5 s), it pings with `getVersion()`. A ping unanswered after `pingTimeoutMs` (10 s) fails every waiting request, and every later one, with `DUCKDB_STOPPED` ("DuckDB stopped responding: its worker may have crashed. Reload the page to start it again."). `cancel` and `close` resolve quietly once it's dead.
   - A pending query answers pings between its polls (live test: a 3 s query pinged every 200 ms with a 1 s timeout).
   - Review fix: no verdict is passed while a `runQuery` runs (the driver's ROLLBACK on drop and the ENUM re-read can't be interrupted, and a worker busy in one answers no ping). A verdict is undone when the ping answers late: the requests it failed stay failed, and later ones run. So one missed ping no longer kills the bridge until a reload. The tutorial has its own `AsyncDuckDB` (`tutorialDuckDb`, its own worker, since Task 6's I1), so a long tutorial query can't trip the demo bridge's clock. Tests: "is undone when the late ping answers" and "isn't passed while a runQuery is running", both seen failing first.
   - Tests: `duckdb-bridge.test.ts` +2 (the dead case hung first; the slow-but-alive case passed before and after), and the live long-query case in `engine-duckdb-browser.test.ts`.
   - Probe `t-fail.mjs worker`: before, still "streaming…" at the 20 s timeout; after, the run fails in 12.4 s with that message.
6. **Firefox with storage blocked.** The bare 500 had two causes. `app.html`'s inline script threw on `localStorage`, and so did libraries that read it unguarded at startup: paraglide's `localStorage` locale strategy, mode-watcher and runed's persisted state.
   - Fix: `app.html` reads through a guarded `read()`. When `localStorage` or `sessionStorage` throws on access, it gives the page an in-memory `Storage` before anything else runs. Nothing is kept anyway, and the demo's IndexedDB fails the same way, so the demo shows its `STORAGE_UNAVAILABLE` notice.
   - Tests: `src/app-html.test.ts` (3). "runs when localStorage throws" failed against HEAD's `app.html`; the in-memory case failed first.
   - Probe `t-blocked.mjs`: before "500 Internal Error"; after, the demo loads with the notice ("…nothing you do here is kept…") and "Demo database loaded with sample data". The only page error left is bug 8's Monaco `parentNode`.
   - The web build uses the same `app.html` (the unit test covers the script). It wasn't run in a blocked browser here, since it needs the server and a session.
7. **A lone surrogate in a unary call.** `encodeCoreRequest` now sends every string through `wellFormedJson`, keys included, so every `call` on every transport (Tauri `core_call`, `/api/rpc`, the demo's module, the keepalive) is well-formed, as streams already were. `wellFormed`/`wellFormedJson` moved to `$lib/core/well-formed.ts`, which `rust-client` imports without a cycle. Tests: Tauri, HTTP and the module, 3 cases; all failed first, the module's with the raw `invalid request: unexpected end of hex escape`. Probe `t-gridui2.mjs`: before, two "Failed to update cell: INVALID_ARGUMENT …" toasts; after, the edit is queued and applied ("1 statement executed successfully"), stored with U+FFFD.
8. **DuckDB-WASM's worker logs failing SQL.** DuckDB-WASM 1.32's worker `console.log`s every request that fails (`catch(t){return console.log(t), this.failWith(e,t)}`), whatever logger the page passes, and it has no setting for that. Our blob worker script (`duckdbWorkerScript`) now silences the worker's console before `importScripts`; errors still reach the page as rejected requests. `warn` and `error` are silenced too, on purpose: the 1.32 worker's `warn`/`error` lines print file names and the URLs a query passes to `read_csv` and friends (`"FAIL WITH: …"`, "fall back to full HTTP read for: <url>", "Buffering missing file: <name>"), and its emscripten hook sends DuckDB's C++ messages there. The demo and the tutorial both start through it. Test: `duckdb-bundles.test.ts`'s worker-script case failed first. Probe `t-leak.mjs chromium`: 2 hits (the SQL with `LEAKVALUE2`, as `[worker log]` and `[log]`), now 0.
9. **The editor with many `{{p}}`** (pre-existing, not fixed; follow-up below).
- **Also:** the manual checks' cancel is the results pagination bar's Cancel (fixed).
- **Runs:** `CI=1 npx vitest run` 119 files, 1,880 tests; `npm run check` 0/0 (4,720 files); oxlint clean; the autofixer clean on the changed part of `routes/(app)/+layout.svelte`; `build`, `build:web` and `build:demo` pass, with `seaquel_browser` in `build-demo` only. No new en.json keys. The release module is unchanged in size (1,481 KB brotli); the test module's glue grew by the trap bookkeeping.

## Task 8: Docs, measurement, checkpoints

- **CLAUDE.md:**
  - the demo section (Core in the page, the module, the transport, the snapshot, the old keys deleted, the clang prerequisite);
  - Core's `browser` feature;
  - `seaquel-storage`'s two executors;
  - the DuckDB engine's two drivers and their differences (S5);
  - every "until phase 8" line;
  - the `demo-connection` claim;
  - the tutorial's provider.
- **Design doc:** the status line, "As built in phase 8", "Phase 8 cost", the "Risks" and "Open questions" entries on the demo gap, WASM size and the browser storage backend.
- **This plan:** execution notes, release notes (the demo's behaviour now matches desktop; earlier demo data is not carried over, per Q2; the assistant is off in the demo; what visitors will notice per Q6; DuckDB-WASM 1.4.3's differences from desktop, per Task 4's notes: JSON shows as text, BIT as bytes, TIMETZ without its offset, TIME_NS and GEOMETRY unavailable, and ENUM columns shown only by a single SELECT, with other statements returning an ENUM asking for a cast to VARCHAR), the checkpoint, manual checks.
- **Effort log:** rows and totals.
- **Measurement:** sizes and load times as in "Sizes and load times today", before and after.
- **The `demo:update` dry run:** in a scratch copy of the tree, `npm run build:demo`, then `cp -r build-demo <scratch>/static/demo` exactly as the website's command does, served under `/demo`, opened in all three browsers. The website repo isn't touched.
- **The full check list**, with one live run at each checkpoint.

**Status (Task 8):** done. CLAUDE.md (the demo section rewritten as "The demo": Core in the page, the start, `openBrowserCore`, the transport, the snapshot, the view-state journal, the old keys, trap recovery, product choices, tests, and `demo:update`'s prerequisites; the second module's scripts and the clang prerequisite; Core's `browser` feature and `WasmExecutor`; `ensure_demo_connection`, which makes the `demo-connection` claim true; `seaquel-storage`'s two executors; a `seaquel-browser` entry; the DuckDB engine's two drivers and the browser driver's differences; the tutorial's own provider and DuckDB instance; `float_roundtrip`; the unsupported-build page; `CORE_RESTARTED`'s reconnect; `wellFormedJson` on every request; and every "until phase 8" line and twin, `duckdb.ts`, sql.js and demo `DuckDBProvider` mention removed or rewritten as history), the design doc (the status line, "As built in phase 8", the migration plan's phase 8 entry, "Phase 8 cost", and the Risks and Open questions entries), this plan's sections below, and the effort log are written. Checkpoint 8b and the `demo:update` dry run passed with nothing to fix. The owner's manual checks are pending.

---

## Manual checks

For the owner, after Task 8. The agents drove the demo only through Playwright's headless Chromium, Firefox and WebKit builds (plus one headed Chromium run in the probe). These checks cover what only a person in a real browser can confirm. Each says what to do and what you should see.

**Setup.**
- Once: `brew install llvm` (Q9). Then `npm run build:demo`. It prints both modules' sizes, the clang it used (`/opt/homebrew/opt/llvm/bin/clang`) and `within the brotli budget (1516561 of 2000000 bytes)` or close to it.
- Serve it under `/demo` as the website does: copy `build-demo` to a scratch `static/demo` and serve that directory's parent with a static server that maps `/demo/manage` to `manage.html` (the probe's `scratchpad/p8-probe/lib.mjs` does), or run the website's dev server after `demo:update`.
- To try the demo over old data: before switching, open the live `seaquel.app/demo` (or the website's current `static/demo` served from the same origin and port), save a query and run a few. Then load the new build from that same origin.

**Start and reload**
- [ ] A cold load shows "Demo Database" connected, the tabs Getting Started, Migration Tips, Query 1 and E-Commerce Overview, and one toast: "Demo database loaded with sample data". No error toast.
- [ ] Reload three times: the same tabs (no "Query 2"), one "E-Commerce Overview" in the dashboards list, and no "Couldn't save the dashboard" toast.
- [ ] Run a query, reload: the Queries panel still says "1 executed" (history survives a reload now; it didn't before).
- [ ] Save a query, star a history entry, move a dashboard widget, add a label to the demo connection. Reload, then quit the browser and open the demo again: all four are still there.

**Old data and storage**
- [ ] Over old data: the first load of the new build shows none of the earlier saved queries or history. DevTools → Application → Local Storage has no `seaquel_db` key; IndexedDB has `seaquel-demo` → `files` → `meta.db`.
- [ ] Firefox with cookies and site data blocked for the site (Settings → Privacy → Manage Exceptions → Block): the demo loads, shows a warning toast that nothing will be kept, and then "Demo database loaded with sample data". It no longer shows a bare "500 Internal Error".
- [ ] A private window in each browser: the demo works; after closing the window and opening a new private one, nothing from the first is there.
- [ ] Two tabs: save query A in tab 1 and query B in tab 2, then reload tab 1. Tab 1 shows B and not A (the last tab to write wins, Q1 A). Neither tab shows the other's change until it reloads.

**Close right after a change** (the probe's M5)
- [ ] Chrome: switch the active tab from Query 1 to E-Commerce Overview and close the browser tab within half a second. Reopen the demo: E-Commerce Overview is active.
- [ ] Safari: the same, then save a query and close the tab at once. The active tab is kept; the saved query may be missing (a known limit: WebKit can drop an IndexedDB write still running at unload). Note how often it's missing.

**Queries and edits**
- [ ] The AI assistant isn't offered: no AI button in the header, no AI item in the command palette or Settings.
- [ ] Run `SELECT sum(range) FROM range(30000000000)` and press **Cancel** in the results' pagination bar. It stops within a second and the next query runs at once.
- [ ] Edit a cell in `demo.customers` with pending changes on: the sheet shows `?` placeholders and a "Values: …" line; apply. Turn pending changes off and edit `demo.orders`. Both stick until a reload (the sample tables are seeded again on every load, as before).
- [ ] Data tab of `demo.orders`: filter with `IN` and a range. The rows and "Showing … of … rows" are right; on a full page the total may be marked as an estimate.
- [ ] `SELECT * FROM demo.customers WHERE false` shows the column names with no rows.
- [ ] `CREATE TABLE demo.t AS SELECT 1 AS x; SELECT * FROM demo.t;` then edit `x` in the grid: refused as not editable (no primary key).
- [ ] The DuckDB extensions tab lists extensions; install and load `json`.

**Failure**
- [ ] DevTools → Network → block `cdn.jsdelivr.net`, then load the demo: a toast says DuckDB failed to start, with the reason. Unblock and reload: it works.
- [ ] In Chrome, open `chrome://inspect/#workers` and terminate the demo's DuckDB worker. Run a query: within about 15 s it fails with "DuckDB stopped responding … Reload the page". Reload: it works.

**Tutorial and builds**
- [ ] `/demo/learn` runs its lessons. After a lesson creates its tables, the demo connection's schema tree still shows only `demo` with its four tables.
- [ ] `npm run dev` (the desktop build) opened in a plain browser shows "This build runs inside the Seaquel app…" and nothing else.
- [ ] Safari and Firefox at the previous major version (Q8 A): the cold load and one run work.
- [ ] On a phone: the demo loads (not tested; Q8 A only asks that it loads).
- [ ] Headed, with DevTools open: the console shows no SQL, values or names after a failing query (`SELECT * FROM demo.nope WHERE name = 'LEAK'`). The Monaco `parentNode` error at load (bug 8) and "Could not create web worker(s)" are older; note whether they show.
- [ ] `npm run demo:update` in the website repo: it ends with `static/demo` holding about 307 files (34 MB), and the deployed `/demo` passes the first check above.

---

## Execution notes

Executed task by task with subagents: Tasks 1–4 overlapping, a review after each task (two rounds for Task 5), Checkpoint 8a, Task 6 with three test moves forked in parallel, a probe in three browsers, one round of probe fixes with a review, and Task 8. Where the result departs from the text above, the repo is authoritative. Per-task times are in `2026-10-06-phase-8-effort.md`; the measured cost is in the design doc ("Phase 8 cost").

**What went differently from the plan**

- **The phase logged ~12.2 h against "expect about 27.5 h"** (range ~21.8–32.5 h): first passes ~8.25 h (Task 8 included), review fixes ~2.05 h (about 28% of Tasks 1–7's first passes, against the ~70% budgeted), probe fixes ~1.65 h, Checkpoint 8a ~0.25 h. From the first task's start to Task 8's end was about 7.3 hours of calendar time, since Tasks 1–4 ran side by side.
- **The spikes paid off.** Every open technical question (sqlx in the browser, SQLite in wasm32, Core with `workspace`, DuckDB-WASM's cancel and Arrow bytes) had been answered with running code before the plan was written, so no task had to change direction. The surprises were in DuckDB-WASM's protocol, not in Core.
- **The bridge grew `pollPending`** and is bound as plain functions returning promises, not `async` externs, because an `async` extern calls JavaScript only when first polled and a drop's cancel and `ROLLBACK` must leave before the next call (Task 4). DuckDB-WASM's pending results also send no end marker and no ENUM dictionary, which needed the IPC reader's own end handling and a `runQuery` re-read for ENUM.
- **The module's glue needed its own patch.** Re-instantiating it the way the editor module does would have let a late DuckDB answer run the new instance with the old one's pointers; the glue stamps each closure with an instance generation (Task 5), and the probe added the trap identity check WebKit needs.
- **`open` resolves the commit counter** rather than `{ok} | {error}`, and refusals reject with the `RpcError`'s JSON text.
- **`float_roundtrip`** went on for the whole workspace (Task 5 review): the JSON wire lost the last bit of some floats on every interface, which only a suite that crosses JSON could see.
- **Decision 12 was amended** (Task 6 review): the tutorial has its own DuckDB-WASM instance, because sharing one catalog showed Learn's tables in the demo connection and let either drop the other's tables.
- **`isDemo()` became a build constant** with an unsupported-build page (Task 6 review), closing item 11's follow-up inside the phase.
- **The probe's fixes added a `localStorage` key** after all: the view-state journal (`seaquel.demo.pendingViewState`), because Chromium runs no task between `beforeunload` and `pagehide`. Decision 6 had said the snapshot is the only store. `app.html` also gives a page whose storage is blocked an in-memory `Storage`, and the bridge gained a liveness check for a dead worker.
- **Library cases on other engines.** The module's Core registers only DuckDB, so 18 recorded library cases answer `ENGINE_NOT_AVAILABLE` in the TypeScript replay (checked as refusals); five more replay whole with the recorded `postgres` read as `duckdb`. Core's native replay still runs all of them.
- **64 storage repo cases are no longer replayed in TypeScript**, since the methods they call are gone from the client; `repos.rs` replays them natively.
- **Q3's budget held with room**: the module is 1,481 KB brotli of 2,000 KB, and cold loads got faster, not slower.
- **Checkpoint 8b found nothing** to fix: every check passed on its first run.

**Decisions made during execution**

- **Owner:** Q1–Q9, all before execution.
- **Coordinator, for the owner to overrule:** `float_roundtrip` for the workspace; Decision 12's amendment (two DuckDB instances); the unsupported-build page; the view-state journal and the `app.html` fallback (probe fixes 1 and 6); accepting WebKit's possible loss of a non-view-state write made just before closing (probe fix 2); the liveness thresholds (5 s before a ping, 10 s for its answer); leaving the editor's slowness with many `{{p}}` uses as a follow-up.

---

## Release notes

For the release that ships phase 8 and the website's next `demo:update`. The website's `static/demo` dates from 2026-09-23, before phase 1, so that update also ships everything since phase 1 to the demo (the editor module among it). Earlier notes still apply as written.

The demo (`seaquel.app/demo`):

- **The demo now runs the same engine as the desktop app**, compiled to WebAssembly and running in your browser. Queries, paging, edits, the data tab, saved queries, history, dashboards and settings behave as they do on desktop.
- **Earlier demo data is not carried over.** Saved queries, history and dashboards from earlier visits are gone, and the old copy in your browser's local storage is deleted on the first visit.
- **What you do in the demo is kept** in your browser (IndexedDB) across reloads and browser restarts: saved queries, history, dashboards, labels, settings and open tabs. Query history used to disappear on every reload; it no longer does. A private window keeps nothing after it closes, and with site data blocked the demo still loads and says nothing will be kept (Firefox used to show a bare "500 Internal Error").
- **Reloads are quiet again.** A reload no longer shows "Couldn't save the dashboard: There's already a dashboard called "E-Commerce Overview" in this project." and no longer opens another query tab ("Query 2", "Query 3", …).
- **The AI assistant is off in the demo** for now: no API key can be kept or sent there.
- **Changes you may notice**, all as on desktop:
  - A table without a primary key can't be edited from the grid.
  - Pending changes show the SQL with `?` placeholders and the values beside it.
  - Data-tab filter values are sent as parameters, and a full page's total can be an estimate, marked as one.
  - Cancel stops the query in DuckDB, and the next query runs at once.
  - An empty result shows its column names.
  - `{{param}}` values may add at most 32 MiB to a run.
  - Timestamps are shown formatted as dates and times, and error messages start with "Query failed:".
  - A query ending in a `--` comment pages correctly.
- **DuckDB in the browser is version 1.4.3** (the desktop app has 1.5), and its results differ from the desktop app's in a few types: JSON shows as text, BIT as bytes, TIMETZ without its offset; TIME_NS and GEOMETRY aren't available; an ENUM column shows only when the statement is a single SELECT, and any other statement returning one asks you to cast it to VARCHAR.
- **The tutorial (Learn) has its own database.** Its tables no longer show up in the demo connection, and nothing in one can drop the other's tables.
- **If something breaks in the page**, the engine restarts from the last save and reconnects on its own. If DuckDB's worker stops answering, the query fails after about 15 seconds with "DuckDB stopped responding … Reload the page". If DuckDB can't be downloaded, the page says so with the reason.
- **The first visit downloads more** (about 23 MB uncompressed from the site, 17.5 MB before; DuckDB itself still comes from jsDelivr), yet the demo is ready slightly sooner than before in Chrome, Firefox and Safari.

Every interface:

- **Floating-point values keep their exact value** when written back: the last bit of some floats (FLOAT's maximum among them) could change on the way, so an edit keyed on such a value could miss its row.
- **Text with a broken surrogate pair** (a grid edit, a name) is saved with a replacement character instead of failing with "unexpected end of hex escape".

For developers:

- `npm test`, `npm run dev:demo`, `npm run build:demo` and the website's `demo:update` need an LLVM clang with the wasm32 backend; on a Mac, `brew install llvm` once.
- A desktop build opened in a plain browser shows a page saying to run `npm run dev:demo` instead of acting as the demo.

Known issues:

- Two demo tabs don't see each other's changes until a reload, and the last tab to write wins.
- In Safari, a change made just before closing the tab (other than which tabs are open) can be lost.
- Applied grid edits appear in query history with `?` placeholders and no values, on every interface.
- The editor slows down badly with hundreds of `{{param}}` uses (every interface).
- Edits to the demo's sample tables are lost on reload, as before (they are seeded again on every load).
- The console shows an older Monaco error at load (`… reading 'parentNode'`).

---

## Checkpoint 8b

The full check list, run on 2026-10-02 (about 06:33–06:49) over Tasks 1–7, their review and probe fixes and the CLAUDE.md edits, all uncommitted, one step at a time on the shared `scratchpad/p5a/target`, npm and cargo through `mise exec`; the wasm32 lines with `CC_`/`AR_wasm32_unknown_unknown` pointed at Homebrew's llvm, as at Checkpoint 8a. Script and logs: `scratchpad/p8-t8/{checks.sh,logs/}`. Nothing failed and nothing was fixed.

| Check | Result |
|---|---|
| `npm run e2e:db:seed -- postgresql mysql mariadb sqlserver duckdb` (the compose databases and the SSH server were already up; none started or stopped) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio --no-fail-fast`, live (the `ci.yml` engine env, `SEAQUEL_TEST_REQUIRE_ENGINES=1`, `SEAQUEL_TEST_SSH`) | pass: 2,011 passed, 0 failed, 3 ignored (the two keychain tests and the MCP doc test, as before), in 192 test targets, doc-tests included. 5 min 6 s wall (06:33:59–06:39:05), 11.7 s of it compiling. The same counts as Checkpoint 8a |
| `npm run crates:check` | pass: 25 crates |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| wasm32 clippy: the pure crates; Core and `seaquel-rpc` bare `browser`; the same with `browser,storage,workspace`; `seaquel-storage --all-targets`; `seaquel-engine-duckdb` with only `browser`; `seaquel-browser` with and without `test-hooks` | pass, all seven |
| `seaquel-storage`'s wasm tests (`cargo test --target wasm32-unknown-unknown -p seaquel-storage --all-targets` under `wasm-bindgen-test-runner`; not in CI) | pass: 6 wrapper tests, 7 in `tests/wasm.rs` |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run cli:build`, then `cargo test -p seaquel --lib` | pass: 46 |
| `npm run types:gen` twice | pass: the 232 generated files didn't change on either run |
| `node scripts/build-wasm.mjs --module browser` | pass: 1,481.0 KB brotli, `within the brotli budget (1516561 of 2000000 bytes)` |
| `npm run check` | pass: 0 errors, 0 warnings (4,720 files) |
| `npx oxlint --type-aware --type-check --deny-warnings` | pass: no output |
| `npm run wasm:build:browser-test`, then `CI=1 npx vitest run` | pass: 1,882 tests in 119 files (Checkpoint 8a: 2,070 in 120; Task 6 deleted the twins' tests and 64 TypeScript repo replays, and later rounds added 30), no unhandled errors |
| `npm run build` | pass: 297 files |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288`: 572 files |
| `npm run build:demo` | pass: 307 files |
| `seaquel_browser` in the outputs | `build` and `build-web`: no file named for it and no reference to it, `seaquel-browser` or `browser-pkg`; `build-demo`: the module and 3 files referencing it. No `sql-wasm` in any output |

**The `demo:update` dry run.** The website's script (`seaquel-app/main/packages/marketing/package.json`, read only) is `rm -fr ./static/demo && cd ../../../../seaquel/ && npm run build:demo && cp -r ./build-demo ../seaquel-app/main/packages/marketing/static/demo`. The tree was rsynced into `scratchpad/p8-t8/app` (without `.git`, `node_modules`, `target`, the build outputs, `.svelte-kit` and both modules' packages, so both were built fresh) with `node_modules` linked. There `npm run build:demo` ran with no environment of its own beyond the shared `CARGO_TARGET_DIR` (06:42:27–06:44:51): it found Homebrew's clang, built the editor module (484.9 KB brotli) and the browser module (1,481.0 KB brotli; `wasm-opt` skipped, 1,539.0 KB with it) and wrote 307 files. Then `rm -fr ./static/demo && cp -r …/app/build-demo ./static/demo` in `scratchpad/p8-t8/site`. Served under `/demo` with the probe's server (Cloudflare's `manage.html` resolution) and opened with Playwright 1.63 in Chromium 153.0.8010.12, Firefox 155.0 and WebKit 26.6 (`scratchpad/p8-t8/{dry.mjs,dry.json}`). In each of the three:
- with `seaquel_db`, `seaquel_db.v` and an unrelated key set on the origin first, the cold load reached "Demo database loaded with sample data" as the only toast, with the tabs Getting Started, Migration Tips, Query 1 and E-Commerce Overview (active), and the sample dashboard shown; both `seaquel_db` keys were gone and the unrelated key kept;
- `SELECT 'dry' || count(*)::VARCHAR AS v FROM demo.customers` answered `dry10`, and the history went from 0 to 1;
- IndexedDB held `meta.db` (376,832 bytes);
- after a reload: the same toast only, the same four tabs with Query 1 active, one sample dashboard, history 1, and no `seaquel_db` key;
- page errors: only the older Monaco `parentNode` error at load and Monaco's `Missing requestHandler or method: doCompletionWithEntities` while typing (both in Task 1's recording of the twin build); the console also had Monaco's "Could not create web worker(s)" and `doValidation` warnings, which the probe's runs show too and which weren't checked against the twin build.

**Not run:** the release workflow, a signed build, the website's own scripts (by rule), and a headed browser.

---

## Measurement

Sizes are from the dry run's `build-demo` (Node zlib, brotli 11, as `build-wasm.mjs` prints them; KB = 1,024 bytes for the modules, MB = 10⁶ bytes elsewhere). "Before" is "Sizes and load times today" (the twin build at `8d73887`).

| | Before | After |
|---|---|---|
| `build-demo` on disk | 28 MB, 314 files | 34.1 MB, 307 files |
| JavaScript | 255 files, 25.39 MB raw, 4.29 MB brotli | 248 files, 25.26 MB raw, 4.26 MB brotli |
| The editor module | 1.73 MB raw, 487 KB brotli | 1,703.7 KB raw, 484.9 KB brotli |
| sql.js | 658 KB raw, 279 KB brotli | gone |
| The module (`seaquel_browser_bg.wasm`) | — | 6,262.4 KB raw, 2,022.7 KB gzip -9, **1,481.0 KB brotli** (1,516,561 of the 2,000,000-byte budget, 76%) |
| DuckDB-WASM (jsDelivr) | 34.2 MB + 0.77 MB worker | unchanged |
| Bytes a cold load serves from the demo's host | 17.5 MB | 23.1 MB |

Load time from navigation to the "Demo database loaded with sample data" toast, Playwright headless, served from localhost without compression, DuckDB from jsDelivr, five runs each:

| | Survey (twin, Task 1's session) cold / reload | Probe, twin build, same session | Probe, phase 8 build | Dry run, phase 8 build (after the probe fixes) |
|---|---|---|---|---|
| Chromium | 0.88–0.90 s / 0.13 s | 0.84–0.88 s / 0.66 s | 0.74–0.82 s / 0.59–0.61 s | 0.74–0.89 s / 0.59–0.61 s |
| Firefox | 1.44–1.49 s / 0.30 s | 1.21–1.34 s / 0.97–1.02 s | 1.02–1.15 s / 0.81–0.86 s | 0.99–1.08 s / 0.81–0.87 s |
| WebKit | 1.89–1.92 s / 0.25 s | 1.84–1.89 s / 1.39–1.41 s | 1.80–1.88 s / 1.44–1.51 s | 1.79–1.85 s / 1.47–1.52 s |

The survey's reload numbers don't match the method the probe and the dry run use (a fresh browser context per run, a 1.5 s wait, then the reload), so compare reloads only within the probe's session. Against the twin build measured in that session, a phase 8 cold load is about 40–220 ms faster in every browser, and a reload is 40–150 ms faster in Chromium and Firefox and about 60–100 ms slower in WebKit. Q3's "at most 400 ms slower" holds everywhere. Memory after load (Chromium, CDP, probe): JS heap 25.6 MB used, against 25.8 MB for the twin build; the module's own memory (wasm linear memory) isn't in that number. Other probe figures: a 50 MB metadata file snapshots in 4–5 ms and saves in 49 ms (Chromium), 153 ms (Firefox) and 74 ms (WebKit), with one dropped frame, and reloads in 1.8, 3.5 and 3.3 s; a 100,000-row result in 0.3–0.6 s with a 59–87 ms longest frame.

---

## Follow-ups (not in phase 8)

- **Merging the editor module into the module** for the demo (Q3 B), once the budget is measured.
- **OPFS and a worker** (Q1 B), if a visitor's lost write matters more than it seems to.
- **The tutorial on Core** (Q5 B or C).
- **The demo's AI** (Q7), with phase 6.
- **rusqlite or a newer sqlx** once sqlx moves past libsqlite3-sys 0.30, which would retire the hand-written wrapper.
- **Two DuckDB versions:** the demo's DuckDB-WASM (1.4.3) behind the native engine (1.5). Bump `@duckdb/duckdb-wasm` when a 1.5 build exists, and re-run the engine suite.
- **The value differences in S5** (TIMETZ offset, HUGEINT past 38 digits): upstream DuckDB-WASM's Arrow export.
- ~~**`isDemo()` as a positive check** (item 11)~~: done in Task 6's review (I2), with the unsupported-build page.
- **The Monaco `parentNode` error** at load (bug 8): still in every headless run of the dry run, in all three browsers, as in the twin build. The dry run also logs Monaco's "Could not create web worker(s)" warning and `doValidation`/`doCompletionWithEntities` errors; whether the worker warning is older than phase 8 wasn't checked.
- **History of applied edits, on every interface:** an applied grid edit is recorded in query history as the SQL Core built, with its `?` placeholders and no values (seen in the demo's baseline, `history-survives-reload`). Store the binds, or a display form with the values, next to the history row, so the history shows what ran.
- **The editor with many `{{p}}` uses** (Task 7 probe, item 9; pre-existing, every interface). Typing grows much faster than linearly with the number of `{{p}}` uses. Chromium, measured in the demo while builds ran (so the absolute times are high): inserting 500 uses took 14.7 s, 1,000 took 40.4 s, 2,000 took 102.8 s, and 4,000 crashed the tab. Running to the parameter dialog stays under 0.1 s, and `extractParameters` itself is linear (0.3 ms for 16,000 uses, Node). So the cost is in the editor's per-change work (decorations or the parameter UI), not in `$lib/sql`. Core's own 300,000-use run is refused by its budget as designed.
- **The website's `static/demo` in git**: each `demo:update` adds the build to the website's history; a release asset or a build step there would avoid that.
- **Dead TypeScript left by the twins:** `stripConnectionStringSecrets` (`connection-string-rules.ts`) has no caller outside its test, and nothing in the GUI calls the storage client's `queryHistory.append` any more.
- **`seaquel-storage`'s wasm tests aren't in CI** (`cargo test --target wasm32-unknown-unknown -p seaquel-storage --all-targets` under `wasm-bindgen-test-runner`); Checkpoint 8b ran them by hand.
