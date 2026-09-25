# DuckDB dialect fixtures

> **Frozen.** These fixtures were recorded from the TypeScript `DuckDBAdapter`
> (`src/lib/db/duckdb.ts`). That file stays for the browser demo, but the
> recorder was deleted in phase 2 (Task 21), so the fixtures can no longer be
> re-recorded or checked against it; the Rust parity tests still read every
> file. Change a fixture only when the Rust behaviour is meant to change, and
> say why in the same change.

They were recorded by the phase 2 recorder against a temp copy of
`e2e/test-databases/duckdb/seaquel_test.duckdb` (written by
`npm run e2e:db:seed`), with no server of its own. The corpus is kept at
`docs/plans/artifacts/2026-09-27-duckdb-fixture-corpus.ts.txt`.

A reference copy of the recorder is in `docs/plans/artifacts/`, renamed to
`.txt` so no tooling runs it: `2026-09-27-recorder-dialect-fixtures.test.ts.txt`
is the vitest entry point, and the other `2026-09-27-recorder-*` files are its
helpers, npm wrapper and tsconfig.

The recorder copied `seaquel_test.duckdb` (and its WAL) into a fresh temp
directory and never opened the original. The copy keeps its file name because
DuckDB names the catalog after the file name up to its first dot. The plans' `Table` values
(`seaquel_test.main.orders`) and the scratch setup's `USE seaquel_test` depend
on that name. The Rust tests must copy it under the same name too.

Everything was recorded from **native** DuckDB 1.5.5 (`duckdb` crate
1.10505), through `seaquel-server` and the Rust driver as it decodes values
today. That is the decoding the desktop TypeScript path saw.

## What is recorded

- Pure groups: quote, paginate, CREATE TABLE, ALTER TABLE, ADD COLUMN, column
  types and the SQL text of every catalog query.
- Parse inputs: the schema listing, columns and foreign keys (seed tables, a
  table of every DuckDB type, a non-main schema with a composite primary and
  foreign key, a view), indexes, `EXPLAIN (FORMAT JSON)` and
  `EXPLAIN (ANALYZE, FORMAT JSON)` plans, and the statistics (table sizes,
  index usage, overview, row counts).

The rows are what the adapter's own SQL returned, in the Value wire format,
with `columns` in result order. Rust reads them with `Value::from_wire`.

Not recorded:

- CRUD. The Rust dialect binds parameters (`?`, decision 4 of the phase 2
  plan) instead of the adapter's inline literals, so the port checks it with
  live tests.
- Tables whose names the adapter's `validateIdentifier` rejects
  (`fx_sales."fx order lines"`, `main."fx it's"`) and `main.categories`, whose
  columns query fails once `fx_aux` is attached. Their expectations are
  hand-written bug-fix cases, checked live.
- Row counts of `fx_aux`'s tables. The adapter lists them under `main` and
  `fx_sales`, so it counts the `seaquel_test` table of the same name, or
  fails (`main.fx_items`, which TsEngineClient left uncounted). The corpus
  records one count per table name through `parse.extra`.

### Scratch objects

These are the `scratch` block of `bugfixes.json`, run on the copy:

- a `fx_sales` schema with an expression index, a unique two-column index and
  a view;
- `main.fx_types`, with one column of every DuckDB type and typed defaults;
- tables with spaces and quotes in their names;
- an attached in-memory catalog `fx_aux`. Its `main.users`,
  `main.categories` and `fx_sales.regions` share names with `seaquel_test`'s
  tables. It also has a foreign key, created after `USE fx_aux`, since DuckDB
  rejects a qualified `REFERENCES` across catalogs;
- schema and catalog names with dots and quotes: a default-catalog schema
  `"fx.a.b"`, a default-catalog schema literally named `fx_aux.main` with its
  own `users` table, and a second attached catalog `"fx.we""ird"` with an
  indexed table;
- a temp table.

`fx_aux`, `"fx.we""ird"` and the temp table exist only on the connection that ran the setup.
DuckDB rejects cross-schema and cross-catalog foreign keys, so every foreign
key stays in its own schema.

### EXPLAIN

The adapter asks for `FORMAT JSON`. Plain plans are recorded as returned.
Analyzed plans carry timings, CPU, memory and byte counters that change on
every run, and DuckDB writes a node's keys in no fixed order. The corpus
normalizes them (`normalizeAnalyzed`) so that re-recording is byte-identical:

- `operator_timing` takes 0.0005, 0.00025, 0, 0.001, … in document order,
  which also covers the zero-timing path;
- the root's `latency` becomes 0.0125, which is 12.5 ms of execution time;
- the other timing, CPU, memory and byte figures become 0;
- keys are sorted.

Row counts and cardinalities are kept. DML is recorded without ANALYZE,
because ANALYZE runs the statement.

DuckDB 1.5.5 never picked an index scan on these small tables, so synthetic
plans (`synthetic: …`) cover the branches the captured plans don't reach:
index-scan conditions, `HASH JOIN` spelled with a space, conditions on other
join types, filter lists, nameless nodes, empty and non-JSON plans, analyzed
plans with and without the `EXPLAIN_ANALYZE` wrapper, and the Estimated
Cardinality forms that fix 5 is about.

## Value decoding and Task 16

Task 16 changes how the Rust driver decodes DATE, TIME, TIMESTAMP, INTERVAL,
LIST, STRUCT, MAP, ENUM, HUGEINT and DECIMAL values. **None of the recorded
parse inputs depends on that.** Every cell the adapter's catalog queries
return is VARCHAR, BOOLEAN or a BIGINT count:

- The type names and defaults in `information_schema.columns` are text. For
  example, `main.fx_types` records `INTEGER[3]`,
  `STRUCT("name" VARCHAR, age INTEGER)`, `ENUM('sad', 'ok', 'happy')`,
  `CAST('2024-01-01' AS "DATE")` and `to_days(CAST(trunc(CAST(1 AS DOUBLE)) AS INTEGER))`.
- The primary-key flag is computed in SQL (`list_contains(constraint_column_names, …)`),
  and the foreign-key columns are unnested to VARCHAR in SQL.
- The plans are VARCHAR JSON text.
- The counts are `count(*)` (BIGINT), and the overview's size in the TypeScript
  query is the literal `0`.

A check over the recorded files finds no `$sq` tag and no debug rendering
(`List(…)`, `Timestamp(…)`, `Interval {…}`).

The port has to keep it that way. Its introspection queries run on the new
decoding, so any catalog column that isn't plain text or a number changes
shape under Task 16. **Flagged for the Task 17 port:**

- `duckdb_constraints().constraint_column_names` and `referenced_column_names`
  are `VARCHAR[]`. Today they arrive as a debug dump of the whole column
  chunk, not the row's list. After Task 16 they arrive as an `Array`. Either
  unnest or `list_contains` them in SQL, as the fixed queries do, or cast
  them to VARCHAR. Don't read them as a list cell.
- `duckdb_indexes().expressions` is VARCHAR in 1.5.5 (`[order_no, region_code]`,
  or with quoted elements `['"line no"', '(lower("it\'s"))']`). The fixed
  query casts it to `VARCHAR[]` and unnests it, so the rows stay VARCHAR
  whatever its type is in a later DuckDB.
- `duckdb_indexes().tags` and `duckdb_constraints().constraint_column_indexes`
  are a MAP and a LIST. Never select them.
- `sum(...)` of BIGINTs is HUGEINT. Today that is a JSON string and after
  Task 16 an `Int`. The fixed overview casts its sum to BIGINT.
- `pragma_database_size()`'s `database_size`, `wal_size` and `memory_*` are
  formatted text (`5.7 MiB`). The fixed overview computes bytes from
  `block_size * total_blocks` (BIGINTs).

## For the port

`bugfixes.json` is written by a generator, not by the recorder. It holds the
scratch objects and the expected output of each bug fix. Its `about` lists
fixes 1–15 (1–7 from the plan, 8–15 found while recording). A case with `replaces` supersedes a recorded case, and the parity
test should assert that the recorded TypeScript output differs. Its `input` is
null when the recorded input applies. Otherwise it holds the rows of the
fixed query, run live on the scratch objects.

The generator is copied in
[`docs/plans/artifacts/2026-09-27-duckdb-bugfixes-model.ts.txt`](../../../../docs/plans/artifacts/2026-09-27-duckdb-bugfixes-model.ts.txt).
It is a vitest file that needs a `seaquel-server`, and it imports the
recorder's `server`, `replay` and `corpus` modules (now the
`2026-09-27-recorder-*.txt` copies). It holds the fixed catalog
queries, a model of the fixed parsers (index grouping, the overview, Estimated
Cardinality, FILTER, relationName) and a model of the fixed DDL generators.
Every fix in the models can be switched off. With all of them off, the models
must reproduce every recorded TypeScript output, and the generator checks that.
A case's fix numbers are the fixes whose removal changes its output, so every
recorded case a fix changes gets a `replaces` case (79 in all).

The generator runs every fixed query live. It also runs every DDL output on
a fresh in-memory database, one statement at a time in autocommit, as the table
editor does (`splitDdlScript`, then one `execute` per statement). A
transaction would hide fix 10: DuckDB keeps a dropped index's dependencies
until the transaction ends. Every ALTER TABLE and ADD COLUMN output now runs
to the end. The only DDL that fails is:

- artifacts of the inputs: a sequence that doesn't exist (`defaults_demo_seq`)
  and a table with no columns;
- the deliberate counter-examples: the bare ARRAY, LIST, MAP, STRUCT and UNION
  types (fix 12), and a foreign key written inline in an attached catalog
  ("FOREIGN KEY constraints cannot be defined cross-database", fix 11).

Every type left in the list creates a column.

**Regenerate the derived cases when you re-record.** The `parse-*`
replacements that carry rows, and the live `schema`, `columns`, `indexes` and
`table-sizes` outputs, come from the scratch objects.

### Case kinds and where the port checks them

| kind                                                            | input                                                    | checked by                                                                                                                   |
| --------------------------------------------------------------- | -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `ddl-create`, `ddl-alter`, `ddl-add-column`, `column-types`     | a definition, or null (the recorded one)                 | `tests/dialect_parity.rs`                                                                                                    |
| `sql`                                                           | null, or `{ query, catalog, schema, table }` (row count) | `tests/dialect_parity.rs`: the fixed query text                                                                              |
| `parse-schema`, `parse-columns`, `parse-indexes`, `parse-stats` | the fixed query's rows, or null                          | `tests/introspect_parity.rs`                                                                                                 |
| `parse-explain`                                                 | plan rows, or null                                       | `tests/introspect_parity.rs`                                                                                                 |
| `columns-fk-error`                                              | column rows and a `foreignKeysError`                     | `tests/introspect_parity.rs`: the metadata built when the foreign-key query fails                                            |
| `schema`, `schemas`, `columns`, `indexes`, `table-sizes`        | `{}` or `{ schema, table }`                              | `tests/smoke.rs`, live on the scratch objects (`schema_tables`, `list_schemas`, `table_metadata`, `statistics().tableSizes`) |

`sql.json` is superseded by the fixed queries, except for `getExplainQuery`.
`$1` is the schema as the tree lists it and `$2` is the table; DuckDB binds
`$n` positionally, so pass each value once. The row count is
`SELECT COUNT(*) AS row_count FROM "catalog"."schema"."table"`, with `"`
doubled, using the `table_catalog` and `table_schema` columns of the fixed
table sizes query.

### Catalogs (fix 6)

The adapter ignored catalogs. DuckDB's `information_schema` covers every
attached database, including `temp` and `system`. So an attached database's
schemas appeared as a second `main`, their tables were listed twice, a table's
columns were merged with its namesake's, and a primary key in both catalogs
made the columns query fail outright.

What the fixed queries do:

- The `system` and `temp` catalogs are left out. The TypeScript queries
  already hid temp tables (`table_type` is `LOCAL TEMPORARY`).
- The default catalog, `current_database()`, lists its schemas as they are:
  `main`, `fx_sales`. The common single-file case doesn't change.
- Every other attached catalog lists its schemas as `catalog.schema`, for
  example `fx_aux.main`. All of them are qualified, not only the duplicates,
  so a schema's name doesn't change when another database is attached.
- A catalog or schema name that contains `.` or `"` is double-quoted in the
  listing, with `"` doubled: `"fx.a.b"` (a default-catalog schema),
  `"fx.we""ird".main` (an attached catalog). Plain names stay bare, so
  nothing changes for the usual names. So a default-catalog schema literally
  named `fx_aux.main` is listed as `"fx_aux.main"` and can't be mistaken for
  the attached `fx_aux.main`. Both have live cases.
- The introspection queries match
  `CASE WHEN catalog = current_database() THEN part(schema) ELSE part(catalog) || '.' || part(schema) END = $1`,
  where `part` is that quoting. This is the same expression the listing uses,
  so a table in one catalog no longer picks up another catalog's columns,
  keys or indexes.
- A foreign key's `referencedSchema` is the schema as listed.
- Statistics cover the same catalogs as the tree. The overview adds up their
  sizes (an in-memory catalog is 0 bytes).
- The default catalog is `current_database()`, which follows the session's
  `USE`. After a `USE fx_aux` typed in the query editor, `fx_aux`'s schemas
  are listed bare and the file's are qualified, so the tree changes on the
  next refresh. The plan lists "consider pinning the file's catalog at
  connect time" as a follow-up.
- An attached catalog with the same name as a default-catalog schema makes a
  two-part name for that schema ambiguous. After `ATTACH … AS sales` next to
  a schema `sales`, DuckDB rejects `"sales"."t"` ("Ambiguous reference to
  catalog or schema "sales" - use a fully qualified path"). The listing itself
  stays distinct (`sales` and `sales.main`), but the DDL, CRUD and data-tab
  SQL the port builds from a default-catalog schema would need a three-part
  `"<file catalog>"."sales"."t"`. This is in the same plan follow-up.

**Follow-up for the port (Task 17), recorded in the plan.** A listed schema is
DuckDB identifier syntax, so anything that builds `"schema"."table"` from the
tree must split it quote-aware: `fx_aux.main` becomes `"fx_aux"."main"`,
`"fx.a.b"` becomes `"fx.a.b"`, and `main` becomes `"main"`. That covers the
data tab's SELECT and COUNT (`data-tabs.svelte.ts` builds them with its own
`quoteIdentifier`, which doesn't double `"` either), CRUD, DDL and the row
counts. Until then, attached tables show in the tree but won't open.

The proposal is to let the dialect own this:

- A `Dialect::quote_schema(schema)` method defaults to `quote_ident`. DuckDB
  overrides it to parse the listed schema with fix 14's parser (one part, or
  catalog and schema) and quote each part. The builders in `crud.rs` and
  `ddl.rs` call it for the schema part. The DDL model in the generator does
  this already, as fix 6's DDL side: `CREATE TABLE "fx_aux"."main"."fx_links"`,
  `DROP INDEX "fx_aux"."main"."…"`, checked live.
- `EngineClient` gets `qualifiedTable(schema, table)`, mirrored locally in
  TypeScript per engine as `PAGINATE` is, so there's no round trip.
  `data-tabs.svelte.ts` uses it instead of its own quoting.

Because the listing quotes any part with a `.` or `"`, the split is never
ambiguous.

### Notes

- DuckDB's `AlterTableOptions` are `qualify_drop_index: true` (fix 8),
  `supports_add_foreign_key: false`, `unsupported_notes: Some("DuckDB")` and
  `inline_foreign_keys_on_added_columns: false` (fix 4), and
  `drop_indexes_first: true` (fix 10). `tsql`, `drop_index_on_table`,
  `use_modify_column` and `qualify_index_name` are all false. DuckDB rejects
  any constraint in `ADD COLUMN`, so an FK on a column added in the same edit
  is a note too. Fixes 9 and 10's notes and the schema split need new
  switches (or a `quote_schema` hook), which the port adds.
- `collation` (on `SchemaColumn` and `CreateTableColumn`, added by the MSSQL
  port) stays unset for DuckDB. The fixed columns query doesn't read it, and
  ALTER never emits `COLLATE`.
- Fix 2 lists only `duckdb_indexes()` rows, with `type: "art"`. PRIMARY KEY
  and UNIQUE constraints have no row there, and DROP INDEX can't drop them.
  An expression key keeps DuckDB's text, e.g. `((order_no + 1))`, as the
  Postgres port keeps `pg_get_indexdef`'s.
- Fix 3 formats the size with the SQLite port's `format_bytes`. Fix 15 takes
  the name from `current_database()`. "DuckDB Database" is only the fallback
  when there is no row.
- Fix 7: `table_metadata` catches the foreign-key query's error, logs it and
  returns the columns without foreign keys. The columns and indexes queries
  still fail the call.

### Fixes 8–15 (found while recording)

- **Fix 8:** `qualify_drop_index: true`, so `DROP INDEX "schema"."name"` uses
  the table's original schema. Unqualified, DuckDB looks in `main` only.
- **Fix 9:** DuckDB rejects any constraint in `ADD COLUMN`. A NOT NULL column
  with a default becomes two `;\n` pieces:
  `ADD COLUMN c T DEFAULT d;` then `ALTER COLUMN c SET NOT NULL;`. The
  default fills existing rows, so this works on a table with rows. Without a
  default, SET NOT NULL fails on a table with rows ("NOT NULL constraint
  failed"). It only works on an empty table, and the generator can't tell
  which it is. So the column is added nullable and a note says to fill it and
  run SET NOT NULL.
  In ALTER TABLE, the SET NOT NULL follows fix 10: while the table keeps
  indexes, it is a note and only the ADD COLUMN runs. The standalone
  `generateAddColumnSql` doesn't know the table's indexes, so it always
  emits the SET NOT NULL. Nothing in the UI calls it (`EngineClient` has no
  add-column call).
- **Fix 10:** `drop_indexes_first: true`, the switch MSSQL fix 13 added to
  `AlterTableOptions`: removed indexes are dropped before the renames and
  column changes.
  DuckDB refuses `RENAME COLUMN`, `DROP COLUMN`, `ALTER COLUMN … TYPE` and
  `SET/DROP NOT NULL` while the table has any CREATE INDEX index, even one on
  another column ("Cannot alter entry … there are entries that depend on it").
  ADD COLUMN and SET/DROP DEFAULT work.
  PRIMARY KEY and UNIQUE constraints are backed by ART indexes too:
  - they don't block a rename, a NOT NULL change, or a type change of another
    column;
  - DuckDB refuses DROP COLUMN and TYPE on a column in the constraint;
  - DuckDB refuses DROP COLUMN of any column that comes before a PRIMARY KEY
    or UNIQUE column in the table's column order ("Cannot drop this column:
    an index depends on a column after it!"). In `customers(id PK, name,
email UNIQUE, status, balance)`, dropping `name` fails, and dropping
    `status` or `balance` works.

  Following the other engines' convention, each such statement becomes a note
  that carries the statement, and everything else runs:
  - While indexes the edit keeps remain, a blocked statement becomes
    `-- DuckDB can't run this while the table has indexes; drop "main"."i" first and recreate it afterwards: ALTER TABLE …`.
  - A later statement on a column whose rename became a note (its
    SET DEFAULT, a new index on it) becomes
    `-- DuckDB can't run this before the rename above: …`.
  - A constraint column's DROP COLUMN or TYPE becomes
    `-- DuckDB can't drop or change the type of a column in a PRIMARY KEY or UNIQUE constraint; recreate the table to change it: …`.
  - A DROP COLUMN before a constraint column becomes
    `-- DuckDB can't drop a column that comes before a PRIMARY KEY or UNIQUE column; recreate the table to change it: …`.

  Nothing later in the script names a dropped column: renames and ADD COLUMN
  come before the drops, and the target definition no longer has the column.
  So no dependent statement needs a note there.

  Notes follow the statements, in statement order, before fix 4's. Kept
  indexes are not dropped and recreated: an expression key (fix 2 lists its
  text) would come back as a quoted column name, and the editor's definition
  is all the generator knows about an index. Every ALTER output in these
  fixtures ran to the end live. That includes `add a NOT NULL column` on the
  indexed `customers` table (ADD COLUMN runs, SET NOT NULL is a note) and the
  two DROP COLUMN cases above. The script avoids failing part way only as far
  as the definition is accurate. The rules read the kept indexes, the column
  order, `isPrimaryKey` and `isUnique`, and a constraint the definition doesn't
  flag isn't seen. The port needs switches for these notes.

  **For the port: `isUnique` in edit mode.** These rules depend on the
  definition flagging UNIQUE-constraint columns, and today it doesn't.
  `create-table-tabs.svelte.ts` builds the edit-mode definition with
  `isUnique: false` for every column, and `SchemaColumn` has no unique flag.
  A UNIQUE column is therefore invisible: its DROP COLUMN or TYPE, or a DROP
  COLUMN before it, would be emitted and fail. The port should have the
  columns query report single-column UNIQUE constraints (from
  `duckdb_constraints()`, the same way as `is_primary_key`), and the editor
  should copy that flag into `isUnique`. A composite UNIQUE constraint can't
  be expressed per column, and its columns block drops the same way. Check it
  before relying on these notes.

- **Fix 11:** A foreign key in CREATE TABLE references the table's own
  schema, never `""` (`REFERENCES ""."users"` doesn't parse). **DuckDB
  doesn't support cross-schema or cross-catalog foreign keys** ("Creating
  foreign keys across different schemas or catalogs is not supported"), so
  the definition's `referencedSchema` can't be anything else. In an attached
  catalog a foreign key can't be created from the default catalog at all
  ("FOREIGN KEY constraints cannot be defined cross-database", checked live).
  So it is a note, as in fix 4:
  `-- DuckDB can't create a foreign key in attached catalog "fx_aux" from here: (…) REFERENCES … (…); create the table after USE "fx_aux" to add it`.
  The fix 4 notes keep the definition's reference text.
- **Fix 12:** The column types leave out bare `ARRAY`, `LIST`, `MAP`,
  `STRUCT` and `UNION`, which DuckDB rejects without type arguments. The
  table editor's type picker is a closed list (`create-table-view.svelte`).
  A full type still works through its SQL pane: `parseCreateTableSql` reads
  `INTEGER[]`, `INTEGER[3]` and `VARCHAR[][]` as the type, and
  `STRUCT(a INT, b VARCHAR)`, `MAP(VARCHAR, INTEGER)` and `UNION(…)` as type
  plus "precision", which `buildColumnType` puts back together. The picker
  then shows the type as text, with no length/precision box.
- **Fix 13:** A `FILTER` node's `Expression` becomes its `filter` when nothing
  else set one. This applies to FILTER nodes only; `Expression` on any other
  operator is ignored, as before.
- **Fix 14:** `relationName` is the last part of DuckDB 1.5's
  `catalog.schema.table`. The split is quote-aware: `"…"` parts are
  un-quoted and `""` un-doubled, so a table named `fx.dotted "t"` stays whole.
  An unparseable name is kept as it is. This also matches what the demo's 1.4
  shows.
- **Fix 15:** The overview query returns `current_database() AS database_name`,
  and that is the overview's name.

## DuckDB-WASM (the demo) compared

A check (copied in
[`docs/plans/artifacts/2026-09-27-duckdb-wasm-check.ts.txt`](../../../../docs/plans/artifacts/2026-09-27-duckdb-wasm-check.ts.txt);
its `corpus` import is the recorder's `2026-09-27-recorder-corpus.ts.txt`)
replayed every non-synthetic parse case
against `@duckdb/duckdb-wasm` 1.32.0, which is DuckDB 1.4.3, in Node. It used
the same seeded file and scratch objects, and converted rows with Arrow's
`toJSON()` as `DuckDBProvider` does. 29 cases matched and 39 differed:

- **Plans name tables differently.** 1.5.5's `Table` is
  `catalog.schema.table` (`seaquel_test.main.orders`); 1.4.3's is `orders`.
  That changes `relationName` in every plan with a scan, analyzed or not.
  Fix 14 brings the native one back to `orders`.
- **Plan shapes changed.** 1.5.5 adds compression `PROJECTION`s around
  `UNION` and `DISTINCT`, plans `count(*), max(total)` as
  `COLUMN_DATA_SCAN` (1.4.3: `UNGROUPED_AGGREGATE`), and inserts from a
  SELECT with `BATCH_INSERT` (1.4.3: `INSERT`).
- **Typed defaults quote the type in 1.5.5.** `CAST('2024-01-01' AS "DATE")`
  where 1.4.3 has `AS DATE`.
- The schema listing, indexes, index usage, overview, table sizes, row counts
  and the other columns were identical.
