//! Behaviour every Seaquel engine must have.
//!
//! Engine crates call [`run_smoke`] from `tests/smoke.rs`. Engines that need a
//! server (Postgres, MySQL, MariaDB, MSSQL) read their connection from an
//! environment variable holding `ConnectConfig` JSON (see [`config_from_env`])
//! and skip when it's unset, so `cargo test` works without Docker. CI sets the
//! variables, runs the servers as service containers, and sets
//! [`REQUIRE_ENGINES`] so a missing variable fails instead of skipping.
//!
//! Engines whose introspection runs in Rust also call [`run_introspection`].
//!
//! This is the seed of the conformance suite planned for phase 1.

use futures::{FutureExt, StreamExt};
use seaquel_engine::{
    BatchStatement, CancellationToken, ConnectConfig, Driver, Engine, SchemaColumn, SchemaIndex,
    StreamBatch, Value,
};
use seaquel_types::{ExplainPlanNode, TableKind};
use std::panic::AssertUnwindSafe;

/// When set, [`config_from_env`] panics on a missing variable instead of
/// skipping the test.
pub const REQUIRE_ENGINES: &str = "SEAQUEL_TEST_REQUIRE_ENGINES";

/// How an engine's SQL differs in ways the smoke suite cares about.
pub struct SmokeSpec {
    /// Placeholder for the 1-based parameter `n`.
    pub placeholder: fn(usize) -> String,
    /// Whether `Driver::transaction` is implemented. Engines that don't must
    /// return `TRANSACTION_NOT_SUPPORTED`.
    pub supports_transactions: bool,
    /// Whether `Value::Array` parameters bind. Engines that do must return a
    /// bound integer array as the same `Value::Array` and reject a mixed one
    /// with `QUERY_ERROR`; engines that don't must fail with
    /// `QUERY_ERROR "array parameters are not supported"`.
    pub supports_array_params: bool,
}

impl SmokeSpec {
    /// MySQL, MariaDB, SQLite, DuckDB.
    pub const QUESTION_MARK: SmokeSpec = SmokeSpec {
        placeholder: |_| "?".to_string(),
        supports_transactions: true,
        supports_array_params: false,
    };
    /// Postgres.
    pub const DOLLAR: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("${n}"),
        supports_transactions: true,
        supports_array_params: true,
    };
    /// MSSQL. Its driver doesn't implement transactions yet.
    pub const AT_P: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("@P{n}"),
        supports_transactions: false,
        supports_array_params: false,
    };
}

/// Read a `ConnectConfig` from `var` (JSON). `None` when unset, so the caller
/// can skip, unless [`REQUIRE_ENGINES`] is set.
///
/// # Panics
///
/// If the variable is set but isn't valid `ConnectConfig` JSON, or if it's
/// unset while [`REQUIRE_ENGINES`] is set.
pub fn config_from_env(var: &str) -> Option<ConnectConfig> {
    match std::env::var(var) {
        Ok(raw) => Some(
            serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("{var} is not valid ConnectConfig JSON: {e}")),
        ),
        Err(_) if std::env::var_os(REQUIRE_ENGINES).is_some() => {
            panic!("{var} is not set, and {REQUIRE_ENGINES} requires every engine to run")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

/// 10,000 rows in one column `n` (0..=9999), using only SQL every engine
/// accepts. Larger than the sqlx drivers' 5,000-row batch, so it exercises
/// multi-batch streaming.
pub const TEN_THOUSAND_ROWS: &str = "SELECT a.d + 10 * b.d + 100 * c.d + 1000 * e.d AS n \
     FROM (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) a \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) b \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) c \
     CROSS JOIN (SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
           UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) e";

/// Open, query, write with parameters, run transactions and stream, against a
/// scratch table that is dropped afterwards even if an assertion fails.
pub async fn run_smoke(engine: &dyn Engine, config: &ConnectConfig, spec: &SmokeSpec) {
    assert_eq!(engine.id(), config.driver.as_str(), "engine id must match its driver");
    let driver = engine.open(config).await.expect("open");

    let table = format!("seaquel_smoke_{}", uuid::Uuid::new_v4().simple());
    let outcome = AssertUnwindSafe(smoke_body(&*driver, &table, spec))
        .catch_unwind()
        .await;
    let _ = driver.execute(&format!("DROP TABLE {table}"), vec![]).await;
    driver.close().await.expect("close");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

async fn smoke_body(driver: &dyn Driver, table: &str, spec: &SmokeSpec) {
    let p = spec.placeholder;

    // Plain query.
    let one = driver.query("SELECT 1 AS one", vec![]).await.expect("SELECT 1");
    assert_eq!(one.columns, vec!["one"]);
    assert_eq!(one.rows.len(), 1);
    assert_eq!(as_i64(&one.rows[0][0]), 1);

    // Integer parameters bind exactly: 2^53 + 1 doesn't survive an f64.
    let exact = driver
        .query(&format!("SELECT {} AS n", p(1)), vec![Value::Int(9_007_199_254_740_993)])
        .await
        .expect("SELECT exact integer");
    assert_eq!(as_i64(&exact.rows[0][0]), 9_007_199_254_740_993);

    // Engines with array binding round-trip an array parameter; the others
    // reject it cleanly.
    if spec.supports_array_params {
        let ints = Value::Array(vec![Value::Int(1), Value::Null, Value::Int(9_007_199_254_740_993)]);
        let array = driver
            .query(&format!("SELECT {} AS a", p(1)), vec![ints.clone()])
            .await
            .expect("array parameter");
        assert_eq!(array.rows[0][0], ints);
        let mixed = driver
            .query(
                &format!("SELECT {} AS a", p(1)),
                vec![Value::Array(vec![Value::Int(1), Value::from("x")])],
            )
            .await
            .expect_err("mixed array parameter");
        assert_eq!(mixed.code, "QUERY_ERROR");
    } else {
        let array = driver
            .query(&format!("SELECT {} AS a", p(1)), vec![Value::Array(vec![Value::Int(1)])])
            .await
            .expect_err("array parameter");
        assert_eq!(array.code, "QUERY_ERROR");
        assert!(array.message.contains("array parameters are not supported"), "{}", array.message);
    }

    // DDL and parameterised writes.
    driver
        .execute(
            &format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, label VARCHAR(50))"),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let inserted = driver
        .execute(
            &format!("INSERT INTO {table} (id, label) VALUES ({}, {})", p(1), p(2)),
            vec![Value::Int(1), Value::from("one")],
        )
        .await
        .expect("INSERT");
    assert_eq!(inserted.rows_affected, 1);

    let row = driver
        .query(
            &format!("SELECT id, label FROM {table} WHERE id = {}", p(1)),
            vec![Value::Int(1)],
        )
        .await
        .expect("SELECT by id");
    assert_eq!(row.columns, vec!["id", "label"]);
    assert_eq!(row.rows.len(), 1);
    assert_eq!(as_i64(&row.rows[0][0]), 1);
    assert_eq!(row.rows[0][1], Value::from("one"));

    // The same SQL with differently typed parameters. Drivers that cache
    // prepared statements by SQL text alone reuse the first call's parameter
    // types, so a float or text sent after an integer is reinterpreted (a
    // FLOAT8 1.5 read as INT8 is 4609434218613702656). Twenty rounds so the
    // pool hands back a connection that already prepared the statement.
    let select = format!("SELECT {} AS v", p(1));
    let update = format!("UPDATE {table} SET label = label WHERE id = {}", p(1));
    for round in 0..20 {
        let v = driver
            .query(&select, vec![Value::Int(7)])
            .await
            .expect("SELECT int");
        assert_eq!(as_i64(&v.rows[0][0]), 7, "round {round}: int after other types");
        let v = driver
            .query(&select, vec![Value::Float(1.5)])
            .await
            .expect("SELECT float");
        assert_eq!(v.rows[0][0], Value::Float(1.5), "round {round}: float after int");
        let v = driver
            .query(&select, vec![Value::from("x")])
            .await
            .expect("SELECT text");
        assert_eq!(v.rows[0][0], Value::from("x"), "round {round}: text after float");
        for id in [Value::Int(1), Value::Float(1.0)] {
            let updated = driver
                .execute(&update, vec![id.clone()])
                .await
                .unwrap_or_else(|e| panic!("UPDATE with {id:?}: {e:?}"));
            assert_eq!(updated.rows_affected, 1, "round {round}: UPDATE with {id:?}");
        }
    }

    // Transactions: all or nothing.
    let insert = |id: i64, label: &str| BatchStatement {
        sql: format!("INSERT INTO {table} (id, label) VALUES ({}, {})", p(1), p(2)),
        params: vec![Value::Int(id), Value::from(label)],
    };
    let ok = driver.transaction(vec![insert(2, "two"), insert(3, "three")]).await;
    if spec.supports_transactions {
        ok.expect("transaction");
        assert_eq!(count(driver, table).await, 3);
        let dup = driver.transaction(vec![insert(4, "four"), insert(1, "dup")]).await;
        assert!(dup.is_err(), "duplicate key must fail the transaction");
        assert_eq!(count(driver, table).await, 3, "failed transaction must roll back");
    } else {
        assert_eq!(ok.expect_err("transaction").code, "TRANSACTION_NOT_SUPPORTED");
    }

    // Streaming.
    let batches: Vec<StreamBatch> = driver
        .query_stream(TEN_THOUSAND_ROWS.to_string(), vec![], CancellationToken::new())
        .map(|b| b.expect("stream batch"))
        .collect()
        .await;
    assert!(!batches.is_empty());
    assert_eq!(batches[0].columns, Some(vec!["n".to_string()]));
    assert!(batches[1..].iter().all(|b| b.columns.is_none()));
    assert!(batches.last().unwrap().is_final);
    assert_eq!(batches.iter().filter(|b| b.is_final).count(), 1);
    assert_eq!(batches.iter().map(|b| b.rows.len()).sum::<usize>(), 10_000);
}

async fn count(driver: &dyn Driver, table: &str) -> i64 {
    let r = driver
        .query(&format!("SELECT COUNT(*) AS c FROM {table}"), vec![])
        .await
        .expect("COUNT");
    as_i64(&r.rows[0][0])
}

/// Engines decode integers differently (`Int`, or `Decimal`/`Text` digits
/// for NUMERIC/DECIMAL). Accept any of them.
fn as_i64(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("expected an integer, got {v:?}"))
}

// ── Introspection ────────────────────────────────────────────────────────────

/// A unique, lower-case identifier for scratch objects: `prefix` plus a
/// UUID's hex digits. Needs no quoting in any engine.
pub fn scratch_name(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4().simple())
}

/// What [`run_introspection`] builds and expects. The engine's test fills it
/// in, so the SQL and the expectations stay engine-specific.
pub struct IntrospectionSpec {
    /// The scratch schema `setup` creates and `teardown` drops.
    pub schema: String,
    /// Statements that build the scratch schema, run in order.
    pub setup: Vec<String>,
    /// Statements that drop it. They run after the checks, even when one
    /// fails. A failure is reported on stderr, not raised.
    pub teardown: Vec<String>,
    /// Statements run before `setup` that drop scratch schemas left behind by
    /// earlier runs killed before their teardown (e.g. every schema with this
    /// test's prefix). A failure is reported on stderr, not raised.
    ///
    /// Risk: these can't tell a crashed run's schema from one a concurrent run
    /// (another process against the same database) is still using, so two
    /// simultaneous runs against one database may break each other. Test
    /// databases only.
    pub stale_cleanup: Vec<String>,
    /// Every object `schema_tables` lists in `schema`, in its order.
    pub tables: Vec<(String, TableKind)>,
    /// `table_metadata(schema, table).0` for these tables.
    pub columns: Vec<(String, Vec<SchemaColumn>)>,
    /// `table_metadata(schema, table).1` for these tables.
    pub indexes: Vec<(String, Vec<SchemaIndex>)>,
    /// A table in `schema` that `statistics().table_sizes` must list with a
    /// non-zero size (Postgres bug fix 5 uses a name that needs quoting).
    pub stats_table: String,
    /// An index in `schema` that `statistics().index_usage` must list.
    pub usage_index: String,
    /// A query for `explain`, its parameters, and a relation its plan scans.
    pub explain_sql: String,
    pub explain_params: Vec<Value>,
    pub explain_relation: String,
}

/// Build the scratch schema, check `list_schemas`, `schema_tables`,
/// `table_metadata`, `statistics` and `explain` against `spec`, then drop the
/// schema, even if a check (or the setup) fails.
pub async fn run_introspection(engine: &dyn Engine, config: &ConnectConfig, spec: &IntrospectionSpec) {
    let driver = engine.open(config).await.expect("open");
    run_best_effort(&*driver, "stale cleanup", &spec.stale_cleanup).await;
    let outcome = AssertUnwindSafe(introspection_body(&*driver, spec))
        .catch_unwind()
        .await;
    run_best_effort(&*driver, "teardown", &spec.teardown).await;
    driver.close().await.expect("close");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Run each statement, reporting failures on stderr instead of panicking.
async fn run_best_effort(driver: &dyn Driver, what: &str, statements: &[String]) {
    for sql in statements {
        if let Err(e) = driver.execute(sql, vec![]).await {
            eprintln!("{what} failed: {e:?}\n{sql}");
        }
    }
}

async fn introspection_body(driver: &dyn Driver, spec: &IntrospectionSpec) {
    for sql in &spec.setup {
        driver
            .execute(sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("setup failed: {e:?}\n{sql}"));
    }
    let schema = spec.schema.as_str();

    let schemas = driver.list_schemas().await.expect("list_schemas");
    assert!(schemas.iter().any(|s| s == schema), "{schema} not in {schemas:?}");

    let tables: Vec<(String, TableKind)> = driver
        .schema_tables()
        .await
        .expect("schema_tables")
        .into_iter()
        .filter(|t| t.schema == schema)
        .map(|t| (t.name, t.kind))
        .collect();
    assert_eq!(tables, spec.tables, "schema_tables in {schema}");

    // Every metadata mismatch at once.
    let mut failures = Vec::new();
    for (table, expected) in &spec.columns {
        match driver.table_metadata(schema, table).await {
            Ok((actual, _)) if &actual == expected => {}
            Ok((actual, _)) => failures.push(format!(
                "columns of {table:?}\n  expected: {expected:#?}\n  actual:   {actual:#?}"
            )),
            Err(e) => failures.push(format!("columns of {table:?}: {e:?}")),
        }
    }
    for (table, expected) in &spec.indexes {
        match driver.table_metadata(schema, table).await {
            Ok((_, actual)) if &actual == expected => {}
            Ok((_, actual)) => failures.push(format!(
                "indexes of {table:?}\n  expected: {expected:#?}\n  actual:   {actual:#?}"
            )),
            Err(e) => failures.push(format!("indexes of {table:?}: {e:?}")),
        }
    }
    assert!(failures.is_empty(), "table_metadata:\n\n{}", failures.join("\n\n"));

    let stats = driver.statistics().await.expect("statistics");
    let size = stats
        .table_sizes
        .iter()
        .find(|t| t.schema == schema && t.name == spec.stats_table)
        .unwrap_or_else(|| panic!("{:?} missing from table sizes", spec.stats_table));
    assert!(size.total_size_bytes > 0, "{size:?}");
    assert!(size.data_size.is_some() && size.index_size.is_some(), "{size:?}");
    let usage = stats
        .index_usage
        .iter()
        .find(|i| i.schema == schema && i.index_name == spec.usage_index)
        .unwrap_or_else(|| panic!("{:?} missing from index usage", spec.usage_index));
    assert!(!usage.size.is_empty() && usage.rows_read.is_some(), "{usage:?}");
    let o = &stats.overview;
    assert!(!o.database_name.is_empty(), "{o:?}");
    assert!(o.total_size_bytes.is_some_and(|b| b > 0), "{o:?}");
    assert!(o.connection_count.is_some_and(|c| c >= 1), "{o:?}");
    assert!(o.table_count > 0 && o.index_count > 0, "{o:?}");

    let plan = driver
        .explain(&spec.explain_sql, spec.explain_params.clone(), false)
        .await
        .expect("explain");
    assert!(!plan.is_analyze);
    assert_eq!(plan.execution_time, None);
    assert!(plan.plan.total_cost.is_some(), "{plan:?}");
    assert!(plan.plan.actual_loops.is_none(), "{plan:?}");
    assert!(scans(&plan.plan, &spec.explain_relation), "{plan:#?}");

    let analyzed = driver
        .explain(&spec.explain_sql, spec.explain_params.clone(), true)
        .await
        .expect("explain analyze");
    assert!(analyzed.is_analyze);
    assert!(analyzed.execution_time.is_some(), "{analyzed:?}");
    assert!(analyzed.plan.actual_loops.is_some(), "{analyzed:?}");
    assert!(scans(&analyzed.plan, &spec.explain_relation), "{analyzed:#?}");
}

/// Whether `node` or a descendant reads `relation`.
fn scans(node: &ExplainPlanNode, relation: &str) -> bool {
    node.relation_name.as_deref() == Some(relation) || node.children.iter().any(|c| scans(c, relation))
}
