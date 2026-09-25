//! Blocking work, panics and cancellation, against an in-memory database.
//!
//! The tests run on a current-thread runtime, so a DuckDB call that blocked
//! the runtime would also stop the timers they race it against. Each test
//! runs through [`run`], not `#[tokio::test]`: a `spawn_blocking` task can't
//! be cancelled, and a runtime dropped normally waits for it, so a cancel
//! regression would leave a test waiting on an endless query forever.

use std::future::Future;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_engine::{BatchStatement, CancellationToken, DbError, Driver, StreamBatch, Value};
use tokio::time::{sleep, timeout, Instant};

/// Runs until interrupted: about 10^12 rows, many minutes even in release
/// builds (10^10 took ~8 s in debug).
const ENDLESS: &str = "SELECT count(*) FROM range(1000000000000)";

/// How long a cancelled call may take to give the connection back.
const CANCEL_BUDGET: Duration = Duration::from_secs(2);

/// Runs `test` on a current-thread runtime, then shuts the runtime down
/// without waiting more than a second for blocking tasks (a query that
/// wasn't interrupted keeps its thread; the test process exits anyway). A
/// panic is re-raised after the shutdown.
fn run(test: impl Future<Output = ()>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome = catch_unwind(AssertUnwindSafe(|| rt.block_on(test)));
    rt.shutdown_timeout(Duration::from_secs(1));
    if let Err(panic) = outcome {
        resume_unwind(panic);
    }
}

async fn open() -> Arc<dyn Driver> {
    let config = serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": ":memory:"
    }))
    .unwrap();
    seaquel_engine_duckdb::engine().open(&config).await.unwrap()
}

async fn stream_all(driver: &dyn Driver, sql: &str) -> Vec<Result<StreamBatch, DbError>> {
    driver
        .query_stream(sql.to_string(), vec![], CancellationToken::new())
        .collect()
        .await
}

/// How long a test waits for the connection after a cancel before failing,
/// rather than waiting forever behind a query that wasn't interrupted.
const STUCK: Duration = Duration::from_secs(20);

/// The connection answers a query, within [`STUCK`].
async fn assert_usable(driver: &dyn Driver) {
    let r = timeout(STUCK, driver.query("SELECT 42 AS n", vec![]))
        .await
        .expect("the connection is still busy")
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(42)]]);
}

/// Each query must return rows, through `query` and `query_stream`, and
/// leave the connection usable. Before Task 15 the `TIME_NS` ones panicked
/// in duckdb-rs (`unreachable!("invalid value: Time64(ns)")`), and until
/// Task 16 they were `UNSUPPORTED_TYPE` errors; the driver now reads Arrow
/// itself. `tests/values.rs` checks what each value decodes to.
#[test]
fn unusual_types_return_a_value_or_a_clean_error() {
    run(unusual_types_return_a_value_or_a_clean_error_body());
}

async fn unusual_types_return_a_value_or_a_clean_error_body() {
    let driver = open().await;
    driver
        .execute("CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy')", vec![])
        .await
        .unwrap();

    let readable = [
        "SELECT INTERVAL 1 YEAR",
        "SELECT INTERVAL 3 MONTH",
        "SELECT INTERVAL 2 DAY",
        "SELECT INTERVAL 5 SECOND",
        "SELECT INTERVAL 7 MILLISECOND",
        "SELECT INTERVAL 9 MICROSECOND",
        "SELECT -INTERVAL '1 year 2 months 3 days 04:05:06.789'",
        "SELECT to_years(1), to_months(2), to_days(3), to_hours(4)",
        "SELECT age(TIMESTAMP '2024-01-01', TIMESTAMP '2020-03-04')",
        "SELECT [INTERVAL 1 YEAR, INTERVAL 1 DAY]",
        "SELECT union_value(num := 2)",
        "SELECT union_value(i := INTERVAL 1 YEAR)",
        "SELECT MAP {'a': 1, 'b': 2}",
        "SELECT MAP {1: INTERVAL 1 DAY}",
        "SELECT 'ok'::mood",
        "SELECT '101010'::BIT",
        "SELECT TIMETZ '12:00:00+02'",
        "SELECT 123456789012345678901234567890123456789012::VARINT",
        "SELECT -7::BIGNUM",
        "SELECT 340282366920938463463374607431768211455::UHUGEINT",
        "SELECT '-170141183460469231731687303715884105728'::HUGEINT",
        "SELECT 99999999999999999999999999999999999999::DECIMAL(38,0)",
        "SELECT [1, 2, 3]::INTEGER[3]",
        "SELECT [[1], [2, NULL], []]",
        "SELECT {'a': 1, 'b': {'c': [1, 2]}, 'i': INTERVAL 1 YEAR}",
        "SELECT [TIME_NS '12:00:00.123456789']",
        "SELECT {'t': TIME_NS '12:00:00.123456789'}",
        "SELECT union_value(t := TIME_NS '12:00:00')",
        "SELECT NULL::TIME_NS",
        "SELECT TIMESTAMP_NS '2024-01-01 00:00:00.123456789', TIMESTAMP_S '2024-01-01'",
        "SELECT 'infinity'::TIMESTAMP, '-infinity'::DATE",
        "SELECT 'POINT(1 2)'::GEOMETRY",
        "SELECT '00000000-0000-4000-8000-000000000000'::UUID, '{\"a\": 1}'::JSON",
        "SELECT '\\xFF'::BLOB",
    ];
    for sql in readable {
        let r = driver.query(sql, vec![]).await;
        assert!(r.is_ok(), "{sql}: {r:?}");
        assert_no_debug_text(sql, &r.unwrap().rows);
        for item in stream_all(&*driver, sql).await {
            assert!(item.is_ok(), "{sql} (stream): {item:?}");
            assert_no_debug_text(sql, &item.unwrap().rows);
        }
    }

    let r = driver
        .query("SELECT NULL::TIME_NS AS t", vec![])
        .await
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Null]]);
    let r = driver
        .query("SELECT 1 AS a, TIME_NS '00:00:01.5' AS t", vec![])
        .await
        .unwrap();
    assert_eq!(
        r.rows,
        vec![vec![Value::Int(1), Value::Text("00:00:01.5".into())]]
    );

    // duckdb-rs rejects VARIANT itself, with an error.
    let e = driver.query("SELECT 1::VARIANT", vec![]).await.unwrap_err();
    assert_eq!(e.code, "QUERY_ERROR");

    // Arrow settings change the Arrow types DuckDB sends.
    for setting in ["arrow_lossless_conversion", "arrow_large_buffer_size"] {
        driver
            .execute(&format!("SET {setting} = true"), vec![])
            .await
            .unwrap();
        let r = driver
            .query(
                "SELECT TIMETZ '12:00:00+02', gen_random_uuid(), 'a', 'b'::BLOB, [1], \
                 'POINT(1 2)'::GEOMETRY",
                vec![],
            )
            .await;
        assert!(r.is_ok(), "{setting}: {r:?}");
        driver
            .execute(&format!("RESET {setting}"), vec![])
            .await
            .unwrap();
    }

    assert_usable(&*driver).await;
}

/// No cell is a Rust `Debug` dump, the pre-Task 16 fallback for LIST,
/// STRUCT, MAP, INTERVAL, TIME and TIMESTAMP values.
fn assert_no_debug_text(sql: &str, rows: &[Vec<Value>]) {
    fn check(sql: &str, v: &Value) {
        match v {
            Value::Text(s) => {
                for marker in [
                    "Time64(",
                    "Timestamp(",
                    "Interval {",
                    "Array",
                    "List(",
                    "Struct(",
                    "Map(",
                    "Union(",
                    "Enum(",
                ] {
                    assert!(!s.contains(marker), "{sql}: Debug text {s:?}");
                }
            }
            Value::Array(items) => items.iter().for_each(|v| check(sql, v)),
            _ => {}
        }
    }
    rows.iter().flatten().for_each(|v| check(sql, v));
}

/// The rows after a full batch that were `UNSUPPORTED_TYPE` before Task 16
/// (TIME_NS) now stream: a full batch, then the final one. The error path
/// (a batch, then the error) is covered in `driver.rs`'s unit tests, since
/// no SQL value fails to decode any more.
#[test]
fn stream_reads_time_ns_after_a_full_batch() {
    run(stream_reads_time_ns_after_a_full_batch_body());
}

async fn stream_reads_time_ns_after_a_full_batch_body() {
    let driver = open().await;
    let items = stream_all(
        &*driver,
        "SELECT CASE WHEN i < 6000 THEN NULL ELSE TIME_NS '12:00:00' END AS t FROM range(7000) r(i)",
    )
    .await;
    assert_eq!(items.len(), 2, "{items:?}");
    let first = items[0].as_ref().unwrap();
    assert_eq!(first.columns, Some(vec!["t".to_string()]));
    assert_eq!(first.rows.len(), 5000);
    assert!(!first.is_final);
    let last = items[1].as_ref().unwrap();
    assert!(last.is_final);
    assert_eq!(last.rows.len(), 2000);
    assert_eq!(last.rows[1999], vec![Value::Text("12:00:00".into())]);
    assert_usable(&*driver).await;
}

/// A long query doesn't block the runtime, and dropping its future
/// interrupts it: the connection is free again promptly.
#[test]
fn dropped_query_is_interrupted() {
    run(dropped_query_is_interrupted_body());
}

async fn dropped_query_is_interrupted_body() {
    let driver = open().await;
    tokio::select! {
        r = driver.query(ENDLESS, vec![]) => panic!("finished: {r:?}"),
        _ = sleep(Duration::from_millis(200)) => {}
    }
    let dropped = Instant::now();
    assert_usable(&*driver).await;
    let latency = dropped.elapsed();
    eprintln!("query: connection free {latency:?} after the drop");
    assert!(latency < CANCEL_BUDGET, "{latency:?}");
}

/// Cancelling the way Core does (`take_until` on the token, then dropping
/// the stream) interrupts the query.
#[test]
fn cancelled_stream_is_interrupted() {
    run(cancelled_stream_is_interrupted_body());
}

async fn cancelled_stream_is_interrupted_body() {
    let driver = open().await;
    let token = CancellationToken::new();
    let consume = driver
        .query_stream(ENDLESS.to_string(), vec![], token.clone())
        .take_until(token.clone().cancelled_owned())
        .collect::<Vec<_>>();
    let cancel = async {
        sleep(Duration::from_millis(200)).await;
        token.cancel();
        Instant::now()
    };
    let (items, cancelled) = tokio::join!(consume, cancel);
    assert!(items.is_empty(), "{items:?}");
    assert_usable(&*driver).await;
    let latency = cancelled.elapsed();
    eprintln!("query_stream: connection free {latency:?} after the cancel");
    assert!(latency < CANCEL_BUDGET, "{latency:?}");
}

/// Dropping a call that's still waiting for the connection must not
/// interrupt the call that holds it.
#[test]
fn dropping_a_waiting_call_leaves_the_running_one_alone() {
    run(dropping_a_waiting_call_leaves_the_running_one_alone_body());
}

async fn dropping_a_waiting_call_leaves_the_running_one_alone_body() {
    let driver = open().await;
    // Not constant-folded: it has to scan the range.
    let started = Instant::now();
    let running = async {
        let r = driver
            .query("SELECT sum(i % 7) AS n FROM range(200000000) t(i)", vec![])
            .await;
        (r, started.elapsed())
    };
    let waiting = async {
        sleep(Duration::from_millis(50)).await;
        timeout(Duration::from_millis(100), driver.query("SELECT 1", vec![])).await
    };
    let ((running, took), waiting) = tokio::join!(running, waiting);
    eprintln!("waiting-call test: the running query took {took:?}");
    assert!(
        took > Duration::from_millis(250),
        "the running query must outlast the waiting one's timeout: {took:?}"
    );
    assert!(waiting.is_err(), "should still be waiting: {waiting:?}");
    let r = running.unwrap();
    // 28,571,428 runs of 0..=6, then 0, 1, 2, 3. ~3.3 s in debug, ~0.4 s
    // in release.
    assert_eq!(r.rows[0][0].as_i64(), Some(28_571_428 * 21 + 6));
    assert_usable(&*driver).await;
}

/// A query dropped while DuckDB is still binding it (here: sniffing a whole
/// CSV file, `sample_size = -1`) doesn't run afterwards. The execution
/// would never end on its own.
///
/// An interrupt during the bind makes DuckDB fail the prepare itself, so
/// this passes even without the driver's check after prepare; the unit test
/// `cancel_that_lands_before_execute_is_not_lost` (driver.rs) covers that.
#[test]
fn query_dropped_during_bind_does_not_run() {
    run(query_dropped_during_bind_does_not_run_body());
}

async fn query_dropped_during_bind_does_not_run_body() {
    /// Removes the CSV's directory, also when the test fails.
    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let driver = open().await;
    let dir =
        TempDir(std::env::temp_dir().join(format!("seaquel-duckdb-bind-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let csv = dir.0.join("big.csv");
    let csv = csv.to_str().unwrap().replace('\'', "''");
    driver
        .execute(
            &format!(
                "COPY (SELECT i, i * 2 AS j, 'row ' || i AS s FROM range(1000000) t(i)) \
                 TO '{csv}' (HEADER)"
            ),
            vec![],
        )
        .await
        .unwrap();
    let sql =
        format!("SELECT (SELECT count(*) FROM read_csv('{csv}', sample_size = -1)) + ({ENDLESS})");

    // How long the bind takes (~2.9 s in debug, ~0.4 s in release), so the
    // drop below lands in the middle of it.
    let bind = Instant::now();
    let r = driver.query(&format!("PREPARE p AS {sql}"), vec![]).await;
    let bind = bind.elapsed();
    assert!(r.is_ok(), "{r:?}");
    assert!(
        bind > Duration::from_millis(100),
        "bind too fast to test: {bind:?}"
    );

    let r = timeout(bind / 2, driver.query(&sql, vec![])).await;
    assert!(r.is_err(), "should have timed out: {r:?}");
    let dropped = Instant::now();
    assert_usable(&*driver).await;
    let latency = dropped.elapsed();
    eprintln!("dropped during bind ({bind:?}): connection free {latency:?} after the drop");
    assert!(latency < bind + CANCEL_BUDGET, "{latency:?}");
}

/// Dropping a transaction mid-statement interrupts it and rolls back, and
/// leaves no transaction open.
#[test]
fn dropped_transaction_rolls_back() {
    run(dropped_transaction_rolls_back_body());
}

async fn dropped_transaction_rolls_back_body() {
    let driver = open().await;
    driver
        .execute("CREATE TABLE t (n BIGINT)", vec![])
        .await
        .unwrap();
    let statements = vec![
        BatchStatement {
            sql: "INSERT INTO t VALUES (1)".to_string(),
            params: vec![],
            expect_rows: None,
        },
        BatchStatement {
            sql: format!("INSERT INTO t {ENDLESS}"),
            params: vec![],
            expect_rows: None,
        },
    ];
    let r = timeout(Duration::from_millis(200), driver.transaction(statements)).await;
    assert!(r.is_err(), "should have timed out: {r:?}");
    let dropped = Instant::now();

    let count = timeout(STUCK, driver.query("SELECT count(*) FROM t", vec![]))
        .await
        .expect("the connection is still busy")
        .unwrap();
    let latency = dropped.elapsed();
    eprintln!("transaction: connection free {latency:?} after the drop");
    assert!(latency < CANCEL_BUDGET, "{latency:?}");
    assert_eq!(count.rows[0][0].as_i64(), Some(0));

    // No transaction left open: a new one begins and commits.
    driver
        .transaction(vec![BatchStatement {
            sql: "INSERT INTO t VALUES (5)".to_string(),
            params: vec![],
            expect_rows: None,
        }])
        .await
        .unwrap();
    let count = driver
        .query("SELECT count(*) FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(count.rows[0][0].as_i64(), Some(1));
}

/// Many calls at once take turns on the one connection.
#[test]
fn concurrent_calls_take_turns() {
    run(concurrent_calls_take_turns_body());
}

async fn concurrent_calls_take_turns_body() {
    let driver = open().await;
    let calls = (0..20).map(|i| {
        let driver = driver.clone();
        async move { driver.query(&format!("SELECT {i} AS n"), vec![]).await }
    });
    let results = futures::future::join_all(calls).await;
    for (i, r) in results.into_iter().enumerate() {
        assert_eq!(r.unwrap().rows, vec![vec![Value::Int(i as i64)]]);
    }
}
