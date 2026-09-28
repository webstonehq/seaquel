# run fixtures

These files record what today's TypeScript query runner does when the editor runs SQL: which statements it finds, what it sends Core, what the result grid ends up holding, what goes to pending changes and what history records. Phase 5b moves that work into Core (`seaquel_workspace::run::plan`, `Workspace::run` and `Workspace::page`), and these cases pin it the way `../connect-config` pinned the connection builder. See `docs/plans/2026-10-02-rust-core-phase-5b-plan.md`, Task 2.

**The fixtures are frozen.** After phase 5b the GUI runs through Core, and the TS runner survives only in the demo (`TsQueryRunner`) until phase 8. Change a case only when Core is meant to behave differently, say why in `changes.json` and in "Changes" below, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-02-record-run-fixtures.test.ts.txt`, a vitest file. It ran on `cf2085d` plus the phase 5b working tree after Task 1 (`StatementResult.pageSource`, so paging re-runs the substituted SQL with its binds), with `src/lib` otherwise as it was. To rerun it, copy it to `src/lib/hooks/database/record-run.test.ts`, run it with `FREEZE_RUN=1`, and delete the copy. It needs a tree that still has the TS runner.

It runs the real code:

- `QueryExecutionManager` (`hooks/database/query-execution.svelte.ts`) and `resolve-query.ts`;
- `createExecution` (`components/query-editor/execution.svelte.ts`), the editor's Run and Run current buttons, so `execute`/`executeCurrent` are called exactly as the editor calls them: the destructive prompt first (the recorder confirms it), then the parameter dialog (answered with the case's values) when the text has `{{…}}`;
- every `$lib/sql` function, through the real seaquel-wasm;
- `CoreProvider` (`providers/core-provider.ts`), the provider on desktop and web, over a scripted `CoreClient`.

Stubbed or spied:

- **The `CoreClient`** answers each `db` call from the case's answers, in order, and the case fails on a call it didn't expect or an answer left over.
- **`paginate`** (`getEngineClient`) returns its arguments in a marker the client reads back, so `driver` records `paginate: {limit, offset}` and not the dialect's text.
- **`addToHistory`** and the pending-changes manager are spied. `QueryHistoryManager.addToHistory`'s row count is `affectedRows ?? totalRows`, and that is what `history.rowCount` holds.
- **Toasts** are captured.
- **What the runner computed** is read by wrapping, without changing, `countQuery`, `extractTableFromSelect`, `resolveColumnSources` and `filterAndIndexResults`.

The connection has no schema cache, so the grid's resolved `sourceTable`/`columnSources` aren't recorded. The raw references Core will send are (Decision 9).

Two runs gave byte-identical files.

## Files

| file                 | cases | covers |
| -------------------- | ----- | ------ |
| `plan-postgres.json` | 15    | run all with an error in the middle; `$n` binds, with `{{…}}` in a comment and a string; substitution errors at the cursor and in run all; a `$$` body; utility results hidden and all shown; destructive run all and at the cursor; duplicate column names; a rerun with raw `{{a}}`; a `WITH` returning rows; a `SET` with no columns (hidden); table and column refs |
| `plan-mysql.json`    | 7     | `?` binds with a backticked `;` and a `#` comment; writes with `lastInsertId`; a LIMIT stream; a backslash-escaped string and TRUNCATE; `SHOW TABLES` returning rows; SHOW/USE; `/*M! … */` as a plain comment (the contrast to MariaDB) |
| `plan-mariadb.json`  | 4     | `/*M! … ; … */` in run all and at the cursor (code, so it splits and its DELETE is destructive); a plain comment; `?` binds |
| `plan-sqlite.json`   | 4     | `$n` binds; PRAGMA and a write with `lastInsertId`; LIMIT/OFFSET streams; a trigger body (split apart: seaquel-sql's frozen `edge:begin-end`) |
| `plan-mssql.json`    | 7     | inlined `N'…'` values; `{{…}}` in a string, a bracketed name and comments; a substitution error on an inlining engine; TOP and OFFSET/FETCH stream; a bracketed name holding `;`; SET/EXEC utility |
| `plan-duckdb.json`   | 7     | inlined values; `{{…}}` in a string, a quoted name and a comment; SET as utility; a LIMIT stream; FROM-first returning rows; an attached catalog; page size 0 |
| `cursor.json`        | 15    | the cursor before, inside, at the `;` of, between and after statements; `東京` and `😀` before it; an offset only UTF-16 gets right (as bytes it lands in statement 1); the cursor between the halves of a surrogate pair; a whitespace-only buffer; a comment-only buffer at the cursor and in run all; a MySQL `#` comment holding `;` |
| `execute.json`       | 19    | a partial page (no count), a full page (count), counts as text and bigint, a count that isn't a number, a failed count (the estimate, on page 1 and 2), an empty page, a multi-batch stream, a stream failing after rows, a stream failing mid-run followed by a page and a write, a failed write and utility statement, a `WITH` then a SELECT, goToPage, setPageSize (to 500 and to 0), and Task 1's paging with bound, row-limited, per-statement and inlined parameters |
| `history.json`       | 10    | page 1 at the cursor (the text before substitution), page 2 (none), run all (the whole text, the first shown result's count), all utility, failed streams (none), a failed page in run all (recorded today) and at the cursor (none), write and stream row counts |
| `pending.json`       | 7     | pending changes on: an insert at the cursor, a SELECT at the cursor, run all with every statement type, run all with only writes, a substitution error in run all (an error result, not a pending change), a destructive statement that is deferred, inlined SQL Server values |
| **total**            | 95    |        |

Every `op` appears (`page` 83 times, `utility` 26, `stream` 16, `count` 14, `write` 12), across all six engine ids. Seven `execute` cases and one `history` case page afterwards.

## A case

```jsonc
{
  "name": "pg/run-all-select-insert-error",
  "engine": "postgres",           // the connection type: the SqlEngine Core scans with (MariaDB is "mariadb")
  "notes": "…",                   // optional
  "input": {                      // what db.run will get
    "text": "…",                  // the tab's whole text
    "target": { "type": "all" } | { "type": "current", "cursor": 27 }, // cursor: UTF-16
    "params": null | [{ "name": "a", "value": 5 }], // the dialog's values, cell wire format; null = the dialog didn't open
    "pageSize": 100,              // the tab's page size (0 = stream every SELECT)
    "deferWrites": false,         // pending changes on
    "confirmed": false,           // true when the destructive prompt was shown and confirmed
    "via": "editor" | "rerun"     // rerun: db.queries.execute(tabId), as the file drop and the grid reruns call it
  },
  "statements": [{ "index": 0, "sql": "SELECT 1 AS a" }], // run all: the split; current: the statement at the cursor ([] for none)
  "destructive": [{ "index": 0, "sql": "DROP TABLE t2", "reason": "drop_table" }], // what the prompt listed
  "driver": [                     // the db calls, in order
    { "op": "page", "sql": "SELECT 1 AS a", "paginate": { "limit": 101, "offset": 0 }, "params": [],
      "answer": { "columns": ["a"], "rows": [[1]] } },
    { "op": "write", "sql": "INSERT …", "params": [], "answer": { "rowsAffected": 1 } },
    { "op": "page", "sql": "SELECT nope", "paginate": { … }, "params": [],
      "answer": { "error": { "code": "QUERY_ERROR", "message": "…" } } }
  ],
  "results": [ { … } ],           // every statement's result, hidden ones included (below)
  "deferred": [{ "index": 0, "sql": "…", "source": { "sql": "…", "params": [] }, "queryType": "insert" }],
  "history": null | { "query": "…", "rowCount": 1 },
  "toasts": [{ "kind": "info" | "error", "message": "…" }],
  "pages": [                      // optional: paging afterwards
    { "action": { "type": "goToPage", "page": 2, "resultIndex": 0 } | { "type": "setPageSize", "pageSize": 500 },
      "driver": [ … ], "result": { … }, "history": null }
  ]
}
```

### `driver`

`op` says which `db` call today's GUI made and what for:

- `page`: `db.query` of `paginate(sql, limit, offset)`. `sql` is the statement before paginating. `limit` is the page size plus one. The Rust replay expands `paginate` with the connection's `Dialect::paginate`, and Core sends it through `query_stream` instead of `query` (Decision 5). The rows are the same.
- `count`: `db.query` of `seaquel_sql`'s count query, with the page's binds. It runs only when the page came back full. The total is read from the first cell of the first row.
- `stream`: `db.queryStream`. `answer` is `{columns, rows}` (one final batch), or `batches` (only the first carries `columns`), and optionally `error` after them.
- `write`: `db.execute` (`rowsAffected`, `lastInsertId`).
- `utility`: `db.query`, rows dropped.

`params` are the bind values in the cell wire format. An `answer.error` rejects the call with `CODE: message`.

### `results`

One entry per statement that produced a result, in order. For run all, that's before utility results are hidden.

- `index`: the statement's position in the run, 0 at the cursor.
- `shown`: false for a utility result hidden because another result shows.
- `sql`: the statement as typed (before substitution).
- `source`: `{sql, params}`, what ran after substitution; paging sends it back. `null` on an error result that didn't stream.
- `kind`: `page`, `stream`, `write` or `utility`, from the driver call it made. `null` for a planned failure (substitution in run all) that never reached the database.
- `queryType`: `null` on an error result that didn't stream.
- `columns`, `rows`, `rowCount`, `totalRows`, `page`, `pageSize`, `totalPages`, `error`, `affectedRows`, `lastInsertId`: `tab.results` as the grid holds it, without timings.
  - Columns are deduped (`id`, `id_2`), which is the GUI's job (Decision 5); the driver's names are in `driver`.
  - A write's grid row (`Result` / `N row(s) affected`, `pageSize` 1) and an error's (`Error` / the message) are the view's rendering. Core sends `rowsAffected` and `statementError {code, message}`; `error` here is `CODE: message`.
- `countEstimated` (page only): the page was full and the count failed, so `totalRows` is `offset + pageSize + 1`.
- `table`, `columnRefs`: `seaquel_sql`'s `table_from_select` and `column_refs` on the substituted SQL, present only where the runner computed them: a SELECT that paged successfully or streamed.

### Replay rules

The fields are the TS grid's, so a replay of Core's events has to map them:

- **`source` and `queryType` are `null` on an error result that didn't stream** (the TS made the error result without them). Core sends both in `statementStart` before the statement fails, so compare them only where the fixture has them.
- **`error` is the grid's text.** For a driver failure that's `CODE: message`. For a substitution failure in run all it's the bare message, with no `CODE: ` prefix, and `kind` is `null`. Core sends `statementError {code, message}` for both; compare the message, and the code only where the fixture has a prefix.
- **A missing `table`, `columnRefs` or `countEstimated` means "not compared".** Core sends `table`/`columnRefs` for every SELECT, including one whose page failed, and `countEstimated` for every page.
- **Columns are deduped in `results` and raw in `driver`.** Core sends the driver's names, and the GUI dedupes them (`dedupeColumnNames`).
- **A write's and an error's grid rows are the view's rendering** (above). Core sends `rowsAffected`, `lastInsertId` and the error; `pageSize` 1 on a write result is the view's too.
- **`totalRows`/`totalPages` are `null` where the TS computed `NaN`** (`exec/count-not-numeric`, fixed in `changes.json`).

### `deferred`, `history`, `toasts`

- **`deferred`**: the `pendingChanges.add` calls. `sql` is the statement as typed, and `source` is the substituted SQL and binds. Absent binds are recorded as `[]`, since pending changes send both as no parameters.
- **`history`**: the one `addToHistory` call, as `{query, rowCount}`; `null` for none. Only `db.run` records, so `pages[].history` is always `null`.
- **`toasts`**: what the user was told. The GUI keeps these, but they mark the run-level outcomes: `INVALID_PARAMETERS` at the cursor, nothing to run, and statements added to pending changes.

## What can't be recorded from the TS

- **`CONFIRM_REQUIRED`.** Today the editor prompts before calling the runner, and the file drop and grid reruns don't check at all. So a case records the prompt's list as `destructive` and runs confirmed (`input.confirmed`). The refusal itself (Core without `confirmed`) is Task 4's own test. `rerun` cases hold no destructive statement; their `destructive` is the same check run by the recorder.
- **Cancel, disconnect, a new run on the tab and timings.** These aren't in the cases. They are Task 4's and Task 6's tests.
- **`table`/`columnRefs` for a SELECT that failed before paging.** The runner computes them only after the page came back.

## `changes.json`

Each entry is the name of a case where Core is meant to differ, with the Decision (from the phase 5b plan) that makes it differ, why, and `expected`: the top-level fields of the case as Core should produce them, replacing the recorded ones. The Rust replay (Task 4) asserts that exactly these cases differ from the recording, and that they then match `expected`.

- **Decision 6, nothing to run (1 case):** `cursor/comment-only`. Today a comment-only buffer at the cursor runs whole as a utility statement and is recorded in history. Core runs nothing, and the GUI shows the no-statements toast.
- **Decision 5, an empty page carries its columns (1 case):** `exec/empty-page`.
- **Decision 5, a count that isn't a whole number (1 case):** `exec/count-not-numeric`. Today `parseInt` gives `NaN` for `totalRows` and `totalPages` (and history's row count). Core treats it as a failed count: `offset + pageSize + 1`, flagged `countEstimated` (Task 4's count rule).
- **Decision 11, no history when a statement fails (7 cases):** `history/failed-page-run-all-recorded`, `pg/run-all-select-insert-error`, `pg/rerun-raw-params`, `pg/substitution-error-run-all`, `pending/substitution-error-run-all`, `exec/write-error-continues` and `exec/utility-error`. Today run all skips history only after a failed or cancelled stream. A failed page, write, utility statement or substitution is still recorded. The plan named the failed page; the others are the same rule.
- **Decision 18, row-returning `other` statements show their rows (4 cases):** `pg/with-returns-rows`, `mysql/show-tables-returns-rows`, `duckdb/from-first-is-utility` and `exec/with-and-select`. Today their rows are dropped and the result is a utility result, hidden when anything else shows. In Core a result with at least one column carries its columns, rows and `rowCount`, is never hidden, and is what history counts. `pg/set-no-columns-hidden` pins the other half: no columns, still hidden, no change.

The Decision 5, 11 and 18 entries recompute `shown` and history's `rowCount` with the new rules, so an entry may change fields beyond the one its Decision names.

## Known quirks pinned here

These are today's behaviour, recorded on purpose and not changed in 5b:

- **`sqlite/trigger-body`** pins seaquel-sql's frozen `edge:begin-end` split: the `;` inside a trigger's `BEGIN … END` splits it, so the pieces fail on a real server. The same holds for MySQL and SQL Server bodies (Follow-ups in the plan).
- **`mariadb/executable-comment-*`**: `/*M! … ; … */` is code to MariaDB, so it splits into `SELECT 1 AS a /*M!` and `DELETE FROM t */`, which would be syntax errors on a real server. `mysql/mariadb-comment-is-a-comment` is the same text on MySQL, one statement with a comment.
- **Inlining engines replace `'{{a}}'` inside a string** (`mssql/params-in-comment-and-string`, `duckdb/params-in-comment-and-string`), while Postgres leaves it alone (`pg/params-bound-run-all`).

## Changes

None yet.
