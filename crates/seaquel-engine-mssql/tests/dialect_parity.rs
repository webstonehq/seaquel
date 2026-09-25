//! Parity between `MssqlDialect` and the TypeScript SQL Server adapter
//! (`src/lib/db/mssql.ts`, deleted in phase 2), recorded into
//! `tests/fixtures/*.json` (frozen; see `tests/fixtures/README.md`).
//!
//! One test per fixture file. Each runs every case, then fails once with the
//! name, input, expected and actual output of every case that differed.
//!
//! Intended differences live in `fixtures/bugfixes.json` (fixes 2, 7, 8, 10,
//! 12, 13, 14 and 15 here; see its `about`). A bug-fix case with `replaces`
//! supersedes the recorded case of that name: its output is expected
//! instead, and the recorded (TypeScript) output must differ from it, so the
//! replacement documents a real change.
//!
//! CRUD isn't recorded: the Rust dialect binds `@P1…` where the adapter
//! inlined literals (phase 2 decision 4). `tests/smoke.rs` runs it live;
//! the placeholders are checked here.

use std::collections::HashMap;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value as Json;

use seaquel_engine::{Dialect, Engine, SqlWithBindings, Value};
use seaquel_engine_mssql::MssqlDialect;
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
    seaquel_engine_mssql::engine()
}

fn dialect(engine: &Arc<dyn Engine>) -> &dyn Dialect {
    engine
        .dialect()
        .expect("MssqlEngine::dialect() returns the SQL Server dialect")
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

/// Swap in the bug-fix outputs. Every replacement must name a recorded case
/// and change its output. Returns how many were replaced.
fn apply_replacements<I>(file: &str, cases: &mut [Case<I>]) -> usize {
    let prefix = format!("{file}: ");
    let mut replaced: HashMap<String, Json> = bugfixes()
        .into_iter()
        .filter_map(|c| {
            let name = c.replaces?.strip_prefix(&prefix)?.to_string();
            assert!(
                c.input.is_null(),
                "{file}: {name:?}: a DDL or pagination replacement keeps the recorded input"
            );
            Some((name, c.output))
        })
        .collect();
    let mut n = 0;
    for case in cases.iter_mut() {
        if let Some(output) = replaced.remove(&case.name) {
            assert_ne!(
                output, case.output,
                "{file}: bug fix for {:?} doesn't change the recorded output",
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

/// The hand-written cases of `kind` that don't replace a recorded one.
fn bugfix_cases<I: DeserializeOwned>(kind: &str) -> Vec<Case<I>> {
    bugfixes()
        .into_iter()
        .filter(|c| c.kind == kind && c.replaces.is_none())
        .map(|c| Case {
            input: serde_json::from_value(c.input)
                .unwrap_or_else(|e| panic!("bugfixes.json {:?}: {e}", c.name)),
            name: format!("[fix {}] {}", c.fix, c.name),
            output: c.output,
        })
        .collect()
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
    assert_eq!(n, 10);
}

// ── paginate.json ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PaginateInput {
    sql: String,
    limit: u64,
    offset: u64,
}

fn check_paginate(d: &dyn Dialect, c: &Case<PaginateInput>) -> Option<(String, String, String)> {
    let i = &c.input;
    compare_str(
        format!("{i:?}"),
        expect_str(&c.output),
        d.paginate(&i.sql, i.limit, i.offset),
    )
}

/// Fix 8 (nested ORDER BY isn't the query's) replaces six recorded cases,
/// fix 14 (a trailing semicolon is dropped) one.
#[test]
fn paginate() {
    let file = "paginate.json";
    let mut cases = load::<PaginateInput>(file, include_str!("fixtures/paginate.json"));
    assert_eq!(apply_replacements(file, &mut cases), 7);
    let e = engine();
    let d = dialect(&e);
    assert_eq!(run(file, &cases, |c| check_paginate(d, c)), 14);
}

/// Fixes 8 and 14: the hand-written pagination cases.
#[test]
fn bugfixes_paginate() {
    let cases = bugfix_cases::<PaginateInput>("paginate");
    let e = engine();
    let d = dialect(&e);
    assert_eq!(run("bugfixes.json", &cases, |c| check_paginate(d, c)), 18);
}

// ── column-types.json ────────────────────────────────────────────────────────

#[test]
fn column_types() {
    let file = "column-types.json";
    let cases = load::<Json>(file, include_str!("fixtures/column-types.json"));
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

// ── CRUD (not recorded; see the module docs) ─────────────────────────────────

/// `@P1…` placeholders, values and keys bound in order; the cast map is
/// ignored, as the TypeScript never cast.
#[test]
fn crud_binds_at_p_placeholders() {
    let e = engine();
    let d = dialect(&e);
    let casts: HashMap<String, String> = [("id".to_string(), "int".to_string())]
        .into_iter()
        .collect();
    let row = vec![
        ("id".to_string(), Value::Int(1)),
        ("re]gion".to_string(), Value::from("eu")),
        ("name".to_string(), Value::from("名前")),
    ];
    let pks = vec!["id".to_string(), "re]gion".to_string()];
    let binds = |v: &[Value]| Some(v.to_vec());
    assert_eq!(
        d.build_update(
            "dbo",
            "t]x",
            "name",
            Value::from("x"),
            &pks,
            &row,
            Some(&casts)
        ),
        SqlWithBindings {
            sql: "UPDATE [dbo].[t]]x] SET [name] = @P1 WHERE [id] = @P2 AND [re]]gion] = @P3"
                .into(),
            bind_values: binds(&[Value::from("x"), Value::Int(1), Value::from("eu")]),
        }
    );
    assert_eq!(
        d.build_set_default("dbo", "t", "name", &pks, &row, Some(&casts)),
        SqlWithBindings {
            sql: "UPDATE [dbo].[t] SET [name] = DEFAULT WHERE [id] = @P1 AND [re]]gion] = @P2"
                .into(),
            bind_values: binds(&[Value::Int(1), Value::from("eu")]),
        }
    );
    assert_eq!(
        d.build_insert("dbo", "t", &row, Some(&casts)),
        SqlWithBindings {
            sql: "INSERT INTO [dbo].[t] ([id], [re]]gion], [name]) VALUES (@P1, @P2, @P3)".into(),
            bind_values: binds(&[Value::Int(1), Value::from("eu"), Value::from("名前")]),
        }
    );
    assert_eq!(
        d.build_delete("dbo", "t", &pks, &row, Some(&casts)),
        SqlWithBindings {
            sql: "DELETE FROM [dbo].[t] WHERE [id] = @P1 AND [re]]gion] = @P2".into(),
            bind_values: binds(&[Value::Int(1), Value::from("eu")]),
        }
    );
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

/// Fix 2 replaces the case with `]` in its names, fix 7 (an explicit NULL
/// for a nullable column) six more.
#[test]
fn ddl_create() {
    let file = "ddl-create.json";
    let mut cases = load::<CreateTableDefinition>(file, include_str!("fixtures/ddl-create.json"));
    assert_eq!(apply_replacements(file, &mut cases), 7);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.create_table(&c.input),
        )
    });
    assert_eq!(n, 8);
}

/// Fixes 2, 7 and 13 replace fourteen recorded cases: every one the
/// TypeScript made invalid T-SQL, and a DROP COLUMN, which now drops the
/// column's default constraint first.
#[test]
fn ddl_alter() {
    let file = "ddl-alter.json";
    let mut cases = load::<AlterInput>(file, include_str!("fixtures/ddl-alter.json"));
    assert_eq!(apply_replacements(file, &mut cases), 14);
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
/// Fixes 2 and 7 replace three cases.
#[test]
fn ddl_add_column() {
    let file = "ddl-add-column.json";
    let mut cases = load::<AddColumnInput>(file, include_str!("fixtures/ddl-add-column.json"));
    assert_eq!(apply_replacements(file, &mut cases), 3);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            MssqlDialect.add_column(&i.schema, &i.table, &i.column),
        )
    });
    assert_eq!(n, 4);
}

/// The hand-written ALTER cases that don't replace a recorded one (fixes 7,
/// 10, 12, 13 and 15).
#[test]
fn bugfixes_ddl_alter() {
    let cases = bugfix_cases::<AlterInput>("ddl-alter");
    let e = engine();
    let d = dialect(&e);
    let n = run("bugfixes.json", &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            d.alter_table(&i.from, &i.to),
        )
    });
    assert_eq!(n, 19);
}

/// Fix 15: CREATE TABLE and ADD emit a column's collation (hand-written;
/// `tests/smoke.rs` runs them live).
#[test]
fn bugfixes_ddl_create_and_add_column() {
    let e = engine();
    let d = dialect(&e);
    let creates = bugfix_cases::<CreateTableDefinition>("ddl-create");
    let n = run("bugfixes.json", &creates, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.create_table(&c.input),
        )
    });
    assert_eq!(n, 1);
    let adds = bugfix_cases::<AddColumnInput>("ddl-add-column");
    let n = run("bugfixes.json", &adds, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            MssqlDialect.add_column(&i.schema, &i.table, &i.column),
        )
    });
    assert_eq!(n, 1);
}

/// Every hand-written DDL or pagination case is checked by one of the tests
/// above.
#[test]
fn every_dialect_bugfix_is_checked() {
    let all = bugfixes();
    for c in &all {
        if c.kind.starts_with("ddl-") || c.kind == "paginate" {
            assert!(
                matches!(c.fix, 2 | 7 | 8 | 10 | 12 | 13 | 14 | 15),
                "{:?} has fix {}",
                c.name,
                c.fix
            );
        }
    }
    // 7 + 14 + 3 + 7 replacements; 19 + 18 + 1 + 1 new cases, each kind run
    // by one of the tests above.
    let dialect_cases = all
        .iter()
        .filter(|c| c.kind.starts_with("ddl-") || c.kind == "paginate")
        .count();
    assert_eq!(dialect_cases, 70);
}
