# Phase 5a Implementation Plan: connections in Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** The desktop and web GUIs connect, test, query and disconnect through Core's workspace API. The TypeScript connection logic goes away: building connect configs, putting passwords back into URLs, opening SSH tunnels, and the `userId:` prefix scoping. On web, each connection belongs to the user's workspace in Core, and every database call is checked against it. `/api/db/*` and the `db_*` Tauri commands are replaced by `/rpc`, `/rpc/stream`, `core_call` and `core_stream`.

**Architecture:**
- **Core.** Two operations, `Workspace::connect` and `Workspace::test`, take either a saved connection id or an unsaved form, plus secrets the caller supplies. They reuse `seaquel-workspace::connections`, which phase 4 ported and pinned with fixtures. Core tags every connection and stream with the workspace that opened it. Each database operation goes through the workspace, which refuses ids it doesn't own.
- **`seaquel-rpc`.** A new `Db` request group covers connect, test, disconnect, query, execute, engine calls and cancel. `CoreEvent` carries query stream events and a new "connection closed" event.
- **Transports.** Desktop serves `core_call` and a new `core_stream(request, Channel<CoreEvent>)`. Web serves `POST /rpc` and a new multiplexed `/rpc/stream` WebSocket, opened once per browser session.
- **TypeScript.** A `CoreClient` replaces the Tauri and HTTP providers. `ConnectionManager` becomes a thin view model.
- **The demo.** It stays on its DuckDB-WASM path until phase 8, as the owner decided.

**Tech Stack:** Rust (Core, `seaquel-rpc`, `seaquel-server` with axum and WebSockets, Tauri 2 channels), TypeScript/Svelte 5, vitest, the e2e Docker databases, and the SSH container.

**Inputs:**
- The design doc: decisions 6 and 7, "Core, workspaces and state", "The RPC surface for GUIs", "Secrets", and "What this means for phase 5".
- The phase 4 plan's follow-ups.
- The phase 5 slicing and demo decisions (2026-10-01).
- Two code maps taken on 2026-10-01. The file and line references below come from them.

---

## What the code maps found

1. **The GUI has five connect paths, all in `connection-manager.svelte.ts`:** `add` (a new form), `reconnect` (the reconnect tab's form, or `autoReconnect`), `autoReconnect` (a saved row plus keychain or vault), `test` (a form, never saved) and `toggle`/`remove`.
   - `add` and `reconnect` connect first and save the row afterwards, only on success.
   - Tunnels are opened in TypeScript through the Core `Ssh` group, with the host-key prompt. Their ids are kept in `ConnectionManager.tunnelIds`.
   - `providerConnectionId` lives only in memory.
2. **Web secrets never reach the server unencrypted at rest.** The vault encrypts in the browser, and the server stores only ciphertext. A connect request carries the decrypted password in its body, as it does today. So on web, Core has to accept secrets from the caller: the web workspace has no `SecretStore`. On desktop, a password typed into the form must win over the keychain rules. Today `read_secrets` gives up when `savePassword` is off, even when the form has a password.
3. **Core has one global connection map and one global stream map.** Nothing ties a connection to a workspace. The web `Workspaces` LRU closes storage on eviction but leaves the user's database connections open. `cancel_stream` checks no ownership.
4. **Web tenancy today is `shared/connection-scope.js`.**
   - The Node proxy rewrites Rust connection ids to `userId:rustId`, strips the prefix on the way in, and refuses a mismatch.
   - `server.js` does the same for the first frame of the one-query-per-socket `/api/db/stream` WebSocket.
   - Rust itself has no idea who owns what.
5. **Streams.**
   - **Desktop:** a Tauri `Channel<StreamEvent>` per query, cancelled with `db_cancel_stream(query_id)`.
   - **Web:** one WebSocket per query, cancelled by closing the socket.
   - **`StreamEvent`:** `Batch`, `Done`, or `Error`. A disconnect mid-query ends it with `CONNECTION_CLOSED`.
6. **The connect-config fixtures record two sets of semantics.**
   - **autoReconnect:** a saved row plus the keychain.
   - **The reconnect tab:** the form's `getConnectionData` string, which `add` and `test` also use.
   
   Their README lists 11 quirks, and phase 5 has to decide what to do with each (open question 1). The Rust side already has the form helpers: `FormData`, `build_connection_string`, `connection_data_string`, `reinject_password` and `rewrite_host_port`.
7. **Demo and tutorial calls stay.** `addDemoConnection` connects DuckDB-WASM outside Core, and so does the tutorial's `provider.connect`. Both are frozen and stay.

## Open questions

Each question has a recommendation. The plan is written as if it's taken; if one isn't, only the named task changes.

**Answered (2026-10-01):**
1. Fix the quirks in 5a (not the recommendation). See Decision 6.
2. Close a workspace's connections when it's evicted (not the recommendation). There's no idle close, and the cap stays hard.
3. The events channel carries only what 5a needs (the recommendation).

Execution is subagent-driven.

1. **The recorded quirks.** (Answered: fix them; see Decision 6.) **Recommendation was to keep today's behaviour for all of them in 5a,** with each fixture case replayed by Rust. Moving the GUI onto Core is already a large change, and quirk fixes would each change what connects. List each quirk as a follow-up so it can be decided on its own. The one exception is quirk 7a, where a key=value SQL Server string over SSH throws; there Core returns a clear `INVALID_CONNECTION` instead of crashing. Affects Task 2.
2. **What happens to an evicted web workspace's connections.** Today the LRU (1,024 users) evicts a workspace and leaves its database connections open, orphaned. **Recommendation:**
   - Eviction skips a workspace that has open connections or streams.
   - Web connections that sit idle for 30 minutes are closed (`CONNECTION_CLOSED`), and the GUI shows them as disconnected.
   - The cap becomes a soft cap: if every workspace is busy, the server logs and grows past it rather than dropping someone's live connection.
   
   The alternative is to close a workspace's connections when it's evicted. That's simpler, but a busy server would disconnect active users. Affects Task 5.
3. **How much the events channel carries.** **Recommendation: only what 5a needs,** which is query stream events and `ConnectionClosed { connection_id, code, message }` for idle closes, lost connections and tunnel drops. `StorageChanged`, repo status and the other events the design lists wait for the slice that needs them. Affects Tasks 2–5.

## Decisions (2026-10-01)

### 1. The connect API

```rust
pub enum ConnectTarget {
    /// autoReconnect semantics: the stored row, secrets from the caller first,
    /// then the workspace's SecretStore under the row's save flags.
    Saved { id: String },
    /// The form a user filled in (add, reconnect tab, test): the reconnect tab's
    /// semantics (`getConnectionData`), never read from storage.
    Form { form: ConnectionForm },
}
pub struct ConnectRequest {
    pub target: ConnectTarget,
    pub secrets: SuppliedSecrets,            // db / ssh / ssh_key, all optional, redacted Debug
    pub host_key: HostKeyPolicy,             // KnownOnly or Trust(fingerprint) after the GUI's prompt
    pub create_if_missing: bool,
    pub restricted: bool,                    // MCP only (phase 4)
}
impl Workspace {
    pub async fn connect(&self, core: &Core, req: ConnectRequest) -> Result<ConnectionId, CoreError>;
    pub async fn test(&self, core: &Core, req: ConnectRequest) -> Result<(), CoreError>;
    pub async fn disconnect(&self, core: &Core, id: &ConnectionId) -> Result<(), CoreError>;
}
```

- **`connect_saved` becomes `connect` with `ConnectTarget::Saved`,** and the MCP server moves to it. Its behaviour doesn't change.
- **Supplied secrets always win.** A password the caller passes is used whatever the save flags say. This is the fix for finding 2. Only secrets that weren't supplied come from the store, under the flags. On web the store is absent, so only supplied secrets exist.
- **The host-key prompt stays in the GUI.** An unknown host returns `UNKNOWN_HOST_KEY` with the fingerprint. The GUI prompts, then calls again with `Trust(fp)`, exactly as `createSshTunnelWithHostKeyCheck` does today. Tunnels are owned by the connection, as phase 4 built them. The GUI no longer opens tunnels, so on the GUI side the `Ssh` RPC group is only used by `test`. Decide in Task 6 whether `test` also moves onto `Workspace::test`; that's likely.
- **Saving stays in the GUI.** `add` and `reconnect` save the row after a successful connect, through the storage RPC, the same as today. Moving connection CRUD into a `ConnectionService` is 5c.

### 6. The connection quirks: fixed in 5a (answered question 1)

**One builder for both targets.** `Saved` and `Form` map to the same intermediate row plus supplied secrets. They go through one function, so the 11 cases where the reconnect tab and autoReconnect disagreed now agree. For each quirk in the fixtures README:

| # | Quirk today | 5a behaviour |
|---|---|---|
| 1 | A row with no stored string (postgres, mysql, mariadb, sqlite) can't auto-reconnect | Build the string from the fields, as the MCP server already does |
| 2 | Password reinjection turns `postgresql://` into `postgres://` | Keep the scheme the user stored |
| 3 | An empty username with a password gives `postgres://:pw@…` | Keep it. It's a valid URL, and the server then uses its own default user. Confirm live on each engine, and refuse with a clear message if an engine rejects it |
| 4 | Any string that doesn't split into exactly three `:` parts is rebuilt from the fields, dropping query parameters, the default port, IPv6 brackets and TablePlus parameters | Use the stored or typed string as it is. Rebuild only when it's empty. Put the password in only if the string has none. Apply the SSH host and port rewrite with a real URL parser, keeping IPv6 brackets |
| 5 | Tab defaults: a missing SSL mode becomes `disable` (SQL Server then connects unencrypted), an empty host becomes `localhost`, port 0 becomes 5432, and empty tunnel fields are sent as `""` | A missing SSL mode is the engine's own default (SQL Server: encrypt and trust, as today when unset; Postgres and MySQL: the driver's default). Port 0 becomes the engine's default port. An empty host is an error. An empty tunnel field means none |
| 6 | SQL Server `require` and `verify-*` over SSH fail, because the certificate name is checked against 127.0.0.1 | Add the TLS server name to `ConnectConfig` and pass the row's host, if tiberius allows it. If it can't, say so, keep today's behaviour and document it |
| 7a | A key=value SQL Server string over SSH throws | SQL Server builds from the fields and ignores the string, as today, so the rewrite never touches it |
| 7b | For a TablePlus `+ssh` URL, the database password lands on the SSH user | Parse `+ssh` URLs into their database and SSH parts with the TablePlus parser that already exists (`parseConnectionString`, ported), or refuse them with a clear message. Prefer parsing |
| 7c | The tunnel forwards to the row's host and port, not the string's | Forward to the host and port that the connection actually uses: the string's when there is one |
| 7d | The reconnect tab opens a tunnel for SQLite | File engines never tunnel on any path |
| 8 | MySQL `verify-ca` and `verify-full` pass through unmapped | Map them to `VERIFY_CA` and `VERIFY_IDENTITY` |
| 9 | MariaDB connects through the mysql driver with a `mariadb://` scheme | Keep it if sqlx accepts the scheme, which the live tests say it does, and confirm it |
| 10 | DuckDB keeps `?params` in the file path; `duckdb://` becomes `:memory:`; SQLite `?mode=ro` is kept on one path and dropped on the other; the tab can't auto-connect a DuckDB row with an empty name | Parse DuckDB parameters out of the path. Keep `duckdb://` as `:memory:`. Keep SQLite's `?mode=ro` on both paths. Treat an empty DuckDB name as `:memory:` |
| 11 | The username taken from the URL is percent-decoded | Keep it; decoding is correct |

#### Decision 6, settled by the owner (2026-10-01)

Task 1's v2 fixtures pin these; its README (`connect-config-v2/README.md`) names the cases.

- **A. The supplied or saved password always wins** (refines row 4). A password the caller supplies, or the one saved in the keychain or vault under the save flags, replaces any password already in the string: in URL user info, or the `Password=`/`Pwd=` pair of a key=value string. The give-up rules don't change.
- **B. TablePlus `tLSMode` is translated** (refines row 4). When a string has no `sslmode` (Postgres) or `ssl-mode` (MySQL, MariaDB), `tLSMode` is replaced in place by it: 0 is prefer, 1 disable, 2 require. The rest of the string stays as it is.
- **I. The wizard's SSL mode defaults to unset** (refines row 5). The dropdown gets a "Default" option (Task 6), so a form whose user picked no mode gets the engine's default: SQL Server encrypts and trusts, and Postgres and MySQL use the driver's default. A mode picked on purpose, `disable` included, is kept. Saved rows keep the mode they stored. The fixtures tag these cases `5-wizard`.
- **Resolved in Task 1, as written:**
  - **C.** A `+ssh` URL on a row or form with its own enabled tunnel uses that tunnel. The URL contributes its database part, without TablePlus-only parameters.
  - **D.** DuckDB parameters parsed out of the path go in a new `ConnectConfig` field, `duckdb_config`.
  - **E.** An empty host is `CREDENTIALS_REQUIRED`, only where the host is used (MSSQL, or a string built from the fields).
  - **F.** SSH password auth with an empty SSH password is `CREDENTIALS_REQUIRED` for a form too.
  - **G.** A new `tls_server_name` (the row's host) is set on every tunnelled MSSQL connection.
  - **H.** A typed or stored string ignores the form's SSL mode.

**How the fixtures change.**
- **The frozen v1 fixtures stay frozen.** They record what the app used to do.
- **Task 1 adds a v2 set:** the same inputs, with the intended output under the table above. The README explains every case that changed and which row causes it.
- **Rust replays v2.** v1 becomes a diff report: the test asserts that the cases that changed are exactly the ones the table says.
- **Live checks.** Every row gets a live check against the Docker databases where one is possible.

### 2. Ownership

- Every connection and stream in Core carries the `WorkspaceId` that opened it.
- `Workspace` has the database operations: `query`, `query_stream`, `execute`, `transaction`, `engine` (the dialect and introspection calls), `cancel` and `disconnect`. Each one refuses an id the workspace doesn't own with `CONNECTION_NOT_FOUND`, the same answer as an id that doesn't exist, so it doesn't leak whether the id exists.
- Stream ids are scoped per workspace, so one user can't cancel another's query.
- Desktop has one workspace, so ownership is invisible there.
- `Core`'s own id-based methods stay for the engine tests, but no interface calls them any more. `check-crate-deps` can't enforce that, so Task 7 reviews it.

### 3. RPC and events

- **`Request::Db(DbRequest)`.** The variants are `Connect`, `Test`, `Disconnect`, `Query`, `Execute`, `Transaction`, `Engine(EngineRequest)` and `Cancel { stream_id }`. The same `method`-before-`params` wire rules apply.
- **`CoreEvent`.** It's `Stream { stream_id, event: StreamEvent }` plus `ConnectionClosed { connection_id, code, message }`. The codes are `CONNECTION_CLOSED` (lost), `TUNNEL_CLOSED` and `WORKSPACE_EVICTED`.
- **Desktop.**
  - `core_call` serves `Db` too.
  - New `core_stream(request_bytes, channel)` starts a query stream and pushes `CoreEvent`s to the channel.
  - `core_events(channel)` is registered once at startup for `ConnectionClosed`.
  - The nine `db_*` commands are deleted.
- **Web.**
  - `POST /rpc` serves `Db` for request/response calls.
  - `GET /rpc/stream` is a WebSocket per browser session. The client sends `{op:"start", stream_id, request}` and `{op:"cancel", stream_id}`. The server sends `{stream_id, event}` and `ConnectionClosed`.
  - Node proxies the upgrade at `/api/rpc/stream`. It resolves the session the way `server.js` does today and adds `X-Seaquel-User`. It no longer inspects frames. Rust enforces ownership.
  - `/api/db/*`, `shared/connection-scope.js` and the old upgrade path are deleted.

### 4. TypeScript

- `CoreClient` (`src/lib/core`) has the two transports, Tauri and HTTP plus WebSocket. It's typed from the generated `CoreRequest` and `CoreResponse`, with a stream API that returns an async iterator and an abort.
- `DatabaseProvider` stays as the interface the managers use, with one implementation, `CoreProvider`, over `CoreClient`. It replaces `unified-tauri-provider.ts` and `http-provider.ts`. `RustEngineClient` moves onto `Db.Engine`. The DuckDB provider stays for the demo and tutorial.
- `ConnectionManager`:
  - `add` and `reconnect` send `Form`, and `autoReconnect` sends `Saved` with any secrets the client holds. On web that means the vault; on desktop, none, because Core reads the keychain.
  - `test` sends `Form`.
  - The host-key retry loop moves from `ssh-tunnel.ts` into a small helper around `connect`.
  - Deleted: `setupSshTunnel`, `tunnelIds`, password reinjection, the tunnel URL rewrite, `toRustConfig`, and TS-side tunnel closing on toggle, remove and failure.
- Web vault: the client decrypts and sends secrets with `connect`, as today. Nothing is persisted server-side.

### 5. Trust boundary (a probe is part of the plan)

| Threat (web) | Guard |
|---|---|
| User A uses, disconnects or cancels user B's connection or stream | Workspace ownership on every operation; stream ids scoped per workspace; the same `CONNECTION_NOT_FOUND` for "not yours" and "doesn't exist" |
| A forged `X-Seaquel-User` | Node always sets it from the session and drops any client value (phase 3); Rust is loopback-only |
| Driving Core onto files or other engines | `WEB_ENGINES` and `web_config.rs` (phase 3) apply to `Db.Connect` and `Db.Test`. Task 5 moves these checks from the old routes to the new ones |
| Secrets in logs | Redacted `Debug` on `SuppliedSecrets`; `dispatch` logs method names only |
| Resource use | Streams scoped per workspace; eviction closes a workspace's connections, streams and tunnels (answered question 2) |

---

## Ground rules

These are phase 4's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules;
- parallel-agent file ownership, with small re-read edits to shared files;
- tests never touch the real keychain, data dir or `~/.ssh`;
- no secrets in `Debug`, errors or logs;
- the full check list;
- effort log: `docs/plans/2026-10-01-phase-5a-effort.md`.

Two additions from phase 4's cost notes:
- **One shared `CARGO_TARGET_DIR`** for all agents: `scratchpad/p5a/target`. Clean it between tasks if disk runs low, since DuckDB fills disks.
- **The probe (Task 7) is budgeted separately,** with its fixes at about three times the probe.

## Order and estimates

| # | Task | Estimate | Needs | Alongside |
|---|---|---|---|---|
| 1 | Form-path baseline and v2 fixtures (Decision 6) | 1.5–2 h | — | 2 |
| 2 | Core: `connect`/`test` API, one builder with the quirk fixes, supplied secrets, ownership | 4–5.5 h | 1 | — |
| 3 | `seaquel-rpc`: `Db` group, `CoreEvent`, dispatch | 1.5–2 h | 2 | — |
| 4 | Desktop transport: `core_call` `Db`, `core_stream`, `core_events`; remove `db_*` | 1.5–2 h | 3 | 5 |
| 5 | Web transport: `/rpc` `Db`, `/rpc/stream`, Node proxy, eviction; remove `/api/db` | 3–4 h | 3 | 4 |
| 6 | TS: `CoreClient`, `CoreProvider`, `ConnectionManager` onto Core, deletions | 3–4 h | 4, 5 | — |
| 7 | Trust-boundary probe (web multi-user) | 1–1.5 h | 6 | — |
| 8 | Docs, measurement, checkpoint | 0.75–1 h | all | — |
| | Probe fixes (≈3× the probe) | 3–4.5 h | | |
| | Review fixes (40%) | 6–8 h | | |
| | **Total** | **~26–35 h** | | |

Phase 4's first passes ran at 53–78% of their estimates, so expect roughly 18–25 h logged. The riskiest tasks are 5, because web transport changes touch tenancy, and 6, because every connection path in the GUI changes at once.

---

## Tasks

### Task 1: Record the form-path baseline and write the v2 fixtures

First, write the v2 fixture set described in Decision 6. Use the same inputs, the intended output for each, and a README that states which table row changes which case. Then extend the phase 4 recorder (`docs/plans/artifacts/2026-09-30-freeze-connect-config.mjs.txt`) with `add` and `test` cases. These cover the config the form path sends: `getConnectionData` with no reinjection, plus `createIfMissing`. Aim for at least 30 cases across the engines, SSH, and SQLite/DuckDB `createIfMissing`. Add them to `crates/seaquel-workspace/tests/fixtures/connect-config/` under the frozen-fixture rules and log them in the README.

### Task 2: Core `connect`/`test` API, supplied secrets, ownership

- **Files:** Core `workspace.rs`, `lib.rs` and `ssh.rs`; `seaquel-workspace::connections`, which gets a `ConnectionForm` → row mapping and supplied-secret handling; the tests.
- **Test first:**
  - Replay every fixture case through `connect`, both `Saved` and `Form`, and the new `add`/`test` cases.
  - A supplied password wins with `savePassword` off.
  - Ownership: workspace B can't query, execute, stream, engine-call, cancel or disconnect workspace A's connection. Every one of those returns `CONNECTION_NOT_FOUND`.
  - The MCP server's tests still pass after moving to `connect`.
  - Live tests for each engine and for SSH.
- **Implement:** Decisions 1 and 2. Keep `CoreError` codes stable. Add `WorkspaceId`, a random id generated at open.

### Task 3: `seaquel-rpc` `Db` group and `CoreEvent`

- Add `DbRequest`/`DbResponse` and `CoreEvent`.
- `dispatch_workspace` handles `Db` through the workspace. A stream dispatch returns a stream of `CoreEvent`.
- Regenerate the types.
- Tests: wire snapshots, `method` before `params`, redacted `Debug`, and ownership refusals through dispatch.

### Task 4: Desktop transport

- `core_call` routes `Db`.
- Add `core_stream` and `core_events`.
- Delete the `db_*` commands and `src-tauri/src/db/commands.rs`.
- The early-cancel case must still work: a cancel that arrives before the stream is registered. Phase 3 noted a race here, so fix it with a pre-registration.
- Rust tests on `handle_core_call` and the stream helper.

### Task 5: Web transport

- **Routes and proxying:**
  - `/rpc` routes `Db`.
  - Add `/rpc/stream` (an axum WebSocket, multiplexed, one per session) with the `X-Seaquel-User` check.
  - Node `/api/rpc/stream` proxies the upgrade: session → user header, no frame inspection.
  - Move `WEB_ENGINES` and `web_config.rs` checks onto `Db.Connect` and `Db.Test`.
- **Workspace lifecycle:** when the LRU evicts a workspace, it cancels that workspace's streams and closes its connections and tunnels. Each open GUI then gets `ConnectionClosed` with code `WORKSPACE_EVICTED` over its `/rpc/stream`, if one is connected, and shows those connections as disconnected. The cap stays hard at 1,024 (answered question 2).
- **Delete:** `/api/db/*`, `shared/connection-scope.js` and the old upgrade path in `server.js`.
- **Tests:** router tests with two users, the WebSocket multiplexing, cancel, eviction closing the evicted user's connections (a test cap of 2), and every web engine or option refusal on the new route.

### Task 6: TypeScript onto Core

- Add `src/lib/core/{client.ts,tauri.ts,http.ts}` and `CoreProvider`.
- Move `RustEngineClient` onto `Db.Engine`.
- Rewrite `ConnectionManager` per Decision 4.
- Delete `unified-tauri-provider.ts`, `http-provider.ts`, `toRustConfig` and the TS tunnel handling.
- Update every test that mocks the providers.
- The demo and tutorial keep `DuckDBProvider`.
- The wizard's SSL dropdown gets a "Default" option (the mode unset), and it becomes the initial value (Decision 6, settled choice I).
- vitest cases:
  - each connect path;
  - the host-key retry;
  - web vault secrets;
  - stream abort;
  - `ConnectionClosed` marking a connection disconnected.

### Task 7: Trust-boundary probe

A separate agent runs a two-user web instance (`npm run dev:web:full`) and uses only the browser-facing HTTP and WebSocket endpoints with two signed-in sessions. For each check it records evidence:
- **Cross-user access.** User A tries to reach user B's connections and streams in every way: by guessed id, by id seen in their own responses, by stream id, by cancel, by disconnect, and through engine calls.
- **Forged identity.** User A tries forged headers.
- **Driver and options.** It checks that the web engine and option restrictions still hold.
- **Resource exhaustion.** It checks that eviction closes connections, streams and tunnels.
- **Leaks.** It checks for secrets in responses and logs.

Probe fixes are budgeted separately.

### Task 8: Docs, measurement, checkpoint

- **CLAUDE.md:** the new API, the transports and the ownership rule.
- **Design doc:** a status line and a "Phase 5a cost" section.
- **This plan:** execution notes and release notes.
- **Effort log:** the totals.
- **Checkpoint.**

**Status (Task 8):** done. CLAUDE.md, the design doc's status line and "Phase 5a cost", the execution notes, Task 7 findings, release notes, checkpoint and follow-ups below, and the effort log's totals are written. The full check list ran: everything passes. Two CI steps failed on the first run (the wasm32 `browser` build and oxlint's type check) and were fixed afterwards; see "Checkpoint". The owner ran the manual checks below on 2026-09-27; all pass.

## Manual checks

For the owner, after Task 8. The test databases are `npm run e2e:db:up` plus `npm run e2e:db:seed`. Postgres is `postgres@127.0.0.1:5432/seaquel_test` with no password; SQL Server is `sa`/`Seaquel_Test_123!` on `127.0.0.1:1433`. The SSH container is user `seaquel`, password `seaquel-test-password`, on `127.0.0.1:2222`, and reaches Postgres as `postgres:5432`. To watch a query on the server: `docker exec seaquel-postgres psql -U postgres -c "select pid, state, query from pg_stat_activity where query like '%pg_sleep%' and pid <> pg_backend_pid()"`.

**Desktop** (`npm run tauri dev`, or a local `npm run tauri build`). Back up your data dir first (`~/Library/Application Support/app.seaquel.desktop.dev` for dev, without `.dev` for a build).

- [ ] **Connection string field.** New Postgres connection: the SSL dropdown starts at "Default". Paste `postgresql://postgres@127.0.0.1:5432/seaquel_test`: the fields fill in and the string field goes away. Paste `postgresql://postgres@127.0.0.1:5432/seaquel_test?application_name=x`: the string stays, with a Clear button. Change the port: the string is cleared. Then add SQL Server with the password typed and "save password" off: it connects. Disconnect, and reconnect from the connection's tab with the password: it connects.
- [ ] **Legacy strings.** With a data dir from 2026.9.2 or earlier that has a Postgres connection saved from the wizard (not pasted), with AI schema and data sharing turned on: start the new build. The connection auto-reconnects. `sqlite3 -readonly "<data dir>/seaquel.db" "select name, connection_string, ai_share_schema, ai_share_data from connections"` shows an empty string for it and both sharing flags still on. A connection you pasted a string with extras into keeps its string, minus any password.
- [ ] **Auto-reconnect and test.** Restart the app: saved connections reconnect. "Test connection" on a form works and leaves nothing open.
- [ ] **SSH host-key prompt.** `cp ~/.ssh/known_hosts /tmp/kh.bak && ssh-keygen -R '[127.0.0.1]:2222'`. Add a Postgres connection with host `postgres`, port 5432, database `seaquel_test`, user `postgres`, through SSH `127.0.0.1:2222` as `seaquel` with the password. Connecting shows the fingerprint prompt; Trust connects. Disconnect and connect again: no prompt. Run the `ssh-keygen -R` line again, connect and choose Reject: it fails, and `grep -c '127.0.0.1\]:2222' ~/.ssh/known_hosts` is `0`. Trust it again, then toggle the connection off: `lsof -iTCP -sTCP:LISTEN | grep -i seaquel` no longer shows its local port. Restore with `cp /tmp/kh.bak ~/.ssh/known_hosts`.
- [ ] **Cancel stops the server.** Run `select pg_sleep(60)` and press Stop. Within a second or two the `pg_stat_activity` query above shows no `pg_sleep` row. Same with `SELECT SLEEP(60)` on MySQL (`docker exec seaquel-mysql mysql -uroot -e 'show processlist'`). Also check a 100,000-row result streams and the grid fills.
- [ ] **Two windows.** Open a second window (the log viewer or the theme editor) and keep it open. Run a query in the main window: results arrive once. Start `select pg_sleep(60)`, then reload the main window (Cmd+R): the query disappears from `pg_stat_activity`. Close the second window: the main window keeps working.
- [ ] **Remove.** Remove a connected connection: it disconnects and is gone after a restart.

**Web** (`npm run build:web:full`, then `SEAQUEL_WORKSPACE_CAP=2 npm run start:web`, at `http://localhost:8787`). Leave `BETTER_AUTH_URL`, `ORIGIN` and `SEAQUEL_TRUSTED_ORIGINS` unset. Sign in as user A in one browser and user B in another (or a private window).

- [ ] **Origin at localhost.** Signup, sign-in, saving a connection and running a query all work at `http://localhost:8787` with no origin variable set. `curl -si -X POST http://localhost:8787/api/rpc -H 'Origin: http://evil.example' -H 'Content-Type: application/json' -d '{}'` answers 403 (with or without a session cookie). Optional: open the app at `http://127.0.0.1:8787` too; it works there as well.
- [ ] **Two users.** A and B each add the Docker Postgres and query it. Neither sees the other's connections.
- [ ] **Cancel stops `pg_sleep`.** As A, run `select pg_sleep(60)` and press Stop: gone from `pg_stat_activity` within a second or two. Start it again and disconnect the connection instead: same.
- [ ] **Eviction.** With A and B connected, and B's last query newer than A's, sign in as a third user C (a third browser profile) and run any query. A, the least recently used, gets a toast that the server closed the session's connections, and its connection shows as disconnected. Reconnecting works.
- [ ] **17th connection.** As A, add the same Postgres connection 17 times (duplicates are fine) and connect them all: the 17th is refused with `TOO_MANY_CONNECTIONS`. B can still connect.
- [ ] **Too many tabs.** As A, open the app in 9 tabs and run a query in each, in order: the ninth shows the "Too many Seaquel tabs are open" message. Close a few tabs and run it again: it works.
- [ ] **Engines.** SQLite and DuckDB aren't offered in the wizard.

**MCP.** With the desktop build, run the phase 4 plan's Claude Code check: `claude mcp add` from Settings → MCP, then a row count on an exposed Postgres connection. `list_connections` shows only the checked connections.

**Demo and tutorial.** `npm run build:demo` then `npm run preview:demo`: the demo opens, runs a query and a tutorial lesson. On web, the tutorial runs in the page, and on desktop it runs as before.

---

## Execution notes (2026-09-27)

The plan was executed task by task with subagents, Tasks 4 and 5 partly in parallel, with a review after each task and a second review round on Tasks 3 and 6. Task 7's probe ran once, then again on a fresh instance after its fixes. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-10-01-phase-5a-effort.md`; the measured cost is in the design doc ("Phase 5a cost").

**What went differently from the plan**

- **`ConnectPolicy` instead of a feature gate** (Task 3 review). The plan put `connect`/`test` behind an rpc feature, but Cargo unifies features, so `cargo test --workspace` compiled a web server that could connect anywhere, and the web config check ran after the tunnel opened. Core now has a `ConnectPolicy` with no default: `Unrestricted` on desktop, the CLI and in tests, and `Checked { check, allow_ssh: false }` on web, which refuses a target that needs a tunnel as soon as the row or form is read, before any secret read. `ssh_open` follows it too. The rpc `connect` feature was left as an empty no-op, then replaced by a `workspace` feature at the checkpoint (below).
- **One event sink per webview** (Task 4 review). The plan had one `core_events` channel. A dead webview's channel doesn't reliably fail its sends, so each webview label keeps its own sink, every sink gets every event, and a reload or closing the window cancels that webview's streams.
- **`core_stream` returns the number of events it sent** (Task 6 review). The invoke's reply can overtake channel messages, so the first client waited 500 ms before treating a stream with no `done`/`error` as cancelled. Now it waits for the count, with a 30 s safety limit that logs.
- **`core_stream` takes the request as JSON text**, not raw bytes like `core_call`: a Tauri 2.11 invoke with a raw body can't carry a `Channel`.
- **The early-cancel race is closed in Core** (Task 4): a workspace remembers a cancel for a stream id it hasn't registered (256 per workspace, and not for ids that already finished), and the stream starts cancelled.
- **A desktop with broken storage still connects** (Task 4). `db` calls use one workspace for the run; when storage can't open for good, it's a stand-in on an empty scratch storage where form connects work and saved ones get the storage error.
- **The WebSocket's lifecycle grew in review** (Task 5 review). Rust splits a batch whose frame would pass 4 MiB by rows, caps a user at 8 sockets and a socket at 16 streams, and frames at 8 MiB. The Node proxy re-checks the session every 60 s and closes after 12 h. The close codes are split on purpose: 1008 means access is gone (signed out, license suspended, member removed), and the client stops and says so; 1013 means try again (the gate failed or was slow, or the user has too many sockets), and the client reconnects or reports too many tabs. Treating a gate outage as lost access would have signed users out of their queries.
- **The Origin check sits in `stream-access`**, the route the proxy already calls, so the WebSocket uses the same trusted-origin list as the HTTP routes.
- **No idle close and no busy-skip on eviction.** The plan's text for question 2's recommendation was superseded by the answer: eviction always runs `close_all`, and the cap stays hard.
- **`CONNECTION_CLOSED` and `TUNNEL_CLOSED` are reserved.** Core can't cheaply see a lost connection (pools reconnect, MSSQL reconnects on the next call) and `seaquel-ssh` doesn't report a dropped tunnel, so only `WORKSPACE_EVICTED` is emitted. The GUI handles all three.
- **A connection string lives only while it's visible** (Task 6 review). Core obeys a stored string and ignores the fields it encodes, which the old GUI never did: it rebuilt the string from the fields on every save. So the string is shown whenever it's non-empty, cleared when a field it encodes changes, dropped after a paste unless it has extras, and removed from old rows whose string equals what the old builder made of their fields. The cleanup is a TS persist-once at load, not a Rust rule, so Core keeps obeying any string it's given. Stored strings go through `stripConnectionStringSecrets`. The second review found that the load mapping dropped the AI sharing flags, so the migration's save would have cleared them; that's fixed and tested.
- **Server-side cancel for every stream** (probe). Phase 4 cancelled read-only queries on the server; now every `query_stream` on Postgres and MySQL/MariaDB takes one pooled connection, looks up its backend, and on an unfinished drop sends `pg_cancel_backend` or `KILL QUERY` from a fresh connection, then closes that connection instead of returning it. Because a stream runs outside a transaction, behind a transaction-pooling PgBouncer the looked-up backend could be running someone else's statement, so the cancel also checks that the backend still runs a statement with the same prefix (`statement_prefix`). The review found that MySQL reports statement text differently from what was sent (expanded parameters, trimmed whitespace, characters outside the BMP), so the prefix is cut before the first `?` and before any such character.
- **Connection limits** (probe). `ConnectionLimits` in Core, set only by the web server: 16 connections per user, counting connects and tests in flight under one lock, and pools of 6 (SQL Server: its session plus up to 4 read-only connections). The pool size reaches the engines through `Engine::open_with`, so it stays off the wire and out of the generated TS.
- **The Origin gate** (probe, then review). Every non-GET `/api/*` call except `/api/auth/*` needs a trusted `Origin`. Dev-server origins are trusted only in dev builds. When neither `BETTER_AUTH_URL` nor `ORIGIN` is set, an `Origin` naming the request's own `Host` is trusted, but only for `localhost` and IP-address hosts, since a DNS-rebinding page controls a domain name's `Host`. So a production `http://localhost:8787` works without configuration, and a domain-name install must set `BETTER_AUTH_URL` or `SEAQUEL_TRUSTED_ORIGINS`.
- **SQL Server empty logins are refused before connecting** (Task 2 live check). sqlx takes `://:pw@` and the server then uses its own default user, but SQL Server refuses an empty login, so Core refuses it first with `CREDENTIALS_REQUIRED` (one more v2 case than Task 1 wrote, 68 differing from v1).
- **MSSQL `tls_server_name` works through a tunnel**, but only against a certificate with the right name. The test container's is SQL Server's self-signed fallback with no SAN, so `require` and `verify-*` fail on it with or without a tunnel; a fake-server test checks the SNI instead.

**Task 7 findings.** The probe ran two signed-in users against a local instance (workspace cap 2, no origin variables), using only the browser-facing HTTP and WebSocket endpoints, about 250 logged requests. Held: every cross-user attempt (guessed ids, ids seen in responses, stream ids, cancel, disconnect, engine calls) got `CONNECTION_NOT_FOUND`; forged `X-Seaquel-User` headers were dropped; SQLite, DuckDB, file options and SSH were refused; eviction closed connections and streams. Found:

1. SQL in the server log: sqlx logged each statement at DEBUG and slow ones at WARN, whole.
2. No per-user connection limit.
3. Cancel, disconnect and eviction ended the stream but left the statement running on Postgres and MySQL.
4. Dev-server origins were trusted in production builds.
5. Most state-changing `/api` routes didn't check `Origin`.

All five were fixed and re-probed on a fresh instance: no SQL or canary in the log after slow queries, the 17th connect got 429 while another user still connected, `pg_sleep` was gone within a second of a WebSocket cancel, an HTTP cancel, a disconnect and an eviction, and only the install's own origin passed.

**Decisions made during execution**

- **`ConnectPolicy` has no default.** A Core that doesn't choose can't connect; the engine lookup still comes first so a refused engine is `ENGINE_NOT_AVAILABLE`.
- **Statement logging is off everywhere**, desktop and CLI included, not only on web.
- **The legacy-string cleanup is in TS**, so Core never second-guesses a stored string.
- **The demo and the tutorial stay on DuckDB-WASM** until phase 8, as decided.

**Release notes**

For the release after 2026.9.2. Phases 3 and 4's notes still apply as written.

Changes you may notice:

- **Connection strings.** A connection string is now used exactly as you typed or pasted it, and the form shows it while it's in use. Editing the host, port, database, user or SSL mode clears it and uses the fields instead. Connections saved from the wizard in earlier versions drop the string the app used to build from their fields; nothing about how they connect changes. Saved strings never keep a password.
- **SSL mode "Default".** New connections start with "Default", which uses the database's own default (SQL Server encrypts). Earlier versions defaulted to "disable". Saved connections keep the mode they have.
- **Fixes to connecting:** a password typed after pasting a connection string is used; a connection with an empty user name keeps its password; a pasted `mssql://` URL no longer connects unencrypted; MySQL `verify-ca` and `verify-full` check the certificate; SQL Server `require` and `verify-*` work through an SSH tunnel with a certificate for the server's name; SQLite and DuckDB connections never open an SSH tunnel; TablePlus `tLSMode` in a pasted URL is honoured.
- **Stopping a query stops it on the server** on PostgreSQL, MySQL and MariaDB, for every query, not only the AI's.
- **SQL no longer appears in the app's logs.**

Self-hosted web:

- **Action needed if your install is reached by a domain name:** set `BETTER_AUTH_URL` to its public URL (or list the origin in `SEAQUEL_TRUSTED_ORIGINS`). Every state-changing request now needs a trusted `Origin`, and a domain name is not trusted on its own. Installs reached at `localhost` or an IP address work without configuration.
- **Each user can have 16 open connections.** The 17th is refused until one is closed.
- **Queries stream over one WebSocket per browser tab.** A user can have 8 tabs streaming at once; more show a message asking to close some. The path is now `/api/rpc/stream` (it was `/api/db/stream`); a reverse proxy that allowed WebSocket upgrades only on the old path must allow the new one.
- **When more than 1,024 users have sessions open on the server**, the least recently active user's database connections are closed, and their app says so and offers to reconnect.
- **Stopping a query or disconnecting stops the query on the database server.**
- **The server no longer logs users' SQL.**

---

## Checkpoint

The full check list, run on 2026-09-27 one step at a time on the shared `scratchpad/p5a/target`, with all five containers healthy and the live env (`SEAQUEL_TEST_POSTGRES`, `_MYSQL`, `_MARIADB`, `_MSSQL`, `SEAQUEL_TEST_SSH`, `SEAQUEL_TEST_REQUIRE_ENGINES=1`; values as in `ci.yml`):

| Check | Result |
|---|---|
| `npm run crates:check` | pass |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, live | pass: 1,258 passed, 0 failed, 3 ignored (148 test targets, 371 s) |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` (Decision 11) | failed, then fixed (below) |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run cli:build`, `cargo check -p seaquel` | pass |
| `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 34 passed |
| `npm run types:gen`, generated types unchanged | pass (the 134 files hash the same before and after) |
| `npm run check` | pass: 0 errors, 0 warnings |
| `npx oxlint --type-aware --type-check --deny-warnings` (CI's lint step) | failed with 2 type errors, then fixed (below) |
| `CI=1 npx vitest run` | pass: 1,376 tests in 72 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |

CI doesn't run oxfmt. `npx oxfmt --check` on the repo flags 34 files, nearly all older plans and TOML files that were already unformatted at `5d12ac6`, and it can't parse `ci.yml` (line 213). Of the files phase 5a touched, only `crates/seaquel-rpc/Cargo.toml` is newly flagged (one long dev-dependency line). Left as it is, matching the other Cargo files.

**Two CI failures, fixed after the run:**

1. **Core no longer builds for wasm32 with `browser`.** `seaquel-rpc` now depends on `seaquel-core` with `features = ["workspace"]` (Task 3's review, so `db.connect` is always built), and `browser` refuses `workspace` with its `compile_error!`. `cargo clippy --target wasm32-unknown-unknown -p seaquel-core --no-default-features --features seaquel-core/browser` alone passes; adding `-p seaquel-rpc` fails. Fixed by keeping Core's guard and giving `seaquel-rpc` a `workspace` feature (turning on `seaquel-core/workspace`) in place of the no-op `connect`. `src-tauri` and `seaquel-server` turn it on; the browser line leaves it off, and there `db.connect`/`db.test` answer `NOT_SUPPORTED`. The wasm32 line, CI clippy, `-p seaquel` clippy, fmt, `crates:check` and the `seaquel-rpc`/`seaquel-server` tests pass again.
2. **oxlint's type check fails on `src/lib/core/http.test.ts`** (lines 74 and 132): `Type '(url: string) => FakeSocket' is not assignable to type '(url: string) => WebSocketLike'`. `svelte-check` and vitest pass; only oxlint's type-aware check (tsgo) sees it. The cause was `FakeSocket.readyState`, inferred as `number` where tsgo's `WebSocket["readyState"]` is a literal union. Typing the field as `WebSocket["readyState"]` fixed it; the full oxlint step exits 0 and `src/lib/core` vitest passes.

**Manual checks:** all pass (the owner, 2026-09-27).

**Not run:** the release workflow and a signed build.

---

## Follow-ups (not in 5a)

- **Later slices:**
  - 5b: query execution service.
  - 5c: connection, project and saved-query CRUD in Core.
  - 5d: the `.seaquel` format, repos, dashboards and workflows.
- **`StorageChanged` and the other `CoreEvent`s,** when a slice needs them.
- **Detect lost connections and tunnels.** `CONNECTION_CLOSED` and `TUNNEL_CLOSED` are reserved; today the GUI learns of either on its next call. It needs a health signal from the pools and from `seaquel-ssh`.
- **Cancel gaps.** SQLite's plain stream isn't interrupted on cancel (local, so it only costs CPU), and a non-streamed `query`/`execute` still running at a disconnect isn't cancelled on the server.
- **A live SQL Server TLS check with a real certificate.** `tls_server_name` is tested against a fake server; the test container's certificate has no SAN, so `require`/`verify-*` can't pass against it with or without a tunnel.
- **Move the legacy connection-string cleanup into Core** with connection CRUD (5c), if Core then owns saving rows.
- **The desktop transport has no live automated run.** `core_stream`, `core_events` and the per-webview sinks are covered by Rust unit tests and `TauriCoreClient` by vitest with a mocked `invoke`; only the manual checks run them in the app.
