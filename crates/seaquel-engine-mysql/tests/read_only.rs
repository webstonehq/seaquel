//! `query_read_only` on MySQL and MariaDB (AI safety plan, Task 2): every
//! attack goes through the read-only path and is then checked from a normal
//! session. Runs once per server:
//! - SEAQUEL_TEST_MYSQL, e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3306/seaquel_test"}
//! - SEAQUEL_TEST_MARIADB (MariaDB uses the mysql driver), e.g.
//!   {"driver":"mysql","connection_string":"mysql://root@127.0.0.1:3307/seaquel_test"}
//!
//! Accepted gaps (plan, "Probe results"), not asserted here:
//! - `SELECT … INTO OUTFILE/DUMPFILE` still writes a file for a login with
//!   the FILE privilege: it did on MySQL 8 into `secure_file_priv`, and on
//!   MariaDB 11 into `/tmp`, where `secure_file_priv` is unset. Fix 14
//!   blocks `OUTFILE` and `DUMPFILE`.
//! - `SET GLOBAL` and `SET PERSIST` still work for a login that may use
//!   them. Fix 14 blocks `SET`.
//! - Found while writing these tests: a stored procedure that already exists
//!   and ends the transaction (`COMMIT`, `ROLLBACK` or `START TRANSACTION
//!   READ WRITE`), then runs `SET SESSION transaction_read_only = 0`, still
//!   writes, on both servers. `CALL` runs each statement of a procedure as
//!   its own statement, and a procedure may end the transaction it runs in.
//!   The read-only transaction the driver opens on top of the session
//!   setting stops the same procedure when it doesn't end the transaction
//!   (asserted below). A stored function can't end it (error 1422), so only
//!   `CALL` reaches this, and fix 14's token check blocks `CALL` and `EXEC`
//!   on every engine: the AI can't call a procedure itself. Like Postgres's
//!   existing functions with outside effects, only a read-only login closes
//!   it fully.

use futures::FutureExt;
use seaquel_engine::{ConnectConfig, Value};
use seaquel_engine_testkit::{
    config_from_env, run_read_only, scratch_name, Attack, Check, ReadOnlySpec,
};
use sqlx::Connection;
use std::panic::AssertUnwindSafe;

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

/// sqlx's default `max_connections`, which the driver doesn't change: the
/// most connections the pool can hold, so `after_each` reaches them all.
const POOL: usize = 10;

/// Both servers refuse a write under `transaction_read_only` with error
/// 1792 ("Cannot execute statement in a READ ONLY transaction"), which the
/// driver reports as `READ_ONLY` with the server's message.
const REFUSAL: &str = "Cannot execute statement in a READ ONLY transaction";

#[tokio::test]
async fn read_only_mysql() {
    read_only(Server::Mysql).await;
}

#[tokio::test]
async fn read_only_mariadb() {
    read_only(Server::Mariadb).await;
}

/// Runs `sql` one statement at a time on a plain connection, over the text
/// protocol: `CREATE FUNCTION`/`PROCEDURE` can't be prepared (error 1295),
/// and the driver's `execute` prepares everything.
async fn raw(config: &ConnectConfig, statements: &[String]) -> Result<(), String> {
    let url = config
        .connection_string
        .as_deref()
        .expect("connection_string");
    let mut conn = sqlx::MySqlConnection::connect(url)
        .await
        .map_err(|e| e.to_string())?;
    let mut result = Ok(());
    for sql in statements {
        if let Err(e) = sqlx::Executor::execute(&mut conn, sql.as_str()).await {
            result = Err(format!("{e}\n  {sql}"));
            break;
        }
    }
    let _ = conn.close().await;
    result
}

async fn read_only(server: Server) {
    let Some(config) = server.config() else {
        return;
    };
    let engine = seaquel_engine_mysql::engine();

    let t = scratch_name("seaquel_ro_");
    let hygiene = scratch_name("seaquel_ro_hyg_");
    let f_insert = scratch_name("seaquel_ro_f_");
    let f_flip = scratch_name("seaquel_ro_ff_");
    let p_insert = scratch_name("seaquel_ro_p_");
    let p_flip = scratch_name("seaquel_ro_pf_");
    let p_txn = scratch_name("seaquel_ro_pt_");
    let created = scratch_name("seaquel_ro_new_");
    let renamed = scratch_name("seaquel_ro_ren_");
    let database = scratch_name("seaquel_ro_db_");
    let temp = scratch_name("seaquel_ro_tmp_");
    let index = scratch_name("seaquel_ro_ix_");
    let lock = scratch_name("seaquel_ro_lock_");

    // Stored programs go through the text protocol (see `raw`), before and
    // after the run.
    // `f_flip` and `p_flip` set `transaction_read_only`, which MariaDB 10.x
    // lacks (`tx_read_only`); the test containers are MySQL 8 and MariaDB 11.
    let routines = [
        format!(
            "CREATE FUNCTION {f_insert}() RETURNS INT DETERMINISTIC MODIFIES SQL DATA \
             BEGIN INSERT INTO {t} (id) VALUES (99); RETURN 1; END"
        ),
        format!(
            "CREATE FUNCTION {f_flip}() RETURNS INT DETERMINISTIC MODIFIES SQL DATA \
             BEGIN SET @@session.transaction_read_only = 0; INSERT INTO {t} (id) VALUES (98); RETURN 1; END"
        ),
        format!("CREATE PROCEDURE {p_insert}() BEGIN INSERT INTO {t} (id) VALUES (97); END"),
        format!(
            "CREATE PROCEDURE {p_flip}() \
             BEGIN SET SESSION transaction_read_only = 0; INSERT INTO {t} (id) VALUES (96); END"
        ),
        format!(
            "CREATE PROCEDURE {p_txn}() \
             BEGIN START TRANSACTION; INSERT INTO {t} (id) VALUES (95); COMMIT; END"
        ),
    ];
    let drop_routines = [
        format!("DROP FUNCTION IF EXISTS {f_insert}"),
        format!("DROP FUNCTION IF EXISTS {f_flip}"),
        format!("DROP PROCEDURE IF EXISTS {p_insert}"),
        format!("DROP PROCEDURE IF EXISTS {p_flip}"),
        format!("DROP PROCEDURE IF EXISTS {p_txn}"),
    ];

    // The pool's session defaults, which every pooled connection must still
    // have after each attack. sqlx sets its own `sql_mode` at connect.
    let sql_mode = {
        let driver = engine.open(&config).await.expect("open");
        let r = driver
            .query("SELECT @@session.sql_mode AS m", vec![])
            .await
            .expect("sql_mode");
        driver.close().await.expect("close");
        r.rows[0][0].clone()
    };

    let rows = |n: i64| Check::count(format!("SELECT COUNT(*) AS n FROM {t}"), n);
    let absent = |name: &str| {
        Check::count(
            format!(
                "SELECT COUNT(*) AS n FROM information_schema.tables \
                 WHERE table_schema = DATABASE() AND table_name = '{name}'"
            ),
            0,
        )
    };
    let refused = |name: &str, sql: String| {
        Attack::new(name, sql)
            .refused_with("READ_ONLY")
            .message_contains(REFUSAL)
    };

    let attacks = vec![
        Attack::allowed("SELECT", format!("SELECT COUNT(*) AS n FROM {t}"))
            .returns(vec![vec![Value::Int(3)]]),
        Attack::allowed("WITH", "WITH x AS (SELECT 1 AS a) SELECT a FROM x")
            .returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("bind values", "SELECT ? AS n")
            .params(vec![Value::Int(5)])
            .returns(vec![vec![Value::Int(5)]]),
        // The read-only connection comes from the pool, with sqlx's session
        // time zone, so TIMESTAMP values print as they do in the editor.
        Attack::allowed("session time zone", "SELECT @@session.time_zone AS tz")
            .returns(vec![vec![Value::from("+00:00")]]),
        refused("INSERT", format!("INSERT INTO {t} (id) VALUES (90)")).trace(rows(3)),
        refused("DELETE", format!("DELETE FROM {t}")).trace(rows(3)),
        refused("function that inserts", format!("SELECT {f_insert}()")).trace(rows(3)),
        refused(
            "function that switches read-only off, then inserts",
            format!("SELECT {f_flip}()"),
        )
        .trace(rows(3)),
        refused("DO a function that inserts", format!("DO {f_insert}()")).trace(rows(3)),
        refused(
            "CALL a procedure that inserts",
            format!("CALL {p_insert}()"),
        )
        .trace(rows(3)),
        // Not in the plan's list. Under the session setting alone this one
        // wrote on both servers: a procedure runs each statement as its own,
        // so the INSERT ran read-write. The read-only transaction stops it.
        refused(
            "CALL a procedure that switches read-only off, then inserts",
            format!("CALL {p_flip}()"),
        )
        .trace(rows(3)),
        refused(
            "CALL a procedure that opens its own transaction",
            format!("CALL {p_txn}()"),
        )
        .trace(rows(3)),
        refused("CREATE TABLE", format!("CREATE TABLE {created} (a INT)")).trace(absent(&created)),
        refused("DROP TABLE", format!("DROP TABLE {t}")).trace(rows(3)),
        refused(
            "ALTER TABLE",
            format!("ALTER TABLE {t} ADD COLUMN extra INT"),
        )
        .trace(Check::count(
            format!(
                "SELECT COUNT(*) AS n FROM information_schema.columns \
                 WHERE table_schema = DATABASE() AND table_name = '{t}'"
            ),
            2,
        )),
        refused("TRUNCATE", format!("TRUNCATE TABLE {t}")).trace(rows(3)),
        refused("RENAME TABLE", format!("RENAME TABLE {t} TO {renamed}"))
            .trace(rows(3))
            .trace(absent(&renamed)),
        refused(
            "CREATE INDEX",
            format!("CREATE INDEX {index} ON {t} (label)"),
        )
        .trace(Check::count(
            format!(
                "SELECT COUNT(*) AS n FROM information_schema.statistics \
                 WHERE table_schema = DATABASE() AND table_name = '{t}' AND index_name = '{index}'"
            ),
            0,
        )),
        refused("CREATE DATABASE", format!("CREATE DATABASE {database}")).trace(Check::count(
            format!(
                "SELECT COUNT(*) AS n FROM information_schema.schemata \
                 WHERE schema_name = '{database}'"
            ),
            0,
        )),
        refused(
            "CREATE TEMPORARY TABLE",
            format!("CREATE TEMPORARY TABLE {temp} (a INT)"),
        )
        .trace(Check::on_connections(POOL, table_missing(&temp))),
        // The prepared-statement protocol takes one statement: a syntax error.
        Attack::new("two statements", format!("SELECT 1; DELETE FROM {t}"))
            .refused()
            .trace(rows(3)),
        // Session state: it runs on the read-only connection, which is
        // closed afterwards. `after_each` checks every pooled connection's
        // `sql_mode`.
        Attack::new(
            "SET SESSION sql_mode",
            "SET SESSION sql_mode = 'ANSI_QUOTES'",
        )
        .trace(Check::on_connections(POOL, session_is_default(&sql_mode))),
        Attack::allowed("GET_LOCK", format!("SELECT GET_LOCK('{lock}', 0) AS got"))
            .returns(vec![vec![Value::Int(1)]])
            .trace(lock_free(&lock)),
        // Not a write, and the only statement; the connection is closed.
        Attack::new(
            "SET SESSION transaction_read_only = 0",
            "SET SESSION transaction_read_only = 0",
        )
        .trace(still_refuses(format!("INSERT INTO {t} (id) VALUES (91)")))
        .trace(rows(3)),
    ];

    let spec = ReadOnlySpec {
        setup: vec![
            format!("CREATE TABLE {t} (id INT PRIMARY KEY, label VARCHAR(20))"),
            format!("INSERT INTO {t} (id) VALUES (1), (2), (3)"),
            format!("CREATE TABLE {hygiene} (a INT)"),
        ],
        teardown: vec![
            format!("DROP TABLE IF EXISTS {hygiene}"),
            format!("DROP TABLE IF EXISTS {t}"),
            // Only there if an attack got through.
            format!("DROP TABLE IF EXISTS {created}"),
            format!("DROP TABLE IF EXISTS {renamed}"),
            format!("DROP DATABASE IF EXISTS {database}"),
        ],
        attacks,
        // Each copy sleeps so the pool hands out a different connection to
        // each: no pooled session is read-only or changed, and every one
        // still writes.
        after_each: vec![
            Check::on_connections(POOL, session_is_default(&sql_mode)),
            Check::on_connections(
                POOL,
                Check::executes(format!("INSERT INTO {hygiene} (a) SELECT SLEEP(0.05)")),
            ),
        ],
        // Called on the driver directly: Core's token check would refuse it.
        slow_query: "SELECT SLEEP(5) AS s".into(),
        ..Default::default()
    };

    if let Err(e) = raw(&config, &routines).await {
        let _ = raw(&config, &drop_routines).await;
        panic!("routine setup failed: {e}");
    }
    let outcome = AssertUnwindSafe(run_read_only(&*engine, &config, &spec))
        .catch_unwind()
        .await;
    if let Err(e) = raw(&config, &drop_routines).await {
        eprintln!("routine teardown failed: {e}");
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

/// The session a pooled connection should have: read-write, and sqlx's
/// `sql_mode`. Sleeps so concurrent copies land on different connections.
fn session_is_default(sql_mode: &Value) -> Check {
    Check::rows(
        "SELECT @@session.transaction_read_only AS ro, @@session.sql_mode AS m \
         FROM (SELECT SLEEP(0.05) AS s) AS s",
        vec![vec![Value::Int(0), sql_mode.clone()]],
    )
}

/// `temp` doesn't exist on the connection this runs on: no pooled session
/// holds a temporary table by that name.
fn table_missing(temp: &str) -> Check {
    let sql = format!("SELECT COUNT(*) AS n FROM {temp}, (SELECT SLEEP(0.05) AS s) AS s");
    Check::custom(format!("no session has table {temp}"), move |driver| {
        let sql = sql.clone();
        async move {
            match driver.query(&sql, vec![]).await {
                Err(e) if e.message.contains("1146") => Ok(()),
                other => Err(format!(
                    "expected error 1146 (no such table), got {other:?}"
                )),
            }
        }
    })
}

/// No session holds user lock `name`. The read-only connection is closed
/// when the call returns, but the server ends its thread a moment later, so
/// this polls for up to two seconds.
fn lock_free(name: &str) -> Check {
    let name = name.to_string();
    Check::custom(format!("lock {name} is free"), move |driver| {
        let sql = format!("SELECT IS_FREE_LOCK('{name}') AS free");
        let name = name.clone();
        async move {
            for _ in 0..40 {
                let r = driver
                    .query(&sql, vec![])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
                if r.rows[0][0].as_i64() == Some(1) {
                    return Ok(());
                }
                driver
                    .query("SELECT SLEEP(0.05) AS s", vec![])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
            }
            Err(format!("a session still holds lock {name}"))
        }
    })
}

/// The next read-only call refuses `write` with `READ_ONLY`: nothing an
/// attack set carried over to it.
fn still_refuses(write: String) -> Check {
    Check::custom(
        "the next read-only call still refuses writes",
        move |driver| {
            let write = write.clone();
            async move {
                match driver.query_read_only(&write, vec![]).await {
                    Err(e) if e.code == "READ_ONLY" => Ok(()),
                    other => Err(format!("{write}: expected READ_ONLY, got {other:?}")),
                }
            }
        },
    )
}

/// Why the session setting is needed: under `START TRANSACTION READ ONLY`
/// alone, DDL commits the transaction implicitly and then runs, so `CREATE
/// TABLE` leaves a table on both servers.
#[tokio::test]
async fn read_only_transaction_alone_lets_ddl_through() {
    for server in [Server::Mysql, Server::Mariadb] {
        let Some(config) = server.config() else {
            continue;
        };
        let table = scratch_name("seaquel_ro_ddl_");
        let result = raw(
            &config,
            &[
                "START TRANSACTION READ ONLY".to_string(),
                format!("CREATE TABLE {table} (a INT)"),
                "ROLLBACK".to_string(),
            ],
        )
        .await;
        let driver = seaquel_engine_mysql::engine()
            .open(&config)
            .await
            .expect("open");
        let exists = driver
            .query(
                &format!(
                    "SELECT COUNT(*) AS n FROM information_schema.tables \
                     WHERE table_schema = DATABASE() AND table_name = '{table}'"
                ),
                vec![],
            )
            .await
            .expect("count");
        let _ = driver
            .execute(&format!("DROP TABLE IF EXISTS {table}"), vec![])
            .await;
        driver.close().await.expect("close");
        result.expect("CREATE TABLE under START TRANSACTION READ ONLY");
        assert_eq!(
            exists.rows[0][0].as_i64(),
            Some(1),
            "{server:?}: the table should exist"
        );
    }
}
