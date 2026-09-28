# Phase 5b Implementation Plan: query execution in Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** On desktop and web, running SQL from the editor is one Core call. `db.run` takes the editor's text, the cursor, the parameter values and the page size. Core splits the text, finds the statement at the cursor, substitutes `{{param}}`s and checks each statement's type. It asks for confirmation before destructive statements, decides whether to stream or page, pages with the count probe, times each statement and records history. `db.page` re-pages one statement. The TypeScript keeps the parameter dialog, the destructive prompt, the result grid and the per-tab state. It no longer decides what runs or how. History is written by targeted storage calls, never by replacing the whole list.

**Architecture:**
- **`seaquel-workspace::run`** plans a run and does no I/O. It turns text, target, parameter values and engine into a list of statements, each with its substituted SQL, bind values, query type, kind (page, stream, write or utility) and the table references for inline editing. It also lists the destructive statements. Parity fixtures recorded from today's TypeScript pin it.
- **Core.** `Workspace::run` and `Workspace::page` carry out a plan on one of the workspace's connections under one stream id. They register it like a query stream, so cancel, early cancel, disconnect and eviction already work. Core gains an `Executor` for its clock (`elapsedMs`, history timestamps), and the connection keeps its SQL engine, so MariaDB is scanned as MariaDB. A successful run appends one history row through a new `seaquel-storage` function that also applies the 500-entry cap.
- **`seaquel-rpc`.** `db.run` and `db.page` are stream calls, served next to `db.queryStream` by `dispatch_stream`. `CoreEvent` gains `run`. The transports change only where they name `queryStream`.
- **TypeScript.** `QueryExecutionManager` becomes a view model over run events from a `QueryRunner`. `CoreQueryRunner` serves desktop and web. `TsQueryRunner` is today's logic, moved and kept for the demo until phase 8. The history manager is a read-only cache with a favourite toggle.

**Tech Stack:** Rust (`seaquel-sql`, `seaquel-workspace`, `seaquel-core`, `seaquel-storage`, `seaquel-rpc`, `seaquel-server`, `src-tauri`), TypeScript/Svelte 5, vitest, the e2e Docker databases.

**Inputs:**
- The design doc: "Core, workspaces and state", "The RPC surface for GUIs", phase 5 in "Migration plan", and "Phase 5a cost" with "What this means for phase 5b onwards".
- The phase 5a plan, its execution notes and effort log.
- A read-only survey of the query runner, checked line by line on 2026-10-02 (below).

---

## What the code shows

All line numbers are as of `cf2085d` plus the working tree.

1. **The runner is `src/lib/hooks/database/query-execution.svelte.ts`** (1,326 lines), and about half of it is logic.
   - **Statement at cursor:** `resolve-query.ts:43-78`.
   - **Splitting:** `:774`.
   - **Substitution:** `:575` (at cursor) and `:811` (run all).
   - **Query type:** `:604` and `:817`.
   - **The stream-or-page decision (`shouldStream`):** `:106-109`. A SELECT streams when the page size is 0 or when `hasRowLimit` finds its own LIMIT/OFFSET/FETCH/TOP.
   - **Utility statements** go through `provider.select` and writes through `provider.execute` (`:397-440`). DuckDB needs `select` for `SET`.
   - **Paging:** `client.paginate` fetches `pageSize + 1`. `countQuery` runs only when the page is full, and a failed count is estimated as `offset + pageSize + 1` (`:442-498`).
   - **Streaming** fills the result in place through the `$state` proxy (`:178-257`), with `dedupeColumnNames` on the first batch.
   - **Cancel:** one `AbortController` per tab (`:48`, `:73-97`), which reaches only streams.
   - **Multi-statement runs** continue after an error (`:894-897`) and hide utility results unless every result is one (`:344-348`).
   - **Error results:** `:318-338`. **Re-paging:** `:968-1092`. **The source table and column sources:** `:115-165`. **Edit routing:** `:1257-1325`.
   - **Pending changes:** a non-SELECT is queued instead of run when they are on (`:618`, `:820`).
2. **The paging bug.**
   - A result's `statementSql` is the text before substitution: seeds at `:633` and `:838`, and paged results at `:696` and `:890`.
   - `executeStatementAtIndex` re-runs that text with no binds: the type check at `:1011`, the stream seed and `runStreamingStatement(…, undefined, …)` at `:1028`/`:1038`, and `executeStatement(existingResult.statementSql, …)` at `:1054`.
   - So paging, or changing the page size of, any statement that had `{{param}}`s sends the raw `{{name}}` to the database. This happens on desktop, on web and in the demo.
3. **The destructive prompt runs only from the editor's buttons** (`components/query-editor/execution.svelte.ts:61-125`), on the text before substitution. Three other callers run a tab with no prompt:
   - `db.queries.execute(tabId)` from `services/file-drop.svelte.ts:98`;
   - the rerun after a grid row delete (`components/query-editor/cell-editing.svelte.ts:78`);
   - `setCellDefault`'s rerun (`query-execution.svelte.ts:1165`).
   
   Those reruns also send no parameter values.
4. **History.**
   - `QueryHistoryManager.addToHistory` (`query-history.svelte.ts:21-50`) prepends to the in-memory list, schedules a debounced save and calls `licenseNudgeStore.recordQuery()`.
   - The save is `persistConnectionData` (`persistence-manager.svelte.ts:559-575`). It replaces the connection's whole history with `serializeQueryHistory` (`:378-400`), capped at `MAX_HISTORY_ITEMS = 500` (`:60`) with favourites kept.
   - `flush()` does the same for every connection (`:214`). The `history:<id>` load-guard key refuses the save after a failed load.
   - `scheduleConnectionData` has only this one user (`database.svelte.ts:109`, `:249`).
   - Runs record history:
     - only on page 1;
     - never for a cancelled or failed stream;
     - at the cursor, with the statement's text before substitution (`:666`, `:709`);
     - for run all, with the whole buffer and the first displayed result's time and row count (`:936-939`). That result can be an error result: a failed paged statement in a multi-statement run is still recorded.
   - Pending changes append history too (`pending-changes.svelte.ts:165`).
   - There is no per-item delete. `removeByConnection` runs when a connection is removed.
   - `query_history.connection_id` has a foreign key to `connections` with `foreign_keys = ON` (`seaquel-storage/src/open.rs:129`), so appending for an unsaved connection fails.
   - `query_history::replace_all` backs a frozen repo fixture (`crates/seaquel-storage/tests/repos.rs:217`), so the storage function stays.
5. **Timing is client-side** (`performance.now()` around each call), so today's numbers include IPC or HTTP.
6. **Smaller things the fixtures must pin:**
   - An empty paged result has no column names: `Object.keys` of zero row objects (`:501`).
   - A comment-only buffer run at the cursor runs the whole buffer as a utility statement: `getStatementAtOffset` returns `null` and `resolveQueryOrThrow` falls back to `tab.query` (`resolve-query.ts:60-63`).
   - A substitution error at the cursor toasts and runs nothing. In run all it becomes that statement's error result.
   - A new run on a tab aborts only the previous stream. A non-streamed statement loop keeps going.
7. **Core already has most of the pieces:**
   - `seaquel-sql` (split, statement at cursor, substitution, query type, destructive reason, row limit, count query, table and column references);
   - `Dialect::paginate`;
   - the per-workspace stream registry with early cancel and server-side cancel;
   - the `db` RPC group.
   
   It has no clock: no Core crate uses an `Executor` yet (`crates/clippy.toml` forbids `Instant` and `SystemTime`).
8. **Core scans MariaDB as MySQL.**
   - `sql_engine` (`seaquel-core/src/lib.rs:442-451`) maps driver ids, and a MariaDB connection's driver is `mysql`.
   - The TypeScript passes `connection.type`, so today's editor scans `/*M! … */` as MariaDB code.
   - A Core that planned runs with the driver id would hide a statement inside such a comment from the split and the destructive check.
9. **Hand-typed transactions on pooled connections.** Postgres and MySQL/MariaDB run `query` and `execute` through `self.pool` (`seaquel-engine/src/sqlx_driver.rs:150`, `:262`), and `query_stream` checks out one pooled connection per stream. Neither `driver.rs` sets `after_release` or `before_acquire`. So after a typed `BEGIN`:
   - the next statement may run on another pooled connection;
   - the connection holding the open transaction goes back to the pool;
   - a later statement that gets it runs inside that transaction.
   
   Whether sqlx resets a connection that comes back mid-transaction hasn't been checked; it needs a live check (Follow-ups). SQL Server runs on one session, so a typed transaction works there until a reconnect drops it (CLAUDE.md).
10. **MCP's `run_saved_query`** (`crates/seaquel-mcp/src/tools/saved.rs:183-228`) shares only `seaquel_sql::params::substitute` with the editor. It runs one read-only statement with row, byte and time limits, doesn't split, has no paging and writes no history.

## Answered questions

The owner settled these on 2026-10-02. The plan is written with them.

1. **Scope is "run + history".**
   - New `db.run` (a stream). Core owns statement splitting, the statement at the cursor, `{{param}}` substitution, the query type, the stream-or-page decision, paging with the count probe, server-side timing and recording history.
   - `db.page` re-pages one statement and keeps its substituted SQL and bind values.
   - Not in 5b: inline edits, CRUD, pending changes, the data tab's filter/sort/count query building, and workflows. They are the next slice (Follow-ups).
2. **Core records history.**
   - It appends one row when a run succeeds, on page 1 only, as today.
   - The GUI keeps a read-only cache and the favourite toggle. There's no per-item delete today, so none is added.
   - Whole-list `replaceAll` saves are replaced by targeted calls: append (with the 500-entry cap, favourites kept) and set favourite. These are new `seaquel-storage` functions and `StorageRequest` variants.
   - Nothing may be lost for existing users. The TypeScript must never replace the list after Core has appended.
   - The license nudge stays in TypeScript and is triggered by the run's completion.
3. **Core enforces destructive statements.**
   - `db.run` refuses with a new `CONFIRM_REQUIRED` code, listing the statements and reasons, unless `confirmed: true`.
   - The GUI keeps its synchronous wasm prompt and sends `confirmed` after the user agrees.
4. **Pending changes: atomic for DML.** This is recorded for the slice that moves pending changes (Decision 17), not built here.

**Settled after the plan was drafted (2026-10-02):**
5. **History when one statement of a run fails:** record nothing (Decision 11).
6. **Reruns ask for confirmation** when the tab holds a destructive statement: the row-delete and Set default reruns and the file-drop run. One rule everywhere.
7. **MCP stays on its own path in 5b** (Decision 14).
8. **No row cap for "Stream all" in 5b.** Core's existing `RESULT_TOO_LARGE` ceiling still applies.
9. **Statements `query_type` calls `other` that return rows show them** (Decision 18): `WITH …`, `SHOW`, `EXPLAIN`, `PRAGMA`, `VALUES`, `TABLE` and DuckDB's `FROM …`. Today their rows are discarded. The proper fix, a row-returning statement kind with paging and streaming, is a high-priority Follow-up.

**Also settled:**
- **The demo is frozen until phase 8.** It keeps the TypeScript runner behind a `QueryRunner` seam (Decision 13).
- **The paging-with-params bug is fixed first,** in TypeScript with tests (Task 1). The demo gets the fix too.
- **Hand-typed BEGIN/COMMIT isn't handled in 5b.** It is documented (finding 9) and listed in Follow-ups.
- **Timing comes from Core** (`elapsedMs`). The GUI may run a live counter while it waits.
- **MCP's `run_saved_query`:** see Decision 14, which keeps it where it is.
- **The AI's `run_query` and dashboards stay on `executeReadOnly`** (`db.queryStream` with `readOnly`). 5b must not change them (Decision 15).

Execution is subagent-driven.

## Decisions (2026-10-02)

### 1. Two stream calls, `db.run` and `db.page`

- **Both are streams.** They're served only by `dispatch_stream`, like `db.queryStream`; `dispatch_workspace` refuses them with `INVALID_ARGUMENT`.
- **The id field is `streamId`,** not `runId`. The transports already match a start frame to its request by `streamId`, and `db.cancel { streamId }` cancels a run the same way it cancels a stream. It must be unique per run, with the same rules as `queryStream`'s.
- **`db.run` always runs page 1.** Today's `page` argument to `execute`/`executeCurrent` is always 1 from every caller. Paging is `db.page`. So "history on page 1 only" becomes "`db.run` records, `db.page` never does".
- **Parameters are substituted only when the call has `params`.** That means the dialog was shown. Without them, `{{name}}` goes to the database as typed, as today. Reruns without values are a Follow-up.

### 2. Paging is stateless: the client sends the statement back

- Each `statementStart` carries a `source: {sql, params}`, the SQL after substitution and its bind values. The client keeps it on the result and sends it back in `db.page`.
- Core keeps no per-run state, so nothing leaks when tabs close, reloads and reconnects don't lose it, and eviction has nothing to clean up.
- **This is no new trust.** `db.page` runs SQL the client sends, which `db.query` already does on the same connection.
- **`db.page` still refuses any statement whose query type isn't `select`** (`INVALID_ARGUMENT`), so nothing can skip the confirmation through it. It isn't a write barrier, though (probe N1): `SELECT … INTO`, a SELECT calling a writing function and `FOR UPDATE` are SELECTs by their first word and get through, as they do through `db.query` and `deferWrites`.
- **A handle was rejected.** It would need a lifetime (per tab, per run, or an LRU per workspace), a web memory cap and an eviction story, all to save sending back text the client already holds.

### 3. Text and cursor on the wire

- **`cursor` is a UTF-16 offset** into `text`, Monaco's unit. Core converts it with `utf16_to_byte`, the function `seaquel-wasm` uses today. It moves from `crates/seaquel-wasm/src/offsets.rs` to `seaquel_sql::offsets` with its tests, and `seaquel-wasm` re-exports it. So Core and the editor's wasm pick the same statement for the same offset: one function, one set of rules (before, between and after statements).
- **Well-formed text.**
  - `serde_json` refuses a lone surrogate, which `JSON.stringify` writes as `\udXXX`.
  - The run and page requests go as JSON text (`core_stream` and `/rpc/stream`), so the client sends `text.toWellFormed()`, with a regex fallback where the WebView lacks it.
  - U+FFFD is one UTF-16 unit, as the surrogate was, so the cursor stays valid. It is the same replacement `TextEncoder` already makes for wasm.
- **Statement text Core returns** (`statementStart.sql`, the destructive list, the history row) comes from the well-formed text, so it matches the editor except for that replacement.

### 4. Planning is pure; Core carries it out

- **`seaquel_workspace::run::plan`** takes text, target, parameter values, engine, page size and `defer_writes`, and returns:
  - the statements, each with its display SQL, source, query type, kind, table reference and column references, or a planned failure;
  - the destructive list (Decision 7);
  - the text a history row would record.
- `plan` does no I/O and can't panic on input. `seaquel-workspace` gains a `seaquel-sql` dependency, which `check-crate-deps` allows for a domain crate.
- **`Workspace::run` and `Workspace::page`** register the stream, then execute each planned statement on the connection's driver and emit events.
- **The engine is the connection's, recorded at connect.**
  - Core's `Connection` gains `sql_engine: SqlEngine`, set in `Workspace::connect` from the plan's database type, so MariaDB is `SqlEngine::Mariadb`.
  - `Core::connect` (tests, engine smoke) maps the driver id as today.
  - The run and the read-only check (`check_read_only`, `check_one_statement`) both use it, so a MariaDB `/*M! … */` is code to the split, the destructive check and the AI's check alike.

### 5. How each kind runs

| Kind | When | Driver call | Notes |
|---|---|---|---|
| `page` | `select`, page size > 0, no own row limit | `query_stream` of `dialect.paginate(sql, pageSize + 1, offset)`, collected | The extra row is dropped. When the page was full, `count_query` runs through `query` with the same binds. A failed count is estimated at `offset + pageSize + 1` and `countEstimated: true`, logged with its code only. |
| `stream` | `select` with page size 0 or its own LIMIT/OFFSET/FETCH/TOP | `query_stream` | As `db.queryStream`: batches straight through, no row cap. |
| `write` | `insert`, `update`, `delete` | `execute` | `rowsAffected`, `lastInsertId`. |
| `utility` | `other` | `query`, unpaged | As today (DuckDB's `SET` needs `query`). With no columns the result is dropped, as today. With at least one column it is a rows result (Decision 18). `query` keeps its 100,000-row cap: past it the statement fails with `RESULT_TOO_LARGE`. |

- **Pages go through `query_stream`,** not `query` as today. A cancel then stops a paged SELECT on the server too (5a's `pg_cancel_backend`/`KILL QUERY`). The rows and decoding are the same.
- **Limits.**
  - `pageSize` above `max_query_rows() - 1` (99,999: a page fetches one row more) is `INVALID_ARGUMENT`, and so is `page` 0 or an offset that overflows.
  - A page reads the row past the page and no more, then drops the driver's stream, whatever the SQL made of the limit. The SQL dialects put `LIMIT` on a line of its own, and Core ends a trailing line comment before wrapping the count (Task 4 review: a trailing `--` swallowed both).
  - `db.page` takes exactly one statement, split as the run splits.
  - "Stream all" stays unbounded, as today; a row cap for it is a Follow-up.
  - **Run size (Task 7 probe, I1; owner, 2026-10-02: web only).** Core's `RunLimits` (`CoreBuilder::run_limits`, a sibling of `ConnectionLimits`, passed into `plan` as the page cap is) bounds a run's text, and a page's SQL, before anything scans it, and run all's statement count, both `INVALID_ARGUMENT`. The web server sets `WEB_RUN_LIMITS` (2 MiB, 10,000); the desktop, the CLI, MCP and the demo's runner have none.
  - **Substitution growth (Task 7 probe, N3), in Core** (desktop, web, CLI, MCP; the demo's `TsQueryRunner` has no budget). With `params`, filling in the values may add at most 32 MiB to the run (`check_substitution_budget`: each statement's bound from one `seaquel_sql::params::SizeBound` less its length, before substituting; `MAX_RUN_SUBSTITUTED_BYTES`), `INVALID_PARAMETERS` for the whole run in either target. Only the growth counts, so a 50 MB dump with a few parameters runs on the desktop. `plan` builds `params::Values` once and substitutes each statement with `substitute_with`; `SizeBound` costs a value the first time a statement uses it, and a decimal's written-out exponent by arithmetic (review C1: 1,000 statements × 100 `1e1048575` values took 4.1 s before).
  - **Parameter values (review C1), web only.** `RunLimits` also caps the values a run sends: `WEB_RUN_LIMITS` allows 1,000 and 1 MiB of them (`param_bytes`), `INVALID_PARAMETERS` past either. `db.page` isn't checked: it substitutes nothing, and its binds are per-use copies on MySQL, so a page could be refused for a query that ran. `plan`'s options are one `PlanOptions` (page size, `deferWrites`, page cap, limits).
  - **Column references** (`seaquel_sql::ast::column_refs`) are `None` for a statement over 64 KiB (`MAX_COLUMN_REFS_BYTES`) without parsing it; the grid then isn't editable, as for `*`. The editor's other sqlparser calls (Visual tab, builder) keep their own limits.
- **Columns go out as the driver names them.** The GUI keeps `dedupeColumnNames`, which is display work. An empty page now carries its column names (a listed change). The sqlx engines (Postgres, MySQL/MariaDB, SQLite) read column names from rows, so for an empty `query_stream` they now take them from the statement sqlx prepared for the fetch (in its statement cache: no extra round trip); SQL Server and DuckDB always sent them. Their `query` (a utility statement) still returns none for an empty result.
- **A count that isn't a whole number is a failed count** (Task 4's count rule): estimated and flagged. Today `parseInt` makes `totalRows` and `totalPages` `NaN` (a listed change, `exec/count-not-numeric`).

### 6. Runs of several statements

- **Statements run one at a time, in order,** on the same connection id, as today.
- **A statement's failure is a `statementError`, and the run continues.** A planned failure is reported without touching the database, for example a value that can't be substituted in run-all mode.
- **Run-level failures end the stream with `error`:**
  - `CONFIRM_REQUIRED`;
  - `CONNECTION_NOT_FOUND`;
  - `INVALID_ARGUMENT` (the page size, a `db.page` that isn't a SELECT);
  - a substitution error in "current" mode, `INVALID_PARAMETERS`, matching today's toast;
  - `NOT_SUPPORTED` without an executor.
- **Nothing to run** is `done` with `statements: 0`, and the GUI shows today's `query_no_executable_statements` toast. This covers a comment-only buffer at the cursor, which today runs as a utility statement (a listed change).
- **Cancel ends the whole run.** The statement in flight is dropped. A stream or page stops on the server; a write or utility statement may still finish there, as in 5a's follow-up. The rest don't run. Afterwards nothing more is sent, as with `queryStream`, and the client's iterator ends with `CANCELLED`.
- **A new run on a tab cancels the tab's previous run,** all of it, not only its stream (a listed change).
- **Disconnect and eviction** cancel the run's token. It ends with `CONNECTION_CLOSED` as a stream does.

### 7. `CONFIRM_REQUIRED`

- **Core checks the statement texts before substitution,** the same texts the editor's prompt checks, so the prompt and the refusal agree. Values are bound, or inlined as quoted literals, so substitution can't add a destructive keyword.
- **The check runs before anything executes,** over every statement in the run, deferred ones included. Today the prompt also comes before pending changes queue anything.
- **The refusal is a terminal `error`:** `{code: "CONFIRM_REQUIRED", message, destructive: [{index, sql, reason}], destructiveTotal}`. `index` is the statement's position in the whole text, as the dialog shows today. `destructive` holds the first 100 (`MAX_DESTRUCTIVE_LISTED`, Task 7 probe N3) and `destructiveTotal` counts them all; the dialog adds "…and N more". The demo's runner does the same.
- **The GUI handles it from any caller.** The file-drop run and the reruns after a row delete or Set default get the prompt instead of running unconfirmed (finding 3; a listed change). The editor's own synchronous check stays first, so the usual path prompts without a round trip.
- **It isn't a security boundary.** `db.execute` runs any SQL on the same connection. The confirmation guards against mistakes, not against an API caller. CLAUDE.md says so.

### 8. Pending changes stay in TypeScript: `deferWrites`

- With `deferWrites: true`, every statement whose type isn't `select` comes back as `statementDeferred {index, sql, source, queryType}` and doesn't run.
- The view model queues it with `pendingChanges.add(connection.id, source.sql, queryType, "query-editor", tabId, source.params)`, exactly as today. Core owns the split, substitution and type, and TypeScript owns the queue until the next slice.

### 9. Source table and column sources: Core parses, the GUI looks up

- `statementStart` carries `table` (`seaquel_sql::statements::table_from_select`) and `columnRefs` (`seaquel_sql::ast::column_refs`), both computed on the substituted SQL of a `select`.
- The GUI resolves them to `SourceTableInfo` and `ColumnSourceInfo` against its schema cache, the primary-key lookup `$lib/sql`'s `resolveColumnSources` does today. It splits into `columnSourcesFromRefs(refs, schemas)`, and `sourceTableFromRef(ref, schemas)` is lifted from `resolveSourceTable`.
- Core has no schema cache, and inline edits aren't in 5b. The UI still parses no SQL itself.

### 10. Timing and the clock

- **`Executor` gains `monotonic() -> Duration`.**
  - `TokioExecutor` uses an `Instant` taken at first use, under the same sanctioned `allow` as its `SystemTime`.
  - `WasmExecutor` uses `performance.now()` through `js_sys::Reflect` on the global, so it works in a window and a worker.
- **Core takes it through `CoreBuilder::executor(Arc<dyn Executor>)`.** It has no default: a Core without one answers `db.run` and `db.page` with `NOT_SUPPORTED`, as a Core without a `ConnectPolicy` refuses to connect. `src-tauri`, `seaquel-server` and `seaquel-cli` pass `TokioExecutor`.
- **`elapsedMs`** covers the statement in Core. A page includes its count, as `executeStatement` measures today. A stream runs from the call to its final batch. Numbers get smaller, since IPC and HTTP are no longer in them.
- **The GUI keeps a live counter** while a statement streams and replaces it with `elapsedMs` at `statementDone`.
- **History timestamps** are `unix_time()` formatted like `Date.toISOString()` (`2026-10-02T12:34:56.789Z`), with the workspace `time` crate. Stored rows sort by that text.

### 11. History

- **Storage.**
  - `seaquel_storage::queries::query_history::append(st, item)` inserts the row and deletes the connection's non-favourite rows past the newest 500 (`HISTORY_KEEP`, ordered `timestamp DESC, rowid DESC`), in one transaction. That is `serializeQueryHistory`'s rule, applied to the file instead of to memory.
  - `set_favorite(st, id, favorite)` sets the flag. It sets rather than toggles, so writes queued in either order agree.
  - `replace_all` stays for its frozen repo fixture. Its `StorageRequest` variant and the TypeScript method go.
- **When Core records.**
  - Only `db.run` records, and only when the call has `history`, the run wasn't cancelled, at least one statement ran and no statement failed.
  - The last condition is new: today a failed paged statement in run all is still recorded (finding 4; a listed change).
  - A failed append is logged with its code and doesn't fail the run. That happens for a connection that isn't saved yet, or on the desktop's scratch stand-in.
- **What it records** (today's rules):
  - `query`: the whole text for run all, or the statement's text before substitution at the cursor;
  - `executionTime` and `rowCount` (`rowsAffected`, else `totalRows`): from the first statement that ran and wasn't a utility, else the first that ran. A row-returning `other` statement isn't a utility here (Decision 18);
  - `id`: `hist-<uuid>`;
  - `connectionId`, `connectionNameSnapshot` and `connectionLabelsSnapshot`: from the `history` context the GUI sends.
  
  The saved connection's id isn't Core's connection id, and the labels live in the GUI's state, so the GUI supplies them. Reading them from storage waits for connection CRUD in Core (5c).
- **The run's `done` carries `history`,** the row as appended, and `succeeded`. The GUI inserts the row at the top of its cache and trims it with the same rule. It calls `licenseNudgeStore.recordQuery()` when `succeeded` is true and history was asked for, as `addToHistory` does today.
- **The switch, in two steps, so no step has two writers of the whole list:**
  1. **Task 3.** The TypeScript stops replacing the list. `addToHistory` (used by the TS runner and pending changes) calls `append`, the favourite toggle calls `setFavorite`, and `scheduleConnectionData`, `persistConnectionData`, `serializeQueryHistory`, the history loop in `flush()` and the `history:` load-guard key are deleted. From here on, nothing in the app deletes history except the cap and `removeByConnection`.
  2. **Task 6.** Core runs send `history` and Core appends. The view model only inserts the returned row into the cache. Pending changes and the demo's TS runner keep calling `append` from TypeScript.
- **A failed load no longer blocks anything.** No save replaces the list, so there is nothing to guard. The cache stays empty and appends still go to the file.
- **Existing users lose nothing.** Rows aren't migrated or rewritten, and the first append's cap removes exactly what the next `replaceAll` would have. Pending debounced saves from an older build can't outlive the upgrade (they're in-memory timers).
  - **One accepted difference (Task 3 review):** rows with the same timestamp rank by `rowid DESC`, the one appended last first. `replace_all` inserted newest first, so in a file it wrote the lower rowid is the newer row. When two such rows straddle the 500th place, the prune keeps the older one where `serializeQueryHistory` kept the newer. Only millisecond-equal timestamps exactly at the cap are affected (`append_on_a_tie_at_the_cap_keeps_the_other_row`). Loads use the same order (`ORDER BY timestamp DESC, rowid DESC`, Rust and the demo), so the cache and the file agree.

### 12. Events on the wire

- `CoreEvent` gains `Run { stream_id, event: RunEvent }`, serialised as `{"type":"run","streamId":…,"event":{…}}`.
- `RunEvent`'s terminal events are `done` and `error`, like `StreamEvent`'s. The clients' "one terminal event" logic (`StreamQueue`, `core_stream`'s sent count, the web's `CANCELLED` for a stream that ends with neither) works unchanged.
- A run's `batch` is `StreamBatch` flattened, snake_case (`is_final`), exactly as in `StreamEvent`. It belongs to the latest `statementStart`.
- **Web:** `/rpc/stream` accepts `db.run` and `db.page` starts (the request's `streamId` must equal the frame's). `encode_split` splits a run batch over 4 MiB by rows, as it does a stream's. A run counts as one of the socket's 16 streams. Node's proxy doesn't read frames and doesn't change.
- **Desktop:** `run_core_stream` tracks a run's or page's `streamId` per webview like a query stream's, so a reload or a closed window cancels it.

### 13. The demo seam: `QueryRunner`

```ts
// src/lib/hooks/database/query-runner/types.ts
export interface QueryRunner {
  run(params: RunParams, signal: AbortSignal): AsyncIterable<RunEvent>;
  page(params: PageParams, signal: AbortSignal): AsyncIterable<RunEvent>;
}
```

- `getQueryRunner()` picks with the provider registry's rule: `isTauri() || isWeb()` gives `CoreQueryRunner` (`db.run`/`db.page` through `CoreClient.stream`), and otherwise `TsQueryRunner`.
- **`TsQueryRunner` is today's logic moved** out of `QueryExecutionManager`, with the Task 1 fix: wasm planning, `DuckDBProvider`, `TsEngineClient.paginate`, and `append` through the sql.js storage for history. It emits the same generated `RunEvent`s and refuses unconfirmed destructive runs the same way. Phase 8 deletes it.
- The view model consumes events and doesn't know which runner produced them. That is the least invasive seam. The alternative, keeping the old manager whole for the demo, would leave 1,300 lines of result handling in two copies.

### 14. MCP stays on its own path

- `run_saved_query` runs one statement read-only, with row, byte and time limits, on a read-only storage, and records nothing.
- Moving it onto `Workspace::run` would add a read-only mode, limits and a "no split" rule that the editor doesn't use, for a gain of one shared `substitute` call it already makes.
- MCP gets no code change in 5b. Its tests must pass (Core's `Connection::sql_engine` touches its connect path).
- The shared piece worth having is a read-only run mode for the AI, dashboards and MCP together (Follow-ups, with phase 6).

### 15. The AI's `run_query` and dashboards are untouched

- They keep `QueryCrud.executeReadOnly`, then `CoreProvider.selectReadOnly`, then `db.queryStream` with `readOnly`.
- **What 5b does touch:**
  - `CoreClient.stream` becomes generic over the request (`queryStream`, `run`, `page`);
  - `StreamQueue` over the event type;
  - `check_read_only` uses the connection's `sql_engine` (Decision 4).
- `select-read-only.test.ts`, `duckdb-read-only.test.ts`, the AI tool tests and Core's read-only tests must pass unchanged. The MariaDB engine change can only refuse more.

### 16. Hand-typed BEGIN/COMMIT: documented, not handled

- **Pooled Postgres and MySQL/MariaDB connections** don't keep a typed transaction on one connection (finding 9). A `BEGIN; UPDATE …; COMMIT;` run may execute its statements on different connections and leave one idle in transaction.
- 5b doesn't change this. Core runs statements through the same driver calls as today.
- Task 4's live tests record what happens, so the Follow-up starts from evidence: `pg_stat_activity` for `idle in transaction`, and whether the UPDATE is visible from a fresh connection.

### 17. For the pending-changes slice (owner's decision 4)

When pending changes move to Core, a batch of DML only (insert, update, delete, keyed edits) applies in one transaction through `Driver::transaction`. A batch with DDL keeps today's in-order apply, stopping at the first failure and leaving it and the rest pending. Not built in 5b.

### 18. Row-returning `other` statements show their rows (owner, 2026-10-02)

`query_type` knows only SELECT, INSERT, UPDATE and DELETE by their first word. So `WITH …`, `SHOW`, `EXPLAIN`, `PRAGMA`, `VALUES`, `TABLE` and DuckDB's FROM-first `FROM …` are `other`. Today they run as utility statements and their rows are discarded (`query-execution.svelte.ts:399-415`), and the result is hidden whenever another result shows.

- **Nothing about planning changes.** An `other` statement is still kind `utility` and runs through `query`, unpaged. `query_type`, the destructive check, `deferWrites` and paging are untouched: `db.page` still refuses it, since its type isn't `select`.
- **With at least one column, it is a rows result.** After `query` returns, Core sends one final `batch` with the columns and rows, and `statementDone` with `totalRows` set to the row count. The view model marks the result as a rows result, not a utility one. It is never hidden, it carries its columns, rows and `rowCount`, and history's "first statement that wasn't a utility" counts it (Decision 11).
- **With no columns, it stays a hidden utility result,** as today: no batch, `totalRows` 0.
- **DuckDB's status columns count as none** (Task 4): DuckDB answers `SET`, `CREATE`, `ATTACH`, `CHECKPOINT`… with a lone `Success` column (no rows) or `Count` column (no rows, or one integer row). Unless the statement starts like a query (`WITH`, `FROM`, `VALUES`, `TABLE`, `SELECT`, `SHOW`, `DESCRIBE`, `SUMMARIZE`, `PRAGMA`, `EXPLAIN`, `CALL`, `PIVOT`, `UNPIVOT`), that is a hidden utility result. duckdb-rs doesn't expose the statement's result type, so Core reads the answer's shape.
- **Over the row cap** (`query`'s 100,000, `max_query_rows()`), the statement fails with `RESULT_TOO_LARGE` as a `statementError`, as `query` does today, and the run continues (Decision 6).
- **The demo's `TsQueryRunner` gets the same fix** (Task 6), so its events still match the fixtures.
- **Why not a real kind now.** Paging or streaming these needs a row-returning kind that `query_type` doesn't have, which touches `seaquel-sql`'s frozen `query_type` fixtures and the destructive rules for `WITH … DELETE`. That is the high-priority Follow-up. This decision only stops throwing rows away.
- **Fixtures.** `pg/with-returns-rows`, `mysql/show-tables-returns-rows`, `duckdb/from-first-is-utility` and `exec/with-and-select` are listed changes. `pg/set-no-columns-hidden` pins that nothing changes without columns.

---

## The wire and the API

New Rust types live in `seaquel-workspace::run` (serde, `ts-rs` behind a new `ts` feature), re-exported as `seaquel_core::domain::run`. `seaquel-rpc` wraps them. `types:gen` gains `-p seaquel-workspace --features seaquel-workspace/ts`. `seaquel-rpc` names them through `seaquel_core::domain`, since interface glue may not name domain crates.

```rust
// crates/seaquel-workspace/src/run.rs

/// `db.run`. `Debug` is by hand: it shows the target, page size and flags,
/// never `text`, parameter values or the history context.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunParams {
    pub connection_id: String,            // Core's id
    pub stream_id: String,                // fresh per run; db.cancel takes it
    pub text: String,                     // the editor's whole text, well-formed
    pub target: RunTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<ParamValue>>,  // present only after the dialog
    pub page_size: u32,                   // 0 = stream every SELECT; ≤ max_query_rows() - 1 (a page fetches +1)
    #[serde(default)] pub confirmed: bool,
    #[serde(default)] pub defer_writes: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryContext>,
}

#[serde(tag = "type", rename_all = "camelCase")]
pub enum RunTarget { All, Current { cursor: u64 /* UTF-16 */ } }

/// `WireParameterValue`'s shape: the value in the cell wire format.
pub struct ParamValue { pub name: String, #[serde(default)] pub value: Value }

pub struct HistoryContext {
    pub connection_id: String,             // the saved connection's id
    pub connection_name: String,
    pub connection_labels: Box<RawValue>,  // ConnectionLabel[], stored as given
}

/// `db.page`.
pub struct PageParams {
    pub connection_id: String,
    pub stream_id: String,
    pub source: PageSource,
    pub page: u32,       // ≥ 1
    pub page_size: u32,  // 0 streams
}

/// What a statement ran: its SQL after substitution and its bind values.
pub struct PageSource { pub sql: String, pub params: Vec<Value> }

#[serde(rename_all = "camelCase")]
pub enum StatementKind { Page, Stream, Write, Utility }

pub struct DestructiveStatement { pub index: u32, pub sql: String, pub reason: DestructiveReason }

#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RunEvent {
    StatementStart {
        index: u32,                  // position in the run (0 for "current")
        sql: String,                 // the statement as typed, before substitution
        source: PageSource,
        query_type: QueryType,
        kind: StatementKind,
        page: u32,
        page_size: u32,
        table: Option<TableRef>,                     // select only
        column_refs: Option<Vec<Option<ColumnRef>>>, // select only
    },
    Batch(StreamBatch),              // flattened: {"type":"batch","columns",…,"is_final"}
    StatementDone {
        index: u32,
        elapsed_ms: f64,
        total_rows: u64,             // page: counted or estimated; stream: rows sent
        total_pages: u32,            // 1 unless paged
        count_estimated: bool,
        rows_affected: Option<u64>,  // write only
        last_insert_id: Option<i64>,
    },
    StatementError {
        index: u32, code: String, message: String, elapsed_ms: f64,
        sql: Option<String>,         // only on a planned failure, which has no statementStart
    },
    StatementDeferred { index: u32, sql: String, source: PageSource, query_type: QueryType },
    Done { statements: u32, succeeded: bool, history: Option<PersistedQueryHistoryItem> },
    Error { code: String, message: String, destructive: Option<Vec<DestructiveStatement>> },
}

pub const CONFIRM_REQUIRED: &str = "CONFIRM_REQUIRED";

/// Pure. `Err` only for run-level failures (INVALID_PARAMETERS at the cursor,
/// INVALID_ARGUMENT for the page size).
pub fn plan(
    text: &str,
    target: &RunTarget,
    params: Option<&[(String, Value)]>,
    engine: SqlEngine,
    page_size: u32,
    defer_writes: bool,
    max_page_size: u32,   // Core passes max_query_rows() - 1
) -> Result<RunPlan, PlanError>;

pub struct RunPlan {
    pub statements: Vec<Planned>,
    pub destructive: Vec<DestructiveStatement>,
    pub history_query: String,
}
pub enum Step {
    Run { source: PageSource, query_type: QueryType, kind: StatementKind,
          table: Option<TableRef>, column_refs: Option<Vec<Option<ColumnRef>>> },
    Defer { source: PageSource, query_type: QueryType },
    Fail { code: String, message: String },
}
pub struct Planned { pub index: u32, pub text_index: u32, pub sql: String, pub step: Step }
```

- u64/i64 fields get `ts(type = "number")`, as `ExecuteResult` does.
- `Option` fields are `skip_serializing_if = "Option::is_none"` with `ts(optional)`.

```rust
// crates/seaquel-core
impl CoreBuilder { pub fn executor(self, executor: Arc<dyn Executor>) -> Self; }
impl Workspace {
    /// Registered under (this workspace, stream_id) like a query stream.
    pub fn run<'a>(&'a self, core: &'a Core, params: RunParams) -> BoxStream<'a, RunEvent>;
    pub fn page<'a>(&'a self, core: &'a Core, params: PageParams) -> BoxStream<'a, RunEvent>;
}

// crates/seaquel-rpc/src/db.rs
pub enum DbRequest { /* … */ Run(RunParams), Page(PageParams) }   // stream only
pub enum CoreEvent { Stream { … }, ConnectionClosed { … }, Run { stream_id: String, event: RunEvent } }
pub fn dispatch_stream<'a>(core: &'a Core, ws: &'a Workspace, req: Request)
    -> Result<BoxStream<'a, CoreEvent>, RpcError>;   // ws now lives as long as the stream

// crates/seaquel-rpc/src/workspace.rs, storage group
QueryHistoryAppend = "queryHistoryAppend" [{ item: PersistedQueryHistoryItem }] -> [()],
QueryHistorySetFavorite = "queryHistorySetFavorite" [{ id: String, favorite: bool }] -> [()],
// QueryHistoryReplaceAll: removed
```

```ts
// src/lib/core/client.ts
export type StreamRequest =
  | { method: "db"; params: { method: "queryStream"; params: QueryStreamParams } }
  | { method: "db"; params: { method: "run"; params: RunParams } }
  | { method: "db"; params: { method: "page"; params: PageParams } };
export interface CoreClient {
  call(request: CoreRequest): Promise<CoreResponse>;
  stream(request: StreamRequest & { params: { method: "queryStream" } }, o?: StreamOptions): AsyncIterable<StreamEvent>;
  stream(request: StreamRequest & { params: { method: "run" | "page" } }, o?: StreamOptions): AsyncIterable<RunEvent>;
  events(handler: (event: ConnectionClosedEvent) => void): () => void;
}

// src/lib/types/query.ts
export interface StatementResult extends QueryResult {
  // …
  /** Task 1: the SQL that ran and its binds; paging re-runs this. */
  pageSource?: { sql: string; params: unknown[] };
  countEstimated?: boolean;
}

// src/lib/storage/client.ts
queryHistory: {
  loadByConnection(connectionId: string): Promise<PersistedQueryHistoryItem[]>;
  append(item: PersistedQueryHistoryItem): Promise<void>;
  setFavorite(id: string, favorite: boolean): Promise<void>;
  removeByConnection(connectionId: string): Promise<void>;
};
```

---

## Ground rules

These are 5a's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules;
- parallel-agent file ownership, with small re-read edits to shared files;
- tests never touch the real keychain, data dir or `~/.ssh`;
- no secrets in `Debug`, errors or logs;
- the full check list;
- effort log: `docs/plans/2026-10-02-phase-5b-effort.md`.

### Constraints the executors must obey

From CLAUDE.md and 5a:

- **Core builds for wasm32.** No `tokio::spawn`, `Instant` or `SystemTime` in Core crates (`crates/clippy.toml`). Time comes from the `Executor` (Decision 10). `seaquel-sql` and `seaquel-workspace::run::plan` must not panic on any input.
- **The UI never scans or parses SQL itself.** Splitting, statement at cursor, `{{param}}` handling, query type and the destructive check come from `$lib/sql` (wasm) or Core. On the run path use the `…OrThrow` variants.
- **The UI never does dialect work.** Pagination and count queries are Core's. Don't call `getAdapter(` outside `src/lib/engine/` and `src/lib/db/`. The TS runner's `paginate` goes through `EngineClient`, as today.
- **Error toasts use `errorToast`** from `$lib/utils/toast`, never `toast.error`.
- **Never edit `src/lib/components/ui/*`.**
- **New i18n keys** go in `messages/en.json` and are translated with the `i18n-translator` agent.
- **Run the Svelte MCP `svelte-autofixer`** on every changed `.svelte` file until it reports nothing.
- **No SQL, parameter values or secrets in logs.** Log activity names, counts, kinds and error codes only. `RunParams`/`PageParams` `Debug` omit text and values. `dispatch` logs method names only. Task 4 adds a `capture_logs` test.
- **Frozen fixtures.** The new run fixtures, once recorded (Task 2), change only when behaviour is meant to change, with the reason in the README. `crates/seaquel-storage/tests/fixtures` stays as it is.
- **Storage rules.** A new query is a new `seaquel-storage` function and `StorageRequest` variant, classified in `STORAGE_METHOD_KIND`. No SQL to the metadata database from the UI. No schema change is needed: `idx_history_conn_time` already exists.
- **npm through mise:** `mise exec -- npm run …` and `mise exec -- npx vitest …`. Use `SEAQUEL_WASM_PREBUILT=1` only when no wasm crate changed. Task 4 changes `seaquel-sql` and `seaquel-wasm`, so rebuild after it.
- **One shared `CARGO_TARGET_DIR`** for all agents: `/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`. Clean it between tasks if disk runs low.
- **Web limits from 5a stay as they are:**
  - 16 streams per socket, where a run is one;
  - 8 sockets per user;
  - 8 MiB frames, with batches split by rows past 4 MiB;
  - 16 connections per user, with pools of 6 (SQL Server: its session plus up to 4 read-only);
  - a workspace cap of 1,024;
  - the Origin gate.
  
  Nothing in 5b raises them.
- **`ConnectPolicy` and `executor` have no defaults.** Tests build Core with both.
- **The demo and the tutorial stay on DuckDB-WASM.** `TsQueryRunner` is demo-only.

## Order and estimates

Hours are agent wall time for first passes. 5a's first passes ran at 56–76% of their estimates, and fixes took 42% of the logged time. The probe's fixes are budgeted apart, at about three times the probe.

| # | Task | Estimate | Needs | Alongside |
|---|---|---|---|---|
| 1 | TS: paging with parameters (the bug) | 0.5–0.75 h | — | 2 |
| 2 | Run parity fixtures and the recorder | 1.5–2.5 h | 1 | 3 |
| 3 | History storage calls and the TS switch off `replaceAll` | 1.5–2.5 h | — | 2 |
| 4 | Core: `plan`, `Workspace::run`/`page`, executor, `sql_engine`, history append | 4.5–6 h | 2, 3 | — |
| 5 | `seaquel-rpc` `db.run`/`db.page`, `CoreEvent::Run`, both transports | 2–3 h | 4 | — |
| 6 | TS: `QueryRunner` seam, view model onto run events, confirm, history cache | 4–5.5 h | 5 | — |
| 7 | Probe (web, two users) | 0.75–1 h | 6 | — |
| 8 | Docs, measurement, checkpoint | 0.75–1 h | all | — |
| | Probe fixes (≈3× the probe) | 2.25–3 h | | |
| | Review fixes (≈40%) | 5–7 h | | |
| | **Total** | **~23–32 h** | | |

First passes add up to 15.5–22.25 h. At 5a's rates, expect roughly 10–15 h of first passes and **17–25 h logged**. The riskiest tasks:
- **4:** a new orchestration with cancel, history and a clock in Core.
- **6:** every run path in the GUI changes at once, and `QueryExecutionManager`'s streaming state is subtle (the `$state` proxy rules at `:59-71`).

Expect 6's review to find lifecycle issues: stale results after a tab switch, and a run finishing after its tab closed.

---

## Tasks

### Task 1: Paging with parameters (TypeScript)

**Files:**
- Modify: `src/lib/types/query.ts` (`StatementResult.pageSource`).
- Modify: `src/lib/hooks/database/query-execution.svelte.ts`:
  - set `pageSource: {sql, params: bindValues ?? []}` wherever a result is made (`createStreamingSeed`'s callers at `:631` and `:836`, the results at `:692` and `:887`);
  - in `executeStatementAtIndex` (`:986-1080`), run `existingResult.pageSource?.sql ?? existingResult.statementSql` with `existingResult.pageSource?.params`, for the type check, the stream and `executeStatement`.
- Test: `src/lib/hooks/database/query-execution.svelte.test.ts`.

**Tests first** (they fail before the fix):
- `pages a parameterised statement with its substituted SQL and binds`. Postgres, `SELECT * FROM t WHERE a = {{a}}`, `a = 5`, page size 2, `select` answering 3 rows then 1. `goToPage(2)` calls `select` with `SELECT * FROM t WHERE a = $1 LIMIT 3 OFFSET 2` and `[5]`.
- `changing the page size keeps the binds`: `setPageSize(tab, 500)` sends the binds, page 1.
- `re-streams a row-limited parameterised statement with its binds`: `… LIMIT 5` with a parameter. `setPageSize` calls `selectStream` with `$1` and `[5]`.
- `run all pages each statement with its own binds`: two statements with different parameters. Paging result 1 uses result 1's binds.
- `inlined parameters page with the inlined SQL`: SQL Server, `N'x'` inlined, empty binds.
- `statementSql stays the typed text`: `statementSql` still holds `{{a}}`, and so does the history call.

**Run:** `mise exec -- npx vitest run src/lib/hooks/database/query-execution.svelte.test.ts` passes. `mise exec -- npm run check` gives 0 errors and 0 warnings.

**Review:**
- `statementSql` is still what the tab shows and history records.
- The demo takes the same path.
- No other caller reads `statementSql` to run it: `rg "statementSql" src/lib`.

### Task 2: Run parity fixtures

Record today's runner, after Task 1, so Core is pinned against it the way 5a pinned connects.

**Files:**
- Create: `crates/seaquel-workspace/tests/fixtures/run/{plan-postgres,plan-mysql,plan-mariadb,plan-sqlite,plan-mssql,plan-duckdb,cursor,execute,history,pending}.json`, `README.md` and `changes.json`.
- Create: the recorder, `docs/plans/artifacts/2026-10-02-record-run-fixtures.test.ts.txt`. It is a vitest file. To run it, copy it to `src/lib/hooks/database/record-run.test.ts`, run it with `FREEZE_RUN=1`, then delete the copy.
- Modify: `.oxfmtrc.json` only if the new directory isn't already ignored through its parent pattern.

**Case format:**

```json
{
  "name": "pg/run-all-select-insert-error",
  "engine": "postgres",
  "input": { "text": "…", "target": {"type": "all"}, "params": null, "pageSize": 100, "deferWrites": false },
  "driver": [
    { "op": "page", "sql": "SELECT …", "paginate": {"limit": 101, "offset": 0}, "params": [], "answer": {"columns": ["a"], "rows": [[1]]} },
    { "op": "write", "sql": "INSERT …", "params": [], "answer": {"rowsAffected": 1} },
    { "op": "page", "sql": "SELECT nope", "paginate": {"limit": 101, "offset": 0}, "params": [], "answer": {"error": {"code": "QUERY_ERROR", "message": "…"}} }
  ],
  "results": [ { "index": 0, "sql": "…", "kind": "page", "queryType": "select", "columns": ["a"], "rowCount": 1, "totalRows": 1, "totalPages": 1, "table": null, "columnRefs": null }, "…" ],
  "deferred": [],
  "history": { "query": "…", "rowCount": 1 }
}
```

**How it records:**
- The recorder drives `QueryExecutionManager` with a scripted provider. The provider answers each call from `driver` and fails the case on an unexpected call.
- It records `paginate` as its arguments, not its output. The Rust replay expands them with the connection's `Dialect`, so neither side depends on the other's pagination text.
- `execute`/`executeCurrent` are called as the editor calls them. `addToHistory` and `pendingChanges.add` are spied.
- It writes the resulting `tab.results` without timings, plus the history and pending calls.
- `op`s: `page`, `count`, `stream`, `write` and `utility`, by today's provider method and SQL shape.

**Cases: at least 50, across all six engine ids (MariaDB included):**
- a paged SELECT with a partial page (no count), a full page (count), and a failed count (the estimate);
- a row-limited SELECT that streams, and page size 0;
- a write; a utility statement; a mix where utility results are hidden; all-utility runs where they aren't;
- run all with an error in the middle, continuing;
- parameters bound (Postgres `$1`, MySQL `?`), inlined (SQL Server `N'…'`, DuckDB), and in comments and strings;
- a substitution error at the cursor (nothing runs) and in run all (the statement's error);
- the cursor before, inside, between and after statements, with `東京` and `😀` before it;
- a comment-only buffer at the cursor and in run all;
- MariaDB `/*M! … ; … */`;
- pending changes on, at the cursor and in run all, with a mix of statement types;
- history: page 1 at the cursor, page 2 (none), run all (whole text), a failed stream (none), a failed paged statement in run all (recorded today);
- Task 1's paging with parameters.

**`changes.json`** lists, per case name, the Decision that makes Core differ, with the intended output. The Rust replay (Task 4) asserts that exactly those cases differ. Expected entries:
- the comment-only buffer at the cursor (Decision 6);
- an empty page carrying columns, and a count that isn't a number becoming an estimate (Decision 5);
- a run with any failed statement not recorded in history (Decision 11). Recording found 7 such cases, not just the failed page: today run all skips history only after a failed stream;
- row-returning `other` statements showing their rows (Decision 18, 4 cases);
- none others. A new one found in Task 4 is a finding to report, not something to absorb.

**Run:**
- Record twice. The two outputs are identical byte for byte.
- `git diff --stat` shows only the new directory and the artifact.
- `mise exec -- npx vitest run src/lib/hooks/database` still passes once the copy is deleted.

**Review:**
- Every `op` kind and every Decision 5/6 path has at least one case.
- The README names each file's coverage and the recorder's commit.

### Task 3: History storage calls, and TypeScript off `replaceAll`

**Files:**
- Rust:
  - `crates/seaquel-storage/src/queries/query_history.rs`: `append`, `set_favorite`, `HISTORY_KEEP = 500`.
  - New test file `crates/seaquel-storage/tests/query_history.rs`.
  - `crates/seaquel-rpc/src/workspace.rs`: add `QueryHistoryAppend` and `QueryHistorySetFavorite`, remove `QueryHistoryReplaceAll`, update the dispatch; tests in `crates/seaquel-rpc/tests/workspace.rs`.
  - `npm run types:gen`.
- TypeScript:
  - `src/lib/storage/client.ts`, `rust-client.ts` (methods, `STORAGE_METHOD_KIND`: both `write`), `repos/query-history-repo.ts` and `sqljs-client.ts` (the demo: the same insert and cap in sql.js);
  - `src/lib/hooks/database/query-history.svelte.ts`: `addToHistory` builds the item, inserts it into the cache trimmed by the rule, and calls `append`; `toggleQueryFavorite` calls `setFavorite(id, !favorite)`; new `insertRecorded(item)` for Task 6;
  - `persistence-manager.svelte.ts`: delete `scheduleConnectionData`, `persistConnectionData`, `serializeQueryHistory`, `MAX_HISTORY_ITEMS`, the history loop in `flush()` and the `history:` guard key, but keep `loadConnectionData`;
  - `database.svelte.ts`: `QueryHistoryManager` loses its scheduler.
  - Tests: `query-history.svelte.test.ts` (new), `failed-load.svelte.test.ts`, `persistence-manager.svelte.test.ts`, and a sql.js repo test.

**Tests first:**
- Rust:
  - `append_inserts_one_row`;
  - `append_keeps_the_newest_500_non_favourites`;
  - `append_keeps_favourites_past_the_cap`;
  - `append_prunes_only_its_own_connection`;
  - `append_for_an_unsaved_connection_fails`: the foreign key, `STORAGE_ERROR`;
  - `append_matches_the_old_serializer`: 700 rows with favourites scattered, compared with a Rust port of `serializeQueryHistory`'s rule;
  - `set_favorite_sets_and_clears`;
  - `set_favorite_on_an_unknown_id_changes_nothing`;
  - rpc: wire snapshots of both new methods (`method` before `params`), and `queryHistoryReplaceAll` is an unknown method.
- vitest:
  - `a run appends one row and never replaces the list`: the storage mock sees `append` only;
  - `the favourite toggle sends setFavorite for that id`;
  - `a failed history load doesn't stop appends`, which replaces the old "refuses history saves" case: no replace exists to refuse;
  - `the cache is trimmed like the file`;
  - `pending changes append through the same call`;
  - `flush writes no history`;
  - sql.js: `append` over the cap keeps favourites.

**Run:**
- `cargo test -p seaquel-storage -p seaquel-rpc` passes.
- `mise exec -- npm run types:gen` changes only the storage request/response types.
- `mise exec -- npm run check` gives 0/0. `CI=1 mise exec -- npx vitest run` passes.
- `rg "replaceAll|queryHistoryReplaceAll" src/lib -g '!*.test.ts'` shows nothing for history.

**Review:**
- Nothing in `src/` can write a whole history list any more.
- Timestamps are `toISOString()`, the format Core will write.
- Write order through the `RustStorageClient` queue is kept.
- The demo's history works.

### Task 4: The run service in Core

**Files:**
- `crates/seaquel-sql/src/offsets.rs`: moved from `seaquel-wasm`, with its tests. `crates/seaquel-wasm/src/offsets.rs` re-exports it, keeping `char_column_to_utf16` and `location_to_utf16` there if only wasm uses them.
- `crates/seaquel-workspace/Cargo.toml`: `seaquel-sql`, `ts-rs` optional, feature `ts`.
- `crates/seaquel-workspace/src/run.rs`: types, `plan` and `history_item`.
- New test file `crates/seaquel-workspace/tests/run_plan.rs`.
- `crates/seaquel-runtime/src/lib.rs`: `Executor::monotonic`, for both executors.
- `crates/seaquel-core/src/lib.rs`:
  - `CoreBuilder::executor`;
  - `Connection::sql_engine`;
  - the stream core of `query_stream_as` split so a run can drive several driver calls under one registered token;
  - `check_read_only`/`check_one_statement` on `sql_engine`.
- `crates/seaquel-core/src/workspace.rs`, or a new `run.rs` in Core: `Workspace::run`/`page` (feature `workspace`; the history append under `storage`); `connect` records `sql_engine`.
- New test files `crates/seaquel-core/tests/run.rs` (the fixture replay on a scripted mock driver, built from `tests/mock.rs`'s pieces) and `run_live.rs`.
- `src-tauri/src/lib.rs`, `crates/seaquel-server/src/lib.rs` (`web_core`) and `crates/seaquel-cli`: pass `TokioExecutor`.

**Tests first:**
- Plan (`run_plan.rs`):
  - `replays_the_planning_fields_of_every_fixture`: statements, sources, kinds, destructive list, deferred, and `changes.json` exactly;
  - `current_takes_a_utf16_cursor` (`東京`, `😀`, a cursor inside a surrogate pair rounds down);
  - `a_replaced_lone_surrogate_keeps_offsets`;
  - `destructive_lists_every_statement_before_substitution`;
  - `defer_writes_marks_every_non_select`;
  - `substitution_fails_the_run_at_the_cursor_and_the_statement_in_run_all`;
  - `mariadb_executable_comment_is_code`;
  - `page_size_zero_streams_every_select`;
  - `a_row_limited_select_streams`;
  - `page_size_past_the_cap_is_invalid`;
  - `nothing_to_run_is_empty`;
  - `plan_never_panics`: the scanner and parameter corpora from `seaquel-sql`'s fixtures, as inputs.
- Core (`run.rs`, mock driver):
  - `replays_the_execute_and_history_fixtures`;
  - `counts_only_when_the_page_is_full`;
  - `a_failed_count_is_estimated_and_flagged`;
  - `continues_after_a_statement_error`;
  - `confirm_required_runs_nothing`;
  - `confirmed_runs_the_destructive_statements`;
  - `cancel_ends_the_run_and_drops_the_statement_in_flight`: `HoldsConnection` mode, the driver released, later statements never called;
  - `a_cancel_before_the_start_runs_nothing`;
  - `disconnect_mid_run_ends_with_connection_closed`;
  - `close_all_ends_a_run`;
  - `another_workspace_cannot_run_page_or_cancel` (`CONNECTION_NOT_FOUND`);
  - `page_refuses_a_non_select`;
  - `page_streams_when_the_page_size_is_zero`;
  - `history_is_appended_once_when_the_run_succeeds`;
  - `history_is_skipped_on_error_cancel_page_and_without_context`;
  - `a_failed_append_does_not_fail_the_run` (an unsaved connection id);
  - `history_row_uses_the_first_non_utility_statement`;
  - `a_row_returning_other_statement_shows_its_rows` (Decision 18: one final batch, `totalRows`, counted by history);
  - `an_other_statement_with_no_columns_stays_a_utility`;
  - `an_other_statement_over_the_row_cap_is_result_too_large`, and the run continues;
  - `elapsed_comes_from_the_executor` (a fake executor that advances 5 ms per call);
  - `without_an_executor_run_is_not_supported`;
  - `mariadb_connection_scans_as_mariadb`, for the run and the read-only check;
  - `no_sql_or_values_in_logs` (testkit `capture_logs`, a run with a canary in the text, a parameter and a failing count).
- Live (`run_live.rs`; Postgres, MySQL, MariaDB, SQL Server, SQLite, DuckDB where they apply):
  - run all with a select, an insert, a utility statement and a failing statement;
  - a `WITH`, `SHOW`/`PRAGMA`/`EXPLAIN` (per engine) and DuckDB `FROM …` returning their rows (Decision 18);
  - a paged select over 250 rows with the count;
  - a stream;
  - bound and inlined parameters;
  - cancelling `SELECT pg_sleep(30); SELECT 1` (gone from `pg_stat_activity` within 2 s, and the second statement never runs) and `SELECT SLEEP(30)` on MySQL;
  - `BEGIN; UPDATE …; COMMIT;` on Postgres and MySQL. Record the outcome for Decision 16; don't assert a behaviour.

**Implement:**
- Decisions 3–11 on the Core side.
- `Workspace::run` borrows `&'a self` for the stream's life, since the append needs storage.
- The count reads the first cell of the first row as `Int`, `BigInt`, `Decimal` or `Text` into `u64`. Anything else, including text that isn't a whole number, counts as a failed count (`exec/count-not-numeric`).
- Decision 18: a `utility` statement's `query` result with at least one column goes out as one final `batch` and a `statementDone` with its row count.
- The history row is built by `seaquel_workspace::run::history_item(ctx, query, elapsed_ms, row_count, unix_time, id)`, so the TS runner's test can compare shapes.

**Run:**
- `cargo test -p seaquel-sql -p seaquel-wasm -p seaquel-workspace` passes.
- `cargo test -p seaquel-core --features seaquel-runtime/tokio` passes, with the live env (5a's checkpoint values) and `SEAQUEL_TEST_REQUIRE_ENGINES=1`.
- `cargo test -p seaquel-mcp -p seaquel-cli` passes.
- CI clippy and wasm32 clippy for the pure crates and the Core `browser` line pass.
- `npm run crates:check` passes.
- `mise exec -- npm run wasm:build` then `CI=1 mise exec -- npx vitest run src/lib/sql` passes, since `offsets` moved.

**Review:**
- No `Instant` or `SystemTime` outside the executors.
- `plan` has no I/O.
- The run holds no Core lock across an await.
- Each driver call is wrapped in the run's token.
- Nothing logs SQL.
- `Core`'s id-based methods don't gain run variants.
- MCP and the AI's read-only path are unchanged apart from `sql_engine`.

### Task 5: `seaquel-rpc` and both transports

**Files:**
- `crates/seaquel-rpc/src/db.rs`: `DbRequest::Run`/`Page`, `dispatch_stream` (the new `ws` lifetime), `CoreEvent::Run`, `method()`.
- `crates/seaquel-rpc/tests/db.rs`.
- `crates/seaquel-rpc/Cargo.toml`: nothing new, since the types come through `seaquel_core::domain` under `workspace`.
- `src-tauri/src/lib.rs`: `run_core_stream` takes the stream id from `run`/`page` too; its tests.
- `crates/seaquel-server/src/routes/rpc_stream.rs`: `start` accepts `run`/`page` with the matching `streamId`; `encode_split` handles `CoreEvent::Run { Batch }`; `run_stream`'s `finished` recognises the run's `done`/`error`.
- `crates/seaquel-server/tests/rpc_stream.rs` and `rpc_live.rs`.
- `package.json`: `types:gen` gains `seaquel-workspace`.
- Generated: `RunParams`, `PageParams`, `RunTarget`, `ParamValue`, `HistoryContext`, `PageSource`, `StatementKind`, `RunEvent`, `DestructiveStatement`, and `DbRequest`/`CoreEvent` updated.

**Tests first:**
- rpc:
  - `run_and_page_wire_shapes` (snapshots, `method` before `params`, absent optionals left out);
  - `run_is_stream_only`: `dispatch_workspace` gives `INVALID_ARGUMENT`;
  - `run_events_carry_the_stream_id`;
  - `run_params_debug_shows_no_text_or_values`;
  - `a_foreign_connection_is_not_found_through_dispatch`;
  - `without_the_workspace_feature_run_is_not_supported`.
- server:
  - `a_run_streams_statements_over_the_socket`;
  - `a_run_batch_over_4_mib_is_split_by_rows`;
  - `a_run_counts_as_one_of_16_streams`;
  - `the_start_frame_stream_id_must_match_a_runs`;
  - `a_cancel_frame_stops_a_run` (live: `pg_sleep` gone);
  - `closing_the_socket_cancels_a_run`;
  - `another_users_run_is_connection_not_found`;
  - `eviction_ends_a_run`.
- src-tauri:
  - `core_stream_serves_a_run_and_returns_its_event_count`;
  - `a_reload_cancels_a_running_run`.

**Run:**
- `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio` passes, live for `rpc_live`.
- `mise exec -- npm run cli:build && cargo test -p seaquel --lib` passes.
- `mise exec -- npm run types:gen` runs twice with no diff the second time.
- `mise exec -- npm run check` gives 0/0: the TS doesn't use the new types yet.

**Review:**
- Node's proxy is untouched.
- The `CANCELLED` synthesis for a stream that ends without a terminal event covers runs.
- The Tauri count includes run events.
- No SQL is in the server log after a live run (grep for the canary).

### Task 6: The GUI onto `db.run` and `db.page`

**Note from Task 4's review:** the demo's `duckdb.ts:488` `paginate` still appends ` LIMIT` after the SQL, so a trailing `--` comment swallows it there (the Rust dialects now put `LIMIT` on its own line). Leave it for phase 8, or fix it in `TsQueryRunner` only if that's trivial and doesn't change the frozen demo behaviour.

**Files:**
- `src/lib/core/client.ts`, `tauri.ts`, `http.ts` and their tests: `StreamRequest`, the overloads, `StreamQueue<T>`, demuxing `type: "run"`, and `toWellFormed` on run and page text.
- New: `src/lib/hooks/database/query-runner/{types.ts,core-runner.ts,ts-runner.ts,index.ts}` and tests. `ts-runner.ts` takes today's planning and execution from `query-execution.svelte.ts` with Task 1's fix and Decision 18's (an `other` statement's rows are kept when it returns columns).
- `src/lib/hooks/database/query-execution.svelte.ts`, rewritten as the view model:
  - per-tab `AbortController`;
  - seeds and results on `statementStart`, rows on `batch`, totals on `statementDone`, error results on `statementError`, `pendingChanges.add` on `statementDeferred`;
  - on `done`: utility filtering, the deferred toast and `history.insertRecorded`/`licenseNudgeStore.recordQuery()`;
  - on `error`: `CONFIRM_REQUIRED` goes to `pendingConfirm`, `INVALID_PARAMETERS` to `errorToast`, others to an error result;
  - `goToPage`/`setPageSize` send `db.page` with `result.pageSource`;
  - edit routing unchanged.
- `src/lib/hooks/database/query-history.svelte.ts`: `insertRecorded`.
- `src/lib/sql/index.ts`: `columnSourcesFromRefs` and `sourceTableFromRef`, split out of `resolveColumnSources`.
- `src/lib/components/query-editor/execution.svelte.ts`: sends `confirmed` after the prompt, and shows the dialog for `db.queries.pendingConfirm` on the active tab.
- `query-editor.svelte` only if the dialog wiring needs it.
- New i18n keys, if any, translated.

**Tests first (vitest):**
- `run all shows each statement as its events arrive`;
- `utility results are hidden unless every result is one`;
- `a row-returning other statement is shown with its rows and counts for history` (Decision 18, both runners);
- `a paged result keeps its page source and goToPage sends db.page with it`;
- `setPageSize to 0 re-streams through db.page`;
- `Stop cancels the run once and marks the streaming result stopped`;
- `a new run on the tab cancels the previous run`;
- `a run that finishes after its tab closed changes nothing`;
- `CONFIRM_REQUIRED opens the destructive dialog and Confirm resends with confirmed`;
- `a file-drop run of a destructive statement asks first`;
- `deferred statements go to pending changes with their binds`;
- `done inserts the history row and counts the nudge once`;
- `a page never records history`;
- `elapsed comes from statementDone`;
- `source table and column sources resolve from Core's refs and the schema cache`;
- `the demo runner's events match the Core fixtures`: `TsQueryRunner` over a scripted `DuckDBProvider` for the fixture cases the demo can run, equal up to timings;
- `a lone surrogate is replaced before sending and the cursor still picks the same statement`;
- `select-read-only`, `duckdb-read-only` and the AI tool tests pass unchanged.

**Run:**
- `mise exec -- npm run check` gives 0/0.
- `CI=1 mise exec -- npx vitest run` passes.
- `npx oxlint --type-aware --type-check --deny-warnings` passes (5a's lesson: tsgo sees what svelte-check doesn't).
- The autofixer is clean on each changed `.svelte` file.
- `mise exec -- npm run build`, `build:web` and `build:demo` pass.
- Live, desktop (`npm run tauri dev`) and web (`npm run dev:web:full`): the Manual checks' run, paging and stop items.

**Review:**
- `QueryExecutionManager` calls no `$lib/sql` planning function outside `ts-runner.ts`.
- No `provider.select`/`execute` on the run path for Core.
- `$state` proxy rules are kept: mutate through `getProxiedResult`.
- `pendingConfirm` is cleared on a tab switch and a close.
- The demo's run, paging and history work.

**Status (Task 6, done):**
- `select-read-only.test.ts` got a type-only change: its fake `CoreClient` now satisfies the generic `stream` (a cast on the stored request and on the returned queue). Its assertions are unchanged, as Decision 15 asks.
- A page request while a run is going on the tab is ignored (the pagination controls are disabled while it runs), so paging can't cancel the rest of a run.
- The grid shows a `QUERY_ERROR` as the database's message alone and other codes as `CODE: message`; a statement that never finished shows as cancelled.

### Task 7: Probe

A separate agent runs a two-user web instance (`SEAQUEL_WORKSPACE_CAP=2`, no origin variables) and uses only the browser-facing endpoints, as in 5a. It records evidence for each check:

- **Cross-user.** `db.run`, `db.page` and `db.cancel` against user B's connection and stream ids (guessed and seen): `CONNECTION_NOT_FOUND`, nothing run, B's run not cancelled.
- **History reach.** A `history.connectionId` naming a connection A doesn't have fails inside A's own file, never B's. A's history shows no B rows.
- **Confirmation.** `CONFIRM_REQUIRED` for a destructive run, and `confirmed: true` runs it. Record that `db.execute` is the same trust, as documented.
- **`db.page` with a write** is refused. So are a page size over the cap, page 0 and an overflowing offset.
- **Load.** A 5,000-statement run: server memory and time, the socket's other streams still served. A run of `SELECT * FROM generate_series(1, 5000000)` with page size 0 and a slow reader: server memory stays bounded (the outbox of 64). A run started on a closed socket.
- **Leaks.** No SQL, canary or parameter value in the server log, including from failed counts and history appends. No other user's data in any response.
- **Eviction** mid-run ends the run and stops the statement on the server.

Probe fixes are budgeted separately.

**Status (Task 7 probe fixes, done):**
- **I1.** `column_refs` parsed an 8 MiB page in ~4 GB. Measured per input byte in a release build: up to ~1.3 KB of heap and ~0.2 µs (a dense `SELECT 1,1,…`). Now `None` past 64 KiB (worst case under it ~86 MB, ~12 ms). `plan` on the probe's 932k-statement script took 1.34 s and 199 MB, `1;`×4.2M 839 MB, and one 8 MiB statement 0.5 s and 563 MB (the scanner keeps ~32 bytes per token and runs several times). Hence the 2 MiB text and 10,000-statement caps (Decision 5), on the web only (owner, 2026-10-02: `RunLimits`, `WEB_RUN_LIMITS`; the desktop, CLI, MCP and demo have none): the worst plan left on the web is one 2 MiB statement, ~146 ms and ~140 MB, and 9,700 dense 215-byte SELECTs, ~455 ms of parsing column refs across a run that then makes 9,700 round trips.
- **I2.** Node's `/api/rpc/stream` proxy (`shared/rpc-stream-proxy.js`, used by `server.js` and the Vite plugin alike) now pauses the side it reads when the other has 16 MiB unsent (`PAUSE_BUFFERED_BYTES`), resumes from the send callback once under 8 MiB, and closes both with 1013 past 64 MiB (`MAX_BUFFERED_BYTES`). Both directions.
- **M1, M2.** `/rpc` logs a failure as `code`, `group` and `method` only. The server's logger writes key-values (`startup::format_record`). Every `log!` with key-values in the crates carries ids, codes, counts, lengths, kinds or (DuckDB, desktop) schema and table names; none carries SQL or values. `/rpc/stream`'s workspace-open failure still logs its message: storage paths, no SQL.
- **N3.** Found while bounding frames: `{{p}}` is copied once per use on SQL Server and DuckDB and bound once per use on MySQL/MariaDB, so a large value used many times multiplied without limit (a 1 MiB value 300,000 times: ~300 GB). Now refused before substituting when the values would add more than 32 MiB (Decision 5); only the growth counts, so the text's own size never does (owner, 2026-10-02). The destructive list is capped (Decision 7). `statementStart` still carries `sql` and `source.sql`, often equal: dropping one changes the wire and the frozen run fixtures, and with the caps it costs at most one more copy of a 2 MiB text, so it stays. No size check in `encode_split` for other frames: every non-batch frame is bounded by the text cap, the substitution budget or a database's message.
- **N4.** A `{{name}}` with no value binds NULL. That is today's TS rule, pinned by seaquel-sql's frozen `query-params-test:6 values 0`, so it stays (`a_missing_value_binds_null`). The dialog sends every parameter it finds.
- **N2** (noted only): the confirmation isn't a security boundary (Decision 7).
- **Review fixes (C1, I1, I2, minors).** C1 as in Decision 5 (`planning_with_many_values_is_linear`: 10,000 statements × 10,000 values in 0.21 s debug, 0.05 s release; `the_web_caps_parameter_values`). I1: the server's `format_record` escapes control characters and cuts each key-value at 128 bytes (`MAX_LOG_VALUE_BYTES`), messages at 1 KiB, since stream and connection ids come from the browser and Core logs them at Info before its ownership check; `/rpc/stream` refuses a `streamId` over 128 characters or outside `[A-Za-z0-9_.:-]` with `INVALID_ARGUMENT` and never echoes it (the clients send UUIDs). I2: the proxy's hard cap counts only what arrives from a side after it was paused (read-ahead the pause didn't stop; with pausing off, the backlog before a frame), so a row wider than 64 MiB and the frames right behind it pass, while frames still arriving past 64 MiB close with 1013; the queue held while Rust opens is capped at 64 frames and 64 MiB, pauses the browser past 16 MiB and is flushed through the same backpressure. The editor's own destructive prompt shows the first 100 too (the dialog slices). `/rpc/stream` logs a workspace-open failure's code only. Re-probe (fresh release instance, `lc1.mjs`): `1;`×10,000 with 1,000 `1e1048575` decimals reaches its first statement in 6 ms (was ~400 s of CPU), `SELECT {{t0}};`×10,000 with 1,000 × 1 KB values in 53 ms, 1,001 values and 2 MiB of values are `INVALID_PARAMETERS` in 24–29 ms; a 70 MiB row is delivered (73.4 MB) and two streams started around it on the same socket both finish, socket open; `l5.mjs` with the reader paused keeps Node flat (298 → 286 MB) with Postgres stalled. No SQL or values in the logs. The live `tls_server_name` MSSQL tests fail inside the Bash sandbox (macOS trust settings, error -36) and pass outside it; the full suite was run outside.
- **Re-probe** (fresh release instance, the probe's scripts, `lfix.mjs`/`lfix2.mjs` added): every 8 MiB `l3.mjs` run, page, stream and deferred insert is `INVALID_ARGUMENT` in about 50 ms, Rust at 37–53 MB RSS; the 8 MiB `SELECT 1;` script, 10,001 statements and 645k DROPs are refused in 28–89 ms; `{{p}}`×300k with a 1 MiB value is `INVALID_PARAMETERS` in 70 ms. The largest allowed: a 2 MiB page returning one row, 240 ms and 234 MB peak RSS (sqlx preparing 2 MiB of SQL included); 7,000 SELECTs of 59 columns each, 4.3 s and 404 MB. 150 unconfirmed DROPs: a 10 KB refusal. `l5.mjs`/`l5b.mjs` with the reader paused: Node flat at 152/227 MB and Postgres's query stalled (`active`), then 219/664 MB delivered with Node at 202/283 MB. `/api/rpc` failing on `'canaryM1probe'::int` logs `code=QUERY_ERROR group=db method=query`; no SQL, value or canary in either server log.

### Task 8: Docs, measurement, checkpoint

- **CLAUDE.md:**
  - `db.run`/`db.page`, the run events, `CONFIRM_REQUIRED` and that it isn't a security boundary;
  - Core's `executor` and `sql_engine`;
  - history written only by `append`/`setFavorite`;
  - `QueryRunner` and the demo;
  - the Tauri and web notes that name `queryStream`.
- **Design doc:** the status line and a "Phase 5b cost" section.
- **This plan:** execution notes and release notes.
- **Effort log:** the totals.
- **The full check list,** as 5a's checkpoint, the oxlint type check included.

**Status (Task 8):** done. CLAUDE.md, the design doc's status line and "Phase 5b cost", the execution notes, release notes, checkpoint, manual checks and follow-ups below, and the effort log's totals are written. The full check list ran: everything passes except the three MSSQL `tls_server_name` live tests, which can't load the macOS platform certificates inside the Bash sandbox (error -36), the known environment issue; see "Checkpoint". The manual checks below are the owner's.

## Manual checks

For the owner, after Task 8.

**Setup.**
- Databases: `docker compose -f e2e/test-databases/docker-compose.yml up -d`, then `MSSQL_PASSWORD='Seaquel_Test_123!' npm run e2e:db:seed -- all`.
- Credentials: Postgres `postgres@127.0.0.1:5432/seaquel_test`, no password. MySQL `root@127.0.0.1:3306/seaquel_test` and MariaDB `root@127.0.0.1:3307/seaquel_test`, no password. SQL Server `sa` / `Seaquel_Test_123!` on `127.0.0.1:1433`, database `seaquel_test`, trusting the certificate. DuckDB (desktop only): a new file.
- To watch a statement on the server: `docker exec seaquel-postgres psql -U postgres -c "select pid, state, query from pg_stat_activity where query like '%pg_sleep%' and pid <> pg_backend_pid()"`, and on MySQL `docker exec seaquel-mysql mysql -uroot -e 'show processlist'`.
- Tables, once, on Postgres (Run all): `CREATE TABLE t2 (id int); CREATE TABLE t3 (id int PRIMARY KEY, name text DEFAULT 'x'); INSERT INTO t3 VALUES (1, 'a'), (2, 'b'), (3, 'c');`

**Desktop** (`npm run tauri dev`). Back up the data dir first (`~/Library/Application Support/app.seaquel.desktop.dev`).

- [ ] **Run all with parameters.** On Postgres: `SELECT g FROM generate_series(1, 250) g WHERE g > {{min}}; INSERT INTO t2 VALUES (1); SELECT nope;`, with `min = 10`. Three results: 240 rows over 3 pages, "1 row(s) affected", and an error. No new history entry (a statement failed).
- [ ] **Paging and page size.** On that first result, page 2 starts at 111. Page size 500 shows all 240 on one page; "Stream all" shows them too. Before 5b paging failed with a syntax error at `{{`.
- [ ] **History.** Delete `SELECT nope;` and Run all: one new entry holding the whole text, `{{min}}` as typed. Paging adds nothing. `SELECT nope` alone in a new tab: an error and no entry. Favourite an entry, restart the app: every entry is there and the favourite is kept.
- [ ] **History cap.** `sqlite3 ~/Library/Application\ Support/app.seaquel.desktop.dev/seaquel.db "select count(*) from query_history where connection_id = (select id from connections where name = '<your connection>')"`. If it is over 500 (older builds kept that many plus favourites), run one query and check again: 500 plus the favourites past them. Otherwise just check that it never passes that.
- [ ] **Stop.** `SELECT pg_sleep(30); SELECT 42;` Run all, then Stop within 5 s: gone from `pg_stat_activity` within 2 s, and no `42` result. On MySQL, `SELECT SLEEP(30); SELECT 42;`: gone from the process list.
- [ ] **A new run replaces the old.** Run `SELECT pg_sleep(30)`, then, without Stop, replace the text with `SELECT 1` and run it in the same tab: the sleep leaves `pg_stat_activity` and the tab shows `1`.
- [ ] **Destructive confirm.** `DELETE FROM t2` asks first; Cancel runs nothing (`SELECT count(*) FROM t2` unchanged); Confirm runs it.
- [ ] **Confirm on reruns.** Put `DELETE FROM t2; SELECT * FROM t3;` in a tab, confirm and run. Delete a `t3` row from the grid: the rerun asks again instead of running the `DELETE`. Same after Set default on a `name` cell.
- [ ] **Confirm on a file drop.** Save `DELETE FROM t2;` as `drop.sql` and drop it on the editor: the dialog comes before anything runs.
- [ ] **Many destructive statements.** `for i in $(seq 1 150); do echo "DELETE FROM t2;"; done | pbcopy`, paste and Run all: the dialog lists 100 and says "…and 50 more". Cancel.
- [ ] **Rows from other statements.** Each shows its rows: `WITH x AS (SELECT 1 AS a) SELECT * FROM x` and `EXPLAIN SELECT * FROM t3` on Postgres, `SHOW TABLES` on MySQL, `FROM range(3)` on DuckDB.
- [ ] **DuckDB settings stay hidden.** On DuckDB, `PRAGMA threads = 2; SELECT 1 AS a;` Run all: only `a` shows, no empty grid for the PRAGMA.
- [ ] **Empty results.** `SELECT * FROM t3 WHERE false` shows the headers `id` and `name` with no rows, on Postgres and on MySQL (`SELECT * FROM users WHERE 1 = 0` there).
- [ ] **Trailing comment.** `SELECT g FROM generate_series(1, 250) g -- all of them` at page size 100: 100 rows and 3 pages, not all 250 at once.
- [ ] **Pending changes.** Turn pending changes on, run `INSERT INTO t2 VALUES (2); SELECT 1;`: the insert is queued in the sheet and `SELECT 1` shows. Applying it inserts the row.
- [ ] **Inline editing on a JOIN.** `SELECT a.id, a.name, b.name AS other FROM t3 a JOIN t3 b ON b.id = a.id`: edit an `a.name` cell and save; `SELECT * FROM t3` shows the change.
- [ ] **AI and dashboards.** An AI chat's `run_query` and a dashboard widget still return rows.

**Web** (`npm run build:web:full`, then `SEAQUEL_WORKSPACE_CAP=2 npm run start:web`, at `http://localhost:8787`). Sign in as user A in one browser and user B in another.

- [ ] **The desktop list.** As A, on Postgres: run all with parameters, paging and page size, Stop, a new run replacing the old, the destructive confirm (editor, grid rerun, "…and N more"), history (entry after a success, none after a failure, favourite kept after a reload), `WITH`/`EXPLAIN`/`SHOW` rows, empty-result headers, the trailing comment, pending changes, inline editing on the JOIN. B's history shows none of A's queries.
- [ ] **Run size.** `python3 -c "print(('-- ' + 'x' * 100 + '\n') * 31000 + 'SELECT 1 AS a;')" | pbcopy` (3.2 MB: comments and one statement), paste and Run all: refused with a message naming the 2 MiB limit, and nothing runs. Paste the same text on the desktop: it runs and shows `a`.
- [ ] **Parameter values.** `python3 -c "print(' UNION ALL '.join(f'SELECT {{{{p{i}}}}} AS v' for i in range(1001)))" | pbcopy`, paste, Run all, leave the dialog's fields empty and press Cmd+Enter: refused with "A run can send at most 1,000 parameter values; this one sends 1,001." Change `range(1001)` to `range(1000)`: it runs and shows 1,000 rows. On the desktop the 1,001 run too.
- [ ] **Too many tabs.** Open the app in 9 tabs and run a query in each: the ninth shows the too-many-tabs message, as in 5a.

**Demo** (`npm run build:demo`, then `npm run preview:demo`):

- [ ] A query runs; `SELECT * FROM range(250) t(g) WHERE g > {{min}}` with `min = 10` pages (page 2 starts at 111) and page size 500 shows all 239; history records the run; a tutorial lesson runs.

**MCP.** With the desktop build, `claude mcp add` from Settings → MCP, then ask for a row count through `run_query` and run a saved query with a parameter through `run_saved_query` on an exposed Postgres connection: both return rows, as in 5a.

---

## Execution notes (2026-10-02)

The plan was executed task by task with subagents, Tasks 2 and 3 in parallel, with a review after each task, an approval round on Task 6 and on the probe fixes, and a second probe on a fresh instance after the fixes. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-10-02-phase-5b-effort.md`; the measured cost is in the design doc ("Phase 5b cost").

**What went differently from the plan**

- **Decision 18 was added mid-phase** (owner, Task 2 review). The recorder showed that `WITH`, `SHOW`, `EXPLAIN`, `PRAGMA` and DuckDB's `FROM …` ran as utility statements and their rows were thrown away. They now show their rows, unpaged under `query`'s cap; a real row-returning kind is the high-priority follow-up. `changes.json` gained 4 cases for it (14 in all).
- **The DuckDB status rule** (Task 4, narrowed in review). DuckDB answers every statement without rows with a `Success` or `Count` column, so "any column is a result" would have shown an empty grid for each `SET` or `CREATE`. Core treats a lone `Success` with no rows, or `Count` with no rows or one integer row, as no result, unless the statement starts with a query word. `PRAGMA` isn't one of those words (Decision 18 listed it), so `PRAGMA threads = 2` stays hidden while a listing PRAGMA, which has other columns, shows.
- **Empty results carry columns on every engine** (Task 4 review). The sqlx engines read column names from rows, so an empty page had none, which the plan didn't know. They now take them from the statement sqlx prepared for the fetch, with no extra round trip. `query` (a utility statement) still returns none for an empty result.
- **`paginate` puts `LIMIT` on its own line** (Task 4 review). A trailing `--` comment swallowed the `LIMIT` on Postgres, MySQL/MariaDB, SQLite and DuckDB, so such a page fetched every row. Each dialect's `paginate.json` changed only in that whitespace, with a note in its README. seaquel-sql's `count_query` fixtures pin the same swallowed paren, so Core ends a trailing line comment before it wraps the count instead of changing them. A page also stops reading at the row past the page whatever the SQL made of the limit, and `db.page` takes exactly one statement. The demo's `duckdb.ts` still appends ` LIMIT` on the same line (Follow-ups).
- **Run limits are web-only** (probe I1; owner). The probe fixes first capped every run at 2 MiB and 10,000 statements; the owner made them web-only. `RunLimits` is set per interface with `CoreBuilder::run_limits`, like `ConnectionLimits`; the web server's `WEB_RUN_LIMITS` also caps parameter values at 1,000 and 1 MiB (review C1). The desktop, the CLI, MCP and the demo have none.
- **The substitution budget and `SizeBound`** (probe N3, review C1). Filling in parameter values may add at most 32 MiB to a run, in Core on every interface, counting only the growth so a large dump with a few parameters runs. The first bound cost a value on every use and wrote out decimal exponents; `seaquel_sql::params::SizeBound` costs each used value once and a decimal's exponent by arithmetic, and `plan` builds `params::Values` once, so planning is linear. `plan`'s options became one `PlanOptions`.
- **Proxy backpressure and a connect timeout** (probe I2, review, approval). Node's `/api/rpc/stream` proxy buffered without limit. It now pauses the side it reads at 16 MiB unsent and closes with 1013 when more than 64 MiB arrives from a side after it was paused; the first version counted the backlog before a frame, which closed the socket on the frame right behind a wide row, and the re-probe caught it. The queue held while Rust's socket opens is capped, and a Rust socket that doesn't open in 10 s closes the browser with 1013.
- **Log escaping** (probe M1/M2, review I1, approval). The server's logger now writes key-values, and some are ids from the browser logged before Core's ownership check, so values are escaped, quoted logfmt-style when needed and cut at 128 bytes, messages at 1 KiB, and `/rpc/stream` refuses a `streamId` over 128 characters or outside `[A-Za-z0-9_.:-]`. `/rpc` logs a failure's code, group and method only. sqlparser logs literals at DEBUG, so the desktop and the CLI turn its target off (Task 4).
- **MariaDB's `sql_engine`** (Decision 4). Core's `Connection` records the SQL rules from the connect plan's database type, so a `mariadb` connection, whose driver is `mysql`, scans `/*M! … */` as code for runs and for the AI's read-only checks. `Core::connect` maps the driver id as before.
- **`seaquel-workspace` is a plain Core dependency** (Task 5). The wasm32 browser line builds `seaquel-rpc` without `workspace`, where `seaquel_core::domain` didn't exist, so `DbRequest::Run(RunParams)` couldn't compile. `domain` is now always re-exported (the crate is pure and builds for wasm32), Core's `workspace` feature gates only connecting and running, and a new `ts` feature (`seaquel-workspace/ts`) is turned on by `seaquel-rpc/ts`. `without_the_workspace_feature_run_is_not_supported` can't run as a test, since the crate's dev-dependency on itself turns `workspace` on; the browser clippy line compiles that branch.
- **Seven history cases, not one** (Task 2). Run all recorded history after a failed write, utility statement, substitution or page; only a failed stream skipped it. Decision 11 covers all of them.
- **A comment-only buffer at the cursor** runs nothing (`done` with `statements: 0`, the no-statements toast) instead of running the whole buffer as a utility statement.
- **Hand-typed transactions, recorded** (Decision 16, Task 4 live tests). On Postgres `BEGIN; UPDATE …; COMMIT;` ran on pooled connections, the UPDATE was visible and nothing was left idle in transaction. On MySQL `BEGIN` fails with 1295 (not in the prepared protocol) and the UPDATE autocommits.
- **The GUI's lifecycle** (Task 6 review and approval). A page request while a run is going is ignored; `pendingConfirm` carries its connection and is cleared on a tab switch or close; closing or reloading a project cancels its runs and settles their results; a statement that never finished shows as cancelled. Two i18n keys, plus one for "…and N more", translated.
- **The history tie at the cap** (Task 3 review). Rows with the same timestamp rank by `rowid DESC`, so at exactly the 500th place the prune can keep the older of two millisecond-equal rows where the old serializer kept the newer. Documented in Decision 11 and pinned.
- **The retired `replaceAll` in the frozen storage fixtures** (Task 3). The TS fixture replay called it in 6 cases; a `RETIRED` set skips that call and seeds the same rows by plain SQL, so the fixtures stay frozen.

**Task 7 findings.** The probe ran a two-user web instance (workspace cap 2, no origin variables) against the browser-facing endpoints only. Held: every cross-user `db.run`, `db.page` and `db.cancel` (guessed and seen ids) got `CONNECTION_NOT_FOUND` and ran nothing; history reached only the caller's own file; `CONFIRM_REQUIRED` held until `confirmed`; `db.page` refused writes, page 0, a page size over the cap and an overflowing offset; eviction ended a run and stopped its statement. Found (details in Task 7's status): I1 planning and `column_refs` cost on large scripts, I2 no backpressure in the proxy, M1/M2 log content and format, N3 parameter values multiplied per use. N2 (the confirmation isn't a boundary) and N4 (a missing value binds NULL, as before) were noted and kept. The re-probe on a fresh release instance confirmed every fix, with no SQL, canary or value in either server log.

**Decisions made during execution**

- **Run limits only on the web** (owner). The desktop runs a script of any size, as before.
- **Only substitution growth counts** (owner), so the text's own size never trips the 32 MiB budget.
- **A missing parameter value binds NULL,** as before (a frozen seaquel-sql case pins it).
- **`statementStart` keeps both `sql` and `source.sql`,** though often equal: dropping one would change the wire and the frozen fixtures, and the caps bound the cost.
- **The demo's runner has no substitution budget and no web limits;** it runs in the user's own tab until phase 8.

**Release notes**

For the release after 5a's. Earlier phases' notes still apply as written.

Changes you may notice:

- **Paging a query with `{{parameters}}` works.** Changing the page or the page size used to send the query with the raw `{{name}}` and fail.
- **Stopping a paged query stops it on the server,** as streamed queries already did. Running a new query in a tab stops everything still running there, not only the current statement.
- **`WITH`, `SHOW`, `EXPLAIN`, `PRAGMA`, `VALUES` and DuckDB `FROM …` show their rows.** They used to run and show nothing. They aren't paged yet, and a result over 100,000 rows fails.
- **Empty results show their column names.**
- **A query ending in a `--` comment pages correctly.** It used to fetch every row.
- **The destructive-statement prompt appears everywhere a query runs:** after deleting a row or setting a default in the grid, and when a dropped file runs, not only from the Run buttons. A run with many destructive statements lists the first 100 and says how many more there are.
- **History records a run only when every statement succeeded,** and never records paging. Favourites are kept as before, and existing history is untouched.
- **Timings are measured in the app's Rust core,** so they no longer include the round trip to the window or browser, and are usually a little lower.
- **Running the cursor's statement in an editor holding only comments** does nothing and says so, instead of sending the comments to the database.
- **MariaDB `/*M! … */` comments are treated as code** everywhere, including the AI's read-only check, which can only refuse more.

Self-hosted web:

- **New limits on a run.** A run's text may be at most 2 MiB and hold at most 10,000 statements (`INVALID_ARGUMENT`), and it may send at most 1,000 parameter values totalling 1 MiB (`INVALID_PARAMETERS`). On every platform, filling in parameters may add at most 32 MiB to a run (`INVALID_PARAMETERS`). The desktop app has no size or count limit.
- **Slow browsers no longer grow the server's memory.** The WebSocket proxy pauses a query's results while the browser catches up. A connection that keeps sending more than 64 MiB after being paused is closed with code 1013, and the browser reconnects; so is a socket whose connection to the Rust service doesn't open within 10 s.
- **Stream ids** must be at most 128 characters of letters, digits, `_`, `.`, `:` and `-`. The app sends UUIDs; only a custom client could be affected.
- **Log format.** The Rust service's log lines now end with `key=value` fields (`code`, `group`, `method`, ids, counts), logfmt style, with values quoted and escaped where needed. A failed `/rpc` call is logged as its code, group and method only. Adjust any log parser that expects the old lines.

---

## Checkpoint

The full check list, run on 2026-10-02 one step at a time on the shared `scratchpad/p5a/target`, inside the Bash sandbox, with all five containers healthy and the live env (`SEAQUEL_TEST_POSTGRES`, `_MYSQL`, `_MARIADB`, `_MSSQL`, `SEAQUEL_TEST_SSH`, `SEAQUEL_TEST_REQUIRE_ENGINES=1`; values as in `ci.yml`):

| Check | Result |
|---|---|
| `npm run crates:check` | pass |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, live | 1,360 passed, 3 failed, 3 ignored (154 test targets, 431 s). The 3 are `seaquel-engine-mssql`'s `tls_server_name` tests, the known sandbox issue (below) |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` | pass |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run cli:build`, `cargo check -p seaquel` | pass |
| `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 36 passed |
| `npm run types:gen`, generated types unchanged | pass (the 143 files are identical before and after) |
| `npm run check` | pass: 0 errors, 0 warnings |
| `npx oxlint --type-aware --type-check --deny-warnings` (CI's lint step) | pass |
| `CI=1 npx vitest run` | pass: 1,582 tests in 79 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |

**The MSSQL `tls_server_name` tests** (`a_bracketed_ipv6_host_is_dialled`, `without_it_the_tls_name_is_host`, `the_tls_name_is_tls_server_name_while_the_socket_goes_to_host`) panic in tiberius with `could not load platform certs: … code: -36`: the sandbox blocks the macOS trust settings that rustls-native-certs reads. They passed outside the sandbox in the Task 7 review round (1,362 then; one `rpc_logs` test was added since). Not a 5b change, and CI (Linux) isn't affected.

Fixed during Task 8, docs only: the `CoreBuilder::run_limits` doc comment (it named two limits; there are four) and `seaquel-rpc`'s `workspace` feature comment (it gates `db.run`/`db.page` too). fmt and CI clippy were rerun after them and pass.

CI doesn't run oxfmt. On the files 5b touched, `oxfmt --check` flags only the design doc, the 5a plan and this plan, which were already unformatted (tables and lists in the plans' own style); formatting this plan would fold the "Settled" list into one paragraph, so it's left as it is. CLAUDE.md passes.

**Manual checks:** pending (the owner).

**Not run:** the release workflow and a signed build.

---

## Follow-ups (not in 5b)

- **High priority: a row-returning statement kind** (after Decision 18). `WITH …`, `SHOW`, `EXPLAIN`, `PRAGMA`, `VALUES`, `TABLE` and DuckDB `FROM …` should page and stream like a SELECT instead of running unpaged under `query`'s 100,000-row cap. It needs `query_type` (or a new check) to tell them apart, which changes `seaquel-sql`'s frozen `query_type` fixtures, and care with `WITH … DELETE`/`UPDATE`, which write.
- **The next slice: inline edits, CRUD, pending changes, the data tab and workflows.** The data tab's filter, sort and count query building moves with them. Pending changes follow the owner's decision (Decision 17): a batch of DML only applies in one transaction through `Driver::transaction`; a batch with DDL keeps today's in-order apply, stopping at the first failure. Then 5c (connection, project and saved-query CRUD) and 5d, as planned.
- **Hand-typed transactions** (Decision 16). Pin one pooled connection per run, or a session per tab, on Postgres and MySQL/MariaDB. Task 4's record: Postgres ran the statements on pooled connections with nothing left idle in transaction.
- **MySQL/MariaDB `BEGIN` fails with 1295** (not in the prepared protocol), so a typed transaction's UPDATE autocommits. Send transaction control as text, or refuse it with a clear message.
- **BEGIN … END bodies** split on their inner `;` on SQLite (triggers), MySQL and SQL Server (procedures, triggers, blocks). It's seaquel-sql's frozen `edge:begin-end` behaviour, pinned in `run/plan-sqlite.json`. The pieces fail on the server.
- **Show an estimated total as approximate.** A paged result whose count failed carries `countEstimated`, but the pagination bar shows the estimate as if it were exact.
- **The demo's `duckdb.ts` `paginate`** still appends ` LIMIT` after the SQL, so a trailing `--` comment swallows it in the demo. Fix it with phase 8, or earlier if it stays trivial.
- **Cancelling a write or utility statement on the server.** A cancel, disconnect or eviction drops a streamed or paged statement on the server, but a write or utility statement in flight (and a count) may still finish there (5a's follow-up, still open).
- **A shared read-only run** for the AI's `run_query`, dashboards and MCP's `run_saved_query` (with phase 6), with the parameter definitions and `coerceValue` in Core, shared with the dialog.
- **`SELECT … INTO` through `db.page`.** `db.page` refuses anything but one SELECT, but a SELECT that writes (`SELECT … INTO`, a writing function, `FOR UPDATE`) gets through, as through `db.query` and `deferWrites`. Decide whether paging and pending changes should use the read-only check.
- **The confirm is a heuristic.** `destructive_reason` asks about DROP, TRUNCATE, DROP COLUMN, DELETE or UPDATE with no WHERE, and MERGE … DELETE. `DELETE … WHERE true`, a procedure call or a writing function isn't asked about. Decide with the pending-changes slice whether to widen it; it stays a guard against mistakes, not a boundary.
- **Reruns without parameter values.** The row-delete and Set default reruns and the file-drop run send no `params`, so a tab with `{{name}}` fails on them. Keep the tab's last values and resend them.
- **Explain and Visualize** still resolve the statement at the cursor in TS through wasm. Move them onto the same planning when they move.
- **"Stream all" has no row cap,** and the GUI holds every row. Consider a cap or a spill.
- **History from a second writer** (the CLI in phase 7) needs `StorageChanged`.
- **History's row count and time for run all** come from the first displayed statement, as before. Revisit if they confuse.
- **Phase 8** deletes `TsQueryRunner` when the demo runs Core.
