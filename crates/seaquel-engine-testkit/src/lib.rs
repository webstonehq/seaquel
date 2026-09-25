//! Behaviour every Seaquel engine must have.
//!
//! Engine crates call [`run_smoke`] from `tests/smoke.rs`. Engines that need a
//! server (Postgres, MySQL, MariaDB, MSSQL) read their connection from an
//! environment variable holding `ConnectConfig` JSON (see [`config_from_env`])
//! and skip when it's unset, so `cargo test` works without Docker. CI sets the
//! variables, runs the servers as service containers, and sets
//! [`REQUIRE_ENGINES`] so a missing variable fails instead of skipping.
//!
//! This is the seed of the conformance suite planned for phase 1.

use futures::{FutureExt, StreamExt};
use seaquel_engine::{
    BatchStatement, CancellationToken, ConnectConfig, Driver, Engine, StreamBatch,
};
use serde_json::{json, Value};
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
}

impl SmokeSpec {
    /// MySQL, MariaDB, SQLite, DuckDB.
    pub const QUESTION_MARK: SmokeSpec = SmokeSpec {
        placeholder: |_| "?".to_string(),
        supports_transactions: true,
    };
    /// Postgres.
    pub const DOLLAR: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("${n}"),
        supports_transactions: true,
    };
    /// MSSQL. Its driver doesn't implement transactions yet.
    pub const AT_P: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("@P{n}"),
        supports_transactions: false,
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
            vec![json!(1), json!("one")],
        )
        .await
        .expect("INSERT");
    assert_eq!(inserted.rows_affected, 1);

    let row = driver
        .query(
            &format!("SELECT id, label FROM {table} WHERE id = {}", p(1)),
            vec![json!(1)],
        )
        .await
        .expect("SELECT by id");
    assert_eq!(row.columns, vec!["id", "label"]);
    assert_eq!(row.rows.len(), 1);
    assert_eq!(as_i64(&row.rows[0][0]), 1);
    assert_eq!(row.rows[0][1], json!("one"));

    // Transactions: all or nothing.
    let insert = |id: i64, label: &str| BatchStatement {
        sql: format!("INSERT INTO {table} (id, label) VALUES ({}, {})", p(1), p(2)),
        params: vec![json!(id), json!(label)],
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

/// Engines decode integers differently (JSON number, or a string for
/// NUMERIC/DECIMAL). Accept either.
fn as_i64(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("expected an integer, got {v}"))
}
