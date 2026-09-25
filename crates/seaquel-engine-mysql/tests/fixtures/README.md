# MySQL / MariaDB dialect fixtures

> **Frozen.** These fixtures were recorded from the TypeScript `MysqlAdapter`
> (`src/lib/db/mysql.ts`), which was deleted in phase 2 (Task 19). They can
> no longer be re-recorded or checked against it; the Rust parity tests still
> read every file. Change a fixture only when the Rust behaviour is meant to
> change, and say why in the same change.

They were recorded by the phase 2 recorder with two corpora, `mysql` (MySQL
8.4 on 127.0.0.1:3306) and `mariadb` (MariaDB 11 on 127.0.0.1:3307). The
corpora are kept at `docs/plans/artifacts/2026-09-27-mysql-fixture-corpus.ts.txt`.

A reference copy of the recorder is in `docs/plans/artifacts/`, renamed to
`.txt` so no tooling runs it: `2026-09-27-recorder-dialect-fixtures.test.ts.txt`
is the vitest entry point, and the other `2026-09-27-recorder-*` files are its
helpers, npm wrapper and tsconfig.

- `mysql/` holds every pure group (quote, paginate, CRUD, DDL, column types,
  SQL text) and the parse inputs from MySQL 8.4, including EXPLAIN JSON v1,
  JSON v2 and `EXPLAIN ANALYZE` text.
- `mariadb/` holds the parse inputs from MariaDB 11, including
  `ANALYZE FORMAT=JSON` (MariaDB has no `EXPLAIN ANALYZE`).

Parse inputs are the rows the adapter's own SQL returned through Seaquel's Rust
driver, in the Value wire format, with `columns` in result order. Rust reads them
with `Value::from_wire` (`tests/introspect_parity.rs`); the pure groups are read
by `tests/dialect_parity.rs`.

`bugfixes.json` (one per server) is written by hand: the scratch objects the
recorder and `tests/smoke.rs` build, and the expected output of each bug fix.
Its `about` lists the fixes. A case with `replaces` supersedes a recorded case.
Re-recording changes timings and costs, so the `replaces` cases of fixes 5 and
10 must be regenerated and reviewed together with a re-recording.

Defaults (bug fix 8, decided in review): `SchemaColumn.defaultValue` is the
default as a SQL expression on both servers (`'active'`, `CURRENT_TIMESTAMP`,
`(json_array())`, `0.00`), because the table editor copies it into DDL. The
recorded MySQL `parse-columns` rows come from the TS query, which has no
`EXTRA` column, so they keep the TS values; `bugfixes.json` and `smoke.rs`
check the SQL form. Default-only edits are `ALTER COLUMN … SET DEFAULT` /
`DROP DEFAULT` (fix 7).
