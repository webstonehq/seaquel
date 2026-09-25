# MSSQL dialect fixtures

> **Frozen.** These fixtures were recorded from the TypeScript `MssqlAdapter`
> (`src/lib/db/mssql.ts`), which was deleted in phase 2 (Task 19). They can
> no longer be re-recorded or checked against it; the Rust parity tests still
> read every file. Change a fixture only when the Rust behaviour is meant to
> change, and say why in the same change.

They were recorded by the phase 2 recorder against SQL Server 2022 on
127.0.0.1:1433. The corpus is kept at
`docs/plans/artifacts/2026-09-27-mssql-fixture-corpus.ts.txt`, with its
ShowPlan capture (`2026-09-27-mssql-showplan-capture.ts.txt`) and the types it
used for the Node `mssql` package (`2026-09-27-mssql-showplan-types.d.ts.txt`).

A reference copy of the recorder is in `docs/plans/artifacts/`, renamed to
`.txt` so no tooling runs it: `2026-09-27-recorder-dialect-fixtures.test.ts.txt`
is the vitest entry point, and the other `2026-09-27-recorder-*` files are its
helpers, npm wrapper and tsconfig.

Recorded: the pure groups (quote, paginate, CREATE TABLE, ALTER TABLE, ADD
column, column types, SQL text) and the parse inputs for the schema, columns,
indexes and execution plans. The catalog rows are what the adapter's own SQL
returned through Seaquel's Rust driver, in the Value wire format, with
`columns` in result order; Rust reads them with `Value::from_wire`.

Not recorded:

- CRUD. The Rust dialect binds parameters (`@P1`, decision 4 of the phase 2
  plan) instead of the adapter's inline literals, so Task 13 checks it with
  live tests.
- Statistics. The adapter has none; `run_introspection` should say
  `supports_statistics: false` (Task 13).
- Tables whose names the adapter's `validateIdentifier` rejects (`fx order
items`, `fx]odd name`) and views (the adapter joined `sys.tables`). Their
  expectations are hand-written bug-fix cases (fixes 1 and 3), checked live.

## Execution plans

`parse-explain.json` can't come from the adapter's EXPLAIN, which is broken end
to end: it sends `SET SHOWPLAN_XML ON`, the query and `OFF` as one batch, which
SQL Server rejects, and the STATISTICS XML plan arrives after the query's own
rows, where `seaquel-server` stops reading. The plans are captured with the
Node `mssql` package instead (the ShowPlan capture above, run by the
recorder), on one connection:

- EXPLAIN: `SET SHOWPLAN_XML ON`, the query, `SET SHOWPLAN_XML OFF`, each its
  own batch.
- EXPLAIN ANALYZE: `SET STATISTICS XML ON`, the query (through
  `sp_executesql` when it has parameters), `SET STATISTICS XML OFF`. DML runs
  inside a transaction that is rolled back.

Each case stores the plan rows as
`[{ "Microsoft SQL Server 2005 XML Showplan": "<ShowPlanXML …>" }]`, or no rows
when the server returned no plan (`SELECT 1` under STATISTICS XML). The plan is
`recordsets[1]` for a SELECT and `recordsets[0]` for DML, which returns no
rows. The outputs are the TypeScript `parseExplainResult` (the browser's
`DOMParser`; jsdom's in the recorder) over that XML, which pins the parser the
Rust port writes with roxmltree.

The XML is normalized so that re-recording is byte-identical: compile and run
times, memory grants, cache flags, machine-dependent estimates and statistics
timestamps get fixed values, `WaitStats` is removed, auto-created statistics
names lose their object id, and `StatisticsInfo` elements are sorted.
`ActualElapsedms` becomes 1, 2, 3, 0, 1, … in document order, so both the
zero and the non-zero branch of `actualTotalTime` are covered. The parallel
plan is recorded without ANALYZE, since its per-thread row split changes
between runs. A few synthetic plans cover branches no captured plan reaches:
no plan rows, cells that aren't plans, a statement without a `RelOp`, and a
`RelOp` with only `PhysicalOp`, `EstimateRowSize` and several threads.

## For the port

`bugfixes.json` is written by hand. It holds the scratch objects and the
expected output of each bug fix; its `about` lists fixes 1–15. A case with
`replaces` supersedes a recorded case (the parity test should assert that the
recorded TypeScript output differs). Its `input` is null, so the recorded
input applies, except for fix 10, whose input holds the fixed query's rows.

The cases were written by a script, and a copy of it is in
[`docs/plans/artifacts/2026-09-27-mssql-alter-model.py.txt`](../../../../docs/plans/artifacts/2026-09-27-mssql-alter-model.py.txt).
It includes `create_sql` and `alter_sql`, models of the fixed CREATE TABLE and ALTER generators, and
`paginate_sql`, a model of the fixed pagination tokenizer. Every expected
ALTER statement was run against real tables inside rolled-back transactions.
The fix 10 and 15 cases also check that the column keeps its size and
collation. Every pagination output parses under `SET PARSEONLY ON`. The only
statements that fail are the ones where a UNIQUE constraint or a foreign key
still uses the column (error 4922), which the definition can't name.

**Regenerate the derived cases when you re-record.** The `replaces` cases for
fixes 5, 10 and 11 are derived from recorded outputs, and fix 10 also from
live rows. Re-recording changes plans (costs, row counts) and catalog rows, so
regenerate and review them in the same change.

### Case kinds and where the port checks them

Planned split for Task 13:

| kind                                              | input                       | checked by                                                                                                                                                                                                       |
| ------------------------------------------------- | --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ddl-create`, `ddl-alter`, `ddl-add-column`       | a definition, as recorded   | `tests/dialect_parity.rs`                                                                                                                                                                                        |
| `paginate`                                        | `{ sql, limit, offset }`    | `tests/dialect_parity.rs`, and the local TS `PAGINATE` copy                                                                                                                                                      |
| `sql`                                             | `null` (replaces sql.json)  | `tests/dialect_parity.rs`: the catalog query text                                                                                                                                                                |
| `explain-batches`                                 | `null` (replaces sql.json)  | the driver's EXPLAIN (unit or live test): the batches it runs                                                                                                                                                    |
| `parse-columns`, `parse-indexes`, `parse-explain` | rows                        | `tests/introspect_parity.rs`                                                                                                                                                                                     |
| `parse-explain-error`                             | malformed plan rows         | `tests/introspect_parity.rs`: only `nodeType`, no children and a non-empty `filter` (the parser's message isn't pinned)                                                                                          |
| `columns`, `indexes`                              | `{ schema, table }`         | `tests/smoke.rs`, live on the scratch objects                                                                                                                                                                    |
| `explain`                                         | `{ sql, analyze, params? }` | `tests/smoke.rs`, live. The plan is reduced to `nodeType`, `relationName`, `indexName` and `filter`, plus `actualRows` and `actualLoops` with analyze. Run the UPDATE case in a transaction that is rolled back. |

`sql.json` is superseded. Its `getColumnsQuery` and `getIndexesQuery` cases
are replaced by the fixed catalog queries. The `sql` cases hold their exact
text, with `@P1` as the table and `@P2` as the schema; those queries produced
the fix 10 input rows. Its `getExplainQuery` cases are replaced by
`explain-batches` cases. `getSchemaQuery` and `getSchemasQuery` are unchanged.

### Notes for Tasks 13 and 14

- Fix 15 needed a new optional field: Task 13 added `collation?: string` to
  `SchemaColumn` (seaquel-types) and `CreateTableColumn`. It is set only when
  the column's collation differs from the database default (the columns
  query compares both sides `COLLATE DATABASE_DEFAULT`, so the comparison
  can't hit a collation conflict). The table editor
  (`create-table-tabs.svelte.ts` `addFromTable`) copies it across, and ALTER
  COLUMN, CREATE TABLE and ADD emit ` COLLATE <name>` after the type, except
  for built-in types without a collation.
- EXPLAIN must turn `SET SHOWPLAN_XML OFF` / `SET STATISTICS XML OFF` back off
  even when the query fails or the call is cancelled, or the next statement on
  the held connection returns a plan instead of rows. Task 11's
  `Session::hold_state()` / `release_state()` covers this: a session that is
  dropped while holding state is closed and reconnects. Run the SET batches
  under it.
- The scratch setup ran `CREATE SCHEMA` and `CREATE VIEW` as `EXEC (N'…')`,
  because the Rust driver sent every statement through `sp_executesql` with
  a parameter list, where a statement that must start a batch is a syntax
  error. Task 13 sends such statements as a plain batch and dropped the
  workaround.
- Typed-cells bind-back (Task 14): `SELECT @P1 = x` isn't T-SQL, so use
  `.bind_back("SELECT CASE WHEN @P1 = <lit> THEN 1 ELSE 0 END")`.
- `run_introspection` (Task 13): `supports_statistics: false`.
