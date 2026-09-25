//! Parity between `SqliteDialect` and the TypeScript SQLite adapter
//! (`src/lib/db/sqlite.ts`, deleted in phase 2), recorded into
//! `tests/fixtures/*.json` (frozen; see `tests/fixtures/README.md`).
//!
//! One test per fixture file. Each runs every case, then fails once with the
//! name, input, expected and actual output of every case that differed.
//!
//! Intended differences live in `fixtures/bugfixes.json`:
//! - Fix 2: the DDL quote doubles `"`.
//! - Fix 3: an added index is `CREATE INDEX "main"."i" ON "t"` (SQLite
//!   rejects a schema-qualified table there).
//! - Fix 4: an added foreign key is a comment line (SQLite has no
//!   `ADD FOREIGN KEY`).
//! - Fix 6: a type, nullability or default edit is a comment line instead of
//!   nothing.
//!
//! A bug-fix case with `replaces` supersedes the recorded case of that name:
//! its output is expected instead, and the recorded (TypeScript) output must
//! differ from it, so the replacement documents a real change.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

use seaquel_engine::{Dialect, Engine, RowValues, SqlWithBindings, Value};
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
    seaquel_engine_sqlite::engine()
}

fn dialect(engine: &Arc<dyn Engine>) -> &dyn Dialect {
    engine
        .dialect()
        .expect("SqliteEngine::dialect() returns the SQLite dialect")
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

/// `file`'s replaced cases: recorded case name → the bug-fix output.
fn replacements(file: &str) -> HashMap<String, Json> {
    let prefix = format!("{file}: ");
    bugfixes()
        .into_iter()
        .filter_map(|c| {
            let name = c.replaces?.strip_prefix(&prefix)?.to_string();
            Some((name, c.output))
        })
        .collect()
}

/// Swap in the bug-fix outputs. Every replacement must name a recorded case
/// and change its output. Returns how many were replaced.
fn apply_replacements<I>(file: &str, cases: &mut [Case<I>]) -> usize {
    let mut replaced = replacements(file);
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

/// A JSON object deserialized as ordered pairs. serde_json here has no
/// `preserve_order`, so a `Map` would sort the keys; insert column order and
/// "last duplicate wins" row lookups both depend on source order.
#[derive(Debug, Default)]
struct Ordered(Vec<(String, Json)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut pairs = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Json>()? {
                    pairs.push((k, v));
                }
                Ok(Ordered(pairs))
            }
        }
        d.deserialize_map(V)
    }
}

impl Ordered {
    fn row_values(&self) -> RowValues {
        self.0
            .iter()
            .map(|(k, v)| (k.clone(), Value::from_json_cell(v.clone())))
            .collect()
    }
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
    assert_eq!(n, 4);
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

// ── sql.json (getExplainQuery only) ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SqlInput {
    query: String,
    #[serde(default)]
    sql: String,
    #[serde(default)]
    analyze: bool,
}

#[test]
fn explain_sql() {
    let file = "sql.json";
    let cases: Vec<_> = load::<SqlInput>(file, include_str!("fixtures/sql.json"))
        .into_iter()
        .filter(|c| c.input.query == "getExplainQuery")
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            d.explain_sql(&i.sql, i.analyze),
        )
    });
    assert_eq!(n, 4);
}

// ── crud.json ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrudInput {
    #[serde(rename = "fn")]
    func: String,
    schema: String,
    table: String,
    #[serde(default)]
    column: String,
    /// `null` and absent both mean NULL, as `undefined` binds in TS.
    #[serde(default)]
    value: Json,
    #[serde(default)]
    primary_keys: Vec<String>,
    #[serde(default)]
    row: Ordered,
    #[serde(default)]
    values: Ordered,
    casts: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrudOutput {
    sql: String,
    bind_values: Vec<Json>,
}

fn crud(d: &dyn Dialect, i: &CrudInput) -> SqlWithBindings {
    let row = i.row.row_values();
    let casts = i.casts.as_ref();
    match i.func.as_str() {
        "update" => d.build_update(
            &i.schema,
            &i.table,
            &i.column,
            Value::from_json_cell(i.value.clone()),
            &i.primary_keys,
            &row,
            casts,
        ),
        "setDefault" => {
            d.build_set_default(&i.schema, &i.table, &i.column, &i.primary_keys, &row, casts)
        }
        "insert" => d.build_insert(&i.schema, &i.table, &i.values.row_values(), casts),
        "delete" => d.build_delete(&i.schema, &i.table, &i.primary_keys, &row, casts),
        other => panic!("crud.json: unknown fn {other:?}"),
    }
}

#[test]
fn crud_builders() {
    let file = "crud.json";
    let cases = load::<CrudInput>(file, include_str!("fixtures/crud.json"));
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let out: CrudOutput =
            serde_json::from_value(c.output.clone()).expect("output is {sql, bindValues}");
        let expected = SqlWithBindings {
            sql: out.sql,
            bind_values: Some(
                out.bind_values
                    .into_iter()
                    .map(Value::from_json_cell)
                    .collect(),
            ),
        };
        let actual = crud(d, &c.input);
        (expected != actual).then(|| {
            (
                format!("{:?}", c.input),
                format!("{expected:?}"),
                format!("{actual:?}"),
            )
        })
    });
    assert_eq!(n, 12);
}

/// The TS adapter wrapped placeholders in `CAST(… AS <declared type>)` when
/// given a cast map; under SQLite's type affinity that corrupts values
/// (`CAST('2024-01-01 10:00' AS DATETIME)` is `2024`), so since Task 1 the UI
/// sends none for SQLite and the Rust dialect ignores any it gets, on values
/// and keys alike. The corpus records no cast cases for that reason.
#[test]
fn crud_ignores_casts() {
    let e = engine();
    let d = dialect(&e);
    let casts: HashMap<String, String> = [("id".to_string(), "int".to_string())]
        .into_iter()
        .collect();
    let row = vec![("id".to_string(), Value::Int(1))];
    let pks = vec!["id".to_string()];
    for built in [
        d.build_update("s", "t", "a", Value::Int(2), &pks, &row, Some(&casts)),
        d.build_set_default("s", "t", "a", &pks, &row, Some(&casts)),
        d.build_delete("s", "t", &pks, &row, Some(&casts)),
        d.build_insert("s", "t", &row, Some(&casts)),
    ] {
        assert!(!built.sql.contains("CAST"), "{}", built.sql);
    }
}

/// Fix 11: `build_set_default_expr` assigns the column's default expression
/// (SQLite has no `DEFAULT` in `UPDATE`). Executed by `tests/smoke.rs`.
#[test]
fn bugfixes_set_default() {
    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SetDefaultInput {
        schema: String,
        table: String,
        column: String,
        column_default: Option<String>,
        primary_keys: Vec<String>,
        row: Ordered,
    }
    let file = "bugfixes.json";
    let cases: Vec<Case<SetDefaultInput>> = bugfixes()
        .into_iter()
        .filter(|c| c.kind == "crud-set-default")
        .map(|c| {
            assert_eq!(c.fix, 11, "{file}: {:?}", c.name);
            Case {
                name: c.name,
                input: serde_json::from_value(c.input).expect("set-default input"),
                output: c.output,
            }
        })
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        let i = &c.input;
        let out: CrudOutput = serde_json::from_value(c.output.clone()).expect("{sql, bindValues}");
        let expected = SqlWithBindings {
            sql: out.sql,
            bind_values: Some(
                out.bind_values
                    .into_iter()
                    .map(Value::from_json_cell)
                    .collect(),
            ),
        };
        let actual = d.build_set_default_expr(
            &i.schema,
            &i.table,
            &i.column,
            i.column_default.as_deref(),
            &i.primary_keys,
            &i.row.row_values(),
            None,
        );
        (expected != actual).then(|| {
            (
                format!("{i:?}"),
                format!("{expected:?}"),
                format!("{actual:?}"),
            )
        })
    });
    assert_eq!(n, 17);
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

/// Fix 2 replaces the case with `"` in its names.
#[test]
fn ddl_create() {
    let file = "ddl-create.json";
    let mut cases = load::<CreateTableDefinition>(file, include_str!("fixtures/ddl-create.json"));
    assert_eq!(apply_replacements(file, &mut cases), 1);
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.create_table(&c.input),
        )
    });
    assert_eq!(n, 10);
}

/// Fixes 3, 4 and 6 replace seven recorded cases (see `bugfixes.json`).
#[test]
fn ddl_alter() {
    let file = "ddl-alter.json";
    let mut cases = load::<AlterInput>(file, include_str!("fixtures/ddl-alter.json"));
    assert_eq!(apply_replacements(file, &mut cases), 7);
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
    assert_eq!(n, 15);
}

/// The TS adapter's `generateAddColumnSql`. It isn't on the `Dialect` trait
/// (nothing calls it over the wire), so the crate exposes it directly.
#[test]
fn ddl_add_column() {
    let file = "ddl-add-column.json";
    let cases = load::<AddColumnInput>(file, include_str!("fixtures/ddl-add-column.json"));
    let n = run(file, &cases, |c| {
        let i = &c.input;
        compare_str(
            format!("{i:?}"),
            expect_str(&c.output),
            seaquel_engine_sqlite::SqliteDialect.add_column(&i.schema, &i.table, &i.column),
        )
    });
    assert_eq!(n, 3);
}

// ── bugfixes.json (fixes 2, 3, 4, 6 and 7) ─────────────────────────────────────

/// The hand-written DDL cases that don't replace a recorded one.
#[test]
fn bugfixes_ddl() {
    let file = "bugfixes.json";
    let cases: Vec<Case<Json>> = bugfixes()
        .into_iter()
        .filter(|c| c.kind.starts_with("ddl-") && c.replaces.is_none())
        .map(|c| {
            assert!(
                matches!(c.fix, 2 | 3 | 4 | 6 | 7),
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
            seaquel_engine_sqlite::SqliteDialect.add_column(&i.schema, &i.table, &i.column)
        } else {
            let i: AlterInput = serde_json::from_value(c.input.clone()).unwrap();
            d.alter_table(&i.from, &i.to)
        };
        compare_str(c.input.to_string(), expect_str(&c.output), actual)
    });
    assert_eq!(n, BUGFIX_DDL_CASES);
}

const BUGFIX_DDL_CASES: usize = 9;
