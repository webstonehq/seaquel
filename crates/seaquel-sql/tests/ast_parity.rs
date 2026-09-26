//! Parity of the AST helpers (Task 6) with the TypeScript they replace, over
//! the frozen fixtures in `tests/fixtures/` (see its README), with the bug
//! fixes in `bugfixes.json` applied: a case with `replaces` supersedes that
//! fixture case, and a standalone case of the same kind is checked too.
//!
//! Every mismatch is collected and printed before the test fails, so one run
//! shows them all.

use std::collections::{HashMap, HashSet};

use seaquel_sql::ast::{column_refs, parse_builder_query, parse_error, parse_visual};
use seaquel_sql::SqlEngine;
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn cases(v: &Value) -> &Vec<Value> {
    v["cases"].as_array().expect("cases")
}

fn engine(input: &Value) -> SqlEngine {
    input["engine"]
        .as_str()
        .unwrap_or("postgres")
        .parse()
        .expect("engine")
}

/// `bugfixes.json`'s hand-written cases of one kind: the replacements by
/// fixture case name, and the standalone ones.
struct Fixes {
    replaces: HashMap<String, Value>,
    standalone: Vec<Value>,
}

fn fixes(kind: &str, file: &str) -> Fixes {
    let b = fixture("bugfixes.json");
    let mut replaces = HashMap::new();
    let mut standalone = Vec::new();
    for c in cases(&b) {
        if c["kind"] != kind {
            continue;
        }
        match c["replaces"].as_str() {
            Some(r) => {
                let (f, name) = r.split_once(": ").expect("replaces is `file: case`");
                assert_eq!(f, file, "{r}");
                replaces.insert(name.to_string(), c.clone());
            }
            None => standalone.push(c.clone()),
        }
    }
    Fixes {
        replaces,
        standalone,
    }
}

/// The cases to check: the fixture's (with replacements applied) and the
/// standalone fixes. Each is (name, input, expected output).
fn with_fixes(file: &str, kind: &str) -> Vec<(String, Value, Value)> {
    let f = fixture(file);
    let fx = fixes(kind, file);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in cases(&f) {
        let name = c["name"].as_str().expect("name").to_string();
        let case = match fx.replaces.get(&name) {
            Some(r) => {
                seen.insert(name.clone());
                assert_eq!(
                    r["input"]["sql"], c["input"]["sql"],
                    "{name}: replaced input"
                );
                r
            }
            None => c,
        };
        out.push((name, case["input"].clone(), case["output"].clone()));
    }
    for name in fx.replaces.keys() {
        assert!(
            seen.contains(name),
            "bugfixes.json replaces a missing case {name}"
        );
    }
    for c in &fx.standalone {
        out.push((
            c["name"].as_str().expect("name").to_string(),
            c["input"].clone(),
            c["output"].clone(),
        ));
    }
    out
}

fn report(what: &str, total: usize, failures: &[String]) {
    eprintln!("{what}: {} of {total} match", total - failures.len());
    if !failures.is_empty() {
        for f in failures {
            eprintln!("--- {f}");
        }
        panic!("{what}: {} of {total} differ", failures.len());
    }
}

fn diff(name: &str, sql: &str, expected: &Value, got: &Value) -> String {
    format!(
        "{name}\n    sql: {sql:?}\n    expected: {expected}\n    got:      {got}",
        sql = sql
    )
}

fn tutorial_inputs() -> (HashMap<String, Vec<String>>, Vec<String>) {
    let t = fixture("tutorial.json");
    let schema: HashMap<String, Vec<String>> =
        serde_json::from_value(t["tutorialSchema"].clone()).expect("tutorialSchema");
    let tables: Vec<String> =
        serde_json::from_value(t["tutorialTables"].clone()).expect("tutorialTables");
    (schema, tables)
}

fn valid_tables<'a>(input: &Value, tutorial: &'a [String]) -> Option<&'a [String]> {
    match &input["validTables"] {
        Value::Null => None,
        Value::String(s) if s == "tutorial" => Some(tutorial),
        other => panic!("validTables {other}"),
    }
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serializes")
}

/// tutorial.json runs `parseSql` in PostgreSQL mode over the whole AST corpus,
/// not only the 91 tutorial entries (`tut:`, `hint:`, `var:`), which all
/// match. These four non-tutorial entries parse differently in PostgreSQL
/// mode, and none is a numbered fix:
///
/// - `rt:sub-in:mysql`: backtick names. node-sql-parser's PostgreSQL mode read
///   them; sqlparser's PostgreSQL dialect doesn't (decision 4). The builder
///   parses a MySQL connection's SQL as MySQL (fix 9), and builder.json's
///   case for it matches.
/// - `ed:mssql-offset` (`OFFSET 5 ROWS FETCH NEXT 10 ROWS ONLY`, valid
///   PostgreSQL), `ed:duckdb-basic` (`GROUP BY ALL`) and `ed:duckdb-list`
///   (`QUALIFY`): node-sql-parser rejected them, sqlparser's PostgreSQL dialect
///   accepts them. The builder model has no node for any of the three, so
///   they're left out of the ParsedQuery, as unmodelled clauses always were
///   (a `LIMIT`'s `OFFSET`, an IN list).
const TUTORIAL_PG_MODE_DEVIATIONS: [&str; 4] = [
    "rt:sub-in:mysql",
    "ed:mssql-offset",
    "ed:duckdb-basic",
    "ed:duckdb-list",
];

/// What each of [`TUTORIAL_PG_MODE_DEVIATIONS`] gives instead.
fn pg_mode_deviation(name: &str) -> Value {
    let empty = |tables: Value, order_by: Value, column_aggregates: Value| {
        json!({
            "tables": tables,
            "joins": [],
            "filters": [],
            "groupBy": [],
            "having": [],
            "orderBy": order_by,
            "limit": null,
            "selectAggregates": [],
            "columnAggregates": column_aggregates,
            "subqueries": [],
            "ctes": [],
        })
    };
    match name {
        // Backticks don't parse in PostgreSQL mode (decision 4). The builder
        // parses a MySQL connection as MySQL (fix 9), where it matches.
        "rt:sub-in:mysql" => Value::Null,
        // about: the builder drops unmodelled clauses. It has no OFFSET or
        // FETCH, so `OFFSET 5 ROWS FETCH NEXT 10 ROWS ONLY` is left out and
        // the query loads as an unlimited SELECT.
        "ed:mssql-offset" => empty(
            json!([{ "tableName": "products", "alias": null, "selectedColumns": ["name"] }]),
            json!([{ "column": "products.name", "direction": "ASC" }]),
            json!([]),
        ),
        // about: the builder drops unmodelled clauses. It has no GROUP BY
        // ALL, so the grouping is left out and only the aggregate stays.
        "ed:duckdb-basic" => empty(
            json!([{
                "tableName": "products",
                "alias": null,
                "selectedColumns": ["category_id", "price"],
            }]),
            json!([]),
            json!([{ "tableName": "products", "column": "price", "function": "AVG" }]),
        ),
        // about: the builder drops unmodelled clauses. It has no QUALIFY, so
        // the window filter is left out (and the IN list, as always).
        "ed:duckdb-list" => empty(
            json!([{ "tableName": "products", "alias": null, "selectedColumns": ["name"] }]),
            json!([]),
            json!([]),
        ),
        other => panic!("not a listed deviation: {other}"),
    }
}

#[test]
fn tutorial_parity() {
    let (schema, tables) = tutorial_inputs();
    let cases = with_fixes("tutorial.json", "tutorial");
    let mut failures = Vec::new();
    let mut deviations = Vec::new();
    let mut tutorial_entries = 0;
    for (name, input, expected) in &cases {
        let sql = input["sql"].as_str().expect("sql");
        let is_tutorial = ["tut:", "hint:", "var:"]
            .iter()
            .any(|p| name.starts_with(p));
        tutorial_entries += usize::from(is_tutorial);
        // The tutorial parses as PostgreSQL.
        let got = to_json(&parse_builder_query(
            sql,
            SqlEngine::Postgres,
            &schema,
            valid_tables(input, &tables),
        ));
        if &got == expected {
            continue;
        }
        if TUTORIAL_PG_MODE_DEVIATIONS.contains(&name.as_str()) {
            deviations.push(name.as_str());
            assert_eq!(got, pg_mode_deviation(name), "{name}");
        } else {
            failures.push(diff(name, sql, expected, &got));
        }
    }
    assert_eq!(tutorial_entries, 91);
    assert_eq!(
        deviations, TUTORIAL_PG_MODE_DEVIATIONS,
        "a listed deviation now matches: drop it from the list"
    );
    report(
        "tutorial.json (4 listed PostgreSQL-mode deviations)",
        cases.len() - deviations.len(),
        &failures,
    );
}

#[test]
fn builder_parity_in_the_connection_dialect() {
    // Fix 9: the builder parses in the connection's dialect.
    let (schema, tables) = tutorial_inputs();
    let cases = with_fixes("builder.json", "builder");
    let mut failures = Vec::new();
    for (name, input, expected) in &cases {
        let sql = input["sql"].as_str().expect("sql");
        let got = to_json(&parse_builder_query(
            sql,
            engine(input),
            &schema,
            valid_tables(input, &tables),
        ));
        if got != expected["parsed"] {
            failures.push(diff(name, sql, &expected["parsed"], &got));
        }
    }
    report("builder.json", cases.len(), &failures);
}

#[test]
fn criteria_follow_from_tutorial_parity() {
    // The lesson criteria run in TS on the builder state `parseSql` gives
    // (`criteria.ts` never reads the AST), so the verdicts in criteria.json
    // hold when the ParsedQuery is the same. Check each criteria case has a
    // tutorial.json case with the same SQL, which `tutorial_parity` covers.
    let t = fixture("tutorial.json");
    let tutorial: HashMap<&str, &str> = cases(&t)
        .iter()
        .map(|c| {
            (
                c["name"].as_str().expect("name"),
                c["input"]["sql"].as_str().expect("sql"),
            )
        })
        .collect();
    let c = fixture("criteria.json");
    for case in cases(&c) {
        let name = case["name"].as_str().expect("name");
        assert_eq!(
            tutorial.get(name).copied(),
            case["input"]["sql"].as_str(),
            "{name}"
        );
    }
    assert_eq!(cases(&c).len(), 91);
}

#[test]
fn visual_parity() {
    let cases = with_fixes("visual.json", "visual");
    let mut failures = Vec::new();
    for (name, input, expected) in &cases {
        let sql = input["sql"].as_str().expect("sql");
        let got = match parse_visual(sql, engine(input)) {
            Ok(v) => to_json(&v),
            Err(_) => Value::Null,
        };
        if &got != expected {
            failures.push(diff(name, sql, expected, &got));
        }
    }
    report("visual.json", cases.len(), &failures);
}

#[test]
fn parse_error_parity() {
    let cases = with_fixes("parse-error.json", "parse-error");
    let mut failures = Vec::new();
    for (name, input, expected) in &cases {
        let sql = input["sql"].as_str().expect("sql");
        let got = Value::Bool(parse_error(sql, engine(input)).is_some());
        if &got != expected {
            failures.push(diff(name, sql, expected, &got));
        }
    }
    report("parse-error.json", cases.len(), &failures);
}

/// The wrapper's half of decision 9: `findTable` and the primary keys, as in
/// `column-sources.ts`.
fn lookup(refs: Option<Vec<Option<seaquel_sql::ast::ColumnRef>>>, schemas: &[Value]) -> Value {
    let Some(refs) = refs else {
        return Value::Null;
    };
    Value::Array(
        refs.into_iter()
            .map(|r| {
                let Some(r) = r else { return Value::Null };
                let table = schemas.iter().find(|t| {
                    t["name"] == r.table.as_str()
                        && r.schema.as_deref().is_none_or(|s| t["schema"] == s)
                });
                let Some(table) = table else {
                    return Value::Null;
                };
                let pks: Vec<Value> = table["columns"]
                    .as_array()
                    .expect("columns")
                    .iter()
                    .filter(|c| c["isPrimaryKey"] == true)
                    .map(|c| c["name"].clone())
                    .collect();
                if pks.is_empty() {
                    return Value::Null;
                }
                serde_json::json!({
                    "schema": table["schema"],
                    "table": table["name"],
                    "primaryKeys": pks,
                    "column": r.column,
                })
            })
            .collect(),
    )
}

#[test]
fn column_sources_parity() {
    let f = fixture("column-sources.json");
    let schemas = f["schemas"].as_array().expect("schemas").clone();
    let cases = with_fixes("column-sources.json", "column-sources");
    let mut failures = Vec::new();
    for (name, input, expected) in &cases {
        let sql = input["sql"].as_str().expect("sql");
        let got = lookup(column_refs(sql, engine(input)), &schemas);
        if &got != expected {
            failures.push(diff(name, sql, expected, &got));
        }
    }
    report("column-sources.json", cases.len(), &failures);
}

#[test]
fn acceptance_statements_parse() {
    let f = fixture("acceptance.json");
    let mut failures = Vec::new();
    for c in cases(&f) {
        let name = c["name"].as_str().expect("name");
        let sql = c["input"]["sql"].as_str().expect("sql");
        let err = parse_error(sql, engine(&c["input"]));
        let got = Value::Bool(err.is_none());
        if got != c["output"]["parses"] {
            failures.push(format!(
                "{name}: expected parses={} ({err:?})",
                c["output"]["parses"]
            ));
        }
    }
    report("acceptance.json", cases(&f).len(), &failures);
}
