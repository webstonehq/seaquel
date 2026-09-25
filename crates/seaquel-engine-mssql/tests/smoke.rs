//! Live tests against SQL Server: the shared smoke and introspection suites,
//! the live cases of `fixtures/bugfixes.json` (columns, indexes and EXPLAIN
//! of its scratch objects), CRUD through the dialect, and the dialect's DDL
//! run on real tables.
//!
//! SEAQUEL_TEST_MSSQL, e.g.
//! {"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa",
//!  "password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}
//! The tests run in `seaquel_test` whatever database it names.

mod common;

use futures::FutureExt;
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::panic::AssertUnwindSafe;

use seaquel_engine::{DbError, Dialect, Driver, SchemaColumn, SchemaIndex, Value};
use seaquel_engine_mssql::{MssqlDialect, MssqlDriver};
use seaquel_engine_testkit::{
    run_introspection, run_smoke, scratch_name, IntrospectionExpect, IntrospectionSpec, SmokeSpec,
};
use seaquel_types::{
    CreateTableColumn, CreateTableDefinition, CreateTableIndex, ExplainPlanNode, TableKind,
};

#[tokio::test]
async fn smoke() {
    let Some(config) = common::config() else {
        return;
    };
    run_smoke(&*seaquel_engine_mssql::engine(), &config, &SmokeSpec::AT_P).await;
}

// ── Introspection suite ──────────────────────────────────────────────────────

const INTROSPECT_PREFIX: &str = "sq_t13_";

/// The shared checks on a scratch schema. Its setup runs `CREATE SCHEMA` and
/// `CREATE VIEW` through `execute` as they are (the batch fix), a name with
/// a space, and a column with a collation that isn't the database's (fix
/// 15). Statistics are unsupported; a plain EXPLAIN with a parameter
/// declares it (see `introspect::declare_params`).
#[tokio::test]
async fn introspection() {
    let Some(config) = common::config() else {
        return;
    };
    let schema = scratch_name(INTROSPECT_PREFIX);
    let table = format!("[{schema}].[order items]");
    let col = |name: &str, ty: &str, nullable: bool| SchemaColumn {
        name: name.into(),
        ty: ty.into(),
        cast_type: None,
        nullable,
        default_value: None,
        is_primary_key: false,
        is_foreign_key: false,
        foreign_key_ref: None,
        collation: None,
        is_unique: false,
        in_unique_constraint: false,
    };
    let spec = IntrospectionSpec {
        schema: schema.clone(),
        setup: vec![
            format!("CREATE SCHEMA [{schema}]"),
            format!(
                "CREATE TABLE {table} (id INT NOT NULL CONSTRAINT [oi pk] PRIMARY KEY, \
                 sku NVARCHAR(20) COLLATE Latin1_General_BIN NOT NULL, \
                 qty INT NULL CONSTRAINT [oi qty df] DEFAULT 1)"
            ),
            format!("CREATE INDEX [oi sku] ON {table} (sku) INCLUDE (qty)"),
            format!("CREATE VIEW [{schema}].[v] AS SELECT sku, qty FROM {table}"),
            format!(
                "INSERT INTO {table} (id, sku, qty) SELECT n, N'SKU-' + CAST(n % 40 AS NVARCHAR(10)), n % 5 \
                 FROM (SELECT TOP (500) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS n FROM sys.all_columns) s"
            ),
        ],
        teardown: vec![
            format!("DROP VIEW IF EXISTS [{schema}].[v]"),
            format!("DROP TABLE IF EXISTS {table}"),
            format!("DROP SCHEMA IF EXISTS [{schema}]"),
        ],
        // Every object in every leftover `sq_t13_…` schema, then the schema.
        stale_cleanup: vec![format!(
            "DECLARE @sql nvarchar(max) = N''; \
             SELECT @sql += N'DROP VIEW ' + QUOTENAME(s.name) + N'.' + QUOTENAME(o.name) + N'; ' \
               FROM sys.views o JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE s.name LIKE N'{p}%'; \
             SELECT @sql += N'DROP TABLE ' + QUOTENAME(s.name) + N'.' + QUOTENAME(o.name) + N'; ' \
               FROM sys.tables o JOIN sys.schemas s ON s.schema_id = o.schema_id WHERE s.name LIKE N'{p}%'; \
             SELECT @sql += N'DROP SCHEMA ' + QUOTENAME(name) + N'; ' FROM sys.schemas WHERE name LIKE N'{p}%'; \
             EXEC (@sql);",
            p = INTROSPECT_PREFIX.replace('_', "[_]")
        )],
        tables: vec![
            ("order items".into(), TableKind::Table),
            ("v".into(), TableKind::View),
        ],
        columns: vec![
            (
                "order items".into(),
                vec![
                    SchemaColumn {
                        is_primary_key: true,
                        ..col("id", "int", false)
                    },
                    SchemaColumn {
                        collation: Some("Latin1_General_BIN".into()),
                        ..col("sku", "nvarchar(20)", false)
                    },
                    SchemaColumn {
                        default_value: Some("((1))".into()),
                        ..col("qty", "int", true)
                    },
                ],
            ),
            (
                "v".into(),
                vec![
                    SchemaColumn {
                        collation: Some("Latin1_General_BIN".into()),
                        ..col("sku", "nvarchar(20)", false)
                    },
                    col("qty", "int", true),
                ],
            ),
        ],
        indexes: vec![(
            "order items".into(),
            vec![
                SchemaIndex {
                    name: "oi pk".into(),
                    columns: vec!["id".into()],
                    unique: true,
                    ty: "clustered".into(),
                },
                SchemaIndex {
                    name: "oi sku".into(),
                    columns: vec!["sku".into()],
                    unique: false,
                    ty: "nonclustered".into(),
                },
            ],
        )],
        stats_table: String::new(),
        usage_index: String::new(),
        explain_sql: format!("SELECT sku FROM {table} WHERE qty = @P1"),
        explain_params: vec![Value::Int(3)],
        explain_relation: "order items".into(),
        expect: IntrospectionExpect {
            supports_statistics: false,
            explain_has_execution_time: false,
            ..IntrospectionExpect::ALL
        },
    };
    run_introspection(&*seaquel_engine_mssql::engine(), &config, &spec).await;
}

// ── bugfixes.json ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Bugfixes {
    scratch: Scratch,
    cases: Vec<BugfixCase>,
}

#[derive(Deserialize)]
struct Scratch {
    setup: Vec<String>,
    teardown: Vec<String>,
}

#[derive(Deserialize)]
struct BugfixCase {
    name: String,
    kind: String,
    input: Json,
    output: Json,
}

fn bugfixes() -> Bugfixes {
    serde_json::from_str(include_str!("fixtures/bugfixes.json")).expect("bugfixes.json")
}

async fn run_all(driver: &MssqlDriver, what: &str, statements: &[String], strict: bool) {
    for sql in statements {
        if let Err(e) = driver.execute(sql, vec![]).await {
            if strict {
                panic!("{what} failed: {e:?}\n{sql}");
            }
            eprintln!("{what} failed: {}\n{sql}", e.message);
        }
    }
}

/// A number as the fixtures write it: integral values without `.0`.
fn number(f: f64) -> Json {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        json!(f as i64)
    } else {
        json!(f)
    }
}

/// A plan reduced as the `explain` cases are: node type, relation, index
/// and filter, and with ANALYZE the actual rows and loops.
fn reduce(n: &ExplainPlanNode, analyze: bool) -> Json {
    let mut out = serde_json::Map::new();
    out.insert("nodeType".into(), json!(n.node_type));
    for (key, v) in [
        ("relationName", &n.relation_name),
        ("indexName", &n.index_name),
        ("filter", &n.filter),
    ] {
        if let Some(v) = v {
            out.insert(key.into(), json!(v));
        }
    }
    if analyze {
        if let Some(rows) = n.actual_rows {
            out.insert("actualRows".into(), number(rows));
        }
        if let Some(loops) = n.actual_loops {
            out.insert("actualLoops".into(), json!(loops));
        }
    }
    out.insert(
        "children".into(),
        Json::Array(n.children.iter().map(|c| reduce(c, analyze)).collect()),
    );
    Json::Object(out)
}

/// A transaction statement typed by hand. Through `sp_executesql`, BEGIN
/// and ROLLBACK each report error 266 (the transaction count changed inside
/// the call), but take effect.
async fn manual_tx(driver: &MssqlDriver, sql: &str) {
    if let Err(e) = driver.query(sql, vec![]).await {
        assert!(e.message.contains("code: 266"), "{sql}: {e:?}");
    }
}

/// The `columns`, `indexes` and `explain` cases, live on the scratch objects
/// (fixes 1, 3, 4, 5, 6, 9, 10, 11 and 15). The scratch objects have fixed
/// names, so only this test builds them. The UPDATE runs in a transaction
/// that is rolled back.
#[tokio::test]
async fn bugfix_cases_live() {
    let Some(driver) = common::open().await else {
        return;
    };
    let fixtures = bugfixes();
    run_all(&driver, "stale teardown", &fixtures.scratch.teardown, false).await;
    let outcome = AssertUnwindSafe(async {
        run_all(&driver, "scratch setup", &fixtures.scratch.setup, true).await;
        const TOTAL: &str = "SELECT SUM(total) AS t FROM dbo.fx_orders WHERE customer_id = 3";
        let before = driver.query(TOTAL, vec![]).await.expect("total");
        let mut failures = Vec::new();
        let mut counts = (0, 0, 0);
        for case in &fixtures.cases {
            let actual = match case.kind.as_str() {
                "columns" | "indexes" => {
                    let schema = case.input["schema"].as_str().unwrap();
                    let table = case.input["table"].as_str().unwrap();
                    let (mut columns, indexes) = driver
                        .table_metadata(schema, table)
                        .await
                        .unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
                    // The fixtures predate the UNIQUE flags that Task 18
                    // derives from the indexes: take them back off the
                    // columns that have them, after checking they are there.
                    // (The filtered `fx_customers_email_uq` doesn't count.)
                    const UNIQUE: [(&str, &str); 3] = [
                        ("fx]odd name", "the value"),
                        ("fx_invoices", "number"),
                        ("fx_customers", "external_id"),
                    ];
                    for c in &mut columns {
                        let expected = UNIQUE.iter().any(|(t, n)| *t == table && *n == c.name);
                        assert_eq!(
                            (c.is_unique, c.in_unique_constraint),
                            (expected, expected),
                            "{table}.{}",
                            c.name
                        );
                        c.is_unique = false;
                        c.in_unique_constraint = false;
                    }
                    if case.kind == "columns" {
                        counts.0 += 1;
                        serde_json::to_value(columns).unwrap()
                    } else {
                        counts.1 += 1;
                        serde_json::to_value(indexes).unwrap()
                    }
                }
                "explain" => {
                    counts.2 += 1;
                    let sql = case.input["sql"].as_str().unwrap();
                    let analyze = case.input["analyze"].as_bool().unwrap();
                    let params: Vec<Value> = case.input["params"]
                        .as_array()
                        .map(|p| p.iter().cloned().map(Value::from_json_cell).collect())
                        .unwrap_or_default();
                    let dml = sql.starts_with("UPDATE");
                    if dml {
                        manual_tx(&driver, "BEGIN TRANSACTION").await;
                        assert_eq!(common::trancount(&driver).await, 1);
                    }
                    let plan = driver.explain(sql, params, analyze).await;
                    if dml {
                        manual_tx(&driver, "ROLLBACK TRANSACTION").await;
                        assert_eq!(common::trancount(&driver).await, 0);
                    }
                    let plan = plan.unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
                    json!({ "isAnalyze": plan.is_analyze, "plan": reduce(&plan.plan, analyze) })
                }
                _ => continue,
            };
            if actual != case.output {
                failures.push(format!(
                    "{}\n  expected: {}\n  actual:   {}",
                    case.name, case.output, actual
                ));
            }
        }
        assert_eq!(counts, (7, 3, 8), "columns, indexes and explain cases");
        assert!(
            failures.is_empty(),
            "{} live cases differ:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
        let after = driver.query(TOTAL, vec![]).await.expect("total");
        assert_eq!(
            after.rows, before.rows,
            "the EXPLAIN ANALYZE of the UPDATE was rolled back"
        );
    })
    .catch_unwind()
    .await;
    run_all(
        &driver,
        "scratch teardown",
        &fixtures.scratch.teardown,
        false,
    )
    .await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// EXPLAIN leaves the connection as it found it: after a plan, a failing
/// query and a dropped call, ordinary queries return rows, not plans.
#[tokio::test]
async fn explain_turns_its_session_setting_off() {
    let Some(driver) = common::open().await else {
        return;
    };
    async fn plain(driver: &MssqlDriver) {
        let r = driver.query("SELECT 7 AS n", vec![]).await.expect("query");
        assert_eq!(r.columns, vec!["n"]);
        assert_eq!(r.rows, vec![vec![Value::Int(7)]]);
    }
    for analyze in [false, true] {
        let plan = driver
            .explain(
                "SELECT name FROM sys.objects WHERE object_id = 1",
                vec![],
                analyze,
            )
            .await
            .expect("explain");
        assert_eq!(plan.is_analyze, analyze);
        assert_ne!(plan.plan.node_type, "Query Plan", "{plan:#?}");
        plain(&driver).await;

        let err = driver
            .explain("SELECT * FROM dbo.no_such_table_t13", vec![], analyze)
            .await
            .expect_err("a missing table");
        assert_eq!(err.code, "QUERY_ERROR");
        plain(&driver).await;

        // Dropped after SET … ON was sent: the connection is closed, and
        // the next call reconnects without the setting.
        let fut = driver.explain("WAITFOR DELAY '00:00:02'; SELECT 1 AS a", vec![], analyze);
        let _ = tokio::time::timeout(std::time::Duration::from_millis(300), fut).await;
        plain(&driver).await;
    }
    // Plain EXPLAIN with a parameter declares it: a plan, and nothing ran.
    let plan = driver
        .explain(
            "SELECT name FROM sys.objects WHERE object_id = @P1 AND name <> @P2",
            vec![Value::Int(1), Value::from("x")],
            false,
        )
        .await
        .expect("explain with parameters");
    assert_ne!(plan.plan.node_type, "Query Plan", "{plan:#?}");
    let err = driver
        .explain("SELECT @P1 AS a", vec![Value::Array(vec![])], false)
        .await
        .expect_err("array parameter");
    assert_eq!(err.code, "QUERY_ERROR");
    plain(&driver).await;
}

// ── CRUD through the dialect ─────────────────────────────────────────────────

/// Runs `body` with a table created by `ddl` (`{t}` is its quoted name),
/// dropping it afterwards even if `body` panics.
async fn with_scratch_table<'a, F, Fut>(driver: &'a MssqlDriver, ddl: &str, body: F)
where
    F: FnOnce(&'a MssqlDriver, String) -> Fut,
    Fut: std::future::Future<Output = ()> + 'a,
{
    let name = scratch_name("t13_crud_");
    let t = format!("[dbo].[{name}]");
    driver
        .execute(&ddl.replace("{t}", &t), vec![])
        .await
        .expect("create table");
    let outcome = AssertUnwindSafe(body(driver, name)).catch_unwind().await;
    if let Err(e) = driver.execute(&format!("DROP TABLE {t}"), vec![]).await {
        eprintln!("dropping {t}: {}", e.message);
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn exec(
    driver: &MssqlDriver,
    built: seaquel_engine::SqlWithBindings,
) -> Result<u64, DbError> {
    driver
        .execute(&built.sql, built.bind_values.unwrap_or_default())
        .await
        .map(|r| r.rows_affected)
}

async fn select_all(driver: &MssqlDriver, table: &str, cols: &str) -> Vec<Vec<Value>> {
    driver
        .query(
            &format!("SELECT {cols} FROM [dbo].[{table}] ORDER BY 1"),
            vec![],
        )
        .await
        .expect("select")
        .rows
}

/// Insert, update, set-default and delete with the dialect's `@P` binds:
/// non-Latin text, uniqueidentifier and datetime2 keys, and keys read back
/// from the table the way the UI reads them (uuids and datetimes as text,
/// binary as bytes). `tests/values.rs` covers more key types.
#[tokio::test]
async fn crud_round_trips() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, "t13_crud_").await;
    let d: &'static MssqlDialect = &MssqlDialect;
    let pk = |k: &str| vec![k.to_string()];

    // nvarchar non-Latin text, a composite key with a ] in a name.
    with_scratch_table(
        &driver,
        "CREATE TABLE {t} (id INT NOT NULL, [re]]gion] NVARCHAR(10) NOT NULL, \
         name NVARCHAR(50) NULL DEFAULT N'デフォルト', flag BIT NULL, PRIMARY KEY (id, [re]]gion]))",
        |driver, t| async move {
            let row = |name: &str| {
                vec![
                    ("id".to_string(), Value::Int(1)),
                    ("re]gion".to_string(), Value::from("東京")),
                    ("name".to_string(), Value::from(name)),
                    ("flag".to_string(), Value::Bool(true)),
                ]
            };
            let pks = vec!["id".to_string(), "re]gion".to_string()];
            let n = exec(driver, d.build_insert("dbo", &t, &row("Ünïcödé ✓ 名前 🚀"), None)).await;
            assert_eq!(n.expect("insert"), 1);
            let got = select_all(driver, &t, "[re]]gion], name, flag").await;
            assert_eq!(
                got,
                vec![vec![Value::from("東京"), Value::from("Ünïcödé ✓ 名前 🚀"), Value::Bool(true)]]
            );
            let n = exec(driver, d.build_update("dbo", &t, "name", Value::from("Ελληνικά"), &pks, &row(""), None)).await;
            assert_eq!(n.expect("update"), 1);
            let n = exec(driver, d.build_update("dbo", &t, "flag", Value::Null, &pks, &row(""), None)).await;
            assert_eq!(n.expect("update to NULL"), 1);
            assert_eq!(
                select_all(driver, &t, "name, flag").await,
                vec![vec![Value::from("Ελληνικά"), Value::Null]]
            );
            let n = exec(driver, d.build_set_default("dbo", &t, "name", &pks, &row(""), None)).await;
            assert_eq!(n.expect("set default"), 1);
            assert_eq!(select_all(driver, &t, "name").await, vec![vec![Value::from("デフォルト")]]);
            let n = exec(driver, d.build_delete("dbo", &t, &pks, &row(""), None)).await;
            assert_eq!(n.expect("delete"), 1);
            assert!(select_all(driver, &t, "id").await.is_empty());
        },
    )
    .await;

    // uniqueidentifier key: the UI has it as text.
    with_scratch_table(
        &driver,
        "CREATE TABLE {t} (id UNIQUEIDENTIFIER NOT NULL PRIMARY KEY, v INT NULL)",
        |driver, t| async move {
            let id = "6F9619FF-8B86-D011-B42D-00C04FC964FF";
            let values = vec![
                ("id".to_string(), Value::from(id)),
                ("v".to_string(), Value::Int(1)),
            ];
            assert_eq!(
                exec(driver, d.build_insert("dbo", &t, &values, None))
                    .await
                    .expect("insert"),
                1
            );
            let read = select_all(driver, &t, "id").await.remove(0).remove(0);
            assert_eq!(read, Value::from(id.to_lowercase()), "read back as text");
            let row = vec![("id".to_string(), read)];
            let n = exec(
                driver,
                d.build_update("dbo", &t, "v", Value::Int(2), &pk("id"), &row, None),
            )
            .await;
            assert_eq!(n.expect("update by uuid"), 1);
            let n = exec(driver, d.build_delete("dbo", &t, &pk("id"), &row, None)).await;
            assert_eq!(n.expect("delete by uuid"), 1);
        },
    )
    .await;

    // datetime2 keys: read back as text, bound back as text.
    with_scratch_table(
        &driver,
        "CREATE TABLE {t} (at DATETIME2(3) NOT NULL PRIMARY KEY, v INT NULL)",
        |driver, t| async move {
            for at in ["2026-01-02 03:04:05.678", "2026-01-02 03:04:05"] {
                let values = vec![
                    ("at".to_string(), Value::from(at)),
                    ("v".to_string(), Value::Int(1)),
                ];
                assert_eq!(
                    exec(driver, d.build_insert("dbo", &t, &values, None))
                        .await
                        .expect("insert"),
                    1
                );
            }
            let keys = select_all(driver, &t, "at").await;
            assert_eq!(
                keys,
                vec![
                    vec![Value::from("2026-01-02 03:04:05")],
                    vec![Value::from("2026-01-02 03:04:05.678")]
                ]
            );
            for key in keys {
                let row = vec![("at".to_string(), key[0].clone())];
                let n = exec(
                    driver,
                    d.build_update("dbo", &t, "v", Value::Int(2), &pk("at"), &row, None),
                )
                .await;
                assert_eq!(n.expect("update by datetime2"), 1, "{key:?}");
                let n = exec(driver, d.build_delete("dbo", &t, &pk("at"), &row, None)).await;
                assert_eq!(n.expect("delete by datetime2"), 1, "{key:?}");
            }
        },
    )
    .await;

    // datetime2(7): read back with its seven fractional digits.
    with_scratch_table(
        &driver,
        "CREATE TABLE {t} (at DATETIME2(7) NOT NULL PRIMARY KEY)",
        |driver, t| async move {
            let values = vec![("at".to_string(), Value::from("2026-01-02 03:04:05.1234567"))];
            assert_eq!(
                exec(driver, d.build_insert("dbo", &t, &values, None))
                    .await
                    .expect("insert"),
                1
            );
            let read = select_all(driver, &t, "at").await.remove(0).remove(0);
            assert_eq!(read, Value::from("2026-01-02 03:04:05.1234567"));
            let row = vec![("at".to_string(), read)];
            let n = exec(driver, d.build_delete("dbo", &t, &pk("at"), &row, None)).await;
            assert_eq!(n.expect("delete by datetime2(7)"), 1);
        },
    )
    .await;

    // Binary keys read back as bytes and bind back as varbinary. (Until
    // Task 14 they were base64 text, which silently matched no row.)
    with_scratch_table(
        &driver,
        "CREATE TABLE {t} (id VARBINARY(16) NOT NULL PRIMARY KEY, v INT NULL)",
        |driver, t| async move {
            let bytes = Value::Bytes(vec![0x00, 0xFF, 0x10]);
            let values = vec![
                ("id".to_string(), bytes.clone()),
                ("v".to_string(), Value::Int(1)),
            ];
            assert_eq!(
                exec(driver, d.build_insert("dbo", &t, &values, None))
                    .await
                    .expect("insert"),
                1
            );
            let read = select_all(driver, &t, "id").await.remove(0).remove(0);
            assert_eq!(read, bytes);
            let row = vec![("id".to_string(), read)];
            let n = exec(
                driver,
                d.build_update("dbo", &t, "v", Value::Int(2), &pk("id"), &row, None),
            )
            .await;
            assert_eq!(n.expect("update by bytes"), 1);
            let n = exec(driver, d.build_delete("dbo", &t, &pk("id"), &row, None)).await;
            assert_eq!(n.expect("delete by bytes"), 1);
        },
    )
    .await;
}

// ── DDL on the server ────────────────────────────────────────────────────────

fn column(id: &str, name: &str, ty: &str) -> CreateTableColumn {
    CreateTableColumn {
        id: id.into(),
        name: name.into(),
        ty: ty.into(),
        length: None,
        precision: None,
        nullable: true,
        default_value: String::new(),
        is_primary_key: false,
        is_unique: false,
        collation: None,
        in_unique_constraint: false,
    }
}

/// The table editor's definition of an introspected table
/// (`create-table-tabs.svelte.ts` `addFromTable`): the type split into type
/// and length or precision, the default and collation copied.
fn editor_definition(
    schema: &str,
    table: &str,
    columns: &[SchemaColumn],
    indexes: &[SchemaIndex],
) -> CreateTableDefinition {
    CreateTableDefinition {
        table_name: table.into(),
        schema_name: schema.into(),
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (ty, params) = match c.ty.split_once('(') {
                    Some((ty, rest)) => {
                        (ty.to_string(), Some(rest.trim_end_matches(')').to_string()))
                    }
                    None => (c.ty.clone(), None),
                };
                let (length, precision) = match params {
                    Some(p) if p.contains(',') => (None, Some(p)),
                    p => (p, None),
                };
                CreateTableColumn {
                    length,
                    precision,
                    nullable: c.nullable,
                    default_value: c.default_value.clone().unwrap_or_default(),
                    is_primary_key: c.is_primary_key,
                    collation: c.collation.clone(),
                    ..column(&format!("c{i}"), &c.name, &ty)
                }
            })
            .collect(),
        indexes: indexes
            .iter()
            .enumerate()
            .map(|(i, x)| CreateTableIndex {
                id: format!("i{i}"),
                name: x.name.clone(),
                columns: x.columns.clone(),
                unique: x.unique,
                ty: x.ty.clone(),
            })
            .collect(),
        foreign_keys: vec![],
    }
}

/// Runs `body`, then `cleanup` (best effort), even if `body` panics.
async fn cleanup_after<T>(
    driver: &MssqlDriver,
    cleanup: &str,
    body: impl std::future::Future<Output = T>,
) -> T {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    if let Err(e) = driver.execute(cleanup, vec![]).await {
        eprintln!("{cleanup}: {}", e.message);
    }
    outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The fix 15 CREATE TABLE and ADD cases of `bugfixes.json` run on the
/// server, and the columns keep their collations (none for DECIMAL).
#[tokio::test]
async fn collation_ddl_cases_live() {
    let Some(driver) = common::open().await else {
        return;
    };
    let cases = bugfixes().cases;
    let sql = |kind: &str| {
        cases
            .iter()
            .find(|c| c.kind == kind && c.name.starts_with("fix 15:"))
            .map(|c| c.output.as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("no fix 15 {kind} case"))
    };
    let (create, add) = (sql("ddl-create"), sql("ddl-add-column"));
    const DROP: &str = "DROP TABLE IF EXISTS [dbo].[fx_collate]";
    driver.execute(DROP, vec![]).await.expect("stale cleanup");
    let columns = cleanup_after(&driver, DROP, async {
        run_script(&driver, &create).await.expect("CREATE TABLE");
        run_script(&driver, &add).await.expect("ADD");
        driver
            .table_metadata("dbo", "fx_collate")
            .await
            .expect("metadata")
            .0
    })
    .await;
    let summary: Vec<_> = columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.ty.as_str(),
                c.nullable,
                c.collation.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("id", "int", false, None),
            ("code", "varchar(20)", false, Some("Latin1_General_BIN")),
            ("note", "nvarchar(max)", true, Some("Latin1_General_CS_AS")),
            ("amount", "decimal(10,2)", true, None),
            ("tag", "nvarchar(10)", true, Some("Latin1_General_BIN")),
        ]
    );
}

/// Statements as the UI splits a script (`splitDdlScript`: on `;` and a
/// line break), each run on its own.
async fn run_script(driver: &MssqlDriver, sql: &str) -> Result<(), DbError> {
    for stmt in sql.split(";\n").map(str::trim).filter(|s| !s.is_empty()) {
        driver.execute(stmt, vec![]).await?;
    }
    Ok(())
}

/// Fixes 2, 7, 10, 12, 13 and 15 on a real table: the dialect's CREATE,
/// then an ALTER built from the introspected table that renames, adds,
/// drops a column with a default, retypes an indexed column, changes a
/// default, toggles nullability of a sized and collated column, and drops
/// the primary key's index. It runs statement by statement and, rebuilt,
/// as one batch.
#[tokio::test]
async fn ddl_runs_on_the_server() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, "t13_ddl_").await;
    let d = MssqlDialect;
    let table = format!("{}]x", scratch_name("t13_ddl_"));
    let def = CreateTableDefinition {
        table_name: table.clone(),
        schema_name: "dbo".into(),
        columns: vec![
            CreateTableColumn {
                nullable: false,
                is_primary_key: true,
                ..column("c1", "id", "INT")
            },
            CreateTableColumn {
                length: Some("40".into()),
                ..column("c2", "na]me", "NVARCHAR")
            },
            CreateTableColumn {
                length: Some("20".into()),
                default_value: "'on'".into(),
                ..column("c3", "state", "VARCHAR")
            },
            CreateTableColumn {
                precision: Some("10,2".into()),
                default_value: "0".into(),
                nullable: false,
                ..column("c4", "amount", "DECIMAL")
            },
        ],
        indexes: vec![CreateTableIndex {
            id: "i1".into(),
            name: "t13 name idx".into(),
            columns: vec!["na]me".into()],
            unique: false,
            ty: "nonclustered".into(),
        }],
        foreign_keys: vec![],
    };
    let quoted = format!("[dbo].{}", d.quote_ident(&table));

    let outcome = cleanup_after(&driver, &format!("DROP TABLE IF EXISTS {quoted}"), async {
        run_script(&driver, &d.create_table(&def)).await?;
        driver
            .execute(
                &format!("ALTER TABLE {quoted} ALTER COLUMN [state] VARCHAR(20) COLLATE Latin1_General_BIN NULL"),
                vec![],
            )
            .await?;
        driver
            .execute(&format!("INSERT INTO {quoted} (id, [na]]me]) VALUES (1, N'x')"), vec![])
            .await?;
        let (columns, indexes) = driver.table_metadata("dbo", &table).await?;
        let from = editor_definition("dbo", &table, &columns, &indexes);
        assert_eq!(from.columns[2].collation.as_deref(), Some("Latin1_General_BIN"));
        let mut to = from.clone();
        to.columns[1].name = "full]name".into(); // rename
        to.columns[1].length = Some("80".into()); // retype the indexed column
        to.columns[2].nullable = false; // keeps varchar(20) and its collation
        to.columns[2].default_value = "'off'".into();
        to.columns.remove(3); // drop a column with a default
        to.columns.push(column("c9", "added", "INT"));
        to.indexes.clear(); // the index and the primary key's
        let alter = d.alter_table(&from, &to);
        run_script(&driver, &alter).await?;
        let after = driver.table_metadata("dbo", &table).await?;
        Ok::<_, DbError>((alter, after))
    })
    .await;

    let (alter, (columns, indexes)) = outcome.expect("DDL runs");
    assert!(
        alter.starts_with("IF EXISTS (SELECT 1 FROM sys.key_constraints"),
        "{alter}"
    );
    let summary: Vec<_> = columns
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.ty.as_str(),
                c.nullable,
                c.default_value.as_deref(),
                c.collation.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("id", "int", false, None, None),
            ("full]name", "nvarchar(80)", true, None, None),
            (
                "state",
                "varchar(20)",
                false,
                Some("('off')"),
                Some("Latin1_General_BIN")
            ),
            ("added", "int", true, None, None),
        ],
        "{alter}"
    );
    assert!(!columns[0].is_primary_key, "the primary key was dropped");
    assert!(indexes.is_empty(), "{indexes:?}");

    // The same kind of script runs as one batch too (@df1, @df2, …).
    let table2 = scratch_name("t13_ddl_");
    let def2 = CreateTableDefinition {
        table_name: table2.clone(),
        indexes: vec![],
        ..def
    };
    let quoted2 = format!("[dbo].[{table2}]");
    let outcome = cleanup_after(&driver, &format!("DROP TABLE IF EXISTS {quoted2}"), async {
        run_script(&driver, &d.create_table(&def2)).await?;
        let mut to = def2.clone();
        to.columns[2].default_value = "'x'".into();
        to.columns[3].default_value = "1".into();
        to.columns.remove(1);
        let alter = d.alter_table(&def2, &to);
        assert!(alter.contains("@df3"), "{alter}");
        driver.execute(&alter, vec![]).await?;
        driver.table_metadata("dbo", &table2).await
    })
    .await;
    let (columns, _) = outcome.expect("one batch");
    let defaults: Vec<_> = columns
        .iter()
        .map(|c| (c.name.as_str(), c.default_value.as_deref()))
        .collect();
    assert_eq!(
        defaults,
        vec![
            ("id", None),
            ("state", Some("('x')")),
            ("amount", Some("((1))"))
        ]
    );
}

/// Task 18: checking UNIQUE in edit mode adds the constraint, unchecking it
/// drops it (found by name in `sys.key_constraints`), on names that need
/// escaping, statement by statement and as one batch.
#[tokio::test]
async fn unique_checkbox_runs() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, "t18_uq_").await;
    let d = MssqlDialect;
    let table = format!("{}]'x", scratch_name("t18_uq_"));
    let quoted = format!("[dbo].{}", d.quote_ident(&table));
    let def = |email_unique: bool, with_code: bool| {
        let mut columns = vec![
            CreateTableColumn {
                nullable: false,
                is_primary_key: true,
                ..column("c1", "id", "INT")
            },
            CreateTableColumn {
                length: Some("40".into()),
                is_unique: email_unique,
                ..column("c2", "e]mail'", "NVARCHAR")
            },
        ];
        if with_code {
            columns.push(CreateTableColumn {
                is_unique: true,
                ..column("c3", "code", "INT")
            });
        }
        CreateTableDefinition {
            table_name: table.clone(),
            schema_name: "dbo".into(),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
        }
    };
    let unique_count = || async {
        let r = driver
            .query(
                "SELECT COUNT(*) FROM sys.key_constraints WHERE parent_object_id = OBJECT_ID(@P1) AND type = 'UQ'",
                vec![Value::Text(quoted.clone())],
            )
            .await
            .expect("count");
        r.rows[0][0].clone()
    };

    let outcome = cleanup_after(&driver, &format!("DROP TABLE IF EXISTS {quoted}"), async {
        run_script(&driver, &d.create_table(&def(false, false))).await?;
        assert_eq!(unique_count().await, Value::Int(0));

        // Checked, with a UNIQUE column added in the same edit.
        let add = d.alter_table(&def(false, false), &def(true, true));
        assert!(add.contains("ADD UNIQUE"), "{add}");
        run_script(&driver, &add).await?;
        assert_eq!(unique_count().await, Value::Int(2));
        run_script(
            &driver,
            &format!("INSERT INTO {quoted} VALUES (1, N'a', 1)"),
        )
        .await?;
        assert!(run_script(
            &driver,
            &format!("INSERT INTO {quoted} VALUES (2, N'a', 2)")
        )
        .await
        .is_err());

        // Unchecked, with a type change of the same column (SQL Server
        // refuses that while the constraint is there), as one batch.
        let mut to = def(false, true);
        to.columns[1].length = Some("80".into());
        let drop = d.alter_table(&def(true, true), &to);
        assert!(drop.starts_with("DECLARE @uq1"), "{drop}");
        driver.execute(&drop, vec![]).await?;
        assert_eq!(unique_count().await, Value::Int(1));
        run_script(
            &driver,
            &format!("INSERT INTO {quoted} VALUES (2, N'a', 2)"),
        )
        .await?;
        // Nothing to drop: a no-op, not an error.
        run_script(&driver, &d.alter_table(&def(true, true), &def(false, true))).await?;
        Ok::<_, DbError>(())
    })
    .await;
    outcome.expect("UNIQUE checkbox DDL");
}

/// Task 18: the query runner's row count (`countQuery` in
/// src/lib/engine/sql-scan.ts) strips a top-level ORDER BY on SQL Server,
/// which rejects it in a derived table, and keeps ORDER BY in OVER() and in
/// a TOP subquery. These are the strings its vitest cases produce.
#[tokio::test]
async fn row_count_without_trailing_order_by() {
    let Some(driver) = common::open().await else {
        return;
    };
    let count = |sql: &str| format!("SELECT COUNT(*) as total FROM ({sql}) AS count_query");
    let over = "SELECT name, ROW_NUMBER() OVER (ORDER BY name) AS rn FROM sys.types";
    let sub = "SELECT * FROM (SELECT TOP 3 name FROM sys.types ORDER BY name) s";
    let top = "SELECT TOP 5 name FROM sys.types ORDER BY name";

    // As written, with the trailing ORDER BY, the count fails.
    let err = driver
        .query(&count(&format!("{over} ORDER BY rn")), vec![])
        .await
        .expect_err("ORDER BY in a derived table");
    assert!(err.message.contains("ORDER BY"), "{}", err.message);

    for (sql, expected) in [(over, None), (sub, Some(3)), (top, Some(5))] {
        let r = driver.query(&count(sql), vec![]).await.expect(sql);
        if let Some(n) = expected {
            assert_eq!(r.rows[0][0], Value::Int(n), "{sql}");
        }
    }
}

/// Task 18: introspection reports UNIQUE (a constraint and a plain unique
/// index) and the checkbox drops both.
#[tokio::test]
async fn unique_checkbox_from_metadata() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, "t18_uqm_").await;
    let d = MssqlDialect;
    let table = format!("{}]'x", scratch_name("t18_uqm_"));
    let quoted = format!("[dbo].{}", d.quote_ident(&table));
    let outcome = cleanup_after(&driver, &format!("DROP TABLE IF EXISTS {quoted}"), async {
        run_script(
            &driver,
            &format!(
                "CREATE TABLE {quoted} (id INT PRIMARY KEY, [e]]mail'] NVARCHAR(40) UNIQUE, code INT, note INT);\n\
                 CREATE UNIQUE INDEX [code idx] ON {quoted} (code);\n"
            ),
        )
        .await?;
        seaquel_engine_testkit::run_unique_checkbox(
            &driver,
            &d,
            "dbo",
            &table,
            &["e]mail'", "code"],
            "note",
            |sql| {
                let driver = &driver;
                async move { run_script(driver, &sql).await.map_err(|e| e.message) }
            },
        )
        .await;
        Ok::<_, DbError>(())
    })
    .await;
    outcome.expect("UNIQUE from metadata");
}

/// Task 18 re-review: a filtered unique index and one with INCLUDE columns
/// aren't a column's UNIQUE, and unchecking the box leaves them alone.
#[tokio::test]
async fn unique_checkbox_skips_filtered_and_include_indexes() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, "t18_uqf_").await;
    let d = MssqlDialect;
    let table = scratch_name("t18_uqf_");
    let quoted = format!("[dbo].[{table}]");
    let outcome = cleanup_after(&driver, &format!("DROP TABLE IF EXISTS {quoted}"), async {
        run_script(
            &driver,
            &format!(
                "CREATE TABLE {quoted} (id INT PRIMARY KEY, email NVARCHAR(40) NULL, code INT, extra INT, tag INT UNIQUE);\n\
                 CREATE UNIQUE INDEX email_live ON {quoted} (email) WHERE email IS NOT NULL;\n\
                 CREATE UNIQUE INDEX code_incl ON {quoted} (code) INCLUDE (extra);\n"
            ),
        )
        .await?;
        let (columns, indexes) = driver.table_metadata("dbo", &table).await?;
        let flags: Vec<_> = columns
            .iter()
            .map(|c| (c.name.as_str(), c.is_unique, c.in_unique_constraint))
            .collect();
        assert_eq!(
            flags,
            vec![
                ("id", false, false),
                ("email", false, false),
                ("code", false, false),
                ("extra", false, false),
                ("tag", true, true),
            ]
        );
        let mut from = seaquel_engine_testkit::editor_definition("dbo", &table, &columns, &indexes);
        from.columns[1].is_unique = true;
        from.columns[2].is_unique = true;
        let mut to = from.clone();
        for c in &mut to.columns {
            c.is_unique = false;
        }
        driver.execute(&d.alter_table(&from, &to), vec![]).await?;
        let (_, after) = driver.table_metadata("dbo", &table).await?;
        let mut names: Vec<_> = after.iter().map(|i| i.name.clone()).collect();
        names.retain(|n| !n.starts_with("PK__"));
        assert_eq!(names, vec!["code_incl".to_string(), "email_live".to_string()]);
        Ok::<_, DbError>(())
    })
    .await;
    outcome.expect("filtered and INCLUDE indexes");
}
