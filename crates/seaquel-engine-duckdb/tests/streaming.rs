//! Streaming execution for `query_stream`:
//! DuckDB hands out a result's chunks as it produces them, instead of
//! materializing the whole result before the first one. So the first batch
//! arrives early, a query failing late fails after the batches before it,
//! and a cancel after the first batch interrupts a query that is still
//! running.
//!
//! The connections run with one thread, so a result's rows come in order
//! and a late failure is late. Each test runs through [`run`], as in
//! `live.rs`: a query that wasn't interrupted keeps its blocking thread, and
//! the runtime is shut down without waiting for it.

#[path = "common/engine.rs"]
mod engine_switch;

use std::future::Future;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

#[path = "common/cells.rs"]
#[allow(dead_code)]
mod cells;

use futures::StreamExt;
use seaquel_engine::{CancellationToken, Driver, Value};
use seaquel_engine_testkit::same_value;
use tokio::time::{timeout, Instant};

/// How long a test waits for the first batch. A materialized result of the
/// queries below takes far longer; a streamed one comes once DuckDB has
/// filled its 1 MB streaming buffer (0.4 s for the slowest below in a debug
/// build).
const FIRST_BATCH: Duration = Duration::from_secs(5);

/// How long a cancelled stream may take to end and give the connection
/// back.
const CANCEL_BUDGET: Duration = Duration::from_secs(1);

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
        "path": ":memory:",
        "duckdb_config": { "threads": "1" }
    }))
    .unwrap();
    engine_switch::engine().open(&config).await.unwrap()
}

/// The connection answers a query within [`CANCEL_BUDGET`].
async fn assert_usable(driver: &dyn Driver) {
    let r = timeout(CANCEL_BUDGET, driver.query("SELECT 42 AS n", vec![]))
        .await
        .expect("the connection is still busy")
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(42)]]);
}

/// 50M rows, each hashing a kilobyte: minutes to produce in full, well under
/// a second for the first batch.
#[test]
fn first_batch_arrives_before_the_query_finishes() {
    run(first_batch_arrives_before_the_query_finishes_body());
}

async fn first_batch_arrives_before_the_query_finishes_body() {
    let driver = open().await;
    let started = Instant::now();
    let mut stream = driver.query_stream(
        "SELECT i, md5(i::VARCHAR || repeat('x', 1000)) AS h FROM range(50000000) t(i)".to_string(),
        vec![],
        CancellationToken::new(),
    );
    let first = timeout(FIRST_BATCH, stream.next())
        .await
        .expect("no batch before the whole result was produced")
        .expect("the stream ended")
        .unwrap();
    eprintln!("first batch after {:?}", started.elapsed());
    assert!(!first.is_final);
    assert_eq!(first.columns, Some(vec!["i".to_string(), "h".to_string()]));
    assert!(!first.rows.is_empty() && first.rows.len() < 50_000_000);
    assert_eq!(first.rows[0][0], Value::Int(0));
    drop(stream);
    assert_usable(&*driver).await;
}

/// A query that fails after its first chunks sends those rows first, then
/// the error, which ends the stream. DuckDB runs a streamed query ahead of
/// its reader until `streaming_buffer_size` bytes are buffered (1 MB by
/// default, 10^6 bytes: about 125,000 BIGINTs, which would swallow a failure at row
/// 30,000), so the test sets it to 64 KB (a session setting: DuckDB refuses
/// it in the open config), and checks narrow and wide rows alike: the
/// buffer counts bytes, not rows.
#[test]
fn a_query_failing_late_yields_batches_then_the_error() {
    run(a_query_failing_late_yields_batches_then_the_error_body());
}

async fn a_query_failing_late_yields_batches_then_the_error_body() {
    let driver = open().await;
    driver
        .execute("SET streaming_buffer_size = '64KB'", vec![])
        .await
        .unwrap();
    for (extra, width) in [("", 1), (", repeat('x', 1000) AS w", 2)] {
        let items: Vec<_> = driver
            .query_stream(
                format!(
                    "SELECT CASE WHEN i < 30000 THEN i ELSE error('late failure') END AS v{extra} \
                     FROM range(1000000) t(i)"
                ),
                vec![],
                CancellationToken::new(),
            )
            .collect()
            .await;
        let (last, batches) = items.split_last().expect("no items");
        let e = last
            .as_ref()
            .expect_err("the stream didn't end with the error");
        assert!(e.message.contains("late failure"), "{}", e.message);
        assert!(
            !batches.is_empty(),
            "no batch before the error ({width} columns)"
        );
        let mut next = 0i64;
        for batch in batches {
            let batch = batch.as_ref().unwrap();
            assert!(!batch.is_final);
            for row in &batch.rows {
                assert_eq!(row.len(), width);
                assert_eq!(row[0], Value::Int(next));
                next += 1;
            }
        }
        assert!(next > 0 && next <= 30000, "{next} rows");
    }
    assert_usable(&*driver).await;
}

/// Every typed-cell case (`common/cells.rs`, the set `tests/fixtures/
/// cells.json` records) reads the same through `query_stream`, whose
/// result is executed in streaming mode, as through `query`, which
/// materializes it: the same columns and the same cells.
#[test]
fn typed_cells_stream_as_they_query() {
    run(typed_cells_stream_as_they_query_body());
}

async fn typed_cells_stream_as_they_query_body() {
    let driver = open().await;
    let mut failures = Vec::new();
    let cases = cells::cases();
    for case in &cases {
        for sql in &case.setup {
            driver.execute(sql, vec![]).await.unwrap();
        }
        let queried = driver.query(&case.select, vec![]).await.unwrap();
        let items: Vec<_> = driver
            .query_stream(case.select.clone(), vec![], CancellationToken::new())
            .collect()
            .await;
        let mut columns = None;
        let mut rows = Vec::new();
        for item in items {
            match item {
                Ok(batch) => {
                    columns = columns.or(batch.columns);
                    rows.extend(batch.rows);
                }
                Err(e) => failures.push(format!("{}: {e:?}", case.name)),
            }
        }
        let same = rows.len() == queried.rows.len()
            && rows
                .iter()
                .zip(&queried.rows)
                .all(|(a, b)| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(x, y)));
        if columns.as_ref() != Some(&queried.columns) || !same {
            failures.push(format!(
                "{}:\n  query:  {:?} {:?}\n  stream: {:?} {:?}",
                case.name, queried.columns, queried.rows, columns, rows
            ));
        }
        for sql in &case.teardown {
            driver.execute(sql, vec![]).await.unwrap();
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// A cancel after the first batch of a 3e9-row result interrupts DuckDB:
/// the stream ends and the connection is free again promptly.
#[test]
fn cancel_after_the_first_batch_interrupts() {
    run(cancel_after_the_first_batch_interrupts_body());
}

async fn cancel_after_the_first_batch_interrupts_body() {
    let driver = open().await;
    let token = CancellationToken::new();
    let mut stream = Box::pin(
        driver
            .query_stream(
                "SELECT i, md5(i::VARCHAR) AS h FROM range(3000000000) t(i)".to_string(),
                vec![],
                token.clone(),
            )
            .take_until(token.clone().cancelled_owned()),
    );
    let first = timeout(FIRST_BATCH, stream.next())
        .await
        .expect("no batch before the whole result was produced")
        .expect("the stream ended")
        .unwrap();
    assert!(!first.rows.is_empty());

    token.cancel();
    let cancelled = Instant::now();
    let rest = timeout(CANCEL_BUDGET, stream.collect::<Vec<_>>())
        .await
        .expect("the stream didn't end after the cancel");
    assert!(rest.iter().all(|item| item.is_ok()), "{rest:?}");
    assert_usable(&*driver).await;
    eprintln!(
        "cancelled stream: connection free {:?} after the cancel",
        cancelled.elapsed()
    );
}

/// Rows that span many chunks arrive whole and in order, in 5,000-row
/// batches, the last one marked final.
#[test]
fn a_streamed_result_arrives_whole() {
    run(a_streamed_result_arrives_whole_body());
}

async fn a_streamed_result_arrives_whole_body() {
    let driver = open().await;
    let items: Vec<_> = driver
        .query_stream(
            "SELECT i FROM range(12345) t(i)".to_string(),
            vec![],
            CancellationToken::new(),
        )
        .collect()
        .await;
    let batches: Vec<_> = items.into_iter().map(Result::unwrap).collect();
    assert_eq!(
        batches.iter().map(|b| b.rows.len()).collect::<Vec<_>>(),
        vec![5000, 5000, 2345]
    );
    assert_eq!(batches[0].columns, Some(vec!["i".to_string()]));
    assert!(batches[1..].iter().all(|b| b.columns.is_none()));
    assert!(batches.last().unwrap().is_final);
    let all: Vec<_> = batches.iter().flat_map(|b| b.rows.iter()).collect();
    assert!(all
        .iter()
        .enumerate()
        .all(|(i, row)| row == &&vec![Value::Int(i as i64)]));

    // An empty result still names its columns, on the final batch.
    let items: Vec<_> = driver
        .query_stream(
            "SELECT 1 AS a, 'b' AS b WHERE false".to_string(),
            vec![],
            CancellationToken::new(),
        )
        .collect()
        .await;
    assert_eq!(items.len(), 1);
    let only = items.into_iter().next().unwrap().unwrap();
    assert!(only.is_final && only.rows.is_empty());
    assert_eq!(only.columns, Some(vec!["a".to_string(), "b".to_string()]));
}

/// What a statement with side effects streamed through `query_stream` and
/// dropped after its first batch leaves behind. A write with `RETURNING`
/// finishes before its first chunk (DuckDB's insert is a sink) and stays,
/// in autocommit and in a transaction opened by hand alike. Side effects in
/// a SELECT's expressions (`nextval`) happen only for the rows DuckDB
/// produced before the drop. Pinned so the `Execution::Streaming` docs stay
/// true.
#[test]
fn side_effects_of_a_stream_dropped_after_its_first_batch() {
    run(side_effects_of_a_stream_dropped_after_its_first_batch_body());
}

async fn side_effects_of_a_stream_dropped_after_its_first_batch_body() {
    let driver = open().await;
    let first_batch_then_drop = |sql: &'static str| {
        let driver = driver.clone();
        async move {
            let mut stream = driver.query_stream(sql.to_string(), vec![], CancellationToken::new());
            let first = timeout(FIRST_BATCH, stream.next())
                .await
                .expect("no first batch")
                .expect("the stream ended")
                .unwrap();
            assert!(!first.rows.is_empty());
        }
    };
    let number = |sql: &'static str| {
        let driver = driver.clone();
        async move {
            driver.query(sql, vec![]).await.unwrap().rows[0][0]
                .as_i64()
                .unwrap()
        }
    };

    driver
        .execute("CREATE TABLE t (i BIGINT)", vec![])
        .await
        .unwrap();
    let insert = "INSERT INTO t SELECT i FROM range(1000000) r(i) RETURNING i";
    first_batch_then_drop(insert).await;
    assert_eq!(number("SELECT count(*) FROM t").await, 1_000_000);

    driver.execute("BEGIN", vec![]).await.unwrap();
    first_batch_then_drop(insert).await;
    assert_eq!(number("SELECT count(*) FROM t").await, 2_000_000);
    driver.execute("COMMIT", vec![]).await.unwrap();
    assert_eq!(number("SELECT count(*) FROM t").await, 2_000_000);

    driver.execute("CREATE SEQUENCE s", vec![]).await.unwrap();
    first_batch_then_drop("SELECT nextval('s') AS n FROM range(100000000)").await;
    let advanced = number("SELECT currval('s')").await;
    assert!(
        (5_000..100_000_000).contains(&advanced),
        "the sequence advanced {advanced} times"
    );
    assert_usable(&*driver).await;
}
