//! Parity between the Postgres introspection parsers and SQL
//! (`seaquel_engine_postgres::introspect`) and the recorded TypeScript
//! Postgres adapter (`tests/fixtures/{parse-*,sql}.json`).
//!
//! One test per fixture file. Each runs every case, then fails once with the
//! name, input, expected and actual output of every case that differed.
//! Outputs are compared as typed structs, not raw JSON: `100` and `100.0` are
//! different `serde_json::Value`s but the same `f64`.
//!
//! `parse-indexes.json` has no parity test: bug fix 4 replaces the regex
//! parser over `indexdef` with a catalog query, so its expectations live in
//! `bugfixes.json` and `run_introspection` (see `tests/smoke.rs`).

use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value as Json};

use seaquel_engine::{QueryResult, Value};
use seaquel_engine_postgres::introspect;
use seaquel_types::{
    DatabaseOverview, ExplainResult, IndexUsageInfo, SchemaColumn, SchemaTable, TableSizeInfo,
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

fn load<I: DeserializeOwned>(file: &str, text: &str) -> Vec<Case<I>> {
    serde_json::from_str::<Fixture<I>>(text)
        .unwrap_or_else(|e| panic!("{file}: cannot parse fixture: {e}"))
        .cases
}

/// Runs `check` on every case and panics once, listing every mismatch.
/// `check` returns `Some(description)` for a case that differs.
fn run<I>(
    file: &str,
    cases: &[Case<I>],
    mut check: impl FnMut(&Case<I>) -> Option<String>,
) -> usize {
    assert!(!cases.is_empty(), "{file}: no cases");
    let mut failures = Vec::new();
    for case in cases {
        if let Some(diff) = check(case) {
            failures.push(format!("case {:?}\n{diff}", case.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{file}: {} of {} cases differ\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
    eprintln!("{file}: {} cases passed", cases.len());
    cases.len()
}

/// `Some(diff)` when `actual` differs from the fixture `output` read as `T`.
fn compare<T: DeserializeOwned + PartialEq + Debug>(output: &Json, actual: &T) -> Option<String> {
    let expected: T = serde_json::from_value(output.clone())
        .unwrap_or_else(|e| panic!("fixture output doesn't deserialize: {e}\n{output}"));
    (&expected != actual).then(|| format!("  expected: {expected:#?}\n  actual:   {actual:#?}"))
}

/// Recorded rows (objects keyed by column name) as a `QueryResult`. Cells go
/// through `Value::from_json_cell`, as the driver's JSON decoder does.
/// `wrap` may rewrite a cell (e.g. to `Value::Json`) by column name.
fn query_result(rows: &[Map<String, Json>], wrap: impl Fn(&str, Json) -> Value) -> QueryResult {
    let mut columns: Vec<String> = Vec::new();
    for row in rows {
        for key in row.keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let rows = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(c).cloned().map_or(Value::Null, |j| wrap(c, j)))
                .collect()
        })
        .collect();
    QueryResult { columns, rows }
}

fn plain(rows: &[Map<String, Json>]) -> QueryResult {
    query_result(rows, |_, j| Value::from_json_cell(j))
}

#[derive(Deserialize)]
struct RowsInput {
    rows: Vec<Map<String, Json>>,
}

// ── parse-schema.json ────────────────────────────────────────────────────────

#[test]
fn parse_schema() {
    let file = "parse-schema.json";
    let cases = load::<RowsInput>(file, include_str!("fixtures/parse-schema.json"));
    run(file, &cases, |c| {
        compare::<Vec<SchemaTable>>(&c.output, &introspect::parse_schema(&plain(&c.input.rows)))
    });
}

// ── parse-columns.json ───────────────────────────────────────────────────────

#[test]
fn parse_columns() {
    let file = "parse-columns.json";
    let cases = load::<RowsInput>(file, include_str!("fixtures/parse-columns.json"));
    run(file, &cases, |c| {
        compare::<Vec<SchemaColumn>>(&c.output, &introspect::parse_columns(&plain(&c.input.rows)))
    });
}

/// Branches no recorded column reaches.
#[test]
fn parse_columns_unrecorded_branches() {
    let rows = |default: Json, fk: Json| {
        let row = serde_json::json!({
            "column_name": "c",
            "data_type": "text",
            "is_nullable": "YES",
            "column_default": default,
            "is_primary_key": false,
            "is_foreign_key": true,
            "foreign_key_ref": fk,
        });
        plain(&[row.as_object().unwrap().clone()])
    };
    // `column_default || undefined`: "" is no default.
    let cols = introspect::parse_columns(&rows(Json::from(""), Json::Null));
    assert_eq!(cols[0].default_value, None);
    assert_eq!(cols[0].foreign_key_ref, None);
    // The ref is split on every '.', and kept only with exactly three parts.
    let cols = introspect::parse_columns(&rows(Json::Null, Json::from("a.b.c.d")));
    assert_eq!(cols[0].foreign_key_ref, None);
    let cols = introspect::parse_columns(&rows(Json::Null, Json::from("a.b")));
    assert_eq!(cols[0].foreign_key_ref, None);
    let cols = introspect::parse_columns(&rows(Json::Null, Json::from("")));
    assert_eq!(cols[0].foreign_key_ref, None);
    // Fix 7: `cast_type` fills `cast_type`; the recorded rows have none.
    assert_eq!(cols[0].cast_type, None);
    let mut row = rows(Json::Null, Json::Null);
    row.columns.push("cast_type".into());
    row.rows[0].push(Value::from("app.\"Weird Mood\"[]"));
    let cols = introspect::parse_columns(&row);
    assert_eq!(cols[0].cast_type.as_deref(), Some("app.\"Weird Mood\"[]"));
    let cols = introspect::parse_columns(&rows(Json::from("0"), Json::from("s.t.c")));
    assert_eq!(cols[0].default_value.as_deref(), Some("0"));
    let fk = cols[0].foreign_key_ref.as_ref().unwrap();
    assert_eq!(
        (
            fk.referenced_schema.as_str(),
            fk.referenced_table.as_str(),
            fk.referenced_column.as_str()
        ),
        ("s", "t", "c")
    );
}

// ── parse-stats.json ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct StatsInput {
    kind: String,
    rows: Vec<Map<String, Json>>,
}

#[test]
fn parse_stats() {
    let file = "parse-stats.json";
    let cases = load::<StatsInput>(file, include_str!("fixtures/parse-stats.json"));
    run(file, &cases, |c| {
        let r = plain(&c.input.rows);
        match c.input.kind.as_str() {
            "tableSizes" => {
                compare::<Vec<TableSizeInfo>>(&c.output, &introspect::parse_table_sizes(&r))
            }
            "indexUsage" => {
                compare::<Vec<IndexUsageInfo>>(&c.output, &introspect::parse_index_usage(&r))
            }
            "overview" => compare::<DatabaseOverview>(&c.output, &introspect::parse_overview(&r)),
            other => panic!("{file}: unknown kind {other:?}"),
        }
    });
}

/// `Number(x) || 0` over every shape a numeric cell can take.
#[test]
fn stats_numbers_accept_every_numeric_value() {
    let cell = |v: Value| QueryResult {
        columns: vec![
            "schema_name".into(),
            "table_name".into(),
            "index_name".into(),
            "size".into(),
            "scans".into(),
            "rows_read".into(),
            "unused".into(),
        ],
        rows: vec![vec![
            Value::from("s"),
            Value::from("t"),
            Value::from("i"),
            Value::from("8192 bytes"),
            v.clone(),
            v,
            Value::Bool(false),
        ]],
    };
    let scans = |v: Value| {
        let u = &introspect::parse_index_usage(&cell(v))[0];
        (u.scans, u.rows_read)
    };
    assert_eq!(scans(Value::Int(7)), (7, Some(7)));
    assert_eq!(scans(Value::Float(7.0)), (7, Some(7)));
    assert_eq!(scans(Value::Decimal("7".into())), (7, Some(7)));
    assert_eq!(scans(Value::Text("7".into())), (7, Some(7)));
    assert_eq!(scans(Value::Text(" 7 ".into())), (7, Some(7)));
    assert_eq!(scans(Value::Null), (0, Some(0)));
    assert_eq!(scans(Value::Text("seven".into())), (0, Some(0)));
    assert_eq!(scans(Value::Float(f64::NAN)), (0, Some(0)));
}

// ── parse-explain.json ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExplainInput {
    analyze: bool,
    rows: Vec<Map<String, Json>>,
}

/// Turns the recorded `QUERY PLAN` JSON into a cell.
type Wrap = fn(Json) -> Value;

/// Every case runs three times: with `QUERY PLAN` as the JSON decoder yields
/// it today (`from_json_cell` → `Array`), as `Value::Json` (native decode), and
/// as text (the TS `typeof jsonPlan === "string"` branch).
#[test]
fn parse_explain() {
    let file = "parse-explain.json";
    let cases = load::<ExplainInput>(file, include_str!("fixtures/parse-explain.json"));
    let wrappings: [(&str, Wrap); 3] = [
        ("from_json_cell", Value::from_json_cell),
        ("Json", Value::Json),
        ("Text", |j| Value::Text(j.to_string())),
    ];
    run(file, &cases, |c| {
        let mut diffs = Vec::new();
        for (label, wrap) in wrappings {
            let r = query_result(&c.input.rows, |col, j| {
                if col == "QUERY PLAN" {
                    wrap(j)
                } else {
                    Value::from_json_cell(j)
                }
            });
            let actual = introspect::parse_explain(&r, c.input.analyze).expect("parse_explain");
            if let Some(d) = compare::<ExplainResult>(&c.output, &actual) {
                diffs.push(format!("  [{label}]\n{d}"));
            }
        }
        (!diffs.is_empty()).then(|| diffs.join("\n"))
    });
}

fn explain_rows(column: &str, plan: Json) -> QueryResult {
    QueryResult {
        columns: vec![column.into()],
        rows: vec![vec![Value::Json(plan)]],
    }
}

/// Branches no recorded plan reaches.
#[test]
fn parse_explain_unrecorded_branches() {
    // Lowercase column name.
    let plan = serde_json::json!([{ "Plan": { "Node Type": "Result" }, "Planning Time": 0.5 }]);
    let r = introspect::parse_explain(&explain_rows("query plan", plan), false).unwrap();
    assert_eq!(r.plan.node_type, "Result");
    assert_eq!(r.planning_time, 0.5);
    assert_eq!(r.execution_time, None);

    // No rows: an `Unknown` root and planning time 0.
    let empty = QueryResult {
        columns: vec![],
        rows: vec![],
    };
    let r = introspect::parse_explain(&empty, true).unwrap();
    assert_eq!(r.plan.node_type, "Unknown");
    assert_eq!(r.plan.id, "node-0");
    assert!(r.plan.children.is_empty());
    assert_eq!(r.planning_time, 0.0);
    assert_eq!(r.execution_time, None);
    assert!(r.is_analyze);

    // Filter is read; actual* fields are dropped when not analyzing, and
    // node ids are assigned post-order.
    let plan = serde_json::json!([{
        "Plan": {
            "Node Type": "Seq Scan",
            "Filter": "(id > 1)",
            "Actual Rows": 3,
            "Actual Loops": 1,
            "Plans": [{ "Node Type": "A" }, { "Node Type": "B", "Plans": [{ "Node Type": "C" }] }]
        },
        "Planning Time": 1,
        "Execution Time": 2
    }]);
    let r = introspect::parse_explain(&explain_rows("QUERY PLAN", plan.clone()), false).unwrap();
    assert_eq!(r.plan.filter.as_deref(), Some("(id > 1)"));
    assert_eq!((r.plan.actual_rows, r.plan.actual_loops), (None, None));
    assert_eq!(r.execution_time, Some(2.0));
    let ids = |n: &seaquel_types::ExplainPlanNode| {
        (
            n.id.clone(),
            n.children.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        ids(&r.plan),
        ("node-3".into(), vec!["node-0".into(), "node-2".into()])
    );
    assert_eq!(r.plan.children[1].children[0].id, "node-1");
    let r = introspect::parse_explain(&explain_rows("QUERY PLAN", plan), true).unwrap();
    assert_eq!(
        (r.plan.actual_rows, r.plan.actual_loops),
        (Some(3.0), Some(1))
    );

    // Invalid plan text is an error, not a panic.
    let bad = QueryResult {
        columns: vec!["QUERY PLAN".into()],
        rows: vec![vec![Value::Text("not json".into())]],
    };
    assert!(introspect::parse_explain(&bad, false).is_err());
}

// ── sql.json ─────────────────────────────────────────────────────────────────

/// Fix 7: the `cast_type` column of `COLUMNS_SQL`, the column's type for
/// `CAST($n AS …)` (see the `COLUMNS_SQL` docs): user types qualified from the
/// catalog, built-ins by `format_type`, and character and bit types (found
/// through domains and array elements) without their length.
const COLUMNS_CAST_TYPE: &str = "\t\t\t(SELECT CASE
\t\t\t\tWHEN f.oid = ANY ('{bpchar,varchar,bit,varbit}'::regtype[]::oid[])
\t\t\t\t\tTHEN format_type(f.oid, -1) || CASE WHEN f.arr THEN '[]' ELSE '' END
\t\t\t\tWHEN e.typnamespace <> 'pg_catalog'::regnamespace
\t\t\t\t\tTHEN quote_ident(en.nspname) || '.' || quote_ident(e.typname)
\t\t\t\t\t\t|| CASE WHEN e.oid <> t.oid THEN '[]' ELSE '' END
\t\t\t\tELSE format_type(a.atttypid, a.atttypmod) END
\t\t\t\tFROM pg_attribute a
\t\t\t\tJOIN pg_class cl ON cl.oid = a.attrelid
\t\t\t\tJOIN pg_namespace ns ON ns.oid = cl.relnamespace
\t\t\t\tJOIN pg_type t ON t.oid = a.atttypid
\t\t\t\tJOIN pg_type e ON e.oid = CASE WHEN t.typtype = 'b' AND t.typcategory = 'A' AND t.typelem <> 0
\t\t\t\t\tTHEN t.typelem ELSE t.oid END
\t\t\t\tJOIN pg_namespace en ON en.oid = e.typnamespace
\t\t\t\tCROSS JOIN LATERAL (
\t\t\t\t\tWITH RECURSIVE chain(oid, arr, depth) AS (
\t\t\t\t\t\tSELECT a.atttypid, false, 0
\t\t\t\t\t\tUNION ALL
\t\t\t\t\t\tSELECT CASE WHEN ty.typtype = 'd' THEN ty.typbasetype ELSE ty.typelem END,
\t\t\t\t\t\t\tchain.arr OR ty.typtype <> 'd', chain.depth + 1
\t\t\t\t\t\tFROM chain JOIN pg_type ty ON ty.oid = chain.oid
\t\t\t\t\t\tWHERE ty.typtype = 'd' OR (ty.typtype = 'b' AND ty.typcategory = 'A' AND ty.typelem <> 0)
\t\t\t\t\t)
\t\t\t\t\tSELECT oid, arr FROM chain ORDER BY depth DESC LIMIT 1
\t\t\t\t) f
\t\t\t\tWHERE ns.nspname = c.table_schema AND cl.relname = c.table_name
\t\t\t\t\tAND a.attname = c.column_name AND a.attnum > 0 AND NOT a.attisdropped
\t\t\t) as cast_type";

#[derive(Deserialize)]
struct SqlInput {
    query: String,
}

/// Every catalog query equals the recorded TS text except for bug fixes 1, 4,
/// 5 and 7. Those differences are applied to the recorded text here, so a stray
/// edit anywhere else still fails. The EXPLAIN cases run in
/// `dialect_parity.rs` (`Dialect::explain_sql`).
#[test]
fn sql() {
    let file = "sql.json";
    let cases: Vec<_> = load::<SqlInput>(file, include_str!("fixtures/sql.json"))
        .into_iter()
        .filter(|c| c.input.query != "getExplainQuery")
        .collect();
    run(file, &cases, |c| {
        let recorded = c.output.as_str().expect("sql.json output is a string");
        let (expected, actual) = match c.input.query.as_str() {
            "getSchemaQuery" => (recorded.to_string(), introspect::SCHEMA_SQL),
            "getSchemasQuery" => (recorded.to_string(), introspect::SCHEMAS_SQL),
            "getIndexUsageQuery" => (recorded.to_string(), introspect::INDEX_USAGE_SQL),
            "getDatabaseOverviewQuery" => (recorded.to_string(), introspect::OVERVIEW_SQL),
            // Fix 1: the table and schema are bound, not spliced.
            // Fix 7: one more column, `cast_type`, after `foreign_key_ref`.
            "getColumnsQuery" => (
                recorded
                    .replace(
                        "WHERE table_name = 'users' AND table_schema = 'public'",
                        "WHERE table_name = $1 AND table_schema = $2",
                    )
                    .replace(
                        ") as foreign_key_ref\n\t\tFROM information_schema.columns c",
                        &format!(") as foreign_key_ref,\n{COLUMNS_CAST_TYPE}\n\t\tFROM information_schema.columns c"),
                    ),
                introspect::COLUMNS_SQL,
            ),
            // Fix 5: sizes by relation oid, not by a spliced name.
            "getTableSizesQuery" => (
                recorded.replace("schemaname || '.' || relname", "relid"),
                introspect::TABLE_SIZES_SQL,
            ),
            // Fix 4: a new catalog query. Only its filter is checked here; its
            // results are checked against bugfixes.json in the smoke test.
            "getIndexesQuery" => {
                let actual = introspect::INDEXES_SQL;
                return (!actual.contains("t.relname = $1 AND n.nspname = $2")
                    || actual.contains("pg_indexes"))
                .then(|| format!("  unexpected index query:\n{actual}"));
            }
            other => panic!("{file}: unknown query {other:?}"),
        };
        (expected != actual).then(|| format!("  expected: {expected}\n  actual:   {actual}"))
    });
}
