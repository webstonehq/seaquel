# Rust Core Phase 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** The Postgres dialect (introspection, EXPLAIN, statistics, DDL and CRUD SQL generation) runs in Rust. The desktop app and the web app use it for Postgres connections, and cell values are exact end to end. The phase also measures what porting an engine costs, so phase 2 can be planned from real numbers.

**Architecture:** Phase 1 of `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md`, adapted to what phase 0 found.

- **Dialect logic.** It moves into the `seaquel-engine` crate (as a pure `Dialect` trait plus generic DDL/CRUD builders) and into `seaquel-engine-postgres` (the Postgres dialect, and introspection on the Postgres driver). The TypeScript `PostgresAdapter` is deleted at the end. The other four dialects stay in TypeScript until phase 2.
- **Frontend.** It gains one async `EngineClient` interface that every dialect-dependent call site uses. It has two implementations:
  - The Rust client calls a new `db_engine` endpoint.
  - The TypeScript client wraps today's `DatabaseAdapter` plus provider.
  - A registry picks the Rust client for Postgres on desktop and web, and the TypeScript client everywhere else, including the browser demo.
- **Wire calls.** The endpoint carries a small `seaquel-rpc` crate: an `EngineRequest`/`EngineResponse` pair and a dispatcher onto `Core`. Tauri exposes it as one command. The web server exposes it as one route, whose body carries a top-level `connection_id`, so the Node proxy's tenant scoping keeps working.
- **Cell values.** A typed `Value` enum in `seaquel-types` replaces `serde_json::Value` in every driver. It has one wire format:
  - Plain JSON wherever JavaScript holds the value exactly.
  - Otherwise a tagged object `{"$sq": kind, "v": …}` (see "Value wire format" below).
  - The providers decode tags into JS values (`bigint`, `Uint8Array`, `SqlDecimal`, JSON values) before any UI code sees a row, and encode parameters the other way.

**Tech Stack:**
- Rust: sqlx 0.8 (postgres), rust_decimal, base64, ts-rs 12.
- Frontend: SvelteKit 5, TypeScript, vitest.
- Testing: the e2e Docker databases.

**Decisions already made (2026-09-25):**
1. **Cell values** use tagged values decoded in the providers. That covers bigints JavaScript can't hold exactly, all NUMERIC, all bytea and all JSON cells.
2. **Port fidelity:** keep TypeScript behaviour byte-for-byte except for the bug fixes (four agreed up front, a fifth found while recording fixtures, a sixth found in review after the port, and a seventh closing two of the sixth's follow-ups). Each has its own test and is listed in the "Bug fixes" section.

---

## Ground rules for whoever executes this

- **No git writes.** The repo owner forbids `git add`, `commit`, `mv`, `stash`, branches and worktrees. Use plain `mv`/`cp`/`rm`. Each task ends with a **Checkpoint**: stop, summarise what changed and how you verified it, and let the user review. Read-only git is fine.
- Run everything from the repo root, `/Users/m/projects/github/webstonehq/seaquel`.
- Never edit `src/lib/components/ui/*`. Error toasts use `errorToast` from `$lib/utils/toast`. Run the Svelte MCP `svelte-autofixer` on every `.svelte` file you change.
- Core crates (everything under `crates/` except `seaquel-server`) may not use `tokio::spawn`, `Instant` or `SystemTime`. Clippy enforces this.
- Test databases come from `docker compose -f e2e/test-databases/docker-compose.yml up -d`, then `node e2e/test-databases/seed.mjs postgresql`. Postgres runs at `postgres://postgres@127.0.0.1:5432/seaquel_test`.
- **Parity rule:** where a task says "port", Rust output must equal the recorded TypeScript output (Task 3 fixtures), except for the bug fixes listed below. If a fixture disagrees with your port, the fixture wins, unless the difference is one of those bug fixes.
- Run the full check before each checkpoint:
  - `npm run crates:check`
  - `cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`
  - `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`
  - `cargo check -p seaquel`
  - `npm run check`
  - `npx vitest run`
  - `npx oxlint --type-aware --type-check --deny-warnings`
- **Effort log:** every implementer appends one line per task to `docs/plans/2026-09-25-phase-1-effort.md`. The line gives the task number, the time spent, the lines of Rust added, the lines of TypeScript removed, and any surprise. Task 16 turns this into the phase 2 estimate.

### Value wire format

This is the contract between Rust and every provider. `seaquel-types` and `src/lib/values.ts` implement it, and both have round-trip tests.

| Rust `Value` | Wire JSON | JS value after `decodeCell` |
|---|---|---|
| `Null` | `null` | `null` |
| `Bool(b)` | `true`/`false` | boolean |
| `Int(i)` with \|i\| ≤ 2^53−1 | number | number |
| `Int(i)` otherwise | `{"$sq":"bigint","v":"<i>"}` | `bigint` |
| `Float(f)`, finite | number | number |
| `Float(f)`, NaN/±inf | `{"$sq":"float","v":"NaN"\|"inf"\|"-inf"}` | number (`NaN`/`±Infinity`) |
| `Decimal(s)` | `{"$sq":"decimal","v":"<s>"}` | `SqlDecimal` (keeps the exact text, e.g. `"12.50"`) |
| `Text(s)` | string | string |
| `Bytes(b)` | `{"$sq":"bytes","v":"<base64>"}` | `Uint8Array` |
| `Json(j)` | `{"$sq":"json","v":<j>}` | `j` as-is (the same object the UI gets today) |
| `Array(vs)` | JSON array of encoded elements | array of decoded elements |

**Parameters sent to Rust:**
- `encodeParam` turns `bigint` into the `bigint` tag, `Uint8Array` into `bytes`, and `SqlDecimal` into `decimal`.
- Any non-array object becomes `{"$sq":"json","v":…}`.
- Everything else is sent as it is.

**Decoding parameters in Rust (`Value::from_wire`):**
- Tags are interpreted.
- A plain object becomes `Json`.
- A plain integer that fits `i64` becomes `Int`. An integer between `i64::MAX` and `u64::MAX` becomes `Decimal` (its digits).
- Any number serde_json parses as a float stays `Float`, however large or integral (`1e30`, `2^64`). JS tags its bigints, so a huge plain number can only be a float.
- Other numbers become `Float`, strings become `Text`, and arrays become `Array`.

**Drivers building values from their own decoders** use `Value::from_json_cell`, which never interprets `$sq`:
- An object becomes `Json`.
- An array becomes `Array`.
- Numbers are handled as in `from_wire`.

### Bug fixes (the only intended changes from TypeScript behaviour)

1. **Bind catalog filters.** Catalog queries bind the table and schema names (`$1`, `$2`) instead of splicing `validateIdentifier(...)` into the SQL. Tables named `order items` or `my-table` now load their columns and indexes; today they throw `Invalid SQL identifier`.
2. **Escape DDL identifiers.** The DDL `quote` escapes an embedded `"` as `""`, as `quoteIdentifier` already does.
3. **Schema-qualify DROP INDEX.** `ALTER TABLE` generation emits `DROP INDEX "schema"."name";`, using the original definition's schema (where the index lives).
4. **Read index columns from the catalog.** Index columns come from the catalog (`pg_index`, `pg_attribute` and `pg_get_indexdef(indexrelid, k, true)` per key column) instead of regex-parsing `indexdef`.
   - Plain key columns are returned as their attribute names (`pg_attribute.attname`), unquoted, so they match `SchemaColumn.name` and the DDL generator doesn't quote them twice: a column `"Mixed Case"` comes back as `Mixed Case`.
   - Expression indexes show the expression as `pg_get_indexdef(oid, k, true)` prints it, e.g. `lower(name::text)`.
   - `INCLUDE` columns are excluded.
   - `unique` comes from `indisunique`, and `type` from the access method (`btree`, `gin`, …) instead of the hard-coded `"btree"`.
5. **Statistics survive unusual table names.** The three statistics queries build `schemaname || '.' || relname` and pass it to `pg_*_size()`. That fails with "invalid name syntax" when any table name needs quoting, which breaks the whole Statistics view. Use the relation oid instead (`pg_total_relation_size(relid)`, `pg_relation_size(relid)`, `pg_indexes_size(relid)`). Task 3's recorder dropped the awkward tables before recording stats, so the parity fixtures still apply. Add a `bugfixes.json`-style expectation in Task 6's `run_introspection`: statistics succeed with `order items` present.
6. **Typed primary keys and NULLs in CRUD.** Found in review after Task 13, against the live database. Rows keyed by uuid, date, timestamp or time could not be edited or deleted, and "Set NULL" failed on uuid, date, timestamp, time, interval, inet and bytea columns:
   - The UI sends these values as text, and the WHERE clause compared `pk = $n` with a TEXT parameter: `operator does not exist: uuid = text`. The generic CRUD builders now wrap primary-key placeholders in `CAST($n AS type)` when the cast map has a non-empty type for that key, in update, set-default and delete. Without a cast the SQL is unchanged.
   - `Value::Null` bound as a JSONB NULL, which has no cast to those types (`cannot cast type jsonb to date`). It binds as a TEXT NULL now, which casts explicitly to every type.
   - `Dialect::build_set_default`/`build_delete`, the `buildSetDefault`/`buildDelete` wire requests and `EngineClient` take an optional cast map, and `query-crud.svelte.ts` passes `buildCastMap(…)` to all three. `TsEngineClient` ignores it: the TypeScript helpers (MySQL, SQLite, MSSQL, DuckDB) never cast keys and are unchanged.
   - `crud.json` is replayed without casts on key columns (two cases have one); `bugfixes.json` has four `crud` cases for the key casts, and `smoke.rs` (`crud_on_typed_keys_and_columns`) edits, sets NULL on, sets default on and deletes uuid- and date/timestamp/time-keyed rows live.
   - Casting keys means a key must decode to text that casts back to the same value. A review round trip over ~45 key types found the ones that didn't, now fixed:
     - TIME `24:00:00` decoded as `0:00:00.0` (the `time` crate has no 24:00), so an edit on the 24:00 row would hit the 00:00 row. TIME and TIMETZ 24:00 now decode as `24:00:00`.
     - `bit(n)` and `char(n)` keys: information_schema reports `bit`/`character` without the length, and `CAST(… AS bit)` is bit(1), which truncates, so nothing matched (rows = 0, no error). `castMapForColumns` now maps `bit` → `bit varying` and `character` → `bpchar` (for Postgres, fix 7's `castType` replaces this: `"bit"` and `bpchar`) (`bit = text` has no operator, so leaving bit uncast fails; `char(3) = text` compares without the blank padding and never matched, even before phase 1).
     - `infinity`/`-infinity` dates and timestamps panicked inside sqlx ("overflow adding duration to date"), and so did years past 9999. BC years decoded as `-0043-03-15`, which doesn't cast back. `decode.rs` reads DATE, TIME, TIMETZ, TIMESTAMP and TIMESTAMPTZ (and their arrays) from the binary form itself: `infinity`/`-infinity`, `24:00:00`, `0044-03-15 12:00:00 BC` and `10000-01-01` come back as Postgres prints them; every other value keeps its old format. TIMETZ[] decodes now too.
     - OID cells were 4 raw bytes read as text; they decode as `Int` now.
7. **Exact cast types, and a cast map even when the table isn't cached.** Two gaps left by fix 6:
   - "Set NULL" and edits on enum and array columns failed (`column "m" is of type mood but expression is of type text`). information_schema's `data_type` is only `USER-DEFINED`/`ARRAY` for them, so `castMapForColumns` gave them no cast, and the TEXT NULL doesn't assign uncast.
   - With the table missing from the schema cache (`state.schemas`), `buildCastMap` returned `undefined`, so nothing was cast: typed keys failed (`uuid = text`) and "Set NULL" on json/jsonb failed.
   - `SchemaColumn` gains an optional `castType`. Postgres's `COLUMNS_SQL` adds a `cast_type` column, built from the catalog for the column's `pg_attribute` row:
     - A type outside `pg_catalog` (enum, domain, composite, range, an extension's type such as `public.citext`, or an array of one) is schema-qualified by hand with `quote_ident`: `app."Weird Mood"`, `app.mood[]`, `app.posint`. `format_type` would leave out the schema whenever the type is on the loading connection's search_path, and the CRUD statement may run on a pooled connection with another one. The typmods of user types are left to assignment.
     - A built-in type is `format_type(atttypid, atttypmod)`: `integer[]`, `numeric(10,2)`, `timestamp(0) without time zone`.
     - When the type resolves to `bpchar`, `varchar`, `bit` or `varbit` through its whole chain of domains and array elements (a recursive CTE over `typbasetype`/`typelem`), the cast is that base type without a length, plus `[]` when an array was on the way: `character varying`, `bpchar[]`, `"bit"`, `bit varying`. An explicit `CAST('toolong' AS varchar(5))` truncates silently, and so do `char(n)`, `bit(n)` (which zero-pads) and domains over them at any depth (`CREATE DOMAIN v3b AS v3`, with `v3` over `varchar(3)`, stored `'too'`). Assigning the unbounded value to the column raises "value too long" and still checks the domain's constraints. `"bit"` (quoted, as `format_type(bit, -1)` prints it) has no length, unlike `bit`, which is `bit(1)`; a `bit(n)` key compared with `CAST($n AS "bit")` uses the primary-key index, which `bit varying` couldn't.
   - Cast types are trusted input: the builders interpolate them into the SQL as they are (documented on `CastMap` and `get_cast_placeholder`). They only ever come from the catalog, and `quote_ident` keeps a type named `x"; DROP TABLE y; --` inert (checked in review).
   - `castMapForColumns` casts every column that has a `castType` to it verbatim (text columns get a harmless `text` cast). Columns without one (MySQL, SQLite, MSSQL, DuckDB) keep the old rules exactly.
   - `buildCastMap` is async. When the table or its columns aren't cached and the connection uses the Rust engine, it loads them with `EngineClient.tableMetadata` and builds the map. Loads are remembered per connection, provider connection id (so a reconnect or disconnect starts over), schema and table, as promises, so concurrent edits share one load and a table missing from the cached list isn't loaded on every edit. Every schema (re)load forgets them (`forgetLoadedColumns`, called from the `onSchemaLoaded` callback that connect, reconnect and refresh all go through). The result goes into the schema cache the way the schema tab stores it (`storeTableMetadata` in `schema-cache.ts`, now shared), but only if the cached entry still has no columns, so a refresh that finished first isn't overwritten. If loading fails it returns `undefined` (the old behaviour), logs at debug level, and the next edit retries. TypeScript engines don't load: only SQLite casts, and only for type affinity.
   - Tests: `smoke.rs` `crud_with_catalog_cast_types` (enums in a schema not on the search_path, one needing quotes, one in `public`, int[]/text[]/enum[], json/jsonb, domains including two levels over `varchar(3)` and an array of a domain, castType loaded with the scratch schema on the search_path (still qualified), varchar/char/numeric, too-long values rejected, a domain CHECK kept, a `bit(16)` key through an Index Scan, a `char(3)` key); the `introspection` expectations and a `crud` case in `bugfixes.json`; `introspect_parity.rs` (the SQL change and the parser); vitest for `castMapForColumns` and `buildCastMap` (loading, caching, sharing one load, forgetting on reload/reconnect/failure, not overwriting a refresh).

---

## Part A — Contracts

### Task 1: Dialect types in `seaquel-types`

Move the TypeScript dialect types into Rust, generate them with ts-rs, and make the hand-written TypeScript types re-export the generated ones. That gives the types one source.

**Files:**
- Create: `crates/seaquel-types/src/dialect.rs`
- Modify: `crates/seaquel-types/src/lib.rs` (`mod dialect; pub use dialect::*;`)
- Modify: `crates/seaquel-types/tests/wire_format.rs` (add shape tests)
- Modify: `src/lib/types/{schema,explain,statistics,create-table}.ts` (re-export the generated types; keep the TS-only types such as `SchemaTab`, `ExplainTab`, `StatisticsTab`)
- Create (generated): `src/lib/types/generated/*.ts`

**Types.** Each one mirrors its TypeScript definition exactly, uses `#[serde(rename_all = "camelCase")]`, and carries `#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]`. Serialize, Deserialize, Debug, Clone and PartialEq are derived on all of them.

| Rust type | Source of the TS definition |
|---|---|
| `ForeignKeyRef`, `SchemaColumn`, `SchemaIndex`, `SchemaTable` | `src/lib/types/schema.ts` |
| `ExplainPlanNode`, `ExplainResult` | `src/lib/types/explain.ts` |
| `TableSizeInfo`, `IndexUsageInfo`, `DatabaseOverview`, `DatabaseStatistics` | `src/lib/types/statistics.ts` |
| `ColumnTypeInfo`, `CreateTableColumn`, `CreateTableIndex`, `CreateTableForeignKey`, `CreateTableDefinition` | `src/lib/types/create-table.ts` |
| `SqlWithBindings { sql: String, bind_values: Option<Vec<Value>> }` | `src/lib/db/crud-helpers.ts` (add in Task 2, once `Value` exists) |

Rules:
- String unions become enums with serde renames:
  - `SchemaTable.type` is `TableKind` with `table`, `view` and `materialized-view` (`#[serde(rename = "type")]` on the field).
  - `ColumnTypeInfo.category` is `ColumnCategory` with `"String"`, `"Numeric"`, `"Date/Time"`, `"Boolean"`, `"JSON"`, `"Binary"`, `"UUID"`, `"Network"` and `"Other"`.
- Integer fields that are plain JS numbers get `#[cfg_attr(feature = "ts", ts(type = "number"))]`. These are `rowCount`, sizes, counts, rows and loops, all as `i64`/`f64`.
- `TableSizeInfo.totalSize` and similar human-readable sizes are `String`.

**Step 1 (red).** In `wire_format.rs`, write one serde test per top-level type, checking the camelCase JSON shape. Example:

```rust
#[test]
fn schema_table_shape() {
    let t = SchemaTable {
        name: "users".into(),
        schema: "public".into(),
        kind: TableKind::MaterializedView,
        row_count: None,
        columns: vec![],
        indexes: vec![],
    };
    assert_eq!(
        to_value(&t).unwrap(),
        json!({ "name": "users", "schema": "public", "type": "materialized-view", "columns": [], "indexes": [] })
    );
}
```

Optional fields must be omitted when `None`. Use `#[serde(skip_serializing_if = "Option::is_none")]`: `SchemaColumn.defaultValue` is optional, and TS code checks it with `=== undefined`. Run `cargo test -p seaquel-types` and watch it fail.

**Step 2 (green).** Implement `dialect.rs`, then run the tests.

**Step 3.** Run `npm run types:gen`. In each TS types file, replace the interface with `export type { SchemaTable } from "./generated/SchemaTable";` and so on. Keep the TS-only types.

**Step 4.** Run `npm run check`. It must report 0 errors. Any mismatch is either a transcription error (fix the Rust) or a real TS inconsistency (report it; don't paper over it).

**Checkpoint.** Suggested commit: `Generate the dialect types from seaquel-types`.

---

### Task 2: The typed `Value`

**Files:**
- Create: `crates/seaquel-types/src/value.rs`, re-exported from lib.
- Modify: `crates/seaquel-types/Cargo.toml` (add `base64 = { workspace = true }`).
- Create: `src/lib/values.ts`
- Create: `src/lib/values.test.ts`
- Modify: `crates/seaquel-types/src/lib.rs`
  - `QueryResult.rows` and `StreamBatch.rows` become `Vec<Vec<Value>>` (keep `ts(type = "unknown[][]")`).
  - `BatchStatement.params` becomes `Vec<Value>`.
  - Add `SqlWithBindings`.

**Rust API:**

```rust
pub enum Value { Null, Bool(bool), Int(i64), Float(f64), Decimal(String), Text(String), Bytes(Vec<u8>), Json(serde_json::Value), Array(Vec<Value>) }

impl Value {
    /// Parameters and anything else arriving from a client: interprets `$sq` tags.
    pub fn from_wire(j: serde_json::Value) -> Result<Value, String>;
    /// Cells produced by a driver's own JSON decoder: never interprets `$sq`.
    pub fn from_json_cell(j: serde_json::Value) -> Value;
    /// Integer view for tests and parsers: Int, integral Float, or Decimal/Text digits.
    pub fn as_i64(&self) -> Option<i64>;
    pub fn as_str(&self) -> Option<&str>;
}
impl Serialize for Value { /* the wire table */ }
impl<'de> Deserialize<'de> for Value { /* serde_json::Value::deserialize then from_wire */ }
```

**Rust tests (red first).** One test per row of the wire table in both directions, plus these cases:
- `2^53−1` stays a number and `2^53` is tagged.
- `i64::MIN` is tagged.
- A plain JSON integer of `u64::MAX` becomes `Decimal("18446744073709551615")`; plain `1e30` and 2^64 (parsed as floats) stay `Float`, and `from_json_cell(1e20)` serializes exactly like `json!(1e20)`.
- A plain object without `$sq` becomes `Json`.
- An unknown `$sq` kind is an error.
- `from_json_cell` on an object that contains `$sq` gives `Json` (it doesn't interpret tags).
- Nested arrays of tagged values.

**TS API** (`src/lib/values.ts`):

```ts
/** Exact decimal from the database. Keeps the text Postgres sent (scale included). */
export class SqlDecimal {
  constructor(readonly value: string) {}
  toString(): string { return this.value; }
  toJSON(): string { return this.value; }
}
export function decodeCell(v: unknown): unknown;          // wire → JS (recurses into arrays)
export function decodeRows(rows: unknown[][]): unknown[][]; // in place, only touches objects
export function encodeParam(v: unknown): unknown;          // JS → wire
export function jsonReplacer(key: string, v: unknown): unknown; // bigint→string, Uint8Array→"\\x…" hex, SqlDecimal→string
export function toHex(bytes: Uint8Array): string;          // "\\x0102ff"
/** Stable string key for comparing cell values (PK matching). */
export function cellKey(v: unknown): string;
```

**TS tests (red first).**
- Round-trip every wire row.
- `decodeRows` leaves primitive cells untouched (identity).
- `JSON.stringify(row, jsonReplacer)` never throws for bigint or Uint8Array.
- `cellKey(10n) === cellKey(10)` is **false**, deliberately, since types differ.
- `cellKey(new SqlDecimal("1.50")) === "1.50"`.

**Fix Rust callers.** Now fix every Rust caller until the workspace compiles:
- **Engines' decoders.** Wrap them in `Value::from_json_cell(...)`. Postgres gets a native decoder in Task 7; for now wrap it as well.
- **Binders.** `impl_sqlx_driver!`'s `bind_params` and the MSSQL/DuckDB binders take `&[Value]`:
  - `Int` binds as i64. This fixes the f64 binding bug.
  - `Float` as f64, `Text` as String, `Bool` as bool.
  - `Null` as `None::<String>` for MSSQL; keep sqlx's existing typed-null behaviour.
  - `Json` as a JSON value.
  - `Decimal` as `rust_decimal::Decimal` where the driver supports it (Postgres, MySQL). Where it doesn't, bind the string (SQLite) or the literal text (MSSQL, DuckDB).
  - `Bytes` as `Vec<u8>`.
  - `Array`: Postgres gets its array bind in Task 7; everyone else returns `QUERY_ERROR "array parameters are not supported"`.
- **Binder hook.** The macro gains a `bind_fn = $path` argument so each engine supplies its own binder. The macro must not grow per-database branches.
- **Other callers.** Update Core, server, Tauri and testkit. `as_i64` moves from the testkit onto `Value`.
- **Smoke tests.** They must stay green for all six databases.

**Provider decoding and encoding:**
- `unified-tauri-provider.ts` and `http-provider.ts` call `decodeRows` on every batch and on `select` results before building row objects. They call `encodeParam` on every parameter before sending.
- `tauri-sqlite.ts` decodes too. App storage never produces tags today, but it must not break if it ever does.
- The DuckDB-WASM provider is unchanged. It already produces `bigint`/`Uint8Array`, which the UI will now handle (Task 11).

**Checkpoint.** Run the full check. Suggested commit: `Add the typed Value and its wire format`.

---

## Part B — Parity fixtures

### Task 3: Record the TypeScript Postgres dialect

**Files:**
- Create: `scripts/fixtures/postgres-dialect.fixtures.test.ts`, a vitest file that writes fixtures when `RECORD_FIXTURES=1` and otherwise checks them.
- Create: `crates/seaquel-engine-postgres/tests/fixtures/*.json`
- Modify: `package.json` (`"fixtures:postgres": "RECORD_FIXTURES=1 vitest run scripts/fixtures/postgres-dialect.fixtures.test.ts"`)

The script imports `PostgresAdapter` and the helpers directly. For each group it writes `{ "cases": [ { "name", "input", "output" } ] }`.

| Fixture file | What it records |
|---|---|
| `crud.json` | `buildUpdateSql`, `buildSetDefaultSql`, `buildInsertSql` and `buildDeleteSql` over: single and composite PKs, a cast lookup (the input records a `{column: type}` map; the script turns it into a `CastLookup`), no cast lookup, identifiers containing `"`, and null/bool/number/string/object values |
| `quote.json` | `quoteIdentifier` for plain, `"`-containing, unicode and empty names |
| `paginate.json` | `paginateQuery` |
| `ddl-create.json` | `generateCreateTableSql` for ≥ 8 definitions: PKs, composite PK, unique, FK with and without schema, defaults including sanitized ones (`;`, `--`, empty), length/precision, indexes |
| `ddl-alter.json` | `generateAlterTableSql` for ≥ 12 before/after pairs, covering each branch of the statement order in `alter-table.ts` plus "no changes" |
| `column-types.json` | `getColumnTypes()` |
| `parse-schema.json`, `parse-columns.json`, `parse-indexes.json`, `parse-stats.json`, `parse-explain.json` | Each parser's input rows and output. Inputs come from **real Postgres**: the recorder runs the adapter's SQL against the seeded `seaquel_test` database (via `pg`, already a devDependency of the e2e seeder) and records rows and output. Explain covers `EXPLAIN` and `EXPLAIN ANALYZE` of a join, a sort and an index scan |
| `sql.json` | The literal SQL of `getSchemaQuery`, `getColumnsQuery` (for `public.users`), `getIndexesQuery`, the three statistics queries and `getSchemasQuery` |

Set expected outputs for the four bug fixes by hand in a separate `bugfixes.json`. The recorder never writes that file.

**Steps:**
1. Write the recorder.
2. Run `npm run fixtures:postgres` with Docker up.
3. Run `npx vitest run scripts/fixtures` without the variable; it must pass, because it checks what it just recorded.
4. Commit-ready fixtures: at least 60 cases in total.

**Checkpoint.** Suggested commit: `Record Postgres dialect parity fixtures`.

---

## Part C — Rust dialect

### Task 4: The `Dialect` trait, introspection hooks, generic builders

**Files:**
- Create: `crates/seaquel-engine/src/dialect.rs`
- Create: `crates/seaquel-engine/src/ddl.rs`, the port of `alter-table.ts`, parameterized exactly as in TS by a quote fn and `AlterTableOptions`.
- Create: `crates/seaquel-engine/src/crud.rs`, the port of the **parameterized** builders from `crud-helpers.ts` only: `buildParam*`, `getCastPlaceholder`, and the placeholder fn. YAGNI: the inline builders move with MSSQL/DuckDB in phase 2.
- Modify: `crates/seaquel-engine/src/lib.rs`

**The trait** (`dialect.rs`). It is pure and must build for wasm32:

```rust
pub type CastMap = std::collections::HashMap<String, String>; // column → declared type

pub trait Dialect: MaybeSend + MaybeSync {
    fn quote_ident(&self, id: &str) -> String;
    fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String;
    fn build_update(&self, schema: &str, table: &str, column: &str, value: Value, pks: &[String], row: &RowValues, casts: Option<&CastMap>) -> SqlWithBindings;
    fn build_set_default(&self, schema: &str, table: &str, column: &str, pks: &[String], row: &RowValues) -> SqlWithBindings;
    fn build_insert(&self, schema: &str, table: &str, values: &[(String, Value)], casts: Option<&CastMap>) -> SqlWithBindings;
    fn build_delete(&self, schema: &str, table: &str, pks: &[String], row: &RowValues) -> SqlWithBindings;
    fn create_table(&self, def: &CreateTableDefinition) -> String;
    fn alter_table(&self, from: &CreateTableDefinition, to: &CreateTableDefinition) -> String;
    fn column_types(&self) -> Vec<ColumnTypeInfo>;
    fn explain_sql(&self, sql: &str, analyze: bool) -> String;
}
pub type RowValues = Vec<(String, Value)>; // ordered, as the TS row object was
```

The builders take the row as `RowValues`. Keep insertion order for inserts, as TS does.

**The `Engine` trait** gains:

```rust
fn dialect(&self) -> Option<&dyn Dialect> { None }
```

**The `Driver` trait** gains introspection methods whose default returns `DbError { code: "NOT_SUPPORTED", message: "… is not supported by this engine yet" }`:

```rust
async fn list_schemas(&self) -> Result<Vec<String>, DbError>;
async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError>;
async fn table_metadata(&self, schema: &str, table: &str) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError>;
async fn statistics(&self) -> Result<DatabaseStatistics, DbError>;
async fn explain(&self, sql: &str, params: Vec<Value>, analyze: bool) -> Result<ExplainResult, DbError>;
```

**Tests:**
- `ddl.rs` and `crud.rs` get unit tests for the option branches the Postgres fixtures don't reach: `useModifyColumn`, `supportsDropColumn: false`, and the `?` placeholder. Port a few of the TS behaviours as literal expectations.
- Parity against the fixtures happens in Task 5, where a real quote fn exists.
- Also run `cargo clippy --target wasm32-unknown-unknown -p seaquel-engine -- -D warnings`.

**Checkpoint.** Suggested commit: `Add the Dialect trait and generic DDL/CRUD builders`.

---

### Task 5: `PostgresDialect`

**Files:**
- Create: `crates/seaquel-engine-postgres/src/dialect.rs`
- Modify: `crates/seaquel-engine-postgres/src/lib.rs` (`PostgresEngine::dialect()` returns a static `PostgresDialect`)
- Create: `crates/seaquel-engine-postgres/tests/dialect_parity.rs`

`PostgresDialect` has three parts:
- **Quoting.** `quote_ident` escapes like `qi`. The DDL quote now also escapes (bug fix 2).
- **Builders.** They use the generic builders from Task 4 with the `$N` placeholder.
- **Static data.** `column_types()` is the static list and `explain_sql()` is the TS logic.

**Guidance from the fixture review:**
- serde_json here has no `preserve_order`, so a `serde_json::Map` sorts its keys. Deserialize the fixture `values`/`row` objects into `RowValues` with an order-preserving visitor (or enable `serde_json/preserve_order` for dev-dependencies only). Insert column order matters.
- Build expected bind values with `Value::from_json_cell`. Fixture objects are raw JSON, not `$sq` tags. `"12.50"` is Text, `2.5` is Float and `42` is Int.
- A `CastMap` entry whose type is `""` means no cast, as TS's `!castType` does.

`dialect_parity.rs` loads each fixture file with `include_str!`, feeds `input` to the matching method, and asserts equality with `output`. It is one test per fixture file, and each reports the failing case name. `bugfixes.json` cases for fixes 2 and 3 run here too.

**Red:** write `dialect_parity.rs` first; it doesn't compile. **Green:** implement.

**Checkpoint.** Suggested commit: `Port the Postgres dialect with parity tests`.

---

### Task 6: Postgres introspection

**Files:**
- Create: `crates/seaquel-engine-postgres/src/introspect.rs`. It holds the SQL and the pure parse functions, `parse_schema(rows) -> Vec<SchemaTable>` and so on, which take `&QueryResult`.
- Modify: `crates/seaquel-engine-postgres/src/driver.rs`. It implements the five `Driver` introspection methods by running the SQL on the pool and parsing.
- Create: `crates/seaquel-engine-postgres/tests/introspect_parity.rs`, for the parse functions against the `parse-*.json` fixtures.
- Modify: `crates/seaquel-engine-testkit/src/lib.rs`. Add `run_introspection(engine, config)`. It creates a scratch schema with:
  - a table with a composite PK, an FK, a unique index, an expression index and an `INCLUDE` index
  - a view and a materialized view
  - a table named `order items`

  It asserts on what `schema_tables`, `table_metadata`, `statistics` and `explain` return, then drops the schema.
- Modify: `crates/seaquel-engine-postgres/tests/smoke.rs` (call `run_introspection`)

**Rules:**
- **SQL.** Every query is the fixture's `sql.json` text, except for three changes: catalog filters are bound (`$1`/`$2`), indexes use the new catalog query, and the three statistics queries use `relid` (bug fixes 1, 4 and 5). `introspect_parity.rs` asserts the unchanged queries equal `sql.json`. Fix 5 is tested only in `run_introspection`, not in `bugfixes.json`: that file's TS guard expects exactly fixes 1–4.
- **Parsers.** They must produce the recorded output for every recorded input.
  - The index parser is new, so there's no parity for it.
  - Its expectations for the scratch schema live in `run_introspection` and in `bugfixes.json`.
- **EXPLAIN.** It reads the `QUERY PLAN` column, which arrives as `Value::Json`, and assigns node ids post-order as TS does.
- **Statistics.** `DatabaseStatistics` combines the three stats queries, run concurrently with `futures::join!`.
- **Guidance from the fixture review:**
  - The ported `Number(x) || 0` must accept Int, Float, Decimal and numeric Text, and map Null or unparsable values to 0.
  - Fixtures store `QUERY PLAN` as a JSON array. The live driver yields `Value::Json`, so the harness wraps that cell as `Value::Json` (or the parser accepts both).
  - Branches with no fixture that must still match TS:
    - `column_default` of `""` becomes `None`.
    - `foreign_key_ref` is split on every `.`, and a ref is produced only when there are exactly 3 parts.
    - EXPLAIN: the plan as a string; the lowercase `"query plan"` key; empty rows (`Unknown` node, planning time 0); `Filter` (no recorded node has one); `executionTime` omitted when absent; `actual*` fields dropped when not analyzing.
  - Fix 4 as a single query: `ARRAY(SELECT CASE WHEN i.indkey[k - 1] <> 0 THEN a.attname::text ELSE pg_get_indexdef(i.indexrelid, k, true) END FROM generate_series(1, i.indnkeyatts) k LEFT JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[k - 1] ORDER BY k)`, ordered by index `relname` (C-collated `name`, which matches the fixture order).
  - Fix 5 in `run_introspection`: with `order items` present, `statistics()` is Ok and `tableSizes` contains it with `totalSizeBytes > 0`.
- **Compare fixtures as typed structs, not raw JSON.** Deserialize each fixture `output` into the Rust type (e.g. `ExplainPlanNode`) and compare structs. `serde_json::Value` treats `100` and `100.0` as different, and `planRows`/`actualRows` are `f64`.
- **Always-present stats fields.** TS always sets `rowsRead`, `dataSize`, `indexSize`, `totalSizeBytes` and `connectionCount` (as `|| 0`). The Rust parsers must return `Some(0)` there, not `None`.

**Checkpoint.** Run the smoke test against Docker Postgres. Suggested commit: `Postgres introspection in Rust`.

---

### Task 7: Native Postgres values

**Files:**
- Rewrite: `crates/seaquel-engine-postgres/src/decode.rs`, going from `to_json` to `to_value`. It produces `Value` directly.
- Create: `crates/seaquel-engine-postgres/src/bind.rs`, the Postgres binder, including arrays.
- Modify: `crates/seaquel-engine-postgres/tests/smoke.rs`. Add a value round-trip test.

**Decoding changes:**
- **Integers.** INT2, INT4 and INT8 become `Int`. The JS-safety split now happens at serialization.
- **NUMERIC** becomes `Decimal(text)`, keeping Postgres's scale (`12.50`) and handling NaN and values beyond 28 digits. Decode the text form instead of `rust_decimal`, so nothing is lost.
- **BYTEA** becomes `Bytes`.
- **JSON/JSONB** become `Json`.
- **Arrays** become `Array` of element values. Use the same element types as today, plus NUMERIC[], DATE[] and JSONB[] where sqlx supports them.
- **FLOAT4/FLOAT8** become `Float`.
- **Everything else** stays `Text` exactly as today (dates, times, intervals, network, geometry, enums). Changing date formats is out of scope.
- **Domain types.** Check `yes_or_no`, `sql_identifier` and `character_data` from information_schema: they must decode as `Text`, not garbage.

**Binding:**
- `Int` as i64 and `Float` as f64.
- `Decimal` as `sqlx::postgres::types::PgNumeric`, or via its text with an explicit `::numeric` placeholder cast, whichever sqlx 0.8 supports.
- `Bytes` as `&[u8]`.
- `Array` of homogeneous scalars as the matching Postgres array type; mixed arrays are `QUERY_ERROR`.

**Round-trip test.** Against Docker Postgres, `SELECT` then re-bind `$1` for each of:
- `9007199254740993::int8`
- `-9223372036854775808::int8`
- `'12.50'::numeric`
- `'NaN'::numeric`
- `'123456789012345678901234567890.123'::numeric`
- `'\x00ff10'::bytea`
- `'{"a":1}'::jsonb`
- `ARRAY[1,2]`
- `'nan'::float8`

Assert that the `Value` equals the expected one, and that `SELECT $1 = <literal>` returns true when binding the decoded value back.

**Checkpoint.** Suggested commit: `Decode and bind exact Postgres values`.

---

## Part D — Core and the wire

### Task 8: Core exposes the engine

**Files:** `crates/seaquel-core/src/lib.rs`, `crates/seaquel-core/tests/`

- The connections map keeps the engine alongside the driver: `(Arc<dyn Engine>, Arc<dyn Driver>)`.
- Add:
  - `Core::engine(&self, connection_id) -> Result<Arc<dyn Engine>, DbError>` and `Core::with_dialect(&self, connection_id, |d: &dyn Dialect| …) -> Result<R, DbError>`. `with_dialect` returns `NOT_SUPPORTED` when the engine has no dialect. (As implemented; a `&dyn Dialect` can't outlive the cloned engine `Arc`.)
  - Pass-throughs for the five introspection methods.
- Tests use the mock driver/engine from `tests/mock.rs`, extended with a mock dialect.

**Checkpoint.** Suggested commit: `Core exposes dialects and introspection`.

---

### Task 9: `seaquel-rpc` and the `db_engine` endpoint

**Files:**
- Create: `crates/seaquel-rpc/{Cargo.toml,src/lib.rs,tests/dispatch.rs}`. Classify it as `interface-glue` in `scripts/check-crate-deps.mjs`: it may depend on core, types and engine. Also add it to `INTERFACE_MAY_USE`.
- Modify: `src-tauri` (add command `db_engine`), `crates/seaquel-server` (add route `POST /api/db/engine`)
- Modify: `src/routes/api/db/[...path]/+server.ts` (add `"engine"` to `VALIDATED_PATHS`)

**The request and response types:**

```rust
#[derive(Deserialize, ts_rs::TS)]
pub struct EngineCall { pub connection_id: String, pub request: EngineRequest }

#[derive(Deserialize, ts_rs::TS)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum EngineRequest {
    ListSchemas,
    SchemaTables,
    TableMetadata { schema: String, table: String },
    Statistics,
    Explain { sql: String, params: Vec<Value>, analyze: bool },
    ColumnTypes,
    Paginate { sql: String, limit: u64, offset: u64 },
    BuildUpdate { schema: String, table: String, column: String, value: Value, primary_keys: Vec<String>, row: RowValues, casts: Option<CastMap> },
    BuildSetDefault { schema: String, table: String, column: String, primary_keys: Vec<String>, row: RowValues },
    BuildInsert { schema: String, table: String, values: RowValues, casts: Option<CastMap> },
    BuildDelete { schema: String, table: String, primary_keys: Vec<String>, row: RowValues },
    CreateTable { definition: CreateTableDefinition },
    AlterTable { from: CreateTableDefinition, to: CreateTableDefinition },
}

#[derive(Serialize, ts_rs::TS)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum EngineResponse { Schemas(Vec<String>), Tables(Vec<SchemaTable>), TableMetadata { columns: Vec<SchemaColumn>, indexes: Vec<SchemaIndex> }, Statistics(DatabaseStatistics), Explain(ExplainResult), ColumnTypes(Vec<ColumnTypeInfo>), Sql(String), SqlWithBindings(SqlWithBindings) }

pub async fn dispatch(core: &Core, call: EngineCall) -> Result<EngineResponse, DbError>;
```

- On the wire, `RowValues` is an array of `[column, value]` pairs, which preserves order.
- The generated TS types go to `src/lib/types/generated/` via `npm run types:gen`. Extend that script to cover `seaquel-rpc` too: `-p seaquel-types -p seaquel-rpc`.
- **Tests:** dispatch each variant against a Core with the sqlite engine plus the mock dialect. Server integration tests: one `POST /api/db/engine` happy path, plus `CONNECTION_NOT_FOUND` mapping to 404, as the other routes do.
- Also add a proxy unit test next to the existing ones for `/api/db/engine`: the connection id gets scoped, and a foreign prefix gets a 403.

**Checkpoint.** Suggested commit: `Add seaquel-rpc and the db_engine endpoint`.

---

## Part E — Frontend

### Task 10: `EngineClient`

**Files:**
- Create: `src/lib/engine/types.ts`, the `EngineClient` interface. It has one async method per `EngineRequest` variant, taking and returning the generated types.
- Create: `src/lib/engine/rust-engine-client.ts`. It calls `invoke("db_engine", { call })` on Tauri, or `POST /api/db/engine` on web. It uses the same transport-selection helper the providers use (`isTauri()`/`isWeb()`), encodes params with `encodeParam`, and decodes `SqlWithBindings.bindValues` with `decodeCell`.
- Create: `src/lib/engine/ts-engine-client.ts`. It wraps `getAdapter(type)` plus the provider and connection id, implementing every method with today's call-site logic:
  - get query, then select, then parse
  - builders are direct
  - `listSchemas` runs `getSchemasQuery`
- Create: `src/lib/engine/index.ts`, with `getEngineClient(connection: DatabaseConnection): EngineClient`. It returns Rust when `connection.type === "postgres"` and not in demo mode, and TS otherwise.
- Create: `src/lib/engine/ts-engine-client.test.ts`, which uses a fake provider and checks that each method calls the adapter the way the old call sites did.

**Checkpoint.** Suggested commit: `Add EngineClient with Rust and TS implementations`.

---

### Task 11: Value-aware UI

Each site from the value survey learns about `bigint`, `Uint8Array` and `SqlDecimal`. Import helpers from `$lib/values`.

| # | Site | Change |
|---|---|---|
| 1 | `src/lib/utils/cell-type.ts` `detectCellType` | `bigint` becomes `integer`, `SqlDecimal` becomes `float`, and `Uint8Array` becomes `binary` |
| 2 | `cell-type.ts` `getFormattedCellText`, `stringify`, `formatByteSize` | bigint goes through `Intl.NumberFormat`, which accepts bigint. `SqlDecimal` shows `.value`. `formatByteSize` takes a `Uint8Array` (`byteLength`) and keeps a base64-string branch for MSSQL |
| 3 | `components/formatted-cell.svelte` | integer and float: format bigint and `SqlDecimal` without `Number()`. binary: `toHex` preview (first 16 bytes) plus size |
| 4 | `components/editable-cell.svelte` | the edit text is `String(value)`, or `toHex` for bytes; objects use `JSON.stringify(v, null, 2)` via `jsonReplacer` |
| 5 | `components/virtual-results-table.svelte:337,358,396` | `cellKey`/`String` for strikethrough and FK filters; bytes as hex |
| 6 | `utils/clipboard.ts` | `JSON.stringify(…, jsonReplacer)`; hex for bytes |
| 7 | `utils/export-formats.ts` | JSON with `jsonReplacer`. CSV uses `String` with hex for bytes. `escapeSQLValue`: bigint and `SqlDecimal` unquoted; bytes as `'\x…'::bytea` for Postgres (keep the current quoting for others) |
| 8 | `components/command-palette.svelte` export/copy | use the `export-formats.ts` functions instead of its duplicate CSV/JSON code |
| 9 | `components/charts/chart-utils.ts`, `query-chart.svelte`, `chart-node.svelte`, `dashboard/dashboard-kpi-widget.svelte` | a single `toNumber(v)` helper in `values.ts`: bigint uses `Number()`, which is accepted for charts, `SqlDecimal` uses `Number(value)`, and others behave as today |
| 10 | `dashboard-widget-editor.svelte:363`, `widget-chart-config.svelte:69` | `String` works for all three; verify and leave |
| 11 | `services/ai/context.ts:73` | `String`; bytes become `<n bytes>` |
| 12 | PK matching: `query-editor/view-state.svelte.ts:100,125`, `data-viewer.svelte:116,135`, `hooks/database/pending-changes.svelte.ts:78` | `cellKey(a) === cellKey(b)` |
| 13 | Edit write-back, `query-execution.svelte.ts:1086-1092` | keep writing the edited string (unchanged); PK values are kept as the decoded JS values, so they round-trip exactly through `encodeParam` |
| 14 | `workflow/nodes/result-node.svelte:51` | goes through `clipboard.ts` (site 6) |

**Tests (vitest, new):**
- `cell-type.test.ts` for detection and formatting of every new JS type.
- `export-formats.test.ts` for JSON, CSV and SQL with bigint, Uint8Array and SqlDecimal.
- `values.test.ts` for `toNumber` and `cellKey`.

The DuckDB-WASM demo produces `bigint`/`Uint8Array` today and crashes JSON export. Its JSON export must now work: add a test that feeds a bigint row through `rowsToJSON`.

**Checkpoint.** Suggested commit: `Handle exact values across the UI`.

---

### Task 12: Move call sites onto `EngineClient`

Use the call-site map from the phase 1 research (sites A–G). Every site below changes from "adapter + provider" to `getEngineClient(connection)`.

| Site | New call |
|---|---|
| `connection-manager.svelte.ts` `add` :282, `reconnect` :448, `refreshSchema` :892 | `client.schemaTables()` |
| `connection-manager.svelte.ts` `addDemoConnection` :726 | keep the explicit DuckDB/TS path, but through `TsEngineClient` |
| `onSchemaLoaded` callback signatures (`connection-manager.svelte.ts:55-60`, `database.svelte.ts:310`) | pass `EngineClient` instead of `DatabaseAdapter` + provider connection id |
| `schema-tabs.svelte.ts` `fetchTableMetadata` :38, `refreshTabMetadata` :131, `loadTableMetadataInBackground` :164 | `client.tableMetadata(schema, table)`. Keep the per-table parallelism; a batch call is a later optimisation |
| `statistics-tabs.svelte.ts` `loadStatistics` :101 | `client.statistics()`. Keep the SQLite per-table row-count loop inside `TsEngineClient.statistics`. **Use the tab's connection**, not the active one; this fixes the latent mismatch the research found |
| `explain-tabs.svelte.ts` `performExplain` :81 | `client.explain(sql, binds, analyze)`. Keep the SQLite timing and the MSSQL/DuckDB bind skipping inside `TsEngineClient.explain` |
| `query-crud.svelte.ts` :66, :147, :223, :276 | `client.buildUpdate/…`. `buildCastLookup` becomes `buildCastMap(): Record<string,string>` with the same exclusions; `TsEngineClient` turns it back into a `CastLookup` |
| `query-execution.svelte.ts` `executeStatement` :450 | `await client.paginate(baseQuery, pageSize + 1, offset)` |
| `create-table-tabs.svelte.ts` `generateSql` :179, `executeCreate` :202 | `await client.createTable(def)` / `client.alterTable(orig, def)`. `generateSql` becomes async; update its callers |
| `create-table-view.svelte` :41-42 | `columnTypes` is loaded once per connection with `client.columnTypes()` into `$state` |
| `create-table-view.svelte` :55 | `client.listSchemas()` for the tab's connection, not the active one |
| `create-table-view.svelte` :89-92 | the DDL preview `$effect` becomes debounced (150 ms) and async. It ignores stale responses with a request counter, so a slow reply can't overwrite a newer preview |

**Guidance from the Task 10 review:**
- Call `getEngineClient(connection, this.state)` (managers) or `getEngineClient(connection, db.state)` (components). With `state`, the client looks the live connection id up by connection `id` on every call, which survives `reconnect()` replacing the connection object. Call it where `getAdapter` was called, and inside the existing try blocks. The TS client resolves its adapter eagerly and can throw.
- Don't cache clients across operations. `onSchemaLoaded` may use the client it's given for the immediate background load only.
- `addDemoConnection`: `new TsEngineClient({ type: "duckdb", …, getProvider: () => this.providers.getOrCreateDuckDB() })`.
- DDL generation is now async and can fail over the wire. Wrap `createTable`/`alterTable` in try/catch with `errorToast`, and keep the `"-- No changes detected"` check and its `toast.info`.
- `create-table-view`: keep the schema-load catch (`availableSchemas = []`) and "keep the current sqlText on error". Load `columnTypes` after connect, and reload it when the connection id changes.
- `query-crud`: `buildCastMap` keeps the exclusions and returns `undefined` when there's no connection or table.
- `executeStatement`: build one client per call, not per page.
- Statistics' error text changes from "Not connected to database" to "No connection established"; either is fine.

Keep `parseCreateTableSql` (create-table-view :117) in TypeScript. It's dialect-agnostic and moves in phase 2 with `seaquel-sql`.

**Verify:**
- `npm run check` and `npx vitest run`.
- Grep: no file outside `src/lib/engine/` and `src/lib/db/` calls `getAdapter(`.
- **Manual (desktop, `npm run tauri:dev`, Docker Postgres):**
  - The schema tree loads, including a table named `order items`.
  - Column and index details show, including an expression index.
  - Statistics tab.
  - EXPLAIN and EXPLAIN ANALYZE.
  - Edit a cell and save, with pending changes both on and off.
  - Insert a row and delete a row.
  - Create a table, then alter it.
  - A row keyed by a `uuid` (and one by a `timestamp`) can be edited, set to NULL in a date/timestamp column, set to default and deleted.
  - A `bigint` PK row with id `9007199254740993` can be edited and deleted.
  - bytea shows as hex and NUMERIC keeps its scale.
- **Manual (web, `npm run dev:web:full`):** the same flow, briefly.
- **Manual (demo, `npm run dev:demo`):** DuckDB schema, stats and JSON export all still work.

**Checkpoint.** Suggested commit: `Route dialect calls through EngineClient`.

---

### Task 13: Delete the TypeScript Postgres adapter

- Remove `src/lib/db/postgres.ts` and its registry entry in `src/lib/db/index.ts`.
- `getAdapter("postgres")` now throws the existing "not supported yet" error. `getEngineClient` never asks for it; a unit test asserts that.
- Keep `alter-table.ts` and `crud-helpers.ts`; the other dialects still use them.
- Delete `scripts/fixtures/postgres-dialect.fixtures.test.ts`, whose only input was the deleted adapter.
  - Keep the JSON fixtures: the Rust parity tests still use them as regression tests.
  - Leave a `README.md` in the fixtures directory saying where they came from and that they're now frozen.

**Checkpoint.** Suggested commit: `Remove the TypeScript Postgres adapter`.

---

## Part F — Close-out

### Task 14: CI

- `.github/workflows/ci.yml`:
  - The engines job runs `run_introspection`, which comes automatically through `smoke.rs`.
  - `types:gen` now covers `seaquel-rpc`; the "generated types are up to date" step works unchanged.
- `scripts/check-crate-deps.mjs` classifies `seaquel-rpc` (Task 9); `npm run crates:check` must pass.
- The fixture recorder isn't in CI: it's deleted in Task 13.

**Checkpoint.** Suggested commit: `CI for phase 1`.

---

### Task 15: Docs

- `CLAUDE.md`:
  - `EngineClient` is the only way to do dialect work from the UI.
  - The Value wire format.
  - Postgres now lives in Rust.
- The design doc gets phase 1 status plus what changed from the design:
  - The trait names are `Driver` and `Dialect`; there's no `Connection`.
  - The RPC started in phase 1, and it's connection-scoped.
  - The Value wire format.

**Checkpoint.** Suggested commit: `Document phase 1`.

---

### Task 16: Measure and re-plan

From `docs/plans/2026-09-25-phase-1-effort.md`, write a **"Phase 1 cost"** section into the design doc:
- time per part
- Rust lines added vs TS lines removed
- how many parity cases caught a real difference
- what was harder than expected

Then write a realistic estimate for porting MySQL/MariaDB, SQLite, MSSQL and DuckDB in phase 2. List what's reusable now: the generic builders, `EngineClient`, the Value format, the fixture recorder pattern. Say whether phase 2 should keep the per-engine order proposed in the design.

Final verification: run the full check list from the ground rules and the manual list from Task 12, then report every result.

---

## Execution notes (2026-09-25)

The plan was executed task by task with a review after each. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-09-25-phase-1-effort.md`; the measured cost and the phase 2 estimate are in the design doc under "Phase 1 cost".

- **Bug fix 5 was found while recording fixtures (Task 3).** The table-sizes query built `schemaname || '.' || relname` for `pg_*_size()`, so the Statistics view failed on any table name that needs quoting. Only that one query spliced names; index usage already used `indexrelid`. Fixed with `relid` and checked live in `run_introspection`.
- **Fix 3:** `DROP INDEX` is qualified with the original definition's schema, where the index lives.
- **Fix 4:** plain index key columns come back as `pg_attribute.attname`, unquoted (`"Mixed Case"` → `Mixed Case`). `pg_get_indexdef(…, k, true)` alone returns the quoted form, which the DDL generator would have quoted a second time. Expressions print as `lower(name::text)`, not `lower((name)::text)`.
- **sqlx statement cache (Task 7 review).** sqlx caches prepared statements per connection keyed by SQL text alone and reuses the first prepare's parameter types. Once `Int` and `Float` bound as different types, the same SQL run later with a float wrote its bits as INT8 on Postgres (1.5 became 4609434218613702656), and text after an int failed on Postgres and read back as `Int(0)` on MySQL. `impl_sqlx_driver!` marks every statement `persistent(false)`, and `run_smoke` alternates Int/Float/Text on one SQL for 20 rounds on every engine. MSSQL and DuckDB don't cache and pass.
- **UUID cells were NULL before phase 1.** sqlx's `uuid` feature is off and a UUID isn't a `String` to sqlx. UUID and UUID[] now use a 16-byte decoder.
- **Old Postgres decoder bugs, fixed in Task 7:** NUMERIC with 30 digits panicked (rust_decimal overflow); float and numeric NaN became null; one NULL element nulled the whole array; DATE[]/JSON[]/BYTEA[] came back as binary garbage text. Arrays with no decoder, multi-dimensional arrays and arrays not starting at 1 are now `<unsupported: T[]>` text.
- **NUMERIC binary codec.** sqlx 0.8's `PgNumeric` is `pub(crate)`, and `rust_decimal`/`bigdecimal` lose NaN, Infinity, digits beyond 28 or the display scale. `numeric.rs` reads and writes the binary wire format by hand; no sqlx features changed.
- **Integer arrays** bind as INT4[] when every element fits and INT8[] otherwise, because Postgres has no `int8[] = int4[]`.
- **Persisted bigints (Task 11 review).** Saved workflows keep result rows and were stringified without a replacer, so one bigint cell aborted the whole project save. They now go through `toStorable`/`fromStorable`. `persistProjectDashboards` skipped `stripWidgetRuntimeState`; all three dashboard save paths now share `toPersistedDashboard`.
- **Pagination is local (final review).** `RustEngineClient.paginate` used to be a full `EngineCall` round trip before every paged SELECT, just to append `LIMIT … OFFSET …`. It now computes the SQL in TypeScript, byte for byte with `PostgresDialect::paginate` and tested against `crates/seaquel-engine-postgres/tests/fixtures/paginate.json`. The client takes its `RustEngine` type from `getEngineClient`, so a future non-Postgres Rust engine won't compile without its own paginate. Phase 2 should move this into seaquel-wasm. The CRUD builders are still round trips.
- **Live connection id (Task 10 review).** `reconnect()` replaces the connection object in `state.connections`, so capturing either the object or its id goes stale. Engine clients take `getConnectionId()` and `getEngineClient(connection, state)` looks the connection up by `id` on every call.
- **Create-table preview needs a connection for Postgres.** DDL generation is a round trip now. A disconnected Postgres tab shows "Connect to preview SQL"; TS-engine tabs still preview offline.
- **`TsEngineClient` resolves its adapter lazily (Task 13).** It used to resolve in the constructor, so once `getAdapter("postgres")` threw, building a client for a Postgres connection in the demo would have thrown.
- **Core API.** `Core::dialect(id) -> &dyn Dialect` can't outlive the connections lock, so Core has `engine(id)` and `with_dialect(id, |d| …)`.
- **The fixture recorder was never committed.** It was created in Task 3 and deleted in Task 13 with no commit in between. A reference copy is saved at `docs/plans/artifacts/2026-09-25-postgres-fixture-recorder.ts.txt` for phase 2 to adapt; the fixtures README points there.
- **Task 14** has no line in the effort log.
- **Bug fix 6 was found in review after Task 13.** uuid/date/timestamp/time keys and "Set NULL" on typed columns failed on Postgres (see "Bug fixes"). Key placeholders take the cast map, NULL binds as TEXT, and set-default/delete now carry the cast map over the wire. Checked live in `smoke.rs`, which failed first with `uuid = text` and `cannot cast type jsonb to date`.
- **Bug fix 7 closed two of fix 6's follow-ups** (see "Bug fixes"): enum and array columns cast to the catalog type, and CRUD loads a table's columns when they aren't cached. The design asked for `format_type` verbatim; a live check showed explicit casts to `varchar(n)`, `char(n)`, `bit(n)` and domains over them truncate or pad silently, so the SQL drops those lengths (assignment then checks them). Review then found three gaps, fixed: a domain over a domain over `varchar(n)` still truncated (only one `typbasetype` level was checked; now the whole chain), `format_type` left types on the loading connection's search_path unqualified (user types are now qualified from the catalog), and a table missing from the cached list was loaded on every edit, twice for concurrent edits, and could overwrite a newer refresh (now a deduplicated per-connection cache, forgotten on schema reloads). `smoke.rs` failed first with an empty cast map, and the vitest cases with `undefined` casts.
- **Bug fix 6 review round trip.** A scratch program updated one row by key for ~45 Postgres key types (decoded cell → cast map → UPDATE). Everything but `xml` (no btree) now affects exactly one row; the fixes are listed under bug fix 6. The same run showed a TEXT NULL can't be assigned uncast to json/jsonb (JSONB NULL could), which only matters when the table isn't in the schema cache and there's no cast map (bug fix 7 always builds one now). An untyped NULL (OID 0) would assign to anything, but breaks `SELECT $1 IS NULL` ("could not determine data type"), and query-editor parameters send an empty value as NULL, so `:x IS NULL` filters would fail. TEXT NULL stays.

**Manual checks outstanding.** None of these has been done by a person:

- Desktop (`npm run tauri:dev`, Docker Postgres):
  - The schema tree loads, including a table named `order items`.
  - Column and index details show, including an expression index.
  - Statistics tab.
  - EXPLAIN and EXPLAIN ANALYZE.
  - Edit a cell and save, with pending changes on and off.
  - Insert a row and delete a row.
  - Create a table, then alter it.
  - A row keyed by a `uuid` (and one by a `timestamp`) can be edited, set to NULL in a date/timestamp column, set to default and deleted.
  - "Set NULL" on an enum, an `int[]` and a `jsonb` column (bug fix 7), including right after connecting, before the background metadata load has reached that table.
  - A `bigint` PK row with id `9007199254740993` can be edited and deleted. (Checked headlessly in Task 12 through the real client code against `seaquel-server`, not in the GUI.)
  - bytea shows as hex and NUMERIC keeps its scale.
- Web (`npm run dev:web:full`): the same flow, briefly.
- Demo (`npm run dev:demo`): DuckDB schema, stats and JSON export.
- Save a project with a workflow and a dashboard whose results hold a bigint, reload, and check the values.
- Edit and delete a row keyed by a blob PK on DuckDB and on MSSQL (inline literals `'\xAB\x01'::BLOB` and `0xAB01`).
- The first CI run on GitHub.

**Open follow-ups:**

- tsvector/tsquery decoding is untested.
- `xid`, `cid`, `tid` and the `reg*` types have no decoder and fall back to reading their binary form as text (garbage); OID was fixed in bug fix 6.
- `listSchemas` includes `pg_temp_*` and `pg_toast_temp_*`, as the TS did.
- `run_introspection` in the testkit is Postgres-specific SQL; other engines need their own scratch schema.
- rustfmt isn't enforced and the workspace isn't rustfmt-clean; `cargo fmt -p` reformats files other tasks touched.
- The `introspection = { … }` hook in `impl_sqlx_driver!` could become a trait the macro delegates to.
