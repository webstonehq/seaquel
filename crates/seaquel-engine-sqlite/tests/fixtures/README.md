# SQLite dialect fixtures

> **Frozen.** These fixtures were recorded from the TypeScript `SqliteAdapter`
> (`src/lib/db/sqlite.ts`), which was deleted in phase 2 (Task 19). They can
> no longer be re-recorded or checked against it; the Rust parity tests still
> read every file. Change a fixture only when the Rust behaviour is meant to
> change, and say why in the same change.

They were recorded by the phase 2 recorder from a fresh temp-file database.
The corpus is kept at `docs/plans/artifacts/2026-09-27-sqlite-fixture-corpus.ts.txt`.

A reference copy of the recorder is in `docs/plans/artifacts/`, renamed to
`.txt` so no tooling runs it: `2026-09-27-recorder-dialect-fixtures.test.ts.txt`
is the vitest entry point, and the other `2026-09-27-recorder-*` files are its
helpers, npm wrapper and tsconfig.

Every pure group (quote, paginate, CRUD, DDL, column types, SQL text) and the
parse inputs: `sqlite_master`, `PRAGMA table_info`/`index_list`/
`foreign_key_list`, `EXPLAIN QUERY PLAN` (the SQLite bundled with sqlx) and the
statistics, including the per-table row counts TsEngineClient ran. Tables whose
names the adapter's `validateIdentifier` rejects (`my-table`, `order items`,
`it's`) can't go through its PRAGMA queries, so their expectations are
hand-written (bug fix 1).

Parse inputs are the rows the adapter's own SQL returned through Seaquel's Rust
driver, in the Value wire format, with `columns` in result order. Rust reads them
with `Value::from_wire` (`tests/introspect_parity.rs`); the pure groups are read
by `tests/dialect_parity.rs`.

`bugfixes.json` is written by hand: the scratch objects the recorder and
`tests/smoke.rs` build, and the expected output of each bug fix. Its `about`
lists the fixes (1–8 from the plan, 9 and 10 found while recording, 11 from Task 10:
Set to default assigns the column's default expression). A case with
`replaces` supersedes a recorded case; its `input` is null. The `columns`,
`indexes`, `schema` and `crud-set-default` cases are checked live by `tests/smoke.rs`.

Not replayed here: SQLite's EXPLAIN ANALYZE runs the statement and times it in
the driver (`tests/smoke.rs` checks the root's actual rows and the execution
time), since the recorded `EXPLAIN QUERY PLAN` is the same with or without it.
