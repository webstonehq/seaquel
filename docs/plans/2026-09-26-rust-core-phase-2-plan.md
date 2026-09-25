# Rust Core Phase 2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** Move MySQL/MariaDB, SQLite, MSSQL and DuckDB to Rust the way Postgres moved in phase 1, so every engine has one implementation. That covers dialect, introspection, EXPLAIN, statistics and exact values. Desktop and web use Rust for all five engines. Fix the correctness bugs the phase 2 research found along the way.

**Architecture:** Phase 1's machinery is reused unchanged:

- the `Dialect` trait and generic `ddl.rs`/`crud.rs`
- the `Driver` introspection methods and `seaquel-rpc`
- `EngineClient` and the `Value` wire format

Each engine gets a `dialect.rs` and an `introspect.rs`, plus native `Value` decoding and binding, then is added to `RUST_ENGINES` in `src/lib/engine/index.ts`. The TypeScript adapters for MySQL, SQLite and MSSQL are deleted at the end. **`duckdb.ts` stays, demo-only**: the browser demo runs DuckDB-WASM through `TsEngineClient` until the demo moves onto Core (design phase 8).

**Tech Stack:** Rust (sqlx 0.8 mysql/sqlite, tiberius 0.12, duckdb-rs), roxmltree or quick-xml for MSSQL plans, TypeScript/Svelte 5, vitest, the e2e Docker databases.

**Decisions (2026-09-26):**

1. Scope is the engines only. `seaquel-sql`, sqlparser-rs, `seaquel-wasm` and the editor/query-builder/tutorial parser switch become their own later phase (phase 2b in the design doc).
2. The demo keeps `duckdb.ts` as a demo-only dialect. It is excluded from deletion and from "fix everywhere" expectations; its bugs are listed as demo follow-ups.
3. Bug-fix rule, same as phase 1: fix anything that returns wrong data, silently drops a user's change, or generates invalid SQL. Each fix gets a test. Missing features, such as MSSQL statistics, are follow-ups, not phase 2 work, unless a task says otherwise.
4. MSSQL and DuckDB CRUD in Rust uses **parameterized** SQL (`@P{n}` / `?`), not TypeScript's inline literals, because the native drivers bind parameters. The inline builders are not ported. That removes the MSSQL `N'…'` and literal-escaping problems. Parity for their CRUD is checked with live tests instead of fixtures.

**Inputs:** the two phase 2 research reports are summarised per engine below. Phase 1's patterns and pitfalls are in `docs/plans/2026-09-25-rust-core-phase-1-plan.md` (read its ground rules, Value wire format and execution notes) and in the design doc's "Phase 1 cost" section.

---

## Ground rules for whoever executes this

- **No git writes.** The owner forbids `git add`/`commit`/`mv`/`stash`/branches/worktrees. Use plain `mv`/`cp`/`rm`. Each task ends with a checkpoint: summarise the changes and the verification, then let the user review. Read-only git is fine.
- Run everything from the repo root. Never edit `src/lib/components/ui/*`. Error toasts: `errorToast`. Run the Svelte MCP `svelte-autofixer` on every changed `.svelte` file. Run oxfmt on changed TS.
- Core crates may not use `tokio::spawn`, `Instant` or `SystemTime` (enforced by `crates/clippy.toml`). Engine crates are native-only, so `tokio::task::spawn_blocking` is allowed in `seaquel-engine-duckdb`.
- **Test databases.** `docker compose -f e2e/test-databases/docker-compose.yml up -d`, then `node e2e/test-databases/seed.mjs postgresql mysql mariadb sqlserver`. `SEAQUEL_TEST_*` variables hold ConnectConfig **JSON** (see `.github/workflows/ci.yml`; use 127.0.0.1). Clean up every scratch schema, database or table you create.
- **TDD everywhere.** Write the failing test first: a live engine test, parity test or vitest case.
- **Parity rule.** Rust output equals the recorded TypeScript output, except for the numbered bug fixes of that engine, each of which gets a documented exception and its own test.
- **User-facing strings.** Any new `messages/en.json` key must be translated with the `i18n-translator` agent before the checkpoint.
- **Full check list before every checkpoint:**
  - `npm run crates:check`
  - `cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`
  - `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, with all four `SEAQUEL_TEST_*` variables and `SEAQUEL_TEST_REQUIRE_ENGINES=1`
  - `cargo check -p seaquel`
  - `npm run check`
  - `npx vitest run`
  - `npx oxlint --type-aware --type-check --deny-warnings`
- **Effort log.** Every implementer appends one line per task to `docs/plans/2026-09-26-phase-2-effort.md`: task, wall time, Rust lines added, TS lines removed, surprises. Task 22 needs these numbers.

---

## Part A — Shared groundwork

### Task 1: Stop SQLite edits from corrupting values (live bug, fix first)

`castMapForColumns`/`buildCastMap` (`src/lib/hooks/database/query-crud.svelte.ts`) feed a cast map to the SQLite adapter, which wraps values in `CAST(? AS <declared type>)`. SQLite gives `DATETIME`, `DATE`, `BOOLEAN`, `JSON`, `UUID` and `NUMERIC(…)` numeric affinity:

- `CAST('2024-01-01 10:00' AS DATETIME)` stores `2024`.
- `CAST('{"a":1}' AS JSON)` stores `0`.
- The `"BLOB"` fallback type turns text into a blob.

**Fix:** only Postgres uses casts (MySQL, MSSQL and DuckDB already ignore them). `buildCastMap` returns `undefined` for every engine except `postgres`.

**Tests:**

- vitest: no casts for sqlite/mysql/mssql/duckdb.
- A live SQLite test through the Rust driver and the TS adapter's builder: update a `DATETIME` and a `JSON` column, and assert the stored text is unchanged.

**Checkpoint.**

### Task 2: A reusable parity-fixture recorder

Phase 1's recorder was Postgres-specific; a reference copy is in `docs/plans/artifacts/2026-09-25-postgres-fixture-recorder.ts.txt`. Write one recorder at `scripts/fixtures/dialect-fixtures.test.ts`, parameterized by engine. It writes to `crates/seaquel-engine-<engine>/tests/fixtures/`, and each engine adds its own cases in later tasks.

- **Parser inputs** must be exactly what the TypeScript parsers received at runtime. So the recorder runs each adapter's SQL **through Seaquel's own Rust driver**:
  - Start `seaquel-server` (`cargo run -p seaquel-server`, `BIND_ADDR=127.0.0.1:18788`).
  - Use `/api/db/connect` and `/api/db/query`, then `decodeRows`.
  - This is what `HttpProvider` does, so no per-engine Node driver is needed. Record parse inputs **before** that engine's native-value task changes the decoders.
- **Pure functions** (quoting, pagination, DDL, CRUD, column types) are recorded directly from the adapter.
- **Modes:** record mode with `RECORD_FIXTURES=1 ENGINE=<engine>`; check mode, with no database, replays the stored inputs through the adapter.
- Add `npm run fixtures:record -- <engine>`. Keep scratch-schema setup and teardown SQL per engine in a `bugfixes.json` `scratch` block, as phase 1 did.
- Hard-code the test database name `seaquel_test`; host, port and user may come from env.

The recorder is deleted with the last TS adapter it records (Task 21). Keep a `.txt` reference copy in `docs/plans/artifacts/` at that point.

**Checkpoint.**

### Task 3: Testkit for typed cells and introspection on every engine

In `crates/seaquel-engine-testkit`:

- **`run_typed_cells(engine, config, cases)`.** Each case is SQL literal (or table setup plus select) → expected `Value`. It also binds the decoded value back (`SELECT <p1> = <literal>`) and asserts equality, for engines whose `=` supports the type. Every engine's native-value task supplies a case table.
- **Generalize `run_introspection`.** `IntrospectionSpec` gains per-engine expectation flags: `explain_has_execution_time`, `stats_has_sizes`, `stats_has_connection_count`, `supports_schemas`, and whatever else the engines need. Postgres keeps its current assertions.

**Checkpoint.**

---

## Part B — MySQL / MariaDB

`mysql.ts` is 721 lines. One engine crate serves both; MariaDB connects with driver `"mysql"`. Run every live test against **both** servers.

### Task 4: Record MySQL/MariaDB fixtures

**Guidance from the Tasks 2–3 review:**

- EXPLAIN JSON v2 needs `SET GLOBAL explain_json_format_version=2`, then a **fresh** session from `ctx.openSession()`, because the MySQL driver is a pool. Restore the global in a `finally`.
- Record MariaDB's `ANALYZE FORMAT=JSON` through `parse.extra`.
- Use two corpora: `crateFixtureDir("mysql", "mysql")` and `crateFixtureDir("mysql", "mariadb")`.
- Rust harnesses read cells with `Value::from_wire` and column order from the stored `columns`.
- `run_introspection` expectations: `explain_has_execution_time: false` (TS returns none for MySQL ANALYZE). Check whether MariaDB JSON has a root cost before setting `explain_has_cost`. Run the spec once per server.

Cover quote, paginate, CRUD (`?` placeholders, backtick escaping), DDL create/alter (including `useModifyColumn`), column types, and parse inputs from both servers:

- schema, columns, indexes and stats
- EXPLAIN in `FORMAT=JSON` v1 (MySQL 8), v2 if the server supports it, and ANALYZE text
- MariaDB's JSON and ANALYZE

Write expected outputs for the bug fixes below into `bugfixes.json` by hand.

**Bug fixes** (MySQL):

1. **Bound catalog filters.** Replace `validateIdentifier` splicing with bound parameters, so names containing space, `-` or `.` load.
2. **Escaped identifiers everywhere.** Backticks doubled in the DDL quote.
3. **Correct `isForeignKey`.** Compute it from `KEY_COLUMN_USAGE.REFERENCED_TABLE_NAME` so it no longer requires `COLUMN_KEY='MUL'`; a PRI or UNI column that is also an FK now shows as one. Return the FK reference as separate columns instead of a `CONCAT` that breaks on dotted names.
4. **Correct text decoding.** Catalog names are decoded as UTF-8, replacing the Latin-1 `decodeValue`. `table_type` is decoded too.
5. **Robust EXPLAIN ANALYZE parsing.** The parser accepts fractional and scientific `rows=`, `(never executed)`, and quoted or temporary relation names.
6. **Valid `DROP INDEX`.** Emit `DROP INDEX \`i\` ON \`schema\`.\`table\``. A removed `PRIMARY`index becomes`ALTER TABLE … DROP PRIMARY KEY`.
7. **Default-only changes kept.** A default-only change is no longer dropped. _Revised in the Task 4–5 review:_ it emits `ALTER TABLE … ALTER COLUMN c SET DEFAULT <expr>` or `DROP DEFAULT`, which is valid on MySQL and MariaDB and leaves `ON UPDATE`, `COMMENT` and collation alone. `MODIFY COLUMN` fires only for a type or nullability change, as in the TS, and carries the default then. (MariaDB 11 itself drops a TIMESTAMP's `ON UPDATE` on `SET DEFAULT`; MySQL keeps it.)
8. **Defaults as SQL expressions.** _Decided in the Task 4–5 review, replacing "MariaDB's defaults in MySQL's form":_ `SchemaColumn.defaultValue` is the default as a valid SQL expression, as Postgres already reports it (`'active'::character varying`), because the table editor copies it into DDL verbatim (`create-table-tabs.svelte.ts`).
   - MySQL reports literal defaults unquoted. They're quoted, with `'` and `\` escaped, unless `EXTRA` has `DEFAULT_GENERATED` (expressions: `CURRENT_TIMESTAMP[(n)]` as is, others in parentheses) or the column is numeric or BIT (the value is already a literal). A NULL default is none; an empty string is `''`. This needs `EXTRA` in the columns query.
   - MariaDB already reports SQL (`'active'`, `current_timestamp()`); only its bare word `NULL` becomes no default.
9. **Index usage query** (found while recording). It read `TABLE_SCHEMA` from `mysql.innodb_index_stats`, which has `database_name`, so it failed on both servers and took the Statistics view down.
10. **JSON plans the TS showed as empty** (found while recording): MySQL's EXPLAIN JSON v2 plan at the root (the TS looked under `query_plan`) and a v1 `windowing` block.

**Checkpoint.**

### Task 5: MySQL dialect and introspection

Port the `Dialect` (parity), introspection and EXPLAIN (v1, v2, ANALYZE) into `crates/seaquel-engine-mysql/src/{dialect.rs,introspect.rs}`. Use the macro's `introspection = { … }` hook, as Postgres does. The `ddl.rs` options already cover `use_modify_column`; extend them for fixes 6 and 7.

- **MariaDB EXPLAIN.** If MariaDB's JSON has no MySQL cost fields, parse what it offers (rows, filtered, access type). Don't return empty numbers silently. Branch on the server version, which `SELECT VERSION()` reports at connect.
- **Statistics.** Keep the same queries. The `mysql.innodb_index_stats` read needs privileges: on a permission error, return an empty `indexUsage` instead of failing the whole Statistics tab.

Run `run_introspection` for both servers.

**Checkpoint.**

### Task 6: Native MySQL values

Rewrite `crates/seaquel-engine-mysql/src/decode.rs` to produce `Value` directly. Choose the type by `type_info().name()` and flags, not by `try_decode` fall-through that silently yields NULL.

| Type                        | Now                                                              | Target                                                                                                                                   |
| --------------------------- | ---------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| DECIMAL                     | Text; DECIMAL(65,x) → NULL                                       | `Decimal(exact text)` via the raw text; no 28-digit limit                                                                                |
| BIGINT UNSIGNED above i64   | Decimal                                                          | `Decimal` stays (wire shows digits), but `cell-type` should treat it as an integer: see note                                             |
| BIT                         | always NULL                                                      | `Int` for BIT(1..64), or `Bytes`; choose, document and test                                                                              |
| GEOMETRY                    | always NULL                                                      | `Bytes` (WKB)                                                                                                                            |
| BLOB / BINARY / VARBINARY   | int array, or Text if UTF-8                                      | always `Bytes`                                                                                                                           |
| JSON                        | object → Json, array → Array                                     | always `Json`, from the raw text, so big numbers keep full precision; MariaDB's LONGTEXT JSON stays Text unless the column is known JSON |
| FLOAT                       | f32 widened (0.10000000149…)                                     | `Float` from the f32's shortest decimal text                                                                                             |
| TIME                        | negative or >24h → NULL                                          | `Text` via `MySqlTime`                                                                                                                   |
| Zero dates `0000-00-00`     | NULL                                                             | `Text("0000-00-00…")`                                                                                                                    |
| DATE / DATETIME / TIMESTAMP | `time` Display (`12:34:56.0`, hour not zero-padded, `+00:00:00`) | **keep byte-identical** unless the server's own format is needed for round-trip; test that every decoded temporal value casts back       |

Note on TIMESTAMP (Task 6): values are UTC wall-clock time. sqlx sets the session `time_zone` to `'+00:00'` at connect, and the server converts TIMESTAMP to the session zone. Don't add `?timezone=` to connection strings: in a zone with DST an hour repeats each autumn, so two stored instants print the same and a TIMESTAMP key cast back matches the wrong row or two.

Note on unsigned BIGINT: decide whether u64 above i64 should be a new `Value` variant or stay `Decimal`, and make sure the UI formats it as an integer. The simplest route is `cell-type` treating an integral `SqlDecimal` as an integer. Record the decision.

Also:

- The binder: `Decimal` binds exactly, and falls back to text only when that's lossless.
- `run_typed_cells` case tables for both servers.

**Checkpoint.**

### Task 7: Switch MySQL/MariaDB to Rust

Add `mysql` and `mariadb` to `RUST_ENGINES`; MariaDB maps to engine id `mysql`. Then:

- Verify every `EngineClient` path through the automated end-to-end script pattern from phase 1 Task 12: esbuild-bundled real clients against `seaquel-server` and Docker MySQL **and** MariaDB, covering schema, metadata, stats, explain, CRUD round trips including a BIGINT UNSIGNED key and a DECIMAL(65,30) value, and create/alter.
- Fix `query-params.ts`, which sends `$N` placeholders to MySQL: MySQL needs `?`, and its `' || $N || '` concatenation means OR there.
- **From the Task 6 review:**
  - **Binary columns display consistently.** MySQL binary-collation text can't be told apart from VARBINARY on the wire: sqlx drops the collation id, and we chose not to fork sqlx. So the decoder returns `Text` for clean UTF-8 and `Bytes` otherwise. In the UI, a column declared `binary`/`varbinary`/`blob`/`*blob` renders **string** cells as their UTF-8 bytes (hex preview plus the correct byte length), like `Bytes` cells. Today `formatByteSize(String(v))` assumes base64 and shows e.g. "9 B" with no content.
  - **Binary edits write bytes.** `editable-cell.svelte` shows bytes as `\x…` hex, but saves the edited string as is, so MySQL stores the literal `\x…` text. When the original cell was `Uint8Array` (or the column is binary) and the input is `\x` + hex, send a `Uint8Array`. Test with MySQL VARBINARY and Postgres bytea.
  - **Timestamps are UTC.** The docs say TIMESTAMP values are shown as UTC wall-clock time.
- **Follow-up:** an upstream sqlx PR exposing `MySqlColumn` collation, so the text-or-bytes rule can become exact.

**Checkpoint.**

---

## Part C — SQLite

`sqlite.ts` is 604 lines. Its Rust driver sees runtime storage classes only. The declared type is available from the column's `type_info()`, if a hint is ever needed.

### Task 8: Record SQLite fixtures

**Guidance from the Tasks 2–3 review:**

- Use a temp-file database, not `:memory:`, because the pool would give each connection its own database.
- `pragma_index_info` cases go in `parse.extra` or `bugfixes.json`.
- Replay records only `parseExplainResult`. Check the ANALYZE root `actualRows`/`executionTime` separately in Task 9's live tests.
- `run_introspection`:
  - `supports_schemas: false`
  - `stats_has_table_sizes: false`
  - `overview_has_size: true`
  - `explain_has_cost: false`
  - `explain_has_actuals: false`
  - `explain_has_execution_time: true` once timing is in Rust
- The Rust port's `list_schemas` must return `main`.

Record the pure functions plus parse inputs for `sqlite_master`, `PRAGMA table_info`/`index_list`/`foreign_key_list`, `EXPLAIN QUERY PLAN` and stats.

**Bug fixes** (SQLite):

1. **Bound or escaped names.** PRAGMAs can't take bound parameters, so use `pragma_table_info(?)` table-valued functions, or escape correctly. No `validateIdentifier`.
2. **Escaped DDL quote.**
3. **Valid `CREATE INDEX`.** Emit `CREATE INDEX … ON "t"`; SQLite rejects a schema-qualified table there.
4. **No `ADD FOREIGN KEY` in ALTER.** Emit a clear comment line saying SQLite can't add an FK to an existing table instead of invalid SQL.
5. **Index columns load.** Read them with `pragma_index_info`.
6. **Honest ALTER.** `supportsAlterColumn: false` silently dropped type, nullability and default edits. The generated SQL must say so, with a comment line such as `-- SQLite can't alter column "x"; recreate the table`, instead of pretending nothing changed. Decide whether to also offer the table-rebuild recipe; recommended: comment only, rebuild is a follow-up.
7. **Unique constraints visible.** Autoindexes backing UNIQUE constraints are shown, marked unique, instead of hidden.
8. **Robust EXPLAIN QUERY PLAN parsing.** Quoted and non-ASCII names parse, and `SCAN CONSTANT ROW`, `BLOOM FILTER` and `LAST n TERMS` are handled.

**Checkpoint.**

### Task 9: SQLite dialect and introspection

Port into `crates/seaquel-engine-sqlite/src/{dialect.rs,introspect.rs}`. Move the per-table row counts and the EXPLAIN ANALYZE timing from `TsEngineClient` into Rust:

- `statistics` counts rows per table with quoted names.
- `explain(analyze)` runs the statement and times it in Rust. Use a monotonic clock that's allowed in engine crates: `std::time::Instant` is banned only for Core, so add an allow-comment in the SQLite crate or time with `tokio::time`.
- Keep the TS behaviour of executing the statement; document that ANALYZE executes, as it does in Postgres.

The custom `generateCreateTableSql` (inline PRIMARY KEY/AUTOINCREMENT) ports into the SQLite dialect.

**Checkpoint.**

### Task 10: Native SQLite values and the switch

- **Decoding:** `BLOB` becomes `Bytes`; INTEGER, REAL and TEXT stay as they are. Add a `run_typed_cells` table.
- **Switch:** add `sqlite` to `RUST_ENGINES`.
- **Check the tutorial.** It uses SQLite on Tauri via `tutorial/database.ts`; confirm it still works through the Rust engine.
- **End-to-end script:** schema, metadata, stats, explain, CRUD on a BLOB key, create/alter with the fix 6 comments.
- **Don't claim success for notes only (from the Tasks 8–9 review).** When every edit becomes a note (fixes 4, 6 and 7 turn edits SQLite can't make into `-- …` lines), the table editor (`create-table-tabs.svelte.ts` `executeCreate`) runs nothing and still toasts "updated successfully". It must show a warning that lists the notes instead; add `messages/en.json` keys if needed (translate them with the `i18n-translator` agent).
- **Fix Set to default on SQLite (from the Tasks 8–9 review).** `build_set_default` emits `UPDATE … SET "c" = DEFAULT`, which SQLite rejects: it has no `DEFAULT` in UPDATE. The dialect doesn't know the column's default expression, so the fix needs that information (e.g. from the metadata's `defaultValue`) or the UI must hide the action for SQLite.

**Checkpoint.**

---

## Part D — MSSQL

`mssql.ts` is 448 lines. The tiberius driver has a single `Mutex<Client>`. There are no statistics today; adding them is a follow-up, not in scope.

### Task 11: Positional cells and transactions in the MSSQL driver (driver bugs)

Note: `SET SHOWPLAN_XML ON` must be alone in its batch, and tiberius `Query` runs as `sp_executesql`. Session SET statements for EXPLAIN therefore need `simple_query` on the held client, not `Query`.

1. **Positional cells (correctness).** Read cells with `Row::cells()` instead of by column name. Today `SELECT a.id, b.id` and `SELECT 1, 2` repeat the first column's value. When a result has no rows, return its columns (today's list is empty).
2. **Transactions.** Implement `Driver::transaction` with `BEGIN TRAN` / `COMMIT` / `ROLLBACK` on the held client. Today it returns `TRANSACTION_NOT_SUPPORTED`, which breaks pending-changes commits on MSSQL. Flip `SmokeSpec::AT_P.supports_transactions` to true.
3. **Multiple result sets.** They are available internally, since EXPLAIN ANALYZE needs the second one. `query` still returns the first set.

Live tests first for each.

**Checkpoint.**

### Task 12: Record MSSQL fixtures

**Guidance from the Tasks 2–3 review:**

- ShowPlan XML can't be captured through `seaquel-server`, whose `query` returns only the first result set. Capture it in `parse.extra["parse-explain.json"]` with the Node `mssql` package, which the e2e seeder already uses:
  - SHOWPLAN in separate batches on one connection.
  - `recordsets[1]` for STATISTICS XML.
  - Store rows as `[{ "Microsoft SQL Server 2005 XML Showplan": "<xml…>" }]`.
- Record after Task 11's positional-cells fix.
- Typed-cells bind-back: `SELECT @P1 = x` isn't T-SQL. Use `.bind_back("SELECT CASE WHEN @P1 = <lit> THEN 1 ELSE 0 END")`.
- `run_introspection`: `supports_statistics: false`.

Record the pure functions (quote `[..]` with `]` escaping, `OFFSET/FETCH` pagination, DDL create and add column) and parse inputs (schema, columns, indexes, ShowPlan XML from both plain and analyze).

**Bug fixes** (MSSQL):

1. **Bound catalog filters.** Bind them (`@P1`); no `validateIdentifier`.
2. **Escaped DDL quote.** `]` becomes `]]`.
3. **View columns load.** Join `sys.objects`, not `sys.tables`.
4. **One row per column.** A column in two FKs no longer yields duplicate rows; aggregate, or pick one FK per column deterministically and document it.
5. **Index `unique`.** It is read correctly; BIT arrives as a bool, and `=== 1` was always false.
6. **Index columns exclude INCLUDE.** Filter out `is_included_column`.
7. **Valid T-SQL ALTER.**
   - `ALTER TABLE t ALTER COLUMN c <type> [NOT] NULL`.
   - Renames via `EXEC sp_rename 'schema.table.old', 'new', 'COLUMN'`.
   - `ADD` without `COLUMN`.
   - `DROP INDEX i ON schema.table`.
   - Default changes: T-SQL needs the default-constraint name, so either look it up in the generated SQL (`DECLARE @n sysname = (SELECT name FROM sys.default_constraints …); EXEC('ALTER TABLE … DROP CONSTRAINT ' + QUOTENAME(@n))`), or emit a comment saying the default can't be changed here. Recommended: the lookup.
   - Add the needed switches to `ddl.rs` `AlterTableOptions` rather than forking the generator.
8. **Pagination ignores nested ORDER BY.** When checking for ORDER BY, skip ORDER BY inside `OVER(...)`, subqueries and string literals. A small tokenizer is enough; this is not sqlparser.
9. **EXPLAIN works.**
   - Plain runs `SET SHOWPLAN_XML ON`, then the query, then `SET SHOWPLAN_XML OFF`, each as its own batch on the same held client (verify live that tiberius allows this).
   - Analyze runs `SET STATISTICS XML ON`, then the query, then OFF, and reads the plan from the **second** result set.
   - Parse the XML in Rust (roxmltree), the same way `convertMssqlRelOp` does.
   - _As recorded (Task 12):_ the plan is the result set whose column is `Microsoft SQL Server 2005 XML Showplan`. That's the second set for a SELECT, the only one for DML, and there is none for a trivial statement. The default lookup in fix 7 is `DECLARE @dfN nvarchar(max) = (SELECT N'ALTER TABLE … DROP CONSTRAINT ' + QUOTENAME(d.name) …); IF @dfN IS NOT NULL EXEC (@dfN)`, because `EXEC()` can't take a function call.

_Found while recording (Task 12), accepted as bug fixes:_

10. **Column types keep length, precision and scale.** Report `nvarchar(100)`, `nvarchar(max)`, `decimal(10,2)`, `varbinary(max)`, `datetime2(3)` and similar. For nchar/nvarchar, `max_length` is in bytes, so divide by 2. Fix 7 depends on this: T-SQL reads a bare `nvarchar` as `nvarchar(1)`, so a nullability-only edit from the table editor shrank the column.
11. **Plan names un-escape `]]`.** `stripBrackets` only dropped the outer brackets.
12. **Dropping a constraint's index.** DROP INDEX refuses a PRIMARY KEY or UNIQUE constraint's index. Emit `IF EXISTS (… sys.key_constraints …) ALTER TABLE … DROP CONSTRAINT … ELSE DROP INDEX … ON …` as one piece, with no `;\n` inside.
13. **Alter order and DROP COLUMN defaults.**
    - Drop indexes before the renames and column changes they would block. Make this an `AlterTableOptions` switch, so Postgres, MySQL and SQLite output stays identical.
    - Every DROP COLUMN first drops the column's default constraint, using the fix 7 lookup, in the same piece. The lookup is guarded by `IS NOT NULL`, so it doesn't depend on `defaultValue`.
14. **Pagination tolerates what trails the query.** Drop trailing whitespace, `;` and `--` or `/* */` comments before appending. Today a trailing `;` is a syntax error, and a trailing `--` comment comments out the pagination.
15. **ALTER COLUMN keeps a non-default collation.** Without a COLLATE clause, a nullability-only edit resets the column to the database default (Latin1_General_BIN became SQL_Latin1_General_CP1_CI_AS live). The columns query returns `collation_name` when it differs from `DATABASEPROPERTYEX(DB_NAME(), 'Collation')`. Add an optional `collation` to `SchemaColumn` and `CreateTableColumn` (Task 13); the table editor copies it across, and ALTER COLUMN emits `<type> COLLATE <name> NULL|NOT NULL`.

Also from the Task 12 review:

- ADD and CREATE TABLE state `NULL` or `NOT NULL` explicitly: in the ALTER generator, in `generateAddColumnSql` and in `generateCreateTableDdl`. Without it, nullability depends on the session's `ANSI_NULL_DFLT` settings; under `ANSI_NULL_DFLT_OFF ON` a bare column came out NOT NULL.
- `sql.json` is superseded by the `sql` and `explain-batches` cases in `bugfixes.json`.
- The case kinds and the dialect_parity/introspect_parity/smoke split are listed in `crates/seaquel-engine-mssql/tests/fixtures/README.md`.

**Checkpoint.**

### Task 13: MSSQL dialect and introspection

- **First (from the Tasks 8–9 review):** move the introspect helpers that the postgres, mysql and sqlite crates each copied (`Row` cell access by column name, `number_or_zero`, the `Ids` node-id factory, the `node()` ExplainPlanNode constructor) into `seaquel-engine`, and use them from all three before porting MSSQL.

Port into `crates/seaquel-engine-mssql/src/{dialect.rs,introspect.rs}`, with the introspection methods implemented directly on the driver (no macro). CRUD is parameterized (`@P{n}`), per decision 4. Live tests cover update, insert and delete with nvarchar non-Latin text, uniqueidentifier and datetime2 keys, and binary keys.

Statistics stay unsupported: `NOT_SUPPORTED`, and the UI already shows a fallback. Note it as a follow-up.

- **Driver fix (found in Task 12):** statements without parameters must run as a plain batch (`simple_query`), not through `sp_executesql`. Today every statement goes through `sp_executesql` with a parameter list, and there `CREATE VIEW`, `CREATE PROCEDURE`, `CREATE SCHEMA` and any other statement that must start a batch fail with "Incorrect syntax near the keyword …". Users can't create a view on MSSQL. Add a live test. The fixture scratch setup works around it with `EXEC (N'…')`, and can drop that once this is fixed.

**Checkpoint.**

### Task 14: Native MSSQL values and the switch

| Type                       | Now              | Target                                                                                                                  |
| -------------------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------- |
| decimal / numeric          | f64 (lossy)      | `Decimal(exact)` from `Numeric::value()` and `scale()`. Don't use its Display: −0.5 prints as `0.-5`                    |
| money / smallmoney         | f64              | `Decimal` with 4 decimal places. Tiberius already converts to f64, so exactness holds only to about 9·10¹¹; document it |
| binary / varbinary / image | base64 Text      | `Bytes`                                                                                                                 |
| real                       | f32 widened      | shortest f32 text as `Float`                                                                                            |
| datetimeoffset             | RFC3339 with `T` | keep, or align with datetime2's space form; choose, and test round-trip casts                                           |
| uniqueidentifier           | lowercase        | keep                                                                                                                    |

Binder:

- `Decimal` binds as `Numeric` (exact, scale < 38).
- `Null` binds without the nvarchar type problems: test against a varbinary column.

Then:

- Add a `run_typed_cells` table.
- Add `mssql` to `RUST_ENGINES`.
- Remove the MSSQL branches from `TsEngineClient.explain` bind-skipping (it only stays for the demo's DuckDB).
- Once the Rust MSSQL driver emits native `Bytes`, drop `base64` for mssql from `binaryStringEncoding` (`src/lib/utils/cell-type.ts`); MSSQL strings in binary columns are then text (`"text"`), and the `base64` encoding can go.
- Run the end-to-end script.

**Checkpoint.**

---

## Part E — DuckDB (desktop and web)

`duckdb.ts` is 525 lines and **stays for the demo**. `TsEngineClient` keeps serving DuckDB when `isDemo()`.

### Task 15: Unblock the async runtime and stop DuckDB panics

- **Blocking work off the runtime.**
  - Move every blocking DuckDB call (connect, prepare/query/row loop, execute, transaction) into `tokio::task::spawn_blocking`; add tokio `rt` as a dependency of the DuckDB crate.
  - Hold the connection as `Arc<Mutex<Connection>>`.
  - Map a `JoinError` panic to `DbError`.
- **Panics in duckdb-rs.** Guard the paths where duckdb-rs panics (`row.rs` `unimplemented!` for non-MonthDayNano intervals, `unreachable!` for unknown Arrow types): catch them, or avoid them by casting in SQL.
- **Streaming and cancel (optional, recommended).** Override `query_stream` with a blocking producer feeding a channel. Cancellation uses `conn.interrupt_handle()`.

**Checkpoint.**

### Task 16: Native DuckDB values

| Type                           | Now                                     | Target                                                                 |
| ------------------------------ | --------------------------------------- | ---------------------------------------------------------------------- |
| HUGEINT / UHUGEINT             | Text / Debug string                     | `Int` when it fits, else `Decimal(digits)`                             |
| DECIMAL                        | Text                                    | `Decimal(exact)`                                                       |
| BLOB                           | base64 Text                             | `Bytes`                                                                |
| DATE                           | **Int days since epoch**                | `Text` date, e.g. `2024-01-01`                                         |
| TIME / TIMESTAMP / TIMESTAMPTZ | Debug string                            | `Text` in DuckDB's own format (TIMESTAMPTZ as UTC with offset)         |
| INTERVAL                       | Debug string                            | `Text`, e.g. `1 month 2 days 00:00:01`                                 |
| LIST / ARRAY                   | Debug of the whole column chunk (O(n²)) | `Array` of element values via `ValueRef::to_owned()`                   |
| STRUCT / MAP                   | same                                    | `Json`                                                                 |
| ENUM                           | Debug                                   | `Text` label                                                           |
| UUID / JSON                    | Text                                    | keep Text; JSON columns may become `Json` if the logical type is known |
| FLOAT/DOUBLE NaN/inf           | Null                                    | `Float` (tagged on the wire)                                           |

Also:

- The binder: `Decimal` binds exactly where it fits width 38, otherwise as text.
- Add a `run_typed_cells` table.
- Performance check: a 100k-row result with a LIST column must be linear.
- Once the Rust DuckDB driver emits `Bytes` for BLOB, drop `base64` for duckdb from `binaryStringEncoding` (`src/lib/utils/cell-type.ts`). The demo's DuckDB-WASM already sends `Uint8Array`.

**Checkpoint.**

### Task 17: Record DuckDB fixtures, then its dialect and introspection

Note: record parse inputs **before** Task 16 changes the decoders if possible (swap the order of fixture recording and Task 16), or note that DATE/timestamp inputs differ from what TS saw. Copy `e2e/test-databases/duckdb/seaquel_test.duckdb` to a temp path before recording.

Record fixtures from **native** DuckDB. Also run the recorder's check mode against the demo's DuckDB-WASM version if practical, to note where the versions differ.

**Bug fixes** (desktop/web only; `duckdb.ts` in the demo keeps TS behaviour):

1. **Bound catalog filters; escaped DDL quote.**
2. **Indexes load** from `duckdb_indexes()`.
3. **Real database size** in the overview, via `pragma_database_size`, instead of "In-memory" with 0 bytes.
4. **No `ADD FOREIGN KEY` in ALTER.** Emit a comment line saying DuckDB can't add an FK to an existing table.
5. **Robust Estimated Cardinality parsing** (e.g. `~100`).
6. **Catalogs distinguished** in schema listing: schemas are no longer duplicated across attached catalogs. Decide how to present catalogs; minimal: exclude system and temp catalogs and qualify duplicates.
7. **Foreign-key query failures handled.** A failing FK query no longer fails the whole table metadata; `TsEngineClient` had no try/catch.

Found while recording (the fixtures README has the details and the live results):

8. **Qualified DROP INDEX.** `qualify_drop_index: true`; unqualified, DuckDB looks in `main` only.
9. **No NOT NULL in ADD COLUMN.** DuckDB rejects constraints there: add the column with its default, then `ALTER COLUMN … SET NOT NULL` as a separate statement. Without a default, SET NOT NULL fails on a table with rows, so the column is added nullable with a note.
10. **No statement DuckDB rejects.** `drop_indexes_first: true` (the MSSQL fix 13 switch) drops removed indexes first. While indexes the edit keeps remain, DuckDB refuses RENAME COLUMN, DROP COLUMN, TYPE and SET/DROP NOT NULL (even for an index on another column), and it refuses DROP COLUMN and TYPE on a PRIMARY KEY or UNIQUE column, and DROP COLUMN of any column before one (the constraint's ART index). Each such statement becomes a note carrying it, as the other engines do, and so do later statements on a column whose rename became a note. Everything else runs (e.g. fix 9's ADD COLUMN runs and its SET NOT NULL is a note). The rules read `isPrimaryKey`/`isUnique`, and edit mode sets `isUnique: false` today, so the port must report UNIQUE columns for them to apply. Kept indexes aren't dropped and recreated.
11. **FK REFERENCES in the table's own schema,** never `""`. DuckDB has no cross-schema foreign keys; in an attached catalog it can't create one from the default catalog at all, so there the foreign key is a note, as in fix 4.
12. **No bare ARRAY, LIST, MAP, STRUCT or UNION** in the column types; full types still go through the table editor's SQL pane.
13. **FILTER's `Expression`** is the node's filter (FILTER nodes only).
14. **`relationName` is the table's own name:** the last part of 1.5's `catalog.schema.table`, quote-aware.
15. **Overview name** from `current_database()`.

Then:

- **Qualified schemas outside introspection (fix 6).** With fix 6, tables in attached catalogs are listed under `catalog.schema`, in DuckDB identifier syntax: a catalog or schema part holding `.` or `"` is double-quoted (`"fx.we""ird".main`, or `"a.b"` for a default-catalog schema named `a.b`), so a listed schema is never ambiguous. Everything that builds `"schema"."table"` from the tree (the data tab's SELECT/COUNT in `data-tabs.svelte.ts`, CRUD, DDL, row counts) must produce `"catalog"."schema"."table"`, or attached tables can't be opened. Proposal: a `Dialect::quote_schema(schema)` (default `quote_ident`; DuckDB parses the listed schema quote-aware with fix 14's parser and quotes each part) that the `crud.rs`/`ddl.rs` builders use for the schema part, and an `EngineClient.qualifiedTable(schema, table)` mirrored locally per engine like `PAGINATE`, which `data-tabs.svelte.ts` uses instead of its own quoting (which also doesn't double `"`).
- CRUD is parameterized (`?`), per decision 4.
- Add `duckdb` to `RUST_ENGINES` for desktop and web only; the demo keeps TS.
- Run the end-to-end script.

**Checkpoint.**

---

## Part F — UI dialect snippets, cleanup, close-out

### Task 18: Dialect snippets outside the adapters

These produce invalid SQL today, and none needs a new endpoint:

- **Escaping.** `data-tabs.svelte.ts` `quoteIdentifier`, `components/sidebar/manage/schema-tab.svelte` DROP/TRUNCATE and `components/workflow/nodes/table-node.svelte` must escape the quote character. MySQL must use backticks, not `"…"`.
- **Type-parameter parsing.** `create-table-tabs.svelte.ts:94` breaks on `int(11) unsigned`, `decimal(10,2) zerofill`, enum values containing `)`/`,`, `STRUCT(...)`, `DECIMAL(10,2)[]`, and MSSQL types without a length (`nvarchar` becomes `nvarchar(1)`). Parse robustly: keep suffixes, anchor the regex, handle nested parentheses.
- **Pagination detection.** `/\bLIMIT\b/`-style detection in `query-execution.svelte.ts` (`shouldStream`, `hasPagination`) ignores matches inside string literals and quoted identifiers.
- **MSSQL row count.** The count wrapper strips a trailing ORDER BY for MSSQL. T-SQL rejects ORDER BY in a derived table without TOP/OFFSET.
- **Edits that match no row fail, on all engines** (moved here from the follow-ups; found in the Task 10 review, and in Task 13, where a base64 binary key matched nothing on MSSQL). A grid edit, set-to-default or delete by primary key that affects 0 rows must raise an error (`errorToast`, naming the table and key) instead of reporting success. The direct path (`query-crud.svelte.ts`) has `rowsAffected` from `execute`; check it there. The pending-changes path runs through `transaction()`, which returns no counts. Proposed design: add an optional `expectRows` to `BatchStatement` (`{ min: 1 }` for a keyed UPDATE or DELETE, absent for DDL and INSERT). Each driver's `transaction` compares every statement's affected rows with it before COMMIT and, on a shortfall, rolls back and fails with a new code (`NO_ROWS_AFFECTED`, with the statement's index), so the batch stays all-or-nothing and the UI can point at the pending change that went stale. The sqlx drivers and DuckDB have per-statement counts already; MSSQL sums its statement's DONE tokens (a trigger's rows count too, which only matters for `min`). Returning per-statement counts from `transaction` instead would come too late: the data is committed by then. **As built:** the pending-changes path does not use `transaction()`: `executeAll` runs the changes one by one through `execute` and checks each keyed change's `rowsAffected` there (a 0-row statement changed nothing, so nothing needs rolling back; the failed change and the ones after it stay pending). `expectRows`/`NO_ROWS_AFFECTED` is wire support, implemented and live-tested in all five drivers, with no UI caller yet. The index is in the message (`Statement 2 (index 1) …`). MSSQL connections send `SET NOCOUNT OFF` on open and reconnect, since a server `user options` NOCOUNT would make every edit report 0 rows.

- **Two-part names (from the Task 17 review).** DuckDB lists an attached catalog's schemas as `catalog.schema` (`"a.b"` or `"fx.we""ird".main` when a part holds `.` or `"`). Build names from a listed schema with `EngineClient.qualifiedTable` / `Dialect::quote_schema`, never by quoting the schema as one identifier:
  - Breaks for attached catalogs: `components/sidebar/manage/schema-tab.svelte:27-32` (`quoteId`), `:48` (DROP), `:88` (DELETE), `:89` (TRUNCATE); `components/workflow/nodes/table-node.svelte:37,40`; `components/command-palette.svelte:161` (`queryTable`).
  - Escaping only (no `"` doubling): `hooks/database/data-tabs.svelte.ts:456` (`quoteIdentifier`, still used for column names), `engine/qualified-table.ts:72-79` (`legacyQualifiedTable`, the other engines' table names).
  - Fragile: `monaco/completion-provider.ts:276,282` (inserts `schema.table` unquoted).
  - No schema at all: `hooks/query-builder-sql.ts:147`, `utils/export-formats.ts:67,75`.
- **Unchecking UNIQUE in edit mode does nothing.** `components/create-table-view.svelte:515` toggles `isUnique`, but `generate_alter_table_sql` never emits a UNIQUE change and no note says so. Emit the constraint change where the engine can, else a note.

**Checkpoint.**

### Task 19: Delete the MySQL, SQLite and MSSQL TypeScript adapters

- Remove `mysql.ts`, `sqlite.ts`, `mssql.ts` and their registry entries.
- `getAdapter` then serves only `duckdb` (demo).
- Remove helpers that no longer have callers. `alter-table.ts` and `crud-helpers.ts` stay only as far as `duckdb.ts` needs them; mark them demo-only in their headers.
- `TsEngineClient` keeps only what the DuckDB demo path uses.
- A test asserts `getEngineClient` never asks the TS side for mysql/mariadb/sqlite/mssql outside the demo.
- Freeze the fixtures with a README per engine, as Postgres has.

- Dedupe (from the Task 17 review): `DuckdbDialect::add_column` with ddl.rs's `generate_add_column_ddl` (a switch for fix 9's separate `SET NOT NULL`), and the per-engine `text`/`truthy` row-reading traits in each `introspect.rs` with the shared `Row`.

**Checkpoint.**

### Task 20: CI

- The engines job already runs the smoke tests, which now include typed cells and introspection for every engine. Make sure MariaDB and MSSQL introspection run too.
- Keep `types:gen` current.

**Checkpoint.**

### Task 21: Docs

- **CLAUDE.md:** all engines live in Rust; `duckdb.ts` is demo-only.
- **Design doc:** phase 2 status. The remaining original phase 2 items (`seaquel-sql`, `seaquel-wasm`, the parser switch) are renamed as their own phase.
- **Phase 2 plan:** Execution notes, in phase 1's format.
- **Recorder:** keep a reference copy in `docs/plans/artifacts/`, delete it from `scripts/`, and point the fixtures READMEs at the copy.

**Checkpoint.**

### Task 22: Measure and re-plan

- **Phase 2 cost section** in the design doc: time per engine vs the 16–22 h estimate, lines, bugs found per engine (and by fixtures vs live tests vs review), and what was harder than expected.
- **Estimate** for the parser phase, phase 2b (`seaquel-sql`, `seaquel-wasm` and the switch), and whether a spike should come first.

Final verification: the full check list, plus the manual GUI checks per engine (desktop and web; the demo for DuckDB), listed for the user.

**Status (Task 22):** done. The "Phase 2 cost" section of the design doc has the measured time (~39.8 h logged, ~30.3 h of it the four engines against the 16–22 h estimate), lines, bugs by source and the phase 2b estimate (~27–45 h, with a 4–6 h spike first). The full check list passed; the manual GUI checks are listed below under "Manual checks outstanding".

---

## Execution notes (2026-09-27)

The plan was executed task by task, with a review after each and usually a round of review fixes. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-09-26-phase-2-effort.md`. The measured cost and the estimate for the parser phase (phase 2b in the design doc) are Task 22's and go in the design doc.

**What went differently from the plan**

- **Parity fixtures caught no port bugs**, as in phase 1. In all four ports every recorded case matched unless a planned fix changed it (the only harness issue: MySQL EXPLAIN costs compare within 4 ULPs, because serde_json without `float_roundtrip` misreads a few JS floats). Recording against real servers was still worth it: it found most of the fixes numbered after each plan list (below). The rest came from live tests and reviews.
- **Order.** Task 3 ran before Task 2. DuckDB's fixtures (Task 17) were recorded after its native values (Task 16), not before. No recorded parse input depends on that, since every catalog cell the adapter read is VARCHAR, BOOLEAN or a BIGINT count; the DuckDB fixtures README has the check.
- **The demo wasn't pinned.** `duckdb.ts` stays for the demo (decision 2), with `alter-table.ts` and `crud-helpers.ts` cut down to what it uses and marked demo-only. `getAdapter` throws for every other engine.
- **Pending changes don't go through `transaction()`** (Task 18); see the decisions below.
- **MSSQL `{{params}}` are inlined, not bound.** Task 14 bound them as `@Pn`, and the review reverted that: binding broke `TOP {{n}}`, `CREATE VIEW`, defaults, a user's own `@p1` and varchar index seeks. Strings go in as `N'…'`, which is what fixed `東京` reading back as `??`. MySQL binds `?`; DuckDB inlines on desktop, web and the demo.
- **DuckDB's DDL switches** (`not_null_after_add_column`, `indexes_block_column_changes`, `constraints_block_column_drops`) and the schema quote went into a new `AlterTableRules` with `_with`/`_qs` builder variants, not `AlterTableOptions`, which every engine builds as an exhaustive `const`. UNIQUE flags travel on `SchemaColumn` (`isUnique`, `inUniqueConstraint`) for every engine, derived from the index metadata (DuckDB: from its constraints); a first version sent a separate `uniqueColumns` list for DuckDB only.
- **Shared introspection helpers.** The `Row` accessors, `number_or_zero`, the node-id factory and `node()` that the Postgres, MySQL and SQLite crates had each copied moved into `seaquel-engine` before the MSSQL port (Task 13), and `Row::text`/`truthy` and `generate_add_column_ddl` followed in Task 19.
- **SQLite additions in Task 10**: Set to default (fix 11, a new `Dialect::build_set_default_expr`), a warning instead of "updated successfully" when every edit is a note, and (in its review) a single long-lived connection for in-memory databases.
- **The recorder is gone.** `scripts/fixtures/` and `npm run fixtures:record` were deleted in Task 21. Reference copies are in `docs/plans/artifacts/2026-09-27-recorder-*.txt` and the corpora in `2026-09-27-<engine>-fixture-corpus.ts.txt`. All five engines' fixtures are frozen now.

**Bug fixes per engine.** Numbered as in each crate's `tests/fixtures/bugfixes.json`, whose `about` describes every fix:

- **MySQL/MariaDB, fixes 1–10.** 1–8 are the plan's list (7 and 8 reworked in review). Found while recording: 9, the index usage query read a column `mysql.innodb_index_stats` doesn't have and took the Statistics view down on both servers; 10, EXPLAIN JSON v2 plans (at the root) and v1 `windowing` blocks showed as empty.
- **SQLite, fixes 1–11.** 1–8 from the plan. Found while recording: 9, a foreign key without parent columns showed `referencedColumn: null`; 10, `NOT LIKE 'sqlite_%'` hid tables such as `sqlitex`. From Task 10: 11, Set to default (`SET c = DEFAULT` is a syntax error in SQLite).
- **MSSQL, fixes 1–15.** 1–9 from the plan. Found while recording and in review: 10, column types keep length and precision (a nullability-only edit turned `nvarchar(100)` into `nvarchar(1)`); 11, `]]` in plan names; 12, dropping a PRIMARY KEY or UNIQUE constraint's index; 13, drop indexes first and a column's default before DROP COLUMN; 14, pagination after a trailing `;` or comment; 15, ALTER COLUMN keeps a non-default collation.
- **DuckDB, fixes 1–15.** 1–7 from the plan. Found while recording: 8, qualified DROP INDEX; 9, no NOT NULL in ADD COLUMN; 10, statements DuckDB rejects while indexes or constraints remain become notes; 11, FK REFERENCES in the table's own schema; 12, no bare nested types in the type list; 13, FILTER's expression; 14, `relationName` without the catalog; 15, the overview's name.

Outside the numbered lists:

- Task 1's SQLite casts, which stored a DATETIME as `2024` and JSON as `0`.
- The MSSQL driver: `SELECT a.id, b.id` repeated the first column's value, there were no transactions (pending changes couldn't commit), and CREATE VIEW/PROCEDURE/SCHEMA failing inside `sp_executesql`.
- `query-params.ts` sent `$N` to MySQL.
- Edits that matched no row reported success on every engine (Task 18).
- Identifier quoting in the data tab, schema tab, workflow nodes, command palette and completions (Task 18).
- Decoder bugs:
  - MySQL: DECIMAL(65,30) rounded or NULL, TINYINT(1) holding 5 read as `true`, out-of-range TIME and zero dates read as NULL.
  - MSSQL: decimals were f64.
  - DuckDB: DATE came through as a day count, LIST/STRUCT as a Debug dump of the whole column chunk, and TIME_NS panicked.

**Decisions made during execution**

- **MySQL defaults are SQL expressions** (fix 8, Tasks 4–5 review). `SchemaColumn.defaultValue` is the default as a valid SQL expression on both servers, as Postgres already reported it, because the table editor copies it into DDL. MySQL literals are quoted unless `EXTRA` says `DEFAULT_GENERATED` or the column is numeric or BIT. MariaDB already reports SQL; only its bare `NULL` becomes "no default". A default-only edit is `ALTER COLUMN … SET DEFAULT`/`DROP DEFAULT` (fix 7), which leaves `ON UPDATE`, `COMMENT` and collation alone.
- **MySQL binary strings** decode as `Text` when they are clean UTF-8 and `Bytes` otherwise. sqlx drops the collation id, so `utf8mb4_bin` text and VARBINARY can't be told apart; the UI shows strings in binary columns as their UTF-8 bytes. BIGINT UNSIGNED above i64 stays `Decimal`, shown as an integer.
- **MSSQL batch policy, option (a)** (Task 13 review). Every statement goes through `sp_executesql`, with or without parameters, so a user's `SET` options and `#temp` tables end with the call and can't leak into introspection or grid edits. Only a parameterless statement that must start a batch (`must_start_batch`: CREATE VIEW, PROCEDURE, FUNCTION, TRIGGER, SCHEMA, …) and the driver's own BEGIN/COMMIT/ROLLBACK and EXPLAIN `SET`s run as plain batches. `USE` outlives an RPC call, so after any statement with the keyword `USE` outside strings and comments the driver runs `USE [<connection's database>]` (`restore_database`), and reconnects if that fails.
- **MSSQL poison-and-reconnect** (Task 11 reviews). The driver has one connection and a `dirty` flag, set before any I/O and cleared only once the response has been read to its end. A call that doesn't finish (a dropped future, `RESULT_TOO_LARGE`, a panic, a class 20+ error, a failed rollback) leaves it dirty. `Session`'s `Drop` closes a dirty connection at once, so the server rolls back and frees locks, and the next call reconnects. That loses session state (temp tables, a hand-opened transaction), which CLAUDE.md spells out. Before this, a caller dropped between BEGIN and its reply left tiberius and the server disagreeing about the transaction, and every later request failed with 3989.
- **MSSQL values.** A NULL parameter is sent as the literal `NULL`, since no declared type assigns to every column (checked against 21 types). Money is exact only for |value| < 2^39, because tiberius hands it over as an f64.
- **DuckDB decodes Arrow itself** (Task 16). The driver steps through each result's Arrow chunks and decodes cells from a type tree built once per statement, instead of using duckdb-rs's `Row`/`ValueRef`. That removed duckdb-rs's panics and the per-cell Debug dump: 100k rows with a LIST and a STRUCT column went from 1.86 s to 0.19 s. The connection turns on `arrow_lossless_conversion`, without which TIMETZ loses its offset and HUGEINT shares DECIMAL(38,0)'s carrier. Temporal values are DuckDB's own text (TIMESTAMPTZ in UTC); STRUCT and MAP are JSON with sorted keys. Every call runs in `spawn_blocking`, and dropping a call interrupts it.
- **DuckDB catalogs are quote-aware** (fix 6, Task 17 and its reviews). `system` and `temp` are left out. The default catalog's schemas are listed as they are and every attached catalog's as `catalog.schema`, with a part holding `.` or `"` double-quoted, so a listed schema is never ambiguous. Every catalog query filters on the same expression, the Rust builders quote with `Dialect::quote_schema`, and TypeScript builds names with `EngineClient.qualifiedTable`.
- **Pending changes stay one by one** (Task 18). `executeAll` runs each change through `execute` and checks a keyed change's `rowsAffected`. A statement that hit 0 rows changed nothing, so there's nothing to roll back: the failed change and the ones after it stay pending, and the failed one is highlighted. The plan's all-or-nothing batch wasn't needed.
- **`expectRows` is wire-only.** `BatchStatement.expectRows` and the `NO_ROWS_AFFECTED` error work in all five drivers (checked before COMMIT, rolled back on a shortfall, live-tested), but no UI code sends it yet. MSSQL connections send `SET NOCOUNT OFF` on open, since a server-wide NOCOUNT would make every edit report 0 rows.

**Manual checks outstanding.** None of these has been done by a person. Phase 1's list, in its own plan, was still open when phase 2 started.

- Every engine, on desktop (`npm run tauri:dev` with the Docker databases), and briefly on web (`npm run dev:web:full`):
  - the schema tree, column and index details, Statistics, EXPLAIN and EXPLAIN ANALYZE;
  - edit, insert and delete with pending changes on and off;
  - create a table, alter it, and toggle UNIQUE in edit mode;
  - delete a row from the query editor, then edit it in a stale grid: the edit fails with an error naming the table and key.
- MySQL and MariaDB:
  - edit a row keyed by BIGINT UNSIGNED `18446744073709551615`;
  - DECIMAL(65,30) shows every digit;
  - a VARBINARY cell shows hex and its size, and typing `\x00ff41` saves 3 bytes;
  - a default-only change in the table editor;
  - a query with a `{{param}}`.
- SQLite:
  - changing a column's type shows the notes warning, not "updated successfully";
  - Set to default on a column with a default;
  - edit and delete a row keyed by a BLOB;
  - the tutorial on desktop.
- MSSQL:
  - CREATE VIEW from the query editor;
  - EXPLAIN and EXPLAIN ANALYZE show a plan;
  - commit pending changes;
  - `SELECT a.id, b.id` shows both values;
  - a nullability-only edit keeps `nvarchar(100)` and a non-default collation;
  - edit a row keyed by varbinary;
  - cancel `WAITFOR DELAY '00:00:30'`, then run another query;
  - `{{p}}` set to `東京` comes back unchanged;
  - the Statistics tab shows its fallback.
- DuckDB:
  - `ATTACH` a second file, then open, edit and count its tables from the tree;
  - LIST, STRUCT and TIMESTAMPTZ columns display;
  - cancel a long query;
  - alter a table that has an index (notes).
  - In the demo (`npm run dev:demo`): schema, statistics, a grid edit and JSON export, unchanged from before.

---

## Follow-ups (not in phase 2)

**Next phase.** `seaquel-sql`, `seaquel-wasm` and the parser switch are phase 2b in the design doc.

**All engines**

- Changing an index's columns while keeping its name is dropped silently: `generate_alter_table_sql` in `ddl.rs` matches indexes by name only.
- Removing a UNIQUE constraint's index under Indexes emits `DROP INDEX`, which Postgres rejects for a constraint's index (the UNIQUE checkbox handles it). `ADD UNIQUE` on a MySQL TEXT/BLOB column or an MSSQL `(max)` column fails at run time; a note would be kinder.
- A queued Set to default freezes the default at queue time (SQLite sends the expression, not `DEFAULT`).
- Nothing in the UI sends `expectRows` yet. Writes that report 0 rows although they changed data: Postgres `DO INSTEAD` rules, SQLite `INSTEAD OF` triggers on views, MSSQL with `SET NOCOUNT ON` left on the session.
- FLOAT/REAL display differs: MySQL and MSSQL show an f32 as its shortest text (`0.1`), Postgres FLOAT4 and DuckDB FLOAT widen it (`0.10000000149011612`). DuckDB compares `k = ?` as DOUBLE, so it can't switch alone (`decode::widen`). Decide per engine, or keep the exact value on the wire and shorten it only for display.
- serde_json `preserve_order` workspace-wide would keep DuckDB STRUCT field order and Postgres `json` key order, but changes object iteration everywhere; check the MySQL EXPLAIN walk and the fixtures first.
- The visual query builder leaves column references, aliases and filter values unquoted, and its SQL editor parses in Postgres mode, so backticked or bracketed names fall back to custom SQL.

**MySQL/MariaDB**

- An upstream sqlx change exposing `MySqlColumn` collation would make the text-or-bytes rule exact.
- A functional index key part shows as `""`. MySQL reports a unique NOT NULL key as `PRI` on a table without a primary key.
- `EXPLAIN ANALYZE` names a filter node `Filter:` and drops the condition, as the TS did.

**SQLite**

- ALTER via table rebuild.
- Generated columns are missing: `pragma_table_info` hides them, `pragma_table_xinfo` has them.
- A WITHOUT ROWID table's primary-key index is in `table_metadata` but not in the statistics' index list.
- `ADD COLUMN … NOT NULL` without a default, or with a non-constant default, is emitted and SQLite rejects it; it should be a note.
- `statistics` runs every per-table `COUNT(*)` at once (`join_all`); bound it for databases with many tables.
- EXPLAIN ANALYZE buffers the whole result (and hits the row cap) just to count and time it; stream instead.
- A NULL cell in a BLOB column saves `\x…` input as text; only a cell that already holds bytes saves hex as bytes.

**MSSQL**

- Statistics are `NOT_SUPPORTED`; the tab shows a fallback.
- EXPLAIN names nodes by `LogicalOp`, so the physical operator (Nested Loops, Hash Match, Key Lookup) is lost, and `CompileTime`/`QueryTimeStats` aren't read, so planning and execution time are empty.
- Without a configured database the driver connects to, and restores `USE` to, `master` rather than the login's default. Leave it unset at connect and read `DB_NAME()` once.
- A `BEGIN TRAN` typed in the editor runs inside `sp_executesql`: it reports error 266 but leaves `@@TRANCOUNT` at 1, so later grid edits run inside it and `Driver::transaction` refuses to start. Decide whether hand-typed transaction control runs as a batch, and how the UI shows an open transaction.
- Azure SQL Database has no `USE`, so every statement mentioning it (including `OPTION (USE HINT …)`) would force a reconnect. Skip `restore_database` when `DB_NAME()` is unchanged, or on Azure.
- Money above 2^39 is inexact because tiberius converts it to f64; exactness needs a tiberius change.

**DuckDB**

- Statistics could take per-table sizes from `duckdb_tables().estimated_size` (and `index_count`) instead of `N/A` and a `COUNT(*)` per table.
- The default catalog is `current_database()`, which follows a `USE` typed in the editor, so the schema tree changes with it; consider pinning the file's catalog at connect time. A catalog attached under the name of a default-catalog schema makes `"schema"."table"` ambiguous, so generated SQL for default-catalog tables may need three-part names.

**Demo (`duckdb.ts`)**

- Data-tab filters send `$N` to DuckDB-WASM, which ignores parameters.
- Timestamp keys are formatted as epoch ms in the inline WHERE.
- The TS-side DuckDB bugs fixed in Rust by Task 17 remain.
- `DuckDBAdapter.quote` doesn't double `"` in DDL (the frozen fixtures record it).

**Build**

- `npm run build:web` runs out of Node's default heap; it needs `NODE_OPTIONS=--max-old-space-size=8192`, which the Dockerfile already sets.
