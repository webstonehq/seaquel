//! Phase 5d-2 over the wire: the `settings` and `ui` groups, the `library`
//! additions (dashboards, workflows, chats, a project's connection order),
//! the 35 retired storage methods, `Debug` redaction and the events.

use std::sync::{Arc, Mutex, Once, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::secrets::{MemoryStore, SecretStore};
use seaquel_core::{Core, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_workspace, parse_request, workspace_events, CoreEvent, RpcError, WriteOrigin,
};
use serde_json::{json, Value as Json};

// ── Helpers ──

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    store: Option<Arc<MemoryStore>>,
    _dir: tempfile::TempDir,
}

async fn env(with_secrets: bool) -> Env {
    let core = seaquel_core::with_plugins(|id| id == "postgres")
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = WorkspaceSpec::new(dir.path());
    let store = with_secrets.then(|| Arc::new(MemoryStore::new()));
    if let Some(store) = &store {
        spec = spec.with_secrets(store.clone());
    }
    let ws = core.open_workspace(spec).await.unwrap();
    Env {
        core,
        ws,
        store,
        _dir: dir,
    }
}

const WINDOW: &str = "win-1";

impl Env {
    async fn call_as(&self, origin: Option<&str>, body: &Json) -> Result<Json, RpcError> {
        let req = parse_request(body.to_string().as_bytes())?;
        let res = dispatch_workspace(&self.core, &self.ws, req, WriteOrigin::new(origin)).await?;
        Ok(serde_json::to_value(res).unwrap())
    }

    /// One call of `group` from window `origin`; its `{value, seq}`.
    async fn group_as(
        &self,
        origin: Option<&str>,
        group: &str,
        method: &str,
        params: Json,
    ) -> Result<Json, RpcError> {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let res = self
            .call_as(origin, &json!({"method": group, "params": inner}))
            .await?;
        assert_eq!(res["method"], group, "{res}");
        assert_eq!(res["result"]["method"], method, "{res}");
        let result = res["result"]["result"].clone();
        assert!(result["seq"]["epoch"].is_string(), "{method}: {result}");
        assert!(result["seq"]["n"].is_u64(), "{method}: {result}");
        Ok(result)
    }

    async fn lib(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group_as(Some(WINDOW), "library", method, params).await
    }

    async fn settings(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group_as(Some(WINDOW), "settings", method, params)
            .await
    }

    async fn ui(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group_as(Some(WINDOW), "ui", method, params).await
    }

    async fn project(&self) -> String {
        self.lib("projectEnsureDefault", Json::Null).await.unwrap();
        let list = self.lib("projectsList", Json::Null).await.unwrap();
        list["value"][0]["id"].as_str().unwrap().to_string()
    }

    async fn connection(&self, project: &str) -> String {
        self.lib(
            "connectionCreate",
            json!({"connection": {"projectId": project, "name": "C", "type": "postgres",
                "host": "h", "port": 5432, "databaseName": "d", "username": "u"}}),
        )
        .await
        .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn id_of(v: &Json) -> String {
    v["value"]["id"].as_str().unwrap().to_string()
}

async fn drain(events: &mut (impl futures::Stream<Item = CoreEvent> + Unpin)) -> Vec<Json> {
    let mut got = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await
    {
        got.push(serde_json::to_value(event).unwrap());
    }
    got
}

// ── The library additions ──

#[tokio::test]
async fn every_new_library_method_answers_with_its_own_name() {
    let env = env(false).await;
    let project = env.project().await;
    let conn = env.connection(&project).await;

    // Dashboards.
    let d = env
        .lib(
            "dashboardCreate",
            json!({"dashboard": {"projectId": project, "name": "D",
                "widgets": [ {"id":"w1", "b" : 1} ], "viewport": {"x":0,"y":0,"zoom":1}}}),
        )
        .await
        .unwrap();
    let dashboard = id_of(&d);
    assert!(dashboard.starts_with("dashboard-"), "{d}");
    // Stored as the JSON text that arrived (`json!` sent it compact).
    assert_eq!(d["value"]["widgets"], r#"[{"b":1,"id":"w1"}]"#, "{d}");
    let updated = env
        .lib(
            "dashboardUpdate",
            json!({"id": dashboard, "patch": {"name": "D2", "dateFilter": {"k": 1},
                "captureVersion": true}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["value"]["dashboard"]["name"], "D2");
    assert!(updated["value"]["version"]["id"]
        .as_str()
        .unwrap()
        .starts_with("dver-"));
    assert_eq!(updated["value"]["prunedVersionIds"], json!([]));
    // `null` clears a Clearable, absent keeps it.
    let cleared = env
        .lib(
            "dashboardUpdate",
            json!({"id": dashboard, "patch": {"dateFilter": null}}),
        )
        .await
        .unwrap();
    assert!(
        cleared["value"]["dashboard"]["dateFilter"].is_null(),
        "{cleared}"
    );
    assert_eq!(cleared["value"]["dashboard"]["name"], "D2");
    assert!(cleared["value"]["version"].is_null(), "{cleared}");
    let list = env
        .lib("dashboardsList", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(list["value"].as_array().unwrap().len(), 1);
    let versions = env
        .lib("dashboardVersionsList", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(versions["value"].as_array().unwrap().len(), 1);
    // The list and the update's answer carry no snapshot; the get does.
    let version = &versions["value"][0];
    assert!(version.get("snapshot").is_none(), "{versions}");
    assert!(version["widgetCount"].is_number() && version["bytes"].is_number());
    let whole = env
        .lib(
            "dashboardVersionGet",
            json!({"dashboardId": dashboard, "versionId": version["id"]}),
        )
        .await
        .unwrap();
    assert!(whole["value"]["snapshot"].is_string(), "{whole}");
    assert_eq!(whole["value"]["version"], version["version"]);
    assert_eq!(
        env.lib(
            "dashboardVersionGet",
            json!({"dashboardId": dashboard, "versionId": "dver-none"}),
        )
        .await
        .unwrap_err()
        .code,
        "DASHBOARD_VERSION_NOT_FOUND"
    );
    let removed = env
        .lib("dashboardRemove", json!({"id": dashboard}))
        .await
        .unwrap();
    assert_eq!(removed["value"], Json::Null);
    let missing = env
        .lib("dashboardRemove", json!({"id": dashboard}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "DASHBOARD_NOT_FOUND");

    // Workflows: Core's id and times, the rest byte for byte.
    let w = env
        .lib(
            "workflowCreate",
            json!({"workflow": {"projectId": project,
                "workflow": {"name": "W", "nodes": [], "edges": []}}}),
        )
        .await
        .unwrap();
    let workflow = id_of(&w);
    assert!(workflow.starts_with("workflow-"), "{w}");
    assert_eq!(w["value"]["projectId"], project.as_str());
    let w2 = env
        .lib(
            "workflowUpdate",
            json!({"id": workflow, "workflow": {"name": "W2", "nodes": [], "edges": []}}),
        )
        .await
        .unwrap();
    assert_eq!(w2["value"]["name"], "W2");
    assert_eq!(w2["value"]["id"], workflow.as_str());
    let listed = env
        .lib("workflowsList", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(listed["value"][0]["id"], workflow.as_str());
    assert_eq!(listed["value"][0]["name"], "W2");
    assert!(listed["value"][0].get("nodes").is_none(), "{listed}");
    let got = env
        .lib("workflowGet", json!({"workflowId": workflow}))
        .await
        .unwrap();
    assert_eq!(got["value"], w2["value"]);
    // A rename answers the workflow without its body.
    let renamed = env
        .lib(
            "workflowRename",
            json!({"workflowId": workflow, "name": "W3"}),
        )
        .await
        .unwrap();
    assert_eq!(renamed["value"]["name"], "W3");
    assert!(renamed["value"].get("nodes").is_none(), "{renamed}");
    assert_eq!(
        env.lib(
            "workflowRename",
            json!({"workflowId": "workflow-none", "name": "x"})
        )
        .await
        .unwrap_err()
        .code,
        "WORKFLOW_NOT_FOUND"
    );
    env.lib("workflowRemove", json!({"id": workflow}))
        .await
        .unwrap();
    assert_eq!(
        env.lib("workflowRemove", json!({"id": workflow}))
            .await
            .unwrap_err()
            .code,
        "WORKFLOW_NOT_FOUND"
    );

    // Chats and messages.
    let c = env
        .lib(
            "chatCreate",
            json!({"chat": {"connectionId": conn, "title": "T"}}),
        )
        .await
        .unwrap();
    let chat = id_of(&c);
    env.lib(
        "chatUpdate",
        json!({"id": chat, "patch": {"title": "T2", "touched": true}}),
    )
    .await
    .unwrap();
    let put = env
        .lib(
            "chatMessagesPut",
            json!({"chatId": chat, "messages": [
                {"id": "m1", "role": "user", "content": "hi", "timestamp": "2026-01-01T00:00:00Z"},
                {"id": "m2", "role": "assistant", "content": "yo", "timestamp": "2026-01-01T00:00:00Z",
                 "query": "SELECT 1"}]}),
        )
        .await
        .unwrap();
    assert_eq!(put["value"]["storedBytes"], 4, "{put}");
    let msgs = env
        .lib("chatMessagesList", json!({"chatId": chat}))
        .await
        .unwrap();
    assert_eq!(msgs["value"]["messages"][0]["id"], "m1");
    assert_eq!(msgs["value"]["messages"][1]["id"], "m2");
    let removed = env
        .lib("chatMessagesRemove", json!({"chatId": chat, "ids": ["m1"]}))
        .await
        .unwrap();
    assert_eq!(removed["value"], 1);
    let chats = env
        .lib("chatsList", json!({"connectionId": conn}))
        .await
        .unwrap();
    assert_eq!(chats["value"][0]["title"], "T2");
    env.lib("chatRemove", json!({"id": chat})).await.unwrap();
    assert_eq!(
        env.lib("chatRemove", json!({"id": chat}))
            .await
            .unwrap_err()
            .code,
        "CHAT_NOT_FOUND"
    );

    // The connection order.
    let set = env
        .lib(
            "projectSidebarSet",
            json!({"projectId": project, "connectionOrder": [conn]}),
        )
        .await
        .unwrap();
    assert_eq!(set["value"], json!([conn]));
    let got = env
        .lib("projectSidebarGet", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(got["value"], json!([conn]));
}

// ── Settings ──

#[tokio::test]
async fn every_settings_method_answers_with_its_own_name() {
    let env = env(false).await;

    let set = env
        .settings(
            "settingSet",
            json!({"key": "editorKeybindingMode", "value": "vim"}),
        )
        .await
        .unwrap();
    assert_eq!(set["value"], "vim");
    let got = env
        .settings("settingGet", json!({"key": "editorKeybindingMode"}))
        .await
        .unwrap();
    assert_eq!(got["value"], "vim");
    // `null` deletes.
    env.settings(
        "settingSet",
        json!({"key": "editorKeybindingMode", "value": null}),
    )
    .await
    .unwrap();
    assert!(env
        .settings("settingGet", json!({"key": "editorKeybindingMode"}))
        .await
        .unwrap()["value"]
        .is_null());
    // An unknown key is refused naming it, not a parse error.
    let err = env
        .settings("settingGet", json!({"key": "noSuchKey"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("noSuchKey"), "{}", err.message);
    let err = env
        .settings(
            "settingSet",
            json!({"key": "lastActiveProjectId", "value": "p"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");

    // AI settings and providers.
    let ai = env.settings("aiSettingsGet", Json::Null).await.unwrap();
    assert_eq!(ai["value"]["enabled"], true, "{ai}");
    let patched = env
        .settings(
            "aiSettingsPatch",
            json!({"patch": {"shareDataGlobally": true}}),
        )
        .await
        .unwrap();
    assert_eq!(patched["value"]["shareDataGlobally"], true);
    let created = env
        .settings(
            "aiProviderCreate",
            json!({"provider": {"name": "P", "type": "anthropic"}}),
        )
        .await
        .unwrap();
    let provider = created["value"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["value"]["settings"]["providers"][0]["id"], provider);
    let updated = env
        .settings(
            "aiProviderUpdate",
            json!({"id": provider, "patch": {"name": "P2", "baseUrl": null}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["value"]["providers"][0]["name"], "P2");
    let removed = env
        .settings("aiProviderRemove", json!({"id": provider}))
        .await
        .unwrap();
    assert_eq!(removed["value"]["providers"], json!([]));

    // Themes.
    let themes = env.settings("themesGet", Json::Null).await.unwrap();
    assert_eq!(
        themes["value"]["preferences"],
        json!({"lightThemeId": "default-light", "darkThemeId": "default-dark"})
    );
    let t = env
        .settings(
            "userThemeCreate",
            json!({"theme": {"name": "Mine", "mode": "dark", "colors": {"a": "#fff"}}}),
        )
        .await
        .unwrap();
    let theme = t["value"]["id"].as_str().unwrap().to_string();
    assert!(theme.starts_with("theme-"), "{t}");
    env.settings(
        "themePreferencesSet",
        json!({"lightThemeId": "default-light", "darkThemeId": theme}),
    )
    .await
    .unwrap();
    env.settings(
        "userThemeUpdate",
        json!({"id": theme, "theme": {"name": "Mine 2", "mode": "dark"}}),
    )
    .await
    .unwrap();
    let after = env
        .settings("userThemeRemove", json!({"id": theme}))
        .await
        .unwrap();
    // Removing the dark theme in use resets it.
    assert_eq!(after["value"]["preferences"]["darkThemeId"], "default-dark");
    assert_eq!(
        env.settings("userThemeRemove", json!({"id": theme}))
            .await
            .unwrap_err()
            .code,
        "THEME_NOT_FOUND"
    );

    // Onboarding.
    env.settings("onboardingGet", Json::Null).await.unwrap();
    let ob = env
        .settings("onboardingPatch", json!({"patch": {"learnEnabled": false}}))
        .await
        .unwrap();
    assert_eq!(ob["value"]["learnEnabled"], false, "{ob}");

    // Tutorial.
    let saved = env
        .settings(
            "tutorialSave",
            json!({"lessonId": "l1", "challengeId": "c1", "state": "{\"done\":true}"}),
        )
        .await
        .unwrap();
    assert_eq!(saved["value"][0]["lessonId"], "l1");
    assert_eq!(
        env.settings("tutorialList", Json::Null).await.unwrap()["value"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    env.settings("tutorialRemoveLesson", json!({"lessonId": "l1"}))
        .await
        .unwrap();
    assert_eq!(
        env.settings("tutorialReset", Json::Null).await.unwrap()["value"],
        json!([])
    );

    // Import state.
    assert!(env
        .settings("importStateGet", json!({"source": "tableplus"}))
        .await
        .unwrap()["value"]
        .is_null());
    let saved = env
        .settings(
            "importStateSave",
            json!({"source": "dbeaver", "hasOfferedImport": true, "lastCheckTimestamp": null}),
        )
        .await
        .unwrap();
    assert_eq!(saved["value"]["hasOfferedImport"], true);
    assert_eq!(
        env.settings(
            "importStateSave",
            json!({"source": "other",
            "hasOfferedImport": true, "lastCheckTimestamp": null})
        )
        .await
        .unwrap_err()
        .code,
        "INVALID_ARGUMENT"
    );
}

#[test]
fn setting_set_needs_its_value_even_when_null() {
    // `value` must be present: `null` deletes, and a missing value is a
    // malformed call, not a delete.
    let missing = r#"{"method":"settings","params":{"method":"settingSet","params":{"key":"editorKeybindingMode"}}}"#;
    assert_eq!(
        parse_request(missing.as_bytes()).unwrap_err().code,
        "INVALID_ARGUMENT"
    );
    let null = r#"{"method":"settings","params":{"method":"settingSet","params":{"key":"editorKeybindingMode","value":null}}}"#;
    let req = parse_request(null.as_bytes()).unwrap();
    assert_eq!((req.group(), req.method()), ("settings", "settingSet"));
    assert_eq!(serde_json::to_string(&req).unwrap(), null);
}

#[tokio::test]
async fn an_api_key_is_written_on_the_desktop_and_not_supported_without_a_store() {
    // Without a store (the web): refused before anything is written.
    let env_web = env(false).await;
    let err = env_web
        .settings(
            "aiProviderCreate",
            json!({"provider": {"name": "P", "type": "anthropic"}, "apiKey": "canary-key"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
    assert!(!err.message.contains("canary"));
    assert_eq!(
        env_web.settings("aiSettingsGet", Json::Null).await.unwrap()["value"]["providers"],
        json!([])
    );

    // With one: written under `ai-api-key:<id>`, cleared by `null`.
    let env = env(true).await;
    let store = env.store.clone().unwrap();
    let created = env
        .settings(
            "aiProviderCreate",
            json!({"provider": {"name": "P", "type": "anthropic"}, "apiKey": "canary-key"}),
        )
        .await
        .unwrap();
    let id = created["value"]["id"].as_str().unwrap().to_string();
    let key = format!("ai-api-key:{id}");
    assert_eq!(
        store.get(&key).await.unwrap().as_deref(),
        Some("canary-key")
    );
    // The record never holds the key.
    assert!(!created.to_string().contains("canary"), "{created}");
    // Absent keeps it.
    env.settings(
        "aiProviderUpdate",
        json!({"id": id, "patch": {"name": "P2"}}),
    )
    .await
    .unwrap();
    assert_eq!(
        store.get(&key).await.unwrap().as_deref(),
        Some("canary-key")
    );
    // `null` deletes it.
    env.settings(
        "aiProviderUpdate",
        json!({"id": id, "patch": {}, "apiKey": null}),
    )
    .await
    .unwrap();
    assert_eq!(store.get(&key).await.unwrap(), None);
}

/// A `MemoryStore` that counts reads.
struct CountingStore {
    inner: MemoryStore,
    reads: std::sync::atomic::AtomicUsize,
}

#[seaquel_runtime::async_trait]
impl SecretStore for CountingStore {
    async fn get(&self, key: &str) -> Result<Option<String>, seaquel_core::secrets::SecretError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), seaquel_core::secrets::SecretError> {
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), seaquel_core::secrets::SecretError> {
        self.inner.delete(key).await
    }
}

/// Phase 6 Task 7 (review I2): the page can't read an AI key, so the
/// settings form asks `aiProviderHasKey {id}` when it opens a provider:
/// one keychain read, for that provider only. `aiSettingsGet` reads no
/// secret. The web has no store: `NOT_SUPPORTED` (the page asks its vault).
#[tokio::test]
async fn ai_provider_has_key_reads_one_secret_and_ai_settings_get_none() {
    let core = seaquel_core::with_plugins(|id| id == "postgres")
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(CountingStore {
        inner: MemoryStore::new(),
        reads: Default::default(),
    });
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(store.clone()))
        .await
        .unwrap();
    let env = Env {
        core,
        ws,
        store: None,
        _dir: dir,
    };
    let keyed = env
        .settings(
            "aiProviderCreate",
            json!({"provider": {"name": "Keyed", "type": "anthropic"}, "apiKey": "canary-key"}),
        )
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let keyless = env
        .settings(
            "aiProviderCreate",
            json!({"provider": {"name": "Keyless", "type": "openai-compatible"}}),
        )
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reads = || store.reads.load(std::sync::atomic::Ordering::SeqCst);

    let before = reads();
    let got = env.settings("aiSettingsGet", Json::Null).await.unwrap();
    assert_eq!(reads(), before, "aiSettingsGet read a secret");
    assert!(!got.to_string().contains("hasKey"), "{got}");

    let before = reads();
    let has = env
        .settings("aiProviderHasKey", json!({"id": keyed}))
        .await
        .unwrap();
    assert_eq!(has["value"], true, "{has}");
    assert_eq!(reads(), before + 1);
    let has = env
        .settings("aiProviderHasKey", json!({"id": keyless}))
        .await
        .unwrap();
    assert_eq!(has["value"], false, "{has}");
    assert!(!has.to_string().contains("canary"));
    let err = env
        .settings("aiProviderHasKey", json!({"id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "AI_PROVIDER_NOT_FOUND");

    let web = self::env(false).await;
    let err = web
        .settings("aiProviderHasKey", json!({"id": keyed}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
}

// ── The ui group ──

#[tokio::test]
async fn ui_calls_answer_with_their_own_names_for_the_calling_window() {
    let env = env(false).await;
    let project = env.project().await;

    let got = env
        .ui("windowGet", json!({"windowId": WINDOW}))
        .await
        .unwrap();
    assert_eq!(got["value"], json!({"activeProjectId": null, "from": null}));
    env.ui(
        "windowActivate",
        json!({"windowId": WINDOW, "projectId": project}),
    )
    .await
    .unwrap();
    let got = env
        .ui("windowGet", json!({"windowId": WINDOW}))
        .await
        .unwrap();
    assert_eq!(
        got["value"],
        json!({"activeProjectId": project, "from": "window"})
    );
    let loaded = env
        .ui(
            "windowStateLoad",
            json!({"windowId": WINDOW, "projectId": project}),
        )
        .await
        .unwrap();
    assert_eq!(loaded["value"]["copiedFrom"], "empty", "{loaded}");
    assert_eq!(loaded["value"]["rev"], 0);
    let state = json!({"projectId": project, "queryTabs": [], "schemaTabs": [],
        "explainTabs": [], "erdTabs": [], "tabOrder": [], "activeView": "query"});
    let saved = env
        .ui(
            "windowStateSave",
            json!({"windowId": WINDOW, "projectId": project, "rev": 1, "state": state}),
        )
        .await
        .unwrap();
    assert_eq!(saved["value"], json!({"stale": false, "rev": 1}));
    let stale = env
        .ui(
            "windowStateSave",
            json!({"windowId": WINDOW, "projectId": project, "rev": 1, "state": state}),
        )
        .await
        .unwrap();
    assert_eq!(stale["value"], json!({"stale": true, "rev": 1}));
    let own = env
        .ui(
            "windowStateLoad",
            json!({"windowId": WINDOW, "projectId": project}),
        )
        .await
        .unwrap();
    assert_eq!(own["value"]["rev"], 1);
    assert!(own["value"]["copiedFrom"].is_null(), "{own}");
    assert_eq!(own["value"]["state"]["activeView"], "query");
}

#[tokio::test]
async fn a_window_id_other_than_the_origin_is_refused() {
    let env = env(false).await;
    let project = env.project().await;
    for (origin, method, params) in [
        (Some("win-2"), "windowGet", json!({"windowId": WINDOW})),
        (None, "windowGet", json!({"windowId": WINDOW})),
        (
            Some("win-2"),
            "windowActivate",
            json!({"windowId": WINDOW, "projectId": project}),
        ),
        (
            Some("win-2"),
            "windowStateLoad",
            json!({"windowId": WINDOW, "projectId": project}),
        ),
        (
            Some("win-2"),
            "windowStateSave",
            json!({"windowId": WINDOW, "projectId": project, "rev": 1,
                "state": {"projectId": project}}),
        ),
    ] {
        let err = env
            .group_as(origin, "ui", method, params)
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{method}");
    }
}

#[test]
fn a_bad_rev_is_refused_on_the_wire() {
    for rev in ["-1", "1e300", "1.5", "\"1\"", "18446744073709551616"] {
        let body = format!(
            r#"{{"method":"ui","params":{{"method":"windowStateSave","params":{{"windowId":"w","projectId":"p","rev":{rev},"state":{{}}}}}}}}"#
        );
        assert_eq!(
            parse_request(body.as_bytes()).unwrap_err().code,
            "INVALID_ARGUMENT",
            "{rev}"
        );
    }
}

// ── Wire shapes ──

#[test]
fn new_requests_round_trip_byte_for_byte() {
    for text in [
        // library
        r#"{"method":"library","params":{"method":"dashboardsList","params":{"projectId":"p"}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionsList","params":{"projectId":"p"}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionGet","params":{"dashboardId":"d","versionId":"v"}}}"#,
        r#"{"method":"library","params":{"method":"dashboardCreate","params":{"dashboard":{"projectId":"p","name":"n","widgets":[ 1,2 ],"viewport":{"x" :1}}}}}"#,
        r#"{"method":"library","params":{"method":"dashboardUpdate","params":{"id":"d","patch":{"description":null,"widgets":[],"dateFilter":null,"starred":true,"captureVersion":true}}}}"#,
        r#"{"method":"library","params":{"method":"dashboardRemove","params":{"id":"d"}}}"#,
        r#"{"method":"library","params":{"method":"workflowsList","params":{"projectId":"p"}}}"#,
        r#"{"method":"library","params":{"method":"workflowGet","params":{"workflowId":"w"}}}"#,
        r#"{"method":"library","params":{"method":"workflowCreate","params":{"workflow":{"projectId":"p","workflow":{"b":2, "a":1}}}}}"#,
        r#"{"method":"library","params":{"method":"workflowUpdate","params":{"id":"w","workflow":{"z":[1, 2]}}}}"#,
        r#"{"method":"library","params":{"method":"workflowRemove","params":{"id":"w"}}}"#,
        r#"{"method":"library","params":{"method":"workflowRename","params":{"workflowId":"w","name":"n"}}}"#,
        r#"{"method":"library","params":{"method":"chatsList","params":{"connectionId":"c"}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesList","params":{"chatId":"c"}}}"#,
        r#"{"method":"library","params":{"method":"chatCreate","params":{"chat":{"connectionId":"c","title":"t"}}}}"#,
        r#"{"method":"library","params":{"method":"chatUpdate","params":{"id":"c","patch":{"title":"t","touched":true}}}}"#,
        r#"{"method":"library","params":{"method":"chatRemove","params":{"id":"c"}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesPut","params":{"chatId":"c","messages":[{"id":"m","role":"user","content":"x","timestamp":"t","dashboardId":"d"}]}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesRemove","params":{"chatId":"c","ids":["m"]}}}"#,
        r#"{"method":"library","params":{"method":"projectSidebarGet","params":{"projectId":"p"}}}"#,
        r#"{"method":"library","params":{"method":"projectSidebarSet","params":{"projectId":"p","connectionOrder":["a","b"]}}}"#,
        // settings
        r#"{"method":"settings","params":{"method":"settingGet","params":{"key":"license_nudge"}}}"#,
        r#"{"method":"settings","params":{"method":"settingSet","params":{"key":"query_version_limit","value":"0"}}}"#,
        r#"{"method":"settings","params":{"method":"aiSettingsGet"}}"#,
        r#"{"method":"settings","params":{"method":"aiSettingsPatch","params":{"patch":{"enabled":false}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderCreate","params":{"provider":{"name":"n","type":"openai-compatible","baseUrl":"http://x"}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderCreate","params":{"provider":{"name":"n","type":"anthropic"},"apiKey":"k"}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderUpdate","params":{"id":"a","patch":{"baseUrl":null},"apiKey":null}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderUpdate","params":{"id":"a","patch":{}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderRemove","params":{"id":"a"}}}"#,
        r#"{"method":"settings","params":{"method":"themesGet"}}"#,
        r#"{"method":"settings","params":{"method":"themePreferencesSet","params":{"lightThemeId":"l","darkThemeId":"d"}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeCreate","params":{"theme":{"name":"n" ,"x":1}}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeUpdate","params":{"id":"t","theme":{"name":"n"}}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeRemove","params":{"id":"t"}}}"#,
        r#"{"method":"settings","params":{"method":"onboardingGet"}}"#,
        r#"{"method":"settings","params":{"method":"onboardingPatch","params":{"patch":{"a" : true}}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialList"}}"#,
        r#"{"method":"settings","params":{"method":"tutorialSave","params":{"lessonId":"l","challengeId":"c","state":null}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialRemoveLesson","params":{"lessonId":"l"}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialReset"}}"#,
        r#"{"method":"settings","params":{"method":"importStateGet","params":{"source":"tableplus"}}}"#,
        r#"{"method":"settings","params":{"method":"importStateSave","params":{"source":"dbeaver","hasOfferedImport":false,"lastCheckTimestamp":"t"}}}"#,
        // ui
        r#"{"method":"ui","params":{"method":"windowGet","params":{"windowId":"w"}}}"#,
        r#"{"method":"ui","params":{"method":"windowActivate","params":{"windowId":"w","projectId":"p"}}}"#,
        r#"{"method":"ui","params":{"method":"windowStateLoad","params":{"windowId":"w","projectId":"p"}}}"#,
        r#"{"method":"ui","params":{"method":"windowStateSave","params":{"windowId":"w","projectId":"p","rev":3,"state":{"b":1, "a":[ ]}}}}"#,
    ] {
        let req = parse_request(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(serde_json::to_string(&req).unwrap(), text, "{text}");
    }
}

/// JSON bodies reach Core as the bytes that came in: key order, spacing
/// and number spelling survive into the stored dashboard and workflow.
#[tokio::test]
async fn json_bodies_keep_their_bytes() {
    let env = env(false).await;
    let project = env.project().await;
    let call = |body: String| {
        let env = &env;
        async move {
            let req = parse_request(body.as_bytes()).unwrap();
            let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::new(Some(WINDOW)))
                .await
                .unwrap();
            serde_json::to_value(res).unwrap()["result"]["result"]["value"].clone()
        }
    };
    let widgets = r#"[ {"id":"w1", "b" : 1.50e0} ]"#;
    let d = call(format!(
        r#"{{"method":"library","params":{{"method":"dashboardCreate","params":{{"dashboard":{{"projectId":"{project}","name":"D","widgets":{widgets},"viewport":{{"z":1,"a":2}}}}}}}}}}"#
    ))
    .await;
    assert_eq!(d["widgets"], widgets, "{d}");
    assert_eq!(d["viewport"], r#"{"z":1,"a":2}"#, "{d}");
    let w = call(format!(
        r#"{{"method":"library","params":{{"method":"workflowCreate","params":{{"workflow":{{"projectId":"{project}","workflow":{{"zeta":1e+21,"name":"W","nodes":[]}}}}}}}}}}"#
    ))
    .await;
    let text = w.to_string();
    assert!(text.contains("zeta"), "{text}");
    let raw_answer = |method: &'static str, params: Json| {
        let env = &env;
        async move {
            let req = parse_request(
                json!({"method": "library", "params": {"method": method, "params": params}})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
            let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::none())
                .await
                .unwrap();
            serde_json::to_string(&res).unwrap()
        }
    };
    // `workflowGet` answers the stored JSON as JSON: the number keeps its
    // spelling in the response text. `workflowsList` answers no body.
    let raw = raw_answer("workflowGet", json!({"workflowId": w["id"]})).await;
    assert!(raw.contains(r#""zeta":1e+21"#), "{raw}");
    let raw = raw_answer("workflowsList", json!({"projectId": project})).await;
    assert!(
        raw.contains(r#""name":"W""#) && !raw.contains("zeta"),
        "{raw}"
    );
}

/// Every params object of the new methods, and the `settings` and `ui`
/// envelopes, refuse a field they don't know.
#[test]
fn unknown_request_fields_are_refused() {
    for bad in [
        // library additions: the params, and each draft or patch
        r#"{"method":"library","params":{"method":"dashboardsList","params":{"projectId":"p","x":1}}}"#,
        r#"{"method":"library","params":{"method":"dashboardCreate","params":{"dashboard":{"projectId":"p","name":"n","widgets":[],"viewport":{},"x":1}}}}"#,
        r#"{"method":"library","params":{"method":"dashboardUpdate","params":{"id":"d","patch":{"bogus":1}}}}"#,
        r#"{"method":"library","params":{"method":"dashboardRemove","params":{"id":"d","x":1}}}"#,
        r#"{"method":"library","params":{"method":"workflowCreate","params":{"workflow":{"projectId":"p","workflow":{},"x":1}}}}"#,
        r#"{"method":"library","params":{"method":"workflowUpdate","params":{"id":"w","workflow":{},"x":1}}}"#,
        r#"{"method":"library","params":{"method":"chatCreate","params":{"chat":{"connectionId":"c","title":"t","x":1}}}}"#,
        r#"{"method":"library","params":{"method":"chatUpdate","params":{"id":"c","patch":{"x":1}}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesPut","params":{"chatId":"c","messages":[{"id":"m","role":"user","content":"x","timestamp":"t","toolCalls":[]}]}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesRemove","params":{"chatId":"c","ids":[],"x":1}}}"#,
        r#"{"method":"library","params":{"method":"projectSidebarSet","params":{"projectId":"p","connectionOrder":[],"activeConnectionId":"c"}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionsList","params":{"projectId":"p","x":1}}}"#,
        r#"{"method":"library","params":{"method":"workflowsList","params":{"projectId":"p","x":1}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionGet","params":{"dashboardId":"d","versionId":"v","x":1}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionGet","params":{"dashboardId":"d","versionId":"v","projectId":"p"}}}"#,
        r#"{"method":"library","params":{"method":"dashboardVersionGet","params":{"dashboardId":"d"}}}"#,
        r#"{"method":"library","params":{"method":"workflowGet","params":{"workflowId":"w","x":1}}}"#,
        r#"{"method":"library","params":{"method":"workflowGet","params":{"id":"w"}}}"#,
        r#"{"method":"library","params":{"method":"workflowRename","params":{"workflowId":"w","name":"n","x":1}}}"#,
        r#"{"method":"library","params":{"method":"workflowRename","params":{"workflowId":"w","name":"n","workflow":{}}}}"#,
        r#"{"method":"library","params":{"method":"workflowRename","params":{"workflowId":"w"}}}"#,
        r#"{"method":"library","params":{"method":"workflowRemove","params":{"id":"w","x":1}}}"#,
        r#"{"method":"library","params":{"method":"chatsList","params":{"connectionId":"c","x":1}}}"#,
        r#"{"method":"library","params":{"method":"chatMessagesList","params":{"chatId":"c","x":1}}}"#,
        r#"{"method":"library","params":{"method":"chatRemove","params":{"id":"c","x":1}}}"#,
        r#"{"method":"library","params":{"method":"projectSidebarGet","params":{"projectId":"p","x":1}}}"#,
        // settings
        r#"{"method":"settings","params":{"method":"settingGet","params":{"key":"k","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"settingSet","params":{"key":"k","value":"v","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"aiSettingsGet","params":{}}}"#,
        r#"{"method":"settings","params":{"method":"aiSettingsPatch","params":{"patch":{"providers":[]}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderCreate","params":{"provider":{"name":"n","type":"anthropic","model":"m"}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderCreate","params":{"provider":{"name":"n","type":"anthropic"},"apikey":"k"}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderUpdate","params":{"id":"a","patch":{"x":1}}}}"#,
        r#"{"method":"settings","params":{"method":"aiProviderRemove","params":{"id":"a","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"themePreferencesSet","params":{"lightThemeId":"l","darkThemeId":"d","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeCreate","params":{"theme":{},"x":1}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeUpdate","params":{"id":"t","theme":{},"x":1}}}"#,
        r#"{"method":"settings","params":{"method":"userThemeRemove","params":{"id":"t","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"onboardingPatch","params":{"patch":{},"x":1}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialSave","params":{"lessonId":"l","challengeId":"c","state":null,"x":1}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialRemoveLesson","params":{"lessonId":"l","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialReset","params":{}}}"#,
        r#"{"method":"settings","params":{"method":"tutorialList","params":{}}}"#,
        r#"{"method":"settings","params":{"method":"importStateGet","params":{"source":"s","x":1}}}"#,
        r#"{"method":"settings","params":{"method":"importStateSave","params":{"source":"s","hasOfferedImport":true,"lastCheckTimestamp":null,"x":1}}}"#,
        r#"{"method":"settings","params":{"method":"themesGet"},"x":1}"#,
        r#"{"method":"settings","params":{"method":"themesGet","x":1}}"#,
        // ui
        r#"{"method":"ui","params":{"method":"windowGet","params":{"windowId":"w","x":1}}}"#,
        r#"{"method":"ui","params":{"method":"windowActivate","params":{"windowId":"w","projectId":"p","x":1}}}"#,
        r#"{"method":"ui","params":{"method":"windowStateLoad","params":{"windowId":"w","projectId":"p","x":1}}}"#,
        r#"{"method":"ui","params":{"method":"windowStateSave","params":{"windowId":"w","projectId":"p","rev":1,"state":{},"x":1}}}"#,
        r#"{"method":"ui","params":{"method":"windowForget","params":{"windowId":"w"}}}"#,
    ] {
        let err = parse_request(bad.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{bad}");
    }
}

#[test]
fn a_retired_storage_method_is_unknown() {
    let retired = [
        ("aiChatsLoadByConnection", json!({"connectionId": "c"})),
        ("aiChatsSaveChat", json!({"chat": {}})),
        ("aiChatsRemoveChat", json!({"chatId": "c"})),
        ("aiChatsRemoveByConnection", json!({"connectionId": "c"})),
        ("aiChatsLoadMessages", json!({"chatId": "c"})),
        (
            "aiChatsReplaceAllMessages",
            json!({"chatId": "c", "messages": []}),
        ),
        ("appStateGet", json!({"key": "k"})),
        ("appStateSet", json!({"key": "k", "value": "v"})),
        (
            "connectionOverridesLoad",
            json!({"sharedConnectionId": "c"}),
        ),
        ("connectionOverridesLoadAll", Json::Null),
        ("connectionOverridesSave", json!({"connectionOverride": {}})),
        (
            "connectionOverridesRemove",
            json!({"sharedConnectionId": "c"}),
        ),
        (
            "dashboardVersionsLoadByDashboard",
            json!({"dashboardId": "d"}),
        ),
        ("dashboardVersionsLoadByProject", json!({"projectId": "p"})),
        ("dashboardVersionsInsert", json!({"version": {}})),
        (
            "dashboardVersionsPrune",
            json!({"dashboardId": "d", "deleteIds": []}),
        ),
        ("dashboardsLoadByProject", json!({"projectId": "p"})),
        ("dashboardsSave", json!({"dashboard": {}})),
        ("dashboardsRemove", json!({"id": "d"})),
        ("dashboardsRemoveByProject", json!({"projectId": "p"})),
        ("importStateLoad", json!({"source": "s"})),
        (
            "importStateSave",
            json!({"source": "s", "hasOfferedImport": true, "lastCheckTimestamp": null}),
        ),
        ("onboardingLoad", Json::Null),
        ("onboardingSave", json!({"data": {}})),
        ("projectStateLoad", json!({"projectId": "p"})),
        ("projectStateSave", json!({"state": {}})),
        ("projectStateRemove", json!({"projectId": "p"})),
        ("themesLoadPreferences", Json::Null),
        (
            "themesSavePreferences",
            json!({"lightThemeId": "l", "darkThemeId": "d"}),
        ),
        ("themesLoadUserThemes", Json::Null),
        ("themesSaveUserThemes", json!({"themes": []})),
        ("tutorialLoadAll", Json::Null),
        (
            "tutorialSave",
            json!({"lessonId": "l", "challengeId": "c", "state": null}),
        ),
        ("tutorialRemoveLesson", json!({"lessonId": "l"})),
        ("tutorialRemoveAll", Json::Null),
    ];
    assert_eq!(retired.len(), 35);
    for (method, params) in retired {
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
    // What stays: the vault, the license and the history (phase 5e moved
    // the shared repos to the `shared` group).
    for (method, params) in [
        ("queryHistoryLoadByConnection", json!({"connectionId": "c"})),
        ("licenseLoad", Json::Null),
        ("vaultStateLoad", Json::Null),
        ("userCredentialsLoad", json!({"scope": "db", "key": "c"})),
    ] {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let body = json!({"method": "storage", "params": inner}).to_string();
        parse_request(body.as_bytes()).unwrap_or_else(|e| panic!("{method}: {e}"));
    }
}

#[test]
fn debug_redacts_every_new_params_type() {
    let bodies = [
        json!({"method": "library", "params": {"method": "dashboardCreate", "params": {"dashboard": {
            "projectId": "p", "name": "canary-name", "description": "canary-d",
            "widgets": [{"sql": "canary-widget"}], "viewport": {"canary": 1},
            "dateFilter": {"canary": 2}}}}}),
        json!({"method": "library", "params": {"method": "dashboardUpdate", "params": {"id": "d",
            "patch": {"name": "canary-name", "widgets": ["canary-widget"], "dateFilter": "canary"}}}}),
        json!({"method": "library", "params": {"method": "workflowCreate", "params": {"workflow": {
            "projectId": "p", "workflow": {"name": "canary-name", "rows": ["canary-row"]}}}}}),
        json!({"method": "library", "params": {"method": "workflowUpdate", "params": {"id": "w",
            "workflow": {"name": "canary-name"}}}}),
        json!({"method": "library", "params": {"method": "workflowRename", "params": {
            "workflowId": "w", "name": "canary-name"}}}),
        json!({"method": "library", "params": {"method": "chatCreate", "params": {"chat": {
            "connectionId": "c", "title": "canary-title"}}}}),
        json!({"method": "library", "params": {"method": "chatUpdate", "params": {"id": "c",
            "patch": {"title": "canary-title"}}}}),
        json!({"method": "library", "params": {"method": "chatMessagesPut", "params": {"chatId": "c",
            "messages": [{"id": "m", "role": "user", "content": "canary-content", "timestamp": "t",
            "query": "SELECT 'canary'"}]}}}),
        json!({"method": "settings", "params": {"method": "settingSet", "params": {
            "key": "license_nudge", "value": "{\"canary\":1}"}}}),
        json!({"method": "settings", "params": {"method": "aiProviderCreate", "params": {
            "provider": {"name": "canary-name", "type": "anthropic", "baseUrl": "http://canary"},
            "apiKey": "canary-key"}}}),
        json!({"method": "settings", "params": {"method": "aiProviderUpdate", "params": {"id": "a",
            "patch": {"name": "canary-name"}, "apiKey": "canary-key"}}}),
        json!({"method": "settings", "params": {"method": "userThemeCreate", "params": {
            "theme": {"name": "canary-name", "colors": {"canary": "#fff"}}}}}),
        json!({"method": "settings", "params": {"method": "userThemeUpdate", "params": {"id": "t",
            "theme": {"name": "canary-name"}}}}),
        json!({"method": "settings", "params": {"method": "onboardingPatch", "params": {
            "patch": {"canary": true}}}}),
        json!({"method": "settings", "params": {"method": "tutorialSave", "params": {
            "lessonId": "l", "challengeId": "c", "state": "canary-state"}}}),
        json!({"method": "ui", "params": {"method": "windowStateSave", "params": {
            "windowId": "canary-window", "projectId": "p", "rev": 1,
            "state": {"queryTabs": [{"query": "SELECT 'canary'"}]}}}}),
        json!({"method": "ui", "params": {"method": "windowGet", "params": {
            "windowId": "canary-window"}}}),
    ];
    for body in bodies {
        let req = parse_request(body.to_string().as_bytes()).unwrap();
        let text = format!("{req:?} {req:#?}");
        assert!(!text.contains("canary"), "{text}");
    }
}

#[tokio::test]
async fn responses_debug_shows_no_values() {
    let env = env(false).await;
    let project = env.project().await;
    let body = json!({"method": "library", "params": {"method": "workflowCreate", "params": {
        "workflow": {"projectId": project, "workflow": {"name": "canary-name"}}}}});
    let req = parse_request(body.to_string().as_bytes()).unwrap();
    let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::none())
        .await
        .unwrap();
    let text = format!("{res:?}");
    assert!(!text.contains("canary"), "{text}");
    let body = json!({"method": "settings", "params": {"method": "userThemeCreate", "params": {
        "theme": {"name": "canary-name"}}}});
    let req = parse_request(body.to_string().as_bytes()).unwrap();
    let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::none())
        .await
        .unwrap();
    let text = format!("{res:?}");
    assert!(!text.contains("canary"), "{text}");
}

// ── Events ──

#[tokio::test]
async fn each_new_kind_crosses_as_storage_changed_with_its_origin_and_no_values() {
    let env = env(false).await;
    let project = env.project().await;
    let conn = env.connection(&project).await;
    let mut events = workspace_events(&env.ws);

    let d = env
        .lib(
            "dashboardCreate",
            json!({"dashboard": {"projectId": project, "name": "canary-name",
                "widgets": [], "viewport": {}}}),
        )
        .await
        .unwrap();
    let w = env
        .lib(
            "workflowCreate",
            json!({"workflow": {"projectId": project, "workflow": {"name": "canary"}}}),
        )
        .await
        .unwrap();
    let c = env
        .lib(
            "chatCreate",
            json!({"chat": {"connectionId": conn, "title": "canary"}}),
        )
        .await
        .unwrap();
    let chat = id_of(&c);
    env.lib(
        "chatMessagesPut",
        json!({"chatId": chat, "messages": [{"id": "m1", "role": "user",
            "content": "canary", "timestamp": "t"}]}),
    )
    .await
    .unwrap();
    env.settings(
        "settingSet",
        json!({"key": "skippedUpdateVersion", "value": "9.9.9"}),
    )
    .await
    .unwrap();
    env.settings("aiSettingsPatch", json!({"patch": {"enabled": false}}))
        .await
        .unwrap();
    let theme = env
        .settings("userThemeCreate", json!({"theme": {"name": "canary"}}))
        .await
        .unwrap()["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.settings(
        "onboardingPatch",
        json!({"patch": {"dismissedHints": ["canary"]}}),
    )
    .await
    .unwrap();
    env.settings(
        "tutorialSave",
        json!({"lessonId": "l1", "challengeId": "c1", "state": "canary"}),
    )
    .await
    .unwrap();
    env.settings(
        "importStateSave",
        json!({"source": "tableplus", "hasOfferedImport": true, "lastCheckTimestamp": null}),
    )
    .await
    .unwrap();
    env.ui(
        "windowActivate",
        json!({"windowId": WINDOW, "projectId": project}),
    )
    .await
    .unwrap();
    env.lib(
        "projectSidebarSet",
        json!({"projectId": project, "connectionOrder": [conn]}),
    )
    .await
    .unwrap();

    let got = drain(&mut events).await;
    for event in &got {
        assert_eq!(event["type"], "storageChanged", "{event}");
        assert_eq!(event["origin"], WINDOW, "{event}");
        assert!(!event.to_string().contains("canary"), "{event}");
    }
    let summary: Vec<Json> = got
        .iter()
        .map(|e| json!([e["kind"], e["scope"], e["ids"]]))
        .collect();
    assert_eq!(
        summary,
        [
            json!(["dashboard", project, [id_of(&d)]]),
            json!(["workflow", project, [id_of(&w)]]),
            json!(["chat", conn, [chat]]),
            json!(["chatMessages", chat, ["m1"]]),
            json!(["setting", null, ["skippedUpdateVersion"]]),
            json!(["aiSettings", null, null]),
            json!(["theme", null, [theme]]),
            json!(["onboarding", null, null]),
            json!(["tutorial", null, ["l1"]]),
            json!(["importState", null, ["tableplus"]]),
            json!(["projectState", project, [WINDOW]]),
            json!(["project", null, [project]]),
        ],
        "{got:#?}"
    );
    // Reads emit nothing.
    env.settings("themesGet", Json::Null).await.unwrap();
    env.ui("windowGet", json!({"windowId": WINDOW}))
        .await
        .unwrap();
    assert_eq!(drain(&mut events).await, Vec::<Json>::new());
}

#[tokio::test]
async fn the_remaining_storage_writes_still_emit_storage_events() {
    let env = env(false).await;
    let mut events = workspace_events(&env.ws);
    env.call_as(
        Some("tab-2"),
        &json!({"method": "storage", "params": {"method": "userCredentialsSave",
            "params": {"credential": {"scope": "db", "key": "conn-1", "nonce": "n",
                "ciphertext": "canary-cipher", "updatedAt": "t"}}}}),
    )
    .await
    .unwrap();
    let got = drain(&mut events).await;
    assert_eq!(got.len(), 1, "{got:#?}");
    assert_eq!(got[0]["kind"], "storage");
    assert_eq!(got[0]["ids"], json!(["conn-1"]));
    assert_eq!(got[0]["origin"], "tab-2");
    assert!(!got[0].to_string().contains("canary"));
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
async fn state_calls_log_group_method_and_code_only() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
    let env = env(true).await;
    let project = env.project().await;
    let origin = Some("canary-origin");
    env.group_as(
        origin,
        "settings",
        "aiProviderCreate",
        json!({"provider": {"name": "canary-name", "type": "anthropic"}, "apiKey": "canary-key"}),
    )
    .await
    .unwrap();
    env.group_as(
        origin,
        "settings",
        "settingSet",
        json!({"key": "license_nudge", "value": "{\"canary\":1}"}),
    )
    .await
    .unwrap();
    env.group_as(
        origin,
        "library",
        "dashboardCreate",
        json!({"dashboard": {"projectId": project, "name": "canary-name",
            "widgets": [{"canary": 1}], "viewport": {}}}),
    )
    .await
    .unwrap();
    // A refusal logs its code: a window id that isn't the origin.
    env.group_as(
        origin,
        "ui",
        "windowStateSave",
        json!({"windowId": "canary-window", "projectId": project, "rev": 1,
            "state": {"queryTabs": [{"query": "canary-text"}]}}),
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
            .any(|r| r.contains("group=settings") && r.contains("method=aiProviderCreate")),
        "{records:#?}"
    );
    assert!(
        records.iter().any(|r| r.contains("group=ui")
            && r.contains("method=windowStateSave")
            && r.contains("code=INVALID_ARGUMENT")),
        "{records:#?}"
    );
}

/// Phase 5e review, C1 and re-review R1: every library call that may
/// publish a shared file runs on a 2 MiB thread, the size of a debug
/// desktop build's tokio workers, through `dispatch_workspace`, on a Core
/// with `LocalFiles` and a project linked to a real repo. The calls take
/// every boxed path: a publish, a publish that finds a teammate's change
/// (stale, then the sync and its row writes), and a stale unshare and
/// remove. 768 KiB of the thread is taken first, so the calls must fit in
/// the remaining 1.25 MiB (a caller's own frames need room too). A stack
/// overflow aborts the process, which fails this test binary.
#[test]
fn library_calls_fit_a_2_mib_stack() {
    #[inline(never)]
    fn reserved<R>(f: impl FnOnce() -> R) -> R {
        let pad = [0u8; RESERVED_KIB * 1024];
        std::hint::black_box(&pad);
        let r = f();
        std::hint::black_box(&pad);
        r
    }
    let worker = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            reserved(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(publishing_calls());
            })
        })
        .unwrap();
    worker.join().unwrap();
}

/// How much of the 2 MiB thread `library_calls_fit_a_2_mib_stack` takes
/// before the calls run.
const RESERVED_KIB: usize = 768;

async fn publishing_calls() {
    let env = files_env().await;
    let project = env.project().await;
    let repo = env._dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git2::Repository::init(&repo).unwrap();
    env.ws
        .shared_link_project(
            &env.core,
            &WriteOrigin::new(Some(WINDOW)),
            &project,
            &repo.to_string_lossy(),
            &[],
        )
        .await
        .unwrap();
    let conn = env.connection(&project).await;
    let on_disk = |shared_path: &Json| repo.join(shared_path.as_str().unwrap());
    let teammate = |path: &std::path::Path, from: &str, to: &str| {
        let text = std::fs::read_to_string(path).unwrap();
        std::fs::write(path, text.replace(from, to)).unwrap();
    };
    let code = |v: &Json| v["projection"]["code"].as_str().map(str::to_string);

    // A shared connection: its template is written.
    let shared = env
        .lib(
            "connectionUpdate",
            json!({"id": conn, "patch": {"isLocalOnly": false}}),
        )
        .await
        .unwrap();
    assert_eq!(shared["projection"]["status"], "written", "{shared}");
    env.lib(
        "connectionUpdate",
        json!({"id": conn, "patch": {"host": "h2"}}),
    )
    .await
    .unwrap();

    // A saved query: written, then a stale update (publish → stale → sync
    // → the row takes the file), then a stale unshare.
    let q = env
        .lib(
            "savedQueryCreate",
            json!({"query": {"projectId": project, "name": "Q", "query": "SELECT 1",
                "shared": true}}),
        )
        .await
        .unwrap();
    assert_eq!(q["projection"]["status"], "written", "{q}");
    let q_file = on_disk(&q["value"]["sharedPath"]);
    let q = id_of(&q);
    teammate(&q_file, "SELECT 1", "SELECT 'theirs'");
    let stale = env
        .lib(
            "savedQueryUpdate",
            json!({"id": q, "patch": {"query": "SELECT 2"}}),
        )
        .await
        .unwrap();
    assert_eq!(code(&stale).as_deref(), Some("FILE_CHANGED"), "{stale}");
    teammate(&q_file, "SELECT 'theirs'", "SELECT 'again'");
    let stale = env
        .lib(
            "savedQueryUpdate",
            json!({"id": q, "patch": {"shared": false}}),
        )
        .await
        .unwrap();
    assert_eq!(code(&stale).as_deref(), Some("FILE_CHANGED"), "{stale}");

    // A dashboard: written, renamed, then a stale remove.
    let d = env
        .lib(
            "dashboardCreate",
            json!({"dashboard": {"projectId": project, "name": "D", "shared": true,
                "widgets": [], "viewport": {"x":0,"y":0,"zoom":1}}}),
        )
        .await
        .unwrap();
    assert_eq!(d["projection"]["status"], "written", "{d}");
    let d_id = id_of(&d);
    let renamed = env
        .lib(
            "dashboardUpdate",
            json!({"id": d_id, "patch": {"name": "D2"}}),
        )
        .await
        .unwrap();
    let d_file = on_disk(&renamed["value"]["dashboard"]["sharedPath"]);
    teammate(
        &d_file,
        "\"widgets\": []",
        "\"widgets\": [{\"id\": \"w9\"}]",
    );
    let stale = env
        .lib("dashboardRemove", json!({"id": d_id}))
        .await
        .unwrap();
    assert_eq!(code(&stale).as_deref(), Some("FILE_CHANGED"), "{stale}");

    // The project's rename and the connection's removal.
    env.lib(
        "projectUpdate",
        json!({"id": project, "patch": {"name": "Renamed"}}),
    )
    .await
    .unwrap();
    let removed = env
        .lib("connectionRemove", json!({"id": conn}))
        .await
        .unwrap();
    assert_eq!(removed["projection"]["status"], "deleted", "{removed}");
}

/// [`env`] with secrets, on a Core that may touch the user's files, with
/// libgit2 kept away from the user's git config.
async fn files_env() -> Env {
    static ISOLATE: Once = Once::new();
    static EMPTY: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ISOLATE.call_once(|| {
        let dir = EMPTY.get_or_init(|| tempfile::tempdir().unwrap());
        for level in [
            git2::ConfigLevel::Global,
            git2::ConfigLevel::XDG,
            git2::ConfigLevel::System,
            git2::ConfigLevel::ProgramData,
        ] {
            // SAFETY: once, before this binary's other libgit2 calls.
            unsafe { git2::opts::set_search_path(level, dir.path()).unwrap() };
        }
    });
    let core = seaquel_core::with_plugins(|id| id == "postgres")
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(seaquel_core::LocalFiles::Allowed)
        .build();
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(MemoryStore::new());
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(store.clone()))
        .await
        .unwrap();
    Env {
        core,
        ws,
        store: Some(store),
        _dir: dir,
    }
}
