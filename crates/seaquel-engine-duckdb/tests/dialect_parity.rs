//! Parity between `DuckdbDialect` and the demo's TypeScript DuckDB adapter
//! (`src/lib/db/duckdb.ts`), recorded into `tests/fixtures/*.json` (frozen;
//! see `tests/fixtures/README.md`), and the catalog SQL of
//! `seaquel_engine_duckdb::introspect`.
//!
//! One test per fixture file. Each runs every case, then fails once with the
//! name, input, expected and actual output of every case that differed.
//!
//! Intended differences live in `fixtures/bugfixes.json` (fixes 1, 4, 6 and
//! 8 to 12 here; the README lists them all). A bug-fix case with `replaces`
//! supersedes the recorded case of that name: its output is expected
//! instead (and its input, when it has one), and the recorded TypeScript
//! output must differ from it, so the replacement documents a real change.
//! The replacement counts are pinned per file.
//!
//! CRUD isn't recorded (the Rust dialect binds `?` parameters where the
//! adapter inlined literals); `tests/smoke.rs` runs it live.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value as Json;

use seaquel_engine::{Dialect, Engine};
use seaquel_engine_duckdb::{introspect, DuckdbDialect};
use seaquel_types::{ColumnTypeInfo, CreateTableColumn, CreateTableDefinition};

// ── Harness ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Fixture<I> {
    cases: Vec<Case<I>>,
}

#[derive(Deserialize)]
struct Case<I> {
    name: String,
    input: I,
    output: Json,
}

fn load<I: DeserializeOwned>(file: &str, text: &str) -> Vec<Case<I>> {
    serde_json::from_str::<Fixture<I>>(text)
        .unwrap_or_else(|e| panic!("{file}: cannot parse fixture: {e}"))
        .cases
}

fn engine() -> Arc<dyn Engine> {
    seaquel_engine_duckdb::engine()
}

fn dialect(engine: &Arc<dyn Engine>) -> &dyn Dialect {
    engine
        .dialect()
        .expect("DuckdbEngine::dialect() returns the DuckDB dialect")
}

#[derive(Deserialize)]
struct BugfixFixture {
    cases: Vec<BugfixCase>,
}

#[derive(Deserialize)]
struct BugfixCase {
    name: String,
    fix: u8,
    kind: String,
    #[serde(default)]
    replaces: Option<String>,
    input: Json,
    output: Json,
}

fn bugfixes() -> Vec<BugfixCase> {
    serde_json::from_str::<BugfixFixture>(include_str!("fixtures/bugfixes.json"))
        .expect("bugfixes.json")
        .cases
}

/// Swap in the bug-fix outputs (and inputs, when not null) for `file`'s
/// replaced cases. Every replacement must name a recorded case and change
/// its output. Returns how many were replaced.
fn apply_replacements<I: DeserializeOwned>(file: &str, cases: &mut [Case<I>]) -> usize {
    let prefix = format!("{file}: ");
    let mut replaced: HashMap<String, (Json, Json)> = bugfixes()
        .into_iter()
        .filter_map(|c| {
            let name = c.replaces?.strip_prefix(&prefix)?.to_string();
            Some((name, (c.input, c.output)))
        })
        .collect();
    let mut n = 0;
    for case in cases.iter_mut() {
        if let Some((input, output)) = replaced.remove(&case.name) {
            assert_ne!(
                output, case.output,
                "{file}: bug fix for {:?} doesn't change the recorded output",
                case.name
            );
            case.output = output;
            if !input.is_null() {
                case.input = serde_json::from_value(input).unwrap_or_else(|e| {
                    panic!("{file}: replacement input of {:?}: {e}", case.name)
                });
            }
            n += 1;
        }
    }
    assert!(
        replaced.is_empty(),
        "{file}: bugfixes.json replaces cases that weren't recorded: {:?}",
        replaced.keys().collect::<Vec<_>>()
    );
    n
}

/// Runs `check` on every case and panics once, listing every mismatch.
/// `check` returns `(input, expected, actual)` as printable text.
fn run<I>(
    file: &str,
    cases: &[Case<I>],
    mut check: impl FnMut(&Case<I>) -> Option<(String, String, String)>,
) -> usize {
    assert!(!cases.is_empty(), "{file}: no cases");
    let mut failures = Vec::new();
    for case in cases {
        if let Some((input, expected, actual)) = check(case) {
            failures.push(format!(
                "case {:?}\n  input:    {input}\n  expected: {expected}\n  actual:   {actual}",
                case.name
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{file}: {} of {} cases differ\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
    cases.len()
}

fn expect_str(output: &Json) -> &str {
    output.as_str().expect("fixture output is a string")
}

fn compare_str(input: String, expected: &str, actual: String) -> Option<(String, String, String)> {
    (expected != actual).then(|| (input, format!("{expected:?}"), format!("{actual:?}")))
}

// ── quote.json ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct QuoteInput {
    id: String,
}

#[test]
fn quote() {
    let file = "quote.json";
    let cases = load::<QuoteInput>(file, include_str!("fixtures/quote.json"));
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.quote_ident(&c.input.id),
        )
    });
    assert_eq!(n, 8);
}

/// Bug fix 6's DDL side: a listed schema is quoted part by part.
#[test]
fn quote_schema() {
    let e = engine();
    let d = dialect(&e);
    for (schema, expected) in [
        ("main", "\"main\""),
        ("\"a.b\"", "\"a.b\""),
        ("fx_aux.main", "\"fx_aux\".\"main\""),
        ("\"fx.we\"\"ird\".main", "\"fx.we\"\"ird\".\"main\""),
        ("\"fx_aux.main\"", "\"fx_aux.main\""),
    ] {
        assert_eq!(d.quote_schema(schema), expected, "{schema}");
    }
}

// ── paginate.json ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PaginateInput {
    sql: String,
    limit: u64,
    offset: u64,
}

#[test]
fn paginate() {
    let file = "paginate.json";
    let cases = load::<PaginateInput>(file, include_str!("fixtures/paginate.json"));
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            d.paginate(&i.sql, i.limit, i.offset),
        )
    });
    assert_eq!(n, 5);
}

// ── column-types.json ────────────────────────────────────────────────────────

/// Fix 12 replaces the list (no bare ARRAY, LIST, MAP, STRUCT or UNION).
#[test]
fn column_types() {
    let file = "column-types.json";
    let mut cases = load::<Json>(file, include_str!("fixtures/column-types.json"));
    assert_eq!(apply_replacements(file, &mut cases), 1);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let expected: Vec<ColumnTypeInfo> =
            serde_json::from_value(c.output.clone()).expect("output is ColumnTypeInfo[]");
        let actual = d.column_types();
        (expected != actual).then(|| {
            (
                c.input.to_string(),
                serde_json::to_string(&expected).unwrap(),
                serde_json::to_string(&actual).unwrap(),
            )
        })
    });
    assert_eq!(n, 1);
}

// ── sql.json ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SqlInput {
    query: String,
    #[serde(default)]
    sql: String,
    #[serde(default)]
    analyze: bool,
    #[serde(default)]
    catalog: String,
    #[serde(default)]
    schema: String,
    #[serde(default)]
    table: String,
}

fn catalog_sql(d: &dyn Dialect, i: &SqlInput) -> String {
    match i.query.as_str() {
        "getSchemaQuery" => introspect::schema_sql(),
        "getSchemasQuery" => introspect::schemas_sql(),
        "getColumnsQuery" => introspect::columns_sql(),
        "getForeignKeysQuery" => introspect::foreign_keys_sql(),
        "getIndexesQuery" => introspect::indexes_sql(),
        "getTableSizesQuery" => introspect::table_sizes_sql(),
        "getIndexUsageQuery" => introspect::index_usage_sql(),
        "getDatabaseOverviewQuery" => introspect::OVERVIEW_SQL.to_string(),
        "getTableRowCountQuery" => introspect::row_count_sql(&i.catalog, &i.schema, &i.table),
        "getExplainQuery" => d.explain_sql(&i.sql, i.analyze),
        other => panic!("unknown query {other:?}"),
    }
}

/// Every catalog query but `getExplainQuery` is replaced by its fixed text
/// (fixes 1, 2, 3, 6 and 15): bound `$1`/`$2`, catalogs told apart,
/// `duckdb_indexes()` and `pragma_database_size()`.
#[test]
fn sql() {
    let file = "sql.json";
    let mut cases = load::<SqlInput>(file, include_str!("fixtures/sql.json"));
    assert_eq!(apply_replacements(file, &mut cases), 9);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            catalog_sql(d, &c.input),
        )
    });
    assert_eq!(n, 13);
}

/// The hand-written `sql` case that doesn't replace a recorded one (a row
/// count in an attached catalog, a name with quotes).
#[test]
fn bugfixes_sql() {
    let file = "bugfixes.json";
    let cases: Vec<Case<SqlInput>> = bugfixes()
        .into_iter()
        .filter(|c| c.kind == "sql" && c.replaces.is_none())
        .map(|c| Case {
            name: c.name,
            input: serde_json::from_value(c.input).expect("sql input"),
            output: c.output,
        })
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            catalog_sql(d, &c.input),
        )
    });
    assert_eq!(n, 1);
}

// ── ddl-create.json / ddl-alter.json / ddl-add-column.json ──────────────────

#[derive(Debug, Deserialize)]
struct AlterInput {
    from: CreateTableDefinition,
    to: CreateTableDefinition,
}

#[derive(Debug, Deserialize)]
struct AddColumnInput {
    schema: String,
    table: String,
    column: CreateTableColumn,
}

/// Fixes 1 (`"` doubled) and 11 (a foreign key without a referenced schema
/// names the table's own) replace two cases.
#[test]
fn ddl_create() {
    let file = "ddl-create.json";
    let mut cases = load::<CreateTableDefinition>(file, include_str!("fixtures/ddl-create.json"));
    assert_eq!(apply_replacements(file, &mut cases), 2);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.create_table(&c.input),
        )
    });
    assert_eq!(n, 9);
}

/// Fixes 1, 4, 8, 9 and 10 replace twelve recorded cases.
#[test]
fn ddl_alter() {
    let file = "ddl-alter.json";
    let mut cases = load::<AlterInput>(file, include_str!("fixtures/ddl-alter.json"));
    assert_eq!(apply_replacements(file, &mut cases), 12);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            d.alter_table(&i.from, &i.to),
        )
    });
    assert_eq!(n, 19);
}

/// The TS adapter's `generateAddColumnSql`. It isn't on the `Dialect` trait
/// (nothing calls it over the wire), so the crate exposes it directly.
/// Fixes 1 and 9 replace two cases.
#[test]
fn ddl_add_column() {
    let file = "ddl-add-column.json";
    let mut cases = load::<AddColumnInput>(file, include_str!("fixtures/ddl-add-column.json"));
    assert_eq!(apply_replacements(file, &mut cases), 2);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            DuckdbDialect.add_column(&i.schema, &i.table, &i.column),
        )
    });
    assert_eq!(n, 5);
}

// ── bugfixes.json ────────────────────────────────────────────────────────────

/// The hand-written DDL cases that don't replace a recorded one.
#[test]
fn bugfixes_ddl() {
    let file = "bugfixes.json";
    let cases: Vec<Case<Json>> = bugfixes()
        .into_iter()
        .filter(|c| c.kind.starts_with("ddl-") && c.replaces.is_none())
        .map(|c| {
            assert!(
                matches!(c.fix, 4 | 6 | 9 | 10 | 11),
                "{file}: DDL case {:?} has fix {}",
                c.name,
                c.fix
            );
            Case {
                name: format!("[{}] {}", c.kind, c.name),
                input: c.input,
                output: c.output,
            }
        })
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let actual = if c.name.starts_with("[ddl-create]") {
            let def: CreateTableDefinition = serde_json::from_value(c.input.clone()).unwrap();
            d.create_table(&def)
        } else if c.name.starts_with("[ddl-add-column]") {
            let i: AddColumnInput = serde_json::from_value(c.input.clone()).unwrap();
            DuckdbDialect.add_column(&i.schema, &i.table, &i.column)
        } else {
            let i: AlterInput = serde_json::from_value(c.input.clone()).unwrap();
            d.alter_table(&i.from, &i.to)
        };
        compare_str(c.input.to_string(), expect_str(&c.output), actual)
    });
    assert_eq!(n, 12);
}

/// Every case kind in `bugfixes.json` is checked somewhere: here, in
/// `tests/introspect_parity.rs` or live in `tests/smoke.rs`.
#[test]
fn bugfix_kinds_are_known() {
    const KINDS: &[&str] = &[
        "sql",
        "ddl-create",
        "ddl-alter",
        "ddl-add-column",
        "column-types",
        "parse-schema",
        "parse-columns",
        "parse-indexes",
        "parse-stats",
        "parse-explain",
        "columns-fk-error",
        "schema",
        "schemas",
        "columns",
        "indexes",
        "table-sizes",
    ];
    let all = bugfixes();
    for c in &all {
        assert!(
            KINDS.contains(&c.kind.as_str()),
            "{:?}: kind {}",
            c.name,
            c.kind
        );
        assert!((1..=15).contains(&c.fix), "{:?}: fix {}", c.name, c.fix);
    }
    assert_eq!(all.iter().filter(|c| c.replaces.is_some()).count(), 79);
}
