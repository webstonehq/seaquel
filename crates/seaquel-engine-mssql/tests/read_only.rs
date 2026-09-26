//! `query_read_only` against SQL Server: the shared read-only harness with
//! SQL Server's attacks, the escapes the driver can only detect, and the
//! cases the harness can't express (a transaction opened by hand, a write
//! dropped mid-call, calls at the same time).
//!
//! SQL Server has no read-only transaction or session. The driver runs the
//! query in a transaction it always rolls back (nested two deep, with
//! `IMPLICIT_TRANSACTIONS` on), checks afterwards whether the query ended
//! that transaction itself, on a connection of its own that is dropped
//! after every call. The user's held session is never touched: the
//! harness's `after_each` checks that state set on it at setup is still
//! there.
//!
//! SEAQUEL_TEST_MSSQL, e.g.
//! {"driver":"mssql","host":"127.0.0.1","port":1433,"username":"sa",
//!  "password":"Seaquel_Test_123!","encrypt":true,"trust_cert":true}
//! The tests run in `seaquel_test` whatever database it names.

mod common;

use std::sync::Arc;
use std::time::Duration;

use seaquel_engine::{DbError, Driver, Value};
use seaquel_engine_mssql::MssqlDriver;
use seaquel_engine_testkit::{run_read_only, scratch_name, Attack, Check, ReadOnlySpec};

/// Scratch objects of the harness test and of the gaps test. They differ,
/// so neither test's stale cleanup drops the other's objects mid-run.
const PREFIX: &str = "sq_roa_";
const GAP_PREFIX: &str = "sq_rog_";

/// The start of the message for a query that ended the read-only
/// transaction itself.
const ESCAPED: &str = "The query ended the read-only transaction";
/// The start of the message for a query that tried to, but only unnested
/// (or nested) it: nothing was committed.
const TRIED: &str = "The query tried to commit, roll back or open a transaction";

fn text(s: &str) -> Value {
    Value::from(s)
}

async fn one_cell(driver: &MssqlDriver, sql: &str) -> Value {
    let mut r = driver.query(sql, vec![]).await.expect(sql);
    assert_eq!(r.rows.len(), 1, "{sql}");
    r.rows.remove(0).remove(0)
}

/// Objects every test in this file uses: schema `s`, table `s.t` with ids
/// 1–3, the hygiene table `s.h`, a procedure that inserts and one that
/// commits once.
fn setup(s: &str) -> Vec<String> {
    vec![
        format!("CREATE SCHEMA {s}"),
        format!("CREATE TABLE {s}.t (id INT PRIMARY KEY, label NVARCHAR(50))"),
        format!("INSERT INTO {s}.t VALUES (1, N'one'), (2, N'two'), (3, N'three')"),
        format!("CREATE TABLE {s}.h (n INT)"),
        format!("INSERT INTO {s}.h VALUES (0)"),
        format!("CREATE SEQUENCE {s}.seq AS BIGINT START WITH 1"),
        format!("CREATE PROCEDURE {s}.p_insert AS INSERT INTO {s}.t VALUES (4, N'four')"),
        format!(
            "CREATE PROCEDURE {s}.p_commit AS BEGIN \
             INSERT INTO {s}.t VALUES (5, N'five'); COMMIT TRANSACTION; END"
        ),
        format!(
            "CREATE PROCEDURE {s}.p_commit_all AS BEGIN \
             DELETE FROM {s}.t WHERE id = 1; \
             WHILE @@TRANCOUNT > 0 COMMIT TRANSACTION; END"
        ),
    ]
}

fn teardown(s: &str) -> Vec<String> {
    vec![
        format!("DROP PROCEDURE IF EXISTS {s}.p_insert"),
        format!("DROP PROCEDURE IF EXISTS {s}.p_commit"),
        format!("DROP PROCEDURE IF EXISTS {s}.p_commit_all"),
        format!("DROP VIEW IF EXISTS {s}.v"),
        format!("DROP TABLE IF EXISTS {s}.t2"),
        format!("DROP TABLE IF EXISTS {s}.t3"),
        format!("DROP TABLE IF EXISTS {s}.t"),
        format!("DROP TABLE IF EXISTS {s}.h"),
        format!("DROP SEQUENCE IF EXISTS {s}.seq"),
        format!("DROP SCHEMA IF EXISTS {s}"),
    ]
}

/// Runs `teardown` whatever happens.
async fn best_effort(driver: &MssqlDriver, sqls: &[String]) {
    for sql in sqls {
        if let Err(e) = driver.execute(sql, vec![]).await {
            eprintln!("{sql}: {}", e.message);
        }
    }
}

#[tokio::test]
async fn read_only_attacks_leave_no_trace() {
    let Some(config) = common::config() else {
        return;
    };
    // A second session: the target of `KILL`, and the one that checks a
    // session app lock isn't held after the call.
    let other = Arc::new(MssqlDriver::connect(&config).await.expect("connect"));
    common::drop_stale(&other, PREFIX).await;
    let spid = one_cell(&other, "SELECT CAST(@@SPID AS INT) AS spid")
        .await
        .as_i64()
        .expect("spid");
    let recovery = one_cell(
        &other,
        "SELECT recovery_model_desc FROM sys.databases WHERE name = DB_NAME()",
    )
    .await;
    let recovery = recovery.as_str().expect("recovery model").to_string();
    let other_recovery = if recovery == "SIMPLE" {
        "FULL"
    } else {
        "SIMPLE"
    };
    let advanced = one_cell(
        &other,
        "SELECT CAST(value AS INT) FROM sys.configurations WHERE name = 'show advanced options'",
    )
    .await
    .as_i64()
    .expect("show advanced options");

    let s = scratch_name(PREFIX);
    let db = scratch_name(PREFIX);
    let backup = format!("/tmp/{}.bak", scratch_name(PREFIX));
    let count = format!("SELECT COUNT(*) FROM {s}.t");
    let rows_unchanged = || Check::count(count.clone(), 3);
    let absent = |object: &str| {
        Check::count(
            format!("SELECT CASE WHEN OBJECT_ID(N'{object}') IS NULL THEN 0 ELSE 1 END"),
            0,
        )
    };
    let lock_free = {
        let other = other.clone();
        Check::custom("another session can take the app lock", move |_| {
            let other = other.clone();
            async move {
                let r = other
                    .query(
                        "SELECT APPLOCK_TEST('public', N'sq_ro_lock', 'Exclusive', 'Session') AS ok",
                        vec![],
                    )
                    .await
                    .map_err(|e| e.message)?;
                match r.rows[0][0].as_i64() {
                    Some(1) => Ok(()),
                    other => Err(format!("APPLOCK_TEST returned {other:?}")),
                }
            }
        })
    };

    let attacks = vec![
        // ── Allowed ──
        Attack::allowed("count", count.clone()).returns(vec![vec![Value::Int(3)]]),
        Attack::allowed(
            "WITH",
            format!("WITH c AS (SELECT id FROM {s}.t) SELECT COUNT(*) AS n FROM c"),
        )
        .returns(vec![vec![Value::Int(3)]]),
        Attack::allowed("trailing semicolon", "SELECT 1 AS a;").returns(vec![vec![Value::Int(1)]]),
        // Parameters are forwarded to the nested `sp_executesql` with the
        // types tiberius declares: NULL inlined, a decimal as numeric, text
        // over 4000 bytes as nvarchar(max), bytes as varbinary.
        Attack::allowed(
            "parameters of every kind",
            "SELECT @P1 AS n, @P2 AS b, @P3 AS i, @P4 AS f, @P5 AS d, LEN(@P6) AS len, \
             DATALENGTH(@P7) AS bytes",
        )
        .params(vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(7),
            Value::Float(1.5),
            Value::Decimal("12.50".to_string()),
            text(&"x".repeat(5000)),
            Value::Bytes(vec![1, 2, 3]),
        ])
        .returns(vec![vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(7),
            Value::Float(1.5),
            Value::Decimal("12.50".to_string()),
            Value::Int(5000),
            Value::Int(3),
        ]]),
        Attack::allowed(
            "bound parameter",
            format!("SELECT label FROM {s}.t WHERE id = @P1"),
        )
        .params(vec![Value::Int(2)])
        .returns(vec![vec![text("two")]]),
        // A #temp table ends with the `sp_executesql` call.
        Attack::allowed("#temp table", "SELECT 1 AS a INTO #t; SELECT a FROM #t")
            .returns(vec![vec![Value::Int(1)]]),
        // ── Rolled back ──
        Attack::new("DELETE", format!("DELETE FROM {s}.t")).trace(rows_unchanged()),
        Attack::new(
            "DELETE with OUTPUT",
            format!("DELETE FROM {s}.t OUTPUT deleted.id"),
        )
        .trace(rows_unchanged()),
        Attack::new(
            "DELETE with a bound parameter",
            format!("DELETE FROM {s}.t WHERE id = @P1"),
        )
        .params(vec![Value::Int(1)])
        .trace(rows_unchanged()),
        Attack::new(
            "INSERT then SELECT",
            format!("INSERT INTO {s}.t VALUES (9, N'nine'); SELECT COUNT(*) FROM {s}.t"),
        )
        .trace(rows_unchanged()),
        Attack::new("EXEC of a procedure that inserts", format!("EXEC {s}.p_insert"))
            .trace(rows_unchanged()),
        Attack::new("SELECT INTO", format!("SELECT 1 AS a INTO {s}.t2"))
            .trace(absent(&format!("{s}.t2"))),
        Attack::new("CREATE TABLE", format!("CREATE TABLE {s}.t3 (a INT)"))
            .trace(absent(&format!("{s}.t3"))),
        Attack::new("CREATE VIEW", format!("CREATE VIEW {s}.v AS SELECT 1 AS a"))
            .trace(absent(&format!("{s}.v"))),
        Attack::new("DROP TABLE", format!("DROP TABLE {s}.t")).trace(rows_unchanged()),
        Attack::new("ALTER TABLE", format!("ALTER TABLE {s}.t ADD extra INT")).trace(
            Check::count(
                format!("SELECT COUNT(*) FROM sys.columns WHERE object_id = OBJECT_ID(N'{s}.t')"),
                2,
            ),
        ),
        Attack::new("TRUNCATE", format!("TRUNCATE TABLE {s}.t")).trace(rows_unchanged()),
        Attack::new("##global temp table", "CREATE TABLE ##sq_ro_global (a INT)")
            .trace(absent("tempdb..##sq_ro_global"))
            .after("DROP TABLE IF EXISTS ##sq_ro_global"),
        Attack::new(
            "XACT_ABORT rolls back by itself",
            format!("SET XACT_ABORT ON; DELETE FROM {s}.t; SELECT 1 / 0 AS boom"),
        )
        // Inside the driver's TRY the error dooms the transaction rather
        // than ending it, so the CATCH sees it as the query's own error.
        .refused_with("QUERY_ERROR")
        .message_contains("Divide by zero")
        .trace(rows_unchanged()),
        // ── Ordinary failing queries: the query's own error ──
        //
        // These doom the transaction (XACT_STATE -1) or abort the batch
        // even with XACT_ABORT off. They must not read as escapes.
        failing("CONVERT", "SELECT CONVERT(INT, 'x') AS a", "code: 245", rows_unchanged()),
        failing(
            "CONVERT of a column",
            &format!("DELETE FROM {s}.t WHERE id = 1; SELECT CONVERT(INT, label) FROM {s}.t"),
            "code: 245",
            rows_unchanged(),
        ),
        failing("UNION type mismatch", "SELECT 1 AS a UNION SELECT 'x'", "code: 245", rows_unchanged()),
        failing("bad date", "SELECT CAST('2024-13-45' AS DATE) AS d", "code: 241", rows_unchanged()),
        failing("bad XML", "SELECT CAST('<a>' AS XML) AS x", "code: 9400", rows_unchanged()),
        failing("bad JSON", "SELECT * FROM OPENJSON('bad')", "code: 13609", rows_unchanged()),
        failing("syntax error", "SELECT * FROM", "Incorrect syntax", rows_unchanged()),
        failing("unknown table", "SELECT * FROM no_such_table_sq", "code: 208", rows_unchanged()),
        failing(
            "error with a bound parameter",
            "SELECT CONVERT(INT, @P1) AS a",
            "code: 245", rows_unchanged())
        .params(vec![text("x")]),
        // ── Transaction control ──
        //
        // The transaction is two deep, so one COMMIT only unnests it:
        // nothing is committed, and the server raises 266 when
        // `sp_executesql` returns with a different @@TRANCOUNT.
        Attack::new("COMMIT; DELETE", format!("COMMIT; DELETE FROM {s}.t"))
            .refused_with("READ_ONLY")
            .message_contains(TRIED)
            .trace(rows_unchanged()),
        Attack::new(
            "EXEC of a procedure that commits",
            format!("EXEC {s}.p_commit"),
        )
        .refused_with("READ_ONLY")
        .message_contains(TRIED)
        .trace(rows_unchanged()),
        Attack::new("BEGIN; DELETE", format!("BEGIN TRANSACTION; DELETE FROM {s}.t"))
            .refused_with("READ_ONLY")
            .message_contains(TRIED)
            .trace(rows_unchanged()),
        Attack::new(
            "savepoint",
            format!("SAVE TRANSACTION sp; DELETE FROM {s}.t; ROLLBACK TRANSACTION sp; DELETE FROM {s}.t WHERE id = 1"),
        )
        .trace(rows_unchanged()),
        // These end the transaction, which the driver reports. What they
        // write afterwards runs in an implicit transaction, which is rolled
        // back with the rest.
        Attack::new(
            "ROLLBACK; INSERT",
            format!("ROLLBACK; INSERT INTO {s}.t VALUES (9, N'nine')"),
        )
        .refused_with("READ_ONLY")
        .message_contains(ESCAPED)
        .trace(rows_unchanged()),
        Attack::new("COMMIT twice; DELETE", format!("COMMIT; COMMIT; DELETE FROM {s}.t"))
            .refused_with("READ_ONLY")
            .message_contains(ESCAPED)
            .trace(rows_unchanged()),
        // ── Refused by the server inside a transaction ──
        Attack::new(
            "ALTER DATABASE",
            format!("ALTER DATABASE CURRENT SET RECOVERY {other_recovery}"),
        )
        .refused()
        .trace(Check::value(
            "SELECT recovery_model_desc FROM sys.databases WHERE name = DB_NAME()",
            text(&recovery),
        )),
        Attack::new("CREATE DATABASE", format!("CREATE DATABASE {db}"))
            .refused()
            .trace(Check::count(
                format!("SELECT COUNT(*) FROM sys.databases WHERE name = N'{db}'"),
                0,
            ))
            .after(format!("IF DB_ID(N'{db}') IS NOT NULL DROP DATABASE {db}")),
        Attack::new("KILL", format!("KILL {spid}")).refused().trace(Check::count(
            format!("SELECT COUNT(*) FROM sys.dm_exec_sessions WHERE session_id = {spid}"),
            1,
        )),
        Attack::new(
            "sp_configure; RECONFIGURE",
            format!(
                "EXEC sp_configure 'show advanced options', {}; RECONFIGURE",
                1 - advanced
            ),
        )
        .refused()
        .trace(Check::rows(
            "SELECT CAST(value AS INT), CAST(value_in_use AS INT) FROM sys.configurations \
             WHERE name = 'show advanced options'",
            vec![vec![Value::Int(advanced), Value::Int(advanced)]],
        )),
        Attack::new(
            "BACKUP DATABASE",
            format!("BACKUP DATABASE seaquel_test TO DISK = N'{backup}'"),
        )
        .refused()
        .trace(Check::rows(
            format!(
                "DECLARE @exists INT; EXEC master.dbo.xp_fileexist N'{backup}', @exists OUTPUT; \
                 SELECT @exists"
            ),
            vec![vec![Value::Int(0)]],
        )),
        // ── Session state that outlives a rollback ──
        //
        // None of these is undone by ROLLBACK or by the end of the
        // `sp_executesql` call, so the read-only connection is dropped after
        // every call. They'd reach the user's session only if the query ran
        // on it (the traces run there; `after_each` checks the state set on
        // it at setup, and the lock is checked from a third session).
        Attack::new("SET CONTEXT_INFO", "SET CONTEXT_INFO 0x0102").trace(user_context_info()),
        Attack::new(
            "session context",
            "EXEC sp_set_session_context N'sq_ro', N'v'",
        )
        .trace(Check::count(
            "SELECT CASE WHEN SESSION_CONTEXT(N'sq_ro') IS NULL THEN 0 ELSE 1 END",
            0,
        )),
        Attack::new(
            "session app lock",
            "DECLARE @r INT; EXEC @r = sp_getapplock @Resource = N'sq_ro_lock', \
             @LockMode = 'Exclusive', @LockOwner = 'Session'; SELECT @r AS r",
        )
        .trace(lock_free),
        Attack::new(
            "global cursor",
            "DECLARE sq_ro_c CURSOR GLOBAL FOR SELECT 1 AS a; OPEN sq_ro_c",
        )
        .trace(Check::count("SELECT CURSOR_STATUS('global', 'sq_ro_c')", -3)),
        Attack::new("USE", "USE master").trace(Check::value("SELECT DB_NAME()", "seaquel_test")),
    ];
    // Accepted gaps (plan, "Probe results"), not attacks here because they
    // leave a trace: `SELECT NEXT VALUE FOR` advances a sequence for good,
    // and a query that ends the transaction and commits what it wrote
    // (`DELETE …; COMMIT; COMMIT`, a procedure that commits until
    // @@TRANCOUNT is 0) keeps its writes. `known_gaps_behave_as_documented`
    // pins down what each of them does and how it's reported.

    // State on the user's held session, which read-only calls must neither
    // see changed nor lose to a reconnect.
    let mut spec_setup = setup(&s);
    spec_setup.push("SET CONTEXT_INFO 0x0A0B".to_string());
    spec_setup.push("EXEC sp_set_session_context N'sq_user', N'kept'".to_string());

    let spec = ReadOnlySpec {
        setup: spec_setup,
        teardown: teardown(&s),
        attacks,
        after_each: vec![
            // A normal statement writes and commits: no transaction is
            // left open, and IMPLICIT_TRANSACTIONS is off again.
            Check::executes(format!("UPDATE {s}.h SET n = n + 1")),
            Check::count("SELECT @@TRANCOUNT", 0),
            Check::count("SELECT @@OPTIONS & 2", 0),
            Check::value("SELECT DB_NAME()", "seaquel_test"),
            Check::count(
                "SELECT CASE WHEN OBJECT_ID(N'tempdb..#seaquel_read_only') IS NULL THEN 0 ELSE 1 END",
                0,
            ),
            rows_unchanged(),
            // The user's session is the one set up, untouched.
            user_context_info(),
            Check::value(
                "SELECT CAST(SESSION_CONTEXT(N'sq_user') AS NVARCHAR(10))",
                "kept",
            ),
        ],
        slow_query: "WAITFOR DELAY '00:00:05'".to_string(),
        ..Default::default()
    };
    run_read_only(&*seaquel_engine_mssql::engine(), &config, &spec).await;
}

/// An ordinary failing query: its own error (`QUERY_ERROR`, `code`), not
/// an escape, and `trace` shows nothing changed.
fn failing(name: &str, sql: &str, code: &str, trace: Check) -> Attack {
    Attack::new(name, sql)
        .refused_with("QUERY_ERROR")
        .message_contains(code)
        .trace(trace)
}

/// The user's session still has the CONTEXT_INFO set on it at setup.
fn user_context_info() -> Check {
    Check::count(
        "SELECT CASE WHEN SUBSTRING(CONTEXT_INFO(), 1, 2) = 0x0A0B THEN 1 ELSE 0 END",
        1,
    )
}

/// One documented gap: `sql` through `query_read_only`, what it returns,
/// and whether its write to `s.t` (removing id 1) was committed.
struct Gap {
    name: &'static str,
    sql: String,
    /// `Ok(())`: rows; `Err((code, text))`: an error with that code whose
    /// message contains `text`.
    expect: Result<(), (&'static str, &'static str)>,
    committed: bool,
}

/// The gaps the plan accepts, asserted as they behave, so a change in
/// either direction shows up. Each restores the row it deletes.
#[tokio::test]
async fn known_gaps_behave_as_documented() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::drop_stale(&driver, GAP_PREFIX).await;
    let s = scratch_name(GAP_PREFIX);
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        for sql in setup(&s) {
            driver.execute(&sql, vec![]).await.expect(&sql);
        }

        // `NEXT VALUE FOR` in a SELECT advances the sequence for good: a
        // sequence isn't transactional.
        let r = driver
            .query_read_only(&format!("SELECT NEXT VALUE FOR {s}.seq AS n"), vec![])
            .await
            .expect("NEXT VALUE FOR");
        assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
        let current = one_cell(
            &driver,
            &format!(
                "SELECT CAST(current_value AS BIGINT) FROM sys.sequences \
                 WHERE object_id = OBJECT_ID(N'{s}.seq')"
            ),
        )
        .await;
        assert_eq!(current, Value::Int(1), "the sequence advanced");

        let gaps = [
            // Two COMMITs end the (two deep) transaction: what ran before
            // them is committed. Detected: @@TRANCOUNT is 0 afterwards.
            Gap {
                name: "DELETE; COMMIT; COMMIT",
                sql: format!("DELETE FROM {s}.t WHERE id = 1; COMMIT; COMMIT"),
                expect: Err(("READ_ONLY", ESCAPED)),
                committed: true,
            },
            // The same, with an error afterwards: the driver's CATCH sees no
            // transaction, so it's reported as an escape and not as the
            // division by zero.
            Gap {
                name: "DELETE; COMMIT; COMMIT; error",
                sql: format!("DELETE FROM {s}.t WHERE id = 1; COMMIT; COMMIT; SELECT 1 / 0"),
                expect: Err(("READ_ONLY", ESCAPED)),
                committed: true,
            },
            // Implicit transactions switched off inside the query: after the
            // COMMITs the DELETE commits on its own.
            Gap {
                name: "IMPLICIT_TRANSACTIONS OFF; COMMIT; COMMIT; DELETE",
                sql: format!(
                    "SET IMPLICIT_TRANSACTIONS OFF; COMMIT; COMMIT; DELETE FROM {s}.t WHERE id = 1"
                ),
                expect: Err(("READ_ONLY", ESCAPED)),
                committed: true,
            },
            // A procedure that commits until @@TRANCOUNT is 0.
            Gap {
                name: "EXEC of a procedure that commits everything",
                sql: format!("EXEC {s}.p_commit_all"),
                expect: Err(("READ_ONLY", ESCAPED)),
                committed: true,
            },
            // The query rolls the transaction back, writes in autocommit
            // mode, then fails: the driver's CATCH sees no transaction, so
            // it's an escape, with the query's error.
            Gap {
                name: "ROLLBACK; IMPLICIT_TRANSACTIONS OFF; DELETE; error",
                sql: format!(
                    "ROLLBACK; SET IMPLICIT_TRANSACTIONS OFF; DELETE FROM {s}.t WHERE id = 1; SELECT 1 / 0"
                ),
                expect: Err(("READ_ONLY", "Error 8134: Divide by zero")),
                committed: true,
            },
            // Committed, then the marker table dropped by hand, then an
            // error: the CATCH goes by the transaction, not the marker.
            Gap {
                name: "DELETE; COMMIT; COMMIT; DROP marker; error",
                sql: format!(
                    "DELETE FROM {s}.t WHERE id = 1; COMMIT; COMMIT; \
                     DROP TABLE #seaquel_read_only; SELECT 1 / 0"
                ),
                expect: Err(("READ_ONLY", ESCAPED)),
                committed: true,
            },
        ];
        for gap in gaps {
            let result = driver.query_read_only(&gap.sql, vec![]).await;
            match (&gap.expect, &result) {
                (Ok(()), Ok(_)) => {}
                (Err((code, text)), Err(DbError { code: c, message })) => {
                    assert_eq!(c, code, "{}: {message}", gap.name);
                    assert!(message.contains(text), "{}: {message}", gap.name);
                }
                _ => panic!("{}: expected {:?}, got {result:?}", gap.name, gap.expect),
            }
            let ids = common::ids(&driver, &format!("{s}.t")).await;
            assert_eq!(
                !ids.contains(&Value::Int(1)),
                gap.committed,
                "{}: ids {ids:?}",
                gap.name
            );
            assert_eq!(common::trancount(&driver).await, 0, "{}", gap.name);
            driver
                .execute(
                    &format!(
                        "IF NOT EXISTS (SELECT 1 FROM {s}.t WHERE id = 1) \
                         INSERT INTO {s}.t VALUES (1, N'one')"
                    ),
                    vec![],
                )
                .await
                .expect("restore id 1");
        }
    }))
    .await;
    best_effort(&driver, &teardown(&s)).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// A transaction opened by hand on the user's session: the read-only call
/// runs on its own connection, so it neither refuses nor touches it.
#[tokio::test]
async fn a_transaction_opened_by_hand_is_left_alone() {
    let Some(driver) = common::open().await else {
        return;
    };
    // Through `sp_executesql` the BEGIN outlives the call, with error 266.
    if let Err(e) = driver.query("BEGIN TRANSACTION", vec![]).await {
        assert!(e.message.contains("code: 266"), "{e:?}");
    }
    assert_eq!(common::trancount(&driver).await, 1);
    let r = driver
        .query_read_only("SELECT @@TRANCOUNT AS n", vec![])
        .await
        .expect("read-only beside a transaction opened by hand");
    // Its own transaction, two deep; not the user's.
    assert_eq!(r.rows, vec![vec![Value::Int(2)]]);
    assert_eq!(common::trancount(&driver).await, 1, "left open");
    if let Err(e) = driver.query("ROLLBACK TRANSACTION", vec![]).await {
        assert!(e.message.contains("code: 266"), "{e:?}");
    }
    assert_eq!(common::trancount(&driver).await, 0);
}

/// Calls at the same time (a dashboard refreshing its widgets) each run on
/// their own connection, in parallel up to the cap of four: eight
/// one-second queries take two rounds, not eight, and the user's session
/// stays free meanwhile.
#[tokio::test]
async fn concurrent_calls_run_side_by_side() {
    let Some(driver) = common::open().await else {
        return;
    };
    let started = tokio::time::Instant::now();
    let calls = (0..8).map(|i| {
        let driver = &driver;
        async move {
            driver
                .query_read_only(
                    &format!("WAITFOR DELAY '00:00:01'; SELECT {i} AS i, @@SPID AS spid"),
                    vec![],
                )
                .await
        }
    });
    let user = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let t = tokio::time::Instant::now();
        driver
            .query("SELECT 1 AS a", vec![])
            .await
            .expect("user query");
        t.elapsed()
    };
    let (results, user_wait) = tokio::join!(futures::future::join_all(calls), user);
    let elapsed = started.elapsed();
    let mut spids = Vec::new();
    for (i, r) in results.into_iter().enumerate() {
        let r = r.expect("concurrent read-only call");
        assert_eq!(r.rows[0][0], Value::Int(i as i64));
        spids.push(r.rows[0][1].as_i64().expect("spid"));
    }
    assert!(
        elapsed >= Duration::from_secs(2),
        "at most four at once: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(6), "in parallel: {elapsed:?}");
    assert!(
        user_wait < Duration::from_millis(500),
        "the user's session waited {user_wait:?}"
    );
    let user_spid = driver
        .query("SELECT @@SPID AS spid", vec![])
        .await
        .expect("spid")
        .rows[0][0]
        .as_i64();
    assert!(!spids.contains(&user_spid.unwrap()), "{spids:?}");
}

/// A call dropped while its write waits: the connection is closed, the
/// server rolls back and releases the locks, so a normal query right
/// afterwards sees every row instead of waiting on them.
#[tokio::test]
async fn a_dropped_call_rolls_back_its_write() {
    let Some(driver) = common::open().await else {
        return;
    };
    common::with_table(&driver, |driver, table| async move {
        driver
            .execute(
                &format!("INSERT INTO {table} VALUES (1, N'one'), (2, N'two')"),
                vec![],
            )
            .await
            .expect("insert");
        let slow = tokio::time::timeout(
            Duration::from_millis(300),
            driver.query_read_only(
                &format!("DELETE FROM {table}; WAITFOR DELAY '00:00:10'"),
                vec![],
            ),
        )
        .await;
        assert!(slow.is_err(), "still running when dropped: {slow:?}");
        let ids = tokio::time::timeout(Duration::from_secs(5), common::ids(driver, &table))
            .await
            .expect("the DELETE's locks were released");
        assert_eq!(ids, common::ints(&[1, 2]));
    })
    .await;
}

/// Read-only calls blocked on a row lock the user holds fail after the
/// 10 s lock timeout (1222, a statement error: nothing escaped), so they
/// can't hold the four connection slots for good: five of them all finish,
/// and the next call gets a slot.
#[tokio::test]
async fn calls_blocked_on_a_lock_time_out_and_free_their_slots() {
    let Some(driver) = common::open().await else {
        return;
    };
    let Some(holder) = common::open().await else {
        return;
    };
    common::with_table(&driver, |driver, table| {
        let holder = &holder;
        async move {
            driver
                .execute(&format!("INSERT INTO {table} VALUES (1, N'one')"), vec![])
                .await
                .expect("insert");
            // A transaction left open on the holder's session, holding an
            // exclusive lock on the row.
            if let Err(e) = holder
                .query(
                    &format!("BEGIN TRANSACTION; UPDATE {table} SET label = N'x' WHERE id = 1"),
                    vec![],
                )
                .await
            {
                assert!(e.message.contains("code: 266"), "{e:?}");
            }
            let select = format!("SELECT label FROM {table}");
            let started = tokio::time::Instant::now();
            let blocked = (0..5).map(|_| driver.query_read_only(&select, vec![]));
            let results = futures::future::join_all(blocked).await;
            for r in results {
                let err = r.expect_err("blocked on the lock");
                assert_eq!(err.code, "QUERY_ERROR", "{err:?}");
                assert!(err.message.contains("code: 1222"), "{err:?}");
            }
            // Four at once, then the fifth: two rounds of the lock timeout.
            let elapsed = started.elapsed();
            assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
            let r = tokio::time::timeout(
                Duration::from_secs(5),
                driver.query_read_only("SELECT 1 AS a", vec![]),
            )
            .await
            .expect("a slot is free")
            .expect("a trivial call");
            assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
            if let Err(e) = holder.query("ROLLBACK TRANSACTION", vec![]).await {
                assert!(e.message.contains("code: 266"), "{e:?}");
            }
        }
    })
    .await;
}
