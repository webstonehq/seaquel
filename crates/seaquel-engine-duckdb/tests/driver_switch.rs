//! The suites' driver switch (`common/engine.rs`, the DuckDB helper plan's
//! Task 5): the driver a suite gets is the one `SEAQUEL_TEST_DUCKDB_DRIVER`
//! asked for, told apart by what it does rather than by the switch's own
//! word, so a CI job that meant the remote driver can't pass by testing the
//! native one twice.

use seaquel_engine::ConnectConfig;

#[path = "common/engine.rs"]
mod engine_switch;

use engine_switch::Which;

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

/// A statement past the helper's frame (16 MiB): the remote driver refuses
/// it before sending anything (`INVALID_ARGUMENT`, a difference listed in
/// `REMOTE.md`); the native driver runs it.
#[tokio::test]
async fn the_suites_run_on_the_driver_they_ask_for() {
    let which = engine_switch::which();
    eprintln!("SEAQUEL_TEST_DUCKDB_DRIVER: {which:?}");
    let driver = engine_switch::engine().open(&memory()).await.expect("open");
    let sql = format!("SELECT 42 AS n /* {} */", "x".repeat(17 << 20));
    let result = driver.query(&sql, vec![]).await;
    match which {
        Which::Native => {
            let r = result.expect("the native driver runs a 17 MiB statement");
            assert_eq!(r.rows.len(), 1);
        }
        Which::Remote => {
            let e = match result {
                Ok(_) => panic!("a 17 MiB statement ran: this is not the remote driver"),
                Err(e) => e,
            };
            assert_eq!(e.code, "INVALID_ARGUMENT", "{e}");
            assert!(e.message.contains("DuckDB helper"), "{e}");
        }
    }
    // Either way the connection still answers.
    let r = driver.query("SELECT 1", vec![]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    driver.close().await.unwrap();
}

/// A row past the helper's frame: the remote driver fails the call with
/// `RESULT_TOO_LARGE` and goes on; the native driver returns it. Listed in
/// `REMOTE.md`.
#[tokio::test]
async fn a_row_past_the_frame_is_too_large_only_remotely() {
    let driver = engine_switch::engine().open(&memory()).await.expect("open");
    let result = driver
        .query("SELECT repeat('x', 20 * 1024 * 1024) AS big", vec![])
        .await;
    match engine_switch::which() {
        Which::Native => {
            let r = result.expect("the native driver returns a 20 MiB row");
            assert_eq!(r.rows.len(), 1);
        }
        Which::Remote => {
            let e = match result {
                Ok(_) => panic!("a 20 MiB row came back: this is not the remote driver"),
                Err(e) => e,
            };
            assert_eq!(e.code, "RESULT_TOO_LARGE", "{e}");
        }
    }
    let r = driver.query("SELECT 1", vec![]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    driver.close().await.unwrap();
}
