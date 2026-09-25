//! Parity between the MySQL introspection parsers and SQL
//! (`seaquel_engine_mysql::introspect`) and the TypeScript MySQL adapter
//! (deleted in phase 2), recorded against both servers into
//! `tests/fixtures/{mysql,mariadb}/` (frozen).
//!
//! Parse inputs are the rows the adapter's own SQL returned through Seaquel's
//! Rust driver, stored in the Value wire format: cells are read with
//! `Value::from_wire`, and each result is rebuilt in its stored `columns`
//! order. Outputs are compared as typed structs, not raw JSON (`100` and
//! `100.0` are the same `f64`).
//!
//! A `bugfixes.json` case with `replaces` supersedes the recorded case of that
//! name (fixes 4, 5, 8 and 10 change parser output): its output is expected
//! instead, and the recorded TypeScript output must differ from it.
//!
//! MariaDB's EXPLAIN goes through its own branch (`Flavor::Mariadb`), which
//! the TypeScript adapter never had; the recorded MariaDB plans are replayed
//! through the MySQL branch for parity, and `mariadb_explain_*` checks the
//! MariaDB branch against hand-written expectations.

use std::collections::HashMap;
use std::fmt::Debug;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value as Json};

use seaquel_engine::{QueryResult, Value};
use seaquel_engine_mysql::introspect::{self, Flavor};
use seaquel_types::{
    DatabaseOverview, ExplainPlanNode, ExplainResult, IndexUsageInfo, SchemaColumn, SchemaIndex,
    SchemaTable, TableSizeInfo,
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

#[derive(Clone, Copy)]
enum Server {
    Mysql,
    Mariadb,
}

impl Server {
    const BOTH: [Server; 2] = [Server::Mysql, Server::Mariadb];

    fn dir(self) -> &'static str {
        match self {
            Server::Mysql => "mysql",
            Server::Mariadb => "mariadb",
        }
    }

    fn flavor(self) -> Flavor {
        match self {
            Server::Mysql => Flavor::Mysql,
            Server::Mariadb => Flavor::Mariadb,
        }
    }

    fn fixture(self, file: &str) -> &'static str {
        match (self, file) {
            (Server::Mysql, "parse-schema.json") => {
                include_str!("fixtures/mysql/parse-schema.json")
            }
            (Server::Mysql, "parse-columns.json") => {
                include_str!("fixtures/mysql/parse-columns.json")
            }
            (Server::Mysql, "parse-indexes.json") => {
                include_str!("fixtures/mysql/parse-indexes.json")
            }
            (Server::Mysql, "parse-stats.json") => include_str!("fixtures/mysql/parse-stats.json"),
            (Server::Mysql, "parse-explain.json") => {
                include_str!("fixtures/mysql/parse-explain.json")
            }
            (Server::Mysql, "bugfixes.json") => include_str!("fixtures/mysql/bugfixes.json"),
            (Server::Mysql, "sql.json") => include_str!("fixtures/mysql/sql.json"),
            (Server::Mariadb, "parse-schema.json") => {
                include_str!("fixtures/mariadb/parse-schema.json")
            }
            (Server::Mariadb, "parse-columns.json") => {
                include_str!("fixtures/mariadb/parse-columns.json")
            }
            (Server::Mariadb, "parse-indexes.json") => {
                include_str!("fixtures/mariadb/parse-indexes.json")
            }
            (Server::Mariadb, "parse-stats.json") => {
                include_str!("fixtures/mariadb/parse-stats.json")
            }
            (Server::Mariadb, "parse-explain.json") => {
                include_str!("fixtures/mariadb/parse-explain.json")
            }
            (Server::Mariadb, "bugfixes.json") => include_str!("fixtures/mariadb/bugfixes.json"),
            (_, other) => panic!("no fixture {other}"),
        }
    }

    fn load<I: DeserializeOwned>(self, file: &str) -> Vec<Case<I>> {
        serde_json::from_str::<Fixture<I>>(self.fixture(file))
            .unwrap_or_else(|e| panic!("{}/{file}: cannot parse fixture: {e}", self.dir()))
            .cases
    }

    fn bugfixes(self) -> Vec<BugfixCase> {
        #[derive(Deserialize)]
        struct Bugfixes {
            cases: Vec<BugfixCase>,
        }
        serde_json::from_str::<Bugfixes>(self.fixture("bugfixes.json"))
            .unwrap_or_else(|e| panic!("{}/bugfixes.json: {e}", self.dir()))
            .cases
    }
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

/// Swap in the bug-fix outputs for `file`'s replaced cases. Every
/// replacement must name a recorded case and change its output. Returns how
/// many cases were replaced.
fn apply_replacements<I>(server: Server, file: &str, cases: &mut [Case<I>]) -> usize {
    let prefix = format!("{file}: ");
    let mut replaced: HashMap<String, Json> = server
        .bugfixes()
        .into_iter()
        .filter_map(|c| Some((c.replaces?.strip_prefix(&prefix)?.to_string(), c.output)))
        .collect();
    let mut n = 0;
    for case in cases.iter_mut() {
        if let Some(output) = replaced.remove(&case.name) {
            assert_ne!(
                output,
                case.output,
                "{}/{file}: the bug fix for {:?} doesn't change the recorded output",
                server.dir(),
                case.name
            );
            case.output = output;
            n += 1;
        }
    }
    assert!(
        replaced.is_empty(),
        "{}/{file}: bugfixes.json replaces cases that weren't recorded: {:?}",
        server.dir(),
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

/// Like [`compare`] for EXPLAIN results, with floats equal within a few
/// ULPs: the workspace's serde_json parses without `float_roundtrip`, so a
/// recorded `0.9099999999999999` (JS `0.71 + 0.20`) reads back as `0.91`.
fn compare_plan(output: &Json, actual: &ExplainResult) -> Option<String> {
    fn close(a: &Json, b: &Json) -> bool {
        match (a, b) {
            (Json::Number(x), Json::Number(y)) => {
                let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                x == y || (x - y).abs() <= 4.0 * f64::EPSILON * x.abs().max(y.abs())
            }
            (Json::Array(x), Json::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| close(a, b))
            }
            (Json::Object(x), Json::Object(y)) => {
                let keys = |m: &Map<String, Json>| {
                    m.iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, _)| k.clone())
                        .collect::<Vec<_>>()
                };
                keys(x) == keys(y)
                    && x.iter()
                        .filter(|(_, v)| !v.is_null())
                        .all(|(k, v)| close(v, &y[k]))
            }
            _ => a == b,
        }
    }
    // Round-trip the expectation through the typed struct first, so `null`s
    // and integer/float spellings match what `actual` serializes to.
    let expected: ExplainResult = serde_json::from_value(output.clone())
        .unwrap_or_else(|e| panic!("fixture output doesn't deserialize: {e}\n{output}"));
    let (e, a) = (
        serde_json::to_value(&expected).unwrap(),
        serde_json::to_value(actual).unwrap(),
    );
    (!close(&e, &a)).then(|| format!("  expected: {expected:#?}\n  actual:   {actual:#?}"))
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
        let rows = self
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
            .collect();
        QueryResult {
            columns: self.columns.clone(),
            rows,
        }
    }
}

// ── parse-schema.json ────────────────────────────────────────────────────────

#[test]
fn parse_schema() {
    for server in Server::BOTH {
        let cases = server.load::<Recorded>("parse-schema.json");
        run(
            &format!("{}/parse-schema.json", server.dir()),
            &cases,
            |c| {
                compare::<Vec<SchemaTable>>(&c.output, &introspect::parse_schema(&c.input.result()))
            },
        );
    }
}

// ── parse-columns.json ───────────────────────────────────────────────────────

/// Fix 4 (UTF-8, not Latin-1) changes one MySQL case; fix 8 (MariaDB's
/// bare `NULL` is no default) changes five MariaDB cases. The TS query's
/// MySQL rows have no `extra`, so fix 8's quoting doesn't apply to them; see
/// `bugfixes_columns`.
#[test]
fn parse_columns() {
    for (server, replaced) in [(Server::Mysql, 1), (Server::Mariadb, 5)] {
        let file = "parse-columns.json";
        let mut cases = server.load::<Recorded>(file);
        assert_eq!(apply_replacements(server, file, &mut cases), replaced);
        run(&format!("{}/{file}", server.dir()), &cases, |c| {
            compare::<Vec<SchemaColumn>>(
                &c.output,
                &introspect::parse_columns(&c.input.result(), server.flavor()),
            )
        });
    }
}

/// Hand-written `parse-columns` cases (fix 8 on MySQL rows with `extra`).
#[test]
fn bugfixes_columns() {
    let cases: Vec<Case<Recorded>> = Server::Mysql
        .bugfixes()
        .into_iter()
        .filter(|c| c.kind == "parse-columns" && c.replaces.is_none())
        .map(|c| Case {
            name: c.name,
            input: serde_json::from_value(c.input).expect("columns input"),
            output: c.output,
        })
        .collect();
    let n = run("mysql/bugfixes.json", &cases, |c| {
        compare::<Vec<SchemaColumn>>(
            &c.output,
            &introspect::parse_columns(&c.input.result(), Flavor::Mysql),
        )
    });
    assert_eq!(n, 1);
}

// ── parse-indexes.json ───────────────────────────────────────────────────────

#[test]
fn parse_indexes() {
    for server in Server::BOTH {
        let cases = server.load::<Recorded>("parse-indexes.json");
        run(
            &format!("{}/parse-indexes.json", server.dir()),
            &cases,
            |c| {
                compare::<Vec<SchemaIndex>>(
                    &c.output,
                    &introspect::parse_indexes(&c.input.result()),
                )
            },
        );
    }
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
    for server in Server::BOTH {
        let file = "parse-stats.json";
        let cases = server.load::<StatsInput>(file);
        run(&format!("{}/{file}", server.dir()), &cases, |c| {
            let r = c.input.recorded.result();
            match c.input.kind.as_str() {
                "tableSizes" => {
                    compare::<Vec<TableSizeInfo>>(&c.output, &introspect::parse_table_sizes(&r))
                }
                "indexUsage" => {
                    compare::<Vec<IndexUsageInfo>>(&c.output, &introspect::parse_index_usage(&r))
                }
                "overview" => {
                    compare::<DatabaseOverview>(&c.output, &introspect::parse_overview(&r))
                }
                other => panic!("{file}: unknown kind {other:?}"),
            }
        });
    }
}

// ── parse-explain.json ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExplainInput {
    analyze: bool,
    #[serde(flatten)]
    recorded: Recorded,
}

/// Every recorded plan through the MySQL branch (the TypeScript parser's).
/// Fix 5 (robust `EXPLAIN ANALYZE` text) and fix 10 (a JSON v2 plan at the
/// root) replace the recorded cases they change.
#[test]
fn parse_explain() {
    for (server, replaced) in [(Server::Mysql, 23), (Server::Mariadb, 0)] {
        let file = "parse-explain.json";
        let mut cases = server.load::<ExplainInput>(file);
        assert_eq!(apply_replacements(server, file, &mut cases), replaced);
        run(&format!("{}/{file}", server.dir()), &cases, |c| {
            let actual = introspect::parse_explain(
                &c.input.recorded.result(),
                c.input.analyze,
                Flavor::Mysql,
            )
            .expect("parse_explain");
            compare_plan(&c.output, &actual)
        });
    }
}

// ── bugfixes.json (fixes 5 and 10, parser only) ─────────────────────────────

#[test]
fn bugfixes_explain() {
    let cases: Vec<Case<ExplainInput>> = Server::Mysql
        .bugfixes()
        .into_iter()
        .filter(|c| c.kind == "parse-explain" && c.replaces.is_none())
        .map(|c| Case {
            name: c.name,
            input: serde_json::from_value(c.input).expect("explain input"),
            output: c.output,
        })
        .collect();
    let n = run("mysql/bugfixes.json", &cases, |c| {
        let actual =
            introspect::parse_explain(&c.input.recorded.result(), c.input.analyze, Flavor::Mysql)
                .expect("parse_explain");
        compare_plan(&c.output, &actual)
    });
    assert_eq!(n, 6);
}

// ── sql.json ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SqlInput {
    query: String,
}

/// Every catalog query equals the recorded TS text except for bug fixes 1
/// (bound filters), 3 (foreign keys from `KEY_COLUMN_USAGE`, as separate
/// columns), 4 (`table_type` aliased, so the parser finds it) and 9 (index
/// usage reads `database_name`). The fixes are applied to the recorded text
/// here, so a stray edit anywhere else still fails. The EXPLAIN cases run in
/// `dialect_parity.rs`.
#[test]
fn sql() {
    let file = "sql.json";
    let cases: Vec<_> = Server::Mysql
        .load::<SqlInput>(file)
        .into_iter()
        .filter(|c| c.input.query != "getExplainQuery")
        .collect();
    let fix9 = Server::Mysql
        .bugfixes()
        .into_iter()
        .find(|c| c.fix == 9 && c.kind == "sql")
        .expect("fix 9 sql case");
    run("mysql/sql.json", &cases, |c| {
        let recorded = c.output.as_str().expect("sql.json output is a string");
        let (expected, actual) = match c.input.query.as_str() {
            "getSchemasQuery" => (recorded.to_string(), introspect::SCHEMAS_SQL),
            "getTableSizesQuery" => (recorded.to_string(), introspect::TABLE_SIZES_SQL),
            "getDatabaseOverviewQuery" => (recorded.to_string(), introspect::OVERVIEW_SQL),
            // Fix 4: `table_type` is aliased; MySQL returns the bare column as
            // `TABLE_TYPE`, which the parser (and the TS) never read.
            "getSchemaQuery" => (
                recorded.replace("\t\t\ttable_type\n", "\t\t\ttable_type AS table_type\n"),
                introspect::SCHEMA_SQL,
            ),
            // Fix 9.
            "getIndexUsageQuery" => {
                assert_eq!(
                    fix9.output.as_str(),
                    Some(recorded.replace("TABLE_SCHEMA", "database_name").as_str())
                );
                (
                    recorded.replace("TABLE_SCHEMA", "database_name"),
                    introspect::INDEX_USAGE_SQL,
                )
            }
            // Fix 1: bound, not spliced.
            "getIndexesQuery" => (
                recorded.replace(
                    "WHERE TABLE_NAME = 'users' AND TABLE_SCHEMA = 'seaquel_test'",
                    "WHERE TABLE_NAME = ? AND TABLE_SCHEMA = ?",
                ),
                introspect::INDEXES_SQL,
            ),
            // Fixes 1 and 3.
            "getColumnsQuery" => (
                recorded
                    .replace(
                        "WHERE c.TABLE_NAME = 'users' AND c.TABLE_SCHEMA = 'seaquel_test'",
                        "WHERE c.TABLE_NAME = ? AND c.TABLE_SCHEMA = ?",
                    )
                    .replace("IF(c.COLUMN_KEY = 'MUL' AND EXISTS (", "IF(EXISTS (")
                    // Fix 8.
                    .replace(
                        "c.COLUMN_DEFAULT AS column_default,\n",
                        "c.COLUMN_DEFAULT AS column_default,\n\t\t\tc.EXTRA AS extra,\n",
                    )
                    .replace(FK_REF_TS, FK_REF_RUST),
                introspect::COLUMNS_SQL,
            ),
            other => panic!("{file}: unknown query {other:?}"),
        };
        (expected != actual).then(|| format!("  expected: {expected}\n  actual:   {actual}"))
    });
}

/// Fix 3: the TS `CONCAT(schema, '.', table, '.', column)`, which breaks on a
/// dotted name, …
const FK_REF_TS: &str = "\t\t\t(SELECT CONCAT(kcu.REFERENCED_TABLE_SCHEMA, '.', kcu.REFERENCED_TABLE_NAME, '.', kcu.REFERENCED_COLUMN_NAME)
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_ref";

/// … becomes three columns, read from the same (first by constraint name)
/// foreign key.
const FK_REF_RUST: &str = "\t\t\t(SELECT kcu.REFERENCED_TABLE_SCHEMA
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_schema,
\t\t\t(SELECT kcu.REFERENCED_TABLE_NAME
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_table,
\t\t\t(SELECT kcu.REFERENCED_COLUMN_NAME
\t\t\t\tFROM information_schema.KEY_COLUMN_USAGE kcu
\t\t\t\tWHERE kcu.TABLE_SCHEMA = c.TABLE_SCHEMA
\t\t\t\t\tAND kcu.TABLE_NAME = c.TABLE_NAME
\t\t\t\t\tAND kcu.COLUMN_NAME = c.COLUMN_NAME
\t\t\t\t\tAND kcu.REFERENCED_TABLE_NAME IS NOT NULL
\t\t\t\tORDER BY kcu.CONSTRAINT_NAME
\t\t\t\tLIMIT 1
\t\t\t) AS foreign_key_column";

// ── Branches no recording reaches ────────────────────────────────────────────

fn one_row(cells: &[(&str, Value)]) -> QueryResult {
    QueryResult {
        columns: cells.iter().map(|(c, _)| c.to_string()).collect(),
        rows: vec![cells.iter().map(|(_, v)| v.clone()).collect()],
    }
}

/// One column row; `extra` cells replace or follow the defaults.
fn column_row(default: Value, extra: &[(&str, Value)]) -> QueryResult {
    let mut cells = vec![
        ("column_name", Value::from("c")),
        ("data_type", Value::from("varchar(10)")),
        ("is_nullable", Value::from("YES")),
        ("column_default", default),
        ("is_primary_key", Value::Int(0)),
        ("is_foreign_key", Value::Int(0)),
    ];
    for (name, value) in extra {
        match cells.iter_mut().find(|(n, _)| n == name) {
            Some(cell) => cell.1 = value.clone(),
            None => cells.push((name, value.clone())),
        }
    }
    one_row(&cells)
}

fn bytes(s: &str) -> Value {
    Value::Array(s.bytes().map(|b| Value::Int(i64::from(b))).collect())
}

/// Fix 8 on single values: every default is a SQL expression. MariaDB's
/// already is (only its bare `NULL` means none); MySQL's literal values are
/// quoted unless numeric, BIT or an expression (`DEFAULT_GENERATED`).
#[test]
fn defaults_as_sql_expressions() {
    let mariadb = |raw: &str| {
        introspect::parse_columns(&column_row(Value::from(raw), &[]), Flavor::Mariadb)[0]
            .default_value
            .clone()
    };
    assert_eq!(mariadb("NULL"), None);
    for kept in [
        "'NULL'",
        "'it''s'",
        r"'a\\b'",
        "''",
        "current_timestamp()",
        "0.00",
        "b'1'",
    ] {
        assert_eq!(mariadb(kept).as_deref(), Some(kept));
    }
    let mysql = |raw: Value, extra: &str, ty: &str| {
        let r = column_row(
            raw,
            &[
                ("extra", Value::from(extra)),
                ("data_type", Value::from(ty)),
            ],
        );
        introspect::parse_columns(&r, Flavor::Mysql)[0]
            .default_value
            .clone()
    };
    let s = |v: &str| Value::from(v);
    assert_eq!(mysql(Value::Null, "", "varchar(10)"), None);
    assert_eq!(
        mysql(s("active"), "", "varchar(20)").as_deref(),
        Some("'active'")
    );
    assert_eq!(
        mysql(s("it's"), "", "varchar(20)").as_deref(),
        Some("'it''s'")
    );
    assert_eq!(
        mysql(s(r"a\b"), "", "varchar(20)").as_deref(),
        Some(r"'a\\b'")
    );
    assert_eq!(
        mysql(s("NULL"), "", "varchar(10)").as_deref(),
        Some("'NULL'")
    );
    assert_eq!(mysql(s(""), "", "varchar(10)").as_deref(), Some("''"));
    assert_eq!(mysql(bytes(""), "", "text").as_deref(), Some("''"));
    assert_eq!(mysql(bytes("é"), "", "varchar(10)").as_deref(), Some("'é'"));
    assert_eq!(mysql(s("q"), "", "enum('p','q')").as_deref(), Some("'q'"));
    assert_eq!(
        mysql(s("2024-01-02"), "", "date").as_deref(),
        Some("'2024-01-02'")
    );
    assert_eq!(
        mysql(s("0.00"), "", "decimal(10,2)").as_deref(),
        Some("0.00")
    );
    assert_eq!(mysql(s("5"), "", "int unsigned").as_deref(), Some("5"));
    assert_eq!(mysql(s("b'1'"), "", "bit(1)").as_deref(), Some("b'1'"));
    assert_eq!(
        mysql(s("CURRENT_TIMESTAMP"), "DEFAULT_GENERATED", "datetime").as_deref(),
        Some("CURRENT_TIMESTAMP")
    );
    assert_eq!(
        mysql(
            s("CURRENT_TIMESTAMP(3)"),
            "DEFAULT_GENERATED on update CURRENT_TIMESTAMP(3)",
            "datetime(3)"
        )
        .as_deref(),
        Some("CURRENT_TIMESTAMP(3)")
    );
    assert_eq!(
        mysql(s("json_array()"), "DEFAULT_GENERATED", "json").as_deref(),
        Some("(json_array())")
    );
    assert_eq!(
        mysql(
            s(r"concat(_utf8mb4\'a\',_utf8mb4\'b\')"),
            "DEFAULT_GENERATED",
            "varchar(10)"
        )
        .as_deref(),
        Some("(concat(_utf8mb4'a',_utf8mb4'b'))")
    );
    // The TS query's rows (no `extra`) keep the TS value.
    let c = &introspect::parse_columns(&column_row(s("active"), &[]), Flavor::Mysql)[0];
    assert_eq!(c.default_value.as_deref(), Some("active"));
}

/// Fix 3 on the parser: the separate reference columns win over
/// `foreign_key_ref`, and a dotted name survives.
#[test]
fn foreign_key_reference_columns() {
    let r = column_row(
        Value::Null,
        &[
            ("is_foreign_key", Value::Int(1)),
            ("foreign_key_schema", Value::from("my.db")),
            ("foreign_key_table", bytes("t.1")),
            ("foreign_key_column", Value::from("id")),
        ],
    );
    let c = &introspect::parse_columns(&r, Flavor::Mysql)[0];
    assert!(c.is_foreign_key);
    let fk = c.foreign_key_ref.as_ref().expect("reference");
    assert_eq!(
        (
            fk.referenced_schema.as_str(),
            fk.referenced_table.as_str(),
            fk.referenced_column.as_str()
        ),
        ("my.db", "t.1", "id")
    );
    // No referenced table: no reference.
    let r = column_row(
        Value::Null,
        &[
            ("foreign_key_schema", Value::Null),
            ("foreign_key_table", Value::Null),
            ("foreign_key_column", Value::Null),
        ],
    );
    assert_eq!(
        introspect::parse_columns(&r, Flavor::Mysql)[0].foreign_key_ref,
        None
    );
}

/// Fix 4 on the parser: byte cells are UTF-8; TS `decodeValue` truthiness
/// rules otherwise hold (an empty byte string is a `""` default, an empty
/// text cell none).
#[test]
fn catalog_bytes_are_utf8() {
    let c = &introspect::parse_columns(&column_row(bytes("é"), &[]), Flavor::Mysql)[0];
    assert_eq!(c.default_value.as_deref(), Some("é"));
    let c = &introspect::parse_columns(&column_row(Value::Bytes(vec![]), &[]), Flavor::Mysql)[0];
    assert_eq!(c.default_value.as_deref(), Some(""));
    let c = &introspect::parse_columns(&column_row(Value::from(""), &[]), Flavor::Mysql)[0];
    assert_eq!(c.default_value, None);
    let schema = introspect::parse_schema(&one_row(&[
        ("schema_name", Value::Bytes("sé".into())),
        ("table_name", bytes("tä")),
        ("table_type", bytes("VIEW")),
    ]));
    assert_eq!(schema[0].name, "tä");
    assert_eq!(schema[0].schema, "sé");
    assert_eq!(schema[0].kind, seaquel_types::TableKind::View);
    // TS drops rows whose name decodes to "".
    let schema = introspect::parse_schema(&one_row(&[
        ("schema_name", Value::from("s")),
        ("table_name", Value::Null),
        ("table_type", Value::from("BASE TABLE")),
    ]));
    assert!(schema.is_empty());
}

fn explain_text(text: &str) -> QueryResult {
    one_row(&[("EXPLAIN", Value::from(text))])
}

fn find<'a>(node: &'a ExplainPlanNode, relation: &str) -> Option<&'a ExplainPlanNode> {
    if node.relation_name.as_deref() == Some(relation) {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, relation))
}

/// Invalid JSON is an error, not a panic; an empty result is an empty plan.
#[test]
fn parse_explain_edge_cases() {
    let bad = one_row(&[("EXPLAIN", Value::from("not json"))]);
    assert!(introspect::parse_explain(&bad, false, Flavor::Mysql).is_err());
    let empty = QueryResult {
        columns: vec![],
        rows: vec![],
    };
    for flavor in [Flavor::Mysql, Flavor::Mariadb] {
        let r = introspect::parse_explain(&empty, false, flavor).unwrap();
        assert_eq!(r.plan.node_type, "Query");
        assert!(r.plan.children.is_empty());
        let r = introspect::parse_explain(&empty, true, flavor).unwrap();
        assert!(r.is_analyze);
    }
    // A plan as a JSON cell (native decoding) reads like its text.
    let plan = serde_json::json!({ "query_block": { "table": { "table_name": "t", "access_type": "ref" } } });
    let r = introspect::parse_explain(
        &one_row(&[("EXPLAIN", Value::Json(plan))]),
        false,
        Flavor::Mysql,
    )
    .unwrap();
    assert_eq!(r.plan.node_type, "Index Scan");
    // Analyze text that isn't text reads as no operators.
    let r = introspect::parse_explain(&one_row(&[("EXPLAIN", Value::Int(1))]), true, Flavor::Mysql)
        .unwrap();
    assert_eq!(
        (r.plan.node_type.as_str(), r.plan.children.len()),
        ("Query", 0)
    );
    // Fix 5: a relation name is found after the head's " on ", not in its
    // condition, and a trailing " over" (range scans) isn't part of the index.
    let r = introspect::parse_explain(
        &explain_text("-> Index range scan on c using PRIMARY over (id < 5)  (cost=1 rows=2)"),
        true,
        Flavor::Mysql,
    )
    .unwrap();
    assert_eq!(r.plan.index_name.as_deref(), Some("PRIMARY"));
    assert!(find(&r.plan, "c").is_some());
}

// ── MariaDB EXPLAIN ──────────────────────────────────────────────────────────

fn mariadb_plan(name: &str) -> ExplainResult {
    let cases = Server::Mariadb.load::<ExplainInput>("parse-explain.json");
    let case = cases
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no recorded MariaDB plan {name:?}"));
    let analyze = name.starts_with("ANALYZE FORMAT=JSON");
    introspect::parse_explain(&case.input.recorded.result(), analyze, Flavor::Mariadb)
        .expect("parse_explain")
}

fn shape(node: &ExplainPlanNode) -> String {
    let mut out = node.node_type.clone();
    if let Some(r) = &node.relation_name {
        out.push_str(&format!(" on {r}"));
    }
    if !node.children.is_empty() {
        let kids: Vec<String> = node.children.iter().map(shape).collect();
        out.push_str(&format!(" [{}]", kids.join(", ")));
    }
    out
}

/// Every node, depth first.
fn nodes(node: &ExplainPlanNode) -> Vec<&ExplainPlanNode> {
    let mut out = vec![node];
    for c in &node.children {
        out.extend(nodes(c));
    }
    out
}

/// MariaDB reports `rows` and `cost`, not MySQL's `rows_examined_per_scan`
/// and `cost_info`, and wraps tables in `filesort`/`temporary_table`/…,
/// so the MySQL branch shows no numbers (and sometimes no tables) for it.
#[test]
fn mariadb_explain_reads_rows_costs_and_wrappers() {
    let join = mariadb_plan("EXPLAIN: join");
    assert_eq!(
        shape(&join.plan),
        "Nested Loop [Table Scan on o, Index Scan on c]"
    );
    let o = find(&join.plan, "o").unwrap();
    assert_eq!(o.plan_rows, Some(1000.0));
    assert!(o.total_cost.is_some_and(|c| c > 0.0), "{o:?}");
    assert_eq!(o.filter.as_deref(), Some("o.kind = 'buy'"));
    let c = find(&join.plan, "c").unwrap();
    assert_eq!(c.index_name.as_deref(), Some("PRIMARY"));
    assert_eq!(c.plan_rows, Some(1.0));
    assert!(
        join.plan.total_cost.is_some(),
        "the root carries the query cost"
    );
    assert!(!join.is_analyze);
    assert_eq!(join.execution_time, None);

    // `filesort` over `temporary_table` over the scan.
    let group = mariadb_plan("EXPLAIN: group by");
    assert_eq!(
        shape(&group.plan),
        "Sort [Temporary Table [Table Scan on fx_orders]]"
    );
    assert_eq!(
        group.plan.sort_key.as_deref(),
        Some(&["fx_orders.kind".to_string()][..])
    );

    // A derived table's materialized query block, and a union's blocks.
    let derived = mariadb_plan("EXPLAIN: derived table");
    assert_eq!(
        shape(&derived.plan),
        "Table Scan on <derived2> [Sort [Temporary Table [Index Scan on fx_orders]]]"
    );
    let union = mariadb_plan("EXPLAIN: union");
    assert_eq!(
        shape(&union.plan),
        "Union on <union1,2> [Range Scan on fx_customers, Range Scan on fx_orders]"
    );

    // Subqueries hang off the query block.
    let sub = mariadb_plan("EXPLAIN: subquery in WHERE");
    assert_eq!(
        shape(&sub.plan),
        "Query Block [Table Scan on fx_customers, Table Scan on fx_orders]"
    );

    // Node ids are unique, pre-order.
    let ids: Vec<&str> = nodes(&derived.plan).iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, ["node-0", "node-1", "node-2", "node-3"]);
}

/// Window functions, HAVING, and wrappers the parser doesn't know: the
/// tables inside are still found.
#[test]
fn mariadb_explain_walks_windows_having_and_unknown_wrappers() {
    let plan = serde_json::json!({
        "query_block": {
            "r_loops": 1,
            "r_total_time_ms": 0.2,
            "having_condition": "sum(orders.total) > 10",
            "filesort": {
                "sort_key": "orders.user_id",
                "r_loops": 2,
                "r_total_time_ms": 0.5,
                "window_functions_computation": {
                    "sorts": [{ "filesort": { "sort_key": "sum(orders.total)" } }],
                    "temporary_table": {
                        "nested_loop": [{ "table": { "table_name": "orders", "access_type": "ALL", "rows": 500 } }]
                    }
                }
            },
            "some_future_wrapper": [{ "another": { "table": { "table_name": "users", "rows": 3 } } }]
        }
    });
    let r = introspect::parse_explain(
        &one_row(&[("ANALYZE", Value::Json(plan))]),
        true,
        Flavor::Mariadb,
    )
    .unwrap();
    assert_eq!(
        shape(&r.plan),
        "Having [Sort [Window [Sort, Temporary Table [Table Scan on orders]]], Table Scan on users]"
    );
    let having = &r.plan;
    assert_eq!(having.filter.as_deref(), Some("sum(orders.total) > 10"));
    assert_eq!(having.children[0].actual_total_time, Some(0.25), "per loop");
}

/// `ANALYZE FORMAT=JSON` (MariaDB has no `EXPLAIN ANALYZE`): actual rows,
/// loops and times, the planning time and the execution time.
#[test]
fn mariadb_analyze_reads_actuals() {
    let join = mariadb_plan("ANALYZE FORMAT=JSON: join");
    assert!(join.is_analyze);
    assert!(join.execution_time.is_some_and(|t| t > 0.0), "{join:?}");
    assert!(join.planning_time > 0.0, "{join:?}");
    assert!(join.plan.actual_loops.is_some(), "{:?}", join.plan);
    let o = find(&join.plan, "o").unwrap();
    assert_eq!(o.actual_loops, Some(1));
    assert_eq!(o.actual_rows, Some(1000.0));
    assert!(o.actual_total_time.is_some_and(|t| t > 0.0), "{o:?}");
    let c = find(&join.plan, "c").unwrap();
    assert_eq!(c.actual_loops, Some(333));
    // MariaDB's times add up over all loops; the UI multiplies the time per
    // loop by the loops (`calculateEffectiveTime`), so they're divided.
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-12);
    assert!(close(o.actual_total_time, 0.054625 + 0.023541667), "{o:?}");
    assert!(
        close(c.actual_total_time, (0.072875 + 0.009208333) / 333.0),
        "{c:?}"
    );
    // Every table node of every recorded MariaDB plan has estimated rows.
    for case in Server::Mariadb
        .load::<ExplainInput>("parse-explain.json")
        .into_iter()
        .filter(|c| !c.name.starts_with("synthetic:"))
    {
        let plan = introspect::parse_explain(
            &case.input.recorded.result(),
            case.name.starts_with("ANALYZE"),
            Flavor::Mariadb,
        )
        .unwrap();
        for node in nodes(&plan.plan) {
            if node.relation_name.is_some() && node.node_type != "Union" {
                assert!(node.plan_rows.is_some(), "{}: {node:?}", case.name);
            }
        }
    }
}
