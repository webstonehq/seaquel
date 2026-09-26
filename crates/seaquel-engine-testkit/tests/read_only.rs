//! `run_read_only` against a fake engine: a driver that meets the contract
//! passes a two-attack spec, and drivers that break it are caught.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use seaquel_engine::{ConnectConfig, DbError, Driver, Engine, ExecuteResult, QueryResult, Value};
use seaquel_engine_testkit::{run_read_only, Attack, Check, ReadOnlySpec};
use std::panic::AssertUnwindSafe;

/// How the fake's `query_read_only` breaks the contract, if at all.
#[derive(Clone, Copy, PartialEq)]
enum Flaw {
    None,
    /// Runs writes instead of refusing them.
    Writes,
    /// Leaves the "pool" read-only after a refused write, so a normal
    /// `execute` fails afterwards.
    LeaksSession,
    /// The slow query keeps the connection busy after its future is dropped,
    /// like a query that isn't interrupted.
    IgnoresCancel,
    /// Refuses a write with the wrong code.
    WrongCode,
}

/// One table `t` of integers, plus session state a flawed driver leaks.
struct FakeDriver {
    flaw: Flaw,
    rows: Mutex<Vec<i64>>,
    pool_read_only: AtomicBool,
    /// State left on the read-only path's own connection.
    path_dirty: AtomicBool,
    /// Set while the slow query "runs" on the server.
    busy: Arc<AtomicBool>,
}

fn int_rows(rows: &[i64]) -> Vec<Vec<Value>> {
    rows.iter().map(|n| vec![Value::Int(*n)]).collect()
}

impl FakeDriver {
    /// `INSERT INTO t VALUES (n)` / `DELETE FROM t WHERE n = n`.
    fn write(&self, sql: &str) -> Result<u64, DbError> {
        let n = |prefix: &str| -> i64 {
            sql.trim_start_matches(prefix)
                .trim_end_matches(')')
                .trim()
                .parse()
                .unwrap()
        };
        let mut rows = self.rows.lock().unwrap();
        if sql.starts_with("INSERT INTO t VALUES (") {
            rows.push(n("INSERT INTO t VALUES ("));
            Ok(1)
        } else if sql.starts_with("DELETE FROM t WHERE n = ") {
            let n = n("DELETE FROM t WHERE n = ");
            let before = rows.len();
            rows.retain(|r| *r != n);
            Ok((before - rows.len()) as u64)
        } else {
            Err(DbError::execute_error(format!("fake can't run {sql:?}")))
        }
    }

    fn select(&self, sql: &str) -> Result<QueryResult, DbError> {
        let rows = self.rows.lock().unwrap();
        let (column, rows) = match sql {
            "SELECT 1 AS one" => ("one", vec![vec![Value::Int(1)]]),
            "SELECT COUNT(*) FROM t" => ("count", vec![vec![Value::Int(rows.len() as i64)]]),
            "SELECT n FROM t ORDER BY n" => {
                let mut sorted = rows.clone();
                sorted.sort_unstable();
                ("n", int_rows(&sorted))
            }
            "SELECT read_only" => (
                "read_only",
                vec![vec![Value::Bool(
                    self.pool_read_only.load(Ordering::SeqCst),
                )]],
            ),
            _ => return Err(DbError::query_error(format!("fake can't run {sql:?}"))),
        };
        Ok(QueryResult {
            columns: vec![column.into()],
            rows,
        })
    }
}

/// Clears `busy` when the slow query's future is dropped, unless the flaw
/// says the server ignores that.
struct Running(Arc<AtomicBool>, bool);

impl Drop for Running {
    fn drop(&mut self) {
        if self.1 {
            self.0.store(false, Ordering::SeqCst);
        }
    }
}

#[seaquel_runtime::async_trait]
impl Driver for FakeDriver {
    async fn query(&self, sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        self.select(sql)
    }

    async fn execute(&self, sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        if self.pool_read_only.load(Ordering::SeqCst) {
            return Err(DbError::read_only("the pool was left read-only"));
        }
        Ok(ExecuteResult {
            rows_affected: self.write(sql)?,
            last_insert_id: None,
        })
    }

    async fn query_read_only(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        // A previous slow query still holds the one connection.
        while self.busy.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self.path_dirty.load(Ordering::SeqCst) {
            return Err(DbError::query_error(
                "the read-only connection is still inside a transaction",
            ));
        }
        if sql == "LEAVE_OPEN" {
            // Returns, but leaves the read-only path's own connection dirty.
            self.path_dirty.store(true, Ordering::SeqCst);
            return self.select("SELECT 1 AS one");
        }
        if sql == "SLOW" {
            self.busy.store(true, Ordering::SeqCst);
            let _running = Running(self.busy.clone(), self.flaw != Flaw::IgnoresCancel);
            tokio::time::sleep(Duration::from_secs(60)).await;
            return self.select("SELECT 1 AS one");
        }
        if sql.starts_with("INSERT") || sql.starts_with("DELETE") {
            return match self.flaw {
                Flaw::Writes => {
                    self.write(sql)?;
                    Ok(QueryResult {
                        columns: vec![],
                        rows: vec![],
                    })
                }
                Flaw::LeaksSession => {
                    self.pool_read_only.store(true, Ordering::SeqCst);
                    Err(DbError::read_only(
                        "cannot write in a read-only transaction",
                    ))
                }
                Flaw::WrongCode => Err(DbError::query_error("cannot write")),
                Flaw::None | Flaw::IgnoresCancel => Err(DbError::read_only(
                    "cannot write in a read-only transaction",
                )),
            };
        }
        self.select(sql)
    }

    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
}

struct FakeEngine(Flaw);

#[seaquel_runtime::async_trait]
impl Engine for FakeEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(FakeDriver {
            flaw: self.0,
            rows: Mutex::default(),
            pool_read_only: AtomicBool::new(false),
            path_dirty: AtomicBool::new(false),
            busy: Arc::new(AtomicBool::new(false)),
        }))
    }
}

fn config() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "sqlite" })).unwrap()
}

/// The two-attack spec: a write that must be refused and leave `t` as it
/// was, and a read that must run.
fn spec() -> ReadOnlySpec {
    ReadOnlySpec {
        setup: vec!["INSERT INTO t VALUES (1)".into()],
        teardown: vec!["DELETE FROM t WHERE n = 1".into()],
        attacks: vec![
            Attack::new("insert", "INSERT INTO t VALUES (2)")
                .refused_with("READ_ONLY")
                .trace(Check::rows(
                    "SELECT n FROM t ORDER BY n",
                    vec![vec![Value::Int(1)]],
                )),
            Attack::allowed("select", "SELECT n FROM t ORDER BY n")
                .returns(vec![vec![Value::Int(1)]]),
        ],
        after_each: vec![
            Check::executes("INSERT INTO t VALUES (99)"),
            Check::executes("DELETE FROM t WHERE n = 99"),
            Check::value("SELECT read_only", false),
            Check::count("SELECT COUNT(*) FROM t", 1),
        ],
        slow_query: "SLOW".into(),
        // The fake's cancel is instant; keeps the flaw tests fast.
        cancel_within: Duration::from_secs(1),
    }
}

/// The harness's panic message for a driver with `flaw`.
async fn failure(flaw: Flaw) -> String {
    let panic = AssertUnwindSafe(run_read_only(&FakeEngine(flaw), &config(), &spec()))
        .catch_unwind()
        .await
        .expect_err("the harness must catch the flaw");
    panic
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "(non-string panic)".into())
}

#[tokio::test]
async fn a_driver_that_meets_the_contract_passes() {
    run_read_only(&FakeEngine(Flaw::None), &config(), &spec()).await;
}

#[tokio::test]
async fn a_write_that_gets_through_is_caught_by_its_trace() {
    let message = failure(Flaw::Writes).await;
    assert!(
        message.contains("insert (INSERT INTO t VALUES (2))"),
        "{message}"
    );
    assert!(message.contains("expected a refusal"), "{message}");
    assert!(
        message.contains("trace rows of \"SELECT n FROM t ORDER BY n\""),
        "{message}"
    );
    // The select attack after it saw the row too.
    assert!(message.contains("select ("), "{message}");
}

#[tokio::test]
async fn a_session_left_read_only_is_caught_by_after_each() {
    let message = failure(Flaw::LeaksSession).await;
    assert!(
        message.contains("after_each executes \"INSERT INTO t VALUES (99)\""),
        "{message}"
    );
    assert!(
        message.contains("after_each rows of \"SELECT read_only\""),
        "{message}"
    );
}

#[tokio::test]
async fn a_query_that_outlives_its_cancel_is_caught() {
    let message = failure(Flaw::IgnoresCancel).await;
    assert!(message.contains("cancel (SLOW)"), "{message}");
    assert!(
        message.contains("the dropped query still runs"),
        "{message}"
    );
}

#[tokio::test]
async fn a_refusal_with_the_wrong_code_is_caught() {
    let message = failure(Flaw::WrongCode).await;
    assert!(
        message.contains("expected code Some(\"READ_ONLY\")"),
        "{message}"
    );
}

#[tokio::test]
async fn a_slow_query_that_is_not_slow_is_reported() {
    let spec = ReadOnlySpec {
        slow_query: "SELECT 1 AS one".into(),
        ..spec()
    };
    let panic = AssertUnwindSafe(run_read_only(&FakeEngine(Flaw::None), &config(), &spec))
        .catch_unwind()
        .await
        .expect_err("a fast slow_query must fail the run");
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("the cancel wasn't tested"), "{message}");
}

#[tokio::test]
async fn a_driver_without_query_read_only_fails_every_attack() {
    // The trait default is NOT_SUPPORTED: "allowed" attacks and the cancel
    // check fail, so an engine can't pass by leaving the method out.
    struct Plain;
    #[seaquel_runtime::async_trait]
    impl Driver for Plain {
        async fn query(&self, _sql: &str, _p: Vec<Value>) -> Result<QueryResult, DbError> {
            Ok(QueryResult {
                columns: vec![],
                rows: vec![],
            })
        }
        async fn execute(&self, _sql: &str, _p: Vec<Value>) -> Result<ExecuteResult, DbError> {
            Ok(ExecuteResult {
                rows_affected: 0,
                last_insert_id: None,
            })
        }
        async fn close(&self) -> Result<(), DbError> {
            Ok(())
        }
    }
    struct PlainEngine;
    #[seaquel_runtime::async_trait]
    impl Engine for PlainEngine {
        fn id(&self) -> &'static str {
            "sqlite"
        }
        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(Plain))
        }
    }
    let spec = ReadOnlySpec {
        setup: vec![],
        teardown: vec![],
        attacks: vec![Attack::allowed("select", "SELECT 1 AS one")],
        after_each: vec![Check::executes("INSERT INTO t VALUES (1)")],
        slow_query: "SLOW".into(),
        ..Default::default()
    };
    let panic = AssertUnwindSafe(run_read_only(&PlainEngine, &config(), &spec))
        .catch_unwind()
        .await
        .expect_err("NOT_SUPPORTED must fail the run");
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("NOT_SUPPORTED"), "{message}");
}

/// The message of the panic `run_read_only` raises for `spec`.
async fn panic_of(engine: &dyn Engine, spec: &ReadOnlySpec) -> String {
    let panic = AssertUnwindSafe(run_read_only(engine, &config(), spec))
        .catch_unwind()
        .await
        .expect_err("run_read_only must fail");
    panic
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "(non-string panic)".into())
}

#[tokio::test]
async fn an_attack_that_may_fail_without_a_trace_proves_nothing() {
    // `Flaw::Writes` would get through: without a trace nothing sees it.
    for attack in [
        Attack::new("any", "INSERT INTO t VALUES (2)"),
        Attack::new("refused", "INSERT INTO t VALUES (2)").refused_with("READ_ONLY"),
    ] {
        let spec = ReadOnlySpec {
            attacks: vec![attack],
            ..spec()
        };
        let message = panic_of(&FakeEngine(Flaw::Writes), &spec).await;
        assert!(
            message.contains("read-only spec proves nothing"),
            "{message}"
        );
        assert!(message.contains("has no trace"), "{message}");
    }
    // An allowed query needs none: its rows are what it checks.
    let spec = ReadOnlySpec {
        attacks: vec![Attack::allowed("select", "SELECT 1 AS one")],
        ..spec()
    };
    run_read_only(&FakeEngine(Flaw::None), &config(), &spec).await;
}

#[tokio::test]
async fn a_spec_without_after_each_proves_nothing() {
    let spec = ReadOnlySpec {
        after_each: vec![],
        ..spec()
    };
    let message = panic_of(&FakeEngine(Flaw::None), &spec).await;
    assert!(message.contains("after_each is empty"), "{message}");
}

#[tokio::test]
async fn state_left_on_the_read_only_path_is_blamed_on_its_attack() {
    // The probe after each attack runs through `query_read_only`, so the
    // attack that left the path's own connection dirty is reported itself,
    // not only whatever runs next on that path.
    let spec = ReadOnlySpec {
        attacks: vec![
            Attack::allowed("leaves the path dirty", "LEAVE_OPEN"),
            Attack::allowed("select", "SELECT 1 AS one"),
        ],
        ..spec()
    };
    let message = panic_of(&FakeEngine(Flaw::None), &spec).await;
    let blamed = message
        .split("\n\n")
        .find(|f| f.starts_with("leaves the path dirty (LEAVE_OPEN)"))
        .unwrap_or_else(|| panic!("{message}"));
    assert!(
        blamed.contains("SELECT 1 AS one after the attack failed"),
        "{message}"
    );
}

// ── A pool whose leak only concurrent checks find ──

/// Three "connections". The pool hands out the most recently returned idle
/// one (LIFO), so checks run one at a time always land on the same
/// connection. The leaky `query_read_only` takes the least recently used one
/// and hands it back read-only.
struct PoolDriver {
    idle: Mutex<VecDeque<usize>>,
    read_only: Mutex<[bool; 3]>,
}

impl PoolDriver {
    async fn acquire(&self, newest: bool) -> usize {
        loop {
            let next = {
                let mut idle = self.idle.lock().unwrap();
                if newest {
                    idle.pop_front()
                } else {
                    idle.pop_back()
                }
            };
            if let Some(conn) = next {
                return conn;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    fn release(&self, conn: usize, newest: bool) {
        let mut idle = self.idle.lock().unwrap();
        if newest {
            idle.push_front(conn);
        } else {
            idle.push_back(conn);
        }
    }

    /// Hold a connection for a moment, as a real query does.
    async fn on_connection(&self) -> Result<(), DbError> {
        let conn = self.acquire(true).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        let read_only = self.read_only.lock().unwrap()[conn];
        self.release(conn, true);
        if read_only {
            Err(DbError::read_only(format!(
                "connection {conn} was left read-only"
            )))
        } else {
            Ok(())
        }
    }
}

#[seaquel_runtime::async_trait]
impl Driver for PoolDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        self.on_connection().await?;
        Ok(QueryResult {
            columns: vec!["one".into()],
            rows: vec![vec![Value::Int(1)]],
        })
    }

    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        self.on_connection().await?;
        Ok(ExecuteResult {
            rows_affected: 1,
            last_insert_id: None,
        })
    }

    async fn query_read_only(
        &self,
        sql: &str,
        _params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        if sql == "SLOW" {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        let conn = self.acquire(false).await;
        // The flaw: the connection goes back to the pool read-only.
        self.read_only.lock().unwrap()[conn] = true;
        self.release(conn, false);
        if sql.starts_with("INSERT") {
            return Err(DbError::read_only(
                "cannot write in a read-only transaction",
            ));
        }
        Ok(QueryResult {
            columns: vec!["one".into()],
            rows: vec![vec![Value::Int(1)]],
        })
    }

    async fn close(&self) -> Result<(), DbError> {
        Ok(())
    }
}

struct PoolEngine;

#[seaquel_runtime::async_trait]
impl Engine for PoolEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(PoolDriver {
            idle: Mutex::new((0..3).collect()),
            read_only: Mutex::new([false; 3]),
        }))
    }
}

fn pool_spec(after_each: Check) -> ReadOnlySpec {
    ReadOnlySpec {
        attacks: vec![Attack::new("insert", "INSERT INTO t VALUES (2)")
            .refused_with("READ_ONLY")
            .trace(Check::value("SELECT 1 AS one", 1))],
        after_each: vec![after_each],
        slow_query: "SLOW".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_session_leaked_onto_a_pooled_connection_is_caught_on_connections() {
    // One check at a time keeps landing on the one clean connection...
    run_read_only(&PoolEngine, &config(), &pool_spec(Check::executes("WRITE"))).await;
    // ...three at once reach the leaked one.
    let message = panic_of(
        &PoolEngine,
        &pool_spec(Check::on_connections(3, Check::executes("WRITE"))),
    )
    .await;
    assert!(
        message.contains("after_each on 3 connections at once, executes \"WRITE\""),
        "{message}"
    );
    assert!(message.contains("was left read-only"), "{message}");
}
