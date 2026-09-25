//! Parity between the DuckDB introspection parsers
//! (`seaquel_engine_duckdb::introspect`) and the demo's TypeScript DuckDB
//! adapter (`src/lib/db/duckdb.ts`), recorded into `tests/fixtures/`
//! (frozen).
//!
//! Parse inputs are the rows the adapter's own SQL returned through Seaquel's
//! Rust driver, stored in the Value wire format: cells are read with
//! `Value::from_wire`, and each result is rebuilt in its stored `columns`
//! order. Outputs are compared as typed structs, not raw JSON.
//!
//! A `bugfixes.json` case with `replaces` supersedes the recorded case of that
//! name (fixes 2, 3, 5, 6, 13, 14 and 15 change parser output): its output is
//! expected instead, its input too when it has one (the fixed query's rows),
//! and the recorded TypeScript output must differ from it. The replacement
//! counts are pinned per file.

use std::collections::HashMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value as Json};

use seaquel_engine::{QueryResult, Value};
use seaquel_engine_duckdb::introspect;
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

/// Swap in the bug-fix outputs (and inputs, when not null) for `file`'s
/// replaced cases. Every replacement must name a recorded case and change its
/// output. Returns how many cases were replaced.
fn apply_replacements<I: DeserializeOwned>(file: &str, cases: &mut [Case<I>]) -> usize {
    let prefix = format!("{file}: ");
    let mut replaced: HashMap<String, (Json, Json)> = bugfixes()
        .into_iter()
        .filter_map(|c| {
            Some((
                c.replaces?.strip_prefix(&prefix)?.to_string(),
                (c.input, c.output),
            ))
        })
        .collect();
    let mut n = 0;
    for case in cases.iter_mut() {
        if let Some((input, output)) = replaced.remove(&case.name) {
            assert_ne!(
                output, case.output,
                "{file}: the bug fix for {:?} doesn't change the recorded output",
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

/// Fix 6 replaces the listing (attached catalogs under `catalog.schema`).
#[test]
fn parse_schema() {
    let file = "parse-schema.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 1);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaTable>>(&c.output, &introspect::parse_schema(&c.input.result()))
    });
    assert_eq!(n, 1);
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

/// Fix 6 replaces the two tables whose namesakes in `fx_aux` were mixed in.
#[test]
fn parse_columns() {
    let file = "parse-columns.json";
    let mut cases = load::<ColumnsInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), 2);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaColumn>>(&c.output, &c.input.parse())
    });
    assert_eq!(n, 9);
}

/// Fix 7: the columns without foreign keys when the foreign-key query fails
/// (`table_metadata` passes no foreign-key rows then).
#[test]
fn bugfixes_columns_fk_error() {
    let cases = bugfix_cases::<Recorded>("columns-fk-error");
    let n = run("bugfixes.json columns-fk-error", &cases, |c| {
        compare::<Vec<SchemaColumn>>(
            &c.output,
            &introspect::parse_columns(&c.input.result(), None),
        )
    });
    assert_eq!(n, 1);
}

// ── parse-indexes.json ───────────────────────────────────────────────────────

/// Fix 2 replaces the tables with indexes (the TS listed none).
#[test]
fn parse_indexes() {
    let file = "parse-indexes.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 2);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaIndex>>(&c.output, &introspect::parse_indexes(&c.input.result()))
    });
    assert_eq!(n, 3);
}

// ── parse-stats.json ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct StatsInput {
    kind: String,
    #[serde(flatten)]
    recorded: Recorded,
}

fn parse_stats(c: &Case<StatsInput>) -> Option<String> {
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
        other => panic!("unknown kind {other:?}"),
    }
}

/// Fixes 3, 6 and 15 replace the table sizes, index usage and both overviews.
#[test]
fn parse_stats_recorded() {
    let file = "parse-stats.json";
    let mut cases = load::<StatsInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), 4);
    let n = run(file, &cases, parse_stats);
    assert_eq!(n, 10);
}

/// Hand-written overviews (an in-memory database, 1.5 GB).
#[test]
fn bugfixes_stats() {
    let cases = bugfix_cases::<StatsInput>("parse-stats");
    let n = run("bugfixes.json parse-stats", &cases, parse_stats);
    assert_eq!(n, 2);
}

// ── parse-explain.json ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExplainInput {
    analyze: bool,
    #[serde(flatten)]
    recorded: Recorded,
}

fn parse_explain(c: &Case<ExplainInput>) -> Option<String> {
    compare::<ExplainResult>(
        &c.output,
        &introspect::parse_explain(&c.input.recorded.result(), c.input.analyze),
    )
}

/// Fixes 5, 13 and 14 replace the recorded plans they change.
#[test]
fn parse_explain_recorded() {
    let file = "parse-explain.json";
    let mut cases = load::<ExplainInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), 44);
    let n = run(file, &cases, parse_explain);
    assert_eq!(n, 63);
}

/// Hand-written plans: Estimated Cardinality forms (fix 5) and a table name
/// with a dot and quotes (fix 14).
#[test]
fn bugfixes_explain() {
    let cases = bugfix_cases::<ExplainInput>("parse-explain");
    let n = run("bugfixes.json parse-explain", &cases, parse_explain);
    assert_eq!(n, 5);
}
