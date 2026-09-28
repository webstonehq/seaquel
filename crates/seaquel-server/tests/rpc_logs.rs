//! What the server's logger writes for `/rpc` (phase 5b probe, M1 and M2):
//! records go through `startup::format_record`, the formatter the stderr
//! logger uses, so key-values are written; and a failed call is logged by
//! its code and method, never by its message, which can quote SQL and
//! values.
//!
//! Its own test binary, since the logger is global.

use std::sync::{Mutex, Once, PoisonError};

use axum::http::StatusCode;
use serde_json::json;

mod common;
use common::{next, open_stream, pg_form, send, start_run, Env};

static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        seaquel_server::startup::logs(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            RECORDS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(seaquel_server::startup::format_record(record));
        }
    }

    fn flush(&self) {}
}

fn capture_logs() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
}

fn records() -> Vec<String> {
    RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn a_failed_call_logs_its_code_and_method_but_not_its_message() {
    capture_logs();
    let env = Env::new(4);
    let c = env.connect("alice", pg_form()).await;
    let canary = format!("canaryM1{}", std::process::id());

    // The database's error quotes the SQL.
    let (status, body) = env
        .db(
            "alice",
            "query",
            json!({"connectionId": c, "sql": format!("SELECT echo-fail '{canary}'")}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains(&canary),
        "{body}"
    );

    // A parse error quotes the unknown method.
    let (status, _) = env
        .rpc(
            "alice",
            &json!({"method": "storage", "params": {"method": canary}}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let records = records();
    let failures: Vec<&String> = records
        .iter()
        .filter(|r| r.contains("activity=rpc.error"))
        .collect();
    assert!(
        failures.iter().any(|r| r.contains("code=QUERY_ERROR")
            && r.contains("group=db")
            && r.contains("method=query")),
        "{records:#?}"
    );
    assert!(
        failures.iter().any(|r| r.contains("code=INVALID_ARGUMENT")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}

#[test]
fn the_formatter_writes_key_values() {
    let line = seaquel_server::startup::format_record(
        &log::Record::builder()
            .level(log::Level::Info)
            .target("seaquel_core")
            .args(format_args!("Connected"))
            .key_values(&[("activity", "db.connect"), ("connection_id", "c1")])
            .build(),
    );
    assert_eq!(
        line,
        "[seaquel-server] INFO seaquel_core: Connected activity=db.connect connection_id=c1"
    );
}

/// Phase 5b review (I1): a key-value can come from the browser, so the
/// formatter escapes control characters (one record, one line) and cuts
/// each value at 128 bytes.
#[test]
fn the_formatter_escapes_and_cuts_values() {
    let long = "x".repeat(10_000);
    let line = seaquel_server::startup::format_record(
        &log::Record::builder()
            .level(log::Level::Info)
            .target("seaquel_core::run")
            .args(format_args!("Run"))
            .key_values(&[
                (
                    "stream_id",
                    "s\n[seaquel-server] WARN forged: \u{1b}[31mred",
                ),
                ("connection_id", long.as_str()),
            ])
            .build(),
    );
    assert!(!line.contains('\n') && !line.contains('\u{1b}'), "{line}");
    assert!(
        line.contains(r#"stream_id="s\n[seaquel-server] WARN forged: \u{1b}[31mred""#),
        "{line}"
    );
    assert!(
        line.contains(&format!("connection_id={}…", "x".repeat(128))),
        "{line}"
    );
    assert!(line.len() < 400, "{}", line.len());
}

/// Phase 5b review: a value holding a space, `=`, `"` or `\` is quoted
/// (logfmt), so a browser-supplied id can't forge a field; plain values
/// stay bare.
#[test]
fn the_formatter_quotes_values_that_could_forge_fields() {
    let line = seaquel_server::startup::format_record(
        &log::Record::builder()
            .level(log::Level::Warn)
            .target("seaquel_core::run")
            .args(format_args!("Run"))
            .key_values(&[
                ("connection_id", "x code=OK"),
                ("stream_id", r#"a"b\c"#),
                ("activity", "db.run"),
            ])
            .build(),
    );
    assert!(line.contains(r#"connection_id="x code=OK""#), "{line}");
    assert!(line.contains(r#"stream_id="a\"b\\c""#), "{line}");
    assert!(line.ends_with(" activity=db.run"), "{line}");
    // A logfmt reader sees no `code` field: it is inside the quotes.
    assert!(!line.contains("code=OK "), "{line}");
}

/// Phase 5b review (I1): `/rpc/stream` takes a `streamId` of at most 128
/// characters of `[A-Za-z0-9_.:-]`, and doesn't echo one it refuses. A
/// connection id Core logs before checking it is still one escaped line.
#[tokio::test]
async fn stream_ids_are_checked_and_logged_ids_are_one_line() {
    capture_logs();
    let env = Env::new(4);
    let addr = env.serve().await;
    let mut ws = open_stream(addr, "alice").await;
    for id in [
        "a\nb".to_string(),
        "x".repeat(129),
        "with space".to_string(),
        "é".to_string(),
    ] {
        send(&mut ws, &start_run(&id, "c", "SELECT 1", 10)).await;
        let frame = next(&mut ws).await;
        assert_eq!(
            frame["event"]["code"], "INVALID_ARGUMENT",
            "{id:?}: {frame}"
        );
        assert_eq!(frame["streamId"], "", "{frame}");
    }
    // The clients' ids (UUIDs) and every allowed character pass.
    let ok = format!("{}_.:-{}", "a1-B2", "9".repeat(119));
    assert_eq!(ok.len(), 128);
    let forged = "c\n[seaquel-server] WARN forged";
    send(&mut ws, &start_run(&ok, forged, "SELECT 1", 10)).await;
    let frame = next(&mut ws).await;
    assert_eq!(frame["streamId"], ok.as_str(), "{frame}");
    assert_eq!(frame["event"]["code"], "CONNECTION_NOT_FOUND", "{frame}");

    let records = records();
    assert!(
        records.iter().any(|r| r.contains("activity=db.run")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains('\n'), "{record}");
    }
}
