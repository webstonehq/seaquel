# edit fixtures

These files record what today's TypeScript does with the grid's edits and the data tab: the SQL and binds each cell edit, Set default, insert and delete gets, what goes into pending changes and how it is described, what applying the queue runs and leaves behind, and what the data tab sends for a page and its count. Phase 5c moves that work into Core (`seaquel_workspace::edits`, `Workspace::plan_edits`, `apply_changes` and `table_page`), and these cases pin it the way `../run` pinned the query runner. See `docs/plans/2026-10-03-rust-core-phase-5c-plan.md`, Task 2.

**The fixtures are frozen.** After phase 5c the GUI edits through Core, and the TypeScript survives only in the demo (`TsEditService`) until phase 8. Change a case only when Core is meant to behave differently, say why in `changes.json` and in "Changes" below, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-03-record-edit-fixtures.test.ts.txt`, a vitest file. It ran on `cc08674` plus the phase 5c working tree after Task 1 (edits go to the result's or tab's own connection, `IN`/`NOT IN` bind one placeholder per item, the data tab's refresh sequence, no key values in the edit log lines). To rerun it, build the dialect helper whose source is in the recorder's header, copy the recorder to `src/lib/hooks/database/record-edits.test.ts`, run it with `FREEZE_EDITS=1 SEAQUEL_DIALECT_HELPER=<helper>`, and delete the copy. It needs a tree that still has the TypeScript edit path.

It runs the real code:

- `QueryExecutionManager` (`updateCell`, `setCellDefault`, `deleteRowAt`, `resolveEditTarget`) and `QueryCrudManager`: routing, the cast map, SQLite's default expression, the stale-key rule, queueing and dedupe;
- `DataTabManager` (`updateCell`, `setCellDefault`, `deleteRow`, `saveNewRow`, `refresh`);
- `PendingChangesManager` (`add`, `findForCell`, `update`, `executeAll`) and `describePendingChange`;
- `RustEngineClient` and `CoreProvider`, the desktop clients, over a scripted `CoreClient`;
- every `$lib/sql` function, through the real seaquel-wasm. A query tab's `sourceTable` and `columnSources` come from `extractTableFromSelect` and `columnRefs` on the case's query, resolved against its schema cache with `sourceTableFromRef` and `columnSourcesFromRefs`, as the run manager resolves Core's refs.

Stubbed or spied:

- **The `CoreClient`** answers `db.execute` from the case's answers, in order, `db.query` (the data tab) from the case's rows, and `db.engine tableMetadata` from the case's `metadata`. A case fails on a call it didn't expect or an answer left over.
- **`db.engine buildUpdate`/`buildSetDefault`/`buildInsert`/`buildDelete` are answered by the real Rust dialects**, through a small helper binary outside the repo that calls the same `Dialect` methods `seaquel_rpc::dispatch_on` does (`build_update`, `build_set_default_expr`, `build_insert`, `build_delete`). So the SQL and binds here are what the engine crates build, for whatever casts and default expression the TypeScript sent. The `crud.json` fixtures weren't used.
- **Toasts** are captured, the logger silenced, the pending-changes setting is a plain object, `crypto.randomUUID` is a counter, and `QueryHistoryManager.addToHistory` is spied: its row count is `affectedRows ?? totalRows`, as the real one computes it.
- **The sheet's rule** after `executeAll` (`pending-changes-sheet.svelte`): a full success clears the queue, so `queueAfter` is what the sheet leaves.
- **The query tab's rerun** after an immediate Set default (`QueryExecutionManager.execute`, a `db.run`) is counted, not run.
- **The sidebar's DROP and TRUNCATE text** is built the way `components/sidebar/manage/schema-tab.svelte` builds it (the component isn't rendered), then goes through the real `executeRawDdl`.

Two runs gave byte-identical files.

## Files

| file | cases | covers |
| --- | --- | --- |
| `plan-postgres.json` | 25 | update, set default, insert and delete, immediate and queued; casts on values and keys (numeric with typmod, jsonb NULL, an enum, `text[]`, `bpchar`, uuid, date, jsonb and numeric keys, a bigint key past 2^53); a composite key, and one whose key arrives in another order than the primary key; JSON cells holding an array, a string and a number, and typed JSON text; a table whose columns aren't cached (metadata loaded) and a failed metadata load; stale keys (update, set default, delete); a database error; the data tab replacing a repeated edit (and an edit then Set default of one cell); the query tab queueing it twice; an aliased key and a JOIN whose other table's key isn't in the result; a query tab Set default that reruns; `SELECT *` delete by display names; a key that isn't the primary key |
| `plan-mysql.json` | 7 | backticks and `?`; update and Set default queued; insert with `lastInsertId`; a `binary(16)` key; a stale key (MySQL counts matched rows); JSON cells holding an array, a string and a number, and typed JSON text |
| `plan-mariadb.json` | 3 | an unsigned bigint key past `i64` (it binds as a decimal, see quirks); insert and delete immediate; an aliased key in a query tab delete |
| `plan-sqlite.json` | 6 | Set default with the metadata's default expression (a literal, `CURRENT_TIMESTAMP`), a column without one (NULL), a stale cache default (the metadata's wins), a failed metadata load; immediate update, insert (`lastInsertId`) and delete; a NULL key |
| `plan-mssql.json` | 5 | `@P1…` binds; update and Set default (`DEFAULT`) queued; a composite key; insert with an identity id; a `datetimeoffset` key that matches nothing; an aliased composite key in a query tab delete |
| `plan-duckdb.json` | 3 | an attached catalog (`cat.main`, quoted as two names) through update, Set default, insert and delete; HUGEINT and STRUCT values; a query tab edit and a stale delete |
| `cast-map.json` | 10 | `castMapForColumns`: catalog cast types (user types, arrays, typmods, domains), text-like and user-defined columns left uncast, `bit`/`character` widened, plain types, case kept, odd names, an empty list |
| `apply.json` | 29 | every Decision 5 mode. `single`: an insert with `lastInsertId`, a stale key, an unconfirmed sidebar TRUNCATE. `atomic`: all succeed; a stale key first, in the middle and last; a database error in the middle (Postgres, SQL Server, DuckDB); a stale key in the middle on SQL Server and DuckDB; typed DML with 7 and 0 rows; typed binds; SQL Server and DuckDB edits; a SQLite truncate; an unconfirmed and a confirmed destructive DELETE; a key that isn't the primary key. `inOrder`: DDL in the middle with a stale key after it; sidebar DROP TABLE, DROP VIEW, DROP MATERIALIZED VIEW and TRUNCATE on Postgres, and TRUNCATE and DROP on MySQL, SQL Server and DuckDB (an attached catalog's table too); a typed MERGE; MySQL DDL mixed with inserts and a failing last insert; SQLite DDL failing first |
| `table-page-postgres.json` | 25 | no filters; every operator (`=`, `!=`, `>`, `<`, `>=`, `<=` in one case, `LIKE`, `NOT LIKE`, `IN` with spaces and a trailing comma, `NOT IN`, `IS NULL`, `IS NOT NULL`); `IN`/`NOT IN` numbering after other filters; AND and OR, with a disabled and a blank filter; sort ascending, descending, two columns; page 1 full, a middle page, the last partial page; a count that fails on a full and on a partial page; a count answered as a bigint; an empty page (columns from the cache); a page query that fails; an empty `IN` list (nothing sent); quoted names |
| `table-page-mysql.json` | 3 | `CHAR` casts and `?`; `IN` after `LIKE`; sort with a middle page; `NOT IN` with OR |
| `table-page-mariadb.json` | 2 | `IN` with sort; a full page |
| `table-page-sqlite.json` | 2 | `!=`, `IS NULL` and `NOT IN`; the last page sorted |
| `table-page-mssql.json` | 6 | `ORDER BY (SELECT NULL)` without a sort; a sorted middle page; `@pN` numbering through `IN`; `sql_variant` and `geography` columns listed and cast, on a partial and on a full page (so the count runs over the cast select); a full page with its count |
| `table-page-duckdb.json` | 2 | an attached catalog with filters and sort; a full page in the default catalog |
| `summary.json` | 52 | every branch of `describePendingChange` (insert, update with and without a column, delete, create table, create index and unique index, drop table, drop index, drop view, truncate with and without `TABLE`, alter table, each origin's fallback, the 80-character cut); the sidebar's DROP and TRUNCATE text on each engine; the names the regexes misread: backticks, brackets, three-part and quoted names, a doubled quote, spaces, a non-ASCII name, leading comments, `IF [NOT] EXISTS`, `ONLY`, a qualified SET column |
| **total** | **180** | |

## The files

### `plan-{engine}.json`

One case is a grid on one connection and some edits made on it, one after the other.

```jsonc
{
  "name": "pg/composite-key",
  "engine": "postgres",            // the connection type (MariaDB is "mariadb")
  "notes": "…",                    // optional
  "input": {
    "via": "dataTab" | "queryTab",
    "pending": true,               // the pending-changes setting
    "table": { "schema": "inventory", "table": "region_stock" },  // data tab: its table
    "query": "SELECT …",           // query tab: the SELECT the result came from
    "result": {                    // the grid: display columns, rows (cell wire format),
      "columns": [], "rows": [],   // and the routing the GUI computed
      "sourceTable": { "schema", "name", "primaryKeys" } | null,
      "columnSources": [ … ] | null
    },
    "schemaCache": [ SchemaTable ] // the connection's schema cache
  },
  "metadata": [ { "schema", "table", "columns", "indexes" } ], // the tables as the database has them
  "steps": [
    {
      "action": "updateCell" | "setDefault" | "deleteRow" | "insertRow",
      "row": 0, "column": "qty", "value": 40, "values": { … },   // as the grid sends them
      "edit": { "type": "updateCell", "target": { "schema", "table" }, "key": [["region", "eu"], ["sku", "A-1"]],
                "column": "qty", "value": 40 } | null,             // the Decision 1 intent; null when the GUI refused it
      "driver": [                  // the db calls this step made, in order
        { "op": "tableMetadata", "schema", "table", "answer": { "columns", "indexes" } | { "error" } },
        { "op": "build", "method": "buildUpdate", "params": { … as sent … }, "answer": { "sql", "bindValues" } },
        { "op": "execute", "sql", "params", "answer": { "rowsAffected", "lastInsertId"? } | { "error": { "code", "message" } } }
      ],
      "outcome": { "success": true, "queued": true } | { "success": false, "error": "…", "code": "NO_ROWS_AFFECTED" } | { "saved": true },
      "refreshed": true,           // a data tab reloaded its page after an edit that ran (not logged here)
      "reran": true                // a query tab reran after an immediate Set default (not run here)
    }
  ],
  "queue": [                       // pending changes after the last step
    { "id": "c1", "change": { "type": "edit", "id": "c1", "edit": { … } },   // what db.applyChanges takes back (Decision 2)
      "sql": "…", "params": [], "queryType": "update",
      "dml": true,                 // Decision 5's classification (derived: see below)
      "summary": { "verb": "update", "table": "region_stock", "column": "qty" } | null,  // Decision 12 (derived)
      "description": "Update region_stock.qty",
      "origin": "inline-edit", "target": { "schema", "table", "column", "primaryKeyValues", "newValue" } }
  ],
  "toasts": []
}
```

- **`edit`** is the intent the GUI will send, read off today's builder call: `target` from its schema and table, `key` as `[column, value]` pairs in the order the GUI's primary-key list has them (the schema cache's column order), `value`/`values` as sent. `insertRow`'s `values` keep the grid's column order. `sqlite/set-default-metadata-fails` failed on the metadata read before the builder ran; its `edit` is read off the step instead.
- **`build`** is the engine RPC call the GUI makes today. Its `params.casts` (Postgres only) and `params.column_default` (SQLite Set default) are what Decision 3 stops sending: Core reads them from `metadata`.
- **`outcome.code`** is the code of a failure: the `CODE: ` prefix of the error, or `NO_ROWS_AFFECTED` for the i18n "no row matched" text, which the GUI keeps formatting from the change's table and key (Decision 4).
- **`insertRow`** in a data tab is `saveNewRow`, whose outcome is `{saved}`. Its `lastInsertId` isn't shown by the data tab; the driver answer holds it.
- **`dml` and `summary` are derived, not recorded:** today's TS has neither. `dml` is Decision 5's rule applied to `change` (an intent other than `truncateTable`/`dropObject`, a SQLite `truncateTable`, or typed SQL whose `query_type` is insert, update or delete). `summary` is read back from `description` (and from `changes.json`'s fixed description where there is one): `Update t.c` is `{verb: "update", table: "t", column: "c"}`, `Insert row into t` is `insert`, `Delete row from t` `delete`, `Create table`/`Create index`/`Drop table`/`Drop index`/`Drop view`/`Truncate table`/`Alter table` `createTable`, `createIndex`, `dropTable`, `dropIndex`, `dropView`, `truncate`, `alterTable` (an index's name goes in `table`); a description that is the SQL itself or an origin fallback (`Update cell`, …) is `null`. The verb names are the recorder's; Task 4 may name them otherwise if it maps them one to one.

### `cast-map.json`

`{name, notes?, columns: SchemaColumn[], casts: {column: type}}`: `castMapForColumns(columns)`. Columns that need no cast are absent. Compare `casts` as a map.

### `apply.json`

One case is a queue on one connection and one apply of it.

```jsonc
{
  "name": "apply/pg-stale-middle",
  "engine": "postgres",
  "mode": "atomic",                // Decision 5's mode for this queue. Not recorded (the TS has none): the recorder checks it against the rule
  "input": { "confirmed": true },  // what the sheet sends; it always asks first, so true unless a case says
  "metadata": [ … ],               // as in the plan files
  "queue": [                       // in queue order, built by the real paths with pending changes on; fields as in the plan files
    { "id": "c1", "change": { "type": "edit", "id": "c1", "edit": { … } } | { "type": "sql", "id", "sql", "params" }, … }
  ],
  "driver": [ { "op": "execute", "sql", "params", "answer" } ],  // executeAll's calls
  "outcome": { "executed": 1, "failed": 1, "failedAt": 1, "failedChangeId": "c2", "error": "…", "code": "NO_ROWS_AFFECTED", "hasDdl": false },
  "queueAfter": ["c2", "c3"],      // the change ids left queued, after the sheet's rule (a full success clears the queue)
  "history": [ { "query": "…", "rowCount": 1 } ],  // addToHistory calls
  "toasts": []
}
```

- **The queue's items** come from grid edits (`QueryCrudManager` with pending changes on: `change.type` `edit`), from the editor (`pendingChanges.add` as `statementDeferred` calls it, with `detectQueryType`: `change.type` `sql`) and from the sidebar (the text `schema-tab.svelte` builds, through `executeRawDdl`; `change` is the Decision 11 intent, `truncateTable` or `dropObject` with `kind` `table`, `view` or `materializedView`).
- **`driver` answers are per change**, in queue order, as far as today's loop got. The replay's mock driver answers each change's statement with its answer, whether Core runs it through `execute` or inside `Driver::transaction`.

### `table-page-{engine}.json`

One case is a data tab's state and one `refresh`.

```jsonc
{
  "name": "tp/pg-in-with-spaces",
  "engine": "postgres",
  "input": {
    "tableQuery": { "target": { "schema", "table" }, "filters": [{ "column", "op", "value" }], "logic": "AND", "sort": [{ "column", "direction" }] },
    "page": 1, "pageSize": 100,    // what db.tablePage takes (Decision 9; TableQuery holds only enabled filters with a column)
    "filters": [ DataFilter ],     // the tab's filters as the UI holds them, disabled and blank ones included
    "schemaCache": [ … ],
    "columns": [], "matching": [], // the table: its columns and every row the filters match, in order
    "countCell": …, "countError": { … }, "pageError": { … }  // optional
  },
  "driver": [                      // today: the count, then the page, both db.query
    { "op": "count", "sql", "params", "answer" },
    { "op": "page", "sql", "params", "answer" }
  ],
  "select": { "sql": "SELECT * FROM … WHERE …", "params": [] }, // the page SQL before the paging it appended
  "paging": { "limit": 100, "offset": 0 },                    // today's paging: limit = pageSize, offset = (page - 1) * pageSize (the recorder checks both)
  "count": { "sql": "SELECT COUNT(*) FROM …", "params": [] } | null,
  "result": { "columns", "rows", "rowCount", "totalRows", "page", "pageSize", "totalPages", "error", "sourceTable" }
}
```

The mock answers a page with `matching.slice(offset, offset + limit)` and a count with `countCell` (else `matching.length`), so the replay can serve Core's `pageSize + 1` from the same rows.

### `summary.json`

`{name, engine, notes?, origin, sql, description, summary}`: `describePendingChange(sql, origin)`, and `summary` derived from it as in the plan files. `engine` is the connection the SQL was written for, which Core's `change_summary` scans with.

## Replay rules

Two replays read these files, and each compares its own fields. A field neither lists isn't compared. Where a case is in `changes.json`, its `expected` fields replace the recorded ones first. Timings are ignored everywhere.

### The Rust replay (`seaquel-workspace/tests/edits_plan.rs`, `seaquel-core/tests/edits.rs`)

- **Plan: `sql`, `params`, `queryType`, `dml` and `summary`** of each `PlannedChange` from `plan_edits`, for each step's `edit` (a `null` edit isn't sent):
  - `sql` and `params` equal the step's `build.answer` (`sql`, `bindValues`), and the queue entry's `sql`/`params`. Core builds them from `edit` and `metadata`, with Decision 3's cast map and default expression. Binds compare in the cell wire format.
  - `queryType`, `dml` and `summary` equal the queue entry's (for an immediate step, the fields the entry would have had: the same builder gives the same text).
  - The WHERE follows the key in the order `edit.key` gives it (`pg/key-order-differs-from-primary-key`); the key check compares the key's columns with the primary key's as a set.
  - `tableMetadata` calls aren't compared. Core reads the metadata once per table per call on every engine (Decision 3); today only Postgres (for casts, and only when the cache lacks the columns) and SQLite (Set default) do. Answer them from `metadata`. A scripted `tableMetadata` error applies to Core's read too.
- **Plan: the outcome** of an immediate step (`pending: false`, run as a `single` apply): `success` and `code`. An error's text only for a database error (`CODE: message`); the no-row-matched text is the GUI's.
- **Summary: `summary`** of `change_summary(sql, engine)` equals `summary.json`'s `summary` (after `changes.json`). The English description is not compared in Rust.
- **Cast map: `casts`**, as a map.
- **Apply: the outcome, mapped onto `ApplyOutcome`.** `executed` is `applied`; `failedAt` is `failed.index`; `failedChangeId` is `failed.id`; `code` is `failed.code`; `hasDdl` is `ddl`. `error` compares as in the plan files. A refusal before anything runs (`NOT_EDITABLE`, `INVALID_ARGUMENT`) is `Applied` with `applied: 0` and `failed: {index, id, code}`; `confirmRequired` is `ConfirmRequired`. `results` (per-change rows affected and `lastInsertId`) is new: for `single` and `inOrder` its entries match the `driver` answers of the changes that ran, and it is empty for `atomic`. The replay sends each queue entry's `change`, in order, with `input.confirmed`; the mock driver answers from `driver`.
- **Apply: history rows**: each row's `query` and `rowCount`, in order (ids, times and connection snapshots ignored). The context is the sheet's (history on).
- **Table page: the select.** The base SELECT and its binds equal `select` (the TS text before the ` LIMIT … OFFSET …`, or on SQL Server before ` [ORDER BY (SELECT NULL)] OFFSET … ROWS FETCH NEXT … ROWS ONLY`, which it appended itself).
- **Table page: the paging** is `{limit, offset}` expanded with the connection's `Dialect::paginate`: Core fetches `paging.limit + 1` rows at `paging.offset` (5b's page kind), and `paginate` adds SQL Server's `ORDER BY (SELECT NULL)` when the base has no ORDER BY.
- **Table page: the count** runs after the page and only when it came back full (Decision 9). Its SQL is `seaquel_sql::count_query(select.sql, engine)` with `select.params` (5b's count, which wraps the base: `SELECT COUNT(*) as total FROM (…) AS count_query`), not today's `SELECT COUNT(*) FROM <table><where>`. So Core's count compares with `count_query` of the case's `select`, never with `count.sql`. `count` records today's text, and whether a count runs: `null` in `changes.json` means none.
- **Table page: the result**: `columns`, `rows`, `rowCount`, `totalRows`, `page`, `pageSize`, `totalPages` and `error` (an error as present or not; its text only for a database error). `countEstimated` where `changes.json` gives it. `sourceTable` is the GUI's (the schema cache's primary key), not Core's.

### The TypeScript replay (Task 6's vitest, and `TsEditService` in the demo)

- **Plan: the queue**: each entry's `change`, `origin`, `target` and `description`, and the ids (`c1`, `c2`, … in order of first appearance, the recorder's names for the GUI's ids). The description is `describePendingChange` formatted from Core's `ChangeSummary` with the origin fallback and the 80-character cut, which stay in TypeScript (Decision 12).
- **Plan: dedupe**: a repeated edit of one cell leaves one entry, in the first one's place, holding the new change whole (its `change`, `origin`, `target`, SQL and binds).
- **Plan: the outcome** as the grid sees it: `success`, `queued`, the no-row-matched text for `NO_ROWS_AFFECTED`, and the GUI's own refusals (a `null` edit's error). `refreshed` and `reran`.
- **Apply: `queueAfter`**: cleared after a full success, whole after an atomic failure or a refusal, the applied prefix removed after an in-order failure.
- **Table page and summaries**: the demo's `TsEditService` replays the DuckDB cases through its own builders.
- **Apply queues' display fields** (`sql`, `description`, `summary`, `origin`) aren't compared by either replay; the plan and summary files pin them. The sidebar entries' `origin` is Decision 11's (callers pass it).

## What can't be recorded from the TS

- **Validation before execution, `CONFIRM_REQUIRED`, atomic rollback, history from Core.** Today nothing checks the key against the primary key, nothing asks for confirmation in the apply call (the sheet's dialog always asks and lists nothing), every apply runs in order, and history rows are written in TypeScript with `rowCount: 1`. These are `changes.json` entries.
- **Cancel, a new refresh on the tab, closing the tab, disconnects, limits and logs.** Task 4's and Task 6's own tests.
- **Core's metadata reads on engines that don't read today**, and the SQL Server metadata read for the tiberius casts: the TS reads the schema cache. The cases' `metadata` equals the cache unless a case says otherwise.

## `changes.json`

Each entry is the name of a case where Core is meant to differ, with the Decision (from the phase 5c plan) that makes it differ, why, and `expected`: the top-level fields of the case as Core should produce them, replacing the recorded ones. The replays assert that exactly these cases differ from the recording (under the rules above), and that they then match `expected`.

- **Decision 3, a failed metadata read fails the call (1 case):** `pg/insert-metadata-fails`. Today a Postgres insert whose metadata load fails is built without casts; Core fails the call with the read's code.
- **Decision 4, the key must be the primary key (2 cases):** `pg/key-not-primary-key` (a stale cache keys `users` by `email`: refused with `NOT_EDITABLE`, nothing runs) and `apply/pg-key-not-primary-key-refuses-batch` (the second of two changes: validation first, so nothing runs, `executed` 0, `failedAt` 1, and the queue stays whole).
- **Decision 5, a DML batch is one transaction (8 cases):** a stale key in the middle (`apply/pg-stale-middle`, `apply/mssql-stale-middle`, `apply/duckdb-stale-middle`) or last (`apply/pg-stale-last`), and a database error in the middle (`apply/pg-database-error-middle`, `apply/mssql-error-middle`, `apply/duckdb-error-middle`), apply nothing, keep the queue whole and record no history. `apply/sqlite-truncate-is-dml` is atomic with `ddl` false (a SQLite truncate is a `DELETE FROM` intent, Decision 11), and its history takes rows affected (Decision 8). A stale key first (`apply/pg-stale-first`) and a single change don't differ.
- **Decision 6, dedupe (3 cases):** `pg/query-tab-repeated-edit-queued-twice` (the query tab dedupes: one queued change holding the second value); `pg/data-tab-edit-then-set-default-replaced` and `mariadb/bigint-key-queued` (a replaced change takes the new change's origin, `set-default`; the MariaDB entry is also Decision 12).
- **Decision 7, confirmation on apply (2 cases):** `apply/pg-destructive-unconfirmed` (`DELETE FROM k`) and `apply/pg-sidebar-truncate-unconfirmed` (a queued sidebar TRUNCATE): `confirmRequired` listing it, nothing runs. `apply/pg-destructive-confirmed` runs.
- **Decision 8, history rows carry rows affected (10 cases):** `apply/pg-typed-dml-rows-affected`, `apply/pg-ddl-in-order-stops`, `apply/pg-truncate-and-drop-in-order`, `apply/pg-typed-merge-in-order`, `apply/mysql-ddl-mixed-in-order`, `apply/pg-destructive-confirmed` and the four sidebar DROP/TRUNCATE cases. Where every change affected one row, today's `rowCount: 1` is already right and the case isn't listed.
- **Decision 9, the table page (31 cases):**
  - every page that came back partial runs no count (`count: null`; the total is then `offset + rows`, which is what the count said anyway);
  - `tp/pg-page-query-fails` runs no count, since the page runs first;
  - `tp/pg-count-fails-full-page` shows an estimate (`offset + pageSize + 1`, `countEstimated`) instead of 0, and `tp/pg-count-fails-partial-page` shows the exact total;
  - the SQL Server and DuckDB cases with filters (`tp/mssql-filters-at-p`, `tp/mssql-sql-variant-columns-cast`, `tp/mssql-sql-variant-full-page`, `tp/duckdb-attached-catalog`) bind with the placeholders the engine's `crud.rs` builders use (`select.rs` is parameterized like `crud.rs`): `@P1…` on SQL Server where the data tab writes `@p1`, and `?` on DuckDB where it writes `$1`.
- **Decision 12, descriptions (32 cases: 10 plan, 22 summary):** the plan cases whose queued edits the regexes describe as the SQL itself (MySQL and MariaDB backticks, SQL Server brackets) or as table `main` (DuckDB's `"cat"."main"."t"`), and the `summary.json` cases for quoted, qualified and commented names, a non-ASCII name, `IF [NOT] EXISTS`, `ONLY`, and the sidebar's text on MySQL, SQL Server and DuckDB (the TRUNCATE fallback reads `TABLE` as the table). `change_summary` reads the statement with the scanner, so the table is the object's own name (the last part, unquoted) and the column is the SET target's own name. Each entry gives the fixed `description` and `summary`.
- **Decision 19, arrays, numbers and bools bind as JSON in a JSON column (2 cases):** `pg/json-top-level-values` and `mysql/json-top-level-values` (also Decision 12). The providers decode a JSON cell `{"$sq": "json", "v": [1, 2]}` to a plain array, and `encodeParam` sends an array as a SQL array (`Value::Array`) and a number as a number. So today the array binds as `bigint[]` on Postgres (and `CAST($1 AS jsonb)` fails) and is refused on MySQL ("array parameters are not supported"), and the number casts from `bigint` to `jsonb`, which Postgres has no cast for. The cases queue the edits, so nothing ran and nothing threw; `expected` binds the array and number values as `{"$sq": "json", "v": …}`. Core tags them from the column's type in the metadata. **Text stays text** (the `"abc"` steps keep their bind): the grid sends typed JSON as a string (`editedCellValue`), which the database parses, and `pg/json-typed-text` and `mysql/json-typed-text` pin that (the MySQL one is in `changes.json` for its description only).

Nothing else is meant to differ; a new difference found in Task 4 or 6 is a finding to report, not an entry to add.

## Known quirks pinned here

Today's behaviour, recorded on purpose and not changed by a Decision:

- **Ranges compare text** (`tp/pg-ranges-compare-text`): `CAST(col AS TEXT) > $1`. A follow-up types them.
- **A NULL key never matches** (`sqlite/null-key`): the builders write `key = ?`, so a SQLite table with a NULL in a non-INTEGER primary key can't be edited; the edit fails as stale.
- **An unsigned bigint key past `i64`** (`mariadb/bigint-key-queued`) comes back from the builder as a decimal bind (`Value::from_wire` has no `u64`); MySQL compares it numerically.
- **The sidebar's origins.** `executeRawDdl` guesses them from the text: a SQLite truncate (`DELETE FROM`) and a DROP VIEW or DROP MATERIALIZED VIEW get `query-editor`, and a DROP MATERIALIZED VIEW is described as its SQL (no branch reads it). Decision 11 has callers pass their origin.
- **A decoded JSON string can't be told apart from typed text** (`pg/json-top-level-values`' `"abc"` step, `pg/json-typed-text`): both reach the builder as a JS string and bind as text, which the database parses as JSON text. So a JSON cell whose value is the string `"abc"` is written back as `abc`, which isn't valid JSON. Decision 19 leaves text alone on purpose: tagging it would store every typed JSON edit as a JSON string.
- **An immediate edit in a data tab refreshes the page** (`refreshed`), and the refresh's count and page queries aren't in these cases' `driver`; the table-page files pin them.

## Changes

None yet.
