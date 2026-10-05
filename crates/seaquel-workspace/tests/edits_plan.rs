//! `seaquel_workspace::edits` against the edit fixtures
//! (`tests/fixtures/edits`, recorded from today's TypeScript; see their
//! README, "The Rust replay") and the rules of phase 5c's Decisions 3–5, 9,
//! 12, 17 and 19.
//!
//! This file checks what planning decides on its own: each edit's SQL,
//! binds, query type, DML flag and summary with the case's metadata, the
//! refusals before anything runs, the cast map, the summaries and the data
//! tab's SELECT. Core's `tests/edits.rs` replays the same cases through
//! `Workspace::plan_edits`, `apply_changes` and `table_page`, where the
//! metadata reads, execution, paging, counts and history happen.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use seaquel_engine::{Dialect, Engine, SchemaColumn};
use seaquel_sql::statements::{change_summary, QueryType};
use seaquel_sql::SqlEngine;
use seaquel_types::Value;
use seaquel_workspace::edits::{
    cast_map, check_change_limits, check_edit_limits, classify, plan_edit, plan_sql, table_select,
    ApplyMode, Change, Edit, EditLimits, Filter, FilterLogic, FilterOp, TableMeta, TableQuery,
    TableTarget, INVALID_ARGUMENT, NOT_EDITABLE,
};
use serde_json::{json, Value as Json};

// ── Fixtures ──

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/edits")
}

fn read(file: &str) -> Json {
    let path = fixtures().join(format!("{file}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn changes() -> serde_json::Map<String, Json> {
    match read("changes") {
        Json::Object(map) => map,
        other => panic!("changes.json isn't an object: {other}"),
    }
}

/// `case` with its `changes.json` entry's `expected` fields in place.
fn expected(case: &Json, changes: &serde_json::Map<String, Json>) -> Json {
    let mut case = case.clone();
    if let Some(entry) = changes.get(case["name"].as_str().unwrap()) {
        for (k, v) in entry["expected"].as_object().unwrap() {
            case[k] = v.clone();
        }
    }
    case
}

fn engines() -> Vec<Arc<dyn Engine>> {
    vec![
        seaquel_engine_postgres::engine(),
        seaquel_engine_mysql::engine(),
        seaquel_engine_sqlite::engine(),
        seaquel_engine_mssql::engine(),
        // For its dialect only, which needs no helper.
        seaquel_engine_duckdb::remote_engine(seaquel_engine_duckdb::HelperLocator {
            dir: std::path::PathBuf::from("/nonexistent/bin/duckdb"),
            version: "0.0.0".into(),
        }),
    ]
}

/// The real engine for a connection type (MariaDB is the MySQL engine).
fn real_engine(ty: &str) -> Arc<dyn Engine> {
    let id = if ty == "mariadb" { "mysql" } else { ty };
    engines().into_iter().find(|e| e.id() == id).unwrap()
}

fn sql_engine(ty: &str) -> SqlEngine {
    ty.parse().unwrap()
}

fn dialect(engine: &Arc<dyn Engine>) -> &dyn Dialect {
    engine.dialect().unwrap()
}

fn columns(json: &Json) -> Vec<SchemaColumn> {
    serde_json::from_value(json.clone()).unwrap()
}

/// The case's metadata for `target`, as Core would read it.
fn metadata(case: &Json, target: &TableTarget) -> Option<TableMeta> {
    case["metadata"].as_array()?.iter().find_map(|m| {
        (m["schema"] == target.schema.as_str() && m["table"] == target.table.as_str()).then(|| {
            TableMeta {
                columns: columns(&m["columns"]),
            }
        })
    })
}

fn wire(values: &[Value]) -> Json {
    serde_json::to_value(values).unwrap()
}

// ── Plan files ──

const PLAN_FILES: [&str; 6] = [
    "plan-postgres",
    "plan-mysql",
    "plan-mariadb",
    "plan-sqlite",
    "plan-mssql",
    "plan-duckdb",
];

/// What a step's planning came to: the planned change's fields as JSON, or
/// the refusal's code.
type Planned = Result<Json, String>;

fn plan_step(case: &Json, step: &Json) -> Option<(Json, Planned)> {
    let edit_json = step["edit"].clone();
    if edit_json.is_null() {
        return None;
    }
    let edit: Edit = serde_json::from_value(edit_json.clone()).unwrap();
    let ty = case["engine"].as_str().unwrap();
    let engine = real_engine(ty);
    // A scripted metadata error fails Core's read too (the README).
    let scripted_error = step["driver"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["op"] == "tableMetadata" && d["answer"].get("error").is_some())
        .map(|d| d["answer"]["error"]["code"].as_str().unwrap().to_string());
    let planned = match scripted_error {
        Some(code) => Err(code),
        None => {
            let meta = edit.metadata_target().and_then(|t| metadata(case, t));
            plan_edit(&edit, meta.as_ref(), dialect(&engine), sql_engine(ty))
                .map(|p| serde_json::to_value(&p).unwrap())
                .map_err(|e| e.code)
        }
    };
    Some((edit_json, planned))
}

/// Differences between planning `case` and its recorded (or expected)
/// fields, as the Rust replay compares them.
fn replay_plan(case: &Json) -> Vec<String> {
    let name = case["name"].as_str().unwrap();
    let mut out = Vec::new();
    let mut planned_steps: Vec<(Json, Planned)> = Vec::new();
    for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
        let Some((edit, planned)) = plan_step(case, step) else {
            continue;
        };
        let outcome = &step["outcome"];
        let failed = outcome["success"] == false || outcome["saved"] == false;
        let code = outcome["code"].as_str();
        let build = step["driver"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["op"] == "build");
        match &planned {
            Err(got) => {
                if !(failed && code == Some(got.as_str())) {
                    out.push(format!(
                        "{name} step {i}: refused with {got}, expected {outcome}"
                    ));
                }
            }
            Ok(p) => {
                if failed && code == Some(NOT_EDITABLE) {
                    out.push(format!("{name} step {i}: planned, expected NOT_EDITABLE"));
                }
                if failed && build.is_none() {
                    out.push(format!(
                        "{name} step {i}: planned, expected a refusal {outcome}"
                    ));
                }
                if let Some(build) = build {
                    if p["sql"] != build["answer"]["sql"] {
                        out.push(format!(
                            "{name} step {i}: sql {} != {}",
                            p["sql"], build["answer"]["sql"]
                        ));
                    }
                    if p["params"] != build["answer"]["bindValues"] {
                        out.push(format!(
                            "{name} step {i}: params {} != {}",
                            p["params"], build["answer"]["bindValues"]
                        ));
                    }
                }
            }
        }
        planned_steps.push((edit, planned));
    }
    for q in case["queue"].as_array().unwrap() {
        let id = q["id"].as_str().unwrap();
        let Some((_, planned)) = planned_steps
            .iter()
            .rev()
            .find(|(edit, _)| *edit == q["change"]["edit"])
        else {
            out.push(format!("{name} {id}: no step planned this queue entry"));
            continue;
        };
        let Ok(p) = planned else {
            out.push(format!("{name} {id}: queued, but planning refused it"));
            continue;
        };
        for (field, want) in [
            ("sql", &q["sql"]),
            ("params", &q["params"]),
            ("queryType", &q["queryType"]),
            ("dml", &q["dml"]),
            ("summary", &q["summary"]),
        ] {
            if &p[field] != want {
                out.push(format!("{name} {id}: {field} {} != {want}", p[field]));
            }
        }
    }
    out
}

/// Runs `replay` on every case of `files` against its expected fields, and
/// against its recorded ones: the cases that differ from the recording must
/// all be in `changes.json`. Returns them.
fn replay_all(files: &[&str], replay: impl Fn(&Json) -> Vec<String>) -> (usize, BTreeSet<String>) {
    let changes = changes();
    let mut problems = Vec::new();
    let mut differ = BTreeSet::new();
    let mut n = 0;
    for file in files {
        for case in read(file).as_array().unwrap() {
            n += 1;
            let name = case["name"].as_str().unwrap().to_string();
            problems.extend(replay(&expected(case, &changes)));
            if !replay(case).is_empty() {
                if !changes.contains_key(&name) {
                    problems.push(format!("{name} differs but isn't in changes.json"));
                }
                differ.insert(name);
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    (n, differ)
}

#[test]
fn replays_every_plan_fixture() {
    let (n, differ) = replay_all(&PLAN_FILES, replay_plan);
    assert_eq!(n, 49);
    // The plan cases planning alone tells apart from the recording:
    // Decisions 3, 4, 12 and 19. Dedupe and origins (Decision 6) are the
    // TypeScript replay's.
    let want: BTreeSet<String> = [
        "duckdb/attached-catalog-queued",
        "mariadb/bigint-key-queued",
        "mariadb/query-tab-aliased-delete",
        "mssql/query-tab-delete-queued",
        "mssql/update-and-set-default-queued",
        "mysql/bytes-key",
        "mysql/insert-queued",
        "mysql/json-top-level-values",
        "mysql/json-typed-text",
        "mysql/update-queued",
        "pg/insert-metadata-fails",
        "pg/json-top-level-values",
        "pg/key-not-primary-key",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(differ, want);
}

// ── Cast map ──

#[test]
fn cast_map_matches_the_fixture() {
    let cases = read("cast-map");
    for case in cases.as_array().unwrap() {
        let got = cast_map(&columns(&case["columns"]));
        let want: HashMap<String, String> = serde_json::from_value(case["casts"].clone()).unwrap();
        assert_eq!(got, want, "{}", case["name"]);
    }
    assert_eq!(cases.as_array().unwrap().len(), 10);
}

// ── Summaries ──

fn replay_summary(case: &Json) -> Vec<String> {
    let engine = sql_engine(case["engine"].as_str().unwrap());
    let got = serde_json::to_value(change_summary(case["sql"].as_str().unwrap(), engine)).unwrap();
    if got == case["summary"] {
        vec![]
    } else {
        vec![format!("{}: {got} != {}", case["name"], case["summary"])]
    }
}

#[test]
fn summaries_replay_the_fixture() {
    let (n, differ) = replay_all(&["summary"], replay_summary);
    assert_eq!(n, 52);
    // Every summary.json entry in changes.json is a misread the scanner
    // fixes (Decision 12).
    let listed: BTreeSet<String> = changes()
        .keys()
        .filter(|k| k.starts_with("sum/"))
        .cloned()
        .collect();
    assert_eq!(differ, listed);
}

// ── The data tab's SELECT ──

const TABLE_PAGE_FILES: [&str; 6] = [
    "table-page-postgres",
    "table-page-mysql",
    "table-page-mariadb",
    "table-page-sqlite",
    "table-page-mssql",
    "table-page-duckdb",
];

/// The table's columns from the case's schema cache, as Core reads them
/// on SQL Server.
fn cached_columns(case: &Json) -> Option<Vec<SchemaColumn>> {
    let target = &case["input"]["tableQuery"]["target"];
    case["input"]["schemaCache"]
        .as_array()?
        .iter()
        .find(|t| t["schema"] == target["schema"] && t["name"] == target["table"])
        .map(|t| columns(&t["columns"]))
}

fn replay_select(case: &Json) -> Vec<String> {
    let name = case["name"].as_str().unwrap();
    let ty = case["engine"].as_str().unwrap();
    let engine = real_engine(ty);
    let query: TableQuery = serde_json::from_value(case["input"]["tableQuery"].clone()).unwrap();
    let columns = cached_columns(case);
    let got = table_select(
        &query,
        columns.as_deref(),
        dialect(&engine),
        sql_engine(ty),
        EditLimits::default(),
    );
    match (got, &case["select"]) {
        (Err(e), Json::Null) if e.code == INVALID_ARGUMENT => vec![],
        (Ok(source), want) if !want.is_null() => {
            let got = json!({"sql": source.sql, "params": wire(&source.params)});
            if &got == want {
                vec![]
            } else {
                vec![format!("{name}: {got} != {want}")]
            }
        }
        (got, want) => vec![format!("{name}: {got:?} != {want}")],
    }
}

#[test]
fn table_select_replays_every_table_page_fixture() {
    let (n, differ) = replay_all(&TABLE_PAGE_FILES, replay_select);
    assert_eq!(n, 40);
    // Only the placeholder changes (Decision 9) are visible in the SELECT.
    let want: BTreeSet<String> = [
        "tp/duckdb-attached-catalog",
        "tp/mssql-filters-at-p",
        "tp/mssql-sql-variant-columns-cast",
        "tp/mssql-sql-variant-full-page",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(differ, want);
}

fn pg() -> Arc<dyn Engine> {
    real_engine("postgres")
}

fn query(filters: Vec<Filter>) -> TableQuery {
    TableQuery {
        target: TableTarget {
            schema: "public".into(),
            table: "t".into(),
        },
        filters,
        logic: FilterLogic::And,
        sort: vec![],
    }
}

fn filter(column: &str, op: FilterOp, value: &str) -> Filter {
    Filter {
        column: column.into(),
        op,
        value: value.into(),
    }
}

#[test]
fn in_binds_each_item() {
    let e = pg();
    let q = query(vec![
        filter("a", FilterOp::In, " x , y,,z ,"),
        filter("b", FilterOp::NotIn, "1"),
    ]);
    let got = table_select(
        &q,
        None,
        dialect(&e),
        SqlEngine::Postgres,
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        got.sql,
        "SELECT * FROM \"public\".\"t\" WHERE CAST(\"a\" AS TEXT) IN ($1, $2, $3) \
         AND CAST(\"b\" AS TEXT) NOT IN ($4)"
    );
    assert_eq!(wire(&got.params), json!(["x", "y", "z", "1"]));
    for empty in ["", " , ,", ","] {
        for op in [FilterOp::In, FilterOp::NotIn] {
            let err = table_select(
                &query(vec![filter("a", op, empty)]),
                None,
                dialect(&e),
                SqlEngine::Postgres,
                EditLimits::default(),
            )
            .unwrap_err();
            assert_eq!(err.code, INVALID_ARGUMENT);
        }
    }
}

#[test]
fn mssql_casts_the_types_tiberius_cant_read() {
    let e = real_engine("mssql");
    let col = |name: &str, ty: &str| -> SchemaColumn {
        serde_json::from_value(json!({
            "name": name, "type": ty, "nullable": true,
            "isPrimaryKey": false, "isForeignKey": false,
        }))
        .unwrap()
    };
    let q = query(vec![]);
    for ty in ["sql_variant", "GEOGRAPHY", " geometry ", "HierarchyId"] {
        let cols = [col("id", "int"), col("v", ty)];
        let got = table_select(
            &q,
            Some(&cols),
            dialect(&e),
            SqlEngine::Mssql,
            EditLimits::default(),
        )
        .unwrap();
        assert_eq!(
            got.sql, "SELECT [id], CAST([v] AS NVARCHAR(MAX)) AS [v] FROM [public].[t]",
            "{ty}"
        );
    }
    // Readable types select `*`, and so does any other engine.
    let cols = [col("id", "int"), col("n", "nvarchar(max)")];
    let got = table_select(
        &q,
        Some(&cols),
        dialect(&e),
        SqlEngine::Mssql,
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(got.sql, "SELECT * FROM [public].[t]");
    let cols = [col("v", "sql_variant")];
    let got = table_select(
        &q,
        Some(&cols),
        dialect(&pg()),
        SqlEngine::Postgres,
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(got.sql, "SELECT * FROM \"public\".\"t\"");
}

#[test]
fn filter_ops_and_directions_are_a_closed_set() {
    for bad in [
        json!({"column": "a", "op": "; DROP", "value": ""}),
        json!({"column": "a", "op": "=="}),
        json!({"column": "a", "op": "like"}),
    ] {
        assert!(
            serde_json::from_value::<Filter>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    let bad_sort = json!({"target": {"schema": "s", "table": "t"},
                          "sort": [{"column": "a", "direction": "ASC; DROP"}]});
    assert!(serde_json::from_value::<TableQuery>(bad_sort).is_err());
    let bad_logic = json!({"target": {"schema": "s", "table": "t"}, "logic": "XOR"});
    assert!(serde_json::from_value::<TableQuery>(bad_logic).is_err());
}

// ── Keys, defaults, classification ──

fn users_meta() -> TableMeta {
    let col = |name: &str, ty: &str, pk: bool, default: Option<&str>| -> SchemaColumn {
        let mut c = json!({"name": name, "type": ty, "nullable": !pk,
                           "isPrimaryKey": pk, "isForeignKey": false});
        if let Some(d) = default {
            c["defaultValue"] = json!(d);
        }
        serde_json::from_value(c).unwrap()
    };
    TableMeta {
        columns: vec![
            col("region", "TEXT", true, None),
            col("sku", "TEXT", true, None),
            col("body", "TEXT", false, Some("'empty'")),
            col("tag", "TEXT", false, None),
        ],
    }
}

fn target() -> TableTarget {
    TableTarget {
        schema: "main".into(),
        table: "stock".into(),
    }
}

fn key(pairs: &[(&str, i64)]) -> Vec<(String, Value)> {
    pairs
        .iter()
        .map(|(c, v)| (c.to_string(), Value::Int(*v)))
        .collect()
}

#[test]
fn a_key_that_is_not_the_primary_key_is_not_editable() {
    let e = real_engine("sqlite");
    let meta = users_meta();
    for bad in [
        key(&[("region", 1)]),
        key(&[("region", 1), ("sku", 2), ("tag", 3)]),
        key(&[("region", 1), ("region", 2)]),
        key(&[("body", 1), ("sku", 2)]),
        key(&[]),
    ] {
        for edit in [
            Edit::DeleteRow {
                target: target(),
                key: bad.clone(),
            },
            Edit::SetDefault {
                target: target(),
                key: bad.clone(),
                column: "body".into(),
            },
            Edit::UpdateCell {
                target: target(),
                key: bad.clone(),
                column: "body".into(),
                value: Value::Text("canary".into()),
            },
        ] {
            let err = plan_edit(&edit, Some(&meta), dialect(&e), SqlEngine::Sqlite).unwrap_err();
            assert_eq!(err.code, NOT_EDITABLE, "{bad:?}");
            assert!(!err.message.contains("canary"), "{}", err.message);
        }
    }
    // The same set in another order is the key; the WHERE keeps its order.
    let ok = plan_edit(
        &Edit::DeleteRow {
            target: target(),
            key: key(&[("sku", 2), ("region", 1)]),
        },
        Some(&meta),
        dialect(&e),
        SqlEngine::Sqlite,
    )
    .unwrap();
    assert_eq!(
        ok.sql,
        "DELETE FROM \"main\".\"stock\" WHERE \"sku\" = $1 AND \"region\" = $2"
    );
}

#[test]
fn a_column_not_in_the_metadata_is_not_editable() {
    let e = real_engine("sqlite");
    let meta = users_meta();
    let k = key(&[("region", 1), ("sku", 2)]);
    for edit in [
        // Case matters: `Body` isn't `body`, which SQLite's Set default
        // would otherwise have set to NULL.
        Edit::SetDefault {
            target: target(),
            key: k.clone(),
            column: "Body".into(),
        },
        Edit::UpdateCell {
            target: target(),
            key: k.clone(),
            column: "nope".into(),
            value: Value::Text("canary".into()),
        },
        Edit::InsertRow {
            target: target(),
            values: vec![
                ("body".into(), Value::Text("canary".into())),
                ("TAG".into(), Value::Null),
            ],
        },
    ] {
        let err = plan_edit(&edit, Some(&meta), dialect(&e), SqlEngine::Sqlite).unwrap_err();
        assert_eq!(err.code, NOT_EDITABLE, "{edit:?}");
        assert!(!err.message.contains("canary"), "{}", err.message);
    }
}

#[test]
fn a_table_without_a_primary_key_is_not_editable() {
    let e = real_engine("sqlite");
    let mut meta = users_meta();
    for c in &mut meta.columns {
        c.is_primary_key = false;
    }
    let edit = Edit::DeleteRow {
        target: target(),
        key: key(&[("region", 1)]),
    };
    let err = plan_edit(&edit, Some(&meta), dialect(&e), SqlEngine::Sqlite).unwrap_err();
    assert_eq!(err.code, NOT_EDITABLE);
    // A table that isn't there (no columns, or no metadata) neither, for
    // any grid edit; an insert into a table without a key is fine.
    for meta in [None, Some(TableMeta::default())] {
        let err = plan_edit(&edit, meta.as_ref(), dialect(&e), SqlEngine::Sqlite).unwrap_err();
        assert_eq!(err.code, NOT_EDITABLE);
    }
    let insert = Edit::InsertRow {
        target: target(),
        values: key(&[("tag", 1)]),
    };
    assert!(plan_edit(&insert, Some(&meta), dialect(&e), SqlEngine::Sqlite).is_ok());
}

#[test]
fn sqlite_set_default_uses_the_metadata_default() {
    let e = real_engine("sqlite");
    let edit = |column: &str| Edit::SetDefault {
        target: target(),
        key: key(&[("region", 1), ("sku", 2)]),
        column: column.into(),
    };
    let got = plan_edit(
        &edit("body"),
        Some(&users_meta()),
        dialect(&e),
        SqlEngine::Sqlite,
    )
    .unwrap();
    assert_eq!(
        got.sql,
        "UPDATE \"main\".\"stock\" SET \"body\" = ('empty') WHERE \"region\" = $1 AND \"sku\" = $2"
    );
    assert!(got.dml);
    // Other engines keep DEFAULT.
    let mysql = real_engine("mysql");
    let got = plan_edit(
        &edit("body"),
        Some(&users_meta()),
        dialect(&mysql),
        SqlEngine::Mysql,
    )
    .unwrap();
    assert_eq!(
        got.sql,
        "UPDATE `main`.`stock` SET `body` = DEFAULT WHERE `region` = ? AND `sku` = ?"
    );
}

#[test]
fn a_column_without_a_default_sets_null() {
    let e = real_engine("sqlite");
    let edit = Edit::SetDefault {
        target: target(),
        key: key(&[("region", 1), ("sku", 2)]),
        column: "tag".into(),
    };
    let got = plan_edit(&edit, Some(&users_meta()), dialect(&e), SqlEngine::Sqlite).unwrap();
    assert!(got.sql.contains("SET \"tag\" = (NULL)"), "{}", got.sql);
}

#[test]
fn json_text_stays_text() {
    // The `*/json-typed-text` cases in the replay, and a JSON column's
    // array, number and bool (Decision 19).
    let e = real_engine("mysql");
    let col = |name: &str, ty: &str, pk: bool| -> SchemaColumn {
        serde_json::from_value(json!({"name": name, "type": ty, "nullable": true,
                                      "isPrimaryKey": pk, "isForeignKey": false}))
        .unwrap()
    };
    let meta = TableMeta {
        columns: vec![
            col("id", "int", true),
            col("doc", "JSON", false),
            col("n", "int", false),
        ],
    };
    let update = |column: &str, value: Value| Edit::UpdateCell {
        target: target(),
        key: key(&[("id", 1)]),
        column: column.into(),
        value,
    };
    let bind = |edit: Edit| {
        let p = plan_edit(&edit, Some(&meta), dialect(&e), SqlEngine::Mysql).unwrap();
        wire(&p.params)[0].clone()
    };
    assert_eq!(bind(update("doc", Value::Text("[1]".into()))), json!("[1]"));
    assert_eq!(
        bind(update("doc", Value::Array(vec![Value::Int(1)]))),
        json!({"$sq": "json", "v": [1]})
    );
    assert_eq!(
        bind(update("doc", Value::Bool(false))),
        json!({"$sq": "json", "v": false})
    );
    assert_eq!(bind(update("doc", Value::Null)), json!(null));
    // Not a JSON column: untouched.
    assert_eq!(bind(update("n", Value::Int(5))), json!(5));
}

#[test]
fn classify_is_atomic_only_for_dml() {
    let e = pg();
    let pg = SqlEngine::Postgres;
    let dml = |sql: &str| plan_sql(sql, &[], pg).unwrap().dml;
    for (sql, want) in [
        ("INSERT INTO t VALUES (1)", true),
        ("update t set a = 1", true),
        ("/* c */ DELETE FROM t WHERE a = 1", true),
        (
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = 1",
            false,
        ),
        ("WITH x AS (SELECT 1) UPDATE t SET a = 1", false),
        ("TRUNCATE t", false),
        ("CREATE TABLE z (i int)", false),
        ("ALTER TABLE t ADD c int", false),
        ("SELECT 1", false),
        ("CALL p()", false),
    ] {
        assert_eq!(dml(sql), want, "{sql}");
    }
    let t = TableTarget {
        schema: "public".into(),
        table: "t".into(),
    };
    let plan = |edit: &Edit, engine: SqlEngine| {
        let real = real_engine(engine.as_str());
        let meta = TableMeta {
            columns: users_meta().columns,
        };
        plan_edit(edit, Some(&meta), dialect(&real), engine).unwrap()
    };
    let truncate = Edit::TruncateTable { target: t.clone() };
    let drop = Edit::DropObject {
        target: t.clone(),
        kind: seaquel_workspace::edits::ObjectKind::Table,
    };
    assert!(!plan(&truncate, pg).dml);
    assert_eq!(plan(&truncate, pg).query_type, QueryType::Other);
    let sqlite_truncate = plan(&truncate, SqlEngine::Sqlite);
    assert!(sqlite_truncate.dml);
    assert_eq!(sqlite_truncate.sql, "DELETE FROM \"public\".\"t\"");
    assert!(!plan(&drop, pg).dml);
    let _ = e;
    assert_eq!(classify(&[true]), ApplyMode::Single);
    assert_eq!(classify(&[false]), ApplyMode::Single);
    assert_eq!(classify(&[true, true, true]), ApplyMode::Atomic);
    assert_eq!(classify(&[true, false]), ApplyMode::InOrder);
    assert_eq!(classify(&[false, true]), ApplyMode::InOrder);
}

#[test]
fn a_typed_change_with_two_statements_is_refused() {
    for (sql, engine) in [
        ("UPDATE t SET a = 1; COMMIT", SqlEngine::Postgres),
        ("DELETE FROM t WHERE a = 1; DROP TABLE t", SqlEngine::Sqlite),
        ("UPDATE t SET a = 1 /*! ; DROP TABLE t */", SqlEngine::Mysql),
        (
            "UPDATE t SET a = 1 /*M! ; DROP TABLE t */",
            SqlEngine::Mariadb,
        ),
        ("", SqlEngine::Postgres),
        ("-- only a comment", SqlEngine::Postgres),
    ] {
        let err = plan_sql(sql, &[], engine).unwrap_err();
        assert_eq!(err.code, INVALID_ARGUMENT, "{sql}");
    }
    // A trailing `;` and a `;` in a string are one statement.
    assert!(plan_sql("UPDATE t SET a = ';';", &[], SqlEngine::Postgres).is_ok());
}

// ── Limits ──

/// The web server's edit limits (`seaquel_server::WEB_EDIT_LIMITS`).
const WEB: EditLimits = EditLimits {
    max_changes: Some(10_000),
    max_tables: Some(100),
    max_sql_bytes: Some(2 * 1024 * 1024),
    max_value_bytes: Some(16 * 1024 * 1024),
    max_filters: Some(100),
    max_in_values: Some(1_000),
    max_filter_value_bytes: Some(64 * 1024),
};

fn sql_change(i: usize, sql: &str, params: Vec<Value>) -> Change {
    Change::Sql {
        id: format!("c{i}"),
        sql: sql.to_string(),
        params,
    }
}

#[test]
fn limits_are_the_interfaces() {
    let none = EditLimits::default();
    // No limits by default: a big apply passes.
    let big: Vec<Change> = (0..20_000)
        .map(|i| sql_change(i, "DELETE FROM t WHERE id = 1", vec![]))
        .collect();
    assert!(check_change_limits(&big, none).is_ok());
    let err = check_change_limits(&big, WEB).unwrap_err();
    assert_eq!(err.code, INVALID_ARGUMENT);
    assert!(check_change_limits(&big[..10_000], WEB).is_ok());

    // Distinct tables (each a metadata read); the sidebar's TRUNCATE and
    // DROP read none and don't count.
    let on = |table: String| Change::Edit {
        id: table.clone(),
        edit: Edit::DeleteRow {
            target: TableTarget {
                schema: "s".into(),
                table,
            },
            key: key(&[("id", 1)]),
        },
    };
    let tables: Vec<Change> = (0..101).map(|i| on(format!("t{i}"))).collect();
    assert!(check_change_limits(&tables, none).is_ok());
    assert_eq!(
        check_change_limits(&tables, WEB).unwrap_err().code,
        INVALID_ARGUMENT
    );
    let repeated: Vec<Change> = (0..300).map(|i| on(format!("t{}", i % 100))).collect();
    assert!(check_change_limits(&repeated, WEB).is_ok());
    let edits: Vec<Edit> = tables
        .iter()
        .map(|c| match c {
            Change::Edit { edit, .. } => edit.clone(),
            Change::Sql { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(
        check_edit_limits(&edits, WEB).unwrap_err().code,
        INVALID_ARGUMENT
    );
    let truncates: Vec<Change> = (0..101)
        .map(|i| Change::Edit {
            id: i.to_string(),
            edit: Edit::TruncateTable {
                target: TableTarget {
                    schema: "s".into(),
                    table: format!("t{i}"),
                },
            },
        })
        .collect();
    assert!(check_change_limits(&truncates, WEB).is_ok());

    let long_sql = "x".repeat(2 * 1024 * 1024 + 1);
    let one = [sql_change(0, &long_sql, vec![])];
    assert!(check_change_limits(&one, none).is_ok());
    assert_eq!(
        check_change_limits(&one, WEB).unwrap_err().code,
        INVALID_ARGUMENT
    );

    let big_value = Value::Text("v".repeat(16 * 1024 * 1024 + 1));
    let one = [sql_change(
        0,
        "UPDATE t SET a = $1",
        vec![big_value.clone()],
    )];
    assert!(check_change_limits(&one, none).is_ok());
    assert_eq!(
        check_change_limits(&one, WEB).unwrap_err().code,
        INVALID_ARGUMENT
    );
    let edit = Edit::UpdateCell {
        target: target(),
        key: key(&[("id", 1)]),
        column: "a".into(),
        value: big_value,
    };
    assert!(check_edit_limits(std::slice::from_ref(&edit), none).is_ok());
    assert_eq!(
        check_edit_limits(std::slice::from_ref(&edit), WEB)
            .unwrap_err()
            .code,
        INVALID_ARGUMENT
    );
    let edits = vec![Change::Edit {
        id: "c".into(),
        edit,
    }];
    assert_eq!(
        check_change_limits(&edits, WEB).unwrap_err().code,
        INVALID_ARGUMENT
    );

    // The table page: filters, sort columns, one value's size, IN items.
    let e = pg();
    let select = |q: &TableQuery, limits| {
        table_select(q, None, dialect(&e), SqlEngine::Postgres, limits).map(|_| ())
    };
    let many = query((0..101).map(|_| filter("a", FilterOp::Eq, "1")).collect());
    assert!(select(&many, none).is_ok());
    assert_eq!(select(&many, WEB).unwrap_err().code, INVALID_ARGUMENT);
    assert!(select(
        &query((0..100).map(|_| filter("a", FilterOp::Eq, "1")).collect()),
        WEB
    )
    .is_ok());
    let mut sorted = query(vec![]);
    sorted.sort = (0..101)
        .map(|_| seaquel_workspace::edits::Sort {
            column: "a".into(),
            direction: seaquel_workspace::edits::SortDirection::Asc,
        })
        .collect();
    assert_eq!(select(&sorted, WEB).unwrap_err().code, INVALID_ARGUMENT);
    let long = query(vec![filter(
        "a",
        FilterOp::Like,
        &"x".repeat(64 * 1024 + 1),
    )]);
    assert!(select(&long, none).is_ok());
    assert_eq!(select(&long, WEB).unwrap_err().code, INVALID_ARGUMENT);
    let items: Vec<String> = (0..1_001).map(|i| i.to_string()).collect();
    let in_list = query(vec![filter("a", FilterOp::In, &items.join(","))]);
    assert!(select(&in_list, none).is_ok());
    assert_eq!(select(&in_list, WEB).unwrap_err().code, INVALID_ARGUMENT);
    let split = query(vec![
        filter("a", FilterOp::In, &items[..600].join(",")),
        filter("b", FilterOp::NotIn, &items[600..].join(",")),
    ]);
    assert_eq!(select(&split, WEB).unwrap_err().code, INVALID_ARGUMENT);
    let fits = query(vec![filter("a", FilterOp::In, &items[..1_000].join(","))]);
    assert!(select(&fits, WEB).is_ok());
}

// ── Totality ──

/// The scanner's corpus: every `split.json` input.
fn corpus() -> Vec<String> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../seaquel-sql/tests/fixtures/split.json");
    let json: Json = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut out: Vec<String> = json["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["input"]["sql"].as_str().unwrap().to_string())
        .collect();
    out.extend(["", "\0", "\"", "`", "]", "[", "é\u{301}", "😀"].map(String::from));
    out
}

#[test]
fn planning_never_panics() {
    let corpus = corpus();
    for engine in SqlEngine::ALL {
        let real = real_engine(engine.as_str());
        let d = dialect(&real);
        for text in &corpus {
            // As SQL.
            let _ = plan_sql(text, &[], engine);
            let _ = change_summary(text, engine);
            // As names: the schema, table, column, key and filter.
            let t = TableTarget {
                schema: text.clone(),
                table: text.clone(),
            };
            let col: SchemaColumn = serde_json::from_value(json!({
                "name": text, "type": text, "castType": text, "nullable": true,
                "defaultValue": text, "isPrimaryKey": true, "isForeignKey": false,
            }))
            .unwrap();
            let meta = TableMeta { columns: vec![col] };
            let k = vec![(text.clone(), Value::Text(text.clone()))];
            for edit in [
                Edit::UpdateCell {
                    target: t.clone(),
                    key: k.clone(),
                    column: text.clone(),
                    value: Value::Array(vec![Value::Text(text.clone())]),
                },
                Edit::SetDefault {
                    target: t.clone(),
                    key: k.clone(),
                    column: text.clone(),
                },
                Edit::InsertRow {
                    target: t.clone(),
                    values: k.clone(),
                },
                Edit::DeleteRow {
                    target: t.clone(),
                    key: k.clone(),
                },
                Edit::TruncateTable { target: t.clone() },
                Edit::DropObject {
                    target: t.clone(),
                    kind: seaquel_workspace::edits::ObjectKind::MaterializedView,
                },
            ] {
                let _ = plan_edit(&edit, Some(&meta), d, engine);
            }
            let mut q = query(
                [
                    FilterOp::Eq,
                    FilterOp::In,
                    FilterOp::NotIn,
                    FilterOp::IsNull,
                    FilterOp::NotLike,
                ]
                .into_iter()
                .map(|op| filter(text, op, text))
                .collect(),
            );
            q.target = t.clone();
            let _ = table_select(&q, Some(&meta.columns), d, engine, WEB);
        }
    }
}
