//! Parity between `PostgresDialect` and the recorded TypeScript Postgres
//! dialect (`tests/fixtures/*.json`, now frozen; see `tests/fixtures/README.md`).
//!
//! One test per fixture file. Each runs every case, then fails once with the
//! name, input, expected and actual output of every case that differed.
//! `bugfixes.json` cases for fixes 2, 3, 6 and 7 run here as well; fixes 1 and 4
//! need a database and run with the introspection tests. crud.json is
//! replayed without casts on key columns (fix 6, see `without_pk_casts`).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value as Json;

use seaquel_engine::{Dialect, Engine, RowValues, SqlWithBindings, Value};
use seaquel_types::{ColumnTypeInfo, CreateTableDefinition};

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
    seaquel_engine_postgres::engine()
}

fn dialect(engine: &Arc<dyn Engine>) -> &dyn Dialect {
    engine
        .dialect()
        .expect("PostgresEngine::dialect() returns the Postgres dialect")
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
    assert_eq!(n, 3);
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

/// Bug fix 6 casts primary-key placeholders; TypeScript never did. Two
/// crud.json cases have a cast for a key column (`id: integer`,
/// `region: text`), so the recording is replayed without casts for key
/// columns. `bugfixes_crud` covers the key casts.
fn without_pk_casts(mut i: CrudInput) -> CrudInput {
    if let Some(casts) = i.casts.as_mut() {
        casts.retain(|column, _| !i.primary_keys.contains(column));
    }
    i
}

fn compare_crud(
    d: &dyn Dialect,
    input: &CrudInput,
    output: &Json,
) -> Option<(String, String, String)> {
    let out: CrudOutput =
        serde_json::from_value(output.clone()).expect("output is {sql, bindValues}");
    let expected = SqlWithBindings {
        sql: out.sql,
        bind_values: Some(
            out.bind_values
                .into_iter()
                .map(Value::from_json_cell)
                .collect(),
        ),
    };
    let actual = crud(d, input);
    (expected != actual).then(|| {
        (
            format!("{input:?}"),
            format!("{expected:?}"),
            format!("{actual:?}"),
        )
    })
}

#[test]
fn crud_builders() {
    let file = "crud.json";
    let cases: Vec<Case<CrudInput>> = load::<CrudInput>(file, include_str!("fixtures/crud.json"))
        .into_iter()
        .map(|c| Case {
            name: c.name,
            input: without_pk_casts(c.input),
            output: c.output,
        })
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| compare_crud(d, &c.input, &c.output));
    assert_eq!(n, 24);
}

// ── ddl-create.json / ddl-alter.json ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AlterInput {
    from: CreateTableDefinition,
    to: CreateTableDefinition,
}

#[test]
fn ddl_create() {
    let file = "ddl-create.json";
    let cases = load::<CreateTableDefinition>(file, include_str!("fixtures/ddl-create.json"));
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| {
        compare_str(
            format!("{:?}", c.input),
            expect_str(&c.output),
            d.create_table(&c.input),
        )
    });
    assert_eq!(n, 12);
}

#[test]
fn ddl_alter() {
    let file = "ddl-alter.json";
    let cases = load::<AlterInput>(file, include_str!("fixtures/ddl-alter.json"));
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
    assert_eq!(n, 20);
}

// ── bugfixes.json (fixes 2, 3, 6 and 7) ─────────────────────────────────────────

#[derive(Deserialize)]
struct BugfixFixture {
    cases: Vec<BugfixCase>,
}

#[derive(Deserialize)]
struct BugfixCase {
    name: String,
    fix: u8,
    kind: String,
    input: Json,
    output: Json,
}

#[test]
fn bugfixes_ddl() {
    let file = "bugfixes.json";
    let all: BugfixFixture = serde_json::from_str(include_str!("fixtures/bugfixes.json"))
        .unwrap_or_else(|e| panic!("{file}: cannot parse fixture: {e}"));
    // Fixes 1 and 4 (kinds `columns`/`indexes`) need a database: Task 6.
    let cases: Vec<Case<Json>> = all
        .cases
        .into_iter()
        .filter(|c| matches!(c.fix, 2 | 3))
        .map(|c| {
            assert!(
                matches!(c.kind.as_str(), "ddl-create" | "ddl-alter"),
                "{file}: fix {} case {:?} has unexpected kind {:?}",
                c.fix,
                c.name,
                c.kind
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
        } else {
            let i: AlterInput = serde_json::from_value(c.input.clone()).unwrap();
            d.alter_table(&i.from, &i.to)
        };
        compare_str(c.input.to_string(), expect_str(&c.output), actual)
    });
    assert_eq!(n, 6);
}

#[test]
fn bugfixes_crud() {
    let file = "bugfixes.json";
    let all: BugfixFixture = serde_json::from_str(include_str!("fixtures/bugfixes.json"))
        .unwrap_or_else(|e| panic!("{file}: cannot parse fixture: {e}"));
    let cases: Vec<Case<CrudInput>> = all
        .cases
        .into_iter()
        .filter(|c| c.fix == 6 || c.fix == 7)
        .map(|c| {
            assert_eq!(c.kind, "crud", "{file}: fix {} case {:?}", c.fix, c.name);
            Case {
                name: c.name,
                input: serde_json::from_value(c.input).expect("crud input"),
                output: c.output,
            }
        })
        .collect();
    let e = engine();
    let d = dialect(&e);
    let n = run(file, &cases, |c| compare_crud(d, &c.input, &c.output));
    assert_eq!(n, 5);
}
