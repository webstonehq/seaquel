//! The helper's frame limits (`HELPER.md`, "Limits"): a statement or a row
//! must fit one 16 MiB frame. A statement past it is refused before
//! anything is sent, naming no SQL; a row past it fails its call with
//! `RESULT_TOO_LARGE`; a row under it arrives whole. The connection answers
//! afterwards either way.

use seaquel_engine::ConnectConfig;

#[path = "common/engine.rs"]
mod engine;

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

#[tokio::test]
async fn a_statement_past_the_frame_is_refused_before_it_is_sent() {
    let driver = engine::engine().open(&memory()).await.expect("open");
    let marker = "seaquel-sql-text-marker";
    let sql = format!("SELECT 42 AS n /* {marker} {} */", "x".repeat(17 << 20));
    let e = match driver.query(&sql, vec![]).await {
        Ok(_) => panic!("a 17 MiB statement ran"),
        Err(e) => e,
    };
    assert_eq!(e.code, "INVALID_ARGUMENT", "{}", e.code);
    assert!(e.message.contains("DuckDB helper"), "{}", e.message);
    assert!(!e.message.contains(marker), "the SQL is in the message");
    assert!(e.message.len() < 1024, "{} bytes", e.message.len());
    let r = driver.query("SELECT 1", vec![]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    driver.close().await.unwrap();
}

#[tokio::test]
async fn a_row_past_the_frame_is_too_large() {
    let driver = engine::engine().open(&memory()).await.expect("open");
    let e = match driver
        .query("SELECT repeat('x', 20 * 1024 * 1024) AS big", vec![])
        .await
    {
        Ok(_) => panic!("a 20 MiB row came back"),
        Err(e) => e,
    };
    assert_eq!(e.code, "RESULT_TOO_LARGE", "{e}");
    let r = driver.query("SELECT 1", vec![]).await.unwrap();
    assert_eq!(r.rows.len(), 1);
    driver.close().await.unwrap();
}

#[tokio::test]
async fn a_row_under_the_frame_arrives() {
    let driver = engine::engine().open(&memory()).await.expect("open");
    let r = driver
        .query(
            "SELECT repeat('x', 12 * 1024 * 1024) AS big, 7 AS n",
            vec![],
        )
        .await
        .expect("a 12 MiB row");
    assert_eq!(r.rows.len(), 1);
    match &r.rows[0][0] {
        seaquel_engine::Value::Text(s) => assert_eq!(s.len(), 12 << 20),
        other => panic!("{:?}", std::mem::discriminant(other)),
    }
    assert_eq!(r.rows[0][1], seaquel_engine::Value::Int(7));
    driver.close().await.unwrap();
}
