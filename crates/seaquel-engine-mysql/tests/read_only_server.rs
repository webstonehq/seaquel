//! Phase 4 security review fixes on MySQL and MariaDB: `explain_read_only`
//! plans inside the read-only session and transaction, and a read-only
//! statement stops on the server at its timeout or when the call is
//! dropped. Runs once per server:
//! - SEAQUEL_TEST_MYSQL, e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}
//! - SEAQUEL_TEST_MARIADB, e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use seaquel_engine::{ConnectConfig, Driver, ReadOnlyOptions, Value};
use seaquel_engine_testkit::{config_from_env, scratch_name};
use sqlx::Connection;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Server {
    Mysql,
    Mariadb,
}

impl Server {
    fn config(self) -> Option<ConnectConfig> {
        config_from_env(match self {
            Server::Mysql => "SEAQUEL_TEST_MYSQL",
            Server::Mariadb => "SEAQUEL_TEST_MARIADB",
        })
    }
}

async fn open(config: &ConnectConfig) -> Arc<dyn Driver> {
    seaquel_engine_mysql::engine()
        .open(config)
        .await
        .expect("open")
}

/// Runs statements one at a time over the text protocol: `CREATE
/// FUNCTION` can't be prepared (error 1295).
async fn raw(config: &ConnectConfig, statements: &[String]) {
    let url = config
        .connection_string
        .as_deref()
        .expect("connection_string");
    let mut conn = sqlx::MySqlConnection::connect(url).await.expect("connect");
    for sql in statements {
        sqlx::Executor::execute(&mut conn, sql.as_str())
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    conn.close().await.expect("close");
}

async fn value(driver: &dyn Driver, sql: &str) -> i64 {
    driver.query(sql, vec![]).await.expect(sql).rows[0][0]
        .as_i64()
        .expect("a number")
}

/// Waits up to `within` for no other connection to run a statement that
/// contains `marker`.
async fn gone_within(driver: &dyn Driver, marker: &str, within: Duration) -> bool {
    let sql = format!(
        "SELECT count(*) FROM information_schema.processlist \
         WHERE id <> CONNECTION_ID() AND info LIKE '%{marker}%'"
    );
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while waited < within {
        if value(driver, &sql).await == 0 {
            return true;
        }
        tokio::time::sleep(step).await;
        waited += step;
    }
    false
}

/// 10^10 rows counted: minutes of work, unless something stops it.
fn heavy(marker: &str) -> String {
    let digits = "(SELECT 0 d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 \
                  UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 \
                  UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9)";
    let joins: Vec<String> = (0..10).map(|i| format!("{digits} t{i}")).collect();
    format!(
        "SELECT count(*) AS {marker} FROM {}",
        joins.join(" CROSS JOIN ")
    )
}

#[tokio::test]
async fn explain_read_only_mysql() {
    explain_read_only(Server::Mysql).await;
}

#[tokio::test]
async fn explain_read_only_mariadb() {
    explain_read_only(Server::Mariadb).await;
}

/// MariaDB's optimizer materializes a derived table and reads a table
/// looked up by its primary key while it plans, so a plain EXPLAIN runs
/// the functions in them: `NEXTVAL` advanced the sequence and a
/// deterministic function that calls a writing one inserted its row
/// (asserted at the end, so the fixtures are known to reach the write).
/// `explain_read_only` plans in the read-only session and transaction:
/// refused, nothing changed. MySQL 8 didn't write through EXPLAIN, and
/// nothing changes there either.
async fn explain_read_only(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let t = scratch_name("seaquel_ex_");
    let u = scratch_name("seaquel_ex_u_");
    let w = scratch_name("seaquel_ex_w_");
    let d = scratch_name("seaquel_ex_d_");
    let seq = scratch_name("seaquel_ex_seq_");
    let seq_plain = scratch_name("seaquel_ex_sp_");
    let one = scratch_name("seaquel_ex_one_");
    let mut setup = vec![
        format!("CREATE TABLE {t} (n INT)"),
        format!("CREATE TABLE {u} (n INT)"),
        format!("INSERT INTO {u} VALUES (5), (6)"),
        format!("CREATE TABLE {one} (n INT PRIMARY KEY)"),
        format!("INSERT INTO {one} VALUES (1)"),
        format!(
            "CREATE FUNCTION {w}() RETURNS INT NOT DETERMINISTIC MODIFIES SQL DATA \
             BEGIN INSERT INTO {t} VALUES (1); RETURN 1; END"
        ),
        format!("CREATE FUNCTION {d}() RETURNS INT DETERMINISTIC BEGIN RETURN {w}(); END"),
    ];
    if server == Server::Mariadb {
        setup.push(format!("CREATE SEQUENCE {seq}"));
        setup.push(format!("CREATE SEQUENCE {seq_plain}"));
    }
    let mut teardown = vec![
        format!("DROP FUNCTION IF EXISTS {d}"),
        format!("DROP FUNCTION IF EXISTS {w}"),
        format!("DROP TABLE IF EXISTS {u}"),
        format!("DROP TABLE IF EXISTS {one}"),
        format!("DROP TABLE IF EXISTS {t}"),
    ];
    if server == Server::Mariadb {
        teardown.push(format!("DROP SEQUENCE IF EXISTS {seq}"));
        teardown.push(format!("DROP SEQUENCE IF EXISTS {seq_plain}"));
    }
    // MySQL refuses a non-deterministic writing function without this when
    // binary logging is on.
    raw(
        &config,
        &[
            &["SET GLOBAL log_bin_trust_function_creators = 1".to_string()][..],
            &setup,
        ]
        .concat(),
    )
    .await;

    let outcome = AssertUnwindSafe(async {
        let rows = format!("SELECT count(*) FROM {t}");
        let writing = [
            format!("SELECT * FROM (SELECT {d}() AS v) x"),
            format!("SELECT * FROM {one} WHERE n = {d}()"),
            format!("SELECT * FROM {u} WHERE n = (SELECT {d}())"),
        ];
        for sql in &writing {
            match driver.explain_read_only(sql, vec![], None).await {
                Ok(_) => {}
                Err(e) => assert_eq!(e.code, "READ_ONLY", "{sql}: {}", e.message),
            }
            assert_eq!(value(&*driver, &rows).await, 0, "{sql} wrote");
        }

        // One statement: a second is refused as a whole.
        let err = driver
            .explain_read_only(
                &format!("SELECT 1; INSERT INTO {t} VALUES (2)"),
                vec![],
                None,
            )
            .await
            .expect_err("two statements");
        assert_eq!(err.code, "QUERY_ERROR", "{}", err.message);
        assert_eq!(value(&*driver, &rows).await, 0);

        if server == Server::Mariadb {
            let next = |s: &str| format!("SELECT next_not_cached_value FROM {s}");
            let advancing = |s: &str| {
                [
                    format!("SELECT * FROM (SELECT NEXTVAL({s}) AS v) x"),
                    format!("SELECT * FROM {one} WHERE n = (SELECT NEXTVAL({s}))"),
                ]
            };
            for sql in advancing(&seq) {
                let err = driver
                    .explain_read_only(&sql, vec![], None)
                    .await
                    .expect_err(&sql);
                assert_eq!(err.code, "READ_ONLY", "{sql}: {}", err.message);
                assert_eq!(value(&*driver, &next(&seq)).await, 1, "{sql}");
            }
            // The fixtures reach the writes through a plain EXPLAIN.
            for sql in advancing(&seq_plain) {
                driver.explain(&sql, vec![], false).await.expect(&sql);
            }
            assert!(value(&*driver, &next(&seq_plain)).await > 1);
            driver
                .explain(&writing[0], vec![], false)
                .await
                .expect("plain explain");
            assert!(value(&*driver, &rows).await > 0);
        }

        // A plan comes back, with parameters bound.
        let plan = driver
            .explain_read_only(
                &format!("SELECT * FROM {u} WHERE n = ?"),
                vec![Value::Int(5)],
                Some(Duration::from_secs(5)),
            )
            .await
            .expect("explain");
        assert!(!plan.is_analyze);
    })
    .catch_unwind()
    .await;

    raw(&config, &teardown).await;
    driver.close().await.expect("close");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn timeout_mysql() {
    timeout(Server::Mysql).await;
}

#[tokio::test]
async fn timeout_mariadb() {
    timeout(Server::Mariadb).await;
}

/// The server stops a read-only statement at its timeout
/// (`max_execution_time` on MySQL, `max_statement_time` on MariaDB): the
/// call fails with `TIMEOUT` and the statement is gone from the
/// processlist right after.
async fn timeout(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    let marker = scratch_name("seaquel_to_");
    let options = ReadOnlyOptions::default().with_timeout(Some(Duration::from_millis(500)));
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        driver.query_read_only_with(&heavy(&marker), vec![], options),
    )
    .await
    .expect("the server's timeout didn't end the call")
    .expect_err("the statement timed out");
    assert_eq!(err.code, "TIMEOUT", "{}", err.message);
    assert!(err.message.contains("interrupted"), "{}", err.message);
    assert!(gone_within(&*driver, &marker, Duration::from_secs(3)).await);

    // Without a timeout the path is unchanged.
    let r = driver
        .query_read_only_with("SELECT 1 AS one", vec![], ReadOnlyOptions::default())
        .await
        .expect("SELECT 1");
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
    driver.close().await.expect("close");
}

#[tokio::test]
async fn drop_mysql() {
    drop_cancels(Server::Mysql).await;
}

#[tokio::test]
async fn drop_mariadb() {
    drop_cancels(Server::Mariadb).await;
}

/// Dropping a read-only call mid-statement sends `KILL QUERY`: the
/// statement is gone long before it would have finished.
async fn drop_cancels(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let driver = open(&config).await;
    for marker in [
        scratch_name("seaquel_drop_"),
        scratch_name("seaquel_drop_h_"),
    ] {
        let sql = if marker.starts_with("seaquel_drop_h_") {
            heavy(&marker)
        } else {
            format!("SELECT SLEEP(30) AS {marker}")
        };
        let dropped = tokio::time::timeout(
            Duration::from_millis(700),
            driver.query_read_only_with(&sql, vec![], ReadOnlyOptions::default()),
        )
        .await;
        assert!(dropped.is_err(), "{sql} returned early: {dropped:?}");
        assert!(
            gone_within(&*driver, &marker, Duration::from_secs(5)).await,
            "the dropped statement still runs: {sql}"
        );
    }
    driver.close().await.expect("close");
}
