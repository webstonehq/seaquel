//! Live tests against temp-file SQLite databases (no server needed; a file,
//! not `:memory:`, because every pooled connection would get its own
//! in-memory database).

use std::path::PathBuf;
use std::sync::Arc;

use seaquel_engine::{ConnectConfig, Dialect, Driver, SchemaColumn, SchemaIndex, Value};
use seaquel_engine_sqlite::SqliteDialect;
use seaquel_engine_testkit::{
    run_introspection, run_smoke, IntrospectionExpect, IntrospectionSpec, SmokeSpec,
};
use seaquel_types::{
    CreateTableColumn, CreateTableDefinition, CreateTableForeignKey, CreateTableIndex, TableKind,
};
use serde::Deserialize;

/// A fresh database file, deleted on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("seaquel-smoke-{}.sqlite", uuid::Uuid::new_v4())))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", self.0.display()),
            "create_if_missing": true
        }))
        .unwrap()
    }

    async fn open(&self) -> Arc<dyn Driver> {
        seaquel_engine_sqlite::engine()
            .open(&self.config())
            .await
            .expect("open")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

#[tokio::test]
async fn smoke() {
    let db = TempDb::new();
    run_smoke(
        &*seaquel_engine_sqlite::engine(),
        &db.config(),
        &SmokeSpec::QUESTION_MARK,
    )
    .await;
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
}

#[derive(Deserialize)]
struct BugfixCase {
    kind: String,
    input: serde_json::Value,
    output: serde_json::Value,
}

#[derive(Deserialize)]
struct Listed {
    name: String,
    #[serde(rename = "type")]
    kind: TableKind,
}

fn bugfixes() -> Bugfixes {
    serde_json::from_str(include_str!("fixtures/bugfixes.json")).expect("bugfixes.json")
}

/// A driver on a fresh database with the `bugfixes.json` scratch objects.
async fn scratch_db() -> (TempDb, Arc<dyn Driver>) {
    let db = TempDb::new();
    let driver = db.open().await;
    for sql in bugfixes().scratch.setup {
        driver
            .execute(&sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    (db, driver)
}

/// Rows in the tables whose names the TS rejected, so fix 1's row counts
/// are visible (the TS left them at 0).
const ODD_NAME_ROWS: [&str; 3] = [
    "INSERT INTO \"my-table\" (id) VALUES (1), (2)",
    "INSERT INTO \"it's\" (id) VALUES (1), (2), (3)",
    "INSERT INTO \"order items\" (\"order id\", sku) VALUES (1, 'A'), (1, 'B'), (2, 'A'), (3, 'C')",
];

// ── Introspection ────────────────────────────────────────────────────────────

/// The testkit's checks over the `bugfixes.json` scratch objects. Covers bug
/// fixes 1 (`my-table`, `order items` and `it's` load), 5 (index columns), 7
/// (autoindexes), 9 (implicit foreign-key columns), 10 (`sqlitex` is listed)
/// and the moved TsEngineClient parts (row counts, ANALYZE timing).
#[tokio::test]
async fn introspection() {
    let db = TempDb::new();
    let fixtures = bugfixes();
    let mut columns: Vec<(String, Vec<SchemaColumn>)> = Vec::new();
    let mut indexes: Vec<(String, Vec<SchemaIndex>)> = Vec::new();
    let mut tables = Vec::new();
    for case in fixtures.cases {
        let table = case.input["table"].as_str().unwrap_or_default().to_string();
        match case.kind.as_str() {
            "columns" => {
                columns.push((table, serde_json::from_value(case.output).expect("columns")))
            }
            "indexes" => {
                indexes.push((table, serde_json::from_value(case.output).expect("indexes")))
            }
            "schema" => {
                let listed: Vec<Listed> = serde_json::from_value(case.output).expect("schema");
                tables = listed.into_iter().map(|t| (t.name, t.kind)).collect();
            }
            _ => {}
        }
    }
    assert_eq!(
        (columns.len(), indexes.len(), tables.len()),
        (5, 5, 13),
        "bugfixes.json columns/indexes/schema cases"
    );
    assert!(tables.contains(&("sqlitex".into(), TableKind::Table)));
    assert!(tables.contains(&("customer_names".into(), TableKind::View)));

    // The UNIQUE flags Task 18 derives from the indexes; the recorded
    // columns predate them. Both are in a composite UNIQUE.
    for (table, cols) in &mut columns {
        for c in cols.iter_mut() {
            if table == "order items" && (c.name == "order id" || c.name == "sku") {
                c.in_unique_constraint = true;
            }
        }
    }

    let mut setup = fixtures.scratch.setup;
    setup.extend(ODD_NAME_ROWS.map(String::from));
    let spec = IntrospectionSpec {
        schema: "main".into(),
        setup,
        // The file goes with `db`.
        teardown: vec![],
        stale_cleanup: vec![],
        tables,
        columns,
        indexes,
        stats_table: "order items".into(),
        usage_index: "order items sku idx".into(),
        explain_sql: "SELECT sku FROM \"order items\" WHERE qty = ?;".into(),
        explain_params: vec![Value::Int(1)],
        explain_relation: "order items".into(),
        expect: IntrospectionExpect {
            supports_schemas: false,
            stats_has_table_sizes: false,
            overview_has_size: true,
            stats_has_index_rows_read: false,
            stats_has_connection_count: false,
            explain_has_cost: false,
            explain_has_actuals: false,
            ..IntrospectionExpect::ALL
        },
    };
    run_introspection(&*seaquel_engine_sqlite::engine(), &db.config(), &spec).await;
}

#[tokio::test]
async fn list_schemas_is_main() {
    let db = TempDb::new();
    let driver = db.open().await;
    assert_eq!(
        driver.list_schemas().await.unwrap(),
        vec!["main".to_string()]
    );
}

/// Row counts moved from TsEngineClient, for every table (fix 1: names with
/// a space, `-` or `'` are counted too; the TS left them at 0).
#[tokio::test]
async fn statistics_count_rows_per_table() {
    let (_db, driver) = scratch_db().await;
    for sql in ODD_NAME_ROWS {
        driver.execute(sql, vec![]).await.expect(sql);
    }
    let stats = driver.statistics().await.expect("statistics");
    let count = |name: &str| {
        stats
            .table_sizes
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {:?}", stats.table_sizes))
            .row_count
    };
    assert_eq!(
        [
            count("customers"),
            count("orders"),
            count("a"),
            count("my-table"),
            count("it's"),
            count("order items"),
            count("sqlitex")
        ],
        [200, 1000, 3, 2, 3, 4, 0]
    );
    // Fix 7: the autoindexes are listed and counted; fix 10: `sqlitex` is a table.
    assert!(stats
        .index_usage
        .iter()
        .any(|i| i.index_name == "sqlite_autoindex_customers_1"));
    assert_eq!(stats.overview.table_count, 12);
    assert_eq!(stats.overview.index_count, 10);
    assert!(stats.overview.total_size_bytes.unwrap() > 0);
}

/// SQLite has no analyzing EXPLAIN: with `analyze`, the driver runs the
/// statement (it executes, writes included) and times it (moved from
/// TsEngineClient). The root gets the row count and time; nothing else.
#[tokio::test]
async fn explain_analyze_runs_and_times_the_statement() {
    let (_db, driver) = scratch_db().await;
    let sql = "SELECT id FROM customers WHERE id < ?;";
    let plain = driver
        .explain(sql, vec![Value::Int(11)], false)
        .await
        .unwrap();
    assert!(!plain.is_analyze);
    assert_eq!((plain.execution_time, plain.plan.actual_rows), (None, None));

    let analyzed = driver
        .explain(sql, vec![Value::Int(11)], true)
        .await
        .unwrap();
    assert!(analyzed.is_analyze);
    assert_eq!(analyzed.plan.actual_rows, Some(10.0));
    let ms = analyzed.execution_time.expect("execution time");
    assert!(ms >= 0.0);
    assert_eq!(analyzed.plan.actual_total_time, Some(ms));
    assert_eq!(analyzed.plan.relation_name.as_deref(), Some("customers"));
    assert_eq!(analyzed.plan.plan_rows, None, "no invented estimate");

    // It executes: an analyzed DELETE deletes.
    let deleted = driver
        .explain("DELETE FROM b WHERE x = ?", vec![Value::Int(2)], true)
        .await
        .unwrap();
    assert_eq!(deleted.plan.actual_rows, Some(0.0));
    let left = driver
        .query("SELECT COUNT(*) FROM b", vec![])
        .await
        .unwrap();
    assert_eq!(left.rows[0][0], Value::Int(2));

    // An empty plan (INSERT … VALUES) is still timed.
    let insert = driver
        .explain("INSERT INTO b VALUES (7, 7)", vec![], true)
        .await
        .unwrap();
    assert_eq!(insert.plan.node_type, "Query Plan");
    assert!(insert.execution_time.is_some());
}

/// Fix 8 end to end: names with spaces and non-ASCII names in real plans.
#[tokio::test]
async fn explain_reads_odd_names() {
    let (_db, driver) = scratch_db().await;
    let plan = driver
        .explain(
            "SELECT sku FROM \"order items\" WHERE sku = ?",
            vec![Value::from("A")],
            false,
        )
        .await
        .unwrap()
        .plan;
    assert_eq!(plan.node_type, "Index Only Scan");
    assert_eq!(plan.relation_name.as_deref(), Some("order items"));
    assert_eq!(plan.index_name.as_deref(), Some("order items sku idx"));
    let plan = driver
        .explain(
            "SELECT * FROM \"café\" WHERE \"naïve\" = 'é'",
            vec![],
            false,
        )
        .await
        .unwrap()
        .plan;
    assert_eq!(plan.relation_name.as_deref(), Some("café"));
    assert_eq!(plan.index_name.as_deref(), Some("café_naïve_idx"));
}

// ── DDL (fixes 2, 3, 4 and 6 executed) ───────────────────────────────────────

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

/// Runs a generated script the way the table editor does: split on `;\n`,
/// skip empty and `--` pieces, run the rest in order.
async fn run_script(driver: &dyn Driver, sql: &str) -> Result<usize, seaquel_engine::DbError> {
    let mut ran = 0;
    for stmt in sql
        .split(";\n")
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
    {
        let stmt = if stmt.ends_with(';') {
            stmt.to_string()
        } else {
            format!("{stmt};")
        };
        driver.execute(&stmt, vec![]).await?;
        ran += 1;
    }
    Ok(ran)
}

async fn index_columns(driver: &dyn Driver, table: &str) -> Vec<(String, Vec<String>)> {
    let (_, indexes) = driver
        .table_metadata("main", table)
        .await
        .expect("metadata");
    indexes.into_iter().map(|i| (i.name, i.columns)).collect()
}

#[tokio::test]
async fn ddl_fixes_run() {
    let db = TempDb::new();
    let driver = db.open().await;
    let d = SqliteDialect;

    // Fix 2: names with `"` in CREATE TABLE and its index.
    let mut id = column("c1", "id\"", "INTEGER");
    id.nullable = false;
    id.is_primary_key = true;
    let mut note = column("c2", "n\"ote", "TEXT");
    note.is_unique = true;
    let from = CreateTableDefinition {
        table_name: "we\"ird".into(),
        schema_name: "main".into(),
        columns: vec![id, note],
        indexes: vec![],
        foreign_keys: vec![],
    };
    run_script(&*driver, &d.create_table(&from))
        .await
        .expect("create");
    driver
        .execute("CREATE TABLE users (id INTEGER PRIMARY KEY)", vec![])
        .await
        .unwrap();

    // Fix 3: an added index is valid SQLite (the TS form is not).
    let mut with_index = from.clone();
    with_index.indexes.push(CreateTableIndex {
        id: "i1".into(),
        name: "we\"ird_note".into(),
        columns: vec!["n\"ote".into()],
        unique: false,
        ty: "btree".into(),
    });
    let sql = d.alter_table(&from, &with_index);
    assert_eq!(
        sql,
        "CREATE INDEX \"main\".\"we\"\"ird_note\" ON \"we\"\"ird\" (\"n\"\"ote\");"
    );
    run_script(&*driver, &sql).await.expect("create index");
    assert!(index_columns(&*driver, "we\"ird")
        .await
        .contains(&("we\"ird_note".into(), vec!["n\"ote".into()])));
    let ts = "CREATE INDEX \"ts_form\" ON \"main\".\"we\"\"ird\" (\"n\"\"ote\");";
    assert!(
        driver.execute(ts, vec![]).await.is_err(),
        "SQLite rejects the TS form"
    );

    // Fixes 4 and 6: a rename and a new column run, the new column's foreign
    // key inline; a type change and a foreign key on an existing column are
    // comment lines the editor skips.
    let mut to = with_index.clone();
    to.columns[1].name = "memo".into();
    to.columns[1].ty = "VARCHAR".into();
    to.columns[1].length = Some("20".into());
    to.columns.push(column("c3", "owner_id", "INTEGER"));
    let users_id = |id: &str, column: &str| CreateTableForeignKey {
        id: id.into(),
        column: column.into(),
        referenced_schema: "main".into(),
        referenced_table: "users".into(),
        referenced_column: "id".into(),
    };
    to.foreign_keys.push(users_id("f1", "owner_id"));
    to.foreign_keys.push(users_id("f2", "id\""));
    let sql = d.alter_table(&with_index, &to);
    assert_eq!(
        sql,
        "ALTER TABLE \"main\".\"we\"\"ird\" RENAME COLUMN \"n\"\"ote\" TO \"memo\";\n\
         ALTER TABLE \"main\".\"we\"\"ird\" ADD COLUMN \"owner_id\" INTEGER REFERENCES \"users\" (\"id\");\n\
         -- SQLite can't alter column \"memo\" (type); recreate the table to change it\n\
         -- SQLite can't add a foreign key to an existing table: (\"id\"\"\") REFERENCES \"main\".\"users\" (\"id\"); recreate the table to add it"
    );
    assert_eq!(run_script(&*driver, &sql).await.expect("alter"), 2);
    let (columns, _) = driver.table_metadata("main", "we\"ird").await.unwrap();
    let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id\"", "memo", "owner_id"]);
    assert_eq!(columns[1].ty, "TEXT", "the type change was not applied");
    assert!(!columns[0].is_foreign_key);
    let owner = columns[2]
        .foreign_key_ref
        .as_ref()
        .expect("inline foreign key");
    assert_eq!(
        (
            owner.referenced_table.as_str(),
            owner.referenced_column.as_str()
        ),
        ("users", "id")
    );
    let ts = "ALTER TABLE \"main\".\"we\"\"ird\" ADD FOREIGN KEY (\"memo\") REFERENCES \"main\".\"users\" (\"id\");";
    assert!(
        driver.execute(ts, vec![]).await.is_err(),
        "SQLite has no ADD FOREIGN KEY"
    );

    // ADD COLUMN (the TS always used "main").
    let mut code = column("c9", "co\"de", "VARCHAR");
    code.nullable = false;
    code.default_value = "'x'".into();
    run_script(&*driver, &d.add_column("aux", "we\"ird", &code))
        .await
        .expect("add column");
    let (columns, _) = driver.table_metadata("main", "we\"ird").await.unwrap();
    assert_eq!(
        columns
            .last()
            .map(|c| (c.name.as_str(), c.default_value.as_deref())),
        Some(("co\"de", Some("'x'")))
    );
}

// ── CRUD ─────────────────────────────────────────────────────────────────────

/// `$N` placeholders bind in order through sqlx, and no casts are applied
/// (Task 1: `CAST('2024-01-01 10:00' AS DATETIME)` stores 2024 and
/// `CAST('{"a":1}' AS JSON)` stores 0).
#[tokio::test]
async fn crud_round_trip_keeps_text() {
    let db = TempDb::new();
    let driver = db.open().await;
    driver
        .execute(
            "CREATE TABLE ev (region TEXT, id INTEGER, at DATETIME, doc JSON, note TEXT, PRIMARY KEY (region, id))",
            vec![],
        )
        .await
        .unwrap();
    let d = SqliteDialect;
    let casts = [
        ("at".to_string(), "DATETIME".to_string()),
        ("doc".to_string(), "JSON".to_string()),
    ]
    .into_iter()
    .collect();
    let s = |v: &str| Value::from(v);
    let values = vec![
        ("region".to_string(), s("eu")),
        ("id".to_string(), Value::Int(1)),
        ("at".to_string(), s("2024-01-01 10:00")),
        ("doc".to_string(), s("{\"a\":1}")),
    ];
    let insert = d.build_insert("main", "ev", &values, Some(&casts));
    assert_eq!(
        insert.sql,
        "INSERT INTO \"main\".\"ev\" (\"region\", \"id\", \"at\", \"doc\") VALUES ($1, $2, $3, $4)"
    );
    driver
        .execute(&insert.sql, insert.bind_values.unwrap())
        .await
        .unwrap();

    let pks = vec!["region".to_string(), "id".to_string()];
    let row = vec![
        ("id".to_string(), Value::Int(1)),
        ("region".to_string(), s("eu")),
    ];
    let update = d.build_update(
        "main",
        "ev",
        "at",
        s("2025-06-07 08:09"),
        &pks,
        &row,
        Some(&casts),
    );
    let r = driver
        .execute(&update.sql, update.bind_values.unwrap())
        .await
        .unwrap();
    assert_eq!(r.rows_affected, 1);
    let update = d.build_update(
        "main",
        "ev",
        "doc",
        s("{\"b\":[1,2]}"),
        &pks,
        &row,
        Some(&casts),
    );
    driver
        .execute(&update.sql, update.bind_values.unwrap())
        .await
        .unwrap();

    let got = driver
        .query("SELECT at, typeof(at), doc, typeof(doc) FROM ev", vec![])
        .await
        .unwrap();
    assert_eq!(
        got.rows[0],
        vec![
            s("2025-06-07 08:09"),
            s("text"),
            s("{\"b\":[1,2]}"),
            s("text")
        ]
    );

    let delete = d.build_delete("main", "ev", &pks, &row, None);
    assert_eq!(
        delete.sql,
        "DELETE FROM \"main\".\"ev\" WHERE \"region\" = $1 AND \"id\" = $2"
    );
    let r = driver
        .execute(&delete.sql, delete.bind_values.unwrap())
        .await
        .unwrap();
    assert_eq!(r.rows_affected, 1);
}

/// Fix 7 in the table editor: its definition holds the autoindexes from
/// `table_metadata`. Removing one (or every index) must still give a script
/// that runs: the autoindex is a comment line, the ordinary index is dropped.
#[tokio::test]
async fn removing_an_autoindex_in_the_editor_runs() {
    let (_db, driver) = scratch_db().await;
    let (columns, indexes) = driver.table_metadata("main", "customers").await.unwrap();
    assert!(indexes
        .iter()
        .any(|i| i.name == "sqlite_autoindex_customers_1"));
    // What `create-table-tabs.svelte.ts` builds from the metadata.
    let from = CreateTableDefinition {
        table_name: "customers".into(),
        schema_name: "main".into(),
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| CreateTableColumn {
                nullable: c.nullable,
                default_value: c.default_value.clone().unwrap_or_default(),
                is_primary_key: c.is_primary_key,
                ..column(&format!("c{i}"), &c.name, &c.ty)
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
    };
    // Renaming the autoindex: only the note, no second unique index.
    let mut renamed = from.clone();
    for i in &mut renamed.indexes {
        if i.name == "sqlite_autoindex_customers_1" {
            i.name = "customers_email_key".into();
        }
    }
    let sql = SqliteDialect.alter_table(&from, &renamed);
    assert!(
        sql.starts_with("-- SQLite can't drop or rename index"),
        "{sql}"
    );
    assert_eq!(run_script(&*driver, &sql).await.expect("rename"), 0);

    let mut to = from.clone();
    to.indexes
        .retain(|i| i.name != "sqlite_autoindex_customers_1" && i.name != "customers_name_idx");
    let sql = SqliteDialect.alter_table(&from, &to);
    assert_eq!(
        sql,
        "DROP INDEX \"customers_name_idx\";\n\
         -- SQLite can't drop or rename index \"sqlite_autoindex_customers_1\": it belongs to a UNIQUE or PRIMARY KEY constraint; recreate the table to change it"
    );
    assert_eq!(run_script(&*driver, &sql).await.expect("alter"), 1);
    let names: Vec<String> = index_columns(&*driver, "customers")
        .await
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(names.contains(&"sqlite_autoindex_customers_1".to_string()));
    assert!(!names.contains(&"customers_name_idx".to_string()));
    // The TS form fails.
    let err = driver
        .execute("DROP INDEX \"sqlite_autoindex_customers_1\";", vec![])
        .await
        .expect_err("SQLite can't drop a constraint's index");
    assert!(err.message.contains("cannot be dropped"), "{err:?}");
}

/// Fix 11: Set to default runs, with the default expression taken from
/// `table_metadata` as the UI does: a literal, `CURRENT_TIMESTAMP`
/// (evaluated now, not the insert time), an expression default and a column
/// without one (NULL). The `bugfixes.json` cases run too. The TypeScript
/// `SET … = DEFAULT` fails.
#[tokio::test]
async fn set_default_runs() {
    let (_db, driver) = scratch_db().await;
    driver
        .execute(
            "CREATE TABLE \"set defaults\" (k TEXT, n INTEGER, slug TEXT DEFAULT (lower('A') || 'b'), PRIMARY KEY (k, n))",
            vec![],
        )
        .await
        .unwrap();
    driver
        .execute(
            "INSERT INTO \"set defaults\" (k, n, slug) VALUES ('x', 2, 'custom')",
            vec![],
        )
        .await
        .unwrap();
    driver
        .execute(
            "UPDATE customers SET status = 'gone', score = 7, note = 'n', created_at = '2000-01-01 00:00:00' WHERE id = 1",
            vec![],
        )
        .await
        .unwrap();

    let d = SqliteDialect;
    let pk = vec!["id".to_string()];
    let row = vec![("id".to_string(), Value::Int(1))];
    let (columns, _) = driver.table_metadata("main", "customers").await.unwrap();
    for column in ["status", "score", "note", "created_at"] {
        let c = columns.iter().find(|c| c.name == column).unwrap();
        let default = c.default_value.as_deref().unwrap_or("NULL");
        let built =
            d.build_set_default_expr("main", "customers", column, Some(default), &pk, &row, None);
        let r = driver
            .execute(&built.sql, built.bind_values.unwrap())
            .await
            .unwrap_or_else(|e| panic!("{}: {e:?}", built.sql));
        assert_eq!(r.rows_affected, 1, "{}", built.sql);
    }
    let got = driver
        .query(
            "SELECT status, score, note, created_at > '2020' FROM customers WHERE id = 1",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        got.rows[0],
        vec![
            Value::from("active"),
            Value::Float(0.0),
            Value::Null,
            Value::Int(1)
        ]
    );

    driver.execute(BARE_DEFAULTS, vec![]).await.unwrap();
    driver
        .execute("INSERT INTO \"bare defaults\" (id) VALUES (1)", vec![])
        .await
        .unwrap();

    // Every bugfixes.json case with a default runs against the scratch objects.
    for case in bugfixes()
        .cases
        .into_iter()
        .filter(|c| c.kind == "crud-set-default")
    {
        let binds: Vec<Value> = case.output["bindValues"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(Value::from_json_cell)
            .collect();
        let sql = case.output["sql"].as_str().unwrap();
        let result = driver.execute(sql, binds).await;
        if case.input.get("columnDefault").is_some() {
            assert_eq!(result.expect(sql).rows_affected, 1, "{sql}");
        } else {
            let err = result.expect_err("SQLite has no DEFAULT in UPDATE");
            assert!(err.message.contains("syntax error"), "{err:?}");
        }
    }
    let slug = driver
        .query("SELECT slug FROM \"set defaults\"", vec![])
        .await
        .unwrap();
    assert_eq!(slug.rows[0], vec![Value::from("ab")]);
}

/// Columns with every kind of default SQLite's DEFAULT clause takes raw, and
/// decoy columns named like the bare-word and quoted-name defaults, holding
/// `'SECRET'`: an expression that reads a column would copy it.
const BARE_DEFAULTS: &str = "CREATE TABLE \"bare defaults\" (\
    id INTEGER PRIMARY KEY, active TEXT DEFAULT 'SECRET', dq TEXT DEFAULT 'SECRET', \
    bt TEXT DEFAULT 'SECRET', br TEXT DEFAULT 'SECRET', \
    s DEFAULT active, d DEFAULT \"dq\", q DEFAULT \"it's \"\"x\"\"\", b DEFAULT `bt`, k DEFAULT [br], \
    t DEFAULT TRUE, n DEFAULT +1, h DEFAULT -0x10, x DEFAULT x'00ff', e DEFAULT (1+2), \
    c DEFAULT current_date, z)";

/// Fix 11: Set to default writes what an INSERT would store, for every kind
/// of default, with the default taken from `table_metadata` as the UI does.
#[tokio::test]
async fn set_default_writes_what_insert_would() {
    let db = TempDb::new();
    let driver = db.open().await;
    driver.execute(BARE_DEFAULTS, vec![]).await.unwrap();
    let targets = ["s", "d", "q", "b", "k", "t", "n", "h", "x", "e", "c", "z"];
    // Row 1: every target overwritten; row 2: every default as INSERT stores it.
    let assigns: Vec<String> = targets.iter().map(|c| format!("{c} = 'changed'")).collect();
    driver
        .execute("INSERT INTO \"bare defaults\" (id) VALUES (1), (2)", vec![])
        .await
        .unwrap();
    driver
        .execute(
            &format!(
                "UPDATE \"bare defaults\" SET {} WHERE id = 1",
                assigns.join(", ")
            ),
            vec![],
        )
        .await
        .unwrap();

    let (columns, _) = driver
        .table_metadata("main", "bare defaults")
        .await
        .unwrap();
    let pk = vec!["id".to_string()];
    let row = vec![("id".to_string(), Value::Int(1))];
    for column in targets {
        let c = columns.iter().find(|c| c.name == column).unwrap();
        let built = SqliteDialect.build_set_default_expr(
            "main",
            "bare defaults",
            column,
            Some(c.default_value.as_deref().unwrap_or("NULL")),
            &pk,
            &row,
            None,
        );
        driver
            .execute(&built.sql, built.bind_values.unwrap())
            .await
            .unwrap_or_else(|e| panic!("{}: {e:?}", built.sql));
    }
    let quoted: Vec<String> = targets.iter().map(|c| format!("quote({c})")).collect();
    let got = driver
        .query(
            &format!(
                "SELECT {} FROM \"bare defaults\" ORDER BY id",
                quoted.join(", ")
            ),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        got.rows[0], got.rows[1],
        "set default vs INSERT, columns {targets:?}"
    );
    let text = |v: &str| Value::from(v);
    assert_eq!(
        got.rows[0],
        vec![
            text("'active'"),
            text("'dq'"),
            text("'it''s \"x\"'"),
            text("'bt'"),
            text("'br'"),
            text("1"),
            text("1"),
            text("-16"),
            text("X'00FF'"),
            text("3"),
            got.rows[1][10].clone(),
            text("NULL"),
        ]
    );
    assert!(!got.rows[0].contains(&text("'SECRET'")));
}

/// Task 18: SQLite can't add or drop a UNIQUE constraint on an existing
/// table, so checking or unchecking UNIQUE in edit mode is a note. The
/// statement the note carries fails, and the unique index it suggests works.
#[tokio::test]
async fn unique_checkbox_is_a_note() {
    let db = TempDb::new();
    let driver = db.open().await;
    let d = SqliteDialect;
    let def = |email_unique: bool| CreateTableDefinition {
        table_name: "u\"q".into(),
        schema_name: "main".into(),
        columns: vec![
            CreateTableColumn {
                nullable: false,
                is_primary_key: true,
                ..column("c1", "id", "INTEGER")
            },
            CreateTableColumn {
                is_unique: email_unique,
                ..column("c2", "email", "TEXT")
            },
        ],
        indexes: vec![],
        foreign_keys: vec![],
    };
    run_script(&*driver, &d.create_table(&def(false)))
        .await
        .expect("create");

    let add = d.alter_table(&def(false), &def(true));
    assert_eq!(
        add,
        "-- SQLite can't add a UNIQUE constraint to an existing table: ALTER TABLE \"main\".\"u\"\"q\" ADD UNIQUE (\"email\"); create a unique index instead, or recreate the table"
    );
    assert_eq!(run_script(&*driver, &add).await.expect("notes only"), 0);
    assert!(driver
        .execute(
            "ALTER TABLE \"main\".\"u\"\"q\" ADD UNIQUE (\"email\")",
            vec![]
        )
        .await
        .is_err());
    driver
        .execute(
            "CREATE UNIQUE INDEX \"main\".\"uq_email\" ON \"u\"\"q\" (\"email\")",
            vec![],
        )
        .await
        .expect("a unique index instead");

    let drop = d.alter_table(&def(true), &def(false));
    assert_eq!(
        drop,
        "-- SQLite can't drop the UNIQUE constraint on \"email\" from an existing table; recreate the table to drop it"
    );
}

/// Task 18: introspection reports UNIQUE. Unchecking a plain unique index
/// drops it; a UNIQUE constraint's autoindex is a note.
#[tokio::test]
async fn unique_checkbox_from_metadata() {
    let db = TempDb::new();
    let driver = db.open().await;
    let d = SqliteDialect;
    run_script(
        &*driver,
        "CREATE TABLE \"u\"\"q\" (id INTEGER PRIMARY KEY, email TEXT UNIQUE, code INTEGER, note INTEGER);\n\
         CREATE UNIQUE INDEX \"code idx\" ON \"u\"\"q\" (code);\n",
    )
    .await
    .expect("setup");
    let (columns, indexes) = driver
        .table_metadata("main", "u\"q")
        .await
        .expect("metadata");
    let from = seaquel_engine_testkit::editor_definition("main", "u\"q", &columns, &indexes);
    let flags: Vec<_> = from
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.is_unique))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("id", false),
            ("email", true),
            ("code", true),
            ("note", false)
        ]
    );
    assert_eq!(
        d.alter_table(&from, &from.clone()),
        "-- No changes detected"
    );

    let mut to = from.clone();
    to.columns[1].is_unique = false;
    to.columns[2].is_unique = false;
    let sql = d.alter_table(&from, &to);
    assert_eq!(
        sql,
        "DROP INDEX \"main\".\"code idx\";\n\
         -- SQLite can't drop the UNIQUE constraint on \"email\" from an existing table; recreate the table to drop it"
    );
    assert_eq!(run_script(&*driver, &sql).await.expect("runs"), 1);
    let (columns, _) = driver
        .table_metadata("main", "u\"q")
        .await
        .expect("metadata");
    assert!(columns[1].is_unique && !columns[2].is_unique, "{columns:?}");
}

/// Task 18 re-review: a partial unique index isn't a column's UNIQUE, and
/// unchecking a column that also has one doesn't drop it (a note instead).
#[tokio::test]
async fn unique_checkbox_skips_partial_indexes() {
    let db = TempDb::new();
    let driver = db.open().await;
    let d = SqliteDialect;
    run_script(
        &*driver,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, email TEXT, deleted_at TEXT, code INTEGER);\n\
         CREATE UNIQUE INDEX a_email_live ON t (email) WHERE deleted_at IS NULL;\n\
         CREATE UNIQUE INDEX b_code_live ON t (code) WHERE deleted_at IS NULL;\n\
         CREATE UNIQUE INDEX c_code_uq ON t (code);\n",
    )
    .await
    .expect("setup");
    let (columns, indexes) = driver.table_metadata("main", "t").await.expect("metadata");
    let flags: Vec<_> = columns
        .iter()
        .map(|c| (c.name.as_str(), c.is_unique, c.in_unique_constraint))
        .collect();
    assert_eq!(
        flags,
        vec![
            ("id", false, false),
            ("email", false, false),
            ("deleted_at", false, false),
            ("code", true, true),
        ]
    );
    let from = seaquel_engine_testkit::editor_definition("main", "t", &columns, &indexes);
    let mut to = from.clone();
    to.columns[3].is_unique = false;
    assert_eq!(
        d.alter_table(&from, &to),
        "-- SQLite can't tell which unique index on \"code\" is its UNIQUE: drop it under Indexes"
    );
    // With only the plain one, unchecking drops it and keeps the partial one.
    driver
        .execute("DROP INDEX b_code_live", vec![])
        .await
        .expect("drop");
    let (columns, indexes) = driver.table_metadata("main", "t").await.expect("metadata");
    let from = seaquel_engine_testkit::editor_definition("main", "t", &columns, &indexes);
    let mut to = from.clone();
    to.columns[3].is_unique = false;
    let sql = d.alter_table(&from, &to);
    assert_eq!(sql, "DROP INDEX \"main\".\"c_code_uq\";");
    run_script(&*driver, &sql).await.expect("runs");
    let (_, after) = driver.table_metadata("main", "t").await.expect("metadata");
    let names: Vec<_> = after.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, vec!["a_email_live"]);
}
