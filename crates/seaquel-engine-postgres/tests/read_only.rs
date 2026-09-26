//! `query_read_only` on Postgres (AI safety plan, Task 2): every attack goes
//! through the read-only path and is then checked from a normal session.
//!
//! Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
//! {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}
//!
//! Accepted gaps (plan, "Probe results"), not asserted here:
//! - A function that already exists and has outside effects still runs
//!   them: `COPY … TO PROGRAM` inside a plpgsql function ran under `BEGIN
//!   READ ONLY` as superuser, and `dblink_exec` opens its own session. Fix
//!   14's token check blocks both when the AI writes them itself.
//! - Server-admin functions (`pg_terminate_backend`, `pg_reload_conf`)
//!   aren't writes, so a read-only transaction runs them. Fix 14 blocks them
//!   by name.

use seaquel_engine::Value;
use seaquel_engine_testkit::{
    config_from_env, run_read_only, scratch_name, Attack, Check, ReadOnlySpec,
};

/// sqlx's default `max_connections`, which the driver doesn't change: the
/// most connections the pool can hold, so `after_each` reaches them all.
const POOL: usize = 10;

/// Postgres refuses a write in a read-only transaction with SQLSTATE 25006,
/// which the driver reports as `READ_ONLY` with the server's message.
const REFUSAL: &str = "in a read-only transaction";

#[tokio::test]
async fn read_only() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let engine = seaquel_engine_postgres::engine();

    let t = scratch_name("seaquel_ro_");
    let hygiene = scratch_name("seaquel_ro_hyg_");
    let seq = scratch_name("seaquel_ro_seq_");
    let f_insert = scratch_name("seaquel_ro_f_");
    let f_definer = scratch_name("seaquel_ro_fd_");
    let p_flip = scratch_name("seaquel_ro_pf_");
    let p_commit = scratch_name("seaquel_ro_pc_");
    let into = scratch_name("seaquel_ro_into_");
    let created = scratch_name("seaquel_ro_new_");
    let temp = scratch_name("seaquel_ro_tmp_");
    // An advisory lock key no other test takes: under 2^31, so it's
    // `classid = 0, objid = key` in `pg_locks`.
    let lock_key = i64::from(u32::from_str_radix(&t[t.len() - 7..], 16).unwrap());

    // Large objects have server-wide OIDs, so `lo_create(0)` is checked
    // against the count before the run.
    let large_objects = {
        let driver = engine.open(&config).await.expect("open");
        let r = driver
            .query("SELECT count(*) FROM pg_largeobject_metadata", vec![])
            .await
            .expect("count large objects");
        driver.close().await.expect("close");
        r.rows[0][0].as_i64().expect("count")
    };

    let rows = |n: i64| Check::count(format!("SELECT count(*) FROM {t}"), n);
    let absent = |name: &str| {
        Check::count(
            format!("SELECT count(*) FROM pg_class WHERE relname = '{name}'"),
            0,
        )
    };
    let seq_unchanged = Check::rows(
        format!("SELECT last_value, is_called FROM {seq}"),
        vec![vec![Value::Int(1), Value::Bool(false)]],
    );
    let refused = |name: &str, sql: String| {
        Attack::new(name, sql)
            .refused_with("READ_ONLY")
            .message_contains(REFUSAL)
    };

    let attacks = vec![
        Attack::allowed("SELECT", format!("SELECT count(*) AS n FROM {t}"))
            .returns(vec![vec![Value::Int(3)]]),
        Attack::allowed("WITH", "WITH x AS (SELECT 1 AS a) SELECT a FROM x")
            .returns(vec![vec![Value::Int(1)]]),
        Attack::allowed("bind values", "SELECT $1::int8 AS n")
            .params(vec![Value::Int(5)])
            .returns(vec![vec![Value::Int(5)]]),
        refused("INSERT", format!("INSERT INTO {t} VALUES (99)"))
            .message_contains("cannot execute INSERT in a read-only transaction")
            .trace(rows(3)),
        refused("function that inserts", format!("SELECT {f_insert}()")).trace(rows(3)),
        // SQLSTATE 25001, not 25006: the SELECT took a snapshot, so the mode
        // can no longer change. Refused either way.
        Attack::new(
            "SECURITY DEFINER function that switches read-only off first",
            format!("SELECT {f_definer}()"),
        )
        .refused()
        .trace(rows(3)),
        Attack::new(
            "set_config switches read-only off, then a function inserts",
            format!("SELECT set_config('transaction_read_only', 'off', true), {f_insert}()"),
        )
        .refused()
        .trace(rows(3)),
        // Not in the plan's list. `DO` and `CALL` take a snapshot before
        // they run, and a procedure can't end a transaction it didn't
        // start ("invalid transaction termination").
        Attack::new(
            "DO block that switches read-only off, then inserts",
            format!(
                "DO $$ BEGIN SET LOCAL transaction_read_only = off; \
                 INSERT INTO {t} VALUES (98); END $$"
            ),
        )
        .refused()
        .trace(rows(3)),
        Attack::new(
            "DO block that commits, then inserts",
            format!("DO $$ BEGIN COMMIT; INSERT INTO {t} VALUES (97); END $$"),
        )
        .refused()
        .trace(rows(3)),
        Attack::new(
            "CALL a procedure that switches read-only off, then inserts",
            format!("CALL {p_flip}()"),
        )
        .refused()
        .trace(rows(3)),
        Attack::new(
            "CALL a procedure that commits, then inserts",
            format!("CALL {p_commit}()"),
        )
        .refused()
        .trace(rows(3)),
        refused("SELECT … INTO", format!("SELECT 1 AS x INTO {into}")).trace(absent(&into)),
        refused("CREATE TABLE", format!("CREATE TABLE {created} (a int)")).trace(absent(&created)),
        refused("DROP TABLE", format!("DROP TABLE {t}")).trace(rows(3)),
        refused(
            "data-modifying CTE",
            format!("WITH d AS (DELETE FROM {t} RETURNING *) SELECT * FROM d"),
        )
        .trace(rows(3)),
        refused("nextval", format!("SELECT nextval('{seq}')")).trace(seq_unchanged.clone()),
        refused("setval", format!("SELECT setval('{seq}', 100)")).trace(seq_unchanged),
        // The extended protocol takes one statement ("cannot insert multiple
        // commands into a prepared statement").
        Attack::new("two statements", format!("SELECT 1; DELETE FROM {t}"))
            .refused()
            .trace(rows(3)),
        refused(
            "CREATE TEMP TABLE",
            format!("CREATE TEMP TABLE {temp} (a int)"),
        )
        .trace(absent(&temp)),
        refused("lo_create", "SELECT lo_create(0)".to_string()).trace(Check::count(
            "SELECT count(*) FROM pg_largeobject_metadata",
            large_objects,
        )),
        refused(
            "SELECT … FOR UPDATE",
            format!("SELECT * FROM {t} FOR UPDATE"),
        )
        .trace(Check::count(
            format!(
                "SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation \
                 WHERE c.relname = '{t}'"
            ),
            0,
        )),
        // A session lock survives ROLLBACK; closing the connection frees it.
        Attack::allowed(
            "session advisory lock",
            format!("SELECT 1 AS one FROM pg_advisory_lock({lock_key})"),
        )
        .returns(vec![vec![Value::Int(1)]])
        .trace(advisory_lock_free(lock_key)),
        // Accepted as the first statement of a transaction (no snapshot yet),
        // so it runs; it's the only statement, and the connection is closed.
        Attack::new(
            "SET transaction_read_only = off",
            "SET transaction_read_only = off",
        )
        .trace(still_refuses(format!("INSERT INTO {t} VALUES (96)")))
        .trace(rows(3)),
    ];

    let spec = ReadOnlySpec {
        setup: vec![
            format!("CREATE TABLE {t} (id int PRIMARY KEY)"),
            format!("INSERT INTO {t} VALUES (1), (2), (3)"),
            format!("CREATE TABLE {hygiene} (a int)"),
            format!("CREATE SEQUENCE {seq}"),
            format!(
                "CREATE FUNCTION {f_insert}() RETURNS int LANGUAGE plpgsql AS $$ \
                 BEGIN INSERT INTO {t} VALUES (95); RETURN 1; END $$"
            ),
            format!(
                "CREATE FUNCTION {f_definer}() RETURNS int LANGUAGE plpgsql SECURITY DEFINER AS $$ \
                 BEGIN SET LOCAL transaction_read_only = off; INSERT INTO {t} VALUES (94); RETURN 1; END $$"
            ),
            format!(
                "CREATE PROCEDURE {p_flip}() LANGUAGE plpgsql AS $$ \
                 BEGIN SET LOCAL transaction_read_only = off; INSERT INTO {t} VALUES (93); END $$"
            ),
            format!(
                "CREATE PROCEDURE {p_commit}() LANGUAGE plpgsql AS $$ \
                 BEGIN COMMIT; INSERT INTO {t} VALUES (92); END $$"
            ),
        ],
        teardown: vec![
            format!("DROP PROCEDURE IF EXISTS {p_commit}()"),
            format!("DROP PROCEDURE IF EXISTS {p_flip}()"),
            format!("DROP FUNCTION IF EXISTS {f_definer}()"),
            format!("DROP FUNCTION IF EXISTS {f_insert}()"),
            format!("DROP SEQUENCE IF EXISTS {seq}"),
            format!("DROP TABLE IF EXISTS {hygiene}"),
            format!("DROP TABLE IF EXISTS {t}"),
            // Only there if an attack got through.
            format!("DROP TABLE IF EXISTS {into}"),
            format!("DROP TABLE IF EXISTS {created}"),
        ],
        attacks,
        // Each copy sleeps so the pool hands out a different connection to
        // each: no pooled session is read-only, and every one still writes.
        after_each: vec![
            Check::on_connections(
                POOL,
                Check::value(
                    "SELECT current_setting('transaction_read_only') AS ro FROM pg_sleep(0.05)",
                    "off",
                ),
            ),
            Check::on_connections(
                POOL,
                Check::executes(format!("INSERT INTO {hygiene} SELECT 1 FROM pg_sleep(0.05)")),
            ),
        ],
        // Called on the driver directly: Core's token check would refuse it.
        slow_query: "SELECT 1 AS one FROM pg_sleep(5)".into(),
        ..Default::default()
    };

    run_read_only(&*engine, &config, &spec).await;
}

/// No session holds advisory lock `key`. The read-only connection is closed
/// when the call returns, but the server ends its backend a moment later, so
/// this polls for up to two seconds.
fn advisory_lock_free(key: i64) -> Check {
    Check::custom(
        format!("advisory lock {key} is free"),
        move |driver| async move {
            let sql = format!(
                "SELECT count(*) AS n FROM pg_locks \
             WHERE locktype = 'advisory' AND classid = 0 AND objid = {key} AND objsubid = 1"
            );
            for _ in 0..40 {
                let r = driver
                    .query(&sql, vec![])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
                if r.rows[0][0].as_i64() == Some(0) {
                    return Ok(());
                }
                driver
                    .query("SELECT 1 AS one FROM pg_sleep(0.05)", vec![])
                    .await
                    .map_err(|e| format!("{e:?}"))?;
            }
            Err(format!("a session still holds advisory lock {key}"))
        },
    )
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

/// A result past the row cap fails fast with `RESULT_TOO_LARGE`: the driver
/// closes the connection instead of sending `ROLLBACK`, which would first
/// read every remaining row (1.5–2.5 s on 5M rows in review). The set-
/// returning function is in the select list, so the server streams its rows
/// instead of materialising them first.
#[tokio::test]
async fn over_the_row_cap_fails_fast() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let cap = seaquel_engine::max_query_rows();
    let driver = seaquel_engine_postgres::engine()
        .open(&config)
        .await
        .expect("open");
    let sql = format!("SELECT generate_series(1, {}) AS n", cap * 200);
    let started = tokio::time::Instant::now();
    let err = driver
        .query_read_only(&sql, vec![])
        .await
        .expect_err("past the row cap");
    let elapsed = started.elapsed();
    // Still usable afterwards.
    let one = driver.query_read_only("SELECT 1 AS one", vec![]).await;
    driver.close().await.expect("close");
    assert_eq!(err.code, "RESULT_TOO_LARGE", "{err:?}");
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "{elapsed:?} to refuse {} rows with a cap of {cap}",
        cap * 200
    );
    assert_eq!(one.expect("SELECT 1").rows, vec![vec![Value::Int(1)]]);
}
