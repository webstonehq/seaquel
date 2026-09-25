//! Live tests: the testkit's smoke and introspection checks on an in-memory
//! database (no server, no file), the `bugfixes.json` live cases on a temp
//! copy of the seeded `e2e/test-databases/duckdb/seaquel_test.duckdb`
//! (skipped when it hasn't been seeded; `npm run e2e:db:seed`), and the
//! DDL and CRUD the dialect generates, run statement by statement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value as Json;

use seaquel_engine::{
    ConnectConfig, Dialect, Driver, SchemaColumn, SchemaIndex, SchemaTable, Value,
};
use seaquel_engine_duckdb::DuckdbDialect;
use seaquel_engine_testkit::{
    run_introspection, run_smoke, scratch_name, IntrospectionExpect, IntrospectionSpec, SmokeSpec,
};
use seaquel_types::{
    CreateTableColumn, CreateTableDefinition, ForeignKeyRef, TableKind, TableSizeInfo,
};

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

async fn open(config: &ConnectConfig) -> Arc<dyn Driver> {
    seaquel_engine_duckdb::engine()
        .open(config)
        .await
        .expect("open")
}

#[tokio::test]
async fn smoke() {
    run_smoke(
        &*seaquel_engine_duckdb::engine(),
        &memory(),
        &SmokeSpec::QUESTION_MARK,
    )
    .await;
}

/// The testkit's introspection checks on a scratch schema. DuckDB keeps no
/// table or index sizes and no index usage counters, an in-memory database
/// is 0 bytes, and its plans carry no cost and no loops.
#[tokio::test]
async fn introspection() {
    let s = scratch_name("sq_duck_");
    let col = |name: &str, ty: &str, nullable: bool, pk: bool| SchemaColumn {
        name: name.into(),
        ty: ty.into(),
        cast_type: None,
        nullable,
        default_value: None,
        is_primary_key: pk,
        is_foreign_key: false,
        foreign_key_ref: None,
        collation: None,
        is_unique: false,
        in_unique_constraint: false,
    };
    let mut item_id = col("item id", "INTEGER", true, false);
    item_id.is_foreign_key = true;
    item_id.foreign_key_ref = Some(ForeignKeyRef {
        referenced_schema: s.clone(),
        referenced_table: "items".into(),
        referenced_column: "id".into(),
    });
    let mut label = col("label", "VARCHAR", true, false);
    label.default_value = Some("'x'".into());
    let mut code = col("code", "VARCHAR", false, false);
    code.is_unique = true;
    code.in_unique_constraint = true;
    let spec = IntrospectionSpec {
        schema: s.clone(),
        setup: vec![
            format!("CREATE SCHEMA {s}"),
            format!("CREATE TABLE {s}.items (id INTEGER PRIMARY KEY, label VARCHAR DEFAULT 'x', code VARCHAR NOT NULL UNIQUE)"),
            format!("CREATE TABLE {s}.\"order lines\" (\"line no\" INTEGER, \"item id\" INTEGER REFERENCES {s}.items (id))"),
            format!("CREATE INDEX \"lines by item\" ON {s}.\"order lines\" (\"item id\", \"line no\")"),
            format!("CREATE VIEW {s}.item_labels AS SELECT label FROM {s}.items"),
            format!("INSERT INTO {s}.items VALUES (1, 'a', 'A'), (2, 'b', 'B')"),
        ],
        teardown: vec![format!("DROP SCHEMA IF EXISTS {s} CASCADE")],
        stale_cleanup: vec![],
        tables: vec![
            ("item_labels".into(), TableKind::View),
            ("items".into(), TableKind::Table),
            ("order lines".into(), TableKind::Table),
        ],
        columns: vec![
            (
                "items".into(),
                vec![col("id", "INTEGER", false, true), label, code],
            ),
            (
                "order lines".into(),
                vec![col("line no", "INTEGER", true, false), item_id],
            ),
        ],
        indexes: vec![
            ("items".into(), vec![]),
            (
                "order lines".into(),
                vec![SchemaIndex {
                    name: "lines by item".into(),
                    columns: vec!["item id".into(), "line no".into()],
                    unique: false,
                    ty: "art".into(),
                }],
            ),
        ],
        stats_table: "order lines".into(),
        usage_index: "lines by item".into(),
        explain_sql: format!("SELECT label FROM {s}.items WHERE id = ?;"),
        explain_params: vec![Value::Int(1)],
        explain_relation: "items".into(),
        expect: IntrospectionExpect {
            stats_has_table_sizes: false,
            overview_has_size: false,
            stats_has_index_rows_read: false,
            stats_has_connection_count: false,
            explain_has_cost: false,
            explain_has_actuals: false,
            ..IntrospectionExpect::ALL
        },
    };
    run_introspection(&*seaquel_engine_duckdb::engine(), &memory(), &spec).await;
}

/// UNIQUE constraints in `table_metadata`: `is_unique` for a column that
/// is one on its own, `in_unique_constraint` for any column in one
/// (composite too). Not the primary key, not a unique index.
#[tokio::test]
async fn unique_flags() {
    let d = open(&memory()).await;
    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, email VARCHAR UNIQUE, a INTEGER, b INTEGER, \"it's\" VARCHAR UNIQUE, c INTEGER, UNIQUE (a, b), UNIQUE (b, email))",
        "CREATE UNIQUE INDEX t_c ON t (c)",
        "ATTACH ':memory:' AS aux",
        "CREATE TABLE aux.main.t (x INTEGER UNIQUE, y INTEGER)",
    ] {
        d.execute(sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    let flags = |cols: Vec<SchemaColumn>| {
        cols.into_iter()
            .map(|c| (c.name, c.is_unique, c.in_unique_constraint))
            .collect::<Vec<_>>()
    };
    let s = |n: &str, u, i| (n.to_string(), u, i);
    let (cols, _) = d.table_metadata("main", "t").await.unwrap();
    assert_eq!(
        flags(cols),
        [
            s("id", false, false),
            s("email", true, true),
            s("a", false, true),
            s("b", false, true),
            s("it's", true, true),
            s("c", false, false),
        ]
    );
    let (cols, _) = d.table_metadata("aux.main", "t").await.unwrap();
    assert_eq!(flags(cols), [s("x", true, true), s("y", false, false)]);
}

/// The table editor's definition of a table (`addFromTable`): the UNIQUE
/// flags as `table_metadata` reports them.
async fn definition_of(d: &dyn Driver, schema: &str, table: &str) -> CreateTableDefinition {
    let (cols, indexes) = d.table_metadata(schema, table).await.unwrap();
    CreateTableDefinition {
        table_name: table.into(),
        schema_name: schema.into(),
        columns: cols
            .iter()
            .enumerate()
            .map(|(i, c)| CreateTableColumn {
                nullable: c.nullable,
                is_primary_key: c.is_primary_key,
                is_unique: c.is_unique,
                in_unique_constraint: c.in_unique_constraint,
                default_value: c.default_value.clone().unwrap_or_default(),
                ..column(&format!("c{i}"), &c.name, &c.ty)
            })
            .collect(),
        indexes: indexes
            .iter()
            .enumerate()
            .map(|(i, x)| seaquel_types::CreateTableIndex {
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

/// The review's composite-UNIQUE cases: a TYPE change of a column in
/// `UNIQUE (a, b)`, and dropping a column before one, are notes (they fail
/// live), and the rest of the edit runs.
#[tokio::test]
async fn composite_unique_rules_run() {
    let d = open(&memory()).await;
    for sql in [
        "CREATE TABLE t (id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, z INTEGER, UNIQUE (a, b))",
        "CREATE TABLE u (x INTEGER, a INTEGER, b INTEGER, UNIQUE (a, b))",
        "INSERT INTO t VALUES (1, 1, 1, 1)",
        "INSERT INTO u VALUES (1, 1, 1)",
    ] {
        d.execute(sql, vec![]).await.unwrap();
    }
    let from = definition_of(&*d, "main", "t").await;
    let mut to = from.clone();
    to.columns[1].ty = "BIGINT".into();
    to.columns[3].ty = "BIGINT".into();
    let sql = DuckdbDialect.alter_table(&from, &to);
    assert_eq!(
        sql,
        "ALTER TABLE \"main\".\"t\" ALTER COLUMN \"z\" TYPE BIGINT;\n\
         -- DuckDB can't drop or change the type of a column in a PRIMARY KEY or UNIQUE constraint; recreate the table to change it: ALTER TABLE \"main\".\"t\" ALTER COLUMN \"a\" TYPE BIGINT"
    );
    run_script(&*d, &sql).await.unwrap();
    // The note's statement really fails.
    assert!(d
        .execute(
            "ALTER TABLE \"main\".\"t\" ALTER COLUMN \"a\" TYPE BIGINT",
            vec![]
        )
        .await
        .is_err());

    let from = definition_of(&*d, "main", "u").await;
    let mut to = from.clone();
    to.columns.remove(0);
    to.columns.push(column("n", "note", "VARCHAR"));
    let sql = DuckdbDialect.alter_table(&from, &to);
    assert_eq!(
        sql,
        "ALTER TABLE \"main\".\"u\" ADD COLUMN \"note\" VARCHAR;\n\
         -- DuckDB can't drop a column that comes before a PRIMARY KEY or UNIQUE column; recreate the table to change it: ALTER TABLE \"main\".\"u\" DROP COLUMN \"x\""
    );
    run_script(&*d, &sql).await.unwrap();
    assert!(d
        .execute("ALTER TABLE \"main\".\"u\" DROP COLUMN \"x\"", vec![])
        .await
        .is_err());
}

/// A composite foreign key pairs its columns in order (the query unnests
/// both lists with their positions). Bug fix 7's error path, where the
/// foreign-key query fails, is `bugfixes_columns_fk_error` in
/// `tests/introspect_parity.rs`.
#[tokio::test]
async fn composite_foreign_keys_pair_columns_in_order() {
    let d = open(&memory()).await;
    for sql in [
        "CREATE TABLE p (a INTEGER, b INTEGER, PRIMARY KEY (b, a))",
        "CREATE TABLE c (x INTEGER, y INTEGER, FOREIGN KEY (y, x) REFERENCES p (b, a))",
    ] {
        d.execute(sql, vec![]).await.unwrap();
    }
    let (cols, _) = d.table_metadata("main", "c").await.unwrap();
    let refs: Vec<_> = cols
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.foreign_key_ref
                    .as_ref()
                    .map(|r| r.referenced_column.as_str()),
            )
        })
        .collect();
    assert_eq!(refs, [("x", Some("a")), ("y", Some("b"))]);
}

// ── bugfixes.json, live on the seeded database ──────────────────────────────

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
    #[serde(default)]
    replaces: Option<String>,
    input: Json,
    output: Json,
}

fn bugfixes() -> Bugfixes {
    serde_json::from_str(include_str!("fixtures/bugfixes.json")).expect("bugfixes.json")
}

/// A copy of the seeded database (and its WAL) in a fresh directory, under
/// the same file name: DuckDB names the catalog after it, and the scratch
/// setup runs `USE seaquel_test`. Removed on drop.
///
/// The source is `SEAQUEL_TEST_DUCKDB_FILE` when set (a file named
/// `seaquel_test.duckdb`), else the seeded file in `e2e/test-databases`.
/// Missing, the tests skip, or fail under `SEAQUEL_TEST_REQUIRE_ENGINES`
/// (CI seeds it with `node e2e/test-databases/seed.mjs duckdb`).
struct SeededCopy(PathBuf);

impl SeededCopy {
    fn new() -> Option<Self> {
        let source = std::env::var_os("SEAQUEL_TEST_DUCKDB_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../e2e/test-databases/duckdb/seaquel_test.duckdb")
            });
        if !source.exists() {
            let msg = format!(
                "{} isn't seeded (node e2e/test-databases/seed.mjs duckdb)",
                source.display()
            );
            if std::env::var_os(seaquel_engine_testkit::REQUIRE_ENGINES).is_some() {
                panic!("{msg}");
            }
            eprintln!("skipping: {msg}");
            return None;
        }
        let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&source, dir.join("seaquel_test.duckdb")).unwrap();
        let wal = source.with_extension("duckdb.wal");
        if wal.exists() {
            std::fs::copy(&wal, dir.join("seaquel_test.duckdb.wal")).unwrap();
        }
        Some(Self(dir))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": self.0.join("seaquel_test.duckdb").to_str().unwrap(),
        }))
        .unwrap()
    }
}

impl Drop for SeededCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `schema`, `schemas`, `columns`, `indexes` and `table-sizes` cases,
/// on the scratch objects: attached catalogs told apart (fix 6), names with
/// spaces and quotes (fix 1), indexes from `duckdb_indexes()` (fix 2), rows
/// counted in each table's own catalog.
#[tokio::test]
async fn bugfix_cases_live() {
    let Some(db) = SeededCopy::new() else {
        return;
    };
    let fixtures = bugfixes();
    let d = open(&db.config()).await;
    for sql in fixtures
        .scratch
        .teardown
        .iter()
        .chain(&fixtures.scratch.setup)
    {
        d.execute(sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in fixtures.cases.iter().filter(|c| c.replaces.is_none()) {
        let schema = case.input["schema"].as_str().unwrap_or_default();
        let table = case.input["table"].as_str().unwrap_or_default();
        let diff = match case.kind.as_str() {
            "schema" => {
                let expected: Vec<SchemaTable> =
                    serde_json::from_value(case.output.clone()).unwrap();
                let actual = d.schema_tables().await.expect("schema_tables");
                (expected != actual).then(|| format!("{expected:#?}\n{actual:#?}"))
            }
            "schemas" => {
                let expected: Vec<String> = serde_json::from_value(case.output.clone()).unwrap();
                let actual = d.list_schemas().await.expect("list_schemas");
                (expected != actual).then(|| format!("{expected:?}\n{actual:?}"))
            }
            "columns" => {
                let expected: Vec<SchemaColumn> =
                    serde_json::from_value(case.output.clone()).unwrap();
                let actual = d
                    .table_metadata(schema, table)
                    .await
                    .expect("table_metadata")
                    .0;
                (expected != actual).then(|| format!("{expected:#?}\n{actual:#?}"))
            }
            "indexes" => {
                let expected: Vec<SchemaIndex> =
                    serde_json::from_value(case.output.clone()).unwrap();
                let actual = d
                    .table_metadata(schema, table)
                    .await
                    .expect("table_metadata")
                    .1;
                (expected != actual).then(|| format!("{expected:#?}\n{actual:#?}"))
            }
            "table-sizes" => {
                let expected: Vec<TableSizeInfo> =
                    serde_json::from_value(case.output.clone()).unwrap();
                let actual = d.statistics().await.expect("statistics").table_sizes;
                (expected != actual).then(|| format!("{expected:#?}\n{actual:#?}"))
            }
            _ => continue,
        };
        checked += 1;
        if let Some(diff) = diff {
            failures.push(format!("{:?}\n{diff}", case.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(checked, 18, "live bugfixes.json cases");

    // The overview names the file's catalog and adds up the file's size.
    let o = d.statistics().await.unwrap().overview;
    assert_eq!(o.database_name, "seaquel_test");
    assert!(o.total_size_bytes.is_some_and(|b| b > 0), "{o:?}");
    assert_ne!(o.total_size, "In-memory");

    // UNIQUE flags in the default catalog, none on the attached namesake.
    let unique = |cols: Vec<SchemaColumn>| {
        cols.into_iter()
            .filter(|c| c.is_unique)
            .map(|c| c.name)
            .collect::<Vec<_>>()
    };
    let (cols, _) = d.table_metadata("fx_sales", "regions").await.unwrap();
    assert_eq!(unique(cols), ["name"]);
    let (cols, _) = d
        .table_metadata("fx_aux.fx_sales", "regions")
        .await
        .unwrap();
    assert!(unique(cols).is_empty());
}

/// EXPLAIN reads the relation's own name (fix 14), in the default catalog
/// and an attached one, and binds parameters.
#[tokio::test]
async fn explain_names_the_table() {
    let d = open(&memory()).await;
    for sql in [
        "ATTACH ':memory:' AS \"fx.we\"\"ird\"",
        "CREATE TABLE \"fx.we\"\"ird\".main.\"a.b \"\"c\"\"\" (id INTEGER, v VARCHAR)",
        "INSERT INTO \"fx.we\"\"ird\".main.\"a.b \"\"c\"\"\" VALUES (1, 'x'), (2, 'y')",
    ] {
        d.execute(sql, vec![]).await.unwrap();
    }
    let sql = "SELECT v FROM \"fx.we\"\"ird\".main.\"a.b \"\"c\"\"\" WHERE id = ?";
    for analyze in [false, true] {
        let plan = d.explain(sql, vec![Value::Int(2)], analyze).await.unwrap();
        assert_eq!(plan.is_analyze, analyze);
        assert_eq!(plan.execution_time.is_some(), analyze, "{plan:?}");
        fn names(n: &seaquel_types::ExplainPlanNode, out: &mut Vec<String>) {
            out.extend(n.relation_name.clone());
            n.children.iter().for_each(|c| names(c, out));
        }
        let mut found = Vec::new();
        names(&plan.plan, &mut found);
        assert_eq!(found, ["a.b \"c\""], "{plan:#?}");
    }
}

// ── DDL, run as the table editor runs it ─────────────────────────────────────

/// `splitDdlScript` and one `execute` per statement, in autocommit (a
/// transaction would hide fix 10: DuckDB keeps a dropped index's
/// dependencies until the transaction ends). Comment lines are skipped.
async fn run_script(d: &dyn Driver, sql: &str) -> Result<(), String> {
    for stmt in sql.split(";\n") {
        let stmt = stmt.trim();
        let stmt = stmt.strip_suffix(';').unwrap_or(stmt).trim();
        if stmt.is_empty() || stmt.starts_with("--") {
            continue;
        }
        d.execute(stmt, vec![])
            .await
            .map_err(|e| format!("{stmt}\n  {}", e.message))?;
    }
    Ok(())
}

/// `CREATE SCHEMA`/`ATTACH` for a listed schema, then the table without its
/// foreign keys (the generator's `tableSetup`).
async fn create_from(d: &dyn Driver, def: &CreateTableDefinition) {
    let dialect = DuckdbDialect;
    match seaquel_engine_duckdb::parse_dotted(&def.schema_name).as_deref() {
        Some([catalog, _]) => {
            d.execute(
                &format!("ATTACH ':memory:' AS {}", dialect.quote_ident(catalog)),
                vec![],
            )
            .await
            .unwrap();
        }
        Some([schema]) if schema != "main" => {
            d.execute(
                &format!("CREATE SCHEMA {}", dialect.quote_ident(schema)),
                vec![],
            )
            .await
            .unwrap();
        }
        _ => {}
    }
    let mut plain = def.clone();
    plain.foreign_keys.clear();
    for stmt in dialect.create_table(&plain).split(";\n\n") {
        let stmt = stmt.strip_suffix(';').unwrap_or(stmt);
        d.execute(stmt, vec![])
            .await
            .unwrap_or_else(|e| panic!("{stmt}: {e:?}"));
    }
}

#[derive(Deserialize)]
struct AlterInput {
    from: CreateTableDefinition,
    to: CreateTableDefinition,
}

/// Every ALTER TABLE the dialect generates for the recorded and hand-written
/// cases runs to the end on a fresh database built from `from`: the
/// statements DuckDB would reject are notes (fixes 4, 9, 10), and the rest
/// is valid (fixes 1, 6, 8).
#[tokio::test]
async fn alter_scripts_run() {
    #[derive(Deserialize)]
    struct Recorded {
        cases: Vec<RecordedCase>,
    }
    #[derive(Deserialize)]
    struct RecordedCase {
        name: String,
        input: AlterInput,
    }
    let recorded: Recorded = serde_json::from_str(include_str!("fixtures/ddl-alter.json")).unwrap();
    let mut cases: Vec<(String, AlterInput)> = recorded
        .cases
        .into_iter()
        .map(|c| (c.name, c.input))
        .collect();
    for c in bugfixes().cases {
        if c.kind == "ddl-alter" && c.replaces.is_none() {
            cases.push((c.name, serde_json::from_value(c.input).unwrap()));
        }
    }
    let dialect = DuckdbDialect;
    let mut failures = Vec::new();
    for (name, i) in &cases {
        let d = open(&memory()).await;
        create_from(&*d, &i.from).await;
        let sql = dialect.alter_table(&i.from, &i.to);
        if let Err(e) = run_script(&*d, &sql).await {
            failures.push(format!("{name}: {e}\n{sql}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(cases.len(), 28);
}

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

/// Fix 9 on a table with rows: a NOT NULL column with a default is added
/// and made NOT NULL; one without a default is added nullable with a note.
/// Fix 10: with `isUnique` set (as the table editor copies it from
/// `table_metadata`), dropping a column before a UNIQUE one is a note, and
/// the rest of the script runs.
#[tokio::test]
async fn not_null_and_unique_rules_run() {
    let d = open(&memory()).await;
    let mut id = column("c1", "id", "INTEGER");
    id.is_primary_key = true;
    id.nullable = false;
    let name = column("c2", "name", "VARCHAR");
    let mut email = column("c3", "email", "VARCHAR");
    email.is_unique = true;
    let status = column("c4", "status", "VARCHAR");
    let from = CreateTableDefinition {
        table_name: "customers".into(),
        schema_name: "main".into(),
        columns: vec![id, name, email, status],
        indexes: vec![],
        foreign_keys: vec![],
    };
    create_from(&*d, &from).await;
    d.execute("INSERT INTO customers VALUES (1, 'a', 'a@x', 'on')", vec![])
        .await
        .unwrap();
    // The live metadata agrees with the definition.
    let live = definition_of(&*d, "main", "customers").await;
    assert_eq!(
        live.columns.iter().map(|c| c.is_unique).collect::<Vec<_>>(),
        [false, false, true, false]
    );

    let mut to = from.clone();
    to.columns
        .retain(|c| c.name != "name" && c.name != "status");
    let mut tier = column("c5", "tier", "INTEGER");
    tier.nullable = false;
    tier.default_value = "1".into();
    let mut code = column("c6", "code", "VARCHAR");
    code.nullable = false;
    to.columns.extend([tier, code]);
    let sql = DuckdbDialect.alter_table(&from, &to);
    assert_eq!(
        sql,
        "ALTER TABLE \"main\".\"customers\" ADD COLUMN \"tier\" INTEGER DEFAULT 1;\n\
         ALTER TABLE \"main\".\"customers\" ALTER COLUMN \"tier\" SET NOT NULL;\n\
         ALTER TABLE \"main\".\"customers\" ADD COLUMN \"code\" VARCHAR;\n\
         ALTER TABLE \"main\".\"customers\" DROP COLUMN \"status\";\n\
         -- DuckDB can't add column \"code\" as NOT NULL without a default; fill it, then run ALTER TABLE \"main\".\"customers\" ALTER COLUMN \"code\" SET NOT NULL\n\
         -- DuckDB can't drop a column that comes before a PRIMARY KEY or UNIQUE column; recreate the table to change it: ALTER TABLE \"main\".\"customers\" DROP COLUMN \"name\""
    );
    run_script(&*d, &sql).await.unwrap();
    let r = d
        .query("SELECT tier, code FROM customers", vec![])
        .await
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(1), Value::Null]]);
    // The note's statement really fails.
    let e = d
        .execute(
            "ALTER TABLE \"main\".\"customers\" DROP COLUMN \"name\"",
            vec![],
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("index"), "{e:?}");
}

// ── CRUD by typed keys ───────────────────────────────────────────────────────

/// Update, set-default and delete by a key of each type, with the key value
/// as the driver read it (what the UI sends back), in the default catalog
/// and an attached one. Every statement touches exactly one row.
#[tokio::test]
async fn crud_by_typed_keys() {
    let d = open(&memory()).await;
    let dialect = DuckdbDialect;
    d.execute("ATTACH ':memory:' AS aux", vec![]).await.unwrap();
    let keys = [
        ("main", "DATE", "DATE '2024-01-02'"),
        (
            "main",
            "TIMESTAMP",
            "TIMESTAMP '2024-01-02 03:04:05.123456'",
        ),
        (
            "main",
            "TIMESTAMPTZ",
            "TIMESTAMPTZ '2024-03-10 02:30:00+00'",
        ),
        ("main", "DECIMAL(18,4)", "12.3400"),
        (
            "main",
            "DECIMAL(38,10)",
            "1234567890123456789012345678.0123456789",
        ),
        (
            "main",
            "HUGEINT",
            "-170141183460469231731687303715884105728",
        ),
        (
            "main",
            "UHUGEINT",
            "340282366920938463463374607431768211455",
        ),
        ("main", "BIGINT", "9007199254740993"),
        ("main", "BLOB", "'\\xDE\\xAD\\x00'::BLOB"),
        ("main", "UUID", "'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'"),
        ("aux.main", "INTEGER", "7"),
    ];
    for (n, (schema, ty, literal)) in keys.iter().enumerate() {
        let table = format!("t{n}");
        let qt = seaquel_engine_duckdb::qualified_table(schema, &table);
        d.execute(
            &format!("CREATE TABLE {qt} (k {ty} PRIMARY KEY, v VARCHAR DEFAULT 'dflt')"),
            vec![],
        )
        .await
        .unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        d.execute(&format!("INSERT INTO {qt} VALUES ({literal}, 'a')"), vec![])
            .await
            .unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let key = d
            .query(&format!("SELECT k FROM {qt}"), vec![])
            .await
            .unwrap()
            .rows[0][0]
            .clone();
        let row = vec![("k".to_string(), key.clone())];
        let pks = vec!["k".to_string()];

        let upd = dialect.build_update(schema, &table, "v", Value::from("b"), &pks, &row, None);
        let n1 = d.execute(&upd.sql, upd.bind_values.unwrap()).await;
        assert_eq!(
            n1.map(|r| r.rows_affected).map_err(|e| e.message),
            Ok(1),
            "{ty} update, key {key:?}"
        );
        let def = dialect.build_set_default(schema, &table, "v", &pks, &row, None);
        let n2 = d.execute(&def.sql, def.bind_values.unwrap()).await;
        assert_eq!(
            n2.map(|r| r.rows_affected).map_err(|e| e.message),
            Ok(1),
            "{ty} set default"
        );
        let v = d
            .query(&format!("SELECT v FROM {qt}"), vec![])
            .await
            .unwrap()
            .rows[0][0]
            .clone();
        assert_eq!(v, Value::from("dflt"), "{ty}");
        let del = dialect.build_delete(schema, &table, &pks, &row, None);
        let n3 = d.execute(&del.sql, del.bind_values.unwrap()).await;
        assert_eq!(
            n3.map(|r| r.rows_affected).map_err(|e| e.message),
            Ok(1),
            "{ty} delete"
        );
        let ins = dialect.build_insert(
            schema,
            &table,
            &[("k".into(), key.clone()), ("v".into(), Value::from("c"))],
            None,
        );
        d.execute(&ins.sql, ins.bind_values.unwrap())
            .await
            .unwrap_or_else(|e| panic!("{ty} insert: {e:?}"));
        let back = d
            .query(&format!("SELECT k FROM {qt}"), vec![])
            .await
            .unwrap()
            .rows[0][0]
            .clone();
        assert_eq!(back, key, "{ty}: the key round-trips");
    }
}

/// Task 18: DuckDB has no `ALTER TABLE … ADD`/`DROP CONSTRAINT`, so checking
/// or unchecking UNIQUE in edit mode is a note; the statement it carries
/// fails on the engine.
#[tokio::test]
async fn unique_checkbox_is_a_note() {
    let driver = open(&memory()).await;
    let d = DuckdbDialect;
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
                ..column("c2", "email", "VARCHAR")
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
        "-- DuckDB can't add a UNIQUE constraint to an existing table: ALTER TABLE \"main\".\"u\"\"q\" ADD UNIQUE (\"email\"); create a unique index instead, or recreate the table"
    );
    let err = driver
        .execute(
            "ALTER TABLE \"main\".\"u\"\"q\" ADD UNIQUE (\"email\")",
            vec![],
        )
        .await
        .expect_err("no ADD UNIQUE");
    assert!(err.message.contains("ALTER TABLE"), "{}", err.message);

    // Unchecking a UNIQUE the table has (introspection reports it).
    run_script(&*driver, "DROP TABLE \"main\".\"u\"\"q\"")
        .await
        .expect("drop");
    run_script(&*driver, &d.create_table(&def(true)))
        .await
        .expect("create unique");
    let from = definition_of(&*driver, "main", "u\"q").await;
    assert!(from.columns[1].is_unique, "{from:?}");
    let mut to = from.clone();
    to.columns[1].is_unique = false;
    assert_eq!(
        d.alter_table(&from, &to),
        "-- DuckDB can't drop the UNIQUE constraint on \"email\" from an existing table; recreate the table to drop it"
    );
    let err = driver
        .execute(
            "ALTER TABLE \"main\".\"u\"\"q\" DROP CONSTRAINT \"u\"\"q_email_key\"",
            vec![],
        )
        .await
        .expect_err("no DROP CONSTRAINT");
    assert!(err.message.contains("ALTER TABLE"), "{}", err.message);
}
