//! Live tests of the MSSQL driver beyond the shared smoke suite: positional
//! cells, columns of empty results, transactions and multiple result sets.
//! They read SEAQUEL_TEST_MSSQL (see `smoke.rs`) and skip when it's unset.

use std::panic::AssertUnwindSafe;
use std::time::Duration;
use tokio::time::Instant;

use futures::{FutureExt, StreamExt};
use seaquel_engine::{BatchStatement, CancellationToken, Driver, StreamBatch, Value};
use seaquel_engine_mssql::{MssqlDriver, ResultSet};

mod common;

use common::{ids, insert, ints, open, trancount, with_table};

// ── Positional cells ─────────────────────────────────────────────────────────

#[tokio::test]
async fn duplicate_column_names_keep_their_own_values() {
    let Some(driver) = open().await else { return };

    let r = driver
        .query(
            "SELECT a.id, b.id FROM (VALUES (1)) a(id) CROSS JOIN (VALUES (2)) b(id)",
            vec![],
        )
        .await
        .expect("join");
    assert_eq!(r.columns, vec!["id", "id"]);
    assert_eq!(r.rows, vec![ints(&[1, 2])]);

    // Same name, different types.
    let r = driver
        .query("SELECT 'x' AS v, 5 AS v", vec![])
        .await
        .expect("mixed");
    assert_eq!(r.columns, vec!["v", "v"]);
    assert_eq!(r.rows, vec![vec![Value::from("x"), Value::Int(5)]]);
}

#[tokio::test]
async fn unnamed_constants_keep_their_own_values() {
    let Some(driver) = open().await else { return };

    let r = driver
        .query("SELECT 1, 2, NULL, 'z'", vec![])
        .await
        .expect("constants");
    assert_eq!(r.columns, vec!["", "", "", ""]);
    assert_eq!(
        r.rows,
        vec![vec![
            Value::Int(1),
            Value::Int(2),
            Value::Null,
            Value::from("z")
        ]]
    );
}

#[tokio::test]
async fn empty_results_still_have_columns() {
    let Some(driver) = open().await else { return };

    let r = driver
        .query("SELECT 1 AS a, 'x' AS b WHERE 1 = 0", vec![])
        .await
        .expect("empty constant select");
    assert_eq!(r.columns, vec!["a", "b"]);
    assert!(r.rows.is_empty());

    with_table(&driver, |driver, table| async move {
        let r = driver
            .query(&format!("SELECT * FROM {table}"), vec![])
            .await
            .expect("empty table");
        assert_eq!(r.columns, vec!["id", "label"]);
        assert!(r.rows.is_empty());

        // Streaming carries the columns too.
        let batches: Vec<StreamBatch> = driver
            .query_stream(
                format!("SELECT id, label FROM {table}"),
                vec![],
                CancellationToken::new(),
            )
            .map(|b| b.expect("stream batch"))
            .collect()
            .await;
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0].columns,
            Some(vec!["id".to_string(), "label".to_string()])
        );
        assert!(batches[0].rows.is_empty());
        assert!(batches[0].is_final);
    })
    .await;

    // A statement with no result set at all has no columns.
    let r = driver
        .query("DECLARE @x INT = 1", vec![])
        .await
        .expect("no result set");
    assert!(r.columns.is_empty());
    assert!(r.rows.is_empty());
}

// ── Transactions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn transaction_commits() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        driver
            .transaction(vec![
                insert(&table, Value::Int(1), "one"),
                insert(&table, Value::Int(2), "two"),
                BatchStatement {
                    sql: format!("UPDATE {table} SET label = @P1 WHERE id = @P2"),
                    params: vec![Value::from("TWO"), Value::Int(2)],
                    expect_rows: None,
                },
            ])
            .await
            .expect("transaction");
        let r = driver
            .query(
                &format!("SELECT id, label FROM {table} ORDER BY id"),
                vec![],
            )
            .await
            .expect("SELECT");
        assert_eq!(
            r.rows,
            vec![
                vec![Value::Int(1), Value::from("one")],
                vec![Value::Int(2), Value::from("TWO")],
            ]
        );
        assert_eq!(trancount(driver).await, 0);

        // An empty transaction is fine.
        driver.transaction(vec![]).await.expect("empty transaction");
        assert_eq!(trancount(driver).await, 0);
    })
    .await;
}

#[tokio::test]
async fn failed_transactions_roll_back_and_leave_the_connection_usable() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        driver
            .execute(
                &format!("INSERT INTO {table} (id, label) VALUES (1, 'one')"),
                vec![],
            )
            .await
            .expect("seed");

        // (name, the failing statement). Each follows a good insert of id 10.
        let failures = [
            // A statement-level error: the transaction stays open, so the
            // driver's ROLLBACK undoes the first insert.
            ("duplicate key", insert(&table, Value::Int(1), "dup")),
            // A batch-aborting conversion error.
            (
                "conversion",
                insert(&table, Value::from("not a number"), "bad"),
            ),
            // XACT_ABORT ON: the server rolls back itself, so the driver's
            // rollback finds @@TRANCOUNT = 0 and must not fail.
            (
                "xact_abort",
                BatchStatement {
                    sql: format!(
                        "SET XACT_ABORT ON; INSERT INTO {table} (id, label) VALUES (1, 'dup')"
                    ),
                    params: vec![],
                    expect_rows: None,
                },
            ),
            // A statement that ends the transaction itself.
            (
                "rollback inside",
                BatchStatement {
                    sql: "ROLLBACK TRANSACTION".to_string(),
                    params: vec![],
                    expect_rows: None,
                },
            ),
            // An array parameter fails to bind.
            (
                "bind",
                BatchStatement {
                    sql: format!("INSERT INTO {table} (id) VALUES (@P1)"),
                    params: vec![Value::Array(vec![Value::Int(1)])],
                    expect_rows: None,
                },
            ),
        ];
        for (name, failing) in failures {
            let err = driver
                .transaction(vec![insert(&table, Value::Int(10), "ten"), failing])
                .await
                .expect_err(name);
            let expected_code = if name == "bind" {
                "QUERY_ERROR"
            } else {
                "EXECUTE_ERROR"
            };
            assert_eq!(err.code, expected_code, "{name}: {}", err.message);
            assert_eq!(
                ids(driver, &table).await,
                ints(&[1]),
                "{name}: no partial writes"
            );
            assert_eq!(trancount(driver).await, 0, "{name}: no open transaction");

            // Still usable, for queries and for another transaction.
            let one = driver.query("SELECT 1 AS one", vec![]).await.expect(name);
            assert_eq!(one.rows, vec![ints(&[1])], "{name}");
        }

        driver
            .transaction(vec![insert(&table, Value::Int(2), "two")])
            .await
            .expect("transaction after failures");
        assert_eq!(ids(driver, &table).await, ints(&[1, 2]));
    })
    .await;
}

// ── Callers that go away ─────────────────────────────────────────────────────
//
// A future dropped halfway (hyper drops a handler when its client
// disconnects) leaves the connection dirty; the next call reconnects.

/// The connection works, has no open transaction, and `table` holds `ids`.
async fn assert_usable(driver: &MssqlDriver, table: &str, expected: &[i64], what: &str) {
    let started = Instant::now();
    let one = driver
        .query("SELECT 1 AS one", vec![])
        .await
        .unwrap_or_else(|e| panic!("{what}: next query: {e:?}"));
    assert_eq!(one.rows, vec![ints(&[1])], "{what}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{what}: the next query waited {:?}",
        started.elapsed()
    );
    assert_eq!(trancount(driver).await, 0, "{what}: no open transaction");
    assert_eq!(ids(driver, table).await, ints(expected), "{what}: rows");
}

#[tokio::test]
async fn a_transaction_dropped_after_begin_is_sent_is_rolled_back() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        // One poll sends BEGIN and returns Pending before its reply is read.
        let polled = driver
            .transaction(vec![insert(&table, Value::Int(1), "one")])
            .now_or_never();
        assert!(polled.is_none(), "the transaction should still be running");
        assert_usable(driver, &table, &[], "BEGIN window").await;

        driver
            .transaction(vec![insert(&table, Value::Int(2), "two")])
            .await
            .expect("the next transaction");
        assert_usable(driver, &table, &[2], "after the next transaction").await;
    })
    .await;
}

#[tokio::test]
async fn a_transaction_dropped_at_any_point_is_all_or_nothing() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        let mut committed: Vec<i64> = Vec::new();
        for (round, micros) in [
            0u64, 200, 500, 1_000, 2_000, 3_000, 5_000, 10_000, 20_000, 50_000,
        ]
        .into_iter()
        .enumerate()
        {
            let id = 10 * round as i64;
            let statements = vec![
                insert(&table, Value::Int(id), "a"),
                insert(&table, Value::Int(id + 1), "b"),
                insert(&table, Value::Int(id + 2), "c"),
            ];
            let outcome = tokio::time::timeout(
                Duration::from_micros(micros),
                driver.transaction(statements),
            )
            .await;
            let what = format!("dropped after {micros} µs");
            match outcome {
                Ok(done) => {
                    done.expect("a transaction that finished");
                    committed.extend([id, id + 1, id + 2]);
                }
                // Dropped after COMMIT was sent, before its reply: the
                // server may have committed, but only all three rows.
                Err(_) if ids(driver, &table).await.len() > committed.len() => {
                    committed.extend([id, id + 1, id + 2]);
                    eprintln!("{what}: committed before the drop");
                }
                Err(_) => {}
            }
            assert_usable(driver, &table, &committed, &what).await;
        }
    })
    .await;
}

#[tokio::test]
async fn a_transaction_dropped_mid_statement_is_rolled_back_without_waiting() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        // The first insert has run and holds its lock when the caller gives
        // up during the WAITFOR. The next call must neither see the row nor
        // wait for the server to finish the batch.
        let slow = BatchStatement {
            sql: "WAITFOR DELAY '00:00:05'".to_string(),
            params: vec![],
            expect_rows: None,
        };
        let dropped = tokio::time::timeout(
            Duration::from_millis(300),
            driver.transaction(vec![insert(&table, Value::Int(1), "one"), slow]),
        )
        .await;
        assert!(dropped.is_err(), "the transaction should still be running");
        assert_usable(driver, &table, &[], "mid-statement").await;
    })
    .await;
}

#[tokio::test]
async fn a_long_query_dropped_does_not_block_the_next_one() {
    let Some(driver) = open().await else { return };
    let dropped = tokio::time::timeout(
        Duration::from_millis(200),
        driver.query("WAITFOR DELAY '00:00:05'; SELECT 1 AS a", vec![]),
    )
    .await;
    assert!(dropped.is_err(), "the query should still be running");
    let started = Instant::now();
    let r = driver
        .query("SELECT 2 AS b", vec![])
        .await
        .expect("next query");
    assert_eq!(r.rows, vec![ints(&[2])]);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "waited {:?}",
        started.elapsed()
    );
}

/// The first result cell of `sql` on `driver`, as an integer or text.
async fn cell(driver: &MssqlDriver, sql: &str, params: Vec<Value>) -> Value {
    let r = driver.query(sql, params).await.expect(sql);
    r.rows
        .into_iter()
        .next()
        .map_or(Value::Null, |mut row| row.remove(0))
}

/// Polls `request` (which sends a large `UPDATE {table} SET label = …
/// WHERE id = 1`) until the server, seen from `observer`, has received part
/// of the request but hasn't run it, then drops it. `spid` is the
/// connection sending it. Panics if that state isn't reached, so the test
/// can't pass without exercising a half-sent request.
async fn drop_mid_send<F: std::future::Future>(
    request: F,
    observer: &MssqlDriver,
    spid: i64,
    table: &str,
) {
    let reads = || async {
        cell(
            observer,
            "SELECT num_reads FROM sys.dm_exec_connections WHERE session_id = @P1",
            vec![Value::Int(spid)],
        )
        .await
        .as_i64()
        .expect("num_reads")
    };
    // READ UNCOMMITTED: the row a transaction inserted is visible, locked
    // or not.
    let label = || async {
        cell(
            observer,
            &format!("SELECT label FROM {table} WITH (NOLOCK) WHERE id = 1"),
            vec![],
        )
        .await
    };
    let mut request = std::pin::pin!(request);
    let mut baseline: Option<i64> = None;
    for _ in 0..500 {
        if futures::poll!(request.as_mut()).is_ready() {
            panic!("the request finished before it could be dropped mid-send");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        let label = label().await;
        if label == Value::Null {
            continue; // the transaction's insert hasn't run yet
        }
        assert_eq!(
            label,
            Value::from("one"),
            "the UPDATE ran: the request was sent in full"
        );
        let now = reads().await;
        match baseline {
            None => baseline = Some(now),
            // The server has read more of the connection, but the UPDATE
            // hasn't run: the request is half sent.
            Some(before) if now > before => return,
            Some(_) => {}
        }
    }
    panic!("never saw the request half sent");
}

#[tokio::test]
async fn a_large_request_dropped_mid_send_leaves_the_connection_usable() {
    let Some(driver) = open().await else { return };
    let Some(observer) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        // 32 MB (UTF-16) of parameter: far more than the socket buffers, so
        // the send takes many polls.
        let big = Value::from("x".repeat(16 * 1024 * 1024));
        let update = BatchStatement {
            sql: format!("UPDATE {table} SET label = LEFT(@P1, 10) WHERE id = 1"),
            params: vec![big],
            expect_rows: None,
        };

        // Inside a transaction, after its first insert.
        let spid = cell(driver, "SELECT @@SPID AS spid", vec![])
            .await
            .as_i64()
            .unwrap();
        drop_mid_send(
            driver.transaction(vec![
                insert(&table, Value::Int(1), "one"),
                BatchStatement {
                    sql: update.sql.clone(),
                    params: update.params.clone(),
                    expect_rows: None,
                },
            ]),
            &observer,
            spid,
            &table,
        )
        .await;
        assert_usable(driver, &table, &[], "transaction mid-send").await;

        // On its own.
        driver
            .execute(
                &format!("INSERT INTO {table} (id, label) VALUES (1, 'one')"),
                vec![],
            )
            .await
            .expect("seed");
        let spid = cell(driver, "SELECT @@SPID AS spid", vec![])
            .await
            .as_i64()
            .unwrap();
        drop_mid_send(
            driver.execute(&update.sql, update.params.clone()),
            &observer,
            spid,
            &table,
        )
        .await;
        assert_usable(driver, &table, &[1], "execute mid-send").await;
        assert_eq!(
            cell(
                driver,
                &format!("SELECT label FROM {table} WHERE id = 1"),
                vec![]
            )
            .await,
            Value::from("one")
        );
    })
    .await;
}

#[tokio::test]
async fn a_dropped_transaction_releases_its_locks_at_once() {
    let Some(driver) = open().await else { return };
    let Some(observer) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        let slow = BatchStatement {
            sql: "WAITFOR DELAY '00:00:10'".to_string(),
            params: vec![],
            expect_rows: None,
        };
        let dropped = tokio::time::timeout(
            Duration::from_millis(300),
            driver.transaction(vec![insert(&table, Value::Int(1), "one"), slow]),
        )
        .await;
        assert!(dropped.is_err(), "the transaction should still be running");

        // No further call on `driver`: another connection can read the row
        // the dropped transaction had locked, and it's gone.
        let started = Instant::now();
        let r = observer
            .query(
                &format!("SET LOCK_TIMEOUT 1000; SELECT id FROM {table} WHERE id = 1"),
                vec![],
            )
            .await
            .expect("read the formerly locked row (1222 = still locked)");
        assert!(r.rows.is_empty(), "the insert must be rolled back");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "waited {:?}",
            started.elapsed()
        );
    })
    .await;
}

// ── Multiple result sets ─────────────────────────────────────────────────────

const THREE_SETS: &str = "SELECT 1 AS a; SELECT 'x' AS b, 2 AS c UNION ALL SELECT 'y', 3; \
                          SELECT 4 AS d WHERE 1 = 0";

#[tokio::test]
async fn query_returns_the_first_result_set_and_query_results_all_of_them() {
    let Some(driver) = open().await else { return };

    let first = driver.query(THREE_SETS, vec![]).await.expect("query");
    assert_eq!(first.columns, vec!["a"]);
    assert_eq!(first.rows, vec![ints(&[1])]);

    let all = driver
        .query_results(THREE_SETS, vec![])
        .await
        .expect("query_results");
    assert_eq!(
        all,
        vec![
            ResultSet {
                columns: vec!["a".into()],
                rows: vec![ints(&[1])]
            },
            ResultSet {
                columns: vec!["b".into(), "c".into()],
                rows: vec![
                    vec![Value::from("x"), Value::Int(2)],
                    vec![Value::from("y"), Value::Int(3)],
                ],
            },
            ResultSet {
                columns: vec!["d".into()],
                rows: vec![]
            },
        ]
    );

    // Parameters work across the sets.
    let all = driver
        .query_results(
            "SELECT @P1 AS p; SELECT @P2 AS q",
            vec![Value::Int(7), Value::from("s")],
        )
        .await
        .expect("parameters");
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].rows, vec![ints(&[7])]);
    assert_eq!(all[1].rows, vec![vec![Value::from("s")]]);

    // Statements without a result set don't count.
    let r = driver
        .query(
            "DECLARE @t TABLE (x INT); INSERT INTO @t VALUES (5); SELECT x FROM @t",
            vec![],
        )
        .await
        .expect("insert then select");
    assert_eq!(r.columns, vec!["x"]);
    assert_eq!(r.rows, vec![ints(&[5])]);

    // The trailing sets were drained: the next query sees its own result.
    let next = driver
        .query("SELECT 'next' AS n", vec![])
        .await
        .expect("next");
    assert_eq!(next.rows, vec![vec![Value::from("next")]]);

    // Streaming returns the first set only, too.
    let batches: Vec<StreamBatch> = driver
        .query_stream(THREE_SETS.to_string(), vec![], CancellationToken::new())
        .map(|b| b.expect("stream batch"))
        .collect()
        .await;
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].columns, Some(vec!["a".to_string()]));
    assert_eq!(batches[0].rows, vec![ints(&[1])]);
    let next = driver
        .query("SELECT 'after stream' AS n", vec![])
        .await
        .expect("after stream");
    assert_eq!(next.rows, vec![vec![Value::from("after stream")]]);
}

#[tokio::test]
async fn an_error_in_a_trailing_result_set_fails_the_query() {
    let Some(driver) = open().await else { return };

    let err = driver
        .query("SELECT 1 AS a; SELECT 1 / 0 AS b", vec![])
        .await
        .expect_err("division by zero in the second set");
    assert_eq!(err.code, "QUERY_ERROR");
    assert!(err.message.contains("Divide by zero"), "{}", err.message);

    let next = driver.query("SELECT 2 AS n", vec![]).await.expect("next");
    assert_eq!(next.rows, vec![ints(&[2])]);
}

// ── Batch policy ─────────────────────────────────────────────────────────────

/// Statements that must start a batch work through `query`, `execute` and
/// `transaction` without the `EXEC (N'…')` workaround (Task 13): tiberius
/// sends every other statement through `sp_executesql` with a parameter
/// list, where they are syntax errors.
#[tokio::test]
async fn statements_that_must_start_a_batch_run() {
    let Some(driver) = open().await else { return };
    common::drop_stale(&driver, "t13_batch_").await;
    let schema = seaquel_engine_testkit::scratch_name("t13_batch_");
    let outcome = AssertUnwindSafe(async {
        driver
            .execute(&format!("CREATE SCHEMA {schema}"), vec![])
            .await
            .expect("CREATE SCHEMA");
        // The query editor sends DDL through `query`.
        driver
            .query(&format!("CREATE VIEW {schema}.v AS SELECT 1 AS a"), vec![])
            .await
            .expect("CREATE VIEW");
        let affected = driver
            .execute(
                &format!(
                    "-- a procedure\nCREATE OR ALTER PROCEDURE {schema}.p @x INT AS SELECT @x + a AS b FROM {schema}.v"
                ),
                vec![],
            )
            .await
            .expect("CREATE PROCEDURE");
        assert_eq!(affected.rows_affected, 0);
        driver
            .transaction(vec![BatchStatement {
                sql: format!("CREATE FUNCTION {schema}.f() RETURNS INT AS BEGIN RETURN 41 END"),
                params: vec![],
                expect_rows: None,
            }])
            .await
            .expect("CREATE FUNCTION");
        let r = driver
            .query(&format!("SELECT {schema}.f() + 1 AS n"), vec![])
            .await
            .expect("call the function");
        assert_eq!(r.rows, vec![ints(&[42])]);
    })
    .catch_unwind()
    .await;
    for sql in [
        format!("DROP FUNCTION IF EXISTS {schema}.f"),
        format!("DROP PROCEDURE IF EXISTS {schema}.p"),
        format!("DROP VIEW IF EXISTS {schema}.v"),
        format!("DROP SCHEMA IF EXISTS {schema}"),
    ] {
        if let Err(e) = driver.execute(&sql, vec![]).await {
            eprintln!("{sql}: {}", e.message);
        }
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// Everything else runs through `sp_executesql`, parameters or not, so
/// session state set by one call (the query editor's `SET` or `USE`) ends
/// with it and can't reach the introspection and grid edits that share the
/// connection.
#[tokio::test]
async fn session_state_ends_with_the_call() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        driver
            .execute(
                &format!("INSERT INTO {table} (id, label) VALUES (1, N'a'), (2, N'b')"),
                vec![],
            )
            .await
            .expect("insert");
        let bound_update = |label: &'static str| {
            let sql = format!("UPDATE {table} SET label = @P1 WHERE id = @P2");
            async move {
                driver
                    .execute(&sql, vec![Value::from(label), Value::Int(1)])
                    .await
                    .expect("bound UPDATE")
                    .rows_affected
            }
        };

        // SET NOCOUNT ON would report 0 rows affected.
        driver
            .query("SET NOCOUNT ON", vec![])
            .await
            .expect("NOCOUNT");
        assert_eq!(bound_update("n").await, 1);
        let r = driver
            .execute(&format!("UPDATE {table} SET label = label"), vec![])
            .await
            .expect("parameterless UPDATE");
        assert_eq!(r.rows_affected, 2);

        // SET ROWCOUNT 1 would truncate the schema list and updates.
        driver
            .query("SET ROWCOUNT 1", vec![])
            .await
            .expect("ROWCOUNT");
        let schemas = driver.list_schemas().await.expect("list_schemas");
        assert!(schemas.len() > 1, "{schemas:?}");
        let r = driver
            .execute(
                &format!("UPDATE {table} SET label = label WHERE id > @P1"),
                vec![Value::Int(0)],
            )
            .await
            .expect("UPDATE");
        assert_eq!(r.rows_affected, 2);

        // USE master would break metadata, and a later reconnect would flip
        // the database back under a statement.
        driver.query("USE master", vec![]).await.expect("USE");
        let db = driver
            .query("SELECT DB_NAME() AS db", vec![])
            .await
            .expect("DB_NAME");
        assert_eq!(db.rows, vec![vec![Value::from("seaquel_test")]]);
        let (columns, _) = driver
            .table_metadata("dbo", &table)
            .await
            .expect("metadata");
        assert_eq!(columns.len(), 2);

        // SHOWPLAN_XML would make bound statements return plans and not run.
        let _ = driver.query("SET SHOWPLAN_XML ON", vec![]).await;
        assert_eq!(bound_update("s").await, 1);
        let r = driver
            .query(&format!("SELECT label FROM {table} WHERE id = 1"), vec![])
            .await
            .expect("SELECT");
        assert_eq!(r.rows, vec![vec![Value::from("s")]]);

        // IMPLICIT_TRANSACTIONS would leave every later write uncommitted.
        driver
            .query("SET IMPLICIT_TRANSACTIONS ON", vec![])
            .await
            .expect("IMPLICIT_TRANSACTIONS");
        assert_eq!(bound_update("i").await, 1);
        assert_eq!(trancount(driver).await, 0);

        // A syntax error is a QUERY_ERROR, and the connection is usable.
        let err = driver
            .query("SELEC 1", vec![])
            .await
            .expect_err("syntax error");
        assert_eq!(err.code, "QUERY_ERROR");
        let r = driver.query("SELECT 1 AS one", vec![]).await.expect("next");
        assert_eq!(r.rows, vec![ints(&[1])]);
    })
    .await;
}

/// `#temp` tables live as long as the call that made them, as before
/// Task 13.
#[tokio::test]
async fn temp_tables_end_with_the_call() {
    let Some(driver) = open().await else { return };
    let r = driver
        .query(
            "CREATE TABLE #t13_temp (x INT); INSERT INTO #t13_temp VALUES (5); SELECT x FROM #t13_temp",
            vec![],
        )
        .await
        .expect("one call");
    assert_eq!(r.rows, vec![ints(&[5])]);
    driver
        .query("CREATE TABLE #t13_temp (x INT)", vec![])
        .await
        .expect("create");
    let err = driver
        .query("SELECT x FROM #t13_temp", vec![])
        .await
        .expect_err("gone after its call");
    assert!(
        err.message.contains("Invalid object name"),
        "{}",
        err.message
    );
}

/// A parameterless `execute` counts affected rows.
#[tokio::test]
async fn parameterless_execute_counts_rows() {
    let Some(driver) = open().await else { return };
    with_table(&driver, |driver, table| async move {
        let r = driver
            .execute(
                &format!("INSERT INTO {table} (id, label) VALUES (1, N'a'), (2, N'b')"),
                vec![],
            )
            .await
            .expect("insert");
        assert_eq!(r.rows_affected, 2);
        let r = driver
            .execute(&format!("DELETE FROM {table}"), vec![])
            .await
            .expect("delete");
        assert_eq!(r.rows_affected, 2);
    })
    .await;
}
