Critical and high severity

Security

- Cross-tenant data theft via the storage API (critical, web build). src/routes/api/storage/exec/+server.ts:29 runs any SQL the client sends against the user's meta.db. SQLite's ATTACH DATABASE is not disabled, per-user paths are predictable, and /api/team hands out every member's user id. One member can attach another member's database or the shared auth.db and read password hashes, license keys, or set themselves as owner. The comment claiming the SQL "comes from our own repo code only" is not enforced anywhere.
- XSS from database content in the AI assistant (critical, desktop). src/lib/components/ai-assistant.svelte:307 renders model output through {@html marked.parse()} with no sanitizer, and src-tauri/tauri.conf.json:24 sets CSP to null. Query results and mention context are fed to the model, so a hostile cell value can become script running with full Tauri IPC. The filesystem capability in src-tauri/capabilities/default.json grants read, write, remove, and rename on **, so that script has the whole home directory.
- AI-generated SQL runs without the destructive-statement check (high). The "Open in editor" button at ai-assistant.svelte:317 calls db.queries.execute directly. The confirmation dialog only lives in the query-editor wrapper, so a prompt-injected DROP TABLE runs on click. The command palette has the same bypass.
- SSH tunnel accepts any host key (high). src-tauri/src/ssh_tunnel.rs:82 returns Ok(true) unconditionally, so a man-in-the-middle on the SSH hop sees the SSH password and every forwarded database credential.
- WebSocket stream skips the license and membership gate (high, web). server.js:103 only checks that a session exists. Suspended installs and revoked members can still stream SQL, while the HTTP path in hooks.server.ts is gated correctly.
- Sign-in brute-force limit is spoofable (high, web). Better Auth's rate limiter keys on the leftmost X-Forwarded-For value by default and src/lib/server/auth.ts:207 never sets advanced.ipAddress. Rotating the header gives unlimited password guesses.

Data loss and correctness

- Adding a label deletes the saved password from the keychain (high). persistence-manager.svelte.ts:838 deletes the DB password, SSH password, and passphrase whenever options is omitted. Label changes and the AI-model setter call it that way. The user only notices at the next launch when auto-reconnect fails.
- Legacy JSON migration re-runs forever (high). src/lib/storage/db.ts:60 re-imports old JSON files whenever the connections table is empty, and nothing ever deletes those files. Deleting all connections resurrects old ones and overwrites tabs, saved queries, and shared repos with the stale snapshot.
- CTE queries return no rows (high). query-utils.ts:47 classifies by first keyword only, so WITH ... SELECT falls into the utility branch at query-execution.svelte.ts:373 and its rows are discarded. SHOW, PRAGMA, and EXPLAIN behave the same way.
- Inline edits can hit the wrong row and report success (high). When column sources cannot be resolved, query-execution.svelte.ts:1226 falls back to a regex FROM match and keys the row by display column name. Then query-crud.svelte.ts:119 ignores rowsAffected, so a zero-row or wrong-row update still shows as saved.
- Bigint keys lose precision end to end (high). Postgres INT8 is decoded to a JSON number, JavaScript rounds above 2^53, and crates/seaquel-db/src/lib.rs:248 binds every number as f64. DuckDB and MSSQL correctly try i64 first, so the sqlx drivers are the outliers.
- Parameter substitution is broken for MySQL and MariaDB (high). query-params.ts:126 sends them down the $N placeholder path, but MySQL only understands ?. The inside-string rewrite also uses ||, which is logical OR in MySQL.
- Git conflict resolution leaves the repo stuck (high). src-tauri/src/git.rs:590 builds the commit with HEAD as the only parent and never reads MERGE_HEAD or clears merge state, so the push is rejected and the next pull re-merges the same conflicts. The credential callback is stateless and libgit2 retries it without limit, so a rejected key hangs the app, and all git commands run synchronously on the main thread.
- License nudge store wipes the user's opt-out on a transient load error. src/lib/stores/license-nudge.svelte.ts:78 sets initialized in finally, so the next query persists zeros over a saved "personal" answer. It also writes to SQLite on every query forever, even for licensed users.

Medium severity and maintainability

- Persistence orchestration loses recent work. Save-on-unload is not awaited, destroy() cancels pending writes instead of flushing, one global debounce timer per category clobbers other projects' pending writes, and schema tabs are serialized but never restored. Starred dashboards lose their star on restart because the live loader skips the column.
- Every keystroke pause rewrites the whole project, including saved workflows that embed full result-row arrays, and each executed query rewrites up to 500 history rows. History truncation also drops favorites.
- Naive SQL lexing is re-implemented four times for statement splitting, parameter substitution, destructive detection, and pagination. Comments defeat all of them: SELECT * FROM t -- note gets LIMIT appended on the comment line, and DELETE FROM t -- where? skips the confirm dialog. Consolidating on the existing state-machine tokenizer would fix most of these at once.
- Pending changes apply non-atomically even though the Rust driver exposes a transaction API that no frontend provider calls.
- Dialect drift. SQLite gets SET col = DEFAULT, MSSQL gets Postgres ALTER syntax and non-N string literals, and validateIdentifier in src/lib/db/index.ts:147 rejects legal names with spaces or hyphens so those tables cannot be expanded in the sidebar.
- DuckDB runs synchronously on the tokio runtime and cancellation cannot interrupt a query before its first row. The open_path command opens any string, and one call site passes a URL scraped from remote HTML.
- CSV export has no formula-injection guard and SQL export emits unquoted identifiers.
- Dead code: shared-connection-manager.svelte.ts, record-utils.ts, the greet command, eight unused crates in src-tauri/Cargo.toml, and five identical error structs in the Rust backend.
- Dependencies. Production-relevant advisories are better-auth (critical, device-auth approve bypass), ws (high, memory disclosure), and @sveltejs/kit (high, redirect DoS). All are minor version bumps away. The rest of the 24 advisories are dev-only.
---

# Verification (2026-09-22, against c8dd890)

20 of 23 findings confirmed, 2 partial, 1 wrong.

Corrections to the review above:

- Dependencies: WRONG. `npm audit` has no critical/high; no `ws` advisory. The real issue is `dompurify <=3.4.12` (XSS, via monaco-editor) and low-severity `cookie <0.7.0` (via @sveltejs/kit). Fix with `overrides`.
- Storage API: worse than stated. The easiest target is `${DATA_DIR}/auth.db` (sessions, `member_license.is_owner`, owner license key), so no user ids are needed. An ATTACH through exec persists on the pooled better-sqlite3 handle, so later /query calls can read it. `VACUUM INTO` writes arbitrary files. better-sqlite3 has no authorizer or `sqlite3_limit`, so the guard must be a statement check in the endpoints (not `adapter()`, which server-side schema code uses for PRAGMA).
- Keychain wipe: also writes `savePassword`/`saveSshPassword`/`saveSshKeyPassphrase = undefined` to the row, so auto-reconnect skips the keychain even if the delete were removed.
- Legacy JSON migration: re-running also deletes saved queries created after migration (`saveAll` NOT IN), resurrects deleted projects, and overwrites license/theme/onboarding state.
- Rate limiting: better-auth 1.7.5 trusts XFF only when it holds exactly one value. Behind an appending proxy the IP resolves to null, so all clients share one bucket (5 bad sign-ins/min locks everyone out).
- CTE classification: PARTIAL. Same `select()` path; the rows are discarded. Also: with pending changes on, WITH/EXPLAIN get queued as changes; `INSERT ... RETURNING` rows are lost.
- Schema tabs: dropped because `connection_id` is never saved (`project-state-repo.ts:274`), not because restore is missing. Starred dashboards: `restoreDashboards` (`state-restoration.svelte.ts:217`) omits `starred`; the SQL loader reads it. `destroy()` is never called, so its cancel-vs-flush bug is latent.
- SQL lexing: 3 implementations plus scattered regexes. The one proper tokenizer is `splitSqlStatements` in `src/lib/db/sql-parser.ts`.
- Dead code: 7 unused crates in `src-tauri/Cargo.toml` (sqlx, rust_decimal, thiserror, duckdb, async-stream, tokio-util, base64), not 8.

New issues found during verification:

- `git_resolve_conflict` (`git.rs:702`) joins `file_path` without validation (path traversal).
- Fast-forward pull (`git.rs:271`) does a forced checkout, discarding uncommitted changes.
- Conflict list only includes entries with `c.our` (`git.rs:318`); files deleted on our side are missed.
- AI "Open in editor" and command palette also skip the parameter dialog.

# Remediation plan

## Phase 1: security hotfixes

Status (2026-09-22): Phase 1 complete except fs-scope narrowing, which is deferred to Phase 5 (user decision: with the sanitizer and CSP in place it is defense in depth, and an XSS would still reach keyring and db_query over IPC).

- 1: allowlist (SELECT/INSERT/UPDATE/DELETE/REPLACE/WITH) in `src/lib/server/storage-guard.ts`, applied to all three endpoints.
- 2: DOMPurify in `renderMarkdown`; CSP set in `tauri.conf.json` (connect-src stays broad because AI base URLs are user-configurable).
- 3: AI "Open in editor" now only opens the tab; command palette dispatches `EXECUTE_ACTIVE_QUERY_EVENT`, handled by the editor showing the active tab via `handleExecute`.
- 4: `server.js` authorizes WebSocket upgrades via gated `/api/account/stream-access`.
- 5: `server.js` sets `x-seaquel-client-ip` from the socket (`shared/client-ip.js`, `SEAQUEL_TRUSTED_PROXIES`); Better Auth and adapter-node (`ADDRESS_HEADER`) key on it.
- 6: `check_server_key` verifies against `~/.ssh/known_hosts`. Unknown keys return `UNKNOWN_HOST_KEY` + fingerprint, which `createSshTunnelWithHostKeyCheck` turns into a trust-on-first-use prompt (`ssh-host-key-dialog.svelte`); accepting retries with `trust_new_host_key`, which calls `learn_known_hosts`. `HOST_KEY_MISMATCH` always fails and is never offered for trust.
- 7: `overrides` for dompurify and cookie; `npm audit` clean.

Not yet verified by running the app: CSP smoke test (Monaco, AI chat, ERD export, theme import) and an end-to-end SSH tunnel against a real host.

1. Storage API: in `/api/storage/{exec,query,transaction}`, strip leading comments and reject statements starting with ATTACH/DETACH/VACUUM/PRAGMA. Follow-up: replace client-sent SQL with named operations (~73 statements, one dynamic).
2. AI XSS: add `dompurify` as a direct dependency and sanitize `marked.parse()` output in `renderMarkdown`. Set a real CSP in `tauri.conf.json`. Narrow the fs scope to `$APPDATA/**` + `$TEMP/**` and add `tauri-plugin-persisted-scope` for user-picked folders.
3. AI/command-palette execution: add a user-initiated `requestExecute(tabId)` that runs the destructive and parameter checks, with the confirm dialog lifted to app root. Don't put it in `execute()`; pagination and cell editing re-run it.
4. WebSocket gate: extract `assertApiAccess(locals)` from `handleApiGate`, add gated `/api/db/stream-auth`, and point `resolveSession` in `server.js` at it.
5. Rate limiting: set `advanced.ipAddress` with `trustedProxies` from env; document `ADDRESS_HEADER`/`XFF_DEPTH`.
6. SSH host key: `russh_keys::check_known_hosts`, trust-on-first-use prompt, fail on mismatch.
7. `overrides` for `dompurify >=3.4.13` and `cookie >=0.7.0`.

## Phase 2: data loss

Status (2026-09-22): done. Tests: `src/lib/hooks/database/persistence-manager.svelte.test.ts` (new) and two added cases in `src/lib/stores/license-nudge.svelte.test.ts`.

- `persistConnection` treats `options` as an intent: a missing flag falls back to the connection's stored flag and leaves the keychain alone; only an explicit `false` deletes.
- `db.ts` records `app_state.json_migration_done`; the empty-connections re-attempt runs at most once more for pre-marker installs.
- License nudge sets `initialized` only on a successful load, and `recordQuery` returns early unless `licenseStore.status === "personal"`.
- Schema tabs persist and reload `connection_id`; `restoreDashboards` maps `starred`.
- Per-project/per-connection debounce timers (Maps) replace the two global ones; `cancelPendingPersistenceFor(projectId, connectionIds)` is used when deleting a project so other projects' pending writes survive. `destroy()` flushes instead of cancelling.
- Desktop `onCloseRequested` awaits `flush()` before destroying the main window (standalone windows keep their own close handling).
- History trimming keeps favorites beyond the 500-item cap.

Follow-ups from manual testing (2026-09-22):
- The close-on-flush handler needs `core:window:allow-destroy`; `core:window:default` does not include it, so the main window silently refused to close.
- Restored schema tabs render empty until their connection is live (auto-reconnect is user-triggered). `loadTableMetadataInBackground` now points open tabs at freshly loaded metadata (`syncOpenTabsForTable`) and closes tabs whose table no longer exists (`dropTabsForMissingTables`). Tab writes are per project because BaseTabManager's helpers target the active project.

- `persistConnection`: fall back to the connection's save flags when `options` is omitted; delete a secret only when its flag is explicitly false (or add a metadata-only save for labels and AI model).
- JSON migration: set `app_state.json_migration_done`; never re-run once set.
- License nudge: set `initialized` only on successful load; return early from `recordQuery` for licensed users.
- Save `connection_id` for schema tabs; map `starred` in `restoreDashboards`.
- Per-project persistence timers (Map); await `flush()` in `onCloseRequested`.
- History trim keeps favorites.

## Phase 3: query correctness

- `detectQueryType` looks past WITH; VALUES/SHOW/PRAGMA/EXPLAIN/DESCRIBE are reads. Fill `columns` from the prepared statement in Rust.
- Inline edits: refuse when a column source is unresolved or the query has multiple tables (no regex fallback). Check `rowsAffected` in `updateCellDirect`, `setCellDefaultDirect`, `deleteRow`.
- Bigints: sqlx bind macro tries i64 → u64 → f64; decoders emit integers outside ±2^53 as strings.
- MySQL/MariaDB parameters: `?` placeholders and `CONCAT(...)` inside strings.
- One shared tokenizer (from `sql-parser.ts`) for WHERE/LIMIT/ORDER BY checks and parameter scanning; paginate as `SELECT * FROM (...) q LIMIT ...`.

## Phase 4: Git

- In Merge state, add MERGE_HEAD as a second parent and call `cleanup_state()`.
- Bound credential callback attempts.
- Make git commands async (off the main thread).
- Validate `file_path` in `git_resolve_conflict`; stop the forced checkout from discarding local changes.

## Phase 5: medium items and cleanup

- `transaction()` on the provider interface; use it in pending-changes `executeAll`.
- Persist only the changed tab; strip result rows from saved workflows; append history instead of replacing.
- Dialects: SQLite writes the literal default; MSSQL `sp_rename`, `ADD` without COLUMN, `N'...'` literals; quote identifiers instead of rejecting them in `validateIdentifier`.
- DuckDB: `spawn_blocking` + `interrupt_handle()`.
- `open_url` command allowing only https; validate scraped URLs.
- CSV formula-injection guard; quote identifiers in SQL export.
- Remove dead TS files, `greet`, 7 unused crates; merge error structs.

Each Phase 2/3 fix gets a regression test first.
