//! Phase 4 security review fixes on Postgres: `explain_read_only` plans
//! inside the read-only transaction, and a read-only statement stops on the
//! server at its timeout or when the call is dropped.
//!
//! Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
//! {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use seaquel_engine::{Driver, ReadOnlyOptions, Value};
use seaquel_engine_testkit::{config_from_env, scratch_name};

async fn open() -> Option<Arc<dyn Driver>> {
    let config = config_from_env("SEAQUEL_TEST_POSTGRES")?;
    Some(
        seaquel_engine_postgres::engine()
            .open(&config)
            .await
            .expect("open"),
    )
}

async fn count(driver: &dyn Driver, sql: &str) -> i64 {
    driver.query(sql, vec![]).await.expect(sql).rows[0][0]
        .as_i64()
        .expect("count")
}

/// Backends other than the caller's running a statement that contains
/// `marker`.
async fn running(driver: &dyn Driver, marker: &str) -> i64 {
    count(
        driver,
        &format!(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE pid <> pg_backend_pid() AND query LIKE '%{marker}%'"
        ),
    )
    .await
}

/// Waits up to `within` for no backend to run `marker`'s statement.
async fn gone_within(driver: &dyn Driver, marker: &str, within: Duration) -> bool {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < within {
        if running(driver, marker).await == 0 {
            return true;
        }
        tokio::time::sleep(step).await;
        waited += step;
    }
    false
}

/// A non-volatile function that calls a writing one is folded while the plan
/// is made: a plain EXPLAIN committed its INSERT. `explain_read_only` plans
/// in the read-only transaction, so the INSERT is refused and nothing
/// changes.
#[tokio::test]
async fn explain_read_only_runs_planning_read_only() {
    let Some(driver) = open().await else {
        return;
    };
    let t = scratch_name("seaquel_ex_");
    let seq = scratch_name("seaquel_ex_seq_");
    let w = scratch_name("seaquel_ex_w_");
    let s = scratch_name("seaquel_ex_s_");
    for sql in [
        format!("CREATE TABLE {t} (n int)"),
        format!("CREATE SEQUENCE {seq}"),
        format!(
            "CREATE FUNCTION {w}() RETURNS int LANGUAGE plpgsql VOLATILE \
             AS $$ BEGIN INSERT INTO {t} VALUES (1); RETURN 1; END $$"
        ),
        format!(
            "CREATE FUNCTION {s}() RETURNS int LANGUAGE plpgsql IMMUTABLE \
             AS $$ BEGIN RETURN {w}(); END $$"
        ),
    ] {
        driver.execute(&sql, vec![]).await.expect(&sql);
    }

    let outcome = std::panic::AssertUnwindSafe(async {
        // Folded at plan time: refused, nothing inserted.
        for sql in [
            format!("SELECT {s}()"),
            format!("SELECT * FROM {t} WHERE n = {s}()"),
        ] {
            let err = driver
                .explain_read_only(&sql, vec![], None)
                .await
                .expect_err(&sql);
            assert_eq!(err.code, "READ_ONLY", "{sql}: {}", err.message);
            assert!(
                err.message.contains("read-only transaction"),
                "{}",
                err.message
            );
        }
        assert_eq!(
            count(&*driver, &format!("SELECT count(*) FROM {t}")).await,
            0
        );

        // A sequence isn't advanced by planning, and the plan comes back.
        let plan = driver
            .explain_read_only(&format!("SELECT nextval('{seq}')"), vec![], None)
            .await
            .expect("explain nextval");
        assert!(!plan.is_analyze);
        let last = driver
            .query(&format!("SELECT last_value, is_called FROM {seq}"), vec![])
            .await
            .expect("sequence");
        assert_eq!(last.rows, vec![vec![Value::Int(1), Value::Bool(false)]]);

        // Parameters bind, and the plan names the table.
        let plan = driver
            .explain_read_only(
                &format!("SELECT * FROM {t} WHERE n = $1"),
                vec![Value::Int(3)],
                Some(Duration::from_secs(5)),
            )
            .await
            .expect("explain with a parameter");
        assert_eq!(plan.plan.relation_name.as_deref(), Some(t.as_str()));

        // A second statement is refused as a whole.
        let err = driver
            .explain_read_only(
                &format!("SELECT 1; INSERT INTO {t} VALUES (2)"),
                vec![],
                None,
            )
            .await
            .expect_err("two statements");
        assert!(err.message.contains("multiple commands"), "{}", err.message);
        assert_eq!(
            count(&*driver, &format!("SELECT count(*) FROM {t}")).await,
            0
        );
    })
    .catch_unwind()
    .await;

    for sql in [
        format!("DROP FUNCTION IF EXISTS {s}()"),
        format!("DROP FUNCTION IF EXISTS {w}()"),
        format!("DROP SEQUENCE IF EXISTS {seq}"),
        format!("DROP TABLE IF EXISTS {t}"),
    ] {
        driver.execute(&sql, vec![]).await.expect(&sql);
    }
    driver.close().await.expect("close");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// The server stops a read-only statement at its timeout: the call fails
/// with `TIMEOUT` and the backend is idle again right after.
#[tokio::test]
async fn read_only_timeout_stops_the_statement_on_the_server() {
    let Some(driver) = open().await else {
        return;
    };
    let marker = scratch_name("seaquel_to_");
    let sql = format!("SELECT pg_sleep(30) AS {marker}");
    let options = ReadOnlyOptions::default().with_timeout(Some(Duration::from_millis(500)));
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        driver.query_read_only_with(&sql, vec![], options),
    )
    .await
    .expect("the server's timeout didn't end the call")
    .expect_err("the statement timed out");
    assert_eq!(err.code, "TIMEOUT", "{}", err.message);
    assert!(err.message.contains("statement timeout"), "{}", err.message);
    assert!(gone_within(&*driver, &marker, Duration::from_secs(3)).await);

    // Without a timeout the path is unchanged.
    let r = driver
        .query_read_only_with("SELECT 1 AS one", vec![], ReadOnlyOptions::default())
        .await
        .expect("SELECT 1");
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
    driver.close().await.expect("close");
}

/// Dropping a read-only call mid-statement sends `pg_cancel_backend`: the
/// statement is gone long before it would have finished.
#[tokio::test]
async fn dropping_a_read_only_call_cancels_it_on_the_server() {
    let Some(driver) = open().await else {
        return;
    };
    let marker = scratch_name("seaquel_drop_");
    let sql = format!("SELECT pg_sleep(30) AS {marker}");
    let dropped = tokio::time::timeout(
        Duration::from_millis(700),
        driver.query_read_only_with(&sql, vec![], ReadOnlyOptions::default()),
    )
    .await;
    assert!(dropped.is_err(), "the sleep returned early: {dropped:?}");
    assert!(
        gone_within(&*driver, &marker, Duration::from_secs(5)).await,
        "the dropped statement still runs"
    );
    driver.close().await.expect("close");
}

/// An EXPLAIN whose planning runs a slow immutable function stops at the
/// timeout too.
#[tokio::test]
async fn explain_read_only_timeout_stops_planning() {
    let Some(driver) = open().await else {
        return;
    };
    let slow = scratch_name("seaquel_ex_slow_");
    let create = format!(
        "CREATE FUNCTION {slow}() RETURNS int LANGUAGE plpgsql IMMUTABLE \
         AS $$ BEGIN PERFORM pg_sleep(30); RETURN 1; END $$"
    );
    driver.execute(&create, vec![]).await.expect("create");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        driver.explain_read_only(
            &format!("SELECT {slow}()"),
            vec![],
            Some(Duration::from_millis(500)),
        ),
    )
    .await;
    let gone = gone_within(&*driver, &slow, Duration::from_secs(3)).await;
    driver
        .execute(&format!("DROP FUNCTION IF EXISTS {slow}()"), vec![])
        .await
        .expect("drop");
    driver.close().await.expect("close");
    let err = result
        .expect("the server's timeout didn't end the call")
        .expect_err("planning timed out");
    assert_eq!(err.code, "TIMEOUT", "{}", err.message);
    assert!(gone);
}
