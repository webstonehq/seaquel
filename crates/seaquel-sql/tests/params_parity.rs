//! Parity of `{{param}}` extraction and substitution with the frozen TS
//! fixtures (`tests/fixtures/params.json`), which already hold fix 13's output
//! (the model in the recorder's `params-model.ts`, kept as
//! `docs/plans/artifacts/2026-09-27-sql-recorder-params-model.ts.txt`).
//! `bugfixes.json` has no hand-written params cases to apply, which
//! `no_hand_written_params_cases` checks.
//!
//! Values in and bind values out are in the Value wire format, as the fixture
//! records them.
//!
//! Also every prefix of every scanner-corpus input (`split.json` has one case
//! per input), through every engine, bound and forced inline, with two value
//! sets: no panic, and text without a parameter comes back unchanged.

use std::path::PathBuf;

use seaquel_sql::params::{extract_parameters, has_parameters, substitute};
use seaquel_sql::SqlEngine;
use seaquel_types::Value;
use serde_json::{json, Value as Json};

fn fixture(name: &str) -> Json {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn cases(file: &Json) -> &Vec<Json> {
    file["cases"].as_array().unwrap()
}

fn values_of(input: &Json) -> Vec<(String, Value)> {
    input["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let name = p["name"].as_str().unwrap().to_string();
            let value = Value::from_wire(p.get("value").cloned().unwrap_or(Json::Null)).unwrap();
            (name, value)
        })
        .collect()
}

/// `substitute`'s result in the fixture's shape.
fn run(sql: &str, values: &[(String, Value)], engine: SqlEngine, force: bool) -> Json {
    match substitute(sql, values, engine, force) {
        Ok(s) => json!({
            "sql": s.sql,
            "bindValues": s.bind_values.iter().map(|v| serde_json::to_value(v).unwrap()).collect::<Vec<_>>(),
        }),
        Err(e) => json!({ "error": e.message }),
    }
}

/// A fixture output with its bind values compared as `Value`s: each goes
/// through `Value::from_wire` and back. That drops the one distinction
/// `Value` can't hold, and nothing else: JavaScript's `number` vs `bigint`
/// for an integer. A bigint within ±(2^53−1) (`{"$sq":"bigint","v":"-1"}`)
/// decodes to `Value::Int` and goes out as the number `-1`; a number past
/// 2^53 (`1e18`) decodes to `Value::Int` and goes out as a bigint. The SQL
/// text is the same either way (fix 13 inlines both as digits), and no
/// caller passes a bigint (`coerceValue` gives numbers).
fn as_values(output: &Json) -> Json {
    let mut out = output.clone();
    if let Some(binds) = out.get_mut("bindValues").and_then(Json::as_array_mut) {
        for v in binds {
            *v = serde_json::to_value(Value::from_wire(v.clone()).unwrap()).unwrap();
        }
    }
    out
}

#[test]
fn params_fixtures() {
    let file = fixture("params.json");
    let mut passed = 0;
    let mut failed = Vec::new();
    let mut deviations = 0;
    for case in cases(&file) {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let sql = input["sql"].as_str().unwrap();
        match input["kind"].as_str().unwrap() {
            "extract" => {
                let got = json!({
                    "parameters": extract_parameters(sql),
                    "hasParameters": has_parameters(sql),
                });
                if got == case["output"] {
                    passed += 1;
                } else {
                    failed.push(format!("{name}: got {got}, want {}", case["output"]));
                }
            }
            "substitute" => {
                let values = values_of(input);
                let force = input["forceInline"].as_bool().unwrap();
                for (engine, want) in case["output"].as_object().unwrap() {
                    let e: SqlEngine = engine.parse().unwrap();
                    let got = run(sql, &values, e, force);
                    let as_value = as_values(want);
                    if &as_value != want {
                        deviations += 1;
                    }
                    let want = &as_value;
                    if &got == want {
                        passed += 1;
                    } else {
                        failed.push(format!(
                            "{name} [{engine}]\n  sql:  {sql:?}\n  got:  {got}\n  want: {want}"
                        ));
                    }
                }
            }
            other => panic!("{name}: unknown kind {other}"),
        }
    }
    eprintln!(
        "params.json: {passed} passed ({deviations} with bind values compared as Values), {} failed",
        failed.len()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    // 392 extract cases, and 246 substitute cases on six engines each.
    assert_eq!(passed, 392 + 246 * 6);
    assert_eq!(deviations, 8);
}

#[test]
fn no_hand_written_params_cases() {
    let bugfixes = fixture("bugfixes.json");
    for case in cases(&bugfixes) {
        let kind = case["kind"].as_str().unwrap_or("");
        let replaces = case["replaces"].as_str().unwrap_or("");
        assert!(
            kind != "params" && !replaces.starts_with("params.json"),
            "bugfixes.json has a params case to apply: {}",
            case["name"]
        );
    }
}

/// Values that exercise every branch: quotes, backslashes, the dollar tags
/// the corpus uses, `{{`, a negative number, non-finite floats, decimals and
/// bigints, non-BMP text.
fn prefix_values() -> Vec<(String, Value)> {
    let names = [
        "a", "b", "c", "p", "q", "x", "id", "t", "n", "d", "big", "dec", "name", "v", "e", "f",
    ];
    let vals = [
        Value::Text("it's \\ $$ $t$ $q$ {{a}} \"x\" 😀".into()),
        Value::Int(-1),
        Value::Float(-0.5),
        Value::Float(f64::NAN),
        Value::Decimal("-1.5E-3".into()),
        Value::Int(-9_007_199_254_740_993),
        Value::Null,
        Value::Bool(true),
    ];
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_string(), vals[i % vals.len()].clone()))
        .collect()
}

/// Values that try to break out: a value that forms a dollar tag with the
/// text next to it, quotes after a backslash, a decimal that isn't one.
fn breakout_values() -> Vec<(String, Value)> {
    let vals = [
        Value::Text("$ || 'INJ' --".into()),
        Value::Text("' OR 1=1 -- \\".into()),
        Value::Decimal("1; DROP TABLE t".into()),
        Value::Decimal("NaN".into()),
        Value::Text("x$".into()),
    ];
    prefix_values()
        .into_iter()
        .enumerate()
        .map(|(i, (n, _))| (n, vals[i % vals.len()].clone()))
        .collect()
}

fn prefixes_through(engine: SqlEngine) {
    let corpus = fixture("split.json");
    let extract = cases(&fixture("params.json"))
        .iter()
        .filter(|c| c["input"]["kind"] == "extract")
        .count();
    // The whole scanner corpus: one split.json case, and one params.json
    // extract case, per input.
    assert_eq!(cases(&corpus).len(), extract);
    let value_sets = [prefix_values(), breakout_values()];
    for case in cases(&corpus) {
        let sql = case["input"]["sql"].as_str().unwrap();
        for (end, _) in sql.char_indices().chain([(sql.len(), ' ')]) {
            let prefix = &sql[..end];
            let params = has_parameters(prefix);
            for values in &value_sets {
                for force in [false, true] {
                    let result = substitute(prefix, values, engine, force);
                    if !params {
                        let s = result.unwrap_or_else(|e| panic!("{prefix:?}: {e}"));
                        assert_eq!(s.sql, prefix, "{engine} {force}");
                        assert!(s.bind_values.is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn prefixes_postgres() {
    prefixes_through(SqlEngine::Postgres);
}

#[test]
fn prefixes_mysql() {
    prefixes_through(SqlEngine::Mysql);
}

#[test]
fn prefixes_mariadb() {
    prefixes_through(SqlEngine::Mariadb);
}

#[test]
fn prefixes_sqlite() {
    prefixes_through(SqlEngine::Sqlite);
}

#[test]
fn prefixes_mssql() {
    prefixes_through(SqlEngine::Mssql);
}

#[test]
fn prefixes_duckdb() {
    prefixes_through(SqlEngine::Duckdb);
}
