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
use common::{next, open_stream, pg_form, send, start_run, start_table_page, until_end, Env};

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

/// The edits service logs activity names, codes, counts and the group and
/// method of a failed call, never a key, a value, typed SQL or a filter
/// value.
#[tokio::test]
async fn apply_logs_code_group_and_method_only() {
    capture_logs();
    let env = Env::new(4);
    let addr = env.serve().await;
    let c = env.connect("alice", pg_form()).await;
    let canary = format!("canaryEd{}", std::process::id());
    let target = json!({"schema": "public", "table": "t"});

    // A refused plan (the key isn't the primary key), its key a canary.
    let (status, body) = env
        .db(
            "alice",
            "planEdits",
            json!({"connectionId": c, "edits": [
                {"type": "updateCell", "target": target, "key": [["name", format!("{canary}-key")]],
                 "column": "name", "value": format!("{canary}-value")}]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "NOT_EDITABLE");

    // An apply with canaries in a key, a value, typed SQL and its params,
    // and one refused in its outcome (two statements in one change).
    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            json!({"connectionId": c, "changes": [
                {"type": "edit", "id": "p1", "edit": {"type": "updateCell", "target": target,
                 "key": [["id", format!("{canary}-key")]], "column": "name", "value": format!("{canary}-value")}},
                {"type": "sql", "id": "p2", "sql": format!("UPDATE t SET name = '{canary}-sql' WHERE id = $1"),
                 "params": [format!("{canary}-param")]},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["result"]["applied"], 2, "{body}");
    let (status, body) = env
        .db(
            "alice",
            "applyChanges",
            json!({"connectionId": c, "changes": [
                {"type": "sql", "id": "p1", "sql": format!("DELETE FROM t WHERE a = '{canary}-1'; DELETE FROM t")},
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["result"]["result"]["failed"]["code"], "INVALID_ARGUMENT",
        "{body}"
    );

    // A table page with a canary filter value.
    let mut ws = open_stream(addr, "alice").await;
    send(
        &mut ws,
        &start_table_page(
            "t1",
            &c,
            "t",
            json!([{"column": "name", "op": "=", "value": format!("{canary}-filter")}]),
            1,
            10,
        ),
    )
    .await;
    let got = until_end(&mut ws, "t1", &mut Vec::new()).await;
    assert_eq!(got.last().unwrap()["event"]["type"], "done", "{got:?}");

    let records = records();
    assert!(
        records.iter().any(|r| r.contains("activity=rpc.error")
            && r.contains("code=NOT_EDITABLE")
            && r.contains("group=db")
            && r.contains("method=planEdits")),
        "{records:#?}"
    );
    assert!(
        records
            .iter()
            .any(|r| r.contains("activity=db.applyChanges")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}

/// Review of the probe fixes: tiberius logs every SQL Server error token
/// with its message (`tiberius::tds::stream::token`, ERROR), and that text
/// can quote values (`THROW`'s text, a conversion error, a duplicate key).
/// The server's logger drops the target, so a `THROW` marker never reaches
/// the log. Live: runs only with `SEAQUEL_TEST_MSSQL` set (a ConnectConfig
/// JSON with `host`, `port`, `username` and `password`).
#[tokio::test]
async fn sql_server_error_text_is_not_logged() {
    let Ok(raw) = std::env::var("SEAQUEL_TEST_MSSQL") else {
        return;
    };
    let config: serde_json::Value = serde_json::from_str(&raw).expect("a ConnectConfig JSON");
    capture_logs();
    let env = Env::with_core(
        std::sync::Arc::new(seaquel_server::web_core()),
        4,
        std::sync::Arc::default(),
    );
    let form = json!({"type": "mssql", "name": "live", "host": config["host"],
        "port": config["port"], "databaseName": "master", "username": config["username"],
        "sslMode": "disable"});
    let (status, body) = env
        .db(
            "alice",
            "connect",
            json!({"target": {"type": "form", "form": form}, "secrets": {"db": config["password"]}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c = body["result"]["result"]["connectionId"]
        .as_str()
        .unwrap()
        .to_string();
    let canary = format!("throwcanary{}", std::process::id());
    for (method, sql) in [
        ("execute", format!("THROW 50000, '{canary}-throw', 1")),
        ("query", format!("SELECT CAST('{canary}-cast' AS int)")),
        ("execute", format!("PRINT '{canary}-print'")),
    ] {
        let (_, body) = env
            .db("alice", method, json!({"connectionId": c, "sql": sql}))
            .await;
        assert!(
            body.to_string().contains(&canary) || method == "execute",
            "{body}"
        );
    }
    let records = records();
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}

/// Phase 5d-1: library calls log their group, method and code, never a
/// name, host, string, query text, secret or the origin header.
#[tokio::test]
async fn library_calls_log_group_method_and_code_only() {
    capture_logs();
    let env = Env::new(4);
    let canary = format!("canaryLib{}", std::process::id());
    let origin = format!("{canary}-origin");
    let (status, body) = env
        .library("alice", Some(&origin), "projectEnsureDefault", json!(null))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let draft = json!({"projectId": "default-seaquel", "name": format!("{canary}-name"),
        "type": "postgres", "host": format!("{canary}-host"), "port": 5432,
        "databaseName": format!("{canary}-db"), "username": format!("{canary}-user"),
        "connectionString": format!("postgres://{canary}-user:{canary}-pw@{canary}-host/db")});
    let (status, body) = env
        .library(
            "alice",
            Some(&origin),
            "connectionCreate",
            json!({"connection": draft}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // A refusal: the same name again.
    let (status, body) = env
        .library(
            "alice",
            Some(&origin),
            "connectionCreate",
            json!({"connection": draft}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = env
        .library(
            "alice",
            Some(&origin),
            "savedQueryCreate",
            json!({"query": {"projectId": "default-seaquel", "name": format!("{canary}-q"),
                             "query": format!("SELECT '{canary}-text'")}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let records = records();
    assert!(
        records.iter().any(|r| r.contains("activity=rpc.error")
            && r.contains("code=NAME_TAKEN")
            && r.contains("group=library")
            && r.contains("method=connectionCreate")),
        "{records:#?}"
    );
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
}

/// Phase 5d-2: `settings`, `ui` and the new `library` calls log their
/// group, method and code, never a setting's value, a name, JSON, tab text,
/// an API key or the tab's origin (which is also its window id).
#[tokio::test]
async fn state_calls_log_group_method_and_code_only() {
    capture_logs();
    let env = Env::new(4);
    let canary = format!("canaryState{}", std::process::id());
    let origin = format!("{canary}-win");
    let call = |group: &'static str, method: &'static str, params: serde_json::Value| {
        let env = &env;
        let origin = origin.clone();
        async move {
            let body = json!({"method": group, "params": {"method": method, "params": params}});
            env.rpc_from("alice", &[origin.as_str()], &body).await
        }
    };
    let (status, body) = env
        .library("alice", Some(&origin), "projectEnsureDefault", json!(null))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (group, method, params, want) in [
        (
            "settings",
            "settingSet",
            json!({"key": "license_nudge", "value": format!("{{\"n\":\"{canary}\"}}")}),
            StatusCode::OK,
        ),
        (
            "settings",
            "userThemeCreate",
            json!({"theme": {"name": format!("{canary}-theme"), "c": canary}}),
            StatusCode::OK,
        ),
        (
            "settings",
            "aiProviderCreate",
            json!({"provider": {"name": format!("{canary}-p"), "type": "anthropic"},
                "apiKey": format!("{canary}-key")}),
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "library",
            "dashboardCreate",
            json!({"dashboard": {"projectId": "default-seaquel", "name": format!("{canary}-d"),
                "widgets": [{"sql": canary}], "viewport": {}}}),
            StatusCode::OK,
        ),
        (
            "ui",
            "windowStateSave",
            json!({"windowId": origin, "projectId": "default-seaquel", "rev": 1,
                "state": {"projectId": "default-seaquel", "queryTabs": [{"id": "t1",
                    "name": "Q", "query": format!("SELECT '{canary}'")}], "schemaTabs": [],
                    "explainTabs": [], "erdTabs": [], "tabOrder": ["t1"], "activeView": "query"}}),
            StatusCode::OK,
        ),
        // A refusal: another window's id.
        (
            "ui",
            "windowGet",
            json!({"windowId": format!("{canary}-other")}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, body) = call(group, method, params).await;
        assert_eq!(status, want, "{group}.{method}: {body}");
    }

    let records = records();
    for record in &records {
        assert!(!record.contains(&canary), "{record}");
    }
    assert!(
        records.iter().any(|r| r.contains("activity=rpc.error")
            && r.contains("code=INVALID_ARGUMENT")
            && r.contains("group=ui")
            && r.contains("method=windowGet")),
        "{records:#?}"
    );
    assert!(
        records.iter().any(|r| r.contains("activity=rpc.error")
            && r.contains("code=NOT_SUPPORTED")
            && r.contains("group=settings")
            && r.contains("method=aiProviderCreate")),
        "{records:#?}"
    );
}
