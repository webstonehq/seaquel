//! The workspace RPC: `Request` bodies parsed from bytes, dispatched onto a
//! workspace in a temp dir with a `MemoryStore`, and the wire shapes.

use std::sync::{Arc, Mutex, Once, PoisonError};

use seaquel_core::secrets::MemoryStore;
use seaquel_core::{Core, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_workspace, parse_request, Request, Response, RpcError, SecretRequest, SecretResponse,
    StorageRequest, StorageResponse, WriteOrigin,
};
use serde_json::{json, Value as Json};

// ── Helpers ──

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    _dir: tempfile::TempDir,
}

async fn env(with_secrets: bool) -> Env {
    // Postgres, so the library can save a connection of that type.
    let core = seaquel_core::with_plugins(|id| id == "postgres")
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = WorkspaceSpec::new(dir.path());
    if with_secrets {
        spec = spec.with_secrets(Arc::new(MemoryStore::new()));
    }
    let ws = core.open_workspace(spec).await.unwrap();
    Env {
        core,
        ws,
        _dir: dir,
    }
}

impl Env {
    /// Parse `body` as the interfaces do, dispatch it, and give back the
    /// response as the JSON text the interfaces send.
    async fn call_text(&self, body: &str) -> Result<String, RpcError> {
        let req = parse_request(body.as_bytes())?;
        let res = dispatch_workspace(&self.core, &self.ws, req, WriteOrigin::none()).await?;
        Ok(serde_json::to_string(&res).unwrap())
    }

    async fn call(&self, body: Json) -> Result<Json, RpcError> {
        let text = self.call_text(&body.to_string()).await?;
        Ok(serde_json::from_str(&text).unwrap())
    }

    async fn storage(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        let res = self
            .call(json!({"method": "storage", "params": {"method": method, "params": params}}))
            .await?;
        assert_eq!(res["method"], "storage");
        assert_eq!(res["result"]["method"], method);
        Ok(res["result"]["result"].clone())
    }

    /// A saved connection made through the library; its id.
    async fn saved_connection(&self) -> String {
        let project = self
            .call(json!({"method": "library", "params": {"method": "projectEnsureDefault"}}))
            .await
            .unwrap();
        let project = project["result"]["result"]["value"][0]["id"]
            .as_str()
            .unwrap_or("default-seaquel")
            .to_string();
        let res = self
            .call(
                json!({"method": "library", "params": {"method": "connectionCreate",
                "params": {"connection": connection_draft(&project)}}}),
            )
            .await
            .unwrap();
        res["result"]["result"]["value"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn secret(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        let res = self
            .call(json!({"method": "secret", "params": {"method": method, "params": params}}))
            .await?;
        assert_eq!(res["method"], "secret");
        assert_eq!(res["result"]["method"], method);
        Ok(res["result"]["result"].clone())
    }
}

fn connection_draft(project_id: &str) -> Json {
    json!({
        "projectId": project_id,
        "name": "Prod 🐘",
        "type": "postgres",
        "host": "db.example.com",
        "port": 5432,
        "databaseName": "app",
        "username": "me",
    })
}

// ── Storage ──

fn history_item(id: &str, favorite: bool) -> Json {
    json!({
        "id": id,
        "query": "SELECT 1",
        "timestamp": "2026-01-02T03:04:05.000Z",
        "executionTime": 1.5,
        "rowCount": 1,
        "connectionId": "c1",
        "favorite": favorite,
        "connectionLabelsSnapshot": [{"id": "l1", "name": "Prod", "color": "red"}],
        "connectionNameSnapshot": "Prod",
    })
}

#[tokio::test]
async fn history_appends_and_sets_favourites_through_dispatch() {
    let env = env(false).await;
    let c1 = env.saved_connection().await;
    let with_connection = |mut item: Json| {
        item["connectionId"] = json!(c1);
        item
    };

    let item = with_connection(history_item("hist-1", false));
    assert_eq!(
        env.storage("queryHistoryAppend", json!({"item": item}))
            .await
            .unwrap(),
        Json::Null
    );
    assert_eq!(
        env.storage(
            "queryHistorySetFavorite",
            json!({"id": "hist-1", "favorite": true})
        )
        .await
        .unwrap(),
        Json::Null
    );
    let loaded = env
        .storage("queryHistoryLoadByConnection", json!({"connectionId": c1}))
        .await
        .unwrap();
    assert_eq!(
        loaded,
        json!([with_connection(history_item("hist-1", true))])
    );

    // An unsaved connection fails the foreign key, with storage's code.
    let mut orphan = history_item("hist-2", false);
    orphan["connectionId"] = json!("unsaved");
    let err = env
        .storage("queryHistoryAppend", json!({"item": orphan}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_ERROR");
}

#[test]
fn query_history_replace_all_is_an_unknown_method() {
    let err = parse_request(
        br#"{"method":"storage","params":{"method":"queryHistoryReplaceAll","params":{"connectionId":"c1","items":[]}}}"#,
    )
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(
        err.message.contains("queryHistoryReplaceAll"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn a_method_without_params_needs_no_params_key() {
    let env = env(false).await;
    let text = env
        .call_text(r#"{"method":"storage","params":{"method":"vaultStateLoad"}}"#)
        .await
        .unwrap();
    assert_eq!(
        text,
        r#"{"method":"storage","result":{"method":"vaultStateLoad","result":null}}"#
    );
}

#[tokio::test]
async fn storage_failures_keep_their_code() {
    let env = env(false).await;
    // No connection c1: the foreign key refuses the row.
    let err = env
        .storage(
            "queryHistoryAppend",
            json!({"item": history_item("h1", false)}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_ERROR", "{err}");
}

#[tokio::test]
async fn a_closed_workspace_fails_with_a_storage_error() {
    let env = env(false).await;
    env.ws.close().await;
    let err = env.storage("vaultStateLoad", Json::Null).await.unwrap_err();
    assert_eq!(err.code, "STORAGE_ERROR", "{err}");
}

/// Stored JSON is the text the client sent, byte for byte: key order,
/// number formatting and all, through parsing, dispatch, storage and load.
#[tokio::test]
async fn json_columns_keep_their_bytes() {
    let env = env(false).await;
    let odd = r#"{"zeta":1e+21,"alpha":{"b":1.50,"a":-0.0,"c":[3,1,2]},"mid":"éé","big":12345678901234567890}"#;

    // A top-level JSON value.
    let body = format!(
        r#"{{"method":"storage","params":{{"method":"licenseSave","params":{{"data":{odd}}}}}}}"#
    );
    env.call_text(&body).await.unwrap();
    let text = env
        .call_text(r#"{"method":"storage","params":{"method":"licenseLoad"}}"#)
        .await
        .unwrap();
    assert_eq!(
        text,
        format!(r#"{{"method":"storage","result":{{"method":"licenseLoad","result":{odd}}}}}"#)
    );

    // A JSON row (a shared repo) keeps its bytes too: `shared.reposList`
    // in tests/shared.rs (phase 5e retired `sharedReposSaveAll`).

    // A JSON body in the settings group (phase 5d-2): Core rewrites only
    // the top level of an onboarding patch, and keeps each value's bytes.
    let body = r#"{"method":"settings","params":{"method":"onboardingPatch","params":{"patch":{"dismissedHints":[ "b" ,"a"]}}}}"#;
    let text = env.call_text(body).await.unwrap();
    assert!(text.contains(r#""dismissedHints":[ "b" ,"a"]"#), "{text}");
}

// ── Secrets ──

#[tokio::test]
async fn secrets_round_trip_through_a_memory_store() {
    let env = env(true).await;
    assert_eq!(
        env.secret("get", json!({"key": "db:c1"})).await.unwrap(),
        Json::Null
    );
    env.secret("set", json!({"key": "db:c1", "value": "hunter2 ü"}))
        .await
        .unwrap();
    assert_eq!(
        env.secret("get", json!({"key": "db:c1"})).await.unwrap(),
        json!("hunter2 ü")
    );
    env.secret("delete", json!({"key": "db:c1"})).await.unwrap();
    assert_eq!(
        env.secret("get", json!({"key": "db:c1"})).await.unwrap(),
        Json::Null
    );
    // Deleting a missing entry is fine.
    env.secret("delete", json!({"key": "license-key"}))
        .await
        .unwrap();
}

#[tokio::test]
async fn secrets_without_a_store_are_not_supported() {
    let env = env(false).await;
    for (method, params) in [
        ("get", json!({"key": "db:c1"})),
        ("set", json!({"key": "db:c1", "value": "v"})),
        ("delete", json!({"key": "db:c1"})),
    ] {
        let err = env.secret(method, params).await.unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{method}: {err}");
    }
}

#[tokio::test]
async fn a_bad_secret_key_is_an_invalid_argument() {
    for with_store in [true, false] {
        let env = env(with_store).await;
        for key in ["", "postgres:c1", "db:", "ai-api-key", "db:a\nb"] {
            let err = env
                .secret("set", json!({"key": key, "value": "s3cret-value"}))
                .await
                .unwrap_err();
            assert_eq!(err.code, "INVALID_ARGUMENT", "{key:?}: {err}");
            assert!(!err.message.contains("s3cret-value"), "{err}");
        }
    }
}

// ── Parsing ──

#[test]
fn params_before_method_fails_clearly() {
    let bodies = [
        // The request itself.
        r#"{"params":{"method":"vaultStateLoad"},"method":"storage"}"#,
        // The group's request, for a method with a JSON column...
        r#"{"method":"storage","params":{"params":{"data":{"a":1}},"method":"licenseSave"}}"#,
        r#"{"method":"settings","params":{"params":{"patch":{"a":1}},"method":"onboardingPatch"}}"#,
        r#"{"method":"ui","params":{"params":{"windowId":"w","projectId":"p","rev":1,"state":{}},"method":"windowStateSave"}}"#,
        // ...and for one without, which serde alone would accept.
        r#"{"method":"storage","params":{"params":{"connectionId":"c1"},"method":"queryHistoryRemoveByConnection"}}"#,
        r#"{"method":"secret","params":{"params":{"key":"db:c1"},"method":"get"}}"#,
    ];
    for body in bodies {
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{body}");
        assert!(
            err.message
                .contains(r#""method" must come before "params""#),
            "{body}: {}",
            err.message
        );
    }
}

#[test]
fn invalid_bodies_are_invalid_arguments() {
    for body in [
        "",
        "not json",
        "[]",
        r#"{"method":"nope"}"#,
        r#"{"method":"storage","params":{"method":"nope"}}"#,
        r#"{"method":"storage","params":{"method":"queryHistoryRemoveByConnection","params":{}}}"#,
        r#"{"method":"storage","params":{"method":"queryHistoryRemoveByConnection","params":{"connection_id":"c1"}}}"#,
    ] {
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{body}");
        assert!(
            err.message.starts_with("invalid request: "),
            "{}",
            err.message
        );
    }
}

// ── Wire snapshots, one per group ──

#[test]
fn storage_request_snapshot() {
    let req = Request::Storage(StorageRequest::QueryHistoryRemoveByConnection {
        connection_id: "c1".into(),
    });
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(
        text,
        r#"{"method":"storage","params":{"method":"queryHistoryRemoveByConnection","params":{"connectionId":"c1"}}}"#
    );
    assert_eq!(
        (req.group(), req.method()),
        ("storage", "queryHistoryRemoveByConnection")
    );
    // And back.
    let back = parse_request(text.as_bytes()).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), text);

    let fav = Request::Storage(StorageRequest::QueryHistorySetFavorite {
        id: "hist-1".into(),
        favorite: true,
    });
    assert_eq!(
        serde_json::to_string(&fav).unwrap(),
        r#"{"method":"storage","params":{"method":"queryHistorySetFavorite","params":{"id":"hist-1","favorite":true}}}"#
    );

    let append_body = format!(
        r#"{{"method":"storage","params":{{"method":"queryHistoryAppend","params":{{"item":{}}}}}}}"#,
        history_item("hist-1", false)
    );
    let append = parse_request(append_body.as_bytes()).unwrap();
    assert_eq!(append.method(), "queryHistoryAppend");
    let back: Json = serde_json::from_str(&serde_json::to_string(&append).unwrap()).unwrap();
    assert_eq!(back, serde_json::from_str::<Json>(&append_body).unwrap());

    let unit = Request::Storage(StorageRequest::VaultStateReset);
    assert_eq!(
        serde_json::to_string(&unit).unwrap(),
        r#"{"method":"storage","params":{"method":"vaultStateReset"}}"#
    );

    let res = Response::Storage(StorageResponse::VaultStateLoad(None));
    assert_eq!(
        serde_json::to_string(&res).unwrap(),
        r#"{"method":"storage","result":{"method":"vaultStateLoad","result":null}}"#
    );
    let res = Response::Storage(StorageResponse::VaultStateReset(()));
    assert_eq!(
        serde_json::to_string(&res).unwrap(),
        r#"{"method":"storage","result":{"method":"vaultStateReset","result":null}}"#
    );
}

#[test]
fn secret_request_snapshot() {
    let req = Request::Secret(SecretRequest::Set {
        key: "db:c1".into(),
        value: "pw".into(),
    });
    let text = serde_json::to_string(&req).unwrap();
    assert_eq!(
        text,
        r#"{"method":"secret","params":{"method":"set","params":{"key":"db:c1","value":"pw"}}}"#
    );
    assert_eq!((req.group(), req.method()), ("secret", "set"));
    assert_eq!(
        serde_json::to_string(&parse_request(text.as_bytes()).unwrap()).unwrap(),
        text
    );

    let res = Response::Secret(SecretResponse::Get(Some("pw".into())));
    assert_eq!(
        serde_json::to_string(&res).unwrap(),
        r#"{"method":"secret","result":{"method":"get","result":"pw"}}"#
    );
    let res = Response::Secret(SecretResponse::Delete(()));
    assert_eq!(
        serde_json::to_string(&res).unwrap(),
        r#"{"method":"secret","result":{"method":"delete","result":null}}"#
    );
}

#[test]
fn rpc_error_snapshot() {
    let err = RpcError::new("LEGACY_STORAGE", "old data");
    assert_eq!(
        serde_json::to_string(&err).unwrap(),
        r#"{"code":"LEGACY_STORAGE","message":"old data"}"#
    );
}

// ── Secrets never reach Debug or the log ──

const SECRET: &str = "correct-horse-battery-staple";

#[test]
fn debug_redacts_secret_values() {
    let req = Request::Secret(SecretRequest::Set {
        key: "db:c1".into(),
        value: SECRET.into(),
    });
    let text = format!("{req:?}");
    assert!(!text.contains(SECRET), "{text}");
    assert!(
        text.contains("db:c1") && text.contains("<redacted>"),
        "{text}"
    );

    let res = Response::Secret(SecretResponse::Get(Some(SECRET.into())));
    let text = format!("{res:?}");
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains("<redacted>"), "{text}");
    assert!(!format!("{:?}", SecretResponse::Get(None)).contains("redacted"));
}

/// Every record logged in this test binary, with its key-values.
static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        struct Kvs(String);
        impl<'kvs> log::kv::VisitSource<'kvs> for Kvs {
            fn visit_pair(
                &mut self,
                key: log::kv::Key<'kvs>,
                value: log::kv::Value<'kvs>,
            ) -> Result<(), log::kv::Error> {
                self.0.push_str(&format!(" {key}={value}"));
                Ok(())
            }
        }
        let mut kvs = Kvs(String::new());
        record.key_values().visit(&mut kvs).unwrap();
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}{}", record.target(), record.args(), kvs.0));
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

#[tokio::test]
async fn dispatch_logs_the_method_and_never_the_params() {
    capture_logs();
    let env = env(true).await;
    env.secret("set", json!({"key": "db:c-log", "value": SECRET}))
        .await
        .unwrap();
    assert_eq!(
        env.secret("get", json!({"key": "db:c-log"})).await.unwrap(),
        json!(SECRET)
    );
    // A failing call logs its code, still without the value.
    env.secret("set", json!({"key": "bogus", "value": SECRET}))
        .await
        .unwrap_err();

    let records = RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for record in &records {
        assert!(!record.contains(SECRET), "{record}");
    }
    assert!(
        records
            .iter()
            .any(|r| r.contains("group=secret") && r.contains("method=set")),
        "{records:#?}"
    );
    assert!(
        records
            .iter()
            .any(|r| r.contains("method=set") && r.contains("code=INVALID_ARGUMENT")),
        "{records:#?}"
    );
}

/// A storage-group write emits one `storage` event
/// after it committed, naming the method's key; a read or a failed write
/// emits none.
#[tokio::test]
async fn storage_writes_emit_and_reads_dont() {
    use futures::StreamExt;
    use seaquel_core::{StoredKind, WorkspaceEvent};
    let e = env(false).await;
    let mut events = e.ws.events();
    e.storage(
        "userCredentialsSave",
        json!({"credential": {"scope": "db", "key": "k1", "nonce": "n",
            "ciphertext": "canary-value", "updatedAt": "t"}}),
    )
    .await
    .unwrap();
    e.storage("userCredentialsLoad", json!({"scope": "db", "key": "k1"}))
        .await
        .unwrap();
    e.storage("vaultStateLoad", Json::Null).await.ok();
    // A connection whose project doesn't exist fails on the foreign key.
    let failed = e
        .storage(
            "queryHistoryAppend",
            json!({"item": {"id": "h1", "query": "SELECT 1", "timestamp": "t", "executionTime": 1,
                            "rowCount": 1, "connectionId": "missing", "favorite": false}}),
        )
        .await;
    assert!(failed.is_err());
    let mut got = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(std::time::Duration::from_millis(30), events.next()).await
    {
        if let WorkspaceEvent::StorageChanged(c) = ev {
            got.push(c);
        }
    }
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].kind, StoredKind::Storage);
    assert_eq!(got[0].ids, Some(vec!["k1".to_string()]));
    assert_eq!(got[0].origin, None);
    assert!(!format!("{got:?}").contains("canary"));
}
