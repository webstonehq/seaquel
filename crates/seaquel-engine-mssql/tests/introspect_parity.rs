//! Parity between the SQL Server introspection parsers and SQL
//! (`seaquel_engine_mssql::introspect`) and the TypeScript adapter (deleted
//! in phase 2), recorded into `tests/fixtures/` (frozen).
//!
//! Parse inputs are the rows the adapter's own SQL returned through
//! Seaquel's Rust driver, stored in the Value wire format: cells are read
//! with `Value::from_wire`, and each result is rebuilt in its stored
//! `columns` order. The plans in `parse-explain.json` were captured with the
//! Node `mssql` package (see the fixtures README). Outputs are compared as
//! typed structs, not raw JSON.
//!
//! A `bugfixes.json` case with `replaces` supersedes the recorded case of
//! that name: its output is expected instead, and the recorded TypeScript
//! output must differ from it. Fix 10's replacements also carry their own
//! input, the rows of the fixed columns query.

use std::collections::HashMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value as Json};

use seaquel_engine::{QueryResult, Value};
use seaquel_engine_mssql::introspect;
use seaquel_types::{ExplainPlanNode, ExplainResult, SchemaColumn, SchemaIndex, SchemaTable};

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

/// Swap in the bug-fix outputs (and inputs, where a replacement has one)
/// for `file`'s replaced cases. Every replacement must name a recorded case
/// and change its output. Returns how many cases were replaced.
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
                    panic!("{file}: replacement input for {:?}: {e}", case.name)
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
    #[serde(default)]
    analyze: bool,
}

impl Recorded {
    fn result(&self) -> QueryResult {
        QueryResult {
            columns: self.columns.clone(),
            rows: self
                .rows
                .iter()
                .map(|row| {
                    self.columns
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
}

// ── sql.json ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SqlInput {
    query: String,
    #[serde(default)]
    sql: String,
    #[serde(default)]
    analyze: bool,
}

/// The catalog SQL. Fixes 1, 3, 4, 6, 10 and 15 replace the columns and
/// indexes queries (`sql` cases, the exact text), fix 9 the four EXPLAIN
/// texts (`explain-batches` cases: the batches the driver runs).
#[test]
fn sql_text() {
    let file = "sql.json";
    let mut cases = load::<SqlInput>(file);
    assert_eq!(apply_replacements(file, &mut cases), 6);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        match i.query.as_str() {
            "getExplainQuery" => {
                let actual = introspect::explain_batches(&i.sql, i.analyze).to_vec();
                let expected: Vec<String> = serde_json::from_value(c.output["batches"].clone())
                    .expect("explain-batches output is {batches}");
                (expected != actual)
                    .then(|| format!("  expected: {expected:?}\n  actual:   {actual:?}"))
            }
            query => {
                let actual = match query {
                    "getSchemaQuery" => introspect::SCHEMA_SQL,
                    "getSchemasQuery" => introspect::SCHEMAS_SQL,
                    "getColumnsQuery" => introspect::COLUMNS_SQL,
                    "getIndexesQuery" => introspect::INDEXES_SQL,
                    other => panic!("{file}: unknown query {other:?}"),
                };
                let expected = c.output.as_str().expect("a string");
                (expected != actual)
                    .then(|| format!("  expected: {expected:?}\n  actual:   {actual:?}"))
            }
        }
    });
    assert_eq!(n, 8);
}

// ── parse-schema.json ────────────────────────────────────────────────────────

#[test]
fn parse_schema() {
    let cases = load::<Recorded>("parse-schema.json");
    let n = run("parse-schema.json", &cases, |c| {
        compare::<Vec<SchemaTable>>(&c.output, &introspect::parse_schema(&c.input.result()))
    });
    assert_eq!(n, 1);
}

// ── parse-columns.json ───────────────────────────────────────────────────────

/// Every case is replaced by fix 10: the fixed query's rows carry the full
/// type (and, for `dbo.fx_ledger`, fix 4's one row per column).
#[test]
fn parse_columns() {
    let file = "parse-columns.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 8);
    let n = run(file, &cases, |c| {
        assert!(
            c.input.columns.iter().any(|col| col == "collation_name"),
            "{:?}: the fixed query's rows",
            c.name
        );
        compare::<Vec<SchemaColumn>>(&c.output, &introspect::parse_columns(&c.input.result()))
    });
    assert_eq!(n, 8);
}

// ── parse-indexes.json ───────────────────────────────────────────────────────

/// Fix 5 replaces every case with a unique index: `is_unique` is read.
#[test]
fn parse_indexes() {
    let file = "parse-indexes.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 6);
    let n = run(file, &cases, |c| {
        compare::<Vec<SchemaIndex>>(&c.output, &introspect::parse_indexes(&c.input.result()))
    });
    assert_eq!(n, 8);
}

// ── parse-explain.json ───────────────────────────────────────────────────────

/// Fix 11 replaces the cases whose names had `]]` left in them.
#[test]
fn parse_explain() {
    let file = "parse-explain.json";
    let mut cases = load::<Recorded>(file);
    assert_eq!(apply_replacements(file, &mut cases), 3);
    let n = run(file, &cases, |c| {
        compare::<ExplainResult>(
            &c.output,
            &introspect::parse_explain(&c.input.result(), c.input.analyze),
        )
    });
    assert_eq!(n, 49);
}

/// Fix 9: plan XML that doesn't parse is one `Query Plan` node whose filter
/// is the parser's error (its wording isn't pinned).
#[test]
fn parse_explain_error() {
    let cases: Vec<Case<Recorded>> = bugfixes()
        .into_iter()
        .filter(|c| c.kind == "parse-explain-error")
        .map(|c| {
            assert_eq!(c.fix, 9);
            Case {
                name: c.name,
                input: serde_json::from_value(c.input).expect("rows"),
                output: c.output,
            }
        })
        .collect();
    let n = run("bugfixes.json", &cases, |c| {
        let out = introspect::parse_explain(&c.input.result(), c.input.analyze);
        let plan: &ExplainPlanNode = &out.plan;
        let ok = plan.node_type == c.output["nodeType"].as_str().unwrap()
            && plan.children.len() as u64 == c.output["children"].as_u64().unwrap()
            && plan.filter.as_deref().is_some_and(|f| !f.is_empty());
        (!ok).then(|| format!("  actual: {plan:#?}"))
    });
    assert_eq!(n, 1);
}
