//! `query_read_only` (AI safety Task 3): every attack is refused or runs
//! without a trace, on a temp-file database and on sqlx's shared-cache
//! `sqlite::memory:` database (the desktop tutorial's). No server needed.
//!
//! Each test gets a fresh database, so the fixed object names (`t`, `v`, `w`)
//! can't collide with anything.
//!
//! Accepted gaps (plan: "Probe results", SQLite column), not asserted:
//!
//! - Statements that only change the call's own connection (`BEGIN`,
//!   `SAVEPOINT`) pass the gate: the connection is closed at the end of the
//!   call, and the attacks below check nothing outlives it. PRAGMAs and
//!   `ATTACH`/`DETACH` don't: the connection's authorizer refuses them.
//! - Memory isn't bounded: `SELECT randomblob(1000000000)` builds a 1 GB
//!   value in one step, which the row cap doesn't limit (it's one row) and
//!   a cancel only stops once SQLite next checks the progress handler.
//! - A read that waits behind a writer (shared-cache `SQLITE_LOCKED`, which
//!   sqlx waits out with unlock_notify, or a file's busy timeout) can't be
//!   interrupted at the SQLite level: it isn't running a statement. Dropping
//!   the call drops the wait; the writer's transaction bounds it, and the
//!   driver never leaves one open across calls.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use seaquel_engine::{ConnectConfig, Driver, Value};
use seaquel_engine_testkit::{run_read_only, Attack, Check, ReadOnlySpec, CANCEL_AFTER};

/// A fresh directory for the database file and for the files attacks try to
/// write, deleted on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("seaquel-ro-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(connection_string: &str) -> ConnectConfig {
    serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": connection_string,
        "create_if_missing": true
    }))
    .unwrap()
}

fn file_config(dir: &TempDir) -> ConnectConfig {
    config(&format!("sqlite:{}", dir.path("db.sqlite").display()))
}

fn memory_config() -> ConnectConfig {
    config("sqlite::memory:")
}

/// SQL string literal for a path.
fn lit(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

const SETUP: &[&str] = &[
    "CREATE TABLE t (n INTEGER)",
    "INSERT INTO t VALUES (1)",
    // A view whose INSTEAD OF trigger writes to t.
    "CREATE VIEW v AS SELECT n FROM t",
    "CREATE TRIGGER v_insert INSTEAD OF INSERT ON v BEGIN INSERT INTO t VALUES (NEW.n); END",
    // What after_each writes to, so t keeps exactly one row.
    "CREATE TABLE w (n INTEGER)",
    // FTS5 opens its tables with internal PRAGMAs the authorizer must allow.
    "CREATE VIRTUAL TABLE f5 USING fts5(body)",
    "INSERT INTO f5 VALUES ('hello world')",
];

/// A recursive CTE counting to 10^9, joined to `t` so it holds t's locks
/// (a SHARED lock on the file, a shared-cache table lock in memory) while
/// it runs.
const SLOW_QUERY: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c \
     WHERE x < 1000000000) SELECT count(*) FROM c CROSS JOIN t";

fn t_unchanged() -> Check {
    Check::rows("SELECT n FROM t", vec![vec![Value::Int(1)]])
}

fn no_table(name: &str) -> Check {
    Check::count(
        format!("SELECT COUNT(*) FROM sqlite_master WHERE name = '{name}'"),
        0,
    )
}

/// The attacks and allowed queries, shared by the file and in-memory runs.
/// `journal_mode` is what the database reports before any attack: `wal` for
/// a file (sqlx creates databases in WAL mode), `memory` in memory.
fn spec(dir: &TempDir, journal_mode: &str) -> ReadOnlySpec {
    let vacuum_into = dir.path("vacuum.sqlite");
    let attached = dir.path("attached.sqlite");
    let attached_2 = dir.path("attached-2.sqlite");

    let refused = |name: &str, sql: &str| Attack::new(name, sql).refused_with("READ_ONLY");
    let not_allowed = |name: &str, sql: &str| refused(name, sql).message_contains("not authorized");

    let attacks = vec![
        // ── Writes ──
        refused("insert", "INSERT INTO t VALUES (2)")
            .message_contains("write")
            .trace(t_unchanged()),
        refused("update returning", "UPDATE t SET n = 2 RETURNING n").trace(t_unchanged()),
        refused("delete", "DELETE FROM t").trace(t_unchanged()),
        refused("replace", "REPLACE INTO t VALUES (2)").trace(t_unchanged()),
        refused(
            "write in a CTE",
            "WITH d(n) AS (SELECT 2) INSERT INTO t SELECT n FROM d",
        )
        .trace(t_unchanged()),
        refused(
            "write through an INSTEAD OF trigger",
            "INSERT INTO v VALUES (2)",
        )
        .trace(t_unchanged()),
        // ── More than one statement ──
        refused("select, then insert", "SELECT 1; INSERT INTO t VALUES (2)")
            .message_contains("one statement")
            .trace(t_unchanged()),
        refused(
            "query_only off, then insert",
            "PRAGMA query_only = OFF; INSERT INTO t VALUES (2)",
        )
        .trace(t_unchanged()),
        refused("two selects", "SELECT 1; SELECT 2")
            .message_contains("one statement")
            .trace(t_unchanged()),
        refused(
            "insert hidden after a comment and blank statements",
            "SELECT 1; ;; /* x */ ; INSERT INTO t VALUES (2)",
        )
        .trace(t_unchanged()),
        Attack::new(
            "insert after a NUL byte",
            "SELECT 1\0; INSERT INTO t VALUES (2)",
        )
        .refused_with("QUERY_ERROR")
        .message_contains("NUL")
        .trace(t_unchanged()),
        not_allowed("query_only off alone", "PRAGMA query_only = OFF").trace(t_unchanged()),
        // ── Process-wide settings (`sqlite3_stmt_readonly` is true for
        // them; the authorizer refuses them). The values are harmless if
        // one got through, so a regression fails here instead of crashing
        // the test process: a low hard_heap_limit (200000) aborts it.
        not_allowed("hard_heap_limit", "PRAGMA hard_heap_limit = 1000000000000")
            .trace(Check::count("PRAGMA hard_heap_limit", 0)),
        not_allowed("soft_heap_limit", "PRAGMA soft_heap_limit = 1000000000000")
            .trace(Check::count("PRAGMA soft_heap_limit", 0)),
        not_allowed(
            "temp_store_directory",
            format!("PRAGMA temp_store_directory = {}", lit(&dir.0)).as_str(),
        )
        .trace(Check::rows("PRAGMA temp_store_directory", vec![]))
        .after("PRAGMA temp_store_directory = ''"),
        // Read through a pragma function: the PRAGMA it prepares when it
        // runs goes through the authorizer too.
        not_allowed(
            "pragma function outside the allowlist",
            "SELECT * FROM pragma_hard_heap_limit",
        )
        .trace(Check::count("PRAGMA hard_heap_limit", 0)),
        // ── DDL ──
        refused("create table", "CREATE TABLE x (a)").trace(no_table("x")),
        refused("create table as select", "CREATE TABLE x AS SELECT 1").trace(no_table("x")),
        refused("drop table", "DROP TABLE t").trace(t_unchanged()),
        refused("alter table", "ALTER TABLE t ADD COLUMN m INTEGER").trace(Check::count(
            "SELECT COUNT(*) FROM pragma_table_info('t')",
            1,
        )),
        refused("create index", "CREATE INDEX t_n ON t (n)").trace(no_table("t_n")),
        refused("drop view", "DROP VIEW v").trace(Check::count(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'v'",
            1,
        )),
        refused("create temp table", "CREATE TEMP TABLE tmp (a)").trace(t_unchanged()),
        refused(
            "create temp trigger",
            "CREATE TEMP TRIGGER tt AFTER INSERT ON w BEGIN DELETE FROM t; END",
        )
        .trace(t_unchanged()),
        // ── Files ──
        refused(
            "vacuum into",
            format!("VACUUM INTO {}", lit(&vacuum_into)).as_str(),
        )
        .trace(Check::no_file(&vacuum_into)),
        Attack::new("vacuum", "VACUUM").trace(t_unchanged()),
        not_allowed(
            "attach a new file",
            format!("ATTACH {} AS a", lit(&attached)).as_str(),
        )
        .trace(Check::no_file(&attached)),
        not_allowed(
            "attach a new file, then write to it",
            format!(
                "ATTACH {} AS a; CREATE TABLE a.x (n); INSERT INTO a.x VALUES (1)",
                lit(&attached_2)
            )
            .as_str(),
        )
        .trace(Check::no_file(&attached_2)),
        not_allowed("attach memory", "ATTACH ':memory:' AS m").trace(t_unchanged()),
        not_allowed(
            "attach another shared-cache memory database",
            "ATTACH 'file:seaquel-ro-other?mode=memory&cache=shared' AS m",
        )
        .trace(t_unchanged()),
        not_allowed("detach", "DETACH main").trace(t_unchanged()),
        Attack::new(
            "load_extension",
            "SELECT load_extension('seaquel_no_such_ext')",
        )
        .refused()
        .trace(t_unchanged()),
        // ── Settings stored in the database file ──
        not_allowed("user_version", "PRAGMA user_version = 7")
            .trace(Check::count("PRAGMA user_version", 0)),
        not_allowed("journal_mode", "PRAGMA journal_mode = TRUNCATE")
            .trace(Check::value("PRAGMA journal_mode", journal_mode)),
        Attack::new("analyze", "ANALYZE").trace(no_table("sqlite_stat1")),
        Attack::new("reindex", "REINDEX").trace(t_unchanged()),
        // A lock left behind would make after_each's write wait and fail.
        Attack::new("begin immediate", "BEGIN IMMEDIATE").trace(t_unchanged()),
        Attack::new("begin exclusive", "BEGIN EXCLUSIVE").trace(t_unchanged()),
        // ── Not a statement ──
        Attack::new("empty", "").refused().trace(t_unchanged()),
        Attack::new("only a comment", "-- nothing")
            .refused()
            .trace(t_unchanged()),
        Attack::new("syntax error", "SELEC 1")
            .refused_with("QUERY_ERROR")
            .message_contains("syntax error")
            .trace(t_unchanged()),
        // ── Allowed ──
        Attack::allowed("select", "SELECT n FROM t").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("select from the view", "SELECT n FROM v")
            .returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("with", "WITH c AS (SELECT n FROM t) SELECT n + 1 FROM c")
            .returns(vec![vec![Value::Int(2)]]),
        Attack::allowed("trailing semicolon", "SELECT 1;").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("trailing comment", "SELECT 1; -- note").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed(
            "comments and blank statements around it",
            "/* a */ ; SELECT 1 /* b */ ; ; -- c",
        )
        .returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("bound parameter", "SELECT ? + 1")
            .params(vec![Value::Int(41)])
            .returns(vec![vec![Value::Int(42)]]),
        Attack::allowed("multi-byte text", "SELECT 'ü' AS é; -- ß")
            .returns(vec![vec![Value::from("ü")]]),
        Attack::allowed("pragma function", "SELECT name FROM pragma_table_info('t')")
            .returns(vec![vec![Value::from("n")]]),
        Attack::allowed("query pragma", "PRAGMA table_info(t)"),
        Attack::allowed("query pragma without a value", "PRAGMA user_version")
            .returns(vec![vec![Value::Int(0)]]),
        Attack::allowed("fts5 match", "SELECT body FROM f5 WHERE f5 MATCH 'hello'")
            .returns(vec![vec![Value::from("hello world")]]),
        Attack::allowed("fts5 count", "SELECT count(*) FROM f5").returns(vec![vec![Value::Int(1)]]),
        Attack::allowed(
            "integrity_check with an fts5 table",
            "PRAGMA integrity_check",
        )
        .returns(vec![vec![Value::from("ok")]]),
        Attack::allowed("quick_check with an fts5 table", "PRAGMA quick_check")
            .returns(vec![vec![Value::from("ok")]]),
        not_allowed("page_size with a value", "PRAGMA page_size = 8192")
            .trace(Check::count("PRAGMA page_size", 4096)),
        not_allowed("data_version with a value", "PRAGMA data_version = 5").trace(t_unchanged()),
        Attack::allowed("explain query plan", "EXPLAIN QUERY PLAN SELECT n FROM t"),
    ];

    ReadOnlySpec {
        setup: SETUP.iter().map(|s| s.to_string()).collect(),
        attacks,
        // Several pooled connections at once: each still writes, and none
        // has query_only set (it's only ever set on the read-only path's own
        // connection).
        after_each: vec![
            Check::on_connections(3, Check::executes("INSERT INTO w VALUES (1)")),
            Check::on_connections(3, Check::count("PRAGMA query_only", 0)),
            t_unchanged(),
        ],
        slow_query: SLOW_QUERY.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn read_only_file() {
    let dir = TempDir::new();
    run_read_only(
        &*seaquel_engine_sqlite::engine(),
        &file_config(&dir),
        &spec(&dir, "wal"),
    )
    .await;
}

#[tokio::test]
async fn read_only_in_memory() {
    let dir = TempDir::new();
    run_read_only(
        &*seaquel_engine_sqlite::engine(),
        &memory_config(),
        &spec(&dir, "memory"),
    )
    .await;
}

async fn open(config: &ConnectConfig) -> Arc<dyn Driver> {
    seaquel_engine_sqlite::engine()
        .open(config)
        .await
        .expect("open")
}

/// A read-only query dropped mid-run is interrupted, so it stops holding its
/// locks at once: a write to the table it was reading doesn't wait for it.
/// On a file in rollback-journal mode, the running query holds a SHARED lock
/// that makes a writer wait up to sqlx's 5 s busy timeout (in WAL mode,
/// sqlx's default for new files, readers don't block writers, so this test
/// switches it off first); in memory, a shared-cache table lock the writer
/// waits on (unlock_notify) until the reader finishes.
async fn dropped_query_releases_its_locks(config: ConnectConfig, setup: &[&str]) {
    let driver = open(&config).await;
    for sql in setup.iter().chain(SETUP) {
        driver.execute(sql, vec![]).await.expect(sql);
    }
    let slow = tokio::time::timeout(CANCEL_AFTER, driver.query_read_only(SLOW_QUERY, vec![])).await;
    assert!(slow.is_err(), "the slow query finished: {slow:?}");
    let write = tokio::time::timeout(
        Duration::from_secs(2),
        driver.execute("INSERT INTO t VALUES (2)", vec![]),
    )
    .await;
    assert!(
        matches!(write, Ok(Ok(_))),
        "a write after the dropped query: {write:?}"
    );
    driver.close().await.unwrap();
}

#[tokio::test]
async fn dropped_query_releases_its_locks_file() {
    let dir = TempDir::new();
    dropped_query_releases_its_locks(file_config(&dir), &["PRAGMA journal_mode = DELETE"]).await;
}

#[tokio::test]
async fn dropped_query_releases_its_locks_in_memory() {
    dropped_query_releases_its_locks(memory_config(), &[]).await;
}

/// Two in-memory databases stay apart: the read-only connection opens the
/// pool's own shared-cache database, not another driver's.
#[tokio::test]
async fn in_memory_databases_stay_apart() {
    let a = open(&memory_config()).await;
    let b = open(&memory_config()).await;
    a.execute("CREATE TABLE only_a (n INTEGER)", vec![])
        .await
        .unwrap();
    assert!(a
        .query_read_only("SELECT COUNT(*) FROM only_a", vec![])
        .await
        .is_ok());
    let err = b
        .query_read_only("SELECT COUNT(*) FROM only_a", vec![])
        .await
        .expect_err("b sees a's table");
    assert!(err.message.contains("no such table"), "{err:?}");
    a.close().await.unwrap();
    b.close().await.unwrap();
}

/// A NUL byte is refused on every path that takes SQL text, before sqlx
/// sees it: sqlx 0.8.6 loops forever on SQLite's zero-length tail at a NUL,
/// holding its pooled connection, so a pool's worth of them hung the
/// database for good. Each call is under a timeout so a regression fails
/// instead of hanging the test.
#[tokio::test]
async fn nul_bytes_are_refused_on_every_path() {
    use futures::StreamExt;
    use seaquel_engine::{BatchStatement, CancellationToken};

    const SQL: &str = "SELECT 1\0";
    let within = Duration::from_secs(2);
    let driver = open(&memory_config()).await;
    driver
        .execute("CREATE TABLE t (n INTEGER)", vec![])
        .await
        .unwrap();

    let check = |what: &str, result: Result<Result<(), seaquel_engine::DbError>, _>| {
        let e = match result {
            Ok(Err(e)) => e,
            other => panic!("{what}: expected a refusal, got {other:?}"),
        };
        assert_eq!(e.code, "QUERY_ERROR", "{what}: {e:?}");
        assert!(e.message.contains("NUL"), "{what}: {e:?}");
    };
    // More calls than the pool has connections (10), on every path.
    for _ in 0..3 {
        let r = tokio::time::timeout(within, driver.query(SQL, vec![])).await;
        check("query", r.map(|r| r.map(drop)));
        let r = tokio::time::timeout(within, driver.execute(SQL, vec![])).await;
        check("execute", r.map(|r| r.map(drop)));
        let r = tokio::time::timeout(within, driver.query_read_only(SQL, vec![])).await;
        check("query_read_only", r.map(|r| r.map(drop)));
        let r = tokio::time::timeout(within, driver.explain(SQL, vec![], true)).await;
        check("explain", r.map(|r| r.map(drop)));
        let statement = |sql: &str| BatchStatement {
            sql: sql.to_string(),
            params: vec![],
            expect_rows: None,
        };
        let batch = vec![statement("INSERT INTO t VALUES (1)"), statement(SQL)];
        let r = tokio::time::timeout(within, driver.transaction(batch)).await;
        check("transaction", r);
        let stream = driver.query_stream(SQL.to_string(), vec![], CancellationToken::new());
        let r = tokio::time::timeout(within, stream.collect::<Vec<_>>()).await;
        let first = r.expect("query_stream hung").into_iter().next();
        check("query_stream", Ok(first.expect("no event").map(drop)));
    }
    // Nothing ran (the transaction's INSERT neither), and the pool works.
    let r = tokio::time::timeout(within, driver.query("SELECT COUNT(*) FROM t", vec![]))
        .await
        .expect("the pool is stuck")
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(0)]]);
    driver.close().await.unwrap();
}
