//! Live tests of the crate-internal `Session`, which integration tests can't
//! reach. They read SEAQUEL_TEST_MSSQL and skip when it's unset.

use seaquel_engine::{BatchStatement, Driver, Value};
use seaquel_engine_testkit::config_from_env;

use crate::MssqlDriver;

async fn open() -> Option<MssqlDriver> {
    let mut config = config_from_env("SEAQUEL_TEST_MSSQL")?;
    config.database = Some("seaquel_test".to_string());
    Some(MssqlDriver::connect(&config).await.expect("connect"))
}

async fn trancount(driver: &MssqlDriver) -> i64 {
    let r = driver
        .query("SELECT @@TRANCOUNT AS n", vec![])
        .await
        .expect("@@TRANCOUNT");
    r.rows[0][0].as_i64().expect("an integer")
}

#[tokio::test]
async fn session_batches_keep_session_settings() {
    let Some(driver) = open().await else { return };
    const COUNT: &str = "SELECT COUNT(*) AS n FROM sys.objects WHERE object_id > 0";

    // `SET SHOWPLAN_XML ON` must be alone in its batch, so it can't go
    // through `query` (sp_executesql). A session runs it as a batch and keeps
    // the client for the statements that follow. Under SHOWPLAN_XML only a
    // batch returns a plan: a `sp_executesql` call returns nothing at all.
    let mut session = driver.session().await.expect("session");
    session
        .batch("SET SHOWPLAN_XML ON")
        .await
        .expect("SHOWPLAN_XML ON");
    let plan = session.batch(COUNT).await;
    let rpc = session.query(COUNT, &[]).await;
    session
        .batch("SET SHOWPLAN_XML OFF")
        .await
        .expect("SHOWPLAN_XML OFF");
    let plan = plan.expect("plan");
    assert_eq!(plan.len(), 1);
    let xml = plan[0].rows[0][0].as_str().expect("XML text");
    assert!(xml.starts_with("<ShowPlanXML"), "{xml}");
    assert_eq!(rpc.expect("sp_executesql under SHOWPLAN_XML"), vec![]);

    // `SET STATISTICS XML ON` runs the statement, parameters and all, and
    // adds the actual plan as the next result set.
    session
        .batch("SET STATISTICS XML ON")
        .await
        .expect("STATISTICS XML ON");
    let analyzed = session
        .query(
            "SELECT COUNT(*) AS n FROM sys.objects WHERE object_id > @P1",
            &[Value::Int(0)],
        )
        .await;
    session
        .batch("SET STATISTICS XML OFF")
        .await
        .expect("STATISTICS XML OFF");
    let analyzed = analyzed.expect("STATISTICS XML");
    assert_eq!(analyzed.len(), 2);
    assert_eq!(analyzed[0].columns, vec!["n"]);
    let xml = analyzed[1].rows[0][0].as_str().expect("XML text");
    assert!(xml.starts_with("<ShowPlanXML"), "{xml}");
    drop(session);

    let r = driver
        .query(COUNT, vec![])
        .await
        .expect("after the session");
    assert_eq!(r.columns, vec!["n"]);
    assert_eq!(r.rows.len(), 1);
}

/// Whether this is still the connection that created `#t11_marker` (a
/// session-scoped temp table; SPIDs are reused, so they can't tell).
async fn same_connection(driver: &MssqlDriver) -> bool {
    let r = driver
        .query(
            "SELECT CASE WHEN OBJECT_ID('tempdb..#t11_marker') IS NULL THEN 0 ELSE 1 END AS m",
            vec![],
        )
        .await
        .expect("marker");
    r.rows[0][0].as_i64() == Some(1)
}

#[tokio::test]
async fn held_session_state_is_undone_by_release_or_by_closing_the_connection() {
    let Some(driver) = open().await else { return };
    driver
        .session()
        .await
        .expect("session")
        .batch("CREATE TABLE #t11_marker (x INT)")
        .await
        .expect("marker");
    assert!(same_connection(&driver).await);

    // Released: the connection is clean and kept.
    {
        let mut session = driver.session().await.expect("session");
        session.hold_state();
        session
            .batch("SET SHOWPLAN_XML ON")
            .await
            .expect("SHOWPLAN_XML ON");
        session
            .batch("SET SHOWPLAN_XML OFF")
            .await
            .expect("SHOWPLAN_XML OFF");
        session.release_state();
    }
    assert!(
        same_connection(&driver).await,
        "a released session keeps the connection"
    );

    // Not released (a caller that went away, or forgot): even though every
    // request finished cleanly, the connection is closed, so SHOWPLAN_XML
    // can't leak into the next call.
    {
        let mut session = driver.session().await.expect("session");
        session.hold_state();
        session
            .batch("SET SHOWPLAN_XML ON")
            .await
            .expect("SHOWPLAN_XML ON");
    }
    let r = driver
        .query("SELECT 1 AS a", vec![])
        .await
        .expect("after reconnecting");
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
    assert!(
        !same_connection(&driver).await,
        "an unreleased session closes the connection"
    );
}

#[tokio::test]
async fn a_transaction_refuses_to_nest_in_one_opened_by_hand() {
    let Some(driver) = open().await else { return };
    driver
        .session()
        .await
        .expect("session")
        .batch("BEGIN TRANSACTION")
        .await
        .expect("BEGIN by hand");
    assert_eq!(trancount(&driver).await, 1);

    let err = driver
        .transaction(vec![BatchStatement {
            sql: "SELECT 1".to_string(),
            params: vec![],
            expect_rows: None,
        }])
        .await
        .expect_err("nested transaction");
    assert_eq!(err.code, "EXECUTE_ERROR");
    assert!(
        err.message
            .contains("a transaction is already open on this connection"),
        "{}",
        err.message
    );
    // The hand-opened transaction is untouched.
    assert_eq!(trancount(&driver).await, 1);

    driver
        .session()
        .await
        .expect("session")
        .batch("ROLLBACK TRANSACTION")
        .await
        .expect("ROLLBACK by hand");
    assert_eq!(trancount(&driver).await, 0);
    driver.transaction(vec![]).await.expect("a transaction now");
}

/// A server whose `user options` include NOCOUNT starts every session with
/// NOCOUNT ON, and tiberius then reports 0 rows for every UPDATE. The
/// session-level `SET NOCOUNT ON` below stands in for that option (setting
/// it with `sp_configure` would change the shared test server for every
/// other test). Opening a connection, and every reconnect, turns it off.
#[tokio::test]
async fn nocount_on_for_the_session_is_turned_off_on_open() {
    let Some(driver) = open().await else { return };
    let table = format!(
        "dbo.{}",
        seaquel_engine_testkit::scratch_name("t18_nocount_")
    );
    driver
        .execute(
            &format!("CREATE TABLE {table} (id INT PRIMARY KEY, label NVARCHAR(10))"),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let update = format!("UPDATE {table} SET label = @P1 WHERE id = @P2");
    let affected = || async {
        driver
            .execute(&update, vec![Value::from("x"), Value::Int(1)])
            .await
            .expect("UPDATE")
            .rows_affected
    };
    let outcome = async {
        driver
            .execute(
                &format!("INSERT INTO {table} (id, label) VALUES (1, N'a')"),
                vec![],
            )
            .await
            .expect("INSERT");
        assert_eq!(affected().await, 1);

        // NOCOUNT ON for the session: tiberius reports 0.
        let set_nocount = || async {
            driver
                .session()
                .await
                .expect("session")
                .batch("SET NOCOUNT ON")
                .await
                .expect("SET NOCOUNT ON");
        };
        set_nocount().await;
        assert_eq!(
            affected().await,
            0,
            "tiberius counts nothing under NOCOUNT ON"
        );

        // What opening a connection runs restores the counts.
        driver.reset_open_session().await.expect("reset");
        assert_eq!(affected().await, 1);

        // So does a reconnect.
        set_nocount().await;
        driver.force_reconnect().await;
        assert_eq!(affected().await, 1);
    };
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(outcome)).await;
    let _ = driver.execute(&format!("DROP TABLE {table}"), vec![]).await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
