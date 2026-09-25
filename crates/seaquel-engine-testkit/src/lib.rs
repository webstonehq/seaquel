//! Behaviour every Seaquel engine must have.
//!
//! Engine crates call [`run_smoke`] from `tests/smoke.rs`. Engines that need a
//! server (Postgres, MySQL, MariaDB, MSSQL) read their connection from an
//! environment variable holding `ConnectConfig` JSON (see [`config_from_env`])
//! and skip when it's unset, so `cargo test` works without Docker. CI sets the
//! variables, runs the servers as service containers, and sets
//! [`REQUIRE_ENGINES`] so a missing variable fails instead of skipping.
//!
//! Engines whose introspection runs in Rust also call [`run_introspection`],
//! with [`IntrospectionExpect`] saying what they can report, and engines with
//! native values check them with [`run_typed_cells`].
//!
//! This is the seed of the conformance suite planned for phase 1.

use futures::{FutureExt, StreamExt};
use seaquel_engine::{
    BatchStatement, CancellationToken, ConnectConfig, Driver, Engine, ExpectRows, SchemaColumn,
    SchemaIndex, StreamBatch, Value,
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
    /// MSSQL.
    pub const AT_P: SmokeSpec = SmokeSpec {
        placeholder: |n| format!("@P{n}"),
        supports_transactions: true,
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
    assert_eq!(
        engine.id(),
        config.driver.as_str(),
        "engine id must match its driver"
    );
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
    let one = driver
        .query("SELECT 1 AS one", vec![])
        .await
        .expect("SELECT 1");
    assert_eq!(one.columns, vec!["one"]);
    assert_eq!(one.rows.len(), 1);
    assert_eq!(as_i64(&one.rows[0][0]), 1);

    // Integer parameters bind exactly: 2^53 + 1 doesn't survive an f64.
    let exact = driver
        .query(
            &format!("SELECT {} AS n", p(1)),
            vec![Value::Int(9_007_199_254_740_993)],
        )
        .await
        .expect("SELECT exact integer");
    assert_eq!(as_i64(&exact.rows[0][0]), 9_007_199_254_740_993);

    // Engines with array binding round-trip an array parameter; the others
    // reject it cleanly.
    if spec.supports_array_params {
        let ints = Value::Array(vec![
            Value::Int(1),
            Value::Null,
            Value::Int(9_007_199_254_740_993),
        ]);
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
            .query(
                &format!("SELECT {} AS a", p(1)),
                vec![Value::Array(vec![Value::Int(1)])],
            )
            .await
            .expect_err("array parameter");
        assert_eq!(array.code, "QUERY_ERROR");
        assert!(
            array.message.contains("array parameters are not supported"),
            "{}",
            array.message
        );
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
            &format!(
                "INSERT INTO {table} (id, label) VALUES ({}, {})",
                p(1),
                p(2)
            ),
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
        assert_eq!(
            as_i64(&v.rows[0][0]),
            7,
            "round {round}: int after other types"
        );
        let v = driver
            .query(&select, vec![Value::Float(1.5)])
            .await
            .expect("SELECT float");
        assert_eq!(
            v.rows[0][0],
            Value::Float(1.5),
            "round {round}: float after int"
        );
        let v = driver
            .query(&select, vec![Value::from("x")])
            .await
            .expect("SELECT text");
        assert_eq!(
            v.rows[0][0],
            Value::from("x"),
            "round {round}: text after float"
        );
        for id in [Value::Int(1), Value::Float(1.0)] {
            let updated = driver
                .execute(&update, vec![id.clone()])
                .await
                .unwrap_or_else(|e| panic!("UPDATE with {id:?}: {e:?}"));
            assert_eq!(
                updated.rows_affected, 1,
                "round {round}: UPDATE with {id:?}"
            );
        }
    }

    // Transactions: all or nothing.
    let insert = |id: i64, label: &str| BatchStatement {
        sql: format!(
            "INSERT INTO {table} (id, label) VALUES ({}, {})",
            p(1),
            p(2)
        ),
        params: vec![Value::Int(id), Value::from(label)],
        expect_rows: None,
    };
    let ok = driver
        .transaction(vec![insert(2, "two"), insert(3, "three")])
        .await;
    if spec.supports_transactions {
        ok.expect("transaction");
        assert_eq!(count(driver, table).await, 3);
        let dup = driver
            .transaction(vec![insert(4, "four"), insert(1, "dup")])
            .await;
        assert!(dup.is_err(), "duplicate key must fail the transaction");
        assert_eq!(
            count(driver, table).await,
            3,
            "failed transaction must roll back"
        );
        run_expect_rows(driver, table, p).await;
    } else {
        assert_eq!(
            ok.expect_err("transaction").code,
            "TRANSACTION_NOT_SUPPORTED"
        );
    }

    // Streaming.
    let batches: Vec<StreamBatch> = driver
        .query_stream(
            TEN_THOUSAND_ROWS.to_string(),
            vec![],
            CancellationToken::new(),
        )
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

/// `BatchStatement::expect_rows`: a keyed statement that matches no row
/// rolls the whole batch back with `NO_ROWS_AFFECTED` and its index, and the
/// connection stays usable. A keyed UPDATE that sets a value it already has
/// still counts its row (MySQL must count matched rows, not changed ones).
/// Expects rows 1, 2 and 3, labelled "one", "two" and "three", and leaves
/// them so.
async fn run_expect_rows(driver: &dyn Driver, table: &str, p: fn(usize) -> String) {
    let keyed = |sql: String, params: Vec<Value>| BatchStatement {
        sql,
        params,
        expect_rows: Some(ExpectRows { min: 1 }),
    };
    let set_label = |id: i64, label: &str| {
        keyed(
            format!("UPDATE {table} SET label = {} WHERE id = {}", p(1), p(2)),
            vec![Value::from(label), Value::Int(id)],
        )
    };
    let delete = |id: i64| {
        keyed(
            format!("DELETE FROM {table} WHERE id = {}", p(1)),
            vec![Value::Int(id)],
        )
    };
    let label_of = |id: i64| async move {
        let r = driver
            .query(
                &format!("SELECT label FROM {table} WHERE id = {}", p(1)),
                vec![Value::Int(id)],
            )
            .await
            .expect("SELECT label");
        r.rows[0][0].clone()
    };

    for stale in [set_label(99, "none"), delete(99)] {
        let err = driver
            .transaction(vec![set_label(2, "TWO"), stale])
            .await
            .expect_err("a keyed statement matching no row must fail the transaction");
        assert_eq!(err.code, "NO_ROWS_AFFECTED", "{}", err.message);
        assert!(err.message.contains("(index 1)"), "{}", err.message);
        // Rolled back, and the connection still works.
        assert_eq!(
            label_of(2).await,
            Value::from("two"),
            "statement 0 must roll back"
        );
        assert_eq!(count(driver, table).await, 3);
    }

    // Same-value UPDATE, keyed DELETE, and an INSERT without `expect_rows`.
    driver
        .transaction(vec![
            set_label(2, "two"),
            delete(3),
            BatchStatement {
                sql: format!(
                    "INSERT INTO {table} (id, label) VALUES ({}, {})",
                    p(1),
                    p(2)
                ),
                params: vec![Value::Int(3), Value::from("three")],
                expect_rows: None,
            },
        ])
        .await
        .expect("a same-value keyed UPDATE affects its row");
    assert_eq!(count(driver, table).await, 3);
    assert_eq!(
        driver
            .execute(
                &format!("UPDATE {table} SET label = {} WHERE id = {}", p(1), p(2)),
                vec![Value::from("two"), Value::Int(2)],
            )
            .await
            .expect("same-value UPDATE")
            .rows_affected,
        1,
        "a same-value UPDATE must report its matched row"
    );
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
    v.as_i64()
        .unwrap_or_else(|| panic!("expected an integer, got {v:?}"))
}

// ── Typed cells ──────────────────────────────────────────────────────────────

/// One value [`run_typed_cells`] decodes and, optionally, binds back.
#[derive(Debug, Clone)]
pub struct TypedCellCase {
    /// How failures name this case.
    pub name: String,
    /// Statements run before `select`, e.g. `CREATE TABLE` + `INSERT`.
    pub setup: Vec<String>,
    /// Statements that undo `setup`. They run after the case, even when it
    /// fails. A failure is reported on stderr, not raised.
    pub teardown: Vec<String>,
    /// A query whose first cell (first row, first column) is checked.
    pub select: String,
    /// The SQL literal [`TypedCellCase::literal`] built `select` from, which
    /// [`TypedCellCase::bind_back_eq`] compares against. `None` for cases
    /// built by hand.
    pub literal: Option<String>,
    /// What that cell must decode to. Float NaN equals NaN (see [`same_value`]).
    pub expected: Value,
    /// A query taking the decoded cell as its only parameter (in the engine's
    /// placeholder syntax) whose first cell must be true: `Bool(true)`, or
    /// `Int(1)` for engines without a boolean type. Nothing else counts. `None` for types the
    /// engine can't compare with `=`.
    pub bind_back: Option<String>,
}

impl TypedCellCase {
    /// `SELECT {literal} AS v`, named after the literal, with no setup and no
    /// bind-back check.
    pub fn literal(literal: &str, expected: Value) -> Self {
        TypedCellCase {
            name: literal.to_string(),
            setup: vec![],
            teardown: vec![],
            select: format!("SELECT {literal} AS v"),
            literal: Some(literal.to_string()),
            expected,
            bind_back: None,
        }
    }

    /// Rename the case.
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Check binding back with `sql`.
    pub fn bind_back(mut self, sql: &str) -> Self {
        self.bind_back = Some(sql.to_string());
        self
    }

    /// Check binding back with `SELECT {p1} = {literal}`, where `literal` is
    /// the one [`TypedCellCase::literal`] was built from (not the name, so
    /// [`TypedCellCase::named`] may come before or after) and `p1` the
    /// engine's first placeholder.
    ///
    /// # Panics
    ///
    /// If the case has no `literal`.
    pub fn bind_back_eq(self, p1: &str) -> Self {
        let literal = self
            .literal
            .as_deref()
            .unwrap_or_else(|| panic!("{:?}: bind_back_eq needs a literal case", self.name));
        let sql = format!("SELECT {p1} = {literal} AS eq");
        self.bind_back(&sql)
    }
}

/// `==`, except that float NaN equals NaN (also inside arrays), so a case can
/// expect `Value::Float(f64::NAN)`.
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Float(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same_value(x, y))
        }
        _ => a == b,
    }
}

/// Run every case: `setup`, decode the `select` cell, compare it with
/// `expected`, bind it back if asked, then `teardown`. Every failing case is
/// reported at once, in one panic, after all cases ran.
///
/// Drivers are connection pools: `setup`, `select`, `bind_back` and
/// `teardown` may each run on a different connection. Setup must create
/// persistent, uniquely named objects (e.g. a [`scratch_name`] table), never
/// `TEMPORARY` tables or other session state.
pub async fn run_typed_cells(engine: &dyn Engine, config: &ConnectConfig, cases: &[TypedCellCase]) {
    let driver = engine.open(config).await.expect("open");
    let mut failures = Vec::new();
    for case in cases {
        let outcome = AssertUnwindSafe(typed_cell(&*driver, case))
            .catch_unwind()
            .await;
        run_best_effort(
            &*driver,
            &format!("teardown of {:?}", case.name),
            &case.teardown,
        )
        .await;
        let problem = match outcome {
            Ok(problem) => problem,
            Err(panic) => Some(format!("panicked: {}", panic_message(&panic))),
        };
        if let Some(problem) = problem {
            failures.push(format!("{} ({}): {problem}", case.name, case.select));
        }
    }
    driver.close().await.expect("close");
    assert!(
        failures.is_empty(),
        "{} of {} typed cells failed:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// What's wrong with `case`, if anything.
async fn typed_cell(driver: &dyn Driver, case: &TypedCellCase) -> Option<String> {
    for sql in &case.setup {
        if let Err(e) = driver.execute(sql, vec![]).await {
            return Some(format!("setup failed: {e:?}\n  {sql}"));
        }
    }
    let actual = match driver.query(&case.select, vec![]).await {
        Ok(r) => match r.rows.first().and_then(|row| row.first()) {
            Some(cell) => cell.clone(),
            None => return Some("select returned no cell".into()),
        },
        Err(e) => return Some(format!("select failed: {e:?}")),
    };
    if !same_value(&actual, &case.expected) {
        return Some(format!(
            "decoded wrong\n  expected: {:?}\n  actual:   {actual:?}",
            case.expected
        ));
    }
    let sql = case.bind_back.as_ref()?;
    match driver.query(sql, vec![actual.clone()]).await {
        Ok(r) => match r.rows.first().and_then(|row| row.first()) {
            Some(Value::Bool(true) | Value::Int(1)) => None,
            other => Some(format!(
                "bind back of {actual:?} wasn't true: {other:?}\n  {sql}"
            )),
        },
        Err(e) => Some(format!("bind back of {actual:?} failed: {e:?}\n  {sql}")),
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "(non-string panic)".into())
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
    /// A table in `schema` that `statistics().table_sizes` must list
    /// (Postgres bug fix 5 uses a name that needs quoting).
    pub stats_table: String,
    /// An index in `schema` that `statistics().index_usage` must list when
    /// `expect.stats_has_index_usage`.
    pub usage_index: String,
    /// A query for `explain`, its parameters, and a relation its plan scans.
    pub explain_sql: String,
    pub explain_params: Vec<Value>,
    pub explain_relation: String,
    /// What this engine's introspection can report.
    pub expect: IntrospectionExpect,
}

/// Which parts of [`run_introspection`]'s checks an engine can meet. A `false`
/// flag skips that check; it doesn't assert the opposite. Checks without a
/// flag apply to every engine: `list_schemas` contains `schema`,
/// `schema_tables` and `table_metadata` match, and a plain
/// `explain` isn't analyzed and has no execution time or actuals, and every
/// plan scans `explain_relation`. With `supports_statistics`, `stats_table` is
/// listed and the overview has a database name and non-zero table and index
/// counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntrospectionExpect {
    /// The engine has real schemas that `setup` can create and `teardown` can
    /// drop, so `schema_tables` in `schema` must equal `tables` exactly. When
    /// `false` (SQLite's `main`, one per database), `schema` is the engine's
    /// fixed schema and may hold other objects: only the listed tables named
    /// in `tables` are compared, in order.
    pub supports_schemas: bool,
    /// `statistics()` is implemented. When `false` it must fail with
    /// `NOT_SUPPORTED`, and the `stats_*` and `overview_*` flags are ignored.
    pub supports_statistics: bool,
    /// `stats_table` has a non-zero total size and data and index sizes.
    pub stats_has_table_sizes: bool,
    /// The overview has a non-zero total size.
    pub overview_has_size: bool,
    /// `statistics().index_usage` lists `usage_index` with a size.
    pub stats_has_index_usage: bool,
    /// That `usage_index` entry has `rows_read`.
    pub stats_has_index_rows_read: bool,
    /// The overview has a connection count of at least 1.
    pub stats_has_connection_count: bool,
    /// A plain `explain` reports a total cost at the root.
    pub explain_has_cost: bool,
    /// `explain(.., analyze = true)` runs the query and reports
    /// `is_analyze`. When `false`, the analyze checks below are skipped.
    pub explain_supports_analyze: bool,
    /// An analyzed plan has an execution time.
    pub explain_has_execution_time: bool,
    /// An analyzed plan has actual loops at the root.
    pub explain_has_actuals: bool,
}

impl IntrospectionExpect {
    /// Everything: what Postgres reports.
    pub const ALL: IntrospectionExpect = IntrospectionExpect {
        supports_schemas: true,
        supports_statistics: true,
        stats_has_table_sizes: true,
        overview_has_size: true,
        stats_has_index_usage: true,
        stats_has_index_rows_read: true,
        stats_has_connection_count: true,
        explain_has_cost: true,
        explain_supports_analyze: true,
        explain_has_execution_time: true,
        explain_has_actuals: true,
    };
}

/// Build the scratch schema, check `list_schemas`, `schema_tables`,
/// `table_metadata`, `statistics` and `explain` against `spec`, then drop the
/// schema, even if a check (or the setup) fails.
pub async fn run_introspection(
    engine: &dyn Engine,
    config: &ConnectConfig,
    spec: &IntrospectionSpec,
) {
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
    assert!(
        schemas.iter().any(|s| s == schema),
        "{schema} not in {schemas:?}"
    );

    let expect = &spec.expect;
    let tables: Vec<(String, TableKind)> = driver
        .schema_tables()
        .await
        .expect("schema_tables")
        .into_iter()
        .filter(|t| t.schema == schema)
        .filter(|t| expect.supports_schemas || spec.tables.iter().any(|(name, _)| *name == t.name))
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
    assert!(
        failures.is_empty(),
        "table_metadata:\n\n{}",
        failures.join("\n\n")
    );

    if expect.supports_statistics {
        check_statistics(driver, spec).await;
    } else {
        let err = driver
            .statistics()
            .await
            .expect_err("statistics must be unsupported");
        assert_eq!(err.code, "NOT_SUPPORTED", "{err:?}");
    }

    let plan = driver
        .explain(&spec.explain_sql, spec.explain_params.clone(), false)
        .await
        .expect("explain");
    assert!(!plan.is_analyze);
    assert_eq!(plan.execution_time, None);
    if expect.explain_has_cost {
        assert!(plan.plan.total_cost.is_some(), "{plan:?}");
    }
    assert!(plan.plan.actual_loops.is_none(), "{plan:?}");
    assert!(scans(&plan.plan, &spec.explain_relation), "{plan:#?}");

    if !expect.explain_supports_analyze {
        return;
    }
    let analyzed = driver
        .explain(&spec.explain_sql, spec.explain_params.clone(), true)
        .await
        .expect("explain analyze");
    assert!(analyzed.is_analyze);
    if expect.explain_has_execution_time {
        assert!(analyzed.execution_time.is_some(), "{analyzed:?}");
    }
    if expect.explain_has_actuals {
        assert!(analyzed.plan.actual_loops.is_some(), "{analyzed:?}");
    }
    assert!(
        scans(&analyzed.plan, &spec.explain_relation),
        "{analyzed:#?}"
    );
}

async fn check_statistics(driver: &dyn Driver, spec: &IntrospectionSpec) {
    let (schema, expect) = (spec.schema.as_str(), &spec.expect);
    let stats = driver.statistics().await.expect("statistics");
    let size = stats
        .table_sizes
        .iter()
        .find(|t| t.schema == schema && t.name == spec.stats_table)
        .unwrap_or_else(|| panic!("{:?} missing from table sizes", spec.stats_table));
    if expect.stats_has_table_sizes {
        assert!(size.total_size_bytes > 0, "{size:?}");
        assert!(
            size.data_size.is_some() && size.index_size.is_some(),
            "{size:?}"
        );
    }
    if expect.stats_has_index_usage {
        let usage = stats
            .index_usage
            .iter()
            .find(|i| i.schema == schema && i.index_name == spec.usage_index)
            .unwrap_or_else(|| panic!("{:?} missing from index usage", spec.usage_index));
        assert!(!usage.size.is_empty(), "{usage:?}");
        if expect.stats_has_index_rows_read {
            assert!(usage.rows_read.is_some(), "{usage:?}");
        }
    }
    let o = &stats.overview;
    assert!(!o.database_name.is_empty(), "{o:?}");
    if expect.overview_has_size {
        assert!(o.total_size_bytes.is_some_and(|b| b > 0), "{o:?}");
    }
    if expect.stats_has_connection_count {
        assert!(o.connection_count.is_some_and(|c| c >= 1), "{o:?}");
    }
    assert!(o.table_count > 0 && o.index_count > 0, "{o:?}");
}

/// Whether `node` or a descendant reads `relation`.
fn scans(node: &ExplainPlanNode, relation: &str) -> bool {
    node.relation_name.as_deref() == Some(relation)
        || node.children.iter().any(|c| scans(c, relation))
}

/// The table editor's definition of an introspected table, as
/// `addFromTable` builds it for the UNIQUE checkbox tests (Task 18): the
/// type kept whole, the UNIQUE flags and the indexes copied.
pub fn editor_definition(
    schema: &str,
    table: &str,
    columns: &[seaquel_types::SchemaColumn],
    indexes: &[seaquel_types::SchemaIndex],
) -> seaquel_types::CreateTableDefinition {
    seaquel_types::CreateTableDefinition {
        table_name: table.into(),
        schema_name: schema.into(),
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, c)| seaquel_types::CreateTableColumn {
                id: format!("c{i}"),
                name: c.name.clone(),
                ty: c.ty.clone(),
                length: None,
                precision: None,
                nullable: c.nullable,
                default_value: c.default_value.clone().unwrap_or_default(),
                is_primary_key: c.is_primary_key,
                is_unique: c.is_unique,
                collation: c.collation.clone(),
                in_unique_constraint: c.in_unique_constraint,
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

/// Task 18, the UNIQUE checkbox on an introspected table: `unique` columns
/// (one a UNIQUE constraint, one a plain unique index, set up by the
/// caller) show as checked and `plain` doesn't; re-ticking emits nothing;
/// unchecking runs and leaves no UNIQUE. `run` runs a script the way the
/// table editor does.
pub async fn run_unique_checkbox<F, Fut>(
    driver: &dyn Driver,
    dialect: &dyn seaquel_engine::Dialect,
    schema: &str,
    table: &str,
    unique: &[&str],
    plain: &str,
    run: F,
) where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (columns, indexes) = driver
        .table_metadata(schema, table)
        .await
        .expect("metadata");
    let flags = |columns: &[seaquel_types::SchemaColumn], name: &str| {
        let c = columns.iter().find(|c| c.name == name).expect(name);
        (c.is_unique, c.in_unique_constraint)
    };
    for name in unique {
        assert_eq!(
            flags(&columns, name),
            (true, true),
            "{name} is UNIQUE: {columns:?}"
        );
    }
    assert_eq!(flags(&columns, plain), (false, false), "{plain}");
    let pk = columns.iter().find(|c| c.is_primary_key).expect("pk");
    assert!(!pk.is_unique && !pk.in_unique_constraint, "{pk:?}");

    let from = editor_definition(schema, table, &columns, &indexes);
    assert_eq!(
        dialect.alter_table(&from, &from.clone()),
        "-- No changes detected"
    );

    let mut to = from.clone();
    for c in &mut to.columns {
        c.is_unique = false;
    }
    let sql = dialect.alter_table(&from, &to);
    assert!(!sql.contains("-- "), "{sql}");
    run(sql.clone()).await.expect(&sql);
    let (columns, _) = driver
        .table_metadata(schema, table)
        .await
        .expect("metadata");
    for name in unique {
        assert_eq!(flags(&columns, name), (false, false), "{name} after {sql}");
    }
}
