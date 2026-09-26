#![cfg(feature = "engine-sqlite")]

use futures::StreamExt;
use seaquel_core::{Core, QueryOptions, StreamEvent};
use seaquel_engine::{BatchStatement, ConnectConfig, Value};
use serde_json::json;
use std::path::PathBuf;

/// A SQLite file in the temp dir, removed on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("seaquel-core-{}.sqlite", uuid::Uuid::new_v4())))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", self.0.display()),
            "create_if_missing": true
        }))
        .unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn core() -> Core {
    seaquel_core::with_default_plugins().build()
}

/// Connect and create `nums(n)` holding 1..=rows.
async fn connect_with_rows(core: &Core, db: &TempDb, rows: u32) -> String {
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    core.execute(&id, "CREATE TABLE nums (n INTEGER)", vec![])
        .await
        .unwrap();
    core.execute(
        &id,
        &format!(
            "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<{rows}) \
             INSERT INTO nums SELECT n FROM seq"
        ),
        vec![],
    )
    .await
    .unwrap();
    id
}

fn row_count(events: &[StreamEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            StreamEvent::Batch(b) => b.rows.len(),
            _ => 0,
        })
        .sum()
}

#[tokio::test]
async fn connect_query_disconnect() {
    let core = core();
    let db = TempDb::new();
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    assert!(id.starts_with("sqlite-"), "{id}");
    assert_eq!(core.connection_count(), 1);

    let r = core.query(&id, "SELECT 1 AS one", vec![]).await.unwrap();
    assert_eq!(r.columns, vec!["one"]);
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);

    core.disconnect(&id).await.unwrap();
    assert_eq!(core.connection_count(), 0);
    let err = core.query(&id, "SELECT 1", vec![]).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");

    // Idempotent.
    core.disconnect(&id).await.unwrap();
}

#[tokio::test]
async fn test_does_not_register_a_connection() {
    let core = core();
    let db = TempDb::new();
    core.test(&db.config()).await.unwrap();
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn connect_without_the_engine_fails() {
    let core = Core::builder().build();
    let db = TempDb::new();
    let err = core.connect(&db.config()).await.unwrap_err();
    assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
}

#[tokio::test]
async fn transaction_is_all_or_nothing() {
    let core = core();
    let db = TempDb::new();
    let id = core.connect(&db.config()).await.unwrap().connection_id;
    core.execute(&id, "CREATE TABLE t (id INTEGER PRIMARY KEY)", vec![])
        .await
        .unwrap();

    let insert = |v: i64| BatchStatement {
        sql: "INSERT INTO t (id) VALUES (?)".into(),
        params: vec![Value::from(v)],
        expect_rows: None,
    };
    assert!(core
        .transaction(&id, vec![insert(1), insert(1)])
        .await
        .is_err());

    let r = core
        .query(&id, "SELECT COUNT(*) AS c FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(r.rows, vec![vec![Value::Int(0)]]);
}

#[tokio::test]
async fn stream_emits_batches_then_done() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 12_345).await;

    let events: Vec<StreamEvent> = core
        .query_stream(
            "q1".into(),
            id,
            "SELECT n FROM nums ORDER BY n".into(),
            vec![],
            QueryOptions::default(),
        )
        .collect()
        .await;

    assert_eq!(row_count(&events), 12_345);
    let batches = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Batch(_)))
        .count();
    assert!(
        batches >= 2,
        "expected multi-batch streaming, got {batches}"
    );
    assert!(matches!(events.last(), Some(StreamEvent::Done)));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Done))
            .count(),
        1
    );
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn stream_on_an_unknown_connection_emits_one_error() {
    let core = core();
    let events: Vec<StreamEvent> = core
        .query_stream(
            "q1".into(),
            "nope".into(),
            "SELECT 1".into(),
            vec![],
            QueryOptions::default(),
        )
        .collect()
        .await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        StreamEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_NOT_FOUND"),
        other => panic!("expected an error event, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_ends_the_stream_without_a_terminal_event() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 200_000).await;

    let mut stream = core.query_stream(
        "q1".into(),
        id,
        "SELECT n FROM nums".into(),
        vec![],
        QueryOptions::default(),
    );
    let first = stream.next().await.unwrap();
    assert!(matches!(first, StreamEvent::Batch(_)));

    core.cancel_stream("q1");
    let rest: Vec<StreamEvent> = stream.collect().await;

    assert!(
        rest.iter().all(|e| matches!(e, StreamEvent::Batch(_))),
        "no Done or Error after cancel: {rest:?}"
    );
    let total = row_count(&[first]) + row_count(&rest);
    assert!(
        total < 200_000,
        "cancel should stop fetching, got all {total} rows"
    );
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn dropping_a_stream_unregisters_it() {
    let core = core();
    let stream = core.query_stream(
        "q1".into(),
        "nope".into(),
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );
    assert_eq!(core.running_stream_count(), 1);
    drop(stream);
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn a_reused_query_id_cancels_the_newest_stream() {
    let core = core();
    let db = TempDb::new();
    let id = connect_with_rows(&core, &db, 10).await;

    let older = core.query_stream(
        "q".into(),
        id.clone(),
        "SELECT n FROM nums".into(),
        vec![],
        QueryOptions::default(),
    );
    let newer = core.query_stream(
        "q".into(),
        id,
        "SELECT n FROM nums".into(),
        vec![],
        QueryOptions::default(),
    );
    drop(older);
    assert_eq!(
        core.running_stream_count(),
        1,
        "dropping the older stream must not unregister the newer one"
    );

    core.cancel_stream("q");
    let events: Vec<StreamEvent> = newer.collect().await;
    assert!(events.is_empty(), "cancelled before it started: {events:?}");
}
