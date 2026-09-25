//! Parity between the SQLite introspection parsers and SQL
//! (`seaquel_engine_sqlite::introspect`) and the TypeScript SQLite adapter
//! (deleted in phase 2), recorded into `tests/fixtures/` (frozen).
//!
//! Parse inputs are the rows the adapter's own SQL returned through Seaquel's
//! Rust driver, stored in the Value wire format: cells are read with
//! `Value::from_wire`, and each result is rebuilt in its stored `columns`
//! order. Outputs are compared as typed structs, not raw JSON.
//!
//! A `bugfixes.json` case with `replaces` supersedes the recorded case of that
//! name (fixes 7, 8 and 9 change parser output): its output is expected
//! instead, and the recorded TypeScript output must differ from it.

use std::collections::HashMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value as Json};

use seaquel_engine::{QueryResult, Value};
use seaquel_engine_sqlite::introspect;
use seaquel_types::{
    DatabaseOverview, ExplainResult, IndexUsageInfo, SchemaColumn, SchemaIndex, SchemaTable,
    TableSizeInfo,
};

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

fn fixture(file: &str) -> &'static str {
    match file {
        "parse-schema.json" => include_str!("fixtures/parse-schema.json"),
        "parse-columns.json" => include_str!("fixtures/parse-columns.json"),
        "parse-indexes.json" => include_str!("fixtures/parse-indexes.json"),
        "parse-stats.json" => include_str!("fixtures/parse-stats.json"),
        "parse-explain.json" => include_str!("fixtures/parse-explain.json"),
        "bugfixes.json" => include_str!("fixtures/bugfixes.json"),
        "sql.json" => include_str!("fixtures/sql.json"),
        other => panic!("no fixture {other}"),
    }
}

fn load<I: DeserializeOwned>(file: &str) -> Vec<Case<I>> {
    serde_json::from_str::<Fixture<I>>(fixture(file))
        .unwrap_or_else(|e| panic!("{file}: cannot parse fixture: {e}"))
        .cases
}

#[derive(Deserialize)]
#[allow(dead_code)]
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
    #[derive(Deserialize)]
    struct Bugfixes {
        cases: Vec<BugfixCase>,
    }
    serde_json::from_str::<Bugfixes>(fixture("bugfixes.json"))
        .unwrap_or_else(|e| panic!("bugfixes.json: {e}"))
        .cases
}

/// The hand-written cases of `kind` that don't replace a recorded one.
fn bugfix_cases<I: DeserializeOwned>(kind: &str) -> Vec<Case<I>> {
    bugfixes()
        .into_iter()
        .filter(|c| c.kind == kind && c.replaces.is_none())
        .map(|c| Case {
            input: serde_json::from_value(c.input)
                .unwrap_or_else(|e| panic!("bugfixes.json {:?}: {e}", c.name)),
            name: c.name,
            output: c.output,
        })
        .collect()
}

/// Swap in the bug-fix outputs for `file`'s replaced cases. Every
/// replacement must name a recorded case and change its output. Returns how
/// many cases were replaced.
fn apply_replacements<I>(file: &str, cases: &mut [Case<I>]) -> usize {
    let prefix = format!("{file}: ");
    let mut replaced: HashMap<String, Json> = bugfixes()
        .into_iter()
        .filter_map(|c| Some((c.replaces?.strip_prefix(&prefix)?.to_string(), c.output)))
        .collect();
    let mut n = 0;
    for case in cases.iter_mut() {
        if let Some(output) = replaced.remove(&case.name) {
            assert_ne!(
                output, case.output,
                "{file}: the bug fix for {:?} doesn't change the recorded output",
                case.name
            );
            case.output = output;
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
fn run<I>(
    label: &str,
    cases: &[Case<I>],
    mut check: impl FnMut(&Case<I>) -> Option<String>,
) -> usize {
    assert!(!cases.is_empty(), "{label}: no cases");
    let mut failures = Vec::new();
    for case in cases {
        if let Some(diff) = check(case) {
            failures.push(format!("case {:?}\n{diff}", case.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{label}: {} of {} cases differ\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
    eprintln!("{label}: {} cases passed", cases.len());
    cases.len()
}

/// `Some(diff)` when `actual` differs from the fixture `output` read as `T`.
fn compare<T: DeserializeOwned + PartialEq + Debug>(output: &Json, actual: &T) -> Option<String> {
    let expected: T = serde_json::from_value(output.clone())
        .unwrap_or_else(|e| panic!("fixture output doesn't deserialize: {e}\n{output}"));
    (&expected != actual).then(|| format!("  expected: {expected:#?}\n  actual:   {actual:#?}"))
}

/// A recorded result: column names in result order, rows keyed by name,
/// cells in the Value wire format.
#[derive(Deserialize)]
struct Recorded {
    columns: Vec<String>,
    rows: Vec<Map<String, Json>>,
}

impl Recorded {
    fn result(&self) -> QueryResult {
        result_of(&self.columns, &self.rows)
    }
}

fn result_of(columns: &[String], rows: &[Map<String, Json>]) -> QueryResult {
    QueryResult {
        columns: columns.to_vec(),
        rows: rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|c| {
                        row.get(c).cloned().map_or(Value::Null, |j| {
                            Value::from_wire(j).unwrap_or_else(|e| panic!("cell {c}: {e}"))
                        })
                    })
                    .collect()
            })
            .collect(),
    }
}

// ── parse-schema.json ────────────────────────────────────────────────────────

#[test]
fn parse_schema() {
    let cases = load::<Recorded>("parse-schema.json");
    run("parse-schema.json", &cases, |c| {
        compare::<Vec<SchemaTable>>(&c.output, &introspect::parse_schema(&c.input.result()))
    });
}

// ── parse-columns.json ───────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColumnsInput {
    #[serde(flatten)]
    recorded: Recorded,
    #[serde(default)]
    foreign_key_columns: Vec<String>,
    #[serde(default)]
    foreign_keys: Option<Vec<Map<String, Json>>>,
}

impl ColumnsInput {
    fn parse(&self) -> Vec<SchemaColumn> {
        let fks = self
            .foreign_keys
            .as_ref()
            .map(|rows| result_of(&self.foreign_key_columns, rows));
        introspect::parse_columns(&self.recorded.result(), fks.as_ref())
    }
}

/// Fix 9 replaces the table whose foreign key names no parent column (the TS
/// showed `referencedColumn: null`; the recorded rows have a NULL `to`, which
/// the parser reads as `""`, and the fixed query resolves).
#[test]
fn parse_columns() {
    let file = "parse-columns.json";
    let mut cases = load::<ColumnsInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), 1);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaColumn>>(&c.output, &c.input.parse())
    });
    assert_eq!(n, 7);
}

/// Hand-written `parse-columns` cases: rows of the fixed foreign-key query.
#[test]
fn bugfixes_columns() {
    let cases = bugfix_cases::<ColumnsInput>("parse-columns");
    let n = run("bugfixes.json parse-columns", &cases, |c| {
        compare::<Vec<SchemaColumn>>(&c.output, &c.input.parse())
    });
    assert_eq!(n, 1);
}

// ── parse-indexes.json ───────────────────────────────────────────────────────

/// Fix 7 replaces the tables with autoindexes (`sqlite_autoindex_*`, which
/// the TS hid).
#[test]
fn parse_indexes() {
    let file = "parse-indexes.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 3);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaIndex>>(&c.output, &introspect::parse_indexes(&c.input.result()))
    });
    assert_eq!(n, 7);
}

/// Hand-written `parse-indexes` cases: rows of the fixed query, one per key
/// column (fix 5).
#[test]
fn bugfixes_indexes() {
    let cases = bugfix_cases::<Recorded>("parse-indexes");
    let n = run("bugfixes.json parse-indexes", &cases, |c| {
        compare::<Vec<SchemaIndex>>(&c.output, &introspect::parse_indexes(&c.input.result()))
    });
    assert_eq!(n, 1);
}

// ── parse-stats.json ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct StatsInput {
    kind: String,
    #[serde(flatten)]
    recorded: Recorded,
}

#[test]
fn parse_stats() {
    let file = "parse-stats.json";
    let cases = load::<StatsInput>(file);
    let n = run(file, &cases, |c| {
        let r = c.input.recorded.result();
        match c.input.kind.as_str() {
            "tableSizes" => {
                compare::<Vec<TableSizeInfo>>(&c.output, &introspect::parse_table_sizes(&r))
            }
            "indexUsage" => {
                compare::<Vec<IndexUsageInfo>>(&c.output, &introspect::parse_index_usage(&r))
            }
            "overview" => compare::<DatabaseOverview>(&c.output, &introspect::parse_overview(&r)),
            "rowCount" => compare::<i64>(&c.output, &introspect::parse_row_count(&r)),
            other => panic!("{file}: unknown kind {other:?}"),
        }
    });
    assert_eq!(n, 12);
}

// ── parse-explain.json ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExplainInput {
    analyze: bool,
    #[serde(flatten)]
    recorded: Recorded,
}

/// Fix 8 replaces the recorded plans whose details the TS fell back on.
#[test]
fn parse_explain() {
    let file = "parse-explain.json";
    let mut cases = load::<ExplainInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), EXPLAIN_REPLACED);
    let n = run(file, &cases, |c| {
        compare::<ExplainResult>(
            &c.output,
            &introspect::parse_explain(&c.input.recorded.result(), c.input.analyze),
        )
    });
    assert_eq!(n, 68);
}

const EXPLAIN_REPLACED: usize = 24;

// ── sql.json ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SqlInput {
    query: String,
    #[serde(default)]
    sql: String,
    #[serde(default)]
    table: String,
}

/// Every catalog query equals the recorded TS text except for bug fixes 1
/// (bound names; the PRAGMA statements become their table-valued
/// functions), 5 (index columns), 7 (autoindexes counted and listed), 9
/// (implicit foreign-key columns) and 10 (`sqlite\_` escaped). Each fix is
/// applied to the recorded text here, so a stray edit anywhere else fails.
#[test]
fn sql() {
    let file = "sql.json";
    let cases = load::<SqlInput>(file);
    const LIKE_TS: &str = "NOT LIKE 'sqlite_%'";
    const LIKE_FIXED: &str = "NOT LIKE 'sqlite\\_%' ESCAPE '\\'";
    let n = run(file, &cases, |c| {
        let recorded = c.output.as_str().expect("sql.json output is a string");
        let i = &c.input;
        let (expected, actual) = match i.query.as_str() {
            "getSchemasQuery" => {
                assert_eq!(recorded, "SELECT 'main' as schema_name;");
                (String::from("main"), introspect::schemas().join(","))
            }
            // Fix 10.
            "getSchemaQuery" => (
                recorded.replace(LIKE_TS, LIKE_FIXED),
                introspect::SCHEMA_SQL.into(),
            ),
            "getTableSizesQuery" => (
                recorded.replace(LIKE_TS, LIKE_FIXED),
                introspect::TABLE_SIZES_SQL.into(),
            ),
            // Fix 7: the autoindexes are listed.
            "getIndexUsageQuery" => (
                recorded.replace(" AND m.name NOT LIKE 'sqlite_%'", ""),
                introspect::INDEX_USAGE_SQL.into(),
            ),
            // Fixes 10 (tables) and 7 (indexes).
            "getDatabaseOverviewQuery" => (
                recorded.replacen(LIKE_TS, LIKE_FIXED, 1).replace(
                    "WHERE type = 'index' AND name NOT LIKE 'sqlite_%'",
                    "WHERE type = 'index'",
                ),
                introspect::OVERVIEW_SQL.into(),
            ),
            // Fix 1: quoted with `"` doubled, not validated (same text for plain names).
            "getTableRowCountQuery" => (recorded.to_string(), introspect::row_count_sql(&i.table)),
            // Fix 1: the PRAGMA's table-valued function, bound.
            "getColumnsQuery" => {
                assert_eq!(recorded, format!("PRAGMA table_info('{}')", i.table));
                (
                    "SELECT cid, name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?)"
                        .into(),
                    introspect::COLUMNS_SQL.into(),
                )
            }
            // Fixes 1 and 9.
            "getForeignKeysQuery" => {
                assert_eq!(recorded, format!("PRAGMA foreign_key_list('{}')", i.table));
                (FOREIGN_KEYS_SQL.into(), introspect::FOREIGN_KEYS_SQL.into())
            }
            // Fixes 1 and 5.
            "getIndexesQuery" => {
                assert_eq!(recorded, format!("PRAGMA index_list('{}')", i.table));
                (INDEXES_SQL.into(), introspect::INDEXES_SQL.into())
            }
            "getExplainQuery" => (recorded.to_string(), introspect::explain_sql(&i.sql)),
            other => panic!("{file}: unknown query {other:?}"),
        };
        (expected != actual).then(|| format!("  expected: {expected}\n  actual:   {actual}"))
    });
    assert_eq!(n, 13);
}

/// `PRAGMA foreign_key_list('t')`'s columns, with a NULL `to` resolved to
/// the parent's primary key column at the same position (fix 9).
const FOREIGN_KEYS_SQL: &str = "SELECT fk.id, fk.seq, fk.\"table\", fk.\"from\",
\t\t\tCOALESCE(fk.\"to\", (SELECT p.name FROM pragma_table_info(fk.\"table\") p WHERE p.pk = fk.seq + 1)) AS \"to\",
\t\t\tfk.on_update, fk.on_delete, fk.\"match\"
\t\tFROM pragma_foreign_key_list(?) fk";

/// `PRAGMA index_list('t')`'s columns plus one row per key column (fix 5).
const INDEXES_SQL: &str = "SELECT il.seq, il.name, il.\"unique\", il.origin, il.partial,
\t\t\tCASE ii.cid WHEN -2 THEN '<expression>' WHEN -1 THEN 'rowid' ELSE ii.name END AS column_name
\t\tFROM pragma_index_list(?) il
\t\tLEFT JOIN pragma_index_info(il.name) ii
\t\tORDER BY il.seq, ii.seqno";

/// Fix 1: names the TS rejected are quoted.
#[test]
fn row_count_quotes_any_name() {
    assert_eq!(
        introspect::row_count_sql("it's \"x\""),
        "SELECT COUNT(*) AS row_count FROM \"it's \"\"x\"\"\""
    );
}
