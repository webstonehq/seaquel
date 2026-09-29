//! The `library` group (phase 5d-1): wire shapes, `Clearable` patches over
//! the wire, `Debug` redaction, the retired storage methods, the origin a
//! call carries, and `CoreEvent::StorageChanged`.

use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::secrets::MemoryStore;
use seaquel_core::{Core, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_workspace, parse_request, workspace_events, CoreEvent, LibraryRequest, Request,
    RpcError, WriteOrigin,
};
use serde_json::{json, Value as Json};

// ── Helpers ──

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    _dir: tempfile::TempDir,
}

async fn env(with_secrets: bool) -> Env {
    // Postgres only, as a test engine: a library connection's type must be
    // an engine this Core has.
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
    async fn call_as(&self, origin: Option<&str>, body: &Json) -> Result<Json, RpcError> {
        let req = parse_request(body.to_string().as_bytes())?;
        let res = dispatch_workspace(&self.core, &self.ws, req, WriteOrigin::new(origin)).await?;
        Ok(serde_json::to_value(res).unwrap())
    }

    /// One library call from window `origin`; its `{value, seq}`.
    async fn lib_as(
        &self,
        origin: Option<&str>,
        method: &str,
        params: Json,
    ) -> Result<Json, RpcError> {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let res = self
            .call_as(origin, &json!({"method": "library", "params": inner}))
            .await?;
        assert_eq!(res["method"], "library", "{res}");
        assert_eq!(res["result"]["method"], method, "{res}");
        Ok(res["result"]["result"].clone())
    }

    async fn lib(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.lib_as(Some("tab-1"), method, params).await
    }

    /// The default project's id, made if needed.
    async fn project(&self) -> String {
        self.lib("projectEnsureDefault", Json::Null).await.unwrap();
        let list = self.lib("projectsList", Json::Null).await.unwrap();
        list["value"][0]["id"].as_str().unwrap().to_string()
    }
}

/// A connection draft full of canaries: none may reach `Debug`, a log or
/// an event.
fn draft(project_id: &str) -> Json {
    json!({
        "projectId": project_id,
        "name": "canary-name",
        "type": "postgres",
        "host": "canary-host.example",
        "port": 5432,
        "databaseName": "canary-db",
        "username": "canary-user",
        "sslMode": "require",
        "connectionString": "postgres://canary-user@canary-host.example/canary-db",
    })
}

/// Every event that arrives within a short wait.
async fn drain(events: &mut (impl futures::Stream<Item = CoreEvent> + Unpin)) -> Vec<Json> {
    let mut got = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await
    {
        got.push(serde_json::to_value(event).unwrap());
    }
    got
}

// ── Wire ──

#[tokio::test]
async fn library_calls_round_trip_with_seq() {
    let env = env(false).await;
    let project = env.project().await;

    let created = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap();
    let row = &created["value"];
    let id = row["id"].as_str().unwrap();
    assert!(id.starts_with("conn-"), "{id}");
    assert_eq!(row["name"], "canary-name");
    let seq = &created["seq"];
    assert!(seq["epoch"].is_string(), "{created}");
    assert!(seq["n"].as_u64().unwrap() > 0, "{created}");

    let list = env.lib("connectionsList", Json::Null).await.unwrap();
    assert_eq!(list["value"].as_array().unwrap().len(), 1);
    assert_eq!(list["seq"]["epoch"], seq["epoch"]);
    assert!(list["seq"]["n"].as_u64() >= seq["n"].as_u64());

    let removed = env
        .lib("connectionRemove", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(removed["value"], Json::Null);
    assert!(removed["seq"]["n"].as_u64() > seq["n"].as_u64());
    assert_eq!(
        env.lib("connectionsList", Json::Null).await.unwrap()["value"],
        json!([])
    );
}

#[tokio::test]
async fn every_library_method_answers_with_its_own_name() {
    let env = env(false).await;
    let project = env.project().await;
    let p = env
        .lib("projectCreate", json!({"project": {"name": "Second"}}))
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.lib(
        "projectUpdate",
        json!({"id": p, "patch": {"description": "d"}}),
    )
    .await
    .unwrap();
    let label = env
        .lib(
            "labelCreate",
            json!({"projectId": project, "label": {"name": "Blue", "color": "#0000ff"}}),
        )
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.lib(
        "labelUpdate",
        json!({"projectId": project, "labelId": label, "patch": {"color": "#00ff00"}}),
    )
    .await
    .unwrap();
    let mut conn = draft(&project);
    conn["labelIds"] = json!([label]);
    let c = env
        .lib("connectionCreate", json!({"connection": conn}))
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let removed = env
        .lib(
            "labelRemove",
            json!({"projectId": project, "labelId": label}),
        )
        .await
        .unwrap();
    assert_eq!(removed["value"], json!({"connectionIds": [c]}));

    let q = env
        .lib(
            "savedQueryCreate",
            json!({"query": {"projectId": project, "name": "Q", "query": "SELECT 1"}}),
        )
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let updated = env
        .lib(
            "savedQueryUpdate",
            json!({"id": q, "patch": {"query": "SELECT 2"}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["value"]["query"]["query"], "SELECT 2");
    assert_eq!(updated["value"]["version"]["snapshot"], "SELECT 1");
    assert_eq!(updated["value"]["prunedVersionIds"], json!([]));
    let lists = env
        .lib("savedQueriesList", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(lists["value"].as_array().unwrap().len(), 1);
    let versions = env
        .lib("queryVersionsList", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(versions["value"].as_array().unwrap().len(), 1);
    env.lib("savedQueryRemove", json!({"id": q})).await.unwrap();

    let removed = env.lib("projectRemove", json!({"id": p})).await.unwrap();
    assert_eq!(removed["value"], json!({"connectionIds": []}));
}

#[test]
fn library_request_snapshot() {
    let text = r#"{"method":"library","params":{"method":"connectionUpdate","params":{"id":"conn-1","patch":{"sslMode":null,"port":5433}}}}"#;
    let req = parse_request(text.as_bytes()).unwrap();
    assert_eq!((req.group(), req.method()), ("library", "connectionUpdate"));
    let Request::Library(LibraryRequest::ConnectionUpdate { id, patch, secrets }) = &req else {
        panic!("{req:?}");
    };
    assert_eq!(id, "conn-1");
    // `null` clears, absent keeps.
    assert_eq!(patch.ssl_mode, Some(None));
    assert_eq!(patch.connection_string, None);
    assert_eq!(patch.port, Some(5433.0));
    assert!(secrets.is_empty());
    // And back: the clear stays a `null`, the absent fields (and the empty
    // secrets) stay absent.
    assert_eq!(
        serde_json::to_string(&req).unwrap(),
        r#"{"method":"library","params":{"method":"connectionUpdate","params":{"id":"conn-1","patch":{"port":5433.0,"sslMode":null}}}}"#
    );

    let unit = r#"{"method":"library","params":{"method":"projectEnsureDefault"}}"#;
    let req = parse_request(unit.as_bytes()).unwrap();
    assert_eq!(req.method(), "projectEnsureDefault");
    assert_eq!(serde_json::to_string(&req).unwrap(), unit);

    // A method without params refuses `{}`, and a bad field is refused.
    for bad in [
        r#"{"method":"library","params":{"method":"projectEnsureDefault","params":{}}}"#,
        r#"{"method":"library","params":{"method":"connectionRemove","params":{}}}"#,
        r#"{"method":"library","params":{"method":"connectionUpdate","params":{"id":"c","patch":{"bogus":1}}}}"#,
        r#"{"method":"library","params":{"params":{"id":"c"},"method":"connectionRemove"}}"#,
    ] {
        let err = parse_request(bad.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{bad}");
    }
}

/// Phase 5d-1 probe fix: an unknown field in a request's params is refused,
/// in every group whose callers send exact shapes, so a typo such as
/// `secret` for `secrets` fails instead of silently doing less.
#[test]
fn unknown_request_fields_are_refused() {
    for bad in [
        // library (c3b: both were accepted, the extra field ignored)
        r#"{"method":"library","params":{"method":"connectionRemove","params":{"id":"c","extra":1}}}"#,
        r#"{"method":"library","params":{"method":"connectionUpdate","params":{"id":"c","patch":{"name":"n"},"secret":{"db":"x"}}}}"#,
        r#"{"method":"library","params":{"method":"savedQueriesList","params":{"projectId":"p","x":null}}}"#,
        r#"{"method":"library","params":{"method":"connectionsList","extra":1}}"#,
        // the request itself
        r#"{"method":"library","params":{"method":"connectionsList"},"extra":1}"#,
        // storage, secret, license, git, ssh
        r#"{"method":"storage","params":{"method":"appStateGet","params":{"key":"k","extra":1}}}"#,
        r#"{"method":"secret","params":{"method":"get","params":{"key":"db:c","extra":1}}}"#,
        r#"{"method":"license","params":{"method":"validate","params":{"key":"k","instanceId":"i","extra":1}}}"#,
        r#"{"method":"git","params":{"method":"status","params":{"path":"/p","extra":1}}}"#,
        r#"{"method":"ssh","params":{"method":"close","params":{"tunnelId":"t","extra":1}}}"#,
    ] {
        let err = parse_request(bad.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{bad}");
        assert!(
            err.message.contains("unknown field") || err.message.contains("expected"),
            "{bad}: {}",
            err.message
        );
    }
    // The same requests without the extra field parse.
    for good in [
        r#"{"method":"library","params":{"method":"connectionRemove","params":{"id":"c"}}}"#,
        r#"{"method":"library","params":{"method":"connectionUpdate","params":{"id":"c","patch":{"name":"n"},"secrets":{"db":"x"}}}}"#,
        r#"{"method":"library","params":{"method":"connectionsList"}}"#,
        r#"{"method":"storage","params":{"method":"appStateGet","params":{"key":"k"}}}"#,
        r#"{"method":"secret","params":{"method":"get","params":{"key":"db:c"}}}"#,
        r#"{"method":"license","params":{"method":"validate","params":{"key":"k","instanceId":"i"}}}"#,
        r#"{"method":"git","params":{"method":"status","params":{"path":"/p"}}}"#,
        r#"{"method":"ssh","params":{"method":"close","params":{"tunnelId":"t"}}}"#,
    ] {
        parse_request(good.as_bytes()).unwrap_or_else(|e| panic!("{good}: {e}"));
    }
}

#[tokio::test]
async fn a_clearable_field_is_cleared_by_null_and_kept_when_absent() {
    let env = env(false).await;
    let project = env.project().await;
    let id = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Absent: kept.
    let kept = env
        .lib(
            "connectionUpdate",
            json!({"id": id, "patch": {"port": 6543}}),
        )
        .await
        .unwrap();
    assert_eq!(kept["value"]["sslMode"], "require");
    assert_eq!(kept["value"]["port"], 6543);
    // `null`: cleared.
    let cleared = env
        .lib(
            "connectionUpdate",
            json!({"id": id, "patch": {"sslMode": null}}),
        )
        .await
        .unwrap();
    assert!(cleared["value"]["sslMode"].is_null(), "{cleared}");
    assert_eq!(cleared["value"]["port"], 6543);
}

#[test]
fn debug_shows_no_names_hosts_strings_text_or_secrets() {
    let bodies = [
        json!({"method": "library", "params": {"method": "connectionCreate", "params": {
            "connection": draft("p"),
            "secrets": {"db": "canary-password", "ssh": "canary-ssh"}}}}),
        json!({"method": "library", "params": {"method": "connectionUpdate", "params": {
            "id": "c", "patch": {"name": "canary-name", "host": "canary-host.example"},
            "secrets": {"db": "canary-password"}}}}),
        json!({"method": "library", "params": {"method": "savedQueryCreate", "params": {
            "query": {"projectId": "p", "name": "canary-name", "query": "SELECT 'canary-text'"}}}}),
        json!({"method": "library", "params": {"method": "savedQueryUpdate", "params": {
            "id": "q", "patch": {"query": "SELECT 'canary-text'", "description": "canary-d"}}}}),
        json!({"method": "library", "params": {"method": "projectCreate", "params": {
            "project": {"name": "canary-name", "description": "canary-d"}}}}),
        json!({"method": "library", "params": {"method": "labelCreate", "params": {
            "projectId": "p", "label": {"name": "canary-name", "color": "#ffffff"}}}}),
    ];
    for body in bodies {
        let req = parse_request(body.to_string().as_bytes()).unwrap();
        let text = format!("{req:?}");
        assert!(!text.contains("canary"), "{text}");
    }
}

#[test]
fn a_retired_storage_method_is_unknown() {
    for (method, params) in [
        ("connectionsLoadAll", Json::Null),
        ("connectionsSave", json!({"connection": {}})),
        ("connectionsRemove", json!({"connectionId": "c"})),
        ("projectsLoadAll", Json::Null),
        ("projectsSave", json!({"project": {}})),
        ("projectsSaveAll", json!({"projects": []})),
        ("projectsRemove", json!({"projectId": "p"})),
        ("savedQueriesLoadByProject", json!({"projectId": "p"})),
        (
            "savedQueriesSaveAll",
            json!({"projectId": "p", "queries": []}),
        ),
        ("savedQueriesRemoveByProject", json!({"projectId": "p"})),
        ("queryVersionsLoadByQuery", json!({"queryId": "q"})),
        ("queryVersionsLoadByProject", json!({"projectId": "p"})),
        ("queryVersionsInsert", json!({"version": {}})),
        (
            "queryVersionsPrune",
            json!({"savedQueryId": "q", "deleteIds": []}),
        ),
    ] {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let body = json!({"method": "storage", "params": inner}).to_string();
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{method}");
        assert!(
            err.message.contains("unknown variant"),
            "{method}: {}",
            err.message
        );
    }
}

// ── Events and origins ──

#[tokio::test]
async fn storage_changed_serialises_without_values() {
    let env = env(false).await;
    let project = env.project().await;
    let mut events = workspace_events(&env.ws);
    let created = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap();
    let id = created["value"]["id"].as_str().unwrap();

    let got = drain(&mut events).await;
    assert_eq!(got.len(), 1, "{got:#?}");
    let event = &got[0];
    assert_eq!(
        event,
        &json!({
            "type": "storageChanged",
            "kind": "connection",
            "scope": project,
            "ids": [id],
            "origin": "tab-1",
            "seq": created["seq"],
        })
    );
    assert!(!event.to_string().contains("canary"), "{event}");
    // Not a stream event.
    let parsed: CoreEvent = {
        let mut events = workspace_events(&env.ws);
        env.lib("connectionRemove", json!({"id": id}))
            .await
            .unwrap();
        events.next().await.unwrap()
    };
    assert_eq!(parsed.stream_id(), None);
    assert!(!parsed.is_terminal());
}

#[tokio::test]
async fn the_origin_is_carried_and_a_bad_one_is_dropped() {
    let env = env(false).await;
    let project = env.project().await;
    let mut events = workspace_events(&env.ws);
    for origin in [
        Some("main"),
        None,
        Some("bad origin"),
        Some(&"x".repeat(65) as &str),
        Some("a\nb"),
    ] {
        env.lib_as(
            origin,
            "savedQueryCreate",
            json!({"query": {"projectId": project, "name": format!("{origin:?}"), "query": "SELECT 1"}}),
        )
        .await
        .unwrap();
    }
    let origins: Vec<Json> = drain(&mut events)
        .await
        .into_iter()
        .map(|e| e["origin"].clone())
        .collect();
    assert_eq!(
        origins,
        [
            json!("main"),
            Json::Null,
            Json::Null,
            Json::Null,
            Json::Null
        ]
    );
}

#[tokio::test]
async fn storage_group_writes_carry_the_origin_too() {
    let env = env(false).await;
    let mut events = workspace_events(&env.ws);
    env.call_as(
        Some("tab-2"),
        &json!({"method": "storage", "params": {"method": "appStateSet",
            "params": {"key": "k1", "value": "canary-value"}}}),
    )
    .await
    .unwrap();
    let got = drain(&mut events).await;
    assert_eq!(got.len(), 1, "{got:#?}");
    assert_eq!(got[0]["kind"], "storage");
    assert_eq!(got[0]["ids"], json!(["k1"]));
    assert_eq!(got[0]["origin"], "tab-2");
    assert!(!got[0].to_string().contains("canary"));
}

#[tokio::test]
async fn a_refused_call_emits_nothing_and_keeps_its_code() {
    let env = env(false).await;
    let project = env.project().await;
    env.lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap();
    let mut events = workspace_events(&env.ws);
    let taken = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap_err();
    assert_eq!(taken.code, "NAME_TAKEN");
    let missing = env
        .lib("savedQueryRemove", json!({"id": "saved-nope"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "SAVED_QUERY_NOT_FOUND");
    let last = env
        .lib("projectRemove", json!({"id": project}))
        .await
        .unwrap_err();
    assert_eq!(last.code, "LAST_PROJECT");
    assert_eq!(drain(&mut events).await, Vec::<Json>::new());
}

#[tokio::test]
async fn name_taken_names_the_row_on_the_wire() {
    let env = env(false).await;
    let project = env.project().await;
    let first = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap();
    let id = first["value"]["id"].as_str().expect("the created row's id");
    let taken = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap_err();
    assert_eq!(taken.code, "NAME_TAKEN");
    assert_eq!(taken.taken_by.as_deref(), Some(id));
    let wire = serde_json::to_value(&taken).unwrap();
    assert_eq!(wire["takenBy"], id);
    // Any other error has no `takenBy` key.
    let missing = env
        .lib("savedQueryRemove", json!({"id": "saved-nope"}))
        .await
        .unwrap_err();
    assert!(serde_json::to_value(&missing)
        .unwrap()
        .get("takenBy")
        .is_none());
}

#[tokio::test]
async fn secrets_on_a_workspace_without_a_store_are_not_supported() {
    let env = env(false).await;
    let project = env.project().await;
    let mut d = draft(&project);
    d["savePassword"] = json!(true);
    let err = env
        .lib(
            "connectionCreate",
            json!({"connection": d, "secrets": {"db": "canary-password"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
    assert!(!err.message.contains("canary"));

    // With a store, the secret is written with the row.
    let env = env_with_store().await;
    let project = env.project().await;
    let mut d = draft(&project);
    d["savePassword"] = json!(true);
    let created = env
        .lib(
            "connectionCreate",
            json!({"connection": d, "secrets": {"db": "canary-password"}}),
        )
        .await
        .unwrap();
    let id = created["value"]["id"].as_str().unwrap();
    let got = env
        .call_as(
            None,
            &json!({"method": "secret", "params": {"method": "get", "params": {"key": format!("db:{id}")}}}),
        )
        .await
        .unwrap();
    assert_eq!(got["result"]["result"], "canary-password");
}

async fn env_with_store() -> Env {
    env(true).await
}

// ── Logs ──

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

#[tokio::test]
async fn library_calls_log_group_method_and_code_only() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
    let env = env(true).await;
    let project = env.project().await;
    let mut d = draft(&project);
    d["savePassword"] = json!(true);
    env.lib_as(
        Some("canary-origin"),
        "connectionCreate",
        json!({"connection": d, "secrets": {"db": "canary-password"}}),
    )
    .await
    .unwrap();
    // A refusal logs its code.
    env.lib_as(
        Some("canary-origin"),
        "connectionCreate",
        json!({"connection": draft(&project)}),
    )
    .await
    .unwrap_err();

    let records = RECORDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for record in &records {
        assert!(!record.contains("canary"), "{record}");
    }
    assert!(
        records
            .iter()
            .any(|r| r.contains("group=library") && r.contains("method=connectionCreate")),
        "{records:#?}"
    );
    assert!(
        records
            .iter()
            .any(|r| r.contains("method=connectionCreate") && r.contains("code=NAME_TAKEN")),
        "{records:#?}"
    );
}

/// Phase 5d review, M2: the query history's storage writes are `history`
/// events; `queryHistorySetFavorite` has the row's id and no scope.
#[tokio::test]
async fn query_history_storage_writes_are_history_events() {
    let env = env(false).await;
    let project = env.project().await;
    let c = env
        .lib("connectionCreate", json!({"connection": draft(&project)}))
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut events = workspace_events(&env.ws);
    let storage = |method: &str, params: Json| json!({"method": "storage", "params": {"method": method, "params": params}});
    for body in [
        storage(
            "queryHistoryAppend",
            json!({"item": {"id": "hist-1", "query": "SELECT 'canary-text'", "timestamp": "t",
                "executionTime": 1, "rowCount": 1, "connectionId": c, "favorite": false,
                "connectionNameSnapshot": "n"}}),
        ),
        storage(
            "queryHistorySetFavorite",
            json!({"id": "hist-1", "favorite": true}),
        ),
        storage("queryHistoryRemoveByConnection", json!({"connectionId": c})),
    ] {
        env.call_as(Some("tab-3"), &body).await.unwrap();
    }
    let got: Vec<Json> = drain(&mut events)
        .await
        .into_iter()
        .map(|e| json!([e["kind"], e["scope"], e["ids"], e["origin"]]))
        .collect();
    assert_eq!(
        got,
        [
            json!(["history", c, ["hist-1"], "tab-3"]),
            json!(["history", null, ["hist-1"], "tab-3"]),
            json!(["history", c, null, "tab-3"]),
        ]
    );
}

/// Phase 5d review, M4: an event's `Debug` hides its origin.
#[test]
fn a_storage_changed_debug_hides_the_origin() {
    let event = CoreEvent::StorageChanged {
        kind: seaquel_core::StoredKind::Connection,
        scope: Some("p".into()),
        ids: Some(vec!["c".into()]),
        origin: Some("canary-origin".into()),
        seq: seaquel_core::ChangeSeq {
            epoch: "e".into(),
            n: 1,
        },
    };
    let text = format!("{event:?} {event:#?}");
    assert!(!text.contains("canary"), "{text}");
    assert!(text.contains("<set>"), "{text}");
}
