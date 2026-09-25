//! Live tests against both servers. Each reads its ConnectConfig JSON from
//! an environment variable and skips when it's unset (unless
//! `SEAQUEL_TEST_REQUIRE_ENGINES` is set):
//! - SEAQUEL_TEST_MYSQL, e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}
//! - SEAQUEL_TEST_MARIADB (MariaDB uses the mysql driver), e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}

use seaquel_engine::{ConnectConfig, Dialect, Driver, SchemaColumn, SchemaIndex, Value};
use seaquel_engine_mysql::MysqlDialect;
use seaquel_engine_testkit::{
    config_from_env, run_introspection, run_smoke, scratch_name, IntrospectionExpect,
    IntrospectionSpec, SmokeSpec,
};
use seaquel_types::{CreateTableColumn, CreateTableDefinition, CreateTableIndex, TableKind};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Server {
    Mysql,
    Mariadb,
}

impl Server {
    fn config(self) -> Option<ConnectConfig> {
        config_from_env(match self {
            Server::Mysql => "SEAQUEL_TEST_MYSQL",
            Server::Mariadb => "SEAQUEL_TEST_MARIADB",
        })
    }

    fn bugfixes(self) -> &'static str {
        match self {
            Server::Mysql => include_str!("fixtures/mysql/bugfixes.json"),
            Server::Mariadb => include_str!("fixtures/mariadb/bugfixes.json"),
        }
    }
}

#[tokio::test]
async fn mysql() {
    let Some(config) = Server::Mysql.config() else {
        return;
    };
    run_smoke(
        &*seaquel_engine_mysql::engine(),
        &config,
        &SmokeSpec::QUESTION_MARK,
    )
    .await;
}

#[tokio::test]
async fn mariadb() {
    let Some(config) = Server::Mariadb.config() else {
        return;
    };
    run_smoke(
        &*seaquel_engine_mysql::engine(),
        &config,
        &SmokeSpec::QUESTION_MARK,
    )
    .await;
}

// ── Connection strings ───────────────────────────────────────────────────────

/// `config` with the database in its `mysql://…/<db>` URL replaced.
fn with_database(config: &ConnectConfig, db: &str) -> ConnectConfig {
    let url = config.connection_string.clone().expect("connection_string");
    let (base, query) = url
        .split_once('?')
        .map_or((url.as_str(), None), |(b, q)| (b, Some(q)));
    let slash = base.rfind('/').expect("a database path");
    let mut out = format!("{}/{db}", &base[..slash]);
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    ConnectConfig {
        connection_string: Some(out),
        ..config.clone()
    }
}

/// `config` logged in as `user:password` instead of its own user.
fn with_user(config: &ConnectConfig, user: &str, password: &str) -> ConnectConfig {
    let url = config.connection_string.clone().expect("connection_string");
    let (scheme, rest) = url.split_once("://").expect("a URL");
    let host = rest.split_once('@').map_or(rest, |(_, h)| h);
    ConnectConfig {
        connection_string: Some(format!("{scheme}://{user}:{password}@{host}")),
        ..config.clone()
    }
}

async fn open(config: &ConnectConfig) -> std::sync::Arc<dyn Driver> {
    seaquel_engine_mysql::engine()
        .open(config)
        .await
        .expect("open")
}

// ── Introspection ────────────────────────────────────────────────────────────

/// `bugfixes.json`: the scratch objects and the live expectations.
#[derive(Deserialize)]
struct Bugfixes {
    scratch: Scratch,
    cases: Vec<BugfixCase>,
}

#[derive(Deserialize)]
struct Scratch {
    schema: String,
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

const SCRATCH_PREFIX: &str = "seaquel_introspect_";

/// Drop the scratch databases of earlier runs killed before their teardown.
/// Same caveat as `IntrospectionSpec::stale_cleanup`: test servers only.
async fn drop_stale_databases(driver: &dyn Driver, keep: &str) {
    let r = driver
        .query(
            "SELECT SCHEMA_NAME AS name FROM information_schema.SCHEMATA WHERE SCHEMA_NAME LIKE ?",
            vec![Value::from(format!(
                "{}%",
                SCRATCH_PREFIX.replace('_', "\\_")
            ))],
        )
        .await
        .expect("list scratch databases");
    for row in r.rows {
        let name = match &row[0] {
            Value::Text(s) => s.clone(),
            other => panic!("unexpected schema name cell {other:?}"),
        };
        if name != keep {
            if let Err(e) = driver
                .execute(&format!("DROP DATABASE `{name}`"), vec![])
                .await
            {
                eprintln!("stale cleanup of {name} failed: {e:?}");
            }
        }
    }
}

/// The `bugfixes.json` scratch objects in a fresh database (the adapter's
/// schema, table-size and statistics queries look at `DATABASE()` only, so
/// the driver connects to it). Covers bug fixes 1 (tables named
/// `fx order items`, `fx-my-table` and `fx.dotted` load), 3 (PRI/UNI columns
/// that are foreign keys, a reference to a dotted table), 4 (UTF-8 names and
/// defaults, views listed as views), 5 (`EXPLAIN ANALYZE` of a table whose
/// name has spaces), 8 (MariaDB defaults) and 9 (index usage).
async fn introspection(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let db = scratch_name(SCRATCH_PREFIX);
    let admin = open(&config).await;
    drop_stale_databases(&*admin, &db).await;
    admin
        .execute(&format!("CREATE DATABASE `{db}`"), vec![])
        .await
        .expect("create scratch database");
    admin.close().await.expect("close");

    let recorded: Bugfixes = serde_json::from_str(server.bugfixes()).expect("bugfixes.json");
    // The expectations name the recording database; the scratch SQL is unqualified.
    let text = server.bugfixes().replace(&recorded.scratch.schema, &db);
    let fixtures: Bugfixes = serde_json::from_str(&text).expect("bugfixes.json");

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
        (7, 2, 10),
        "bugfixes.json columns/indexes/schema cases"
    );
    assert!(tables.contains(&("fx_customer_names".into(), TableKind::View)));
    // The UNIQUE flags Task 18 derives from the indexes; the recorded
    // columns predate them.
    const UNIQUE: [(&str, &str, bool); 4] = [
        ("fx_customers", "email", true),
        ("fx_accounts", "email", true),
        ("fx order items", "order id", false),
        ("fx order items", "sku", false),
    ];
    for (table, cols) in &mut columns {
        for c in cols.iter_mut() {
            if let Some((_, _, single)) = UNIQUE.iter().find(|(t, n, _)| t == table && *n == c.name)
            {
                c.is_unique = *single;
                c.in_unique_constraint = true;
            }
        }
    }

    let mut setup = fixtures.scratch.setup;
    // Persistent statistics for innodb_index_stats (fix 9).
    setup.push("ANALYZE TABLE `fx order items`, fx_customers".into());

    let analyzed = match server {
        // MariaDB analyzes with ANALYZE FORMAT=JSON, which has the times.
        Server::Mariadb => true,
        // The TS reported no execution time for MySQL's text tree.
        Server::Mysql => false,
    };
    let spec = IntrospectionSpec {
        schema: db.clone(),
        setup,
        teardown: vec![format!("DROP DATABASE IF EXISTS `{db}`")],
        stale_cleanup: vec![],
        tables,
        columns,
        indexes,
        stats_table: "fx order items".into(),
        usage_index: "fx order items sku idx".into(),
        explain_sql: "SELECT sku FROM `fx order items` WHERE qty = ?;".into(),
        explain_params: vec![Value::Int(3)],
        explain_relation: "fx order items".into(),
        expect: IntrospectionExpect {
            explain_has_execution_time: analyzed,
            ..IntrospectionExpect::ALL
        },
    };
    run_introspection(
        &*seaquel_engine_mysql::engine(),
        &with_database(&config, &db),
        &spec,
    )
    .await;
}

#[tokio::test]
async fn mysql_introspection() {
    introspection(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_introspection() {
    introspection(Server::Mariadb).await;
}

// ── Statistics without a grant on `mysql` ────────────────────────────────────

/// A user who may read `seaquel_test` but not `mysql.innodb_index_stats`
/// still gets statistics, with no index usage.
async fn statistics_without_mysql_grant(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let admin = open(&config).await;
    let user = scratch_name("sq_stats_");
    let user = &user[..user.len().min(32)];
    let password = "Stats_pw_1";
    for sql in [
        format!("CREATE USER '{user}'@'%' IDENTIFIED BY '{password}'"),
        format!("GRANT SELECT ON seaquel_test.* TO '{user}'@'%'"),
    ] {
        admin.execute(&sql, vec![]).await.expect("create user");
    }

    let outcome = async {
        let driver = open(&with_user(&config, user, password)).await;
        let denied = driver
            .query("SELECT COUNT(*) FROM mysql.innodb_index_stats", vec![])
            .await;
        assert!(denied.is_err(), "the user must not read mysql.*");
        let stats = driver.statistics().await;
        driver.close().await.expect("close");
        stats
    }
    .await;

    admin
        .execute(&format!("DROP USER '{user}'@'%'"), vec![])
        .await
        .expect("drop user");
    admin.close().await.expect("close");

    let stats = outcome.expect("statistics without a grant on mysql");
    assert!(stats.index_usage.is_empty(), "{:?}", stats.index_usage);
    assert!(!stats.table_sizes.is_empty());
    assert_eq!(stats.overview.database_name, "seaquel_test");
}

#[tokio::test]
async fn mysql_statistics_without_mysql_grant() {
    statistics_without_mysql_grant(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_statistics_without_mysql_grant() {
    statistics_without_mysql_grant(Server::Mariadb).await;
}

// ── DDL bug fixes on the server ──────────────────────────────────────────────

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

/// Fixes 2, 6 and 7 produce SQL the server runs: a backticked name, a
/// dropped index and the primary key, and a default-only change.
async fn ddl_fixes_run(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let table = format!("{}`q", scratch_name("sq_ddl_"));
    let d = MysqlDialect;
    let from = CreateTableDefinition {
        table_name: table.clone(),
        schema_name: "seaquel_test".into(),
        columns: vec![
            CreateTableColumn {
                nullable: false,
                is_primary_key: true,
                ..column("c1", "id", "INT")
            },
            CreateTableColumn {
                length: Some("20".into()),
                default_value: "'old'".into(),
                ..column("c2", "st`atus", "VARCHAR")
            },
        ],
        indexes: vec![CreateTableIndex {
            id: "i1".into(),
            name: "status`idx".into(),
            columns: vec!["st`atus".into()],
            unique: false,
            ty: "btree".into(),
        }],
        foreign_keys: vec![],
    };
    let mut to = from.clone();
    to.columns[1].default_value = "'new'".into();
    to.indexes.clear();

    let quoted = d.quote_ident(&table);
    let outcome = async {
        for sql in d.create_table(&from).split(";\n\n") {
            driver.execute(sql.trim_end_matches(';'), vec![]).await?;
        }
        let alter = d.alter_table(&from, &to);
        assert!(alter.contains(" ON `seaquel_test`."), "{alter}");
        for sql in alter.split('\n') {
            driver.execute(sql.trim_end_matches(';'), vec![]).await?;
        }
        // Drop the primary key too.
        let mut no_pk = to.clone();
        let mut with_pk = to.clone();
        with_pk.indexes.push(CreateTableIndex {
            id: "i0".into(),
            name: "PRIMARY".into(),
            columns: vec!["id".into()],
            unique: true,
            ty: "btree".into(),
        });
        no_pk.indexes.clear();
        let drop_pk = d.alter_table(&with_pk, &no_pk);
        assert!(drop_pk.ends_with("DROP PRIMARY KEY;"), "{drop_pk}");
        driver
            .execute(drop_pk.trim_end_matches(';'), vec![])
            .await?;
        driver.table_metadata("seaquel_test", &table).await
    }
    .await;
    let _ = driver
        .execute(
            &format!("DROP TABLE IF EXISTS `seaquel_test`.{quoted}"),
            vec![],
        )
        .await;
    driver.close().await.expect("close");

    let (columns, indexes) = outcome.expect("DDL runs");
    assert_eq!(columns[1].name, "st`atus");
    assert_eq!(
        columns[1].default_value.as_deref(),
        Some("'new'"),
        "{columns:?}"
    );
    assert!(!columns[0].is_primary_key, "{columns:?}");
    assert!(indexes.is_empty(), "{indexes:?}");
}

#[tokio::test]
async fn mysql_ddl_fixes_run() {
    ddl_fixes_run(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_ddl_fixes_run() {
    ddl_fixes_run(Server::Mariadb).await;
}

// ── EXPLAIN by server ────────────────────────────────────────────────────────

/// MariaDB has no `EXPLAIN ANALYZE`; its driver analyzes with
/// `ANALYZE FORMAT=JSON` and reads rows and costs from MariaDB's JSON.
async fn explain_by_server(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let sql = "SELECT o.id FROM orders o JOIN users u ON u.id = o.user_id WHERE u.id > ?";
    let plan = driver
        .explain(sql, vec![Value::Int(0)], false)
        .await
        .expect("explain");
    let analyzed = driver
        .explain(sql, vec![Value::Int(0)], true)
        .await
        .expect("analyze");
    driver.close().await.expect("close");

    fn tables(n: &seaquel_types::ExplainPlanNode, out: &mut Vec<(String, Option<f64>)>) {
        if let Some(r) = &n.relation_name {
            out.push((r.clone(), n.plan_rows));
        }
        n.children.iter().for_each(|c| tables(c, out));
    }
    let mut scanned = Vec::new();
    tables(&plan.plan, &mut scanned);
    assert!(scanned.len() >= 2, "{plan:#?}");
    assert!(scanned.iter().all(|(_, rows)| rows.is_some()), "{plan:#?}");
    assert!(plan.plan.total_cost.is_some(), "{plan:#?}");
    assert!(analyzed.is_analyze);
    assert!(analyzed.plan.actual_loops.is_some(), "{analyzed:#?}");
    if server == Server::Mariadb {
        assert!(analyzed.execution_time.is_some(), "{analyzed:#?}");
    }
}

#[tokio::test]
async fn mysql_explain() {
    explain_by_server(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_explain() {
    explain_by_server(Server::Mariadb).await;
}

// ── Defaults as SQL (fixes 7 and 8) ──────────────────────────────────────────

/// The table editor's definition of an introspected table: every column's
/// type and default copied verbatim (`create-table-tabs.svelte.ts`).
fn editor_definition(schema: &str, table: &str, columns: &[SchemaColumn]) -> CreateTableDefinition {
    CreateTableDefinition {
        table_name: table.into(),
        schema_name: schema.into(),
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
        indexes: vec![],
        foreign_keys: vec![],
    }
}

async fn run_all(driver: &dyn Driver, sql: &str) -> Result<(), seaquel_engine::DbError> {
    // Statements end with `;` and a newline (`CREATE TABLE` spans lines).
    for stmt in sql.split(";\n").map(str::trim).filter(|l| !l.is_empty()) {
        driver.execute(stmt.trim_end_matches(';'), vec![]).await?;
    }
    Ok(())
}

/// `(COLUMN_DEFAULT, EXTRA, COLUMN_COMMENT)` of one column, as text.
async fn column_info(driver: &dyn Driver, table: &str, column: &str) -> (String, String, String) {
    let r = driver
        .query(
            "SELECT CAST(COALESCE(COLUMN_DEFAULT, '<null>') AS CHAR) AS d, CAST(EXTRA AS CHAR) AS e, \
             CAST(COLUMN_COMMENT AS CHAR) AS c FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = 'seaquel_test' AND TABLE_NAME = ? AND COLUMN_NAME = ?",
            vec![Value::from(table), Value::from(column)],
        )
        .await
        .expect("column info");
    let text = |v: &Value| match v {
        Value::Text(s) => s.clone(),
        other => format!("{other:?}"),
    };
    let row = &r.rows[0];
    (text(&row[0]), text(&row[1]), text(&row[2]))
}

/// Defaults are SQL expressions (fix 8), so the editor can feed them back
/// into DDL: a default-only edit is `SET DEFAULT` (fix 7), which keeps the
/// column's comment and `ON UPDATE`, and an introspected table re-created
/// from its own definition has the same defaults.
async fn defaults_round_trip(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let table = scratch_name("sq_def_");
    let copy = format!("{table}_copy");
    let d = MysqlDialect;

    let outcome = async {
        driver
            .execute(
                &format!(
                    "CREATE TABLE seaquel_test.{table} (\
                     id INT NOT NULL PRIMARY KEY, \
                     status VARCHAR(20) NOT NULL DEFAULT 'active' COMMENT 'x', \
                     touched TIMESTAMP(3) NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3), \
                     quoted VARCHAR(20) DEFAULT 'it''s a\\\\b', \
                     blank VARCHAR(5) DEFAULT '', \
                     amount DECIMAL(5,2) DEFAULT 1.50, \
                     flag BIT(1) DEFAULT b'1', \
                     doc JSON DEFAULT (JSON_ARRAY()))"
                ),
                vec![],
            )
            .await?;
        let (columns, _) = driver.table_metadata("seaquel_test", &table).await?;
        let defaults: Vec<_> = columns.iter().map(|c| c.default_value.clone()).collect();

        // The introspected definition, re-created under another name.
        let mut def = editor_definition("seaquel_test", &copy, &columns);
        run_all(&*driver, &d.create_table(&def)).await?;
        let (copied, _) = driver.table_metadata("seaquel_test", &copy).await?;
        let copied: Vec<_> = copied.iter().map(|c| c.default_value.clone()).collect();

        // A default-only edit of the original.
        def.table_name = table.clone();
        let mut to = def.clone();
        to.columns[1].default_value = "'inactive'".into();
        to.columns[2].default_value = "'2020-01-02 03:04:05.000'".into();
        let alter = d.alter_table(&def, &to);
        run_all(&*driver, &alter).await?;
        let status = column_info(&*driver, &table, "status").await;
        let touched = column_info(&*driver, &table, "touched").await;
        Ok::<_, seaquel_engine::DbError>((defaults, copied, alter, status, touched))
    }
    .await;
    for t in [&table, &copy] {
        let _ = driver
            .execute(&format!("DROP TABLE IF EXISTS seaquel_test.{t}"), vec![])
            .await;
    }
    driver.close().await.expect("close");

    let (defaults, copied, alter, status, touched) = outcome.expect("defaults round trip");
    let now = match server {
        Server::Mysql => "CURRENT_TIMESTAMP(3)",
        Server::Mariadb => "current_timestamp(3)",
    };
    let doc = match server {
        Server::Mysql => "(json_array())",
        Server::Mariadb => "json_array()",
    };
    let s = |v: &str| Some(v.to_string());
    assert_eq!(
        defaults,
        vec![
            None,
            s("'active'"),
            s(now),
            s(r"'it''s a\\b'"),
            s("''"),
            s("1.50"),
            s("b'1'"),
            s(doc)
        ],
    );
    assert_eq!(copied, defaults, "a re-created table has the same defaults");
    assert!(!alter.contains("MODIFY"), "{alter}");
    assert_eq!(status.2, "x", "the comment survives: {status:?}");
    assert!(status.0.contains("inactive"), "{status:?}");
    assert!(touched.0.contains("2020-01-02 03:04:05"), "{touched:?}");
    // MySQL keeps a TIMESTAMP's ON UPDATE through `SET DEFAULT`. MariaDB 11
    // drops it on `ALTER COLUMN … SET DEFAULT` (checked by hand: its own
    // `SHOW CREATE TABLE` loses the clause), and a `MODIFY` rebuilt from the
    // editor's definition would drop it too, so there's nothing to keep.
    let on_update = touched
        .1
        .to_ascii_lowercase()
        .contains("on update current_timestamp(3)");
    assert_eq!(
        on_update,
        server == Server::Mysql,
        "ON UPDATE after SET DEFAULT: {touched:?}"
    );
}

#[tokio::test]
async fn mysql_defaults_round_trip() {
    defaults_round_trip(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_defaults_round_trip() {
    defaults_round_trip(Server::Mariadb).await;
}

// ── EXPLAIN of window functions and HAVING ───────────────────────────────────

/// MariaDB wraps the scan in `window_functions_computation` under the
/// `filesort` of a HAVING query; the table must still be in the plan.
async fn window_explain(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let sql = "SELECT user_id, SUM(total), ROW_NUMBER() OVER (ORDER BY SUM(total)) \
               FROM orders GROUP BY user_id HAVING SUM(total) > 10";
    let plans = (
        driver.explain(sql, vec![], false).await,
        driver.explain(sql, vec![], true).await,
    );
    driver.close().await.expect("close");
    fn scans(n: &seaquel_types::ExplainPlanNode, relation: &str) -> bool {
        n.relation_name.as_deref() == Some(relation)
            || n.children.iter().any(|c| scans(c, relation))
    }
    for plan in [plans.0.expect("explain"), plans.1.expect("analyze")] {
        assert!(scans(&plan.plan, "orders"), "{plan:#?}");
    }
}

#[tokio::test]
async fn mysql_window_explain() {
    window_explain(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_window_explain() {
    window_explain(Server::Mariadb).await;
}

/// Task 18: checking UNIQUE in edit mode adds the constraint (on an existing
/// and an added column, names with backticks); unchecking it is a note,
/// since dropping needs the index name.
async fn unique_checkbox_runs(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let table = format!("{}`u", scratch_name("sq_uq_"));
    let d = MysqlDialect;
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
                ..column("c2", "e`mail", "VARCHAR")
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
            schema_name: "seaquel_test".into(),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
        }
    };
    let quoted = format!("`seaquel_test`.{}", d.quote_ident(&table));
    let run = |sql: String| {
        let driver = &driver;
        async move {
            for stmt in sql
                .split(";\n")
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with("--"))
            {
                driver.execute(stmt.trim_end_matches(';'), vec![]).await?;
            }
            Ok::<(), seaquel_engine::DbError>(())
        }
    };
    let unique_count = || async {
        let r = driver
            .query(
                "SELECT COUNT(DISTINCT INDEX_NAME) FROM information_schema.STATISTICS \
                 WHERE TABLE_SCHEMA = 'seaquel_test' AND TABLE_NAME = ? AND NON_UNIQUE = 0 AND INDEX_NAME <> 'PRIMARY'",
                vec![Value::Text(table.clone())],
            )
            .await
            .expect("count");
        r.rows[0][0].clone()
    };

    let outcome = async {
        run(d.create_table(&def(false, false))).await?;
        let add = d.alter_table(&def(false, false), &def(true, true));
        assert!(add.contains("ADD UNIQUE (`e``mail`)"), "{add}");
        run(add).await?;
        assert_eq!(unique_count().await, Value::Int(2));
        run(format!("INSERT INTO {quoted} VALUES (1, 'a', 1)")).await?;
        assert!(run(format!("INSERT INTO {quoted} VALUES (2, 'a', 2)"))
            .await
            .is_err());

        let drop = d.alter_table(&def(true, true), &def(false, true));
        assert!(drop.starts_with("-- ") && !drop.contains('\n'), "{drop}");
        Ok::<(), seaquel_engine::DbError>(())
    }
    .await;
    let _ = driver
        .execute(&format!("DROP TABLE IF EXISTS {quoted}"), vec![])
        .await;
    driver.close().await.expect("close");
    outcome.expect("UNIQUE checkbox DDL");
}

#[tokio::test]
async fn mysql_unique_checkbox_runs() {
    unique_checkbox_runs(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_unique_checkbox_runs() {
    unique_checkbox_runs(Server::Mariadb).await;
}

/// Task 18: introspection reports UNIQUE (a constraint and a plain unique
/// index) and the checkbox drops both.
async fn unique_checkbox_from_metadata(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let d = MysqlDialect;
    let table = format!("{}`u", scratch_name("sq_uqm_"));
    let quoted = format!("`seaquel_test`.{}", d.quote_ident(&table));
    let run = |sql: String| {
        let driver = &driver;
        async move {
            for stmt in sql
                .split(";\n")
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with("--"))
            {
                driver
                    .execute(stmt.trim_end_matches(';'), vec![])
                    .await
                    .map_err(|e| format!("{}: {stmt}", e.message))?;
            }
            Ok::<(), String>(())
        }
    };
    let setup = run(format!(
        "CREATE TABLE {quoted} (id INT PRIMARY KEY, `e``mail` VARCHAR(40) UNIQUE, code INT, note INT);\n\
         CREATE UNIQUE INDEX `code idx` ON {quoted} (code);\n"
    ))
    .await;
    if setup.is_ok() {
        seaquel_engine_testkit::run_unique_checkbox(
            &*driver,
            &d,
            "seaquel_test",
            &table,
            &["e`mail", "code"],
            "note",
            run,
        )
        .await;
    }
    let _ = driver
        .execute(&format!("DROP TABLE IF EXISTS {quoted}"), vec![])
        .await;
    driver.close().await.expect("close");
    setup.expect("setup");
}

#[tokio::test]
async fn mysql_unique_checkbox_from_metadata() {
    unique_checkbox_from_metadata(Server::Mysql).await;
}

#[tokio::test]
async fn mariadb_unique_checkbox_from_metadata() {
    unique_checkbox_from_metadata(Server::Mariadb).await;
}
