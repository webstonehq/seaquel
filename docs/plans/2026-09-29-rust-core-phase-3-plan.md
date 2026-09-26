# Phase 3 Implementation Plan: storage, secrets, infrastructure

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** Move app metadata storage, secrets, SSH tunnels, git and licensing out of the webview, the Node server and `src-tauri` into their own crates behind Core, with a workspace-scoped RPC that desktop and web share. The webview and the browser stop sending SQL to the metadata database. The legacy JSON import and `tauri-plugin-store` go. CI builds Core for `wasm32-unknown-unknown` with a `browser` feature set.

**Architecture:** Five new infrastructure crates: `seaquel-storage`, `seaquel-secrets`, `seaquel-ssh`, `seaquel-git` and `seaquel-license`. Core gets a `Workspace` that owns one user's storage and secret store. Desktop opens one at startup, and web opens one per user, keyed by the `X-Seaquel-User` header that Node sets over loopback. `seaquel-rpc` gets the workspace-level `Request`/`Response` the design doc describes, served as the `core_call` Tauri command and `POST /rpc` on `seaquel-server`. Only the variants for the moved pieces exist in this phase; the `db_*` commands and `/api/db/*` stay until phase 5. In TypeScript, a `StorageClient` interface replaces the `getDatabase()` handle. The Rust client serves desktop and web. The demo keeps the TS repositories over sql.js until phase 8, the same way it keeps `duckdb.ts`.

**Tech Stack:** Rust (sqlx 0.8 SQLite, keyring =3.6.3, russh 0.48, git2 0.19 vendored, reqwest, ed25519-dalek, axum 0.8, ts-rs 12), TypeScript/Svelte 5, vitest, Better Auth (unchanged), an OpenSSH test container.

**Inputs:**

- the design doc's "Core, workspaces and state", "Storage ownership", "Secrets", "The RPC surface for GUIs", "Web licensing over loopback" and "Constraints this puts on Core", plus decisions 7, 8, 9, 13 and 16;
- the "Phase 2b cost" and "AI safety cost" estimating notes;
- a map of today's code, taken on 2026-09-29.

The numbers below come from that map.

---

## Open questions

Each has a recommendation, and the plan is written as if the recommendation is taken. If one isn't, only the named tasks change.

**Answered (2026-09-29):** the owner took all three recommendations: two ship points, connection ownership in phase 5, and typed storage calls. Execution is subagent-driven.

**Part 1 checkpoint (2026-09-26):** full check list green; the owner's manual checks passed. Part 2 started.

1. **Ship in two parts?** Phase 3 touches five subsystems plus licensing and is the largest phase so far. **Recommendation: two ship points.**
   - **Part 1 (Tasks 1–9)** is storage, secrets, the workspace and the RPC. The app then ships with no SQL crossing from the webview to its own database.
   - **Part 2 (Tasks 10–15)** is SSH, git, licensing and the wasm32 build.

   Nothing in Part 2 depends on Part 1 beyond `Workspace` and the RPC plumbing from Tasks 6–7. If you'd rather ship once, nothing changes but the checkpoint after Task 9.

2. **Web connection scoping: move into Core now, or in phase 5?** Today the Node proxy prefixes connection ids with `userId:` (`shared/connection-scope.js`). That prefix is all of web tenant isolation for database connections. This phase gives Rust a notion of users (the `X-Seaquel-User` header) for storage anyway, so connection ownership could move to Core in the same step. **Recommendation: phase 5**, when `ConnectionService` moves into Core and `/api/db/*` becomes `/rpc`. Moving it now means reworking `/api/db/*` and the WebSocket proxy twice. The prefix check has held through three phases, and the header this phase adds is what phase 5 will build on.
3. **Typed storage calls or a SQL pass-through?** The cheap way to move storage is a Rust endpoint that runs the SQL the TS repositories already build. That keeps `/api/storage`'s shape and loses the point of the design's "the web client can't send arbitrary SQL anymore". **Recommendation: typed calls.**
   - Each repository method becomes one RPC call.
   - The SQL lives in `seaquel-storage`.
   - The 19 TS repositories stay only as the demo's backend.

   This is also the layer phase 5's domain services sit on, so it isn't throwaway.

## Decisions (2026-09-29)

### 1. The workspace and the RPC

```rust
// seaquel-core
pub struct WorkspaceSpec {
    pub data_dir: PathBuf,              // desktop: the app data dir; web: DATA_DIR/users/<id>
    pub secrets: Option<Arc<dyn SecretStore>>, // desktop: keychain; web: None
}
impl Core {
    pub async fn open_workspace(&self, spec: WorkspaceSpec) -> Result<Arc<Workspace>, CoreError>;
}
impl Workspace {
    pub fn storage(&self) -> &Storage;
    pub fn secrets(&self) -> Option<&dyn SecretStore>;
}

// seaquel-rpc
#[serde(tag = "method", content = "params")]
pub enum Request { Storage(StorageRequest), Secret(SecretRequest), Ssh(SshRequest), Git(GitRequest), License(DesktopLicenseRequest) }
pub async fn dispatch_workspace(core: &Core, ws: &Workspace, req: Request) -> Result<Response, RpcError>;
```

**Where it runs:**

- **Desktop.** `src-tauri` opens the workspace once in `setup` and keeps it in Tauri state. One new command, `core_call(request)`, dispatches onto it.
- **Web.**
  - `seaquel-server` serves `POST /rpc`. It reads `X-Seaquel-User` and validates it with the same rule as `src/lib/server/storage.ts` (no `/`, `\` or `..`, non-empty). It opens or reuses that user's workspace from an LRU of up to 1,024. Each workspace's pool holds at most 2 connections and closes idle ones after 60 s, so idle users don't hold file handles.
  - The new SvelteKit route `src/routes/api/rpc/+server.ts` requires `locals.user`, goes through `handleApiGate` like every `/api/*` route, and forwards the body to Rust with `X-Seaquel-User: <locals.user.id>`. It always sets the header itself and never forwards one from the client.
  - `/rpc` without the header is a 400.
  - The loopback-only bind becomes a documented security requirement in `crates/seaquel-server/src/main.rs` and CLAUDE.md, as the design says.
- **Errors** cross as `RpcError { code, message }`, the same shape as `DbError`, and TS maps them with the existing error helpers.

### 2. Storage

- **`seaquel-storage` owns the schema and the queries.** It has one module per table group and typed functions that mirror today's repositories (`connections::load_all`, `saved_queries::save_all(project_id, …)`, `project_state::save(…)`, …). Row types are serde structs exported with ts-rs, and their JSON matches today's `Persisted*` TS types field for field. Task 2 freezes those shapes.
- **Where the types live.** The row types, and the request and response types for secrets, SSH, git and licensing, live in `seaquel-types` (a `storage` module and so on), not in the infra crates. `seaquel-rpc` names them without depending on an infra crate, which Decision 12 forbids, and the design doc already puts DTOs such as `SavedQuery` there. The infra crates depend on `seaquel-types`.
- **Engine.** sqlx SQLite with WAL, `busy_timeout = 5000` and `foreign_keys = ON` on every connection. That matches both of today's backends. The desktop file stays `<dataDir>/seaquel.db`, and a web user's stays `DATA_DIR/users/<id>/meta.db`, so no data moves.
- **Batches stay batches.** Today's "transaction" is a list of statements with no reads in between. The Rust functions that replace them (`project_state::save`, `saved_queries::save_all`, `query_history::replace_all`, `ai_chats::replace_all_messages`) each run in one sqlx transaction.
- **Write order.** The TS write queues (`enqueueWrite` in `tauri-sqlite.ts` and `http-sqlite.ts`) guarantee that writes land in the order they were issued. `PersistenceManager` relies on that for its debounced saves, so `RustStorageClient` keeps one queue for writes. Reads skip it, as today.
- **The metadata DB leaves Core's connection registry.** Desktop no longer opens `seaquel.db` through `db_connect`, so it stops appearing as a user connection to `db_*`.

### 3. Schema and migrations

- **Baseline.** `schema::baseline(&mut conn)` ports `schema.ts`: `DDL_STATEMENTS`, the 26 inline `ADD COLUMN` upgrades, the `connection_id` to `project_id` moves, the `canvas` to `workflow` renames, and `migration.svelte.ts`'s v3 and v4 column adds.
  - It runs on every open in one transaction and is idempotent, like today's code.
  - Afterwards `schema_version`'s latest row is at least 4. Fresh files get a row with 4.
- **After the baseline**, `sqlx::migrate!("migrations")` runs numbered migrations. The directory starts empty, with a README. The next schema change is a numbered file, never another inline upgrade.
- **Every release's database upgrades.** Only `v2026.4.5-beta.1` wrote storage version 3; every other release with SQLite storage (`v2026.4.5` onwards) wrote 4. So no metadata database older than the baseline exists, and the TS data migrations for v2–v4 (`MigrationManager`) are deleted.
  - Task 2 freezes the schema each release created: `v2026.4.5-beta.1`, `v2026.4.5`, `v2026.4.8`, `v2026.9.1`, `v2026.9.2` and today's tree.
  - A test opens each through the baseline and compares the result with today's schema.

**Task 2 findings (2026-09-29), which change the above:**

- **Today's code can't open a `v2026.4.5-beta.1` file.** `upgradeSchema` adds columns before it creates tables, so it fails on `ALTER TABLE ai_messages …` for a table that beta.1 never had. This has been broken since `v2026.4.8`, and each launch leaves the file half-upgraded. The Rust baseline creates missing tables first and adds columns after, all in one transaction. A beta.1 file then upgrades to the same structure as a beta.1 file taken through `v2026.4.5`, which is `upgraded/v2026.4.5-beta.1-via-v2026.4.5.sql`. Test it with `upgrades/v2026.4.5-beta.1-data.json`, and check that the data survives.
- **Compare schemas by structure, not text.** Files from before `v2026.4.8` keep a different column order and different `CREATE` text. Files that started on beta.1 keep a nullable `project_id` with no foreign key on `saved_queries` and `dashboards`. That is behaviour to keep, not to repair.
- **Version pruning splits between TS and Rust.** `pruneOldVersions` rebuilds the oldest kept version from diff-match-patch patches, which count UTF-16 code units. The Rust ports count bytes or chars, so an emoji would come out differently. The TS keeps computing the promoted snapshot, in `utils/query-versions.ts` and `dashboard-versions.ts`, where the diffs are made. Storage only executes the result: `QueryVersionsPrune { saved_query_id, delete_ids, promote: Option<{ id, snapshot }> }`, and the same for dashboards, in one transaction. The quirks to keep: `keepCount` 0 does nothing for query versions but deletes every dashboard version. Moving diffing into Rust is phase 5's call.
- **Foreign keys are off in the demo after its first write.** sql.js's `export()` reopens the database with `PRAGMA foreign_keys = 0`, so nothing cascades in the demo. Task 8 fixes this in `web-sqlite.ts` by setting the pragma again after each export. The ten `demoDiffers` cases then agree with the other backends, and that is a deliberate change to those fixtures.

**Task 4 review findings (2026-09-29):**

- **Data steps, not SQL migrations, for data cleanups.** Migration 0001 as SQL was quadratic in row count and in string length, truncated at NUL bytes, and could never be edited once shipped. It became a Rust data step, which runs once from `open` after the migrator and is recorded in `_seaquel_data_steps`. It reuses `strip_connection_string_password` exactly. SQL migrations stay for schema changes.
- **JSON columns are `RawValue`,** so stored JSON stays byte-identical to what the TS wrote. `preserve_order` isn't an option: Cargo unifies features across the build, and the DuckDB decoder relies on sorted keys. Two consequences for Task 6:
  - `Request` must be adjacently tagged with `method` before `params`. Test that a body with `params` first fails clearly.
  - `core_call` must take the raw IPC body (`tauri::ipc::Request`, `InvokeBody::Raw`) and `serde_json::from_slice` it. Going through Tauri's `serde_json::Value` would sort the keys inside stored JSON on desktop only.
- **Consequences for Task 8's client:**
  - build a `Date` from a truthy `lastConnected`;
  - `toStorable` saved workflows on save, and `fromStorable` them on load, one at a time, dropping any that throw, as today;
  - map the generated `Persisted*` types to the app's existing types in `rust-client.ts`, and don't let two same-named types spread through the app;
  - check this against the Task 2 `connections` and `project-state` fixtures through the real TS code.
- **From the Task 6 review:**
  - **The request body.** Desktop sends `invoke("core_call", new TextEncoder().encode(JSON.stringify(request)))`. Build every request as `{ method, params }` so `method` comes first.
  - **Methods with no params.** For these, leave `params` out entirely. `{}` is rejected.
  - **Typing.** The generated `StorageRequest` and `StorageResponse` share the `method` discriminant, so `rust-client.ts` gets its param and result types from `Extract<…>`, without glue per method.

### 4. Legacy JSON and `tauri-plugin-store`

This is decision 16. `seaquel-storage::open` refuses with `LEGACY_STORAGE` when `seaquel.db` doesn't exist and any of `database_connections.json`, `projects.json` or `app_state.json` does. The message names the fix: install a release from 2026.4.5 through 2026.9.x, launch it once, then upgrade.

The desktop app shows it on a blocking error screen: a new `messages/en.json` key and a small component. It has no retry button, only a link to the releases page.

Deleted:

- `json-migration.ts`, `legacy.ts`, `tauri-storage.ts`, `web-storage.ts` and `types.ts`'s `Store` types;
- `PersistenceManager.loadLegacyConnectionState` and `removeLegacyConnectionState`;
- the `app_state['json_migration_done']` write;
- `tauri-plugin-store`, including its registration, capabilities and npm package;
- `@tauri-apps/plugin-fs`, if nothing else uses it (check).

### 5. Data dir

`seaquel_storage::data_dir(identifier)` reads `SEAQUEL_DATA_DIR` first, then falls back to `dirs::data_dir()/<identifier>`, which is what Tauri 2's `app_data_dir` resolves to on every platform. `src-tauri` passes `app.seaquel.desktop` or `app.seaquel.desktop.dev` from its config, so the dev split stays.

Tests pin the path per platform:

- macOS: `~/Library/Application Support/<id>`
- Linux: `$XDG_DATA_HOME` or `~/.local/share/<id>`
- Windows: `%APPDATA%\<id>`

In debug builds, `src-tauri` also asserts at startup that `app_data_dir` agrees. `get_data_dir` stays as a command because the UI shows the path, but it now calls this function.

### 6. Cross-process change events

The design's `data_version` polling and `StorageChanged` event are deferred to phase 4. Until `seaquel mcp` exists, no second process writes the file. Phase 4's plan owns them.

### 7. Secrets

- **`seaquel-secrets`** has a `SecretStore` trait (`get`, `set`, `delete`; `MaybeSend`, async) and a `KeychainStore { service }`.
  - `KeychainStore` uses `keyring = "=3.6.3"` with `apple-native`, `windows-native` and `sync-secret-service`: the exact version and features `tauri-plugin-keyring` 0.1.0 resolves today.
  - Calls run on a blocking thread, since a keychain prompt can block.
- **Existing entries stay readable.** The plugin is a pass-through: `Entry::new(service, user)` with the service `"app.seaquel.desktop"` (hard-coded in `keyring.ts`) and the key as the user. Same crate, same features, same process, so the entries read back unchanged.
  - Task 1 still proves it on each OS the owner has.
  - The service stays `app.seaquel.desktop` in dev builds too. Changing it would orphan every saved password, and today's dev builds already share it.
- **RPC.** `Secret::Get | Set | Delete { key }`.
  - Keys must match `db:<id>`, `ssh:<id>`, `ssh-key:<id>`, `license-key` or `ai-api-key:<id>`, and anything else is refused.
  - `get` returns `None` for a missing entry and an error for anything else. Today's JS returns `null` on every error, which hides a locked keychain; the TS wrapper keeps that behaviour for callers but logs the error.
- **Web.** The web workspace has no `SecretStore`, so `Secret::*` returns `NOT_SUPPORTED`. The vault stays in the browser. Its `vault_state` and `user_credentials` rows go through the typed storage calls like everything else.
- **Removed:** `tauri-plugin-keyring` and `tauri-plugin-keyring-api`, plus the unused single-key `ai-api-key` methods in `keyring.ts`.
- **Core doesn't read secrets on connect in this phase.** TS still fetches the password and passes it to `db_connect`. `ConnectionConnect { id, secrets }` is phase 5.

### 8. SSH

- **`seaquel-ssh`** takes over `ssh_tunnel.rs`: the `TunnelConfig`, the host-key check and trust-on-first-use (`UNKNOWN_HOST_KEY` with the fingerprint, then a retry with `trustHostKey` pinned to that fingerprint (see the Task 10 review findings below), and `HOST_KEY_MISMATCH` never auto-accepted), the 30 s connect timeout, and password or key-file auth.
  - The known_hosts path is a field (default `~/.ssh/known_hosts`) so tests use a temp file.
  - russh stays at 0.48. An upgrade is a follow-up, not part of a move.
- **Core owns the `TunnelManager`** behind an `ssh` feature. The RPC is `Ssh::Open { config } -> { tunnel_id, local_port }` and `Ssh::Close { tunnel_id }`.
- **Fix: closing now ends the tunnel.** Today, close only stops the accept loop. The russh session and any open channels stay up until they drop, so a DB client can keep using a "closed" tunnel. Close now disconnects the session and aborts in-flight forwards.
- **Unchanged:** `connection-manager.svelte.ts` keeps the tunnel lifecycle (connect, reconnect, test, disconnect). Moving it into `ConnectionService` is phase 5.
- **Removed:** `check_tunnel_status` and `list_active_tunnels`, which have no callers.
- **Web stays without SSH.** `features/index.ts` keeps it off there; see Follow-ups.

**Task 10 review findings (2026-09-26):**

- **The trust retry pins the fingerprint.** `trust_new_host_key: bool` became `trustHostKey: Option<String>`: the `SHA256:…` fingerprint the user approved in the prompt. The retry records an unknown key only if its fingerprint is exactly that one; any other key fails with `UNKNOWN_HOST_KEY` and its own fingerprint, and nothing is written. Before, the retry was a second connection that recorded whatever key it met, so an attacker who appeared between the prompt and the retry got recorded.
- **`/rpc` never opens tunnels.** `dispatch_workspace` answers `NOT_SUPPORTED` for `Ssh` unconditionally, as for Git and License. Keying it on the `ssh` feature wasn't enough: Cargo unifies features, and `cargo test --workspace` already built `seaquel-server` with real tunnels. Only the desktop's `dispatch_ssh` serves the group.
- **A key of a new algorithm is "unknown", not a mismatch.** russh's `check_known_hosts_path` compares only recorded keys of the presented key's algorithm. If known_hosts holds only an RSA key for a host and the server now presents Ed25519 (russh prefers it), the user sees a first-use prompt, not `HOST_KEY_MISMATCH`. Pinning the fingerprint doesn't change that: it guarantees the recorded key is the one the user was shown, not that the user was told the host was already known under another key. It still matters, but only as far as a user approves a fingerprint without checking it. Treating "other algorithms recorded" as a mismatch would break hosts first recorded by OpenSSH with a different preferred algorithm, so it stays as is. A prompt that says "this host is known under another key type" is a follow-up with the russh upgrade.

### 9. Git

- **`seaquel-git`** takes over `git.rs`: clone, init, pull (fetch plus merge, with conflicts), push, status (with ahead/behind), commit, resolve conflict, conflict content, set remote and remote URL. The credential chain is unchanged: agent, then the given key, then `~/.ssh/id_ed25519` and `id_rsa`, then user/password.
  - git2 is blocking, so each call runs on `tokio::task::spawn_blocking`. That's allowed in an infra crate; the no-spawn rule is for Core.
  - The home directory for default keys is a parameter, so tests don't touch `~/.ssh`.
- **Removed:** `git_stage_file` and `git_discard_file`, which have no callers.
- **Web stays without shared projects.**

### 10. Licensing

Two modules that share little, as the map found.

- **`seaquel_license::desktop`** is the activation client: `activate`, `validate` and `deactivate` on `/api/licenses/*`, with today's compile-time base URL rule and error codes. It has one request function instead of three copies.
  - The store and its 12-hour revalidation (`license.svelte.ts`) stay in TS. The license split doc keeps desktop honour-system and puts that store out of scope.
- **`seaquel_license::server`** covers:
  - the install id;
  - the control-plane client (`/api/cloud/*` with `X-Install-Id` and `X-License-Key`, the same contract);
  - the soft/hard TTL ladder (24 h and 14 d, same env overrides);
  - `member_license`;
  - air-gap bundle storage, verification (Ed25519 via `ed25519-dalek`, canonical JSON byte-identical to `canonical.ts`) and the offline equivalents in `local-control`.
- **`seaquel-server` serves `/internal/license/*`**, as the design lists:
  - `gate?user=<id>`
  - `signup-check`, `register-install`
  - `bind-member`, `unbind-member`
  - `members`
  - `airgap/status`, `airgap/upload`, `airgap/clear`

  These routes refuse any peer that isn't loopback (axum `ConnectInfo`), and neither `server.js` nor the SvelteKit proxy forwards `/internal/*`.

- **auth.db ownership in this phase.** Node keeps applying migrations 006–012 through `auth.ts`. They're frozen history, and no new license migration is written in this phase. Rust opens `auth.db` with sqlx (WAL, busy timeout) and owns every read and write of `member_license`, `install`, `install_cache` and `airgap_bundle`. When those tables are missing, Rust answers `NOT_READY` (503). That only happens if Rust is asked before Node has opened `auth.db` once.
- **What stays in Node:**
  - Better Auth;
  - session purging on revocation;
  - the signup route's order of steps, which now calls Rust for each licensing step;
  - the redirects in `(app)/+layout.server.ts`;
  - a 5-second per-user cache of the gate answer in `hooks.server.ts`, the burst cache the 2026-05-19 design wanted.

### 11. Features and the wasm32 build

- **Core's features:** `storage`, `secrets`, `ssh`, `git`, `license-desktop` and `license-server`, besides the engine features.
  - `src-tauri` enables `storage`, `secrets`, `ssh`, `git` and `license-desktop`.
  - `seaquel-server` enables `storage` and `license-server`. That keeps libgit2, OpenSSL and russh out of the Docker image.
  - `browser` enables none of them and no engines.
- **`seaquel-rpc` keeps every variant in every build** so the generated TS types don't depend on features. A variant whose feature is off returns `NOT_SUPPORTED`.
- **CI adds** `cargo clippy --target wasm32-unknown-unknown -p seaquel-core -p seaquel-rpc --no-default-features --features seaquel-core/browser -- -D warnings`.

### 11b. SQLite and DuckDB are off on web (2026-09-26)

The Task 13 review turned up a hole in the self-hosted web build that is older than this phase, and a probe confirmed it. A signed-in web user could open a SQLite or DuckDB connection on any server path. Neither Node nor Rust checked the engine or the path, and DuckDB's external access was on. That exposed:

- `auth.db`, which holds sessions, license keys and emails;
- every other user's `meta.db`;
- any file the server user can read, through DuckDB's `read_*`, `sqlite_scan` and `ATTACH`;
- file writes, through `COPY TO` and new SQLite files.

**Owner decision: disable both engines on web.** `seaquel-server` is built without `engine-sqlite` and `engine-duckdb`, so Core refuses the driver server-side. The web wizard hides both cards. The fix ships with phase 3, not as a separate patch. A confined per-user file area is a follow-up, if anyone asks for it.

**Task 14 findings (2026-09-26):**

- **Features alone don't hold the rule.** `seaquel-server` builds Core with `default-features = false` and only `engine-postgres`, `engine-mysql` and `engine-mssql`, but `cargo test --workspace` unifies features and compiles SQLite and DuckDB back in. So the server's Core registers engines by id: `seaquel_core::with_plugins(|id| WEB_ENGINES.contains(&id))`, with `WEB_ENGINES = ["postgres", "mysql", "mssql"]` (MariaDB connects as `mysql`). Core refuses any other driver on `/api/db/connect` and `/api/db/test` with `ENGINE_NOT_AVAILABLE` (now 400), whatever was compiled in; `tests/web_engines.rs` checks it in the unified build. Node's `/api/db/[...path]` refuses the same drivers first, with the same error body.
- **The web tutorial ran on the server's DuckDB.** `getDuckDBProvider()` returned the HTTP provider on web, so the query-builder tutorial opened an in-memory DuckDB on the server (which can read any file). It now uses DuckDB-WASM in the page on web, as the demo does. That loads the WASM bundle from jsDelivr, so an air-gapped self-hosted install has no tutorial.
- **The UI.** `sqliteSupport` and `duckdbSupport` are off on web. The wizard offers `availableDatabaseTypes()`, the connection-string parser refuses a `sqlite:`/`duckdb:` string with the reason, and `ConnectionManager.add`/`reconnect`/`test` throw it before any provider call, so a SQLite or DuckDB connection saved on desktop fails cleanly in a web workspace (`autoReconnect` shows it as an error toast). DBeaver/TablePlus import, file drop, deep links and shared projects are desktop-only already.
- **The image.** The stripped server binary went from 47.6 MB to 14.4 MB and no longer links libstdc++ (DuckDB). Neither build linked OpenSSL, so the runtime stage drops `libssl3`.

### 12. Crate rules

`scripts/check-crate-deps.mjs` puts the five new crates in `DOMAIN_AND_INFRA`, which may depend on anything except engine crates. Interfaces still reach them only through Core, per rule 3, so `seaquel-server`'s license routes call `core.license_server()`, not `seaquel-license` directly. The script gets a case for that.

### 13. Bugs fixed on the way

1. **A connection string that isn't a URL is saved with its password.** `stripPasswordFromConnectionString` returns an ADO-style `Server=…;Password=…` string unchanged when URL parsing fails, and it goes into `connections.connection_string` in plaintext. `connections::save` in Rust now removes `password` and `pwd` pairs, case-insensitively, from key=value strings, and removes passwords and `password`/`pwd` query parameters from URL strings that parse. A password inside a URL that doesn't parse (`postgres://u:p#w@h/db`) survives, as it does today. Test both formats, and check that an existing row is cleaned the next time it's saved. (Rows already on disk are cleaned once by a Rust data step, not a SQL migration; see the Task 4 review findings.)
2. **A closed SSH tunnel keeps forwarding** (Decision 8).
3. **`/api/storage/*` accepts any `SELECT`/`INSERT`/`UPDATE`/`DELETE`/`WITH` the browser sends** against the user's own database. That's not a cross-user hole, but it is a wide surface; the routes are deleted.

---

## What moves

| From                                                                                                                                     | To                                                | Stays for the demo    |
| ---------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- | --------------------- |
| `src/lib/storage/schema.ts` (532), `repos/*` (19), `create-repo.ts` (244)                                                                | `seaquel-storage`                                 | yes, until phase 8    |
| `storage/tauri-sqlite.ts` (103), `http-sqlite.ts` (106), `db.ts` (96)                                                                    | `RustStorageClient` over `core_call` / `/api/rpc` | `web-sqlite.ts` (112) |
| `json-migration.ts` (321), `legacy.ts` (46), `tauri-storage.ts` (69), `web-storage.ts` (126), `hooks/database/migration.svelte.ts` (312) | deleted (Decisions 3–4)                           | —                     |
| `src/lib/server/storage.ts` (165), `storage-guard.ts` (55) + test, `routes/api/storage/*` (91)                                           | deleted                                           | —                     |
| `services/keyring.ts` (Tauri half)                                                                                                       | `seaquel-secrets`                                 | —                     |
| `src-tauri/src/ssh_tunnel.rs` (455)                                                                                                      | `seaquel-ssh`                                     | —                     |
| `src-tauri/src/git.rs` (890)                                                                                                             | `seaquel-git`                                     | —                     |
| `src-tauri/src/license.rs` (171)                                                                                                         | `seaquel_license::desktop`                        | —                     |
| `src/lib/server/{licensing,license-cache,member-license,install}.ts` (537), `airgap/*` (966)                                             | `seaquel_license::server`                         | —                     |

The Tauri commands that go are the 12 `git_*`, the 4 `ssh_*`, the 3 license commands and `greet`, which has no caller: 20 of 39. `db_transaction` loses its only caller, the desktop storage backend, but stays: `DatabaseProvider` exposes it, and phase 5 revisits the `db_*` set as a whole. `core_call` is added.

`db_*` stays until phase 5. The import readers (`read_dbeaver_config`, `read_tableplus_config`) and `get_username` go with `seaquel-workspace` in phase 5. The rest are interface code that stays: clipboard, `open_path`, logs, updater, `get_data_dir`.

---

## Ground rules for whoever executes this

- **No git writes.** The owner forbids `git add`, `commit`, `mv` and `stash`, branches and worktrees. Use plain `mv`, `cp` and `rm`. Each task ends with a checkpoint: summarise the changes and the verification, then let the user review. Read-only git is fine, including `git show <tag>:<path>` for Task 2.
- **Repo conventions:**
  - Run everything from the repo root.
  - Never edit `src/lib/components/ui/*`.
  - Error toasts use `errorToast`.
  - Run the Svelte MCP `svelte-autofixer` on every changed `.svelte` file, and oxfmt on changed TS.
- **Crate rules:**
  - Core may not use `tokio::spawn`, `Instant` or `SystemTime` (`crates/clippy.toml`).
  - Infra crates are native-only and may use `spawn_blocking`.
  - A clock or timer Core needs (the gate cache TTL, idle pool timeouts) goes through `Executor` or lives in the infra crate.
- **Parallel agents.**
  - Each agent owns the files its task names and uses its own `CARGO_TARGET_DIR` outside the repo.
  - Don't run `cargo fmt --all` while others are editing; run rustfmt on your own files.
  - If the build fails in a file you don't own, wait and rerun; don't edit it.
  - Re-read the plan and the effort log right before editing them.
- **TDD.** Write the failing test first. For a moved module, port its behaviour's tests before its code: the Task 2 fixtures, the ported `airgap` vitest cases, a live SSH or git case.
- **Secrets never reach logs.** No `Debug` on a type that holds a password, key, passphrase or license key without redaction. Logging a request's params is forbidden in `dispatch_workspace`. Add a test for the redaction.
- **User data.** Any test that opens a real data dir, keychain or `~/.ssh` is forbidden. Use temp dirs, a test keychain service name (`app.seaquel.test.<random>`) and temp known_hosts files. The one exception is Task 1's manual keychain check, which the owner runs.
- **User-facing strings.** Any new `messages/en.json` key is translated with the `i18n-translator` agent before the checkpoint.
- **Full check list before every checkpoint:**
  - `npm run crates:check`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`
  - the wasm32 clippy line from `ci.yml`, plus Decision 11's new line from Task 14 onward
  - `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, run with all four `SEAQUEL_TEST_*` variables, `SEAQUEL_TEST_SSH` from Task 10 onward, and `SEAQUEL_TEST_REQUIRE_ENGINES=1`
  - `cargo check -p seaquel`
  - `npm run wasm:build`
  - `npm run check`
  - `CI=1 npx vitest run`
  - `npx oxlint --type-aware --type-check --deny-warnings`
  - `npm run build`
  - `NODE_OPTIONS=--max-old-space-size=8192 npm run build:web`
  - `npm run build:demo`
- **Effort log.** Every implementer appends one line per task to `docs/plans/2026-09-29-phase-3-effort.md`: task, wall time, Rust lines added, TS lines removed, and surprises.

---

## Order and estimates

| #   | Task                                                                     | Estimate     | Needs   | Can run alongside |
| --- | ------------------------------------------------------------------------ | ------------ | ------- | ----------------- |
| 1   | Crates, rules, keychain check                                            | 0.75–1 h     | —       | 2                 |
| 2   | Freeze the TS storage baseline                                           | 1–1.5 h      | —       | 1                 |
| 3   | `seaquel-storage`: open, data dir, baseline, migrations                  | 1.5–2 h      | 1, 2    | 5                 |
| 4   | `seaquel-storage`: the typed queries                                     | 3–4 h        | 3       | 5                 |
| 5   | `seaquel-secrets`                                                        | 0.75–1 h     | 1       | 3, 4              |
| 6   | `Workspace`, `Request`, `dispatch_workspace`, `core_call`                | 1.5–2 h      | 3, 5    | 4                 |
| 7   | Web: `/rpc`, `X-Seaquel-User`, `/api/rpc`                                | 1.5–2 h      | 6       | 4                 |
| 8   | TS: `StorageClient`, switch call sites, secrets, deletions               | 3–4 h        | 4, 6, 7 | —                 |
| 9   | Legacy refusal screen, Part 1 checkpoint                                 | 0.75–1 h     | 8       | —                 |
| 10  | `seaquel-ssh` and tunnels in Core                                        | 1.5–2 h      | 6       | 11, 12            |
| 11  | `seaquel-git`                                                            | 1.5–2 h      | 6       | 10, 12            |
| 12  | `seaquel_license::desktop`                                               | 0.5 h        | 6       | 10, 11            |
| 13  | `seaquel_license::server`, `/internal/license/*`, Node switch            | 4–5 h        | 7       | 10–12             |
| 14  | Core `browser` feature, wasm32 CI, CI and Docker                         | 1–1.5 h      | 10–13   | —                 |
| 15  | Docs, measure, Part 2 checkpoint                                         | 0.75–1 h     | 14      | —                 |
|     | Review fixes (40%, as the cost notes recommend where code holds secrets) | 9–12 h       |         |                   |
|     | **Total**                                                                | **~33–43 h** |         |                   |

Going by the last two phases, first passes run at about half their estimate. SQL Server was the exception, where the planned mechanism was wrong. That would put roughly 12–15 h of first passes plus 5–7 h of fixes, around **17–22 h logged**. The riskiest estimates are Task 4 (19 repositories whose JSON must match exactly) and Task 13 (licensing has the most branches, and a mistake there locks users out).

---

## Part A — Groundwork

### Task 1: Crates, rules, keychain check

**Files:**

- Create: `crates/seaquel-{storage,secrets,ssh,git,license}/{Cargo.toml,src/lib.rs}`, each with a one-paragraph crate doc.
- Modify: the root `Cargo.toml` (members; workspace deps `keyring = "=3.6.3"`, `russh`/`russh-keys = "0.48"`, `git2 = "0.19"`, `reqwest`, `ed25519-dalek`, `dirs`), `scripts/check-crate-deps.mjs` (Decision 12), `crates/seaquel-core/Cargo.toml` (the optional features from Decision 11, off by default for now)
- Create: `docs/plans/2026-09-29-phase-3-effort.md`

**Steps:**

1. **Crate rules first.** Extend `check-crate-deps.mjs` with the five crates and a rule that interface crates may not name them. Write a case that fails today:
   - make a temp copy of the metadata JSON with `seaquel-server` depending on `seaquel-license`;
   - the check must reject it;
   - see how the script is tested, or add a `--self-test` flag if it has none.
2. **Crates.** Add the five crates as empty libraries with their dependencies declared. `cargo check` should pass, and `npm run crates:check` too.
3. **Keychain check** (`crates/seaquel-secrets/tests/keychain.rs`, `#[ignore]` unless `SEAQUEL_TEST_KEYCHAIN=1`):
   - write, read, overwrite and delete a random key under a random service with `keyring::Entry`;
   - read a missing key and expect `NoEntry`.

   It's ignored by default because CI runners have no keychain. The owner runs it on macOS. It proves the pinned crate and features work.

4. **Manual check for the owner.** Save a connection password in today's build, then check after Task 8 that the new build connects without asking. This is the compatibility check that matters; the plugin source makes it near certain (Decision 7). List it under Manual checks.

### Task 2: Freeze the TS storage baseline

The TypeScript is the spec. Record what it does before anything replaces it, as phase 2b did with the parser.

**Files:**

- Create: `scripts/freeze-storage-baseline.mjs` (it runs once; keep it in `docs/plans/artifacts/` afterwards, as phase 2 did with the parity recorder)
- Create: `crates/seaquel-storage/tests/fixtures/schemas/{v2026.4.5-beta.1,v2026.4.5,v2026.4.8,v2026.9.1,v2026.9.2,current}.sql`
- Create: `crates/seaquel-storage/tests/fixtures/repos/*.json`
- Create: `crates/seaquel-storage/tests/fixtures/README.md`: the fixtures are frozen, how they were made, and when to change one (only when the behaviour is meant to change, with the reason)

**Steps:**

1. **Schemas.** For each tag, `git show <tag>:src/lib/storage/schema.ts` and the files it imports into a temp dir. Run its `initializeSchema` on an empty better-sqlite3 database the way `db.ts` did at that tag, including its `CURRENT_STORAGE_VERSION` row. Dump `.schema` plus the `schema_version` rows.
   - For `current`, also run `upgradeSchema` on each old dump and check that the result equals `current`. That shows the TS upgrade path is complete, before Rust copies it.
   - Record any difference in the README. For example, if `saved_queries.project_id` ends up nullable without an FK on upgraded files but not on fresh ones, that difference is behaviour to keep.
2. **Repositories.** Run each of the 19 repositories against a fresh `current` database: save representative rows (every nullable, bool and JSON column both set and unset, unicode, empty arrays), then load them back.
   - Record the input objects, the rows as stored (`SELECT *`) and the loaded objects.
   - Cover the batch methods (`project_state.save` with tabs and canvases, `saved_queries.save_all` replacing an earlier set, `query_history.replace_all`, `ai_chats.replace_all_messages`, `query_versions.prune_old_versions`) and cascades (deleting a project, connection or saved query).
   - About 60 cases.
3. **Row shapes.** Record the `Persisted*` TS types each repository returns, as a list of fields, optionality and JSON types. Task 4's ts-rs types must regenerate to the same thing.

## Part B — Storage and secrets

### Task 3: `seaquel-storage`: open, data dir, baseline, migrations

**Files:**

- Create: `crates/seaquel-storage/src/{lib.rs,open.rs,data_dir.rs,schema.rs}`, `migrations/README.md`, `migrations/0001_strip_connection_string_passwords.sql` (its content lands in Task 4), `tests/{baseline.rs,open.rs}`

**Steps:**

1. **Failing tests:**
   - `baseline.rs`: for each schema fixture, load it into a temp file, run `Storage::open`, then compare the normalised schema with `current.sql`. Normalise by sorting `sqlite_master` and comparing `PRAGMA table_info`/`foreign_key_list`/`index_list` per table, so whitespace in stored DDL doesn't matter. Opening twice changes nothing.
   - `open.rs`: covers the cases in step 4 below.
   - `data_dir.rs` unit tests: covers `SEAQUEL_DATA_DIR`, plus the per-platform paths from Decision 5, gated by `cfg(target_os)`.
2. **Baseline.** Port `DDL_STATEMENTS` and `upgradeSchema` statement by statement, keeping the order. Add `migrateToV3`'s and `migrateToV4`'s column adds (they're idempotent). Check for columns with `PRAGMA table_info`, the same as the TS.
3. **Pool.** `Storage::open(path, OpenOptions { max_connections, idle_timeout })` creates the file if it's missing, sets the pragmas from Decision 2 on every connection (`after_connect`), runs the baseline and then `sqlx::migrate!`.
4. **What `open` must handle:**
   - a fresh file gets `schema_version` 4;
   - an existing v3 file ends at 4;
   - legacy JSON files and no DB give `LEGACY_STORAGE` (Decision 4);
   - legacy JSON files next to an existing DB are fine and are ignored;
   - a file that isn't SQLite gives `STORAGE_CORRUPT`, and the file is left untouched;
   - a DB whose baseline fails is left unchanged, because the baseline is one transaction. Test this with a fixture that has a conflicting column.
5. **`data_dir(identifier)`** per Decision 5.

### Task 4: `seaquel-storage`: the typed queries

**Files:**

- Create: `crates/seaquel-storage/src/{types.rs, queries/<one per repo>.rs}` and `tests/repos.rs`
- Modify: `migrations/0001_…sql`, `package.json`'s `types:gen` (add `seaquel-storage`)

**Steps:**

1. **Failing test.** `tests/repos.rs` replays every Task 2 case: same inputs, and the same stored rows and loaded objects. Compare loaded objects as JSON values, so field order doesn't matter but presence does.
2. **Types.** In `crates/seaquel-types/src/storage.rs` (Decision 2), write the serde structs with `#[serde(rename_all = "camelCase")]` where the TS uses camelCase. Keep the TS's null and undefined distinction: `Option` with `skip_serializing_if` where the TS omits a field, and a plain `Option` where it sends `null`. Derive ts-rs behind the `ts` feature and export to `src/lib/types/generated/`. Check the generated files against Task 2's row shapes.
3. **Queries.** Port the 19 repositories, one module each, porting the SQL exactly: the `ON CONFLICT DO UPDATE` upserts, the column codecs (`bool` as 0/1, `json` via `serde_json`, `safeJsonParse` falling back to the default on bad JSON as today), and each batch in one transaction.
4. **Password stripping** (Decision 13.1) in `connections::save`, with its migration. Test both string formats, and that the migration cleans an old row while leaving URL-form strings without passwords alone.
5. **No `Debug` leaks.** No stored type carries a secret. `user_credentials` holds ciphertext, but still give it a redacted `Debug`.

### Task 5: `seaquel-secrets`

**Files:**

- Create: `crates/seaquel-secrets/src/{lib.rs,keychain.rs,memory.rs}` and `tests/store.rs`

**Steps:**

1. **Failing tests.** Test `MemoryStore`, the test double Core's tests will use, and run the same suite against `KeychainStore` when `SEAQUEL_TEST_KEYCHAIN=1`. The suite covers:
   - get missing;
   - set, get, overwrite, delete;
   - delete missing is not an error;
   - UTF-8 values;
   - an empty value.
2. **Implement.** `KeychainStore::new(service)` with Decision 7's crate and features. Each call goes through `spawn_blocking`. Map `keyring::Error::NoEntry` to `None`, and every other error to a `SecretError` whose message names the key but never the value.
3. **Key validation.** `validate_key(&str)` has its own unit tests. It lives here, not in the RPC, so every interface gets it.

### Task 6: `Workspace`, `Request`, `dispatch_workspace`, `core_call`

**Files:**

- Modify: `crates/seaquel-core/src/lib.rs`. Split it into modules if it passes about 800 lines: `workspace.rs` and `lib.rs`.
- Modify: `crates/seaquel-rpc/src/lib.rs` (the new `workspace.rs` module) and `src-tauri/src/lib.rs` (open the workspace in `setup`; the `core_call` command; drop `greet`)
- Create: `crates/seaquel-core/tests/workspace.rs` and `crates/seaquel-rpc/tests/workspace.rs`

**Steps:**

1. **Failing tests:**
   - Opening a workspace on a temp dir works, and a second workspace on another dir is independent.
   - `Request::Storage(...)` round-trips a connection save and load through `dispatch_workspace`.
   - `Secret::Get` on a workspace without a store gives `NOT_SUPPORTED`.
   - A bad secret key gives `INVALID_ARGUMENT`.
   - `Request` JSON matches the generated TS shape, as a snapshot of `serde_json::to_string` for one variant per group.
2. **Implement.** Implement Decision 1. `StorageRequest` has one variant per query function, named `<Repo><Method>` (`ConnectionsLoadAll`, `ProjectStateSave`, …), and its responses are typed the same way.
3. **Desktop.** `setup` opens the workspace from `data_dir` and puts it in state. If that fails with `LEGACY_STORAGE` or `STORAGE_CORRUPT`, the app still starts, and `core_call` returns that error to every storage call so Task 9's screen can show it. `core_call` is `async fn core_call(request: Request, state) -> Result<Response, RpcError>`.
4. **Logging.** `dispatch_workspace` logs the method name only, never params (see Ground rules). Add a test with a capturing logger that a `Secret::Set` value doesn't appear.

### Task 7: Web: `/rpc`, `X-Seaquel-User`, `/api/rpc`

**Files:**

- Create: `crates/seaquel-server/src/routes/rpc.rs`, `crates/seaquel-server/tests/rpc.rs` and `src/routes/api/rpc/+server.ts` with a test
- Modify: `crates/seaquel-server/src/{lib.rs,main.rs}`, `server.js` (pass `DATA_DIR` to the child if it doesn't already; check)

**Steps:**

1. **Failing tests** (Rust, axum router tests):
   - no header gives 400;
   - a header with `..` or `/` gives 400;
   - two users' saves don't see each other;
   - a user's file lands at `DATA_DIR/users/<id>/meta.db`;
   - the workspace LRU evicts and reopens cleanly: set the cap to 2 in the test and use three users.
2. **Implement** `POST /rpc` per Decision 1, with `AppState` holding the LRU. The server's workspaces get no `SecretStore`.
3. **Node route.** `src/routes/api/rpc/+server.ts`: 401 without `locals.user`; it forwards the body with the header set from `locals.user.id`, and a client-sent `X-Seaquel-User` is dropped. A vitest with the fetch stubbed checks the header is overwritten.
4. **Existing data.** Point a dev web server at a `DATA_DIR` made by today's build, with users that have saved queries. Everything loads. This is a manual check, and it's in the list.

### Task 8: TS: `StorageClient`, switch call sites, secrets, deletions

**Files:**

- Create: `src/lib/storage/client.ts` (the `StorageClient` interface: one property per repository, with the same method names minus the `db` argument), `rust-client.ts` (`core_call` on desktop, `/api/rpc` on web, and one write queue), `sqljs-client.ts` (the demo: wraps the existing repositories over `web-sqlite.ts`) and `client.test.ts`
- Modify:
  - `src/lib/storage/{db.ts,index.ts}`: `getStorage()` returns a `StorageClient` and replaces `getDatabase()`
  - the ~160 call sites in the 23 files the map lists
  - `services/keyring.ts`: `TauriKeyringService` calls `core_call` `Secret::*`
- Delete: every file in Decisions 3 and 4 and the "What moves" table that isn't the demo's, `src/lib/server/storage.ts`, `storage-guard.ts` and its test, and `routes/api/storage/*`
- Modify: `src-tauri/Cargo.toml` and `src-tauri/src/lib.rs` (drop `tauri-plugin-store` and `tauri-plugin-keyring`), `src-tauri/capabilities/*.json` (drop their permissions), `package.json` (drop `@tauri-apps/plugin-store` and `tauri-plugin-keyring-api`; drop `@tauri-apps/plugin-fs` if unused)

**Steps:**

1. **Failing tests.**
   - `client.test.ts` runs one round-trip per repository against both clients. The Rust client runs against a fake transport that replays the Task 2 fixtures; the sql.js client runs for real. The two must agree.
   - A write-order test issues save A then save B without awaiting, and the transport sees A before B.
2. **The switch.** `repo.method(db, …)` becomes `storage.repo.method(…)`. Do it mechanically, file by file, and run `npm run check` after each. `PersistenceManager`, `migration.svelte.ts` (deleted) and the stores are the bulk.
3. **Delete** the files, routes and dependencies. `npm run check`, vitest and the three builds must pass. Grep for leftovers: `getDatabase(`, `plugin-store`, `plugin-keyring`, `/api/storage`.
4. **Update the existing tests** that mock `$lib/storage` (`persistence-manager`, `dashboard-manager`, `ui-state`, `license-nudge`) to mock `getStorage`.

### Task 9: Legacy refusal screen; Part 1 checkpoint

**Files:**

- Create: `src/lib/components/storage-error-screen.svelte`
- Modify: the root layout, to show it when the first storage call returns `LEGACY_STORAGE` or `STORAGE_CORRUPT`, and `messages/en.json` (two keys, then translate)

**Steps:**

1. **A vitest** on the component's logic: which error shows what, and that other errors don't block the app.
2. **Implement.** The legacy message names the fix from Decision 4 and links to the GitHub releases page. The corrupt message gives the file path and says the file wasn't changed.
3. **Checkpoint:** run the full check list and the Part 1 manual checks. If the owner took open question 1, Part 1 ships here.

## Part C — SSH, git, licensing

### Task 10: `seaquel-ssh` and tunnels in Core

**Files:**

- Create: `crates/seaquel-ssh/src/{lib.rs,tunnel.rs,host_key.rs}` and `tests/tunnel.rs`
- Modify: `e2e/test-databases/docker-compose.yml`. Add `linuxserver/openssh-server` on port 2222 with password and key users, and `AllowTcpForwarding yes`, plus a fixture key pair under `e2e/test-databases/ssh/`. Also modify `.github/workflows/ci.yml` (the service container and `SEAQUEL_TEST_SSH`), Core (the `ssh` feature and the `TunnelManager`), `seaquel-rpc` (`Ssh::*`) and `src/lib/services/ssh-tunnel.ts` (call `core_call`).
- Delete: `src-tauri/src/ssh_tunnel.rs` and its four commands

**Steps:**

1. **Failing live tests** against the container. Tunnel to the Postgres test container through SSH and run `SELECT 1`. Cover:
   - password auth;
   - key auth;
   - key auth with a passphrase;
   - a wrong password gives the same code as today;
   - an unknown host gives `UNKNOWN_HOST_KEY` with a fingerprint, then trust, then a second connect with no prompt (temp known_hosts);
   - a changed host key gives `HOST_KEY_MISMATCH` (swap the known_hosts line);
   - **close ends a live forward:** open a DB connection through the tunnel, close the tunnel, and the next query fails. This fails against today's code (Decision 8).
2. **Port** the module, taking the Tauri state out. Keep the error codes identical, because the TS host-key prompt matches on them.
3. **Core and the RPC:** `TunnelManager` in Core under the `ssh` feature, with Open/Close. On Core drop, every tunnel closes.

### Task 11: `seaquel-git`

**Files:**

- Create: `crates/seaquel-git/src/{lib.rs,ops.rs,credentials.rs}` and `tests/ops.rs`
- Modify: Core (the `git` feature), `seaquel-rpc` (`Git::*`), `src/lib/services/git.ts` (call `core_call`; drop `stageFile` and `discardFile`)
- Delete: `src-tauri/src/git.rs` and its commands

**Steps:**

1. **Failing tests** on local repos: a bare repo in a temp dir as `origin` over `file://`.
   - clone, commit and push, then clone again and see the commit;
   - pull fast-forward;
   - pull with a conflict: the conflict is listed, the conflict content comes back, resolve, commit, then push;
   - status and ahead/behind after each;
   - set remote and get remote URL;
   - init;
   - the signature fallback when no git config exists (set `HOME` to a temp dir for the test process, or pass the config explicitly).
2. **Credentials.** Unit-test the callback's order with a fake: agent only when the URL has a user, then the given key, then the defaults in the passed home dir, then user and password.
3. **Port** the module. Every public function is async over `spawn_blocking`. Keep the `SyncResult` and `RepoStatus` JSON identical: snapshot them against the TS types in `services/git.ts`.

### Task 12: `seaquel_license::desktop`

**Files:**

- Create: `crates/seaquel-license/src/desktop.rs` and `tests/desktop.rs`
- Modify: Core (the `license-desktop` feature), `seaquel-rpc` (`License::Activate | Validate | Deactivate`), `src/lib/api/tauri.ts`
- Delete: `src-tauri/src/license.rs` and its commands

**Steps:**

1. **Failing tests** against a local axum fake of `/api/licenses/*`:
   - request bodies match today's (`{key, instance_name}` and `{key, instance_id}`);
   - responses parse;
   - each error code: network, 4xx with a body, bad JSON.
2. **Port** it with the base URL passed in (the compile-time rule lives in `src-tauri`) and one request function.

### Task 13: `seaquel_license::server`, `/internal/license/*`, Node switch

**Files:**

- Create: `crates/seaquel-license/src/server/{mod.rs,install.rs,cache.rs,member.rs,control.rs,airgap/{verify.rs,canonical.rs,bundle_store.rs,local_control.rs}}` and `tests/{airgap_verify.rs,bundle_store.rs,local_control.rs,routing.rs,ladder.rs}`, plus `crates/seaquel-server/src/routes/internal_license.rs` and its tests
- Modify:
  - `src/hooks.server.ts` (the gate calls Rust, with a 5-second cache);
  - `src/routes/api/{signup,team,team/[containerUserId],airgap/bundle}/+server.ts` and `(app)/revalidate`, `(app)/settings/airgap` (call Rust);
  - `server.js` and the SvelteKit proxies (never forward `/internal/*`)
- Delete: `src/lib/server/{licensing,license-cache,member-license,install}.ts` and `airgap/*`, together with the tests the Rust ones replace. Keep `api/airgap/e2e.test.ts` and `signup.test.ts`, updated to the new client (see step 4).

**Steps:**

1. **Port the tests first:**
   - `verify.test.ts`'s golden vectors (seed, pubkey, fingerprint, canonical JSON, payload, signature) become `airgap_verify.rs` word for word. These vectors are shared with seaquel-app, so they can't change.
   - `bundle-store.test.ts` and `local-control.test.ts` port 1:1 over a temp auth.db built from migrations 006–012.
   - `licensing.test.ts`'s routing cases (online vs air-gap, `isNetworkFailure`) run against a fake control plane.
   - New `ladder.rs` covers: unregistered, soft-fresh without a network call, stale then success, stale then failure within grace, stale then failure after grace (`revalidate`), and suspended. No TS test covers this today.
2. **Canonical JSON must match `canonical.ts` byte for byte:** sorted keys, no whitespace, JS number formatting (use `ryu-js`, already in the workspace), and JS string escaping. Test it against the golden vectors, plus a case with non-ASCII text and a control character.
3. **Routes.** Add `/internal/license/*` with the loopback check. Test that a non-loopback peer gets 403: call the router with a non-loopback `ConnectInfo`.
4. **The Node switch.** Each route calls Rust through one small `src/lib/server/license-client.ts`.
   - `api/airgap/e2e.test.ts` keeps its end-to-end cases by starting the Rust server binary on a temp `DATA_DIR`: the first vitest that needs a Rust process.
   - If that's too heavy for `vitest run`, move those cases into `crates/seaquel-server/tests/` and keep only the Node-side assertions (session purge, redirects).
5. **Manual check:** run the web build against a staging or fake control plane, sign up the first owner, add a member, then revoke, suspend and upload an air-gap bundle. It's in the list.

## Part D — Close-out

### Task 14: Core `browser` feature, wasm32 CI, CI and Docker

**Files:**

- Modify: `crates/seaquel-core/Cargo.toml` (`browser`; `uuid` with `js` on wasm32), `crates/seaquel-rpc/Cargo.toml`, `.github/workflows/ci.yml` (the wasm32 line; the SSH service; the new crates in the test run), `Dockerfile` (the server's features; confirm git2 and russh are gone from the image build), `.github/workflows/release.yml` (nothing expected; check)

**Steps:**

1. **Wasm32 build.** Run Decision 11's wasm32 line locally; it fails until the native pieces are gated. Gate them.
2. **Confirm features in the builds.** `cargo tree -p seaquel-server -e features` shows no git2, russh or keyring. Record the Docker build time before and after in the effort log.
3. **Check** that CI's `rust` job runs the new crates' tests, and that the `engines` job gets the SSH container.

### Task 15: Docs, measure, Part 2 checkpoint

- **CLAUDE.md:**
  - the storage rule (UI code reads and writes metadata only through `getStorage()`, and SQL for the metadata DB lives in `seaquel-storage`);
  - the numbered-migrations rule;
  - secrets through Core;
  - the web `X-Seaquel-User` and loopback requirement;
  - `/internal/*`;
  - update the Tauri command list and the Backend crate list.
- **Design doc:** a status line and a "Phase 3 cost" section in the earlier format: time per task against this plan, lines, bugs by source, and what was harder than expected.
- **This plan:** execution notes, in phase 2b's format.
- **Release notes:**
  - versions before 2026.4.5 must upgrade through a 2026.4.5–2026.9.x release first;
  - saved passwords, SSH settings and shared repos carry over;
  - closing an SSH tunnel now closes it;
  - connection strings in key=value form no longer keep a password on disk;
  - self-hosters: none. Make sure the release notes say it, since data files and env are unchanged.
- **Effort log:** the totals.
- **Checkpoint.**

**Status (Task 15):** done. CLAUDE.md, README.md, the design doc's status line, its as-built notes and "Phase 3 cost", the execution notes and release notes below and the effort log's totals are written. Two items above changed: self-hosters do have changes (Decision 11b, the environment allow-list, proxies, the 503 page; see the release notes), and CLAUDE.md's release steps name the root `Cargo.lock`, since `src-tauri/Cargo.lock` is ignored by Cargo (Follow-ups). The full check list passed. The Part 2 manual checks below are not run yet.

---

## Manual checks

For the owner. Desktop is `npm run tauri:dev`, web is `npm run dev:web:full`, and the demo is `npm run dev:demo`.

**Part 1 (after Task 9):**

- **Upgrade in place, desktop.** Run today's release once with some projects, connections with saved passwords, saved queries, dashboards, history and AI chats. Then start the new build on the same data dir: everything is there, and connections open without asking for a password (Task 1's keychain check).
- **Upgrade in place, web.** Run the same check against a `DATA_DIR` from today's web build, with two users. Each sees only their own data.
- **Legacy refusal.** Move `seaquel.db` away and drop an old `projects.json` in the data dir: the refusal screen shows and nothing is created. Put `seaquel.db` back and the JSON file is ignored.
- **Demo.** Save a query, reload, and it's still there (sql.js path).
- **Editing.** Tabs, split panes, starred items and a theme change all persist across a restart.

**Part 2 (after Task 15).** Not run yet. The test containers are `npm run e2e:db:up`; the SSH one is user `seaquel`, password `seaquel-test-password`, on `127.0.0.1:2222`, and reaches Postgres as `postgres:5432`.

- **SSH trust prompt (desktop).** Add a Postgres connection through the SSH container (or a real bastion not yet in `~/.ssh/known_hosts`). The prompt shows a `SHA256:` fingerprint; it matches `ssh-keyscan -p 2222 127.0.0.1 2>/dev/null | ssh-keygen -lf -`. Approve: it connects, and `grep '127.0.0.1\]:2222' ~/.ssh/known_hosts` shows the key. Reconnect: no prompt. Change a character of that key in `known_hosts` and reconnect: a host-key mismatch error and no prompt. Remove the line afterwards.
- **SSH close (desktop).** While connected, `lsof -nP -iTCP -sTCP:LISTEN | grep -i seaquel` lists the tunnel's local port. Switch the connection off: the port is gone, and `docker exec seaquel-postgres psql -U postgres -c "select count(*) from pg_stat_activity where client_addr is not null"` drops.
- **Git conflict round trip (desktop).** `git init --bare /tmp/sq-origin.git && git clone /tmp/sq-origin.git /tmp/sq-other && git -C /tmp/sq-other commit --allow-empty -m init && git -C /tmp/sq-other push origin HEAD`. Add `/tmp/sq-origin.git` as a shared repo, save a query into it and sync. Edit that query's file in `/tmp/sq-other`, commit and push; edit the same query in the app and sync: the conflict is listed with both sides. Restart the app: it's still listed. Resolve it and sync: the push succeeds, and `git -C /tmp/sq-other pull && git -C /tmp/sq-other log --graph --oneline -4` shows a merge commit with two parents. Also clone a real repo over SSH and over HTTPS with a token; with a wrong token, sync fails once with an auth error instead of hanging.
- **Desktop license.** Activate, restart (it validates), deactivate. If you're behind a proxy, activation still works through it.
- **Web: SQLite and DuckDB refused.** `npm run dev:web:full`, signed in: the wizard offers PostgreSQL, MySQL, MariaDB and SQL Server only, and a pasted `sqlite:///etc/passwd` connection string is refused with the reason. Directly: `curl -s -X POST 127.0.0.1:8788/api/db/connect -H 'content-type: application/json' -d '{"driver":"sqlite","connection_string":"sqlite:///etc/passwd"}'` gives `ENGINE_NOT_AVAILABLE`, and `-d '{"driver":"postgres","connection_string":"postgres://u@h/db?sslkey=/etc/passwd"}'` gives `CONNECTION_OPTION_NOT_ALLOWED`.
- **Web tutorial.** Open the SQL tutorial and pass one lesson. In the network tab the DuckDB `.wasm` and worker come from the app's own origin, with no request to `cdn.jsdelivr.net`. In the demo (`npm run dev:demo`) they still come from jsDelivr.
- **Web: `/internal` needs the secret.** `curl -si 127.0.0.1:8788/internal/license/install` and the same with `-H 'x-seaquel-internal: wrong'` both give 403. `curl -si localhost:5173/internal/license/install` never returns Rust's JSON (Node doesn't forward it).
- **Web: license service down.** Stop `dev:web:full` and run `npm run dev:web` alone (no Rust). `curl -si localhost:5173/login` gives 503 with the "Seaquel is unavailable" page, `curl -si -X POST localhost:5173/api/rpc` gives 503 `{"code":"license_service_unavailable"}`, and `curl -si localhost:5173/health` gives 200.
- **Web license.** First-owner signup, member signup, revoke, suspend, air-gap bundle upload and clear (README, "Air-gapped mode", on a `docker build` image).

---

## Execution notes (2026-09-26)

The plan was executed task by task, in two parts with the owner's checkpoint between them, with a review after each task and usually a round of review fixes. Tasks 1–2, 3–5 and 10–13 ran partly in parallel. Where the result departs from the text above, the repo is authoritative; the findings blocks under Decisions 2, 3, 8, 11 and 11b record the changes as they were made. Per-task times and surprises are in `2026-09-29-phase-3-effort.md`. The measured cost is in the design doc ("Phase 3 cost").

**What went differently from the plan**

- **The storage baseline creates tables before it adds columns** (Task 2 recording, Task 3). Today's `upgradeSchema` can't open a `v2026.4.5-beta.1` file and has left each one half-upgraded on every launch since `v2026.4.8`. The Rust baseline upgrades it to the same structure as a beta.1 file taken through `v2026.4.5`, data included. Files that started on beta.1 keep a nullable `project_id` with no foreign key on `saved_queries` and `dashboards`; that stays.
- **Data steps instead of migration 0001** (Task 4 review). The password cleanup as SQL was quadratic (280 s on the review's input), truncated at NUL bytes and could never be fixed after shipping, since sqlx checksums migration files. It became a Rust data step recorded in `_seaquel_data_steps`, and `migrations/` is still empty. The migrator ignores versions it doesn't know (`set_ignore_missing(true)`), so migrations must be expand-only (`migrations/README.md`).
- **Byte-exact JSON shaped the RPC** (Tasks 4 and 6). Stored JSON columns are `RawValue`s so they stay byte-identical, which means `method` must come before `params`, and `core_call` takes the raw bytes of a `Uint8Array` (a JS object would reach Rust as a `serde_json::Value`, which sorts keys).
- **The desktop opens storage lazily** (Task 6 review), on the first storage call. `LEGACY_STORAGE`, `STORAGE_CORRUPT` and `NO_DATA_DIR` are kept and block the app (Task 9's screen); other failures retry. Secret, SSH, git and license calls don't need storage, so the desktop routes them to `dispatch_secret`, `dispatch_ssh`, `dispatch_git` and `dispatch_license`, and `dispatch_workspace` refuses them.
- **Saves refuse after a failed load** (Task 9 review). Before, a failed load followed by any save replaced the collection with the empty one in memory. Not in the plan; see `load-guard.ts` and CLAUDE.md.
- **SSH trust pins the fingerprint** (Task 10 review): `trustHostKey` carries the approved `SHA256:` fingerprint, and only that key is recorded.
- **Two old git bugs surfaced in the plan's conflict test** (Task 11): the merge commit had one parent and left the merge open, and a push the remote refused reported success. The review added a cap on credential attempts, refused commits while files are conflicted, and `GitRepoStatus.conflict_files`.
- **The license port needed JavaScript's semantics throughout** (Task 13): numbers as `f64`, `atob`, `TextDecoder`, `parseInt`, `toISOString` and UTF-16 sort order, so canonical JSON and the bundle checks match `canonical.ts` byte for byte. The e2e cases moved to `crates/seaquel-server/tests/internal_license.rs`, since starting the Rust binary from vitest meant compiling the server with DuckDB. Session purging runs in Node after Rust's transaction commits.
- **`/internal/*` needs a per-boot secret as well as loopback** (Task 13 review). A query running inside the Rust process also connects from loopback. `server.js` generates `SEAQUEL_INTERNAL_SECRET`; `npm run dev:web:full` wraps `scripts/with-internal-secret.mjs`.
- **The Rust HTTP client isn't Node's fetch** (Tasks 12 and 13 reviews). `reqwest::Client::new()` panics on a broken OS certificate store, so both clients build lazily with a fallback (`http.rs`). The server's client adds `NODE_EXTRA_CA_CERTS` itself, and honours `HTTP(S)_PROXY`/`NO_PROXY`, which Node's fetch ignored.
- **Decision 11b, not in the plan** (Task 13 review, Task 14). SQLite and DuckDB are off on web. Cargo's feature unification put them back into `seaquel-server` in workspace builds, so the server registers engines by id (`with_plugins`). The Task 14 review then found connection options that name server files or sockets, and the operator's `PG*` variables and `~/.pgpass`, which sqlx reads; `web_config.rs` refuses the first, and `shared/rust-env.js` plus the startup scrub remove the second. The web tutorial moved from the server's DuckDB to DuckDB-WASM served from the image.
- **The wasm32 build of Core needed almost nothing** (Task 14): `uuid`'s `js` feature and one `cfg`. `browser` plus any native feature is a `compile_error!`.

**Bug fixes per area.** Older than phase 3 unless marked new:

- **Storage and data safety:** beta.1 files not opening (Task 2); the demo losing foreign keys after its first write (Task 2, fixed in Task 8); a startup storage failure giving an empty project and skipping shared repos (Task 9); saves after a failed load wiping projects, shared repos, project state, saved queries, history and AI messages, and `Vault.setup` orphaning every credential (Task 9 review and its fix round); key=value connection strings saved with their password (Decision 13.1, plan research).
- **Web server and proxy:** the `/api/db` path traversal (Task 7 review); `seaquel-server` had no logger, so every log line was dropped (Task 7 review); a non-loopback `BIND_ADDR` was accepted (Task 7 review).
- **Web engines:** SQLite and DuckDB file access on the server (Task 13 review); the web tutorial running on the server's DuckDB (Task 14); TLS key and certificate paths, sockets, `PG*` variables and `~/.pgpass` (Task 14 review).
- **SSH:** close left the session and forwards up (Decision 8, plan research); the trust retry recorded whatever key it met (Task 10 review); a tunnel stayed open after a connection was switched off or failed to connect (Task 10 and its review); `TunnelConfig`'s `Debug` printed the password (Task 10).
- **Git:** the merge commit and rejected pushes (Task 11); credential loops, commits with conflicts and the empty conflict list after a restart (Task 11 review).
- **Licensing:** `LicenseResponse`'s `Debug` printed the key and a trailing `/` in the base URL gave `//api` (Task 12); the reqwest panic (Task 12 review, new with rustls); `NODE_EXTRA_CA_CERTS`, the gate cache race, the purge retry, the in-process loopback hole and the payload hash check (Task 13 review, new in the port); a `tenant-info` answer without a tenant id wiping the tenant (Task 13 review, as the TS did).

**Decisions made during execution**

- **Web SQLite and DuckDB are off** (Decision 11b), owner's call, shipped with this phase rather than as a separate patch.
- **`dispatch_workspace` refuses SSH, git and license unconditionally**, not by feature, so `/rpc` can never reach them whatever Cargo compiled in.
- **The web image serves DuckDB-WASM itself** (~75 MB), so an air-gapped install has a tutorial. The demo keeps jsDelivr, since its copy lives in the website repo.
- **`SEAQUEL_DATA_DIR` set to an empty string is ignored**, where `get_data_dir` used to take it as-is.
- **The keychain service stays `app.seaquel.desktop` in dev builds**, so dev and release share saved passwords, as before.
- **The version diffs stay in TS** (Task 2): storage executes the prune the TS plans, because diff-match-patch counts UTF-16 units.

**Release notes**

For everyone:

- **Upgrading from a version before 2026.4.5.** This release no longer imports the JSON files those versions kept data in. If it finds them and no `seaquel.db`, it shows a screen that says so and changes nothing. Install any release from 2026.4.5 through 2026.9.x, start it once, then install this one.
- **Everything else carries over:** saved passwords, SSH settings and known hosts, shared projects, and all projects, queries, dashboards, history and chats.
- **Data files first created by 2026.4.5-beta.1 open again.** Since 2026.4.8 they failed to upgrade and were left half-upgraded on each launch. This release upgrades them with their data.
- **A failed load no longer wipes data.** If projects, saved queries, history, an AI chat or the credential vault failed to load at startup, the next save replaced them with an empty list. Those saves are now refused until a load succeeds; a settings change shows the error.
- **Connection strings in key=value form (`Server=…;Password=…`) are no longer saved with their password.** Only URL-form strings had it removed. Strings already saved are cleaned the first time this release opens your data.

SSH tunnels (desktop):

- **Closing a tunnel ends it.** Before, it only stopped new connections, and the SSH session and open forwards stayed up, so a database client could keep using a "closed" tunnel.
- **Switching a connection off closes its tunnel and frees the local port.** It used to stay open until the next reconnect. A tunnel is also closed when its connection fails.
- **Trusting a new host key records the key you were shown.** Trusting used to reconnect and record whichever key the server presented the second time. Now only the key with the fingerprint in the prompt is saved; any other key prompts again.
- **Known limitation:** a host known under one key type (say RSA) that now presents another (Ed25519) shows a first-use prompt, not the mismatch warning. Check the fingerprint before you approve it.

Shared projects (desktop):

- **Resolving a pull conflict makes a proper merge commit.** It used to leave the merge open, so the next push was rejected and the next pull conflicted again.
- **A push the remote rejects is reported as an error.** It used to say the push succeeded. A non-fast-forward rejection marks the repo as behind.
- **The conflict list survives a restart**, and committing is refused while files are still conflicted.
- **A wrong password or key fails instead of retrying.** Each credential is tried once, at most four in all.

Licensing (desktop):

- **License activation uses rustls** with the OS certificate store plus Mozilla's roots, instead of the OS TLS stack. A proxy CA installed in the OS store still works, and the system proxy is still used. Behind a TLS-intercepting proxy, three things change: missing intermediate certificates are no longer fetched, TLS 1.0 and 1.1 are refused, and some malformed certificates the OS accepted may fail.

Self-hosted web:

- **Security: `/api/db` path traversal.** An encoded path such as `/api/db/x%2F..%2Fquery` skipped the check that a connection belongs to the signed-in user, and could reach other routes of the Rust service. The proxy now forwards an exact list of paths.
- **Security: SQLite and DuckDB are off on web.** A signed-in user could open a SQLite or DuckDB "connection" on any path on the server: `auth.db` (sessions, license keys and emails), other users' data, and through DuckDB any file the server can read, with writes through `COPY TO`. The web app now offers PostgreSQL, MySQL, MariaDB and SQL Server only. A SQLite or DuckDB connection saved on desktop fails with a clear error on web. The SQL tutorial runs DuckDB-WASM in the browser, served from the image.
- **Security: connection options that name server files are refused.** Postgres and MySQL certificate, CA and key paths, `passfile`, Unix sockets and URLs without a host fail with `CONNECTION_OPTION_NOT_ALLOWED`. Client-certificate TLS and socket connections are desktop-only.
- **Security: `PG*` and `MYSQL*` variables no longer reach connections.** The database driver took defaults such as `PGPASSWORD`, `PGHOST` and `~/.pgpass` from the container, so an operator's credentials could reach a user's connection to a host of their choosing. The Rust service now gets an allow-listed environment and no home directory.
- **Licensing runs in the Rust service**, behind `/internal/license/*`, which needs a loopback peer and a secret `server.js` generates at each start. There is nothing to configure: no new settings, and the data files and `auth.db` migrations are unchanged.
- **Split deployments fail closed.** Node and the Rust service on different hosts (a non-loopback `SEAQUEL_RUST_URL`) no longer works: licensing refuses the calls and every page returns 503.
- **When the Rust service is down**, pages show "Seaquel is unavailable" and `/api/*` returns 503 `{"code":"license_service_unavailable"}`. `/health` still answers, so the container isn't restarted for it.
- **License calls honour `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY`.** Node's fetch ignored them. If the container sets them for other tools, calls to seaquel.app now go through that proxy.
- **`NODE_EXTRA_CA_CERTS` still works** for a TLS-inspecting proxy, as do `SSL_CERT_FILE` and `SSL_CERT_DIR`.
- **The image is about 75 MB larger**, for the tutorial's DuckDB-WASM files, and no longer installs `libssl3`. The server binary went from 48 MB to 14 MB.

---

## Follow-ups (not in this phase)

- **`data_version` polling and `StorageChanged`**, for when `seaquel mcp` writes the same file (phase 4). Phase 4 also serialises opens of one metadata file: sqlx's migrate lock is a no-op on SQLite, so two processes, or two pools in the web server (an evicted pool still finishing a request and a fresh one), can race a pending SQL migration, and the loser's request gets one 500 (`migrations/README.md`).
- **Connection ownership in Core** (open question 2; phase 5).
- **SSH and shared projects on web.** They need per-user known_hosts and repo storage on the server, and a decision on whether the server should hold SSH keys at all.
- **Upgrade russh** from 0.48. With it, a prompt that says "this host is known under another key type" when known_hosts has the host under another algorithm; today that reads as a first-use prompt (Decision 8's Task 10 findings).
- **Move the license migrations into Rust** when the next one is needed. Node keeps applying 006–012 until then.
- **The browser storage backend** (phase 8). No `StorageBackend` trait exists yet. Phase 8's rusqlite-on-sqlite-wasm-rs spike decides whether one is needed or whether the same queries run on a different executor.
- **A confined per-user file area**, if anyone wants SQLite or DuckDB on web: a directory per user, with DuckDB's external access off. Until then both stay off (Decision 11b).
- **`rustls-platform-verifier`** for the license clients, so TLS is verified by the OS (AIA fetching, the OS's own policy) and TLS-intercepting proxies behave as they did with native-tls.
- **The demo's DuckDB-WASM comes from jsDelivr.** Serving it from seaquel.app means adding ~75 MB to the website repo; that's the website's call.
- **The web image's DuckDB-WASM assets** are 75 MB (the mvp and eh bundles). Shipping only `eh` (every current browser supports exceptions) would halve it. They're also sent uncompressed, like `seaquel-wasm` (phase 2b's `precompress` follow-up).
- **Delete `src-tauri/Cargo.lock`.** `src-tauri` has been a workspace member since before v2026.9.1, so Cargo uses the root `Cargo.lock` and ignores this one; it has only been kept in step by the version bumps. Nothing references it (CI, release, Docker). CLAUDE.md's release steps already point at the root lockfile.
- **Stale doc comment:** `crates/seaquel-rpc/src/workspace.rs`'s module doc still says SSH, git and licensing "join in later tasks" and that a group answers `NOT_SUPPORTED` when its feature is off; SSH, git and license are now refused unconditionally.
- **Still open from the AI safety phase:** blocking or probing MotherDuck's `md_*` and DuckLake's maintenance functions, and DuckDB extension autoinstall on plain SELECTs. Both are the owner's call (`2026-09-28-ai-safety-plan.md`, Follow-ups). Neither affects web now that it has no DuckDB.
