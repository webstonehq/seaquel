# Phase 5c Implementation Plan: edits, pending changes, the data tab and workflows in Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** implemented; manual checks pending (the owner). The owner answered the open questions on 2026-10-03, taking the recommendation on each ("Answered questions"). Where the code departs from the text below, the repo is authoritative; see "Execution notes", "Checkpoint" and "Follow-ups" at the end.

**Goal:** On desktop and web, every write the grid makes goes through Core, and so does the data tab's query. A cell edit, Set default, row insert or row delete is an edit intent: a table, a key, a column and a value. Core reads the table's metadata, checks the key against the primary key, builds the SQL with the connection's dialect and runs it, or hands it back to be queued. Applying pending changes is one Core call. A batch of DML only applies in one transaction; a batch with DDL applies in order and stops at the first failure (the owner's Decision 17 in 5b). The data tab sends its filters, sort and page, and Core builds, pages and counts the query. Workflow query nodes run read-only on their own connection. The TypeScript keeps the queue, the sheet, the grid and the dialogs, and no longer builds or classifies SQL.

**Architecture:**
- **`seaquel-workspace::edits`** is pure. It turns edit intents plus table metadata into statements (with the engine's `Dialect` builders), decides how a batch applies, summarises a change for the sheet, and builds the data tab's SELECT and count from a typed `TableQuery`. Parity fixtures recorded from today's TypeScript pin it.
- **Core.** `Workspace::plan_edits` and `Workspace::apply_changes` run on one of the workspace's connections. Apply uses `Driver::transaction` for a DML-only batch and `execute` otherwise, then appends history. `Workspace::table_page` is a stream: it builds the query and runs it through 5b's page executor, so cancel, the count probe and the row cap work as for `db.page`.
- **`seaquel-rpc`.** `db.planEdits` and `db.applyChanges` are unary; `db.tablePage` is a stream served by `dispatch_stream` and emits `RunEvent`s; `db.duckdbExtension` replaces the extensions tab's hand-built SQL. The engine RPC loses `paginate` and the four CRUD builders, which nothing on desktop or web calls any more.
- **TypeScript.** An `EditService` seam, like 5b's `QueryRunner`: `CoreEditService` on desktop and web, `TsEditService` (today's builders and apply loop, moved) for the demo until phase 8. `QueryCrudManager`, `PendingChangesManager` and `DataTabManager` become view models over it.

**Tech Stack:** Rust (`seaquel-sql`, `seaquel-workspace`, `seaquel-engine` and the engine crates, `seaquel-core`, `seaquel-storage`, `seaquel-rpc`, `seaquel-server`, `src-tauri`), TypeScript/Svelte 5, vitest, the e2e Docker databases.

**Inputs:**
- The design doc: "Core, workspaces and state", phase 5 in "Migration plan", "Phase 5b cost" and its "What this means for the next slice".
- The phase 5b plan, especially Decisions 7, 8, 13, 15 and 17, its execution notes and follow-ups, and its effort log (the estimating basis).
- A read-only survey of the edit, pending-changes, data-tab and workflow code, checked line by line on 2026-10-03 (below).

**Naming.** The design doc's 5b follow-ups called the connection, project and saved-query CRUD "5c". This slice takes that name; the CRUD slice becomes 5d. Task 8 updates the design doc.

---

## What the code shows

All line numbers are as of `cc08674` plus the working tree.

1. **Grid edits are `QueryCrudManager`** (`src/lib/hooks/database/query-crud.svelte.ts`, 557 lines).
   - `updateCellDirect` (`:175-255`), `setCellDefaultDirect` (`:261-343`), `insertRow` (`:348-397`) and `deleteRow` (`:402-455`) each ask `EngineClient` for the SQL (`buildUpdate`, `buildSetDefault`, `buildInsert`, `buildDelete`), then either queue it (`pendingChanges.add`, with a `target` for the grid's overlays) or run it with `provider.execute`.
   - **Casts.** `castMapForColumns` (`:39-47`) maps columns to `CAST(… AS type)` types: a column's `castType`, else its `type` unless text-like (`UNCAST_TYPES`), with `bit`/`character` widened (`UNBOUNDED_CAST`). `buildCastMap` (`:107-147`) returns one for Postgres only, from the schema cache when it has the table's columns, else from a `tableMetadata` load cached per provider connection id (`loadedColumns`, `:78`). The types are interpolated into the SQL (`crates/seaquel-engine/src/dialect.rs:10-16`: "trusted input"), and on the Rust client they cross the wire from the browser (`rust-engine-client.ts:262-338`).
   - **Set default on SQLite** reads the column's default expression from a fresh `tableMetadata` (`setDefaultExpression`, `:156-169`), also interpolated (`build_param_set_expr`, `crud.rs:187-218`).
   - **The stale-key rule.** A keyed edit that runs now and affects 0 rows is an error naming the table and key (`matchedRow`, `:54-64`; message from `stale-edit.ts:47-56`, i18n). Engines count differently (`stale-edit.ts:5-20`): MySQL counts matched rows, SQL Server adds trigger rows, a Postgres rule or SQLite INSTEAD OF trigger reports 0 for a write it made. Nothing checks for more than one row.
   - **Dedupe.** The data tab's edits replace a queued edit of the same cell (`deduplicatePending`, `findForCell`, `pending-changes.svelte.ts:69-86`); the query tab's don't (`query-execution.svelte.ts:915-920` passes no option), so editing a cell twice there queues two UPDATEs.
   - **`executeReadOnly`** (`:491-529`) is the AI's and dashboards' path and must not change (5b Decision 15).
   - **`executeRaw`** (`:461-470`) runs any SQL through `provider.select` (`db.query`, 100,000-row cap). **`executeRawDdl`** (`:535-556`) runs DDL or queues it, guessing the origin with `toUpperCase().startsWith` (`:542-548`) and always typing it `other`.
2. **Edit routing** stays in the view model (`query-execution.svelte.ts`).
   - `updateCell` (`:902-944`) and `setCellDefault` (`:950-973`) go through `resolveEditTarget` (`:1061-1129`): Core's column refs, resolved against the schema cache by `columnSourcesFromRefs`/`sourceTableFromRef` (`src/lib/sql/index.ts:513-545`, 5b Decision 9), route a display column to its table and column and pick the key out of the row; a missing key column is an error.
   - A query tab's row delete doesn't use them: it sends `activeResult.sourceTable` and the whole row keyed by display names (`components/query-editor/cell-editing.svelte.ts:68-71`), then reruns the tab (`:78`). Set default reruns it too (`query-execution.svelte.ts:969-971`).
   - `getRowFromTab` (`:1033`) has no caller.
3. **Pending changes are UI state** (`pending-changes.svelte.ts`, 210 lines).
   - `pendingChangesByConnection` is plain `$state` keyed by the saved connection id (`state.svelte.ts:196`). Nothing persists it; a reload loses the queue. The feature is on by default (`stores/pending-changes-settings.svelte.ts:6`), so by default every grid edit is queued.
   - Queued from: grid edits (above), the editor's deferred statements (`statementDeferred`, `query-execution.svelte.ts:751-762`, binds decoded from 5b's wire format), `executeRawDdl` (sidebar drop and truncate, `components/sidebar/manage/schema-tab.svelte:41-100`) and the table editor (`create-table-tabs.svelte.ts:216-229`, one change per statement).
   - `executeAll` (`:131-197`) runs each change with `provider.execute` in order. A keyed edit affecting 0 rows fails (`:154-157`, `expectsRow` in `stale-edit.ts:22-32`). The first failure stops the loop and leaves it and the rest queued (`:179-183`). Every applied change appends a history row with its placeholder SQL, `rowCount: 1` and `executionTime: 0` (`:165-174`). `hasDdl` means "a change typed `other` ran" (`:160-162`); the sheet then reloads the schema and refreshes the connection's data tabs (`components/pending-changes-sheet.svelte:140-145`).
   - The sheet always asks before applying (`showConfirmDialog`, `:117-121`) but lists nothing destructive.
   - **Descriptions** are regexes over the SQL (`pending-change-description.ts:7-120`), and the sheet's SQL view rewrites `$N` with the bind values by regex (`pending-changes-sheet.svelte:84-94`), which is wrong for `?` and `@Pn` and inside strings. Both break the rule that the UI never scans SQL.
4. **Core already has an atomic batch.** `Driver::transaction` (`crates/seaquel-engine/src/lib.rs:383-399`) runs `BatchStatement`s on one connection and rolls back on the first failure; `expect_rows: {min}` (`crates/seaquel-types/src/lib.rs:322-366`) rolls back a keyed edit that matched nothing with `NO_ROWS_AFFECTED`.
   - Implemented for the sqlx engines (`sqlx_driver.rs:305-340`, SQLite through it, `seaquel-engine-sqlite/src/driver.rs:104-109`), DuckDB (`seaquel-engine-duckdb/src/driver.rs:766-775`) and SQL Server on its held session, refusing to start while a hand-opened transaction is open (`seaquel-engine-mssql/src/driver.rs:860-881`).
   - It is served as `db.transaction` (`crates/seaquel-rpc/src/db.rs:89-92`) and nothing in the GUI calls it.
   - It returns `()`. On a database error it doesn't say which statement failed; only `NO_ROWS_AFFECTED` carries the index, in its message (`seaquel-types/src/lib.rs:133-142`).
   - Live tests cover SQL Server, DuckDB and SQLite (`seaquel-engine-mssql/tests/live.rs:113`, `seaquel-engine-duckdb/tests/live.rs`, `seaquel-core/tests/core.rs:108`). **Postgres and MySQL/MariaDB have none**, and 5b found MySQL refuses a typed `BEGIN` through the prepared protocol (1295). sqlx's own `begin()` sends it as text, but no test shows it.
5. **The data tab builds its SQL in TypeScript** (`data-tabs.svelte.ts`, 457 lines).
   - `buildQuery` (`:336-379`) and `buildCountQuery` (`:384-406`): `SELECT * FROM <qualifiedTable>`, each enabled filter as `CAST(col AS <text type>) <op> <placeholder>` (`$N`, `?` or `@pN`, and `TEXT`, `CHAR` or `NVARCHAR(MAX)`, from `filterDialect`, `:12-16`), joined by AND or OR; `ORDER BY` from the sort; `LIMIT … OFFSET …`, or on SQL Server `OFFSET … FETCH` with `ORDER BY (SELECT NULL)` (`:369-376`). None of it goes through `Dialect::paginate`.
   - On SQL Server a table with `sql_variant`, `geography`, `geometry` or `hierarchyid` columns lists its columns and casts those to `NVARCHAR(MAX)` (`:417-431`), because tiberius 0.12 panics on their metadata (`seaquel-engine-mssql/src/session.rs:121-131`). This needs the column types.
   - `refresh` (`:114-213`) runs the count, then the page, both through `provider.select` (`db.query`: no cancel, no streaming). A failed count is silently 0 (`:149-151`), so the tab shows one page. Columns of an empty page come from the schema cache (`:160-161`); primary keys too (`:452-456`).
   - Filters live in the tab's memory; only the table ids persist (`persistence-manager.svelte.ts:286-294`). The operators are a closed set in the UI (`types/data-tab.ts:11-23`, `components/data-filter-bar.svelte:22-35`).
6. **Workflows run anything, on the active connection.** `WorkflowManager` takes `executeQuery` (`workflow-manager.svelte.ts:29`), wired to `queries.executeRaw` (`hooks/database.svelte.ts:236-239`). A query node records its `connectionId` (`:96-108`) and `executeQueryNode` (`:370-455`) ignores it. No read-only mode, no row cap below `db.query`'s 100,000, no cancel, no history. Result rows are node data (`types/workflow.ts:44-53`), and a saved workflow stores its nodes' data (`:76-83`, `:95-104`), rows included.
7. **The DuckDB extensions tab** builds `INSTALL`, `LOAD`, `UPDATE EXTENSIONS` and `FROM community` SQL from a name it validates as `^[A-Za-z0-9_]+$` (`extensions-duckdb-tabs.svelte.ts:83-135`) and runs it through `executeRaw` (`hooks/database.svelte.ts:188-191`). Two actions send two statements in one `db.query` (`:115`, `:127`). Both run: the DuckDB driver's `query` goes through duckdb-rs `Connection::prepare` (`seaquel-engine-duckdb/src/driver.rs:385`), which in the locked duckdb-rs 1.10505.0 splits with `duckdb_extract_statements`, executes every statement but the last, and prepares the last (`duckdb-1.10505.0/src/inner_connection.rs:141-150`). Bind values would go to the last statement only; these actions send none. The tab opens for any DuckDB connection, the demo's included (`components/sidebar/manage/connections.svelte:235-242`).
8. **The edit calls the engine RPC serves** (`crates/seaquel-rpc/src/lib.rs:62-148`, `:170-265`): `buildUpdate`, `buildSetDefault`, `buildInsert` and `buildDelete` take the casts and SQLite default expression from the caller. `paginate` is also still there, though `RustEngineClient` pages locally with a TS port of each dialect (`rust-engine-client.ts:83-177`, `:248-252`), and since 5b nothing on desktop or web calls that port except `TsQueryRunner`'s `paginate` in the demo (`query-runner/index.ts:34`), which goes through `TsEngineClient`.
9. **The demo** (`TsEngineClient`, `src/lib/engine/ts-engine-client.ts:160-200`) builds inline CRUD with `crud-helpers.ts` (values escaped as literals, `:14-33`) through `duckdb.ts`, and `DuckDBProvider.execute` ignores bind values (`providers/duckdb-provider.ts:309-319`). `DuckDBProvider.selectReadOnly` exists (`:290-307`), so read-only workflows work in the demo without a new seam.
10. **`insertRow`'s `lastInsertId`** reaches the insert dialog's toast (`components/insert-row-dialog.svelte:64-72`), so a single immediate insert must keep returning it.

### Bugs and gaps the survey found

- **Edits go to the active connection, not the result's.** Every CRUD call reads `state.activeConnection` (`query-crud.svelte.ts:186`, `:271`, `:357`, `:410`). Data tabs keep their own `connectionId` and don't switch the active connection when clicked (`components/header-tabs.svelte:146-149`; ERD, statistics, workflow and extensions tabs do, `:122-124`, `:154`). So with a data tab of connection A open and B active, editing, deleting or inserting in that tab runs against B with A's table name; the tab's refresh still reads A (`data-tabs.svelte.ts:118`). A query tab's results don't record their connection at all (`query-execution.svelte.ts:322` runs on the active one), so editing an old result after switching connections has the same problem.
- **`IN` and `NOT IN` filters always fail.** They build `CAST(col AS TEXT) IN $1` (`data-tabs.svelte.ts:350-357`), a syntax error on every engine.
- **Range filters compare text.** `>`, `<`, `>=` and `<=` compare `CAST(col AS TEXT)` with the typed value, so `id > 9` misses 10 to 89.
- **Data tab refreshes race.** `refresh` has no sequence or cancel; two quick page clicks can land in the wrong order. A failed count shows one page (`:149-151`), and `if (!this.providers) return;` after setting `isLoading` (`:137`) would leave the spinner on.
- **Key values in the log.** `matchedRow` logs the message with the key (`query-crud.svelte.ts:62`), and `executeAll` logs the failure's message (`pending-changes.svelte.ts:177`), which holds it for a stale key and can hold values in a database error.
- **The query tab's row delete** ignores column sources (finding 2), so a result with an aliased key column binds NULL and fails as stale.
- **Workflow nodes ignore their connection** and can write (finding 6).
- **Two extension actions** put two statements in one `db.query` (finding 7). Checked in the code: duckdb-rs runs both, so it works today; it relies on that duckdb-rs behaviour, which `db.duckdbExtension` (Q9) replaces with one call per statement.

## Answered questions

The owner settled these on 2026-10-03, taking the recommendation on every one. Q1, Q4, Q5 and Q6 were asked directly; Q2, Q3, Q7, Q8 and Q9 were taken as recommended. Each keeps the options that were weighed and the reason, so later changes start from them.

### Q1. Who owns the pending-changes queue?

- **A. TypeScript keeps the queue; Core is stateless.** The queue holds what to apply (edit intents, or SQL the user typed); Core plans and applies what the client sends back, as `db.page` does in 5b.
- **B. Core keeps it in workspace memory.** It survives a web page reload but not an eviction or a desktop restart, needs events to keep the sheet in sync, a per-user memory cap and an answer for eviction losing it silently.
- **C. Core persists it** in `seaquel.db` (a new migration). It survives restarts, but writes row values, possibly sensitive, to the metadata file, and needs `StorageChanged` once the CLI writes too.

**Answer (owner, asked): A.** The queue is UI state today and isn't persisted, so nobody loses anything. Stateless Core means nothing leaks when tabs close, nothing to evict and no new trust: `db.execute` already runs any SQL on the same connection. What changes is what the queue holds: intents instead of SQL the UI got back from the engine RPC, so Core builds and classifies at apply time. Persisting the queue can come later as a feature of its own.

### Q2. The atomic DML rule in detail

The owner decided (5b Decision 17): a batch of DML only applies in one transaction; a batch with DDL keeps today's in-order apply.

- **What counts as DML.** Recommended: an edit intent (update cell, set default, insert row, delete row), or a typed statement whose `seaquel_sql::statements::query_type` is `insert`, `update` or `delete`. Everything else is not DML: `other` (DDL, `TRUNCATE`, `MERGE`, `WITH … UPDATE`, `CALL`/`EXEC`, `COPY`, `SET`, `USE`), and a `select`, which the editor never defers but a client could send. `query_type` reads the first word, so this errs toward in-order, never toward a transaction that can't hold.
  - Alternatives: a per-engine list of transactional statements (Postgres DDL, SQLite DDL, DuckDB DDL, SQL Server DDL are transactional; MySQL's aren't). More atomic batches, but four rule sets to test, and it departs from the owner's rule. Leave it as a follow-up.
- **One statement per change.** Core splits each typed change as the run splits and refuses more than one (`INVALID_ARGUMENT`), so `UPDATE …; COMMIT` can't ride inside a DML batch.
- **A batch of one** runs through `execute`, as today. That keeps `lastInsertId` for the insert dialog, and a single statement has nothing to roll back.
- **MySQL/MariaDB.** DDL commits implicitly, which is why DDL batches stay in order. A DML-only batch is a real transaction for InnoDB; a MyISAM table ignores the rollback, and MySQL only warns. Documented, not detected. Task 3 adds the live transaction tests MySQL, MariaDB and Postgres lack (finding 4).
- **SQL Server's single session.** The transaction runs on the held session and refuses to start while a hand-opened transaction is open (`EXECUTE_ERROR`), which Core reports as a failed apply with nothing applied. In-order applies run on the same session through `execute`. A failure mid-transaction closes the session, and the next call reconnects (CLAUDE.md), which loses `#temp` tables and session state: documented.
- **Which change failed.** `Driver::transaction` doesn't say (finding 4). Recommended: it returns the failed statement's index with the error, in all four implementations (Task 3).

**Answer (taken as recommended):** as above. It is the owner's rule made precise, conservative where the statement's type is unclear.

### Q3. Do grid edits queue or apply at once?

- **A. Keep today's rule:** the pending-changes setting decides, on by default.
- **B. Always queue.**
- **C. Always apply at once**, pending changes only for the editor.

**Answer (taken as recommended): A**, unchanged, with one fix: the query tab dedupes a repeated edit of the same cell the way the data tab does (a listed change). The setting is a user preference; this slice changes where edits run, not when.

### Q4. The stale-key rule

- **A. Today's: at least one row.** A keyed update, set default or delete that affects 0 rows fails with `NO_ROWS_AFFECTED`. Immediately, nothing changed; in-order, the apply stops there; in a transaction, everything rolls back (`expect_rows: {min: 1}`).
- **B. Exactly one row**, rolling back past one. Catches a key that isn't unique. But SQL Server counts trigger rows, so any table with a trigger would fail every edit, and immediate edits would need a transaction each.
- **C. Optimistic concurrency:** the WHERE also compares the edited cell's old value. Detects someone else's change, but needs null-safe equality per engine and exact comparison of floats, JSON and bytes.

**Answer (owner, asked): A, plus a key check in Core.** Core loads the table's metadata anyway (Decision 3), so it refuses an edit whose key columns aren't exactly the table's primary key (`NOT_EDITABLE`). That closes the many-rows case at its source (a stale schema cache, a view) without trigger false positives. C is a follow-up. The known false positive stays documented: a Postgres rule or SQLite INSTEAD OF trigger reports 0 for a write it made.

### Q5. Does the data tab's filter DSL move to Core, and in what shape?

- **A. It stays in TypeScript** and only the SQL runs in Core. It keeps hand-rolled placeholders and pagination in the UI, which CLAUDE.md forbids.
- **B. Core builds it from a typed `TableQuery`**: table, filters (`column`, an operator from a closed enum serialised as today's text, `"="` to `"IS NOT NULL"`, and a text value), `AND`/`OR`, sort (`column`, `ASC`/`DESC`), page and page size. Core quotes, binds, casts, paginates with `Dialect::paginate` and counts with 5b's count rule. An unknown operator is refused by serde; unknown columns are the database's error.

**Answer (owner, asked): B**, as a stream (`db.tablePage`) reusing 5b's page executor, so a new request or a closed tab cancels the old one on the server, and the race goes away. Keep today's text comparison, pinned by the fixtures, and fix `IN`/`NOT IN` first (Task 1): a comma-separated list, each item trimmed and bound, `IN ($1, $2, …)`; an empty list is refused. Typed range comparison is a follow-up: it needs the column's type bound per engine, and Postgres refuses `integer > text` without a cast.

### Q6. Workflows: read-only, limits, a timeout?

- **A. As today:** any SQL through `db.query`, on the active connection.
- **B. Through `db.run`:** splitting, confirmation, pending changes and history, for a canvas that shows one result per node.
- **C. Read-only, like dashboards:** `executeReadOnly` on the node's own connection, one statement, with a row cap.

**Answer (owner, asked): C**, on the node's saved connection, with `maxRows` 10,000 (a truncated result says so on the node) and cancel when a node is re-run or deleted. Workflows are for exploring and charting, like dashboards, which went read-only in the AI safety phase. Result rows are saved with the workflow (finding 6), which argues for a cap well under 100,000. Writing from a node is refused with `READ_ONLY`: any workflow that writes today stops working, which the release notes must say (owner, 2026-10-03). No timeout for now: Core's `timeout` doesn't cross the transports, and a node can be stopped. Nothing new is needed for the demo (finding 9).

### Q7. History for edits and applies

- **A. Today's rule, written by Core:** each applied pending change appends a row (its SQL as queued), now with Core's time and rows affected instead of 0 and 1; immediate grid edits record nothing.
- **B. Every write records**, immediate edits included.
- **C. One row per apply**, holding the batch.

**Answer (taken as recommended): A.** It keeps what users see today, and history stays a record of queries rather than of cell clicks. For an atomic batch the rows are appended after the commit, all or none; in order, one per change that ran. A new `query_history::append_many` writes them in one storage transaction and prunes once. The license nudge keeps counting runs only.

### Q8. `CONFIRM_REQUIRED` for applies with destructive statements

- **A. No check on apply:** the editor's statements were confirmed when `db.run` deferred them (5b Decision 7 checks deferred statements too), and the sheet always asks.
- **B. The same rule as `db.run`:** `db.applyChanges` refuses an unconfirmed batch holding a destructive statement, listing them.

**Answer (taken as recommended): B.** One rule everywhere was 5b's settled item 6. The sheet's existing dialog gains the list and sends `confirmed: true`, so users see no extra step. Keyed deletes aren't destructive by `destructive_reason` (they have a WHERE); the sidebar's DROP and TRUNCATE already have their own dialogs and send `confirmed`. Don't widen the heuristic here (5b follow-up); it stays a guard against mistakes, not a boundary.

### Q9. The DuckDB extensions tab

- **A. Leave it on `executeRaw`,** splitting the two-statement actions.
- **B. A small typed call, `db.duckdbExtension`,** with the action and the name. Core checks the connection is DuckDB, validates the name, builds each statement and runs them one at a time. The demo keeps its TypeScript path.

**Answer (taken as recommended): B.** It is small, takes the last hand-built dialect SQL out of that tab, and lets `QueryExecutionManager` drop `executeRaw` altogether.

---

## Decisions (2026-10-03)

Settled with the answers above.

### 1. Edit intents; Core builds

- The grid sends intents: `updateCell`, `setDefault`, `insertRow`, `deleteRow`, plus `truncateTable` and `dropObject` for the sidebar (Decision 11). Each names a table (`schema`, `table`) and, for keyed ones, the key as `[column, value]` pairs picked out of the row by today's routing (`resolveEditTarget`, column sources). Values are in the cell wire format.
- **Routing stays in the GUI** (5b Decision 9): it maps display columns to table columns with the schema cache and Core's column refs. The query tab's row delete joins it (finding 2).
- **Core builds with the connection's `Dialect`**: `build_update`, `build_set_default_expr`, `build_insert`, `build_delete`, unchanged. Their frozen `crud.json` fixtures keep pinning the SQL.

### 2. The queue holds intents; Core is stateless (Q1)

- A `PendingChange` gains `change: Change`, what `db.applyChanges` takes back: `{type: "edit", edit}` or `{type: "sql", sql, params}` for a statement the editor deferred or the table editor generated. The display fields stay (`sql`, `bindValues`, `queryType`, `description`, `target`, `origin`), filled from `db.planEdits` for edits and from `statementDeferred` for typed SQL.
- Core rebuilds an edit's SQL at apply time from fresh metadata, so what runs matches the table as it is then. The queue's `sql` is for display.
- The client's `queryType` is never trusted: Core classifies each change itself (Decision 5).

### 3. Core reads the table's metadata, per call

- For each table an edit call touches, Core calls `Driver::table_metadata` once per call (deduped within a batch). From it: the Postgres cast map (`castMapForColumns`'s rules, ported to `seaquel_workspace::edits::cast_map` and pinned by fixtures), SQLite's default expression for Set default, and the primary key for the key check (Decision 4).
- **No casts or default expressions cross the wire any more.** They are interpolated SQL, and on the web they came from the browser.
- **No cache in Core.** The TypeScript cached per provider connection id (`loadedColumns`) and dropped it on schema reloads. A Core cache would need to know about DDL run through `db.run` and `db.execute`. One catalog query per edit call is measured in the probe; a cache is a follow-up if it's slow.
- A table Core can't find is `NOT_EDITABLE`, as is one without a primary key.
- **A failed metadata read fails the call** with the read's code (`QUERY_ERROR`, …), before anything runs. Today a Postgres edit whose load fails is built without casts (`buildCastMap` returns `undefined`); that fallback goes. Listed in the Task 2 fixtures' `changes.json` (`pg/insert-metadata-fails`).

### 4. The stale-key rule and the key check (Q4)

- Keyed edits must affect at least one row: `NO_ROWS_AFFECTED` (immediately, or `expect_rows: {min: 1}` in a transaction, or checked after `execute` in order).
- The key's columns must be exactly the table's primary-key columns from the metadata, else `NOT_EDITABLE`, before anything runs.
- The message the user sees stays the i18n `edit_no_row_matched` with table and key, formatted in TypeScript from the change's `target` and the failure's code and index. Core's messages and logs carry no key values.

### 5. Applying a batch (Q2)

`db.applyChanges` takes the connection, the changes in queue order, `confirmed` and a history context, and returns an outcome.

| Mode | When | How | On failure |
|---|---|---|---|
| `single` | one change | `execute` | nothing to undo; `NO_ROWS_AFFECTED` for a keyed edit that matched nothing |
| `atomic` | two or more, all DML | `Driver::transaction`, keyed edits with `expect_rows: {min: 1}` | rolled back; nothing applied; the failed change's index when the driver knows it |
| `inOrder` | two or more, any not DML | `execute` one by one | stop; the applied ones are done, the failed one and the rest stay queued |

- **Classification** as in Q2: edit intents other than `truncateTable`/`dropObject` and typed `insert`/`update`/`delete` are DML. A SQLite truncate is a `DELETE FROM` intent and counts as DML; elsewhere `TRUNCATE` doesn't.
- **Each typed change is one statement** (split with the connection's `sql_engine`), else `INVALID_ARGUMENT` before anything runs.
- **Validation first.** Metadata, key checks, splitting and the destructive check (Decision 7) all happen before the first statement; a refusal applies nothing.
- **`ddl: true`** when any applied change isn't DML, so the sheet reloads the schema as `hasDdl` did.
- **The GUI** clears the queue after a full success, keeps it whole after an atomic failure, and removes the applied prefix after an in-order failure (today's rule), showing the failure on its change.
- **Cancel.** A unary call. Dropping it (a closed window; on web, the request aborted) rolls an open transaction back. A progress stream with a Stop button is a follow-up.

### 6. Immediate or queued: unchanged (Q3)

The setting decides. With it off, a grid edit is `db.applyChanges` with one change (`single`); with it on, `db.planEdits` fills the queue entry. The query tab dedupes like the data tab (a listed change).

### 7. Confirmation on apply (Q8)

- `db.applyChanges` refuses an unconfirmed batch holding a statement `destructive_reason` flags, with `{outcome: "confirmRequired", destructive, destructiveTotal}` (the first 100, as 5b's `MAX_DESTRUCTIVE_LISTED`). Edits are checked as the SQL Core built.
- The sheet's apply dialog lists them and sends `confirmed: true`. The sidebar's DROP/TRUNCATE dialogs send it too.

### 8. History (Q7)

- With a history context, Core appends one row per applied change, as queued (`sql` as the text, rows affected as `rowCount`, Core's time), through `query_history::append_many` (one storage transaction, one prune). Atomic: after the commit, all of them. In order: the ones that ran. Single immediate edits (not from the queue) record nothing; the call sends no context.
- The outcome returns the rows; the GUI inserts them into its cache with `insertRecorded`, as 5b's runs do. A failed append is logged with its code and doesn't fail the apply.
- `PendingChangesManager` stops calling `addToHistory`; the demo's `TsEditService` keeps appending from TypeScript.

### 9. The data tab: `db.tablePage` (Q5)

- A stream, one statement, emitting `RunEvent`s: `statementStart` (with `source`, the built SQL and binds, `queryType: select`, `kind: page`), batches, `statementDone` (totals, `countEstimated`), `done`. Registered under its `streamId` like `db.page`, so `db.cancel`, disconnect and eviction reach it.
- Core builds `SELECT <cols> FROM <quote_schema(schema)>.<quote_ident(table)> [WHERE …] [ORDER BY …]` with a new pure builder in `seaquel-engine` (`select.rs`), parameterized like `crud.rs` by quote functions, the placeholder and the text type (`TEXT`, `CHAR` on MySQL/MariaDB, `NVARCHAR(MAX)` on SQL Server). On SQL Server it reads the column types and casts the four types tiberius can't read (the metadata call happens only there).
- **Placeholders are the engine's `crud.rs` ones**, since `select.rs` is parameterized like `crud.rs`: `@P1…` on SQL Server where the data tab writes `@p1`, and `?` on DuckDB where it writes `$1` (`$1` on Postgres and SQLite, `?` on MySQL/MariaDB, as today). Listed in the Task 2 fixtures' `changes.json`.
- It then runs 5b's page kind: `Dialect::paginate(sql, pageSize + 1, offset)`, and the count only when the page is full, estimated and flagged when it fails. The count is `count_query(select)` over the built SELECT (5b's, `SELECT COUNT(*) as total FROM (…) AS count_query`, which strips SQL Server's trailing ORDER BY), not today's `SELECT COUNT(*) FROM <table> WHERE …`; on SQL Server with `sql_variant` columns it wraps the listed, cast select. Listed changes: no count for a partial page (the total is then exact anyway); a failed count shows an estimate instead of one page; `IN` works (Task 1 fixes it in TS first, so the fixtures record it working).
- Page size is 1 to `max_query_rows() - 1`; the tab has no "Stream all".
- The GUI keeps one operation per data tab: a new refresh cancels the previous one (the race), and closing the tab cancels it. Columns of an empty page come from Core (5b gives every engine's empty page its columns); primary keys still come from the schema cache for `sourceTable`.

### 10. Workflows (Q6)

- `executeQueryNode` calls `queries.executeReadOnly(node.connectionId, sql, signal, name, WORKFLOW_MAX_ROWS)` with `WORKFLOW_MAX_ROWS = 10_000`. `executeReadOnly` itself doesn't change (5b Decision 15).
- One `AbortController` per node: re-running or deleting the node cancels the previous run. The result node shows `truncated`.
- A write is refused with `READ_ONLY`; a node whose connection is gone or disconnected shows `executeReadOnly`'s own message.

### 11. DDL from the sidebar and the table editor

- The sidebar's DROP and TRUNCATE become intents (`dropObject {kind: table | view | materializedView}`, `truncateTable`), built by the dialect (SQLite truncates with `DELETE FROM`, as `schema-tab.svelte:86-91` does). The UI stops building that SQL.
- The table editor's statements (from `db.engine` `createTable`/`alterTable`) go through `db.applyChanges` as typed changes when applied at once: two or more DDL statements apply in order and stop at the first failure, as the loop at `create-table-tabs.svelte.ts:231-247` does.
- `executeRawDdl` and its origin guessing go; callers pass their origin.

### 12. Descriptions and the sheet's SQL view

- `seaquel_sql::statements::change_summary(sql, engine) -> Option<ChangeSummary {verb, table, column?}>` reads the statement with the scanner (quoted names, comments and all). It is exposed through `seaquel-wasm` as `$lib/sql`'s `changeSummary`. The English text and the origin fallback stay in TypeScript (`describePendingChange` keeps its signature and loses its regexes). Parity fixtures record today's descriptions; differences only where the regexes misread quoting (listed).
- The sheet's SQL view shows the SQL and its values beside it, not a `$N` rewrite (a listed change). An inline preview per dialect is a follow-up.

### 13. Edits target the result's connection

- A query tab's `StatementResult` records the connection it came from (`connectionId`, the saved id). Edits on it use that connection's current Core id, looked up at the time of the edit, and refuse with the existing "No connection established" when it's disconnected. Data tabs use their `connectionId`, and so do their pending overlays and inserts. This is the first fix of the slice (Task 1, in TypeScript, before the fixtures are recorded); the new calls then carry the result's Core id and never read the active connection.

### 14. The engine RPC loses what nothing calls

- `EngineRequest::{Paginate, BuildUpdate, BuildSetDefault, BuildInsert, BuildDelete}` go, with `RustEngineClient`'s `paginate`, `mssqlPaginate` port and `build*` methods. The `Dialect` methods stay; Core calls them.
- `EngineClient` loses `paginate` and `build*`; `TsEditService` and `TsQueryRunner` call the demo's `duckdb.ts` adapter directly, as `TsEngineClient` does now.

### 15. `Driver::transaction` reports the failed statement

- It returns `Result<(), TransactionError>`, `TransactionError { index: Option<usize>, error: DbError }`, in the default and the four implementations. `db.transaction` keeps its wire (the error's code and message). `NO_ROWS_AFFECTED`'s message is unchanged.

### 16. The demo seam: `EditService`

```ts
// src/lib/hooks/database/edit-service/types.ts
export interface EditService {
  plan(params: PlanEditsParams): Promise<PlannedChange[]>;
  apply(params: ApplyChangesParams): Promise<ApplyOutcome>;
  tablePage(params: TablePageParams, signal: AbortSignal): AsyncIterable<RunEvent>;
}
```

- `getEditService()` picks as `getQueryRunner` does: `isTauri() || isWeb()` gives `CoreEditService`, the demo `TsEditService`.
- `TsEditService` is today's builders, `executeAll` loop and `buildQuery`, moved, speaking the generated types. It applies DML-only batches in `BEGIN`/`COMMIT` on DuckDB-WASM's one connection, so the demo follows the same rule and replays the DuckDB fixture cases. Phase 8 deletes it.
- The DuckDB extensions tab keeps `executeRaw` semantics in the demo through a small `extensions` method on the demo's provider path; desktop uses `db.duckdbExtension`.

### 17. Limits on the web

- `EditLimits`, set per interface with `CoreBuilder::edit_limits` beside `run_limits` (the 5b pattern: none on the desktop, the CLI and MCP):
  - `max_changes` per apply or plan call, `INVALID_ARGUMENT` past it;
  - `max_sql_bytes`, the typed changes' SQL together;
  - `max_value_bytes`, every value an apply or plan carries (`param_bytes`' counting);
  - `max_filters` and `max_in_values` per table page, and `max_filter_value_bytes` per filter.
- Starting values for `WEB_EDIT_LIMITS`: 10,000 changes, 2 MiB of SQL, 16 MiB of values, 100 filters, 1,000 `IN` items, 64 KiB per filter value. The probe measures them and they change if it says so.
- `/rpc`'s 64 MiB body limit stays the outer bound.

### 18. Logs

Activity names, ids, counts, modes, kinds and codes only. `Edit`, `Change`, `TableQuery` and the params types have hand-written `Debug` showing table names and counts, never values, keys or SQL. The TS log lines at `query-crud.svelte.ts:62` and `pending-changes.svelte.ts:177` lose the message (Task 1). A `capture_logs` test with canaries in values, keys, filter values and typed SQL (Task 4).

### 19. Arrays, numbers and bools bind as JSON in a JSON column (found in Task 2)

- `plan_edit` binds a value for a column whose metadata type is `json`/`jsonb` (Postgres) or `JSON` (MySQL, MariaDB, SQLite's declared `JSON`, DuckDB) as `Value::Json` when the intent carried an `Array`, a number (`Int`, `Float`, `Decimal`, `BigInt`) or a `Bool`. `Json` stays as it is; `NULL` stays `NULL`. Keys are converted the same way.
- **`Text` stays `Text`**, so the database parses it as today. The grid sends typed JSON as a string (`editedCellValue`, `src/lib/utils/cell-type.ts:286-299`); tagging it would store `"{\"a\":1}"`, a JSON string, instead of the object, and corrupt every typed JSON edit. The cost, kept and documented: a decoded JSON string value (a cell holding `"abc"`) can't be told apart from typed text, so writing it back sends `abc`.
- Why: the providers decode a JSON cell `{"$sq": "json", "v": …}` to its plain JS value, and `encodeParam` sends an array as a SQL array (`Value::Array`) and a number as a number. So a JSON cell holding an array fails on every engine (Postgres binds `bigint[]` and `CAST($1 AS jsonb)` fails; MySQL, MariaDB and SQLite refuse array parameters), and a number has no cast to `jsonb`.
- Task 4 implements it; the Task 2 fixtures record today's binds (`pg/json-top-level-values`, `mysql/json-top-level-values`) with the fixed ones in `changes.json`, and `pg/json-typed-text`/`mysql/json-typed-text` pin typed text staying text. Typed SQL from the editor is unchanged.

---

## The wire and the API

New Rust types live in `seaquel-workspace::edits` (serde, `ts-rs` behind its `ts` feature), re-exported as `seaquel_core::domain::edits`, and wrapped by `seaquel-rpc`. `types:gen` needs no new crate.

```rust
// crates/seaquel-workspace/src/edits.rs

pub struct TableTarget { pub schema: String, pub table: String }

/// `Debug` by hand: the table, the column and counts, never values.
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Edit {
    UpdateCell { target: TableTarget, key: RowValues, column: String, value: Value },
    SetDefault { target: TableTarget, key: RowValues, column: String },
    InsertRow { target: TableTarget, values: RowValues },
    DeleteRow { target: TableTarget, key: RowValues },
    TruncateTable { target: TableTarget },
    DropObject { target: TableTarget, kind: ObjectKind },
}
#[serde(rename_all = "camelCase")]
pub enum ObjectKind { Table, View, MaterializedView }

/// A queue entry as `db.applyChanges` takes it back. `id` is the GUI's.
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Change {
    Edit { id: String, edit: Edit },
    Sql { id: String, sql: String, #[serde(default)] params: Vec<Value> },
}

/// `db.planEdits`.
pub struct PlanEditsParams { pub connection_id: String, pub edits: Vec<Edit> }

/// One planned edit, for the queue's display fields.
pub struct PlannedChange {
    pub sql: String,
    pub params: Vec<Value>,
    pub query_type: QueryType,
    pub dml: bool,
    pub summary: Option<ChangeSummary>,
}

/// `db.applyChanges`.
pub struct ApplyChangesParams {
    pub connection_id: String,
    pub changes: Vec<Change>,
    #[serde(default)] pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryContext>,           // 5b's
}

#[serde(rename_all = "camelCase")]
pub enum ApplyMode { Single, Atomic, InOrder }

#[serde(tag = "outcome", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ApplyOutcome {
    Applied {
        mode: ApplyMode,
        applied: u32,                               // changes that took effect
        results: Vec<ChangeResult>,                 // single and inOrder; empty for atomic
        failed: Option<ApplyFailure>,
        ddl: bool,
        history: Vec<PersistedQueryHistoryItem>,
    },
    ConfirmRequired { destructive: Vec<DestructiveStatement>, destructive_total: u32 },
}
pub struct ChangeResult { pub id: String, pub rows_affected: u64, pub last_insert_id: Option<i64> }
pub struct ApplyFailure {
    pub id: Option<String>,   // None when the driver can't say which statement failed
    pub index: Option<u32>,
    pub code: String,         // NO_ROWS_AFFECTED, NOT_EDITABLE, EXECUTE_ERROR, …
    pub message: String,      // the database's; no key values from Core
}

/// `db.tablePage`. `Debug` shows the table, counts and the page, never filter values.
pub struct TablePageParams {
    pub connection_id: String,
    pub stream_id: String,
    pub query: TableQuery,
    pub page: u32,       // ≥ 1
    pub page_size: u32,  // 1 ..= max_query_rows() - 1
}
pub struct TableQuery {
    pub target: TableTarget,
    #[serde(default)] pub filters: Vec<Filter>,
    #[serde(default)] pub logic: FilterLogic,       // "AND" | "OR"
    #[serde(default)] pub sort: Vec<Sort>,
}
pub struct Filter { pub column: String, pub op: FilterOp, #[serde(default)] pub value: String }
/// Serialised as today's `DataFilterOperator` text.
pub enum FilterOp {
    #[serde(rename = "=")] Eq, #[serde(rename = "!=")] Ne,
    #[serde(rename = ">")] Gt, #[serde(rename = "<")] Lt,
    #[serde(rename = ">=")] Ge, #[serde(rename = "<=")] Le,
    #[serde(rename = "LIKE")] Like, #[serde(rename = "NOT LIKE")] NotLike,
    #[serde(rename = "IN")] In, #[serde(rename = "NOT IN")] NotIn,
    #[serde(rename = "IS NULL")] IsNull, #[serde(rename = "IS NOT NULL")] IsNotNull,
}
pub struct Sort { pub column: String, pub direction: SortDirection }  // "ASC" | "DESC"

/// Web only (Decision 17). Default: no limit.
pub struct EditLimits {
    pub max_changes: Option<usize>,
    pub max_sql_bytes: Option<usize>,
    pub max_value_bytes: Option<usize>,
    pub max_filters: Option<usize>,
    pub max_in_values: Option<usize>,
    pub max_filter_value_bytes: Option<usize>,
}

/// Pure planning. `Err` for a refusal before anything runs.
pub fn plan_edit(edit: &Edit, meta: &TableMeta, dialect: &dyn Dialect, engine: SqlEngine)
    -> Result<PlannedChange, PlanError>;
pub fn classify(changes: &[Planned]) -> ApplyMode;
pub fn cast_map(columns: &[SchemaColumn]) -> CastMap;           // castMapForColumns
pub fn table_select(query: &TableQuery, meta: Option<&TableMeta>, dialect: &dyn Dialect,
    engine: SqlEngine, limits: EditLimits) -> Result<PageSource, PlanError>;
```

```rust
// crates/seaquel-sql/src/statements.rs
pub struct ChangeSummary { pub verb: ChangeVerb, pub table: Option<String>, pub column: Option<String> }
pub fn change_summary(sql: &str, engine: SqlEngine) -> Option<ChangeSummary>;

// crates/seaquel-engine/src/select.rs (pure, like crud.rs)
pub fn build_table_select(/* qs, qi, placeholder, text type, columns to cast */) -> SqlWithBindings;

// crates/seaquel-engine/src/lib.rs
pub struct TransactionError { pub index: Option<usize>, pub error: DbError }
async fn transaction(&self, statements: Vec<BatchStatement>) -> Result<(), TransactionError>;

// crates/seaquel-core
impl CoreBuilder { pub fn edit_limits(self, limits: EditLimits) -> Self; }
impl Workspace {
    pub async fn plan_edits(&self, core: &Core, params: PlanEditsParams) -> Result<Vec<PlannedChange>, CoreError>;
    pub async fn apply_changes(&self, core: &Core, params: ApplyChangesParams) -> Result<ApplyOutcome, CoreError>;
    pub fn table_page<'a>(&'a self, core: &'a Core, params: TablePageParams) -> BoxStream<'a, RunEvent>;
    pub async fn duckdb_extension(&self, core: &Core, connection_id: &str, action: ExtensionAction)
        -> Result<Option<QueryResult>, CoreError>;
}

// crates/seaquel-storage/src/queries/query_history.rs
pub async fn append_many(st: &Storage, items: &[PersistedQueryHistoryItem]) -> Result<(), StorageError>;
// Core-internal: no StorageRequest variant (nothing in the GUI calls it).

// crates/seaquel-rpc/src/db.rs
pub enum DbRequest {
    /* … */
    PlanEdits(PlanEditsParams),
    ApplyChanges(ApplyChangesParams),
    TablePage(TablePageParams),                       // stream only, like Run and Page
    DuckdbExtension { connection_id: String, action: ExtensionAction },
}
pub enum ExtensionAction {           // name: ^[A-Za-z0-9_]+$, else INVALID_ARGUMENT
    List, Install { name: String }, Load { name: String }, Update { name: String },
    InstallCommunity { name: String }, InstallAndLoad { name: String },
}
// EngineRequest: Paginate, BuildUpdate, BuildSetDefault, BuildInsert, BuildDelete removed.
```

- `CoreEvent::Run` carries `db.tablePage`'s events (it is a run with one statement), so the transports need only accept the new start method: `/rpc/stream` and `run_core_stream` take `tablePage` with its `streamId`.
- `DbRequest::stream_id` returns `tablePage`'s; `dispatch_workspace` refuses it with `INVALID_ARGUMENT` like `run`.
- u64/i64 fields get `ts(type = "number")`; optional fields `skip_serializing_if` with `ts(optional)`, as 5b.

```ts
// src/lib/types/pending-changes.ts
export interface PendingChange {
  id: string;
  connectionId: string;
  change: Change;                 // generated; what apply sends back
  sql: string;                    // display
  bindValues?: unknown[];         // display (decoded)
  queryType: QueryType;           // display; Core classifies again
  description: string;
  addedAt: Date;
  sourceTabId?: string;
  origin: PendingChangeOrigin;
  target?: PendingChangeTarget;
}

// src/lib/types/query.ts
export interface StatementResult extends QueryResult {
  // …
  /** The saved connection the result came from; edits go there (Decision 13). */
  connectionId?: string;
}

// src/lib/hooks/database/edit-service/{types,core-service,ts-service,index}.ts
export interface EditService { plan; apply; tablePage }   // Decision 16
```

---

## Ground rules

5b's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules;
- parallel-agent file ownership, with small re-read edits to shared files;
- tests never touch the real keychain, data dir or `~/.ssh`;
- no secrets, SQL or values in `Debug`, errors or logs;
- the full check list;
- effort log: `docs/plans/2026-10-03-phase-5c-effort.md`.

### Constraints the executors must obey

From CLAUDE.md, 5a and 5b:

- **Core builds for wasm32.** No `tokio::spawn`, `Instant` or `SystemTime` in Core crates (`crates/clippy.toml`); time comes from the `Executor`. `seaquel-sql`, `seaquel-workspace::edits` and `seaquel-engine::select` must not panic on any input.
- **The UI never scans or parses SQL.** Descriptions come from `$lib/sql`'s `changeSummary` (wasm). No `toUpperCase().startsWith`, no placeholder regexes.
- **The UI never does dialect work.** Quoting, placeholders, casts, pagination, counts, CRUD and DDL text are Core's (the demo's `TsEditService` excepted, like `TsQueryRunner`). Don't call `getAdapter(` outside `src/lib/engine/`, `src/lib/db/` and the demo services.
- **`executeReadOnly` and the AI and dashboard paths don't change** (5b Decision 15). `select-read-only.test.ts`, `duckdb-read-only.test.ts` and the AI tool tests pass unchanged; workflows only become a new caller.
- **`db.run`, `db.page` and the run fixtures don't change.** `deferWrites` keeps feeding the queue; only what the queue entry holds changes.
- **Error toasts use `errorToast`**; success and info use `toast`.
- **Never edit `src/lib/components/ui/*`.**
- **New i18n keys** go in `messages/en.json`, translated with the `i18n-translator` agent.
- **Run the Svelte MCP `svelte-autofixer`** on every changed `.svelte` file until it reports nothing.
- **Frozen fixtures.** The new edit fixtures, once recorded (Task 2), change only when behaviour is meant to change, with the reason in the README. `crud.json` and the other engine fixtures, the run fixtures and `crates/seaquel-storage/tests/fixtures` stay as they are.
- **Storage rules.** `append_many` is a new `seaquel-storage` function. No schema change: the queue isn't persisted (Q1).
- **npm through mise:** `mise exec -- npm run …`, `mise exec -- npx vitest …`. Task 4 changes `seaquel-sql` and `seaquel-wasm`: rebuild with `npm run wasm:build` after it; `SEAQUEL_WASM_PREBUILT=1` only when no wasm crate changed.
- **One shared `CARGO_TARGET_DIR`** for all agents: `/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`. Clean it between tasks if disk runs low.
- **Web limits from 5a and 5b stay:** 16 streams per socket (a table page is one), 8 sockets per user, 8 MiB frames with batches split past 4 MiB, 16 connections per user with pools of 6, a workspace cap of 1,024, the Origin gate, `WEB_RUN_LIMITS`, the proxy's backpressure. Nothing in 5c raises them.
- **`ConnectPolicy` and `executor` have no defaults.** Tests build Core with both.
- **The MSSQL `tls_server_name` live tests** fail inside the Bash sandbox (macOS trust settings, -36) and pass outside it; run the full live suite outside it for checkpoints.
- **The demo and the tutorial stay on DuckDB-WASM.** `TsEditService` is demo-only.

## Order and estimates

Estimated from 5b's logged per-task times, not from its estimates, which first passes beat by six to eight times. 5b's first passes took ~2.7 h and its fixes ~3.3 h (55% of ~6 h), most of them the probe's. 5c has more surfaces than 5b's one service (three services, a driver trait change, a seam and a GUI cleanup), so first passes are sized at 1.2–1.9 times 5b's, with a probe-fix budget about equal to the first passes, as the design doc recommends.

| # | Task | First pass | Needs | Alongside |
|---|---|---|---|---|
| 1 | TS fixes: result connection, `IN`, data tab race, log lines | 0.15–0.25 h | — | 3 |
| 2 | Edit, apply and table-page parity fixtures, the recorder | 0.3–0.5 h | 1 | 3 |
| 3 | `Driver::transaction` reports its index; live transaction tests | 0.2–0.35 h | — | 1, 2 |
| 4 | Core: `edits` planning, `plan_edits`/`apply_changes`/`table_page`, `select.rs`, `change_summary`, `append_many`, limits | 0.8–1.2 h | 2, 3 | — |
| 5 | `seaquel-rpc` calls, both transports, engine RPC cleanup, `types:gen` | 0.35–0.55 h | 4 | — |
| 6 | TS: `EditService` seam, CRUD/pending/data tab view models, sheet, sidebar DDL, workflows, extensions | 0.8–1.2 h | 5 | — |
| 7 | Probe (web, two users) | 0.25–0.4 h | 6 | — |
| 8 | Docs, measurement, checkpoint | 0.5–0.6 h | all | — |
| | Review fixes (5b: ~35% of first passes) | 1–1.5 h | | |
| | Probe fixes (≈ the first passes, as in 5b) | 2–3 h | | |
| | **Total** | **~6.5–9.5 h** | | |

First passes add up to 3.35–5.05 h. Expect ~7 h logged if the probe finds as much as 5b's did. The riskiest tasks:
- **4:** the apply modes, transactions on five engines, and a second consumer of 5b's page executor. Watch that validation finishes before the first statement.
- **6:** three view models change at once, and the queue's shape changes under the sheet and the grid overlays. Expect its review to find lifecycle issues: a data tab page finishing after its tab closed, an apply finishing after a project switch, a queue entry edited while an apply is in flight.
- **The probe** again: every edit call now plans on the server, reading metadata per call, and the filter DSL is new input from the browser.

---

## Tasks

### Task 1: Fixes in TypeScript first

So the fixtures record the fixed behaviour, as 5b's Task 1 did. The wrong-database bug comes first, before anything else in the slice: every grid edit, Set default, insert and delete must run on the connection its data came from, never on whichever connection is active.

**Files:**
- **The wrong-database bug:**
  - `src/lib/types/query.ts`: `StatementResult.connectionId`, the saved connection the result came from.
  - `src/lib/hooks/database/query-execution.svelte.ts`: set `connectionId` on every result a run or page makes (seeds, pages, error results); `updateCell`, `setCellDefault`, the delegates (`:976-1004`) and the Set default rerun take the connection from the result, not from `state.activeConnection`.
  - `src/lib/hooks/database/query-crud.svelte.ts`: every method (`updateCellDirect`, `setCellDefaultDirect`, `insertRow`, `deleteRow`, `buildCastMap`, `setDefaultExpression`) takes the saved connection id and looks it up in `state.connections` at the time of the call, so a reconnect's new Core id is used; a disconnected or removed connection is refused with the existing "No connection established". Its schema cache is that connection's, not the active one's. The pending queue entry is keyed by that connection too.
  - `src/lib/components/data-viewer.svelte` and `src/lib/hooks/database/data-tabs.svelte.ts` (`saveNewRow`): pass `tab.connectionId`. The pending overlays read that connection's queue (`pendingChangesByConnection[tab.connectionId]`), not `activePendingChanges`.
  - `src/lib/components/query-editor/cell-editing.svelte.ts` and `src/lib/components/insert-row-dialog.svelte`: pass the result's or tab's connection.
- **The log leak:** `query-crud.svelte.ts:62` logs the table and the fact that no row matched, never the key; `pending-changes.svelte.ts:177` logs the index and the error's code (the text before `:` when it has one), never the message, which can hold key values or row values from the database.
- **The data tab:** `IN`/`NOT IN` as a comma list of placeholders, each item trimmed (an empty list: an error result, nothing sent); a per-tab sequence number so a stale refresh drops its answer; `isLoading` cleared on the `providers` guard.
- Tests: `query-crud.svelte.test.ts`, a new `data-tabs.svelte.test.ts`, `query-execution.svelte.test.ts`, `pending-changes.svelte.test.ts`.

**Tests first** (they fail before the fix):
- `a data tab on connection A edits A while B is active in the sidebar`: open a data tab on A, make B active, then edit a cell, Set default, delete a row and save a new row in that tab. Every provider call carries A's Core id and A's table; none carries B's. With pending changes on, the changes are queued under A.
- `an edit on an old query result goes to the connection it came from`: run on A, switch to B, edit the old result: A's Core id.
- `an edit after a reconnect uses the connection's new Core id`;
- `an edit on a result whose connection is disconnected or removed is refused, and nothing runs`;
- `the cast map comes from the result's connection's schema cache`;
- `the stale-key log line holds no key value` and `a failed apply logs no message`: log spies with a canary in the key and in the database error;
- `IN builds one placeholder per item on each engine` and `NOT IN`, and `an empty IN list sends nothing`;
- `a slower earlier refresh doesn't overwrite a later page`.

**Run:** `mise exec -- npx vitest run src/lib/hooks/database src/lib/components` passes; `mise exec -- npm run check` 0/0; the autofixer on the changed `.svelte` files.

**Review:** `rg "activeConnection" src/lib/hooks/database/query-crud.svelte.ts` shows nothing; no edit, insert or delete path reads the active connection; no log line in the edit and apply paths interpolates a message, key or value; the demo takes the same path.

### Task 2: Edit, apply and table-page parity fixtures

Record today's TypeScript, after Task 1, so Core is pinned against it.

**Files:**
- Create `crates/seaquel-workspace/tests/fixtures/edits/`:
  - `plan-{postgres,mysql,mariadb,sqlite,mssql,duckdb}.json`: intent, table metadata and schema-cache state in; SQL, binds and the queue entry's display fields out;
  - `cast-map.json`: `castMapForColumns` over Postgres column sets (enums, arrays, domains, `bit`, `character`, text-like, user types);
  - `apply.json`: a queue, a scripted driver, and the outcome (applied count, the queue left behind, history calls, `hasDdl`);
  - `table-page-{engine}.json`: a tab's state in; the count and page SQL with binds, and the result shown;
  - `summary.json`: `describePendingChange` over typed SQL on each engine, with quoted and qualified names, comments and odd spacing;
  - `README.md`, `changes.json`.
- Create the recorder `docs/plans/artifacts/2026-10-03-record-edit-fixtures.test.ts.txt` (vitest; copy to `src/lib/hooks/database/record-edits.test.ts`, run with `FREEZE_EDITS=1`, delete the copy).

**How it records:** drives the real `QueryCrudManager`, `PendingChangesManager.executeAll` and `DataTabManager.refresh` over the real `CoreProvider` and `RustEngineClient` with a scripted `CoreClient`, as 5b's recorder did, so `driver` is the `db` calls the GUI sends today. The engine client's `build*` answers come from the real dialect through a small Rust helper binary or from the `crud.json` fixtures; the recorder notes which.

**Cases: at least 60, across all six engine ids:**
- update, set default (SQLite's expression, a column without one), insert, delete; composite keys; bigint, decimal, bytes, JSON, NULL and date keys; DuckDB `catalog.schema`; a Postgres table missing from the cache (metadata loaded);
- pending on and off; a repeated edit of the same cell in a data tab (replaced) and in a query tab (queued twice, recorded as today);
- apply: all succeed; a stale key first, in the middle, last; a database error in the middle; a batch with DDL; a batch of one insert (`lastInsertId`); a SQLite truncate; a typed `MERGE`;
- table page: no filters; each operator; `IN` with spaces; AND and OR; sort both ways; page 1, a middle page, the last partial page; a failed count; SQL Server with a `sql_variant` column; SQL Server without `ORDER BY`;
- descriptions for every branch of `describePendingChange`.

**`changes.json`** lists, per case, the Decision that makes Core differ and the intended output. The Rust replay (Task 4) asserts exactly these differ:
- a DML batch failing in the middle applies nothing, and a stale key in one rolls back all (Decision 5);
- a key that isn't the table's primary key is refused (Decision 4);
- history rows carry rows affected and Core's time (Decision 8);
- a partial page runs no count, and a failed count is estimated and flagged instead of 0 (Decision 9);
- the repeated query-tab edit is replaced (Decision 6);
- descriptions where the regexes misread a quoted or qualified name (Decision 12);
- none others. A new one found in Task 4 is a finding to report.
- Added when Task 2 recorded (2026-10-03): a failed metadata read fails the call (Decision 3); the table page's SQL Server and DuckDB placeholders (Decision 9); a replaced queued change takes the new change's origin (Decision 6); JSON values bind as JSON (Decision 19); an unconfirmed sidebar TRUNCATE is `confirmRequired` (Decision 7).

**Replay rules** (in the README, split between the Rust replay and Task 6's vitest): the table page compares the base SELECT (the TS text before the `LIMIT`/`OFFSET` it appended) exactly, with the placeholder changes of Decision 9; the paging as `{limit, offset}` expanded with the dialect's `paginate` (Core fetches `pageSize + 1`); and the count as `count_query(select)`, run only after a full page (Decision 9), never today's `SELECT COUNT(*) FROM <table> WHERE …` text. Timings are ignored.

**Run:** record twice, byte-identical; `git diff --stat` shows only the new directory and the artifact; `mise exec -- npx vitest run src/lib/hooks/database` passes once the copy is deleted.

**Review:** every apply mode and every filter operator has a case; the README names each file's coverage and the recorder's commit.

### Task 3: `Driver::transaction` reports its index; live transaction tests

**Files:**
- `crates/seaquel-engine/src/lib.rs`: `TransactionError`, the default.
- `crates/seaquel-engine/src/sqlx_driver.rs`, `seaquel-engine-sqlite/src/driver.rs`, `seaquel-engine-duckdb/src/driver.rs`, `seaquel-engine-mssql/src/driver.rs`: return the failing statement's index (a bind error before anything runs: its index; BEGIN or COMMIT failing: `None`).
- `crates/seaquel-core/src/lib.rs`, `workspace.rs`: `transaction` passes it through; `crates/seaquel-rpc/src/db.rs` maps it to today's error for `db.transaction`.
- Mocks: `crates/seaquel-server/tests/common/mod.rs`, Core's `tests/mock.rs`.
- Tests: the engines' `tests/live.rs` (Postgres and MySQL gain transaction tests), `seaquel-core/tests/core.rs`.

**Tests first:**
- per engine (Postgres, MySQL, MariaDB, SQLite, SQL Server, DuckDB; live where it needs a server): `a_failing_statement_rolls_back_and_names_its_index`, `a_short_expect_rows_rolls_back_with_no_rows_affected`, `all_statements_commit`;
- MySQL: `a_transaction_starts_through_sqlx_begin` (5b's 1295 on a typed `BEGIN` doesn't apply), and `ddl_inside_commits_implicitly` recorded, not asserted as a behaviour;
- SQL Server: `a_hand_opened_transaction_refuses_the_batch` still passes, with `index: None`;
- rpc: `db.transaction`'s wire error is unchanged.

**Run:** `cargo test -p seaquel-engine -p seaquel-engine-{postgres,mysql,sqlite,mssql,duckdb} -p seaquel-core -p seaquel-rpc --features seaquel-runtime/tokio`, live, `SEAQUEL_TEST_REQUIRE_ENGINES=1`. CI clippy and the wasm32 lines.

**Review:** no behaviour changes but the index; nothing logs SQL.

### Task 4: The edits service in Core

**Files:**
- `crates/seaquel-sql/src/statements.rs`: `change_summary`, with tests; `crates/seaquel-wasm`: export it.
- `crates/seaquel-engine/src/select.rs`: `build_table_select` (pure), with unit tests per placeholder style.
- `crates/seaquel-workspace/src/edits.rs`: the types, `plan_edit`, `cast_map`, `classify`, `table_select`, `EditLimits`, hand-written `Debug`.
- New test file `crates/seaquel-workspace/tests/edits_plan.rs`: the planning replay.
- `crates/seaquel-storage/src/queries/query_history.rs`: `append_many`; tests in `crates/seaquel-storage/tests/query_history.rs`.
- `crates/seaquel-core/src/edits.rs` (new): `Workspace::plan_edits`, `apply_changes`, `table_page`, `duckdb_extension`; `CoreBuilder::edit_limits`. `table_page` reuses `run.rs`'s `execute` for the page kind (made `pub(crate)`).
- New test files `crates/seaquel-core/tests/edits.rs` (replay on the scripted mock driver) and `edits_live.rs`.
- `crates/seaquel-server/src/lib.rs`: `WEB_EDIT_LIMITS` in `web_core()` and the test Core.

**Tests first:**
- Planning (`edits_plan.rs`):
  - `replays_every_plan_fixture` and `cast_map_matches_the_fixture`, with `changes.json` exactly;
  - `a_key_that_is_not_the_primary_key_is_not_editable`, `a_table_without_a_primary_key_is_not_editable`;
  - `sqlite_set_default_uses_the_metadata_default`, `a_column_without_a_default_sets_null`;
  - `classify_is_atomic_only_for_dml`: intents, typed insert/update/delete, `MERGE`, `WITH … UPDATE`, `TRUNCATE`, DDL, a `SELECT`;
  - `a_typed_change_with_two_statements_is_refused` (including MySQL `/*! … */` and MariaDB `/*M! … */`);
  - `table_select_replays_every_table_page_fixture`; `in_binds_each_item`; `mssql_casts_the_types_tiberius_cant_read`;
  - `limits_are_the_interfaces` (none by default; the web's refuse at each bound);
  - `planning_never_panics`: the scanner corpus as SQL and as identifiers.
- Core (`edits.rs`, mock driver):
  - `replays_the_apply_fixtures`;
  - `a_batch_of_one_runs_through_execute_and_returns_last_insert_id`;
  - `a_dml_batch_is_one_transaction` and `its_failure_applies_nothing_and_names_the_change`;
  - `a_batch_with_ddl_applies_in_order_and_stops`;
  - `validation_refuses_before_any_statement_runs` (a bad key in change 3 of 5: the driver sees nothing);
  - `confirm_required_lists_destructive_statements` and `confirmed_applies_them`;
  - `history_is_appended_after_commit_and_only_for_what_ran`, `a_failed_append_does_not_fail_the_apply`, `no_context_no_history`;
  - `metadata_is_loaded_once_per_table_per_call`;
  - `table_page_counts_only_a_full_page`, `a_failed_count_is_estimated`, `table_page_is_cancelled_by_db_cancel_and_by_disconnect`;
  - `another_workspace_cannot_plan_apply_or_page` (`CONNECTION_NOT_FOUND`);
  - `duckdb_extension_refuses_a_bad_name_and_other_engines`;
  - `no_sql_keys_or_values_in_logs` (`capture_logs`, canaries in a value, a key, a filter value and a typed change).
- Live (`edits_live.rs`; Postgres, MySQL, MariaDB, SQL Server, SQLite, DuckDB):
  - update, set default, insert and delete on a table with a uuid, date or composite key (Postgres casts);
  - an atomic batch with a failing third statement: nothing visible from a fresh connection;
  - a batch with DDL on MySQL: the DDL and what came before it stay;
  - a table page with each operator, sort and the count, 250 rows over 3 pages;
  - SQL Server: a table with a `sql_variant` column pages; a hand-opened transaction refuses an atomic batch.

**Implement:** Decisions 1–5, 7–9, 11, 12, 15, 17, 18 and 19 on the Core side.

- A validation refusal inside a batch (a key that isn't the primary key, a failed metadata read, a typed change with two statements) is `Applied { applied: 0, results: [], failed: Some(ApplyFailure { id, index, code: NOT_EDITABLE | …, … }), ddl: false, history: [] }`, not an `Err`: the GUI marks that change. Nothing ran (`apply/pg-key-not-primary-key-refuses-batch`).
- Decision 19 converts only `Array`, numbers and `Bool` to `Json` for a JSON column; `Text` is never converted (typed JSON arrives as text). Test: `json_text_stays_text` with the `*/json-typed-text` cases.
- The key check compares the key's columns with the primary key's as a set, and the WHERE follows the key's order as sent (`pg/key-order-differs-from-primary-key`).
- `ChangeSummary`'s verbs map one to one onto the fixtures' derived `summary` (`insert`, `update`, `delete`, `createTable`, `createIndex`, `dropTable`, `dropIndex`, `dropView`, `truncate`, `alterTable`); Rust compares those fields, never the English text. Every driver call runs under the connection's ownership check; `table_page` registers like `db.page`. The metadata for a batch is loaded before the first statement. Nothing holds a Core lock across an await.

**Run:**
- `cargo test -p seaquel-sql -p seaquel-wasm -p seaquel-workspace -p seaquel-storage` passes.
- `cargo test -p seaquel-core --features seaquel-runtime/tokio` passes, live, `SEAQUEL_TEST_REQUIRE_ENGINES=1`.
- `cargo test -p seaquel-mcp -p seaquel-cli` passes.
- CI clippy, both wasm32 lines, `npm run crates:check`.
- `mise exec -- npm run wasm:build`, then `CI=1 mise exec -- npx vitest run src/lib/sql`.

**Review:** planning has no I/O; metadata calls are deduped; validation precedes execution; no `Instant`/`SystemTime`; no SQL or values logged; `db.run`/`db.page` behaviour and the run fixtures unchanged.

### Task 5: `seaquel-rpc` and both transports

**Files:**
- `crates/seaquel-rpc/src/db.rs`: `PlanEdits`, `ApplyChanges`, `DuckdbExtension` (unary, in `dispatch_workspace`); `TablePage` (stream only, in `dispatch_stream`, `CoreEvent::Run`); `method()`, `stream_id()`.
- `crates/seaquel-rpc/src/lib.rs`: remove `Paginate` and the four `Build*` from `EngineRequest` and `dispatch_on`.
- `crates/seaquel-rpc/tests/{db,dispatch}.rs`, `crates/seaquel-server/tests/{rpc_db,rpc_stream,rpc_live}.rs`: new cases; the removed engine calls' cases go.
- `crates/seaquel-server/src/routes/rpc_stream.rs`: accept `tablePage` starts with a matching, valid `streamId`.
- `src-tauri/src/lib.rs`: `run_core_stream` tracks `tablePage`'s stream id (through `DbRequest::stream_id`, already generic).
- `npm run types:gen`.

**Tests first:**
- rpc: wire snapshots (`method` before `params`, optionals left out, `FilterOp` as today's text); `table_page_is_stream_only`; `apply_changes_is_unary`; the `Debug` of every new params type shows no value, key, filter value or SQL; `a_foreign_connection_is_not_found_through_dispatch`; `removed_engine_calls_are_unknown_methods`.
- server: `a_table_page_streams_over_the_socket`, `a_cancel_frame_stops_a_table_page` (live: the COUNT or page gone from `pg_stat_activity`), `another_users_apply_is_connection_not_found`, `the_web_edit_limits_apply`, `apply_logs_code_group_and_method_only`.
- src-tauri: `a_reload_cancels_a_table_page`.

**Run:** `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio`, live for `rpc_live`; `mise exec -- npm run cli:build && cargo test -p seaquel --lib`; `types:gen` twice with no diff the second time; `npm run check` 0/0 (the TS may not use the new types yet, but `RustEngineClient` must compile without the removed variants: its `build*`/`paginate` go here or in Task 6, whichever keeps `check` green).

**Review:** Node's proxy untouched; the `CANCELLED` synthesis covers table pages; no SQL or values in the server log after a live apply (canary grep).

### Task 6: The GUI onto the edits service

**Files:**
- New `src/lib/hooks/database/edit-service/{types.ts,core-service.ts,ts-service.ts,index.ts}` and tests. `ts-service.ts` takes today's builders (through the demo's `duckdb.ts` adapter), the `executeAll` loop and `buildQuery`/`buildCountQuery`/`buildSelectClause`, emitting the generated types, with DML batches in `BEGIN`/`COMMIT`.
- `src/lib/hooks/database/query-crud.svelte.ts`: intents out, outcomes in; `buildCastMap`, `castMapForColumns`, `setDefaultExpression`, `loadedColumns`, `forgetLoadedColumns`, `executeRaw` and `executeRawDdl` go. `executeReadOnly` stays exactly as it is.
- `src/lib/hooks/database/pending-changes.svelte.ts`: entries hold `change`; `add` takes a planned change; `executeAll` becomes `apply` over `EditService.apply`, handling the three modes and `confirmRequired`; no `addToHistory` (Core's rows go to `insertRecorded`).
- `src/lib/hooks/database/pending-change-description.ts`: `changeSummary` from `$lib/sql`, the English and the origin fallback kept.
- `src/lib/hooks/database/stale-edit.ts`: `expectsRow` goes; `noRowMatchedMessage` formats Core's `NO_ROWS_AFFECTED` for a change's `target`.
- `src/lib/hooks/database/data-tabs.svelte.ts`: `refresh` through `EditService.tablePage`, one `AbortController` per tab (cancel on a new refresh, `remove`, project switch); `buildQuery`/`buildCountQuery`/`buildSelectClause`/`filterDialect` move to `ts-service.ts`.
- `src/lib/hooks/database/query-execution.svelte.ts`: `statementDeferred` queues a `{type: "sql"}` change; the query tab's delete goes through `resolveEditTarget`; the delegates lose `executeRaw`/`executeRawDdl`; `getRowFromTab` goes.
- `src/lib/components/pending-changes-sheet.svelte`: the confirm dialog lists destructive statements and sends `confirmed`; the SQL view shows values beside the SQL; failures per mode.
- `src/lib/components/sidebar/manage/schema-tab.svelte`: drop and truncate as intents.
- `src/lib/hooks/database/create-table-tabs.svelte.ts`: apply through `EditService.apply` with typed changes.
- `src/lib/components/insert-row-dialog.svelte`: nothing renders it (found in Task 1, which gave it a `connectionId` prop). Delete it, or wire it up, rather than port it.
- Task 1 already did the query tab's delete through `resolveEditTarget` (`deleteRowAt`); workflows (`database.svelte.ts`, `executeRaw` on the active connection) are still this task's.
- `src/lib/hooks/database/workflow-manager.svelte.ts`, `hooks/database.svelte.ts`: workflows on `executeReadOnly` with the node's connection, `WORKFLOW_MAX_ROWS`, a controller per node; the result node shows `truncated`.
- `src/lib/hooks/database/extensions-duckdb-tabs.svelte.ts`: `db.duckdbExtension` on desktop, the TS path in the demo.
- `src/lib/engine/{types.ts,rust-engine-client.ts,ts-engine-client.ts}`: `paginate` and `build*` go from the interface and the Rust client; the demo services call the adapter.
- New i18n keys, if any, translated.

**A cancelled in-order apply (found in Task 4's review).** Dropping `db.applyChanges` (a closed window, an aborted web request, an eviction) rolls an atomic batch back, but an in-order or single apply keeps what already ran, committed, with no outcome and no history rows (`crates/seaquel-core/src/edits.rs`, module doc). The GUI can't tell which changes ran: after an apply that ended without an outcome, keep the queue, mark it as possibly partly applied, and reload the schema and the connection's data tabs before the user applies again.

**Tests first (vitest):**
- `an immediate edit sends one applyChanges and shows the new value`;
- `a queued edit stores the intent and its planned display fields`;
- `a repeated edit of the same cell replaces the queued one in a query tab too`;
- `apply clears the queue after success`, `keeps it whole after an atomic failure and marks the change`, `removes the applied prefix after an in-order failure`;
- `confirmRequired opens the dialog with the list and Confirm resends confirmed`;
- `NO_ROWS_AFFECTED shows the i18n message with the change's table and key`;
- `apply history rows go to the cache; immediate edits record none`;
- `ddl in an apply reloads the schema and refreshes the connection's data tabs`;
- `a data tab page is a tablePage stream; a new refresh cancels the old; closing the tab cancels it`;
- `a data tab with a failed count shows an estimate`;
- `sidebar drop and truncate send intents with confirmed`;
- `the table editor applies its statements in order and stops at the first failure`;
- `a workflow node runs read-only on its own connection and shows truncated`, `a write in a node is refused`, `re-running a node cancels the previous run`;
- `the extensions tab sends typed actions on desktop`;
- `the demo service replays the DuckDB fixture cases` (TsEditService over a scripted DuckDB-WASM connection, as 5b's `ts-runner-duckdb.test.ts`);
- `select-read-only`, `duckdb-read-only`, the AI tool tests and the run view-model tests pass unchanged.

**Run:** `npm run check` 0/0; `CI=1 mise exec -- npx vitest run`; `npx oxlint --type-aware --type-check --deny-warnings`; the autofixer on each changed `.svelte`; `build`, `build:web`, `build:demo`; live on desktop (`npm run tauri dev`) and web (`npm run dev:web:full`): the Manual checks' edit, apply and data-tab items.

**Review:** no `$lib/sql` scanning or SQL text built outside the demo services; no `provider.execute`/`select` left on desktop or web edit paths; `rg "activeConnection" src/lib/hooks/database/query-crud.svelte.ts` empty; `$state` proxy rules kept for the grid write-back; the demo's edits, apply, data tab, workflows and extensions work.

**Execution notes (Task 6, 2026-10-03):**
- **An apply that ends without an answer is `interrupted`, whatever its mode** (`PendingChangesManager.apply`). No answer means a code that says nothing about what ran: `NETWORK_ERROR`, `PROTOCOL_ERROR`, `UNKNOWN`, `CANCELLED`, `WS_CLOSED` or `HTTP_5xx`. An atomic batch can commit and then lose its reply, so treating it as rolled back would let a retry run every change twice. The queue is kept and marked (`state.pendingChangesInterrupted`, a banner in the sheet), and the schema and the connection's data tabs reload. A Core refusal (any other code) applied nothing and keeps the queue unmarked.
- **The sheet's confirmation** (`listDestructive`, `confirmedFor`): the dialog lists the destructive statements `$lib/sql` finds in each change's SQL and sends `confirmed` only when it listed some. With nothing listed it goes unconfirmed, so Core asks about anything the list missed and the dialog reopens with Core's list.
- **Two quick queued edits of one cell** keep the later: `QueryCrudManager` numbers each queued update or Set default per cell and drops a plan that lands after a later edit of the same cell.
- **The demo's gaps, for phase 8** (`TsEditService`, deleted then with `TsQueryRunner`):
  - it reads no table metadata, so it doesn't check an edit's key against the primary key (no `NOT_EDITABLE`);
  - its data tab page is the pre-5c query: it always counts (a count before every page, `0` when the count fails, never estimated), and it pages with `LIMIT/OFFSET` on `pageSize` rows rather than Core's `pageSize + 1` with a count only after a full page;
  - its filter values are inlined as escaped literals (`formatLiteralValue`), because `DuckDBProvider.select` ignores bind values. The `$1` binds the demo sent before 5c were dropped, so filtering in the demo failed at HEAD.

### Task 7: Probe

A separate agent runs a two-user web instance (`SEAQUEL_WORKSPACE_CAP=2`, no origin variables) and uses only the browser-facing endpoints, as in 5a and 5b. It records evidence for each check:

- **Cross-user.** `db.planEdits`, `db.applyChanges`, `db.tablePage`, `db.cancel` and `db.duckdbExtension` against user B's connection and stream ids: `CONNECTION_NOT_FOUND`, nothing run, B's page not cancelled.
- **Input into SQL.** Schema, table, column, key and sort names holding quotes, `;`, comments and NUL; a `FilterOp` outside the enum; a sort direction that isn't `ASC`/`DESC`; a key naming a non-key column; an `in` list of 10,000; a typed change with two statements. All refused or quoted; nothing but the intended statement reaches the database (`pg_stat_statements` or the MySQL general log).
- **Atomicity.** A 1,000-edit DML batch with a failure at 999: nothing visible. Eviction and a closed socket mid-apply: rolled back. MySQL with a DDL change: in order.
- **Confirmation.** An unconfirmed `DELETE FROM t` typed change is `confirmRequired`; confirmed, it runs.
- **Load.** 10,000 changes, 16 MiB of values: time and memory on the server, other users served. A table page on a 5,000,000-row table: the count's cost, and a cancel stops it on the server. 100 filters.
- **Metadata cost.** Edits per second on Postgres with the per-call `table_metadata` (Decision 3), to decide whether a cache is needed. Also time SQL Server's per-page `table_metadata` read in `db.tablePage` (the tiberius casts need the column types; left per page as planned, Task 4 review).
- **Leaks.** No SQL, key, value or filter value in either server log, including from failed applies and counts.

Probe fixes are budgeted separately.

**Findings (Task 7, 2026-10-03).** Two users on one release instance (workspace cap 2), the browser-facing endpoints only.
- **Held.** Every cross-user `db.planEdits`, `db.applyChanges`, `db.tablePage`, `db.cancel` and `db.duckdbExtension` got `CONNECTION_NOT_FOUND` and ran nothing; B's page ran to its end. Quotes, `;`, comments and brackets in schema, table, column, key and sort names were quoted or refused (`NOT_EDITABLE` for an unknown table or column, the database's error for a table page) on Postgres, MySQL and SQL Server; a key naming a non-key column, a partial composite key, an empty key and a keyless table were `NOT_EDITABLE`. A 1,000-edit atomic batch failing at 998 (stale key or bad value) changed nothing, in ~200 ms. MySQL with a DDL change applied in order and kept what ran. Unconfirmed `DELETE FROM t`, `UPDATE` without WHERE, and sidebar TRUNCATE and DROP were `confirmRequired` (150 listed as 100 of 150); a keyed delete wasn't. SQL Server refused an atomic batch while a hand-opened transaction was open. Past each web limit (10,001 changes or edits, 101 tables, 2 MiB of SQL, 16 MiB of values) the call was `INVALID_ARGUMENT` naming it. A 5,000,000-row table page with a filter took 5.3 s with its count, and a cancel or a closed socket took it off the server within 0.7 s. The web's 16-stream, 8-socket and run limits held.
- **Measured.** Postgres, one client: an edit applied at 12.3 ms (82/s) against 6.2 ms for the same typed UPDATE, so the per-call metadata read is ~6 ms; eight clients reached 464 edits/s. Planning 100 tables took ~600 ms. SQL Server's per-page column read adds ~1.2 ms to a table page (1.6 ms against 0.4 ms for the same SQL through `db.page`). No cache is needed yet.
- **Found.** I1: closing the browser tab didn't stop an apply (Node's route never aborted its call to Rust), so an aborted atomic apply committed. I2: adapter-node's 512 KB `BODY_SIZE_LIMIT` applied to `/api/rpc`, so an apply of a few thousand changes never reached Rust. M1: eviction didn't stop an apply in flight (it committed). M2: a NUL in a table name reached the metadata read and came back as the database's `QUERY_ERROR`. M3: an empty page past the end reported the wrong total (from 5b's `db.page` too). M4: eight concurrent 17 MB applies from one user took Rust to 877 MB and Node to 952 MB. M5: Postgres notices (a `RAISE WARNING`'s text) reached the server log.
- **Fixed** (effort log, "Task 7" rows; details in "Execution notes"). The re-probes on fresh instances confirmed each fix: an aborted or evicted atomic apply left the table untouched, an in-order one stopped at the statement in flight, 10,000 changes in a 17.4 MB body applied in 1.6 s, 22 MB was refused with 413, page 30 of a 2,000-row table said 2,000 rows, and eight concurrent 17 MB applies peaked at 242 MB (Rust) and 546 MB (Node) with two applied and six refused with 429. No SQL, key, value or canary reached either server log, including SQL Server's `THROW`, conversion errors and `PRINT`.

### Task 8: Docs, measurement, checkpoint

- **CLAUDE.md:** edit intents and the key check; `db.planEdits`/`db.applyChanges` and the three apply modes; `db.tablePage`; workflows read-only; `db.duckdbExtension`; `EditLimits`; `Driver::transaction`'s index; the `EditService` seam; the engine RPC's removed calls; history from applies.
- **Design doc:** the status line, "Phase 5c cost" and "What this means for the next slice"; the CRUD slice renamed 5d.
- **This plan:** execution notes and release notes. The release notes must say that workflow query nodes are now read-only: a workflow that writes stops working (owner, 2026-10-03). They also name edits following their connection, `IN`/`NOT IN` working, atomic DML applies, the 10,000-row cap on workflow nodes and the web's edit limits.
- **Effort log:** the totals.
- **The full check list,** as 5b's checkpoint, the oxlint type check included.

**Status (Task 8):** done. CLAUDE.md, the design doc's status line and "Phase 5c cost", the execution notes, release notes, checkpoint, manual checks and follow-ups below, and the effort log's totals are written. The full check list ran; see "Checkpoint". The manual checks below are the owner's.

## Manual checks

For the owner, after Task 8.

**Setup.**
- Databases: `docker compose -f e2e/test-databases/docker-compose.yml up -d`, then `MSSQL_PASSWORD='Seaquel_Test_123!' npm run e2e:db:seed -- all`.
- Credentials: Postgres `postgres@127.0.0.1:5432/seaquel_test`, no password. MySQL `root@127.0.0.1:3306/seaquel_test` and MariaDB `root@127.0.0.1:3307/seaquel_test`, no password. SQL Server `sa` / `Seaquel_Test_123!` on `127.0.0.1:1433`, database `seaquel_test`, trusting the certificate. DuckDB (desktop only): a new file.
- To watch a statement on the server: `docker exec seaquel-postgres psql -U postgres -d seaquel_test -c "select pid, state, query from pg_stat_activity where datname = 'seaquel_test' and pid <> pg_backend_pid()"`.
- Tables, once, on Postgres (pending changes off, Run all): `CREATE TABLE e (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), n int, s text DEFAULT 'x', d date, doc jsonb); INSERT INTO e (n, s, d, doc) SELECT g, 'r' || g, date '2024-01-01' + g, '[1, 2]' FROM generate_series(1, 250) g; CREATE TABLE k (a int, b text, v text, PRIMARY KEY (a, b)); INSERT INTO k VALUES (1, 'x', 'one'), (2, 'y', 'two'); CREATE TABLE t2 (id int); CREATE VIEW ev AS SELECT * FROM e;`
- On MariaDB: `CREATE TABLE j (id int PRIMARY KEY, doc JSON); INSERT INTO j VALUES (1, '[1, 2]'), (2, '{"a": 1}');`
- On SQL Server: `CREATE TABLE v (id int IDENTITY PRIMARY KEY, x sql_variant, s nvarchar(20)); INSERT INTO v (x, s) VALUES (1, 'a'), ('b', 'b');`

**Desktop** (`npm run tauri dev`). Back up the data dir first (`~/Library/Application Support/app.seaquel.desktop.dev`).

- [ ] **Immediate edits, data tab.** Pending changes off (Settings). Open `e` from the sidebar. Edit an `s` cell, Set default on another (shows `x`), insert a row, delete a row. Each shows at once, and no history entry appears.
- [ ] **Immediate edits, query result.** `SELECT * FROM k` and edit a `v` cell (composite key), then delete a row: both saved (`SELECT * FROM k`).
- [ ] **Wrong connection.** Open the data tab for Postgres `e`, then click a MySQL connection in the sidebar so it's active, click back on the `e` tab (don't reselect Postgres) and edit a cell: the change is in Postgres `e`, and MySQL is untouched. Same for an old result: run `SELECT * FROM k` on Postgres, make MySQL active, edit the old result's `v`.
- [ ] **Views.** Open `ev` and edit a cell: refused with "The row's key isn't the primary key…" or "…has no primary key…"; nothing changes.
- [ ] **Queued edits.** Pending changes on. Edit the same `s` cell twice in a `SELECT * FROM e` result: one queued change, the second value. In the `e` data tab insert and delete a row: the grid shows both overlays.
- [ ] **Atomic apply with a stale key.** Queue three edits of different `e` rows. In another tab with pending off, `DELETE FROM e WHERE n = <one of those rows' n>`. Apply: the sheet says none were saved and marks that change; the other two are unchanged in the database; the queue is whole. Remove the marked change and apply: the two save.
- [ ] **DDL batch in order.** Queue an edit of `e`, then `CREATE TABLE z (i int)` typed in the editor, then an edit of a row you then delete by hand. Apply: the first edit and `z` stay (the sidebar shows `z`), the failing edit is marked and stays queued with nothing after it.
- [ ] **Destructive confirm.** Queue `DELETE FROM t2` from the editor. Apply: the dialog lists it; Cancel keeps it queued; Confirm runs it.
- [ ] **History.** After a successful apply, one history entry per change, each with its rows affected.
- [ ] **Sidebar DROP/TRUNCATE.** With pending off, truncate `t2` from the sidebar (its own dialog asks, then it runs). With pending on, drop `z`: queued as one change; apply drops it and the sidebar reloads.
- [ ] **Data tab filters and paging.** In `e`: `n > 100` AND `s LIKE 'r1%'`, then OR; `n IN 1, 2, 3` (3 rows); `n NOT IN 1, 2` (248); `d IS NULL` (0). Sort by `n` both ways. At page size 100, sorted by `n`, page 2 starts near 101 and page 3 is partial. Click through pages quickly: the tab ends on the last one clicked. `n > 9` still compares as text (so 10 to 89 are missing; known).
- [ ] **JSON.** In `e`, edit a `doc` cell from `[1, 2]` to `[1, 2, 3]`, and another to `{"a": 1}`: `SELECT jsonb_typeof(doc) FROM e WHERE doc <> '[1, 2]'` shows `array` and `object`, not `string`. On MariaDB, edit `j.doc` to `[3]`: `SELECT JSON_TYPE(doc) FROM j` shows `ARRAY`.
- [ ] **TRANSACTION_OPEN (SQL Server).** Pending off: run `BEGIN TRANSACTION` (it may report error 266; the transaction stays open). Pending on: queue two edits of `v.s` and apply: refused with "This connection has a transaction you opened yourself…", nothing applied. Pending off: `ROLLBACK`, then apply again: both save. The `v` data tab pages (the `sql_variant` column shows).
- [ ] **SQLite and DuckDB.** SQLite: Set default on a column with a default expression; truncate a table from the sidebar with pending on (queued, applies with the other DML). DuckDB: edit a row in an attached catalog's table.
- [ ] **Workflows.** A query node `SELECT * FROM e` shows rows. `SELECT * FROM generate_series(1, 20000)` shows 10,000 and says the rest were cut. `DELETE FROM e` is refused as read-only. A node made on Postgres still runs on Postgres with MySQL active.
- [ ] **DuckDB extensions tab.** On a DuckDB connection: the list loads; install and load `json`; install a community extension (`h3`, needs network).
- [ ] **AI and dashboards.** A chat's `run_query` and a dashboard widget still return rows.

**Web** (`npm run build:web:full`, then `SEAQUEL_WORKSPACE_CAP=2 npm run start:web`, at `http://localhost:8787`). Sign in as user A in one browser and user B in another.

- [ ] **The desktop list** for Postgres, MySQL/MariaDB and SQL Server (no SQLite or DuckDB on web): immediate and queued edits, the wrong-connection case, atomic and in-order applies, the confirm, history, sidebar DROP/TRUNCATE, filters and paging, JSON, `TRANSACTION_OPEN`, workflows. B sees none of A's history.
- [ ] **The change limit.** Pending on, on Postgres: `python3 -c "print('INSERT INTO t2 VALUES (1);\n' * 5001)" | pbcopy`, paste, Run all, then the same with `5000`: 10,001 queued. Apply: refused with "At most 10000 changes can be applied at once; this apply has 10001. Apply them in parts.", nothing inserted. Remove one and apply: 10,000 rows in `t2`.
- [ ] **Closing the tab mid-apply.** Pending on: queue `UPDATE e SET s = 'slow' || (SELECT '' FROM pg_sleep(10)) WHERE n = 1` from the editor and an edit of another `e` row (two DML changes, atomic). Apply and close the browser tab within 10 s. After 10 s: `SELECT s FROM e WHERE n = 1` isn't `slow`, the other edit isn't saved, and `pg_stat_activity` shows nothing `idle in transaction`.
- [ ] **"May be partly applied".** Queue `CREATE TABLE z2 AS SELECT 1 AS i FROM pg_sleep(10)` from the editor and an edit (in order). Apply, and within 10 s set the browser's devtools Network to Offline: the sheet keeps the whole queue and shows the "may be partly applied" banner. Go back online and clear the queue by hand.

**Demo** (`npm run build:demo`, then `npm run preview:demo`):

- [ ] An edit in a data tab and in a query result; a queued batch applied; the data tab's filters (they failed before 5c) and paging; a workflow node shows rows and refuses `DELETE`; the extensions tab lists.

**MCP.** With the desktop build, `claude mcp add` from Settings → MCP, then ask for a row count through `run_query` and run a saved query through `run_saved_query` on an exposed Postgres connection: both return rows, as in 5b.

---

## Execution notes (2026-10-03)

The plan was executed task by task with subagents, Tasks 1, 2 and 3 overlapping, with a review after each task, a probe on a two-user web instance, two rounds of probe fixes each with its own review and re-probe, and this checkpoint. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-10-03-phase-5c-effort.md`; the measured cost is in the design doc ("Phase 5c cost").

**What went differently from the plan**

- **The wrong-connection fix went wider** (Task 1 and its review). Besides the four CRUD calls, paging, the Set default and delete reruns, the query tab's pending overlays, the extensions tab, the confirm retry and the sidebar's DROP/TRUNCATE all read the active connection; each now uses its result's or tab's. The sheet and the header badge follow a new `state.pendingConnectionId` (the focused data tab's or result's connection, else the active one). Edits look the Core id up after their last await, since a reconnect mid-build sent them to the stale id. An existing test that refused Confirm when the active connection changed was rewritten: switching connections no longer refuses, a reconnect, disconnect or removal does. Workflows moved in Task 6.
- **Decision 19 (JSON) was added in Task 2** and narrowed in its re-review: arrays, numbers and bools bind as JSON in a JSON column, and text never converts, since the grid sends typed JSON as text. Task 4's review found MariaDB's `JSON` columns report `longtext`, so it never fired there; MariaDB's metadata now reports `json` for a column whose CHECK is exactly ``json_valid(`col`)``. A decimal converts only when it's an integer or has at most 15 significant digits.
- **180 fixture cases, 89 listed changes** (the plan asked for at least 60). Task 2 added change classes the plan didn't list: a failed metadata read fails the call (Decision 3), the table page's SQL Server and DuckDB placeholders (Decision 9), a replaced queued change takes the new origin (Decision 6), JSON binds (Decision 19) and an unconfirmed sidebar TRUNCATE (Decision 7). Every partial table page differs, since the count no longer runs there.
- **`Driver::transaction` returns each statement's rows affected** (`Result<Vec<u64>, TransactionError>`, not `()`), because atomic applies write history with rows affected. `db.transaction`'s wire is unchanged.
- **A hand-opened transaction has its own code** (Tasks 3 and 5). DuckDB turned out not to refuse a nested `BEGIN`: it aborts the user's transaction. The DuckDB driver now probes with two `txid_current()` calls and refuses the batch, as SQL Server does; both answer `TRANSACTION_OPEN` (409 on web) with the `TRANSACTION_ALREADY_OPEN` message, instead of `EXECUTE_ERROR`.
- **The key check is stricter than planned** (Task 4 review): an update, Set default or insert column the metadata doesn't list is `NOT_EDITABLE` too. SQLite Set default on a column with no default sets NULL (the first pass fell back to `DEFAULT`, which SQLite refuses). The data tab reuses the query builder's `SortDirection` (it gained `Deserialize`).
- **`max_tables`** (100 distinct tables per call) joined `EditLimits` in Task 4's review, since each table is a metadata read; it's checked before any read.
- **The wire** (Task 5): `DbResponse` and `Response` are `Serialize` only; `PlannedChange.summary` is left out when absent; `EngineResponse::SqlWithBindings` went with the `Build*` calls. New web statuses: `NOT_EDITABLE` 400, `CONFIRM_REQUIRED` and `TRANSACTION_OPEN` 409.
- **The GUI** (Task 6): the insert dialog was deleted (nothing rendered it); a new origin `drop-view` for the sidebar's view drops; the table editor sends `confirmed: true` (the editor is the confirmation); the sheet's own dialog lists what `$lib/sql` finds and lets Core ask again for anything it missed; an apply without an answer is `interrupted` whatever its mode (Task 6 notes); `QueryHistoryManager.addToHistory` lost its last caller and went. The demo's data tab filters were broken before 5c (DuckDB-WASM's provider drops binds); `TsEditService` inlines escaped literals.
- **Probe fixes** (Task 7, two rounds):
  - I1: `/api/rpc` aborts its call to Rust when the client's socket closes (`platform.req` under adapter-node; a Vite plugin puts the request in an `AsyncLocalStorage` in `dev:web`), since SvelteKit's `request.signal` doesn't fire once the body is read. The statement in flight may still run to its end on the server (dropping an sqlx future doesn't cancel it; 5b's follow-up).
  - I2: adapter-node's limit is global, so `server.js` moves the operator's `BODY_SIZE_LIMIT` to `SEAQUEL_BODY_SIZE_LIMIT`, sets adapter-node's to `Infinity` and imports the handler after that; `handleBodyLimit` gives `/api/rpc` 20 MiB and every other route the operator's value, by `Content-Length` and by counting a body sent without one.
  - M1: each workspace has a `closing` token that `close_all` cancels; an apply races it (`WORKSPACE_CLOSED`).
  - M2: a NUL in any name is `INVALID_ARGUMENT` before any metadata read.
  - M3: an empty page past the first runs the count (estimated as `offset` when it fails), in `db.page` and `db.tablePage`.
  - M4: the first fix counted only bodies of 1 MiB or more, which the review found let many small applies through at once. Now Rust holds each user's bodies in flight to 40 MiB and at most 4 apply, plan and extension calls, and Node holds each user's bodies to 40 MiB before reading; in both a lone call always runs. `TOO_MANY_REQUESTS` (429), shown as a translated sentence.
  - M5: `sqlx::postgres::notice` is dropped by the server, desktop and CLI log filters; the review found tiberius logs every SQL Server error token and `PRINT` with the server's text, so `tiberius::tds::stream::token` is dropped too and the rest of tiberius held at WARN.

**Decisions made during execution**

- **Text in a JSON column stays text** (Decision 19, owner's rule for the grid's typed JSON). A JSON string cell (`"abc"`) written back is sent as `abc`; kept and documented.
- **Workflows that write stop working** (owner, 2026-10-03, Q6): the release notes say so.
- **No metadata cache.** The probe measured the per-call read at ~6 ms on Postgres; a cache stays a follow-up.
- **SQL Server's per-page column read stays per page** (~1.2 ms in the probe).
- **The body limit keeps its name for operators.** `BODY_SIZE_LIMIT` is still what an operator sets; `SEAQUEL_BODY_SIZE_LIMIT` is where `server.js` moves it.

**Release notes**

For the release after 5b's. Earlier phases' notes still apply as written.

Changes you may notice:

- **Grid edits go to the right database.** Editing, inserting, deleting or setting a default in a data tab or an older query result used to run on whichever connection was active in the sidebar. It now runs on the connection the data came from.
- **Pending changes that are all inserts, updates and deletes apply all or nothing.** If one fails, for example because its row was deleted meanwhile, none are saved and the queue stays as it was, with the failing change marked. A batch that also holds other statements (`CREATE`, `ALTER`, `TRUNCATE`, …) still applies in order and stops at the first failure.
- **Views and tables without a primary key can't be edited from the grid** any more: the edit is refused before anything runs. A key that isn't the table's primary key is refused the same way.
- **`IN` and `NOT IN` filters work** in the data tab: separate values with commas. `>`, `<`, `>=` and `<=` still compare as text.
- **The data tab stops its query when you leave.** Closing the tab or paging again stops the previous query on the server, and quick paging ends on the last page clicked. When the total can't be counted, the tab shows an estimate instead of a single page.
- **JSON cells holding arrays, numbers or `true`/`false`** can be saved (they failed before), including MariaDB's `JSON` columns.
- **Workflow query nodes are read-only.** They run on the node's own connection, can only read (a workflow that writes stops working and says why), and show at most 10,000 rows, saying when a result was cut. Re-running or deleting a node stops its query.
- **The pending changes sheet** lists the destructive statements in its confirmation, shows each change's values beside its SQL, and marks the queue "may be partly applied" when an apply ends without an answer (a dropped network, a closed server) instead of guessing.
- **Editing the same cell twice in a query result** queues one change, as the data tab already did.
- **History entries for applied changes** show the rows affected and the real time.
- **With a transaction you opened yourself** on SQL Server or DuckDB, applying several changes is refused until you commit or roll back. On DuckDB the apply used to abort your transaction.
- **The demo's data tab filters work again.**

Self-hosted web:

- **Workflows are read-only** (above). Any workflow that writes to a database stops working after the upgrade.
- **Request body limits.** `BODY_SIZE_LIMIT` (512K by default) now applies to every route except `/api/rpc`, which takes up to 20 MiB (a higher `BODY_SIZE_LIMIT` raises it, up to 64 MiB). A larger `/api/rpc` body gets 413 with `{"code": "INVALID_ARGUMENT"}` naming the limit. `server.js` moves the value to `SEAQUEL_BODY_SIZE_LIMIT` at startup and sets adapter-node's own to `Infinity`: start the app through `server.js` (the image and `npm run start:web` do), and don't set `SEAQUEL_BODY_SIZE_LIMIT` yourself. Before this release `/api/rpc` was held to `BODY_SIZE_LIMIT` too, so large applies failed.
- **New limits on edits** (`400 INVALID_ARGUMENT`, naming the limit): 10,000 changes per apply, touching at most 100 tables, with at most 2 MiB of SQL and 16 MiB of values; per data tab page 100 filters and 100 sort columns, 1,000 `IN` values and 64 KiB per filter value. A name holding a NUL character is refused.
- **Per-user limits on calls in flight** (`429 TOO_MANY_REQUESTS`): 40 MiB of request bodies at once, in Node and in the Rust service, and 4 apply, plan or extension calls at once. A single call always runs.
- **New error codes on `/api/rpc`:** `NOT_EDITABLE` (400), `CONFIRM_REQUIRED`, `NO_ROWS_AFFECTED` and `TRANSACTION_OPEN` (409), `TOO_MANY_REQUESTS` (429). `TRANSACTION_OPEN` replaces `EXECUTE_ERROR` for a batch refused because of a hand-opened transaction; its message is unchanged.
- **Views are no longer editable from the grid** (`NOT_EDITABLE`).
- **Different SQL on your database.** Each edit now reads the table's catalog first (one metadata query per table per call). The data tab's queries use `@P1…` on SQL Server (was `@p1…`), its count is `SELECT COUNT(*) as total FROM (…) AS count_query` over the page's query and runs only after a full page. Update monitoring or query-store filters that match the old text.
- **A closed browser tab stops its apply** on the server, and so does evicting a workspace (`WORKSPACE_CLOSED`): an all-DML apply rolls back, an in-order one stops at the statement in flight.
- **Log content.** Postgres notices and SQL Server's error and `PRINT` token stream no longer reach the Rust service's log; the rest of tiberius logs at WARN.
- **The engine RPC's `paginate` and `build*` calls are gone.** Only a custom client could be affected.

---

## Checkpoint

The full check list, run on 2026-10-03 one step at a time on the shared `scratchpad/p5a/target`, inside the Bash sandbox, with all five containers healthy and the live env (`SEAQUEL_TEST_POSTGRES`, `_MYSQL`, `_MARIADB`, `_MSSQL`, `SEAQUEL_TEST_SSH`, `SEAQUEL_TEST_REQUIRE_ENGINES=1`; values as in `ci.yml`), npm through `mise exec`:

| Check | Result |
|---|---|
| `npm run crates:check` | pass |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, live | 1,474 passed, 3 failed, 3 ignored (162 test targets, ~273 s of test time). The 3 are `seaquel-engine-mssql`'s `tls_server_name` tests, the known sandbox issue (below) |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` | pass |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run cli:build`, `cargo check -p seaquel` | pass |
| `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 40 passed |
| `npm run types:gen`, generated types unchanged | pass (the 163 files are identical before and after) |
| `npm run check` | pass: 0 errors, 0 warnings |
| `npx oxlint --type-aware --type-check --deny-warnings` (CI's lint step) | pass |
| `CI=1 npx vitest run` | pass: 1,707 tests in 89 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |

**The MSSQL `tls_server_name` tests** (`a_bracketed_ipv6_host_is_dialled`, `without_it_the_tls_name_is_host`, `the_tls_name_is_tls_server_name_while_the_socket_goes_to_host`) panic in tiberius with `could not load platform certs: … code: -36`: the sandbox blocks the macOS trust settings that rustls-native-certs reads. They passed in Task 3 and in both probe-fix rounds. Not a 5c change, and CI (Linux) isn't affected.

The desktop checks compiled the working tree's unrelated CLI-download changes in `src-tauri` too; they broke nothing. The tree also has a stray `c7.out` at the repo root (a probe script's error output, 15:45), which isn't 5c's and should be deleted before committing.

CI doesn't run oxfmt. `oxfmt --check` passes on CLAUDE.md and flags the design doc, this plan and the effort log, whose tables and lists are in the plans' own style (the design doc was already flagged in 5b); left as they are.

**Manual checks:** pending (the owner).

**Not run:** the release workflow and a signed build.

---

## Follow-ups (not in 5c)

- **Next slice, 5d:** the connection, project and saved-query CRUD (the design doc's old "5c").
- **High priority, from 5b:** a row-returning statement kind, and showing an estimated total as an estimate (the data tab now gets estimates too, and the bar shows them as exact).
- **Typed range filters.** `>`/`<` compare text (kept). Bind the value as the column's type per engine; Postgres refuses `integer > text` without a cast.
- **`IN` values holding commas.**
- **Optimistic concurrency** for edits (Q4 option C): compare the edited cell's old value too.
- **A metadata cache in Core,** invalidated by DDL Core runs, if ~6 ms per call (the probe) becomes a problem.
- **Apply progress and Stop:** a stream call for long batches.
- **Atomic DDL** on the engines whose DDL is transactional (Postgres, SQLite, DuckDB, SQL Server).
- **Cancelling the statement in flight on the server** when an apply, write or count is dropped: the abort stops the apply, but a running statement (a `pg_sleep` in the probe) finishes on the server. Still open from 5a/5b.
- **Node holds every `/api/rpc` body it reads** until Rust answers; streaming the body through would drop the per-user budget's memory (546 MB peak at the limits). A slow sender holds its reservation until Node's request timeout.
- **JSON string cells** can't be told apart from typed text (Decision 19), so `"abc"` is written back as `abc`. A typed JSON editor would close it.
- **Pinned quirks:** unsigned bigint keys past i64 bind as decimals; NULL keys never match; MyISAM ignores an atomic batch's rollback (documented, not detected).
- **Persisting the queue** (Q1 option C), if users ask.
- **An inline SQL preview** per dialect in the sheet.
- **Refreshing an edited row** without rerunning the tab (after Set default or a delete), where the engine has `RETURNING`/`OUTPUT`.
- **A timeout for workflow nodes** once Core's `timeout` crosses the transports.
- **Still open from 5b:** hand-typed transactions on pooled connections, MySQL's `BEGIN` over the prepared protocol, BEGIN … END bodies splitting, the demo's `paginate` comment bug, reruns without parameter values, a wider destructive check, `SELECT … INTO` through `db.page`.
- **Phase 8** deletes `TsEditService` with `TsQueryRunner`, and with it the demo's gaps (no key check, a count before every page, inlined filter literals).
