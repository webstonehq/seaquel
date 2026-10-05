//! State through Core (phase 5d-2): the replay of every state fixture
//! (`crates/seaquel-workspace/tests/fixtures/state`) under its README's
//! Rust replay rules and `changes.json`, and Core's own rules for
//! dashboards, saved workflows, chats and messages, and the change events
//! every state write emits.
//!
//! Storage is a temp dir and secrets a `TestStore`; nothing touches the real
//! keychain or data dir.
#![cfg(all(feature = "storage", feature = "secrets"))]
// Native-only tests: tasks on the test runtime and the wall clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use common::{
    drop_link_columns, dump, fx_with, insert_rows, state_core, web_state_limits, Fx, TestStore,
    TickClock, T0,
};
use futures::StreamExt;
use seaquel_core::domain::library::name_key;
use seaquel_core::domain::state::{
    AiProviderDraft, AiProviderPatch, AiSettingsPatch, ChatDraft, ChatMessageDraft, ChatPatch,
    DashboardDraft, DashboardPatch, WorkflowDraft,
};
use seaquel_core::storage::{Storage, StorageOptions};
use seaquel_core::{
    Core, CoreError, StateLimits, StoredKind, Workspace, WorkspaceEvent, WorkspaceSpec, WriteOrigin,
};
use seaquel_types::storage::{ImportState, ThemePreferences, TutorialProgress};
use serde_json::value::RawValue;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-workspace/tests/fixtures/state"
);
const FILES: [&str; 9] = [
    "view-state.json",
    "workflows.json",
    "dashboards.json",
    "chats.json",
    "settings.json",
    "ai-settings.json",
    "themes.json",
    "misc.json",
    "old-data.json",
];
const BETA_SCHEMA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-storage/tests/fixtures/schemas/v2026.4.5-beta.1.sql"
);

/// The recorder's tables, in seed order.
const SEED_TABLES: [&str; 16] = [
    "projects",
    "connections",
    "connection_overrides",
    "project_state",
    "tabs",
    "saved_canvases",
    "dashboards",
    "dashboard_versions",
    "ai_chats",
    "ai_messages",
    "app_state",
    "theme_preferences",
    "user_themes",
    "onboarding_state",
    "tutorial_progress",
    "import_state",
];

/// The tables compared whole after every step, with their sort keys.
const COMPARED: [(&str, &str); 14] = [
    ("connection_overrides", "shared_connection_id"),
    ("project_state", "project_id"),
    ("tabs", "project_id, id"),
    ("saved_canvases", "id"),
    ("dashboards", "id"),
    ("dashboard_versions", "dashboard_id, version"),
    ("ai_chats", "id"),
    ("ai_messages", "chat_id, id"),
    ("app_state", "key"),
    ("theme_preferences", "id"),
    ("user_themes", "id"),
    ("onboarding_state", "id"),
    ("tutorial_progress", "lesson_id, challenge_id"),
    ("import_state", "source"),
];

/// Columns holding JSON, compared as parsed JSON.
fn json_column(table: &str, column: &str, row: &Value) -> bool {
    matches!(
        (table, column),
        ("saved_canvases", "data")
            | ("dashboards", "widgets" | "viewport" | "date_filter")
            | ("dashboard_versions", "snapshot")
            | ("user_themes", "data")
            | ("onboarding_state", "data")
            | (
                "project_state",
                "tab_order"
                    | "connection_order"
                    | "pane_layout"
                    | "starred_shared_query_ids"
                    | "starred_shared_dashboard_ids"
            )
    ) || (table == "app_state"
        && column == "value"
        && matches!(row["key"].as_str(), Some("license_nudge" | "aiSettings")))
}

// ── Tokens ──

/// Every `<id:n>` token in `s`.
fn tokens_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = s[from..].find("<id:") {
        let start = from + at;
        let Some(close) = s[start..].find('>') else {
            break;
        };
        let end = start + close + 1;
        out.push(s[start..end].to_string());
        from = end;
    }
    out
}

fn collect_tokens(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.extend(tokens_in(s)),
        Value::Array(a) => a.iter().for_each(|v| collect_tokens(v, out)),
        Value::Object(o) => o.iter().for_each(|(k, v)| {
            out.extend(tokens_in(k));
            collect_tokens(v, out)
        }),
        _ => {}
    }
}

fn substitute(s: &str, bound: &HashMap<String, String>) -> String {
    let mut out = s.to_string();
    for t in tokens_in(s) {
        if let Some(v) = bound.get(&t) {
            out = out.replace(&t, v);
        }
    }
    out
}

/// `v` with its tokens bound and `<now>` sent as `now`.
fn prepare(v: &Value, bound: &HashMap<String, String>, now: &str) -> Value {
    match v {
        Value::String(s) if s == "<now>" => Value::String(now.to_string()),
        Value::String(s) => Value::String(substitute(s, bound)),
        Value::Array(a) => Value::Array(a.iter().map(|v| prepare(v, bound, now)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (substitute(k, bound), prepare(v, bound, now)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn is_uuid_v4(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.get_version_num() == 4) && s.len() == 36
}

fn is_iso(s: &str) -> bool {
    s.len() == 24 && s.ends_with('Z') && s.as_bytes()[10] == b'T'
}

// ── Matching ──

struct Ctx<'a> {
    bound: &'a HashMap<String, String>,
    started: &'a str,
}

/// Whether `actual` matches `expected`: tokens bound, `<now>` any time
/// from the case (or the fixed time the GUI sent), numbers by value.
fn matches(expected: &Value, actual: &Value, cx: &Ctx) -> bool {
    match (expected, actual) {
        (Value::String(e), Value::String(a)) => {
            if e == "<now>" {
                return is_iso(a) && a.as_str() >= cx.started;
            }
            substitute(e, cx.bound) == *a
        }
        (Value::Number(e), Value::Number(a)) => e.as_f64() == a.as_f64(),
        (Value::Null, Value::Null) => true,
        (Value::Bool(e), Value::Bool(a)) => e == a,
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(e, a)| matches(e, a, cx))
        }
        (Value::Object(e), Value::Object(a)) => {
            e.len() == a.len()
                && e.iter().all(|(k, v)| {
                    a.get(&substitute(k, cx.bound))
                        .is_some_and(|av| matches(v, av, cx))
                })
        }
        _ => false,
    }
}

/// The legacy provider cleanup, on a parsed `aiSettings` record.
fn clean_ai(v: &mut Value) {
    if let Some(ps) = v.get_mut("providers").and_then(Value::as_array_mut) {
        for p in ps {
            if let Some(o) = p.as_object_mut() {
                o.remove("model");
                let legacy = o.remove("provider").filter(|v| !v.is_null());
                if o.get("type").is_none_or(Value::is_null) {
                    o.insert("type".into(), legacy.unwrap_or(json!("anthropic")));
                }
            }
        }
    }
}

/// A row with its JSON columns parsed (text that doesn't parse stays
/// text), a snapshot's `description: null` dropped, and `aiSettings`
/// cleaned.
fn normal_row(table: &str, row: &Value) -> Value {
    let mut out = row.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    let cols: Vec<String> = obj.keys().cloned().collect();
    for c in cols {
        if !json_column(table, &c, row) {
            continue;
        }
        let Some(text) = obj[&c].as_str() else {
            continue;
        };
        let Ok(mut parsed) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if table == "dashboard_versions" {
            if let Some(o) = parsed.as_object_mut() {
                if o.get("description").is_some_and(Value::is_null) {
                    o.remove("description");
                }
            }
        }
        if table == "app_state" && row["key"] == "aiSettings" {
            clean_ai(&mut parsed);
        }
        obj.insert(c, json!({ "$json": parsed }));
    }
    out
}

fn sort_key(row: &Value, order: &str, bound: &HashMap<String, String>) -> String {
    order
        .split(',')
        .map(|c| match &row[c.trim()] {
            Value::String(s) => substitute(s, bound),
            Value::Number(n) => format!("{:020.6}", n.as_f64().unwrap_or_default()),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\u{1}")
}

fn rows_match(table: &str, order: &str, expected: &[Value], actual: &[Value], cx: &Ctx) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut e: Vec<Value> = expected.iter().map(|r| normal_row(table, r)).collect();
    let mut a: Vec<Value> = actual.iter().map(|r| normal_row(table, r)).collect();
    e.sort_by_key(|r| sort_key(r, order, cx.bound));
    a.sort_by_key(|r| sort_key(r, order, cx.bound));
    e.iter().zip(&a).all(|(e, a)| matches(e, a, cx))
}

// ── Reading what a step left ──

struct Snapshot {
    tables: BTreeMap<String, Vec<Value>>,
    secrets: BTreeMap<String, String>,
}

async fn snapshot(st: &Storage, store: &TestStore) -> Snapshot {
    let mut tables = BTreeMap::new();
    for (table, order) in COMPARED {
        let mut rows = dump(st, table, order).await;
        drop_link_columns(table, &mut rows);
        if table == "dashboards" {
            // Migration `0002` added `name_key`: each must be its row's.
            for row in &mut rows {
                let obj = row.as_object_mut().unwrap();
                if let Some(Value::String(key)) = obj.remove("name_key") {
                    let name = obj["name"].as_str().unwrap_or_default();
                    assert_eq!(key, name_key(name), "{row}");
                }
            }
        }
        if table == "saved_canvases" {
            // Migration `0003` added `meta`: each must be its row's.
            for row in &mut rows {
                let obj = row.as_object_mut().unwrap();
                if let Some(Value::String(meta)) = obj.remove("meta") {
                    if meta == "null" {
                        // Marked: a body that doesn't read.
                        continue;
                    }
                    let data: Value =
                        serde_json::from_str(obj["data"].as_str().unwrap_or("null")).unwrap();
                    let text = |k: &str| data.get(k).filter(|v| v.is_string()).cloned();
                    assert_eq!(
                        serde_json::from_str::<Value>(&meta).unwrap(),
                        json!({"name": text("name"), "createdAt": text("createdAt"),
                            "updatedAt": text("updatedAt")}),
                        "{row}"
                    );
                }
            }
        }
        if table == "dashboard_versions" {
            // Migration `0003` added `widget_count`: each must be its row's.
            for row in &mut rows {
                let obj = row.as_object_mut().unwrap();
                if let Some(Value::Number(n)) = obj.remove("widget_count") {
                    let snap: Value =
                        serde_json::from_str(obj["snapshot"].as_str().unwrap()).unwrap();
                    assert_eq!(
                        n.as_u64(),
                        snap["widgets"].as_array().map(|w| w.len() as u64),
                        "{row}"
                    );
                } else {
                    obj.remove("widget_count");
                }
            }
        }
        tables.insert(table.to_string(), rows);
    }
    Snapshot {
        tables,
        secrets: store.entries(),
    }
}

// ── Running a case ──

struct Replay {
    _dir: tempfile::TempDir,
    core: Core,
    ws: Arc<Workspace>,
    store: Arc<TestStore>,
    bound: HashMap<String, String>,
    /// Tokens a create binds (`binds`), never given a GUI uuid.
    core_tokens: HashSet<String>,
    started: String,
}

async fn raw_file(path: &Path) -> sqlx::SqlitePool {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    sqlx::SqlitePool::connect_with(opts).await.unwrap()
}

async fn open_case(case: &Value) -> Replay {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    if case["file"] == "v2026.4.5-beta.1" {
        let sql = std::fs::read_to_string(BETA_SCHEMA).unwrap();
        let pool = raw_file(&path).await;
        sqlx::raw_sql(&sql).execute(&pool).await.unwrap();
        pool.close().await;
    }
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    st.close().await;
    let store = TestStore::new();
    if let Some(secrets) = case["secrets"].as_object() {
        for (k, v) in secrets {
            store.put(k, v.as_str().unwrap());
        }
    }
    let clock = TickClock::new();
    let started = seaquel_core::domain::run::iso_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap(),
    );
    let core = state_core(clock, StateLimits::default());
    let spec = WorkspaceSpec::new(dir.path());
    let spec = if case["target"] == "web" {
        spec
    } else {
        spec.with_secrets(store.clone())
    };
    let ws = core.open_workspace(spec).await.unwrap();
    // The open's own key (the string-secrets upgrade ran and found
    // nothing), which the recording never had.
    sqlx::query("DELETE FROM app_state WHERE key = 'connectionStringSecretsUpgraded'")
        .execute(ws.storage().pool())
        .await
        .unwrap();
    // The seed goes in after the open: the frozen baseline, which runs on
    // every open, rewrites a stored `active_view` of `canvas` (and the
    // data steps fill keys), while the recording's rows are the seed as
    // inserted (`old-data/project-state-canvas-view`). A seeded dashboard
    // so has no `name_key`, as a row an older release wrote.
    for table in SEED_TABLES {
        if let Some(rows) = case["seed"][table].as_array() {
            insert_rows(ws.storage(), table, rows).await;
        }
    }
    let mut core_tokens = HashSet::new();
    for step in case["steps"].as_array().unwrap() {
        for call in step["core"].as_array().into_iter().flatten() {
            if let Some(b) = call["binds"].as_str() {
                core_tokens.extend(tokens_in(b));
            }
        }
    }
    Replay {
        _dir: dir,
        core,
        ws,
        store,
        bound: HashMap::new(),
        core_tokens,
        started,
    }
}

fn param<T: serde::de::DeserializeOwned>(p: &Value, key: &str) -> T {
    serde_json::from_value(p.get(key).cloned().unwrap_or(Value::Null))
        .unwrap_or_else(|e| panic!("params.{key}: {e}\n{p}"))
}

fn opt_param<T: serde::de::DeserializeOwned + Default>(p: &Value, key: &str) -> T {
    match p.get(key) {
        None => T::default(),
        Some(v) => {
            serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("params.{key}: {e}"))
        }
    }
}

/// `Clearable`: absent `None`, `null` `Some(None)`.
fn clearable(p: &Value, key: &str) -> Option<Option<String>> {
    p.get(key).map(|v| v.as_str().map(str::to_string))
}

fn raw_param(p: &Value, key: &str) -> Box<RawValue> {
    RawValue::from_string(p[key].to_string()).unwrap()
}

fn to_json<T: serde::Serialize>(
    r: Result<seaquel_core::Seqd<T>, CoreError>,
) -> Result<Value, CoreError> {
    r.map(|v| serde_json::to_value(v.value).unwrap())
}

impl Replay {
    /// Gives each GUI token in `raw` (not yet bound, not a Core id's) a v4
    /// uuid of its own, then runs the call.
    async fn call(&mut self, call: &Value) -> Result<Value, CoreError> {
        let method = call["method"].as_str().unwrap();
        let raw = call.get("params").cloned().unwrap_or(Value::Null);
        let mut tokens = Vec::new();
        collect_tokens(&raw, &mut tokens);
        for t in tokens {
            if self.bound.contains_key(&t) {
                continue;
            }
            assert!(
                !self.core_tokens.contains(&t),
                "{t} in {method} names a Core id not made yet"
            );
            self.bound.insert(t, uuid::Uuid::new_v4().to_string());
        }
        let p = prepare(&raw, &self.bound, &self.started);
        let (core, ws) = (&self.core, &self.ws);
        let replay = WriteOrigin::new(Some("replay"));
        let window = WriteOrigin::new(p["windowId"].as_str());
        let s = |k: &str| -> String { param::<String>(&p, k) };
        match method {
            // ui
            "windowGet" => to_json(ws.get_window(&window, &s("windowId")).await),
            "windowStateLoad" => to_json(
                ws.load_window_state(core, &window, &s("windowId"), &s("projectId"))
                    .await,
            ),
            "windowStateSave" => to_json(
                ws.save_window_state(
                    core,
                    &window,
                    &s("windowId"),
                    &s("projectId"),
                    param::<u64>(&p, "rev"),
                    raw_param(&p, "state"),
                )
                .await,
            ),
            "windowActivate" => to_json(
                ws.activate_window(core, &window, &s("windowId"), &s("projectId"))
                    .await,
            ),
            // library
            "dashboardsList" => to_json(ws.list_dashboards(core, &s("projectId")).await),
            "dashboardVersionsList" => {
                to_json(ws.list_dashboard_versions(core, &s("projectId")).await)
            }
            "dashboardCreate" => to_json(
                ws.create_dashboard(core, &replay, param::<DashboardDraft>(&p, "dashboard"))
                    .await,
            ),
            "dashboardUpdate" => to_json(
                ws.update_dashboard(
                    core,
                    &replay,
                    &s("id"),
                    param::<DashboardPatch>(&p, "patch"),
                )
                .await,
            ),
            "dashboardRemove" => to_json(ws.remove_dashboard(core, &replay, &s("id")).await),
            "workflowsList" => to_json(ws.list_workflows(core, &s("projectId")).await),
            "workflowCreate" => to_json(
                ws.create_workflow(core, &replay, param::<WorkflowDraft>(&p, "workflow"))
                    .await,
            ),
            "workflowUpdate" => to_json(
                ws.update_workflow(core, &replay, &s("id"), raw_param(&p, "workflow"))
                    .await,
            ),
            "workflowRemove" => to_json(ws.remove_workflow(core, &replay, &s("id")).await),
            "chatsList" => to_json(ws.list_chats(core, &s("connectionId")).await),
            "chatMessagesList" => to_json(ws.list_chat_messages(core, &s("chatId")).await),
            "chatCreate" => to_json(
                ws.create_chat(core, &replay, param::<ChatDraft>(&p, "chat"))
                    .await,
            ),
            "chatUpdate" => to_json(
                ws.update_chat(core, &replay, &s("id"), param::<ChatPatch>(&p, "patch"))
                    .await,
            ),
            "chatRemove" => to_json(ws.remove_chat(core, &replay, &s("id")).await),
            "chatMessagesPut" => to_json(
                ws.put_chat_messages(
                    core,
                    &replay,
                    &s("chatId"),
                    param::<Vec<ChatMessageDraft>>(&p, "messages"),
                )
                .await,
            ),
            "projectSidebarSet" => to_json(
                ws.set_project_sidebar(
                    core,
                    &replay,
                    &s("projectId"),
                    param::<Vec<String>>(&p, "connectionOrder"),
                )
                .await,
            ),
            // settings
            "settingGet" => to_json(ws.get_setting(&s("key")).await),
            "settingSet" => to_json(
                ws.set_setting(
                    core,
                    &replay,
                    &s("key"),
                    param::<Option<String>>(&p, "value"),
                )
                .await,
            ),
            "aiSettingsGet" => to_json(ws.get_ai_settings().await),
            "aiSettingsPatch" => to_json(
                ws.patch_ai_settings(core, &replay, param::<AiSettingsPatch>(&p, "patch"))
                    .await,
            ),
            "aiProviderCreate" => to_json(
                ws.create_ai_provider(
                    core,
                    &replay,
                    param::<AiProviderDraft>(&p, "provider"),
                    clearable(&p, "apiKey"),
                )
                .await,
            ),
            "aiProviderUpdate" => to_json(
                ws.update_ai_provider(
                    core,
                    &replay,
                    &s("id"),
                    param::<AiProviderPatch>(&p, "patch"),
                    clearable(&p, "apiKey"),
                )
                .await,
            ),
            "aiProviderRemove" => to_json(ws.remove_ai_provider(core, &replay, &s("id")).await),
            "themesGet" => to_json(ws.get_themes().await),
            "themePreferencesSet" => to_json(
                ws.set_theme_preferences(
                    core,
                    &replay,
                    ThemePreferences {
                        light_theme_id: s("lightThemeId"),
                        dark_theme_id: s("darkThemeId"),
                    },
                )
                .await,
            ),
            "userThemeCreate" => to_json(
                ws.create_user_theme(core, &replay, raw_param(&p, "theme"))
                    .await,
            ),
            "userThemeUpdate" => to_json(
                ws.update_user_theme(core, &replay, &s("id"), raw_param(&p, "theme"))
                    .await,
            ),
            "userThemeRemove" => to_json(ws.remove_user_theme(core, &replay, &s("id")).await),
            "onboardingGet" => to_json(ws.get_onboarding().await),
            "onboardingPatch" => to_json(
                ws.patch_onboarding(core, &replay, raw_param(&p, "patch"))
                    .await,
            ),
            "tutorialList" => to_json(ws.list_tutorial().await),
            "tutorialSave" => to_json(
                ws.save_tutorial(
                    core,
                    &replay,
                    TutorialProgress {
                        lesson_id: s("lessonId"),
                        challenge_id: s("challengeId"),
                        state: opt_param::<Option<String>>(&p, "state"),
                    },
                )
                .await,
            ),
            "tutorialRemoveLesson" => to_json(
                ws.remove_tutorial_lesson(core, &replay, &s("lessonId"))
                    .await,
            ),
            "tutorialReset" => to_json(ws.reset_tutorial(&replay).await),
            "importStateGet" => to_json(ws.get_import_state(&s("source")).await),
            "importStateSave" => to_json(
                ws.save_import_state(
                    core,
                    &replay,
                    &s("source"),
                    ImportState {
                        has_offered_import: param::<bool>(&p, "hasOfferedImport"),
                        last_check_timestamp: opt_param::<Option<String>>(&p, "lastCheckTimestamp"),
                    },
                )
                .await,
            ),
            other => panic!("unknown method {other}"),
        }
    }

    /// Binds a create's `binds` token to the id Core answered, after
    /// checking its prefix and uuid.
    fn bind(&mut self, call: &Value, answer: &Value) -> Result<(), String> {
        let Some(binds) = call["binds"].as_str() else {
            return Ok(());
        };
        let method = call["method"].as_str().unwrap();
        let id = match method {
            "dashboardUpdate" => answer["version"]["id"].as_str(),
            _ => answer["id"].as_str(),
        }
        .ok_or_else(|| format!("{method} answered no id to bind"))?;
        let tokens = tokens_in(binds);
        let prefix = &binds[..binds.find("<id:").unwrap()];
        let rest = id
            .strip_prefix(prefix)
            .ok_or_else(|| format!("{id} hasn't the prefix {prefix:?}"))?;
        if !is_uuid_v4(rest) {
            return Err(format!("{id} isn't {prefix:?} and a v4 uuid"));
        }
        self.bound.insert(tokens[0].clone(), rest.to_string());
        Ok(())
    }
}

fn load(file: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
    serde_json::from_str::<Vec<Value>>(&text).unwrap()
}

fn changes() -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/changes.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// Drops every key holding `null` or `[]`, at every level (the rule for a
/// `legacy` state).
fn drop_empty(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .filter(|(_, v)| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
                .map(|(k, v)| (k.clone(), drop_empty(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(drop_empty).collect()),
        other => other.clone(),
    }
}

/// Why a read's answer doesn't match its `expect`, or `None`.
fn check_expect(method: &str, expect: &Value, answer: &Value, cx: &Ctx) -> Option<String> {
    let ok = match method {
        "windowGet" => matches(expect, answer, cx),
        "windowStateLoad" => {
            let same = expect["copiedFrom"] == answer["copiedFrom"]
                && expect["rev"].as_u64() == answer["rev"].as_u64();
            let state = if expect["copiedFrom"] == "legacy" {
                matches(
                    &drop_empty(&expect["state"]),
                    &drop_empty(&answer["state"]),
                    cx,
                )
            } else {
                matches(&expect["state"], &answer["state"], cx)
            };
            same && state
        }
        "dashboardsList" => {
            let got: Vec<Value> = answer
                .as_array()
                .unwrap()
                .iter()
                .map(|d| json!({"id": d["id"], "starred": d["starred"]}))
                .collect();
            matches(&expect["dashboards"], &Value::Array(got), cx)
        }
        "dashboardVersionsList" | "workflowsList" => {
            let mut e: Vec<String> = expect["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| substitute(v.as_str().unwrap(), cx.bound))
                .collect();
            let mut a: Vec<String> = answer
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["id"].as_str().unwrap_or_default().to_string())
                .collect();
            e.sort();
            a.sort();
            e == a
        }
        "chatsList" => {
            let got: Vec<Value> = answer
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].clone())
                .collect();
            matches(&expect["ids"], &Value::Array(got), cx)
        }
        "chatMessagesList" => {
            let got: Vec<Value> = answer["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].clone())
                .collect();
            matches(&expect["ids"], &Value::Array(got), cx)
        }
        _ => true,
    };
    (!ok).then(|| format!("{method} answered {answer}, expected {expect}"))
}

/// The AI settings a step's view shows (`settings`, or `ai.settings` next
/// to the settings stores').
fn view_ai(view: &Value) -> Option<&Value> {
    if view["ai"]["settings"].is_object() {
        Some(&view["ai"]["settings"])
    } else if view["settings"]["providers"].is_array() {
        Some(&view["settings"])
    } else {
        None
    }
}

/// Why the step's loads (and the last answer of each settings record)
/// don't show what its view does, or what its rows hold.
fn check_loads(
    calls: &[(Value, Result<Value, CoreError>)],
    view: Option<&Value>,
    rows: &Value,
    cx: &Ctx,
) -> Vec<String> {
    let mut why = Vec::new();
    // A load is compared with the view when it is the step's last call of
    // that record (a write after it changes what the view shows).
    let last = |methods: &[&str], load: &str| {
        calls
            .iter()
            .rev()
            .find(|(c, _)| methods.contains(&c["method"].as_str().unwrap()))
            .filter(|(c, _)| c["method"] == load)
            .and_then(|(c, r)| r.as_ref().ok().map(|v| (c, v)))
    };
    if let Some(view) = view {
        if let Some((_, settings)) = last(
            &[
                "aiSettingsGet",
                "aiSettingsPatch",
                "aiProviderCreate",
                "aiProviderUpdate",
                "aiProviderRemove",
            ],
            "aiSettingsGet",
        ) {
            if let Some(expected) = view_ai(view) {
                if !matches(expected, settings, cx) {
                    why.push(format!("AI settings {settings}, view {expected}"));
                }
            }
        }
        if let Some((_, themes)) = last(
            &[
                "themesGet",
                "themePreferencesSet",
                "userThemeCreate",
                "userThemeUpdate",
                "userThemeRemove",
            ],
            "themesGet",
        ) {
            let ids: Vec<Value> = themes["userThemes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].clone())
                .collect();
            let want: Vec<Value> = view["userThemes"]
                .as_array()
                .map(|a| a.iter().map(|t| t["id"].clone()).collect())
                .unwrap_or_default();
            if !matches(&view["preferences"], &themes["preferences"], cx)
                || !matches(&Value::Array(want), &Value::Array(ids), cx)
            {
                why.push(format!("themes {themes}, view {view}"));
            }
        }
        if let Some((_, v)) = last(&["onboardingGet", "onboardingPatch"], "onboardingGet") {
            for (k, want) in view.as_object().unwrap() {
                if !matches(want, &v[k], cx) {
                    why.push(format!("onboarding.{k} {}, view {want}", v[k]));
                }
            }
        }
        for (c, r) in calls {
            if c["method"] == "importStateGet" {
                if let Ok(v) = r {
                    let source = c["params"]["source"].as_str().unwrap();
                    let got = v["hasOfferedImport"].as_bool().unwrap_or(false);
                    if view[source].as_bool() != Some(got) {
                        why.push(format!("importStateGet {source} {got}, view {view}"));
                    }
                }
            }
        }
    }
    for (c, r) in calls {
        let Ok(v) = r else { continue };
        match c["method"].as_str().unwrap() {
            "tutorialList" => {
                let e = rows["tutorial_progress"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let got: Vec<Value> = v
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| json!({"lesson_id": t["lessonId"], "challenge_id": t["challengeId"], "state": t["state"]}))
                    .collect();
                if !rows_match("tutorial_progress", "lesson_id, challenge_id", &e, &got, cx) {
                    why.push(format!("tutorialList {v}, rows {e:?}"));
                }
            }
            "settingGet" => {
                let key = c["params"]["key"].as_str().unwrap();
                let stored = rows["app_state"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["key"] == key)
                    .map_or(Value::Null, |r| r["value"].clone());
                if !matches(&stored, v, cx) {
                    why.push(format!("settingGet {key} {v}, rows {stored}"));
                }
            }
            _ => {}
        }
    }
    why
}

#[tokio::test(flavor = "multi_thread")]
async fn replays_every_fixture() {
    let changes = changes();
    let mut failures = Vec::new();
    let mut listed_seen = HashSet::new();
    let (mut cases, mut steps, mut calls_run) = (0, 0, 0);
    for file in FILES {
        for case in load(file) {
            cases += 1;
            let name = case["name"].as_str().unwrap().to_string();
            let mut r = open_case(&case).await;
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                steps += 1;
                let entry = changes
                    .get(&name)
                    .and_then(|c| c["expected"]["steps"].get(i.to_string()));
                if entry.is_some() {
                    listed_seen.insert(format!("{name}#{i}"));
                }
                let outcome = entry
                    .and_then(|e| e.get("outcome"))
                    .unwrap_or(&step["outcome"]);
                // A corrected entry may replace the step's calls (what a GUI on
                // Core sends where the recording followed today's path).
                let calls: Vec<Value> = entry
                    .and_then(|e| e.get("core"))
                    .unwrap_or(&step["core"])
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let refused_at = (outcome["ok"] == false).then(|| {
                    outcome["call"]
                        .as_u64()
                        .map_or(calls.len().saturating_sub(1), |c| c as usize)
                });
                let mut results = Vec::new();
                let mut why = Vec::new();
                for (ci, call) in calls.iter().enumerate() {
                    calls_run += 1;
                    let method = call["method"].as_str().unwrap();
                    let result = r.call(call).await;
                    match (&result, refused_at == Some(ci)) {
                        (Ok(v), false) => {
                            if let Err(e) = r.bind(call, v) {
                                why.push(e);
                            }
                        }
                        (Ok(_), true) => why.push(format!(
                            "call {ci} ({method}) succeeded, expected {outcome}"
                        )),
                        (Err(e), false) => why.push(format!(
                            "call {ci} ({method}) failed: {}: {}",
                            e.code, e.message
                        )),
                        (Err(e), true) => {
                            if outcome["code"].as_str() != Some(e.code.as_str()) {
                                why.push(format!("call {ci} ({method}) {} != {outcome}", e.code));
                            }
                            if let Some(t) = outcome["takenBy"].as_str() {
                                if e.taken_by.as_deref() != Some(t) {
                                    why.push(format!("takenBy {:?} != {t}", e.taken_by));
                                }
                            }
                            if call["group"] == "settings" && method.starts_with("setting") {
                                let key = call["params"]["key"].as_str().unwrap();
                                if !e.message.contains(key) {
                                    why.push(format!(
                                        "the refusal doesn't name {key}: {}",
                                        e.message
                                    ));
                                }
                            }
                        }
                    }
                    results.push((call.clone(), result));
                }
                let snap = snapshot(r.ws.storage(), &r.store).await;
                let rows = match entry.and_then(|e| e.get("rows")) {
                    Some(rows) => rows.clone(),
                    None => step["rows"].clone(),
                };
                let cx = Ctx {
                    bound: &r.bound,
                    started: &r.started,
                };
                for (call, result) in &results {
                    if let (Some(expect), Ok(v)) = (call.get("expect"), result) {
                        if let Some(w) =
                            check_expect(call["method"].as_str().unwrap(), expect, v, &cx)
                        {
                            why.push(w);
                        }
                    }
                }
                let view = match entry.and_then(|e| e.get("view")) {
                    Some(Value::Null) => None,
                    Some(v) => Some(v),
                    None => Some(&step["view"]),
                };
                why.extend(check_loads(&results, view, &rows, &cx));
                let mut unbound = Vec::new();
                for (table, order) in COMPARED {
                    let e = rows[table].as_array().cloned().unwrap_or_default();
                    collect_tokens(&Value::Array(e.clone()), &mut unbound);
                    let a = &snap.tables[table];
                    if !rows_match(table, order, &e, a, &cx) {
                        why.push(format!(
                            "{table}:\n  expected {}\n  actual   {}",
                            prepare(&Value::Array(e), &r.bound, "<now>"),
                            Value::Array(a.clone())
                        ));
                    }
                }
                unbound.retain(|t| !r.bound.contains_key(t));
                unbound.sort();
                unbound.dedup();
                if !unbound.is_empty() {
                    why.push(format!("{unbound:?} bound to no id"));
                }
                let secrets = entry
                    .and_then(|e| e.get("secretStore"))
                    .unwrap_or(&step["secretStore"]);
                let e: BTreeMap<String, String> = secrets
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| {
                                (substitute(k, &r.bound), v.as_str().unwrap().to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if e != snap.secrets {
                    why.push(format!(
                        "secretStore: expected {e:?}, actual {:?}",
                        snap.secrets
                    ));
                }
                // A listed step's outcome or rows must differ from the
                // recording.
                if let Some(entry) = entry {
                    let same_rows = entry.get("rows").is_none_or(|er| {
                        COMPARED.iter().all(|(t, o)| {
                            let a = er[*t].as_array().cloned().unwrap_or_default();
                            let b = step["rows"][*t].as_array().cloned().unwrap_or_default();
                            rows_match(t, o, &a, &b, &cx)
                        })
                    });
                    let same_outcome = entry.get("outcome").is_none_or(|o| *o == step["outcome"]);
                    if (entry.get("rows").is_some() || entry.get("outcome").is_some())
                        && same_rows
                        && same_outcome
                    {
                        why.push("listed in changes.json but matches the recording".into());
                    }
                }
                if !why.is_empty() {
                    failures.push(format!(
                        "{name} step {i}{}:\n{}",
                        if entry.is_some() { " (listed)" } else { "" },
                        why.join("\n")
                    ));
                }
            }
            r.ws.close().await;
        }
    }
    for (name, entry) in &changes {
        if name == "*" {
            continue;
        }
        for i in entry["expected"]["steps"].as_object().unwrap().keys() {
            if !listed_seen.contains(&format!("{name}#{i}")) {
                failures.push(format!(
                    "changes.json lists {name} step {i}, which wasn't replayed"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {steps} steps in {cases} cases differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    assert!(
        cases >= 112 && steps >= 377 && calls_run > 1000,
        "{cases} cases, {steps} steps, {calls_run} calls"
    );
}

// ── Core's own rules ──

async fn fx() -> Fx {
    fx_with(StateLimits::default(), true).await
}

fn none() -> WriteOrigin {
    WriteOrigin::none()
}

fn j<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

fn rv(v: Value) -> Box<RawValue> {
    RawValue::from_string(v.to_string()).unwrap()
}

fn dash(project: &str, name: &str) -> DashboardDraft {
    j(
        json!({"projectId": project, "name": name, "widgets": [], "viewport": {"x": 0, "y": 0, "zoom": 1}}),
    )
}

async fn rows(ws: &Workspace, table: &str, order: &str) -> Vec<Value> {
    dump(ws.storage(), table, order).await
}

// ── Dashboards ──

#[tokio::test]
async fn a_dashboard_draft_can_take_the_next_free_name_and_be_shared() {
    let f = fx().await;
    let first =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "New Dashboard"))
            .await
            .unwrap()
            .value;
    assert!(!first.shared);
    // Without it, a second one is refused.
    let e =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "new dashboard"))
            .await
            .unwrap_err();
    assert_eq!(e.code, "NAME_TAKEN");
    let renamed = |name: &str, shared: bool| -> DashboardDraft {
        j(json!({"projectId": "p1", "name": name, "widgets": [],
            "viewport": {"x": 0, "y": 0, "zoom": 1},
            "renameIfTaken": true, "shared": shared}))
    };
    let second =
        f.ws.create_dashboard(&f.core, &none(), renamed("New Dashboard", false))
            .await
            .unwrap()
            .value;
    assert_eq!(second.name, "New Dashboard (2)");
    let third =
        f.ws.create_dashboard(&f.core, &none(), renamed("NEW DASHBOARD", true))
            .await
            .unwrap()
            .value;
    assert_eq!(third.name, "NEW DASHBOARD (3)");
    assert!(third.shared, "stored shared in the one call");
    // A free name is kept as sent.
    let free =
        f.ws.create_dashboard(&f.core, &none(), renamed("Sales", false))
            .await
            .unwrap()
            .value;
    assert_eq!(free.name, "Sales");
}

#[tokio::test]
async fn a_dashboard_star_is_saved_without_touching_updated_at() {
    let f = fx().await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    let starred =
        f.ws.update_dashboard(&f.core, &none(), &d.id, j(json!({"starred": true})))
            .await
            .unwrap()
            .value
            .dashboard;
    assert!(starred.starred);
    assert_eq!(starred.updated_at, d.updated_at);
    let shared =
        f.ws.update_dashboard(&f.core, &none(), &d.id, j(json!({"shared": true})))
            .await
            .unwrap()
            .value
            .dashboard;
    assert!(shared.updated_at > d.updated_at, "sharing is an edit");
}

#[tokio::test]
async fn a_version_only_when_asked() {
    let f = fx().await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    let moved =
        f.ws.update_dashboard(
            &f.core,
            &none(),
            &d.id,
            j(json!({"viewport": {"x": 5, "y": 0, "zoom": 1}})),
        )
        .await
        .unwrap()
        .value;
    assert!(moved.version.is_none());
    let renamed =
        f.ws.update_dashboard(
            &f.core,
            &none(),
            &d.id,
            j(json!({"name": "Revenue", "captureVersion": true})),
        )
        .await
        .unwrap()
        .value;
    let v = renamed.version.unwrap();
    assert!(v.id.starts_with("dver-") && is_uuid_v4(&v.id[5..]));
    assert_eq!((v.version, v.widget_count), (1.0, Some(0)));
    // The snapshot is the stored dashboard before the change.
    let whole =
        f.ws.get_dashboard_version(&f.core, &d.id, &v.id)
            .await
            .unwrap()
            .value;
    assert_eq!(whole.snapshot.len() as u64, v.bytes);
    let snap: Value = serde_json::from_str(&whole.snapshot).unwrap();
    assert_eq!(snap["name"], "Sales");
    assert_eq!(snap["viewport"], json!({"x": 5, "y": 0, "zoom": 1}));
    assert_eq!(rows(&f.ws, "dashboard_versions", "version").await.len(), 1);
}

/// 5d-2 Task 7 probe fix: `dashboardVersionsList` and `workflowsList`
/// answered every body (294 MiB and 480 MiB at the probe's sizes). They
/// answer metadata only, and so does `dashboardUpdate`'s new version.
#[tokio::test]
async fn the_lists_carry_no_bodies() {
    let f = fx().await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    let pad = "w".repeat(100_000);
    let widgets = json!([{"id": "a", "sql": pad}, {"id": "b"}]);
    f.ws.update_dashboard(&f.core, &none(), &d.id, j(json!({"widgets": widgets})))
        .await
        .unwrap();
    let updated =
        f.ws.update_dashboard(
            &f.core,
            &none(),
            &d.id,
            j(json!({"name": "Revenue", "captureVersion": true})),
        )
        .await
        .unwrap()
        .value;
    let answer = serde_json::to_string(&updated).unwrap();
    assert!(
        !answer.contains("snapshot") && answer.matches(&pad).count() == 1,
        "only the dashboard's own widgets, not the version's snapshot"
    );
    let v = updated.version.unwrap();
    assert_eq!(v.widget_count, Some(2));
    assert!(v.bytes > 100_000);

    let versions =
        f.ws.list_dashboard_versions(&f.core, "p1")
            .await
            .unwrap()
            .value;
    let list = serde_json::to_value(&versions).unwrap();
    assert_eq!(
        list,
        json!([{"id": v.id, "dashboardId": d.id, "version": 1, "createdAt": v.created_at,
            "widgetCount": 2, "bytes": v.bytes}])
    );

    let rows_body = "r".repeat(200_000);
    let w =
        f.ws.create_workflow(
            &f.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: rv(json!({"name": "Flow", "nodes": [{"rows": rows_body}]})),
            },
        )
        .await
        .unwrap()
        .value;
    let w: Value = serde_json::from_str(w.get()).unwrap();
    let workflows = f.ws.list_workflows(&f.core, "p1").await.unwrap().value;
    let list = serde_json::to_value(&workflows).unwrap();
    assert_eq!(
        list,
        json!([{"id": w["id"], "projectId": "p1", "name": "Flow", "createdAt": w["createdAt"],
            "updatedAt": w["updatedAt"], "bytes": workflows[0].bytes}])
    );
    assert!(workflows[0].bytes > 200_000);
    assert!(f
        .ws
        .list_workflows(&f.core, "p2")
        .await
        .unwrap()
        .value
        .is_empty());
}

/// `dashboardVersionGet` and `workflowGet` answer one body whole. A
/// version is found only under its own dashboard (another project's
/// dashboard's version, named under this one, isn't); a missing workflow,
/// or one whose stored JSON doesn't read, is `WORKFLOW_NOT_FOUND`. They
/// are reads: no event.
#[tokio::test]
async fn one_version_or_workflow_is_fetched_whole() {
    let f = fx().await;
    let mut events = f.ws.events();
    let version_of = |d: &str, name: &str| {
        let f = &f;
        let d = d.to_string();
        let name = name.to_string();
        async move {
            f.ws.update_dashboard(
                &f.core,
                &none(),
                &d,
                j(json!({"name": name, "captureVersion": true})),
            )
            .await
            .unwrap()
            .value
            .version
            .unwrap()
        }
    };
    let d1 =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    let d2 =
        f.ws.create_dashboard(&f.core, &none(), dash("p2", "Elsewhere"))
            .await
            .unwrap()
            .value;
    let v1 = version_of(&d1.id, "Revenue").await;
    let v2 = version_of(&d2.id, "Other").await;
    let body = r#"{"name":"Flow","nodes":[{"n":1.50}]}"#;
    let w =
        f.ws.create_workflow(
            &f.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: RawValue::from_string(body.into()).unwrap(),
            },
        )
        .await
        .unwrap()
        .value;
    let wid = serde_json::from_str::<Value>(w.get()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    insert_rows(
        f.ws.storage(),
        "saved_canvases",
        &[json!({"id": "bad", "project_id": "p1", "data": "not json"})],
    )
    .await;
    while tokio::time::timeout(std::time::Duration::from_millis(50), events.next())
        .await
        .is_ok_and(|e| e.is_some())
    {}

    let got =
        f.ws.get_dashboard_version(&f.core, &d1.id, &v1.id)
            .await
            .unwrap()
            .value;
    assert_eq!((got.id.as_str(), got.version), (v1.id.as_str(), 1.0));
    assert_eq!(
        serde_json::from_str::<Value>(&got.snapshot).unwrap()["name"],
        "Sales"
    );
    for (dashboard, version, code) in [
        (
            d1.id.as_str(),
            v2.id.as_str(),
            "DASHBOARD_VERSION_NOT_FOUND",
        ),
        (
            d2.id.as_str(),
            v1.id.as_str(),
            "DASHBOARD_VERSION_NOT_FOUND",
        ),
        (d1.id.as_str(), "dver-none", "DASHBOARD_VERSION_NOT_FOUND"),
        ("dashboard-none", v1.id.as_str(), "DASHBOARD_NOT_FOUND"),
        ("", v1.id.as_str(), "DASHBOARD_NOT_FOUND"),
        (d1.id.as_str(), "", "DASHBOARD_VERSION_NOT_FOUND"),
        (d1.id.as_str(), "x\0", "INVALID_ARGUMENT"),
    ] {
        let e =
            f.ws.get_dashboard_version(&f.core, dashboard, version)
                .await
                .unwrap_err();
        assert_eq!(e.code, code, "{dashboard} {version}");
    }

    let got = f.ws.get_workflow(&f.core, &wid).await.unwrap().value;
    assert_eq!(got.get(), w.get(), "byte for byte");
    for (id, code) in [
        ("workflow-none", "WORKFLOW_NOT_FOUND"),
        ("bad", "WORKFLOW_NOT_FOUND"),
        ("", "WORKFLOW_NOT_FOUND"),
        ("x\0", "INVALID_ARGUMENT"),
    ] {
        assert_eq!(
            f.ws.get_workflow(&f.core, id).await.unwrap_err().code,
            code,
            "{id}"
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), events.next())
            .await
            .is_err(),
        "reads emit nothing"
    );
}

#[tokio::test]
async fn dashboard_limit_zero_keeps_all() {
    let f = fx().await;
    f.ws.set_setting(
        &f.core,
        &none(),
        "dashboard_version_limit",
        Some("0".into()),
    )
    .await
    .unwrap();
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    for n in 0..5 {
        let u =
            f.ws.update_dashboard(
                &f.core,
                &none(),
                &d.id,
                j(json!({"name": format!("S{n}"), "captureVersion": true})),
            )
            .await
            .unwrap()
            .value;
        assert!(u.pruned_version_ids.is_empty());
    }
    let versions: Vec<f64> = rows(&f.ws, "dashboard_versions", "version")
        .await
        .iter()
        .map(|r| r["version"].as_f64().unwrap())
        .collect();
    assert_eq!(versions, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    // A limit of 2 then keeps the newest two.
    f.ws.set_setting(
        &f.core,
        &none(),
        "dashboard_version_limit",
        Some("2".into()),
    )
    .await
    .unwrap();
    let u =
        f.ws.update_dashboard(
            &f.core,
            &none(),
            &d.id,
            j(json!({"name": "S9", "captureVersion": true})),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(u.pruned_version_ids.len(), 4);
    assert_eq!(rows(&f.ws, "dashboard_versions", "version").await.len(), 2);
}

#[tokio::test]
async fn dashboard_versions_keep_within_the_web_byte_budget() {
    let limits = StateLimits {
        max_dashboard_version_bytes: Some(3000),
        ..StateLimits::default()
    };
    let f = fx_with(limits, true).await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    let big = "x".repeat(1000);
    for n in 0..6 {
        f.ws.update_dashboard(
            &f.core,
            &none(),
            &d.id,
            j(json!({"description": format!("{big}{n}"), "captureVersion": true})),
        )
        .await
        .unwrap();
    }
    // Each snapshot is over 1,000 bytes: two fit in 3,000.
    assert_eq!(rows(&f.ws, "dashboard_versions", "version").await.len(), 2);
}

#[tokio::test]
async fn an_update_of_a_removed_dashboard_is_not_found() {
    let f = fx().await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Sales"))
            .await
            .unwrap()
            .value;
    f.ws.remove_dashboard(&f.core, &none(), &d.id)
        .await
        .unwrap();
    let e =
        f.ws.update_dashboard(&f.core, &none(), &d.id, j(json!({"viewport": {"x": 1}})))
            .await
            .unwrap_err();
    assert_eq!(e.code, "DASHBOARD_NOT_FOUND");
    assert!(
        rows(&f.ws, "dashboards", "id").await.is_empty(),
        "not re-inserted"
    );
    let e =
        f.ws.remove_dashboard(&f.core, &none(), &d.id)
            .await
            .unwrap_err();
    assert_eq!(e.code, "DASHBOARD_NOT_FOUND");
}

#[tokio::test]
async fn dashboard_names_clash_within_a_project() {
    let f = fx().await;
    let a =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Café"))
            .await
            .unwrap()
            .value;
    let e =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", " CAFE\u{301} "))
            .await
            .unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.taken_by.as_deref()),
        ("NAME_TAKEN", Some(a.id.as_str()))
    );
    // Another project may have it.
    f.ws.create_dashboard(&f.core, &none(), dash("p2", "Café"))
        .await
        .unwrap();
    // A rename to its own name in another case isn't a clash.
    f.ws.update_dashboard(&f.core, &none(), &a.id, j(json!({"name": "CAFÉ"})))
        .await
        .unwrap();
    let b =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "Other"))
            .await
            .unwrap()
            .value;
    let e =
        f.ws.update_dashboard(&f.core, &none(), &b.id, j(json!({"name": "café"})))
            .await
            .unwrap_err();
    assert_eq!(e.code, "NAME_TAKEN");
    let e =
        f.ws.create_dashboard(&f.core, &none(), dash("nope", "X"))
            .await
            .unwrap_err();
    assert_eq!(e.code, "PROJECT_NOT_FOUND");
}

// ── Workflows ──

#[tokio::test]
async fn a_workflow_gets_core_id_and_times_and_keeps_the_rest_byte_for_byte() {
    let f = fx().await;
    let body = r#"{"id":"gui-id","name":"Flow","nodes":[{"n":1.50,"big":12345678901234567890}],"projectId":"p9","createdAt":"x","future":{"a" : 1}}"#;
    let w =
        f.ws.create_workflow(
            &f.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: RawValue::from_string(body.into()).unwrap(),
            },
        )
        .await
        .unwrap()
        .value;
    let text = w.get();
    assert!(
        text.contains(r#""nodes":[{"n":1.50,"big":12345678901234567890}]"#),
        "{text}"
    );
    assert!(text.contains(r#""future":{"a" : 1}"#), "{text}");
    let v: Value = serde_json::from_str(text).unwrap();
    let id = v["id"].as_str().unwrap();
    assert!(id.starts_with("workflow-") && is_uuid_v4(&id[9..]));
    assert_eq!(v["projectId"], "p1");
    assert_eq!(v["createdAt"], v["updatedAt"]);
    let stored = rows(&f.ws, "saved_canvases", "id").await;
    assert_eq!(
        (stored[0]["id"].as_str(), stored[0]["data"].as_str()),
        (Some(id), Some(text))
    );

    f.clock.advance(5);
    let u =
        f.ws.update_workflow(
            &f.core,
            &none(),
            id,
            rv(json!({"name": "Renamed", "nodes": []})),
        )
        .await
        .unwrap()
        .value;
    let u: Value = serde_json::from_str(u.get()).unwrap();
    assert_eq!(
        (&u["id"], &u["projectId"], &u["createdAt"], &u["name"]),
        (
            &v["id"],
            &v["projectId"],
            &v["createdAt"],
            &json!("Renamed")
        )
    );
    assert!(u["updatedAt"].as_str() > v["updatedAt"].as_str());
    let e =
        f.ws.update_workflow(&f.core, &none(), "workflow-none", rv(json!({"name": "x"})))
            .await
            .unwrap_err();
    assert_eq!(e.code, "WORKFLOW_NOT_FOUND");
    f.ws.remove_workflow(&f.core, &none(), id).await.unwrap();
    assert_eq!(
        f.ws.remove_workflow(&f.core, &none(), id)
            .await
            .unwrap_err()
            .code,
        "WORKFLOW_NOT_FOUND"
    );
}

/// A rename is Core's (`workflowRename`), done on the
/// stored row inside the write, so a save another window made between two
/// renames here isn't lost (the GUI used to read the body, then write it
/// back whole). One event per rename; a missing workflow is not found; the
/// web's name limit applies.
#[tokio::test]
async fn a_rename_keeps_a_save_that_landed_before_it() {
    let f = fx().await;
    let mut events = f.ws.events();
    let w =
        f.ws.create_workflow(
            &f.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: rv(json!({"name": "Flow", "nodes": [1]})),
            },
        )
        .await
        .unwrap()
        .value;
    let id = serde_json::from_str::<Value>(w.get()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.ws.rename_workflow(&f.core, &none(), &id, "First")
        .await
        .unwrap();
    // Another window saves new nodes (and its own idea of the name).
    f.ws.update_workflow(
        &f.core,
        &none(),
        &id,
        rv(json!({"name": "First", "nodes": [1, 2, 3]})),
    )
    .await
    .unwrap();
    let renamed =
        f.ws.rename_workflow(&f.core, &none(), &id, "Second")
            .await
            .unwrap()
            .value;
    assert_eq!(
        (renamed.id.as_str(), renamed.name.as_str()),
        (id.as_str(), "Second")
    );
    let stored: Value =
        serde_json::from_str(f.ws.get_workflow(&f.core, &id).await.unwrap().value.get()).unwrap();
    assert_eq!(
        stored["nodes"],
        json!([1, 2, 3]),
        "the other window's save stays"
    );
    assert_eq!(stored["name"], "Second");
    assert_eq!(renamed.updated_at.as_deref(), stored["updatedAt"].as_str());
    let mut renames = 0;
    while let Ok(Some(e)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), events.next()).await
    {
        if let WorkspaceEvent::StorageChanged(c) = e {
            assert_eq!(c.kind, StoredKind::Workflow);
            renames += 1;
        }
    }
    assert_eq!(renames, 4, "create, rename, update, rename: one event each");
    assert_eq!(
        f.ws.rename_workflow(&f.core, &none(), "workflow-none", "x")
            .await
            .unwrap_err()
            .code,
        "WORKFLOW_NOT_FOUND"
    );
    assert_eq!(
        f.ws.rename_workflow(&f.core, &none(), &id, "  ")
            .await
            .unwrap_err()
            .code,
        "INVALID_ARGUMENT"
    );
    let web = fx_with(web_state_limits(), false).await;
    let w = web
        .ws
        .create_workflow(
            &web.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: rv(json!({"name": "Flow"})),
            },
        )
        .await
        .unwrap()
        .value;
    let wid = serde_json::from_str::<Value>(w.get()).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = web
        .ws
        .rename_workflow(&web.core, &none(), &wid, &"n".repeat(2000))
        .await
        .unwrap_err();
    assert!(e.message.contains("max_name_bytes"), "{e:?}");
}

/// Opening a workspace fills the list metadata an
/// older release's writes left out (its replace-all workflow save).
#[tokio::test]
async fn opening_a_workspace_refills_list_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let core = state_core(TickClock::new(), StateLimits::default());
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    insert_rows(
        ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Main", "description": null, "created_at": T0,
            "updated_at": T0, "git_repo_path": null}),
        ],
    )
    .await;
    insert_rows(
        ws.storage(),
        "saved_canvases",
        &[json!({"id": "w1", "project_id": "p1", "data": "{\"name\":\"Old\"}"})],
    )
    .await;
    ws.close_all(&core).await;
    drop(ws);
    let core = state_core(TickClock::new(), StateLimits::default());
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let rows = dump(ws.storage(), "saved_canvases", "id").await;
    assert_eq!(
        rows[0]["meta"],
        r#"{"name":"Old","createdAt":null,"updatedAt":null}"#
    );
}

#[tokio::test]
async fn project_state_writes_leave_workflows_alone() {
    let f = fx().await;
    f.ws.create_workflow(
        &f.core,
        &none(),
        j(json!({"projectId": "p1", "workflow": {"name": "Flow"}})),
    )
    .await
    .unwrap();
    let before = rows(&f.ws, "saved_canvases", "id").await;
    let main = WriteOrigin::new(Some("main"));
    f.ws.save_window_state(&f.core, &main, "main", "p1", 1, rv(common_state("p1")))
        .await
        .unwrap();
    f.ws.set_project_sidebar(&f.core, &none(), "p1", vec!["c1".into()])
        .await
        .unwrap();
    assert_eq!(rows(&f.ws, "saved_canvases", "id").await, before);
}

/// A minimal view state of `project`.
fn common_state(project: &str) -> Value {
    json!({
        "projectId": project, "queryTabs": [{"id": "t1", "name": "Q", "query": "SELECT 1"}],
        "schemaTabs": [], "explainTabs": [], "erdTabs": [], "tabOrder": ["t1"],
        "activeQueryTabId": "t1", "activeSchemaTabId": null, "activeExplainTabId": null,
        "activeErdTabId": null, "activeView": "query", "activeConnectionId": "c1"
    })
}

#[tokio::test]
async fn a_workflow_past_16_mib_is_refused_on_web() {
    let f = fx_with(web_state_limits(), false).await;
    let rows_json = format!("[{}]", vec!["1"; 9 * 1024 * 1024].join(","));
    let body = format!(r#"{{"name":"Big","nodes":{rows_json}}}"#);
    assert!(body.len() > 16 * 1024 * 1024);
    let e =
        f.ws.create_workflow(
            &f.core,
            &none(),
            WorkflowDraft {
                project_id: "p1".into(),
                workflow: RawValue::from_string(body).unwrap(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(e.message.contains("max_workflow_bytes"), "{e:?}");
    assert!(rows(&f.ws, "saved_canvases", "id").await.is_empty());
}

#[tokio::test]
async fn desktop_has_no_workflow_cap() {
    let f = fx().await;
    let rows_json = format!("[{}]", vec!["1"; 9 * 1024 * 1024].join(","));
    let body = format!(r#"{{"name":"Big","nodes":{rows_json}}}"#);
    f.ws.create_workflow(
        &f.core,
        &none(),
        WorkflowDraft {
            project_id: "p1".into(),
            workflow: RawValue::from_string(body).unwrap(),
        },
    )
    .await
    .unwrap();
}

// ── Chats ──

fn msg(id: &str, role: &str, content: &str, ts: &str) -> ChatMessageDraft {
    j(json!({"id": id, "role": role, "content": content, "timestamp": ts}))
}

async fn chat(f: &Fx) -> String {
    f.ws.create_chat(
        &f.core,
        &none(),
        j(json!({"connectionId": "c1", "title": "T"})),
    )
    .await
    .unwrap()
    .value
    .id
}

#[tokio::test]
async fn messages_are_upserted_not_replaced() {
    let f = fx().await;
    let c = chat(&f).await;
    assert!(is_uuid_v4(&c), "{c}");
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "user", "q", T0), msg("m2", "assistant", "…", T0)],
    )
    .await
    .unwrap();
    // A second window's turn names only its messages; the first stay.
    f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m3", "user", "q2", T0)])
        .await
        .unwrap();
    // A changed message keeps its place (same timestamp, first stored).
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m2", "assistant", "answer", T0)],
    )
    .await
    .unwrap();
    let listed = f.ws.list_chat_messages(&f.core, &c).await.unwrap().value;
    let ids: Vec<&str> = listed.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["m1", "m2", "m3"]);
    assert_eq!(listed.messages[1].content, "answer");
    f.ws.remove_chat_messages(&f.core, &none(), &c, vec!["m3".into()])
        .await
        .unwrap();
    assert_eq!(
        f.ws.list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .messages
            .len(),
        2
    );
}

#[tokio::test]
async fn a_message_id_of_another_chat_is_refused() {
    let f = fx().await;
    let a = chat(&f).await;
    let b = chat(&f).await;
    f.ws.put_chat_messages(&f.core, &none(), &a, vec![msg("m1", "user", "q", T0)])
        .await
        .unwrap();
    let e =
        f.ws.put_chat_messages(
            &f.core,
            &none(),
            &b,
            vec![msg("m2", "user", "x", T0), msg("m1", "user", "stolen", T0)],
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert_eq!(
        f.ws.list_chat_messages(&f.core, &b)
            .await
            .unwrap()
            .value
            .messages
            .len(),
        0
    );
    assert_eq!(
        f.ws.list_chat_messages(&f.core, &a)
            .await
            .unwrap()
            .value
            .messages[0]
            .content,
        "q"
    );
    for bad in [msg("a b", "user", "x", T0), msg("m", "system", "x", T0)] {
        let e =
            f.ws.put_chat_messages(&f.core, &none(), &a, vec![bad])
                .await
                .unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT");
    }
    let e =
        f.ws.put_chat_messages(
            &f.core,
            &none(),
            "no-chat",
            vec![msg("m9", "user", "x", T0)],
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "CHAT_NOT_FOUND");
}

#[tokio::test]
async fn removing_a_chat_takes_its_messages() {
    let f = fx().await;
    let c = chat(&f).await;
    f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m1", "user", "q", T0)])
        .await
        .unwrap();
    f.ws.remove_chat(&f.core, &none(), &c).await.unwrap();
    assert!(rows(&f.ws, "ai_messages", "id").await.is_empty());
    assert_eq!(
        f.ws.remove_chat(&f.core, &none(), &c)
            .await
            .unwrap_err()
            .code,
        "CHAT_NOT_FOUND"
    );
    let e =
        f.ws.create_chat(
            &f.core,
            &none(),
            j(json!({"connectionId": "nope", "title": ""})),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "CONNECTION_NOT_FOUND");
}

#[tokio::test]
async fn a_put_past_the_chat_budget_is_refused_and_stores_nothing() {
    let limits = StateLimits {
        max_chat_bytes: Some(100),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let c = chat(&f).await;
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "user", &"a".repeat(60), T0)],
    )
    .await
    .unwrap();
    let e =
        f.ws.put_chat_messages(
            &f.core,
            &none(),
            &c,
            vec![
                msg("m2", "user", "b", T0),
                msg("m3", "assistant", &"c".repeat(40), T0),
            ],
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT");
    assert!(
        e.message.contains("max_chat_bytes") && e.message.contains("new chat"),
        "{e:?}"
    );
    let listed = f.ws.list_chat_messages(&f.core, &c).await.unwrap().value;
    assert_eq!((listed.messages.len(), listed.stored_bytes), (1, 60));
    // Exactly at the budget is fine.
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m2", "user", &"b".repeat(40), T0)],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn replacing_a_message_counts_its_new_size_not_both() {
    let limits = StateLimits {
        max_chat_bytes: Some(100),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let c = chat(&f).await;
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "assistant", &"a".repeat(90), T0)],
    )
    .await
    .unwrap();
    // 90 replaced by 95: 95 in all, within 100 (90 + 95 would not be).
    let put =
        f.ws.put_chat_messages(
            &f.core,
            &none(),
            &c,
            vec![msg("m1", "assistant", &"a".repeat(95), T0)],
        )
        .await
        .unwrap()
        .value;
    assert_eq!(put.stored_bytes, 95);
}

#[tokio::test]
async fn desktop_has_no_chat_budget() {
    let f = fx().await;
    let c = chat(&f).await;
    let big = "x".repeat(2 * 1024 * 1024);
    let msgs: Vec<ChatMessageDraft> = (0..40)
        .map(|i| msg(&format!("m{i}"), "user", &big, T0))
        .collect();
    f.ws.put_chat_messages(&f.core, &none(), &c, msgs)
        .await
        .unwrap();
    assert_eq!(
        f.ws.list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .stored_bytes,
        80 * 1024 * 1024
    );
}

#[tokio::test]
async fn chat_messages_list_answers_the_stored_bytes() {
    let f = fx().await;
    let c = chat(&f).await;
    f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m1", "user", "héllo", T0)])
        .await
        .unwrap();
    let listed = f.ws.list_chat_messages(&f.core, &c).await.unwrap().value;
    assert_eq!(listed.stored_bytes, "héllo".len() as u64);
    let none_chat =
        f.ws.list_chat_messages(&f.core, "missing")
            .await
            .unwrap()
            .value;
    assert_eq!((none_chat.messages.len(), none_chat.stored_bytes), (0, 0));
}

#[tokio::test]
async fn chat_messages_list_says_when_a_chat_is_full() {
    // Bytes: one more message of the largest size could pass the budget.
    let limits = StateLimits {
        max_chat_bytes: Some(100),
        max_message_bytes: Some(30),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let c = chat(&f).await;
    let put =
        f.ws.put_chat_messages(
            &f.core,
            &none(),
            &c,
            vec![
                msg("m1", "user", &"a".repeat(30), T0),
                msg("m2", "assistant", &"a".repeat(30), T0),
                msg("m3", "user", &"a".repeat(10), T0),
            ],
        )
        .await
        .unwrap()
        .value;
    assert!(!put.full, "70 + 30 fits in 100");
    assert!(
        !f.ws
            .list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .full
    );
    let put =
        f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m4", "user", "b", T0)])
            .await
            .unwrap()
            .value;
    assert!(put.full, "71 + 30 doesn't");
    assert!(
        f.ws.list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .full
    );

    // Count: at `max_messages_per_chat` it's full too.
    let limits = StateLimits {
        max_messages_per_chat: Some(2),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let c = chat(&f).await;
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "user", "a", T0), msg("m2", "assistant", "b", T0)],
    )
    .await
    .unwrap();
    assert!(
        f.ws.list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .full
    );
}

#[tokio::test]
async fn a_desktop_chat_is_never_full() {
    let f = fx().await;
    let c = chat(&f).await;
    let big = "x".repeat(4 * 1024 * 1024);
    let msgs: Vec<ChatMessageDraft> = (0..20)
        .map(|i| msg(&format!("m{i}"), "user", &big, T0))
        .collect();
    let put =
        f.ws.put_chat_messages(&f.core, &none(), &c, msgs)
            .await
            .unwrap()
            .value;
    assert!(!put.full);
    assert!(
        !f.ws
            .list_chat_messages(&f.core, &c)
            .await
            .unwrap()
            .value
            .full
    );
}

#[tokio::test]
async fn a_chat_title_alone_keeps_updated_at() {
    let f = fx().await;
    let c =
        f.ws.create_chat(
            &f.core,
            &none(),
            j(json!({"connectionId": "c1", "title": "A"})),
        )
        .await
        .unwrap()
        .value;
    let t =
        f.ws.update_chat(&f.core, &none(), &c.id, j(json!({"title": "B"})))
            .await
            .unwrap()
            .value;
    assert_eq!(
        (t.title.as_str(), t.updated_at.as_str()),
        ("B", c.updated_at.as_str())
    );
    let t =
        f.ws.update_chat(&f.core, &none(), &c.id, j(json!({"touched": true})))
            .await
            .unwrap()
            .value;
    assert!(t.updated_at > c.updated_at);
}

// ── Limits ──

#[tokio::test]
async fn limits_refuse_before_anything_is_read() {
    let f = fx_with(web_state_limits(), false).await;
    // The storage is closed: a call that read would fail with a storage
    // error, so INVALID_ARGUMENT shows the check ran first.
    f.ws.close().await;
    let big = "x".repeat(5 * 1024 * 1024);
    let cases: Vec<(&str, Result<(), CoreError>)> = vec![
        (
            "max_dashboard_bytes",
            f.ws.create_dashboard(
                &f.core,
                &none(),
                j(json!({"projectId": "p1", "name": "D", "widgets": [big], "viewport": {}})),
            )
            .await
            .map(|_| ()),
        ),
        (
            "max_message_bytes",
            f.ws.put_chat_messages(&f.core, &none(), "c", vec![msg("m", "user", &big, T0)])
                .await
                .map(|_| ()),
        ),
        (
            "max_messages_per_chat",
            f.ws.put_chat_messages(
                &f.core,
                &none(),
                "c",
                (0..5001)
                    .map(|i| msg(&format!("m{i}"), "user", "x", T0))
                    .collect(),
            )
            .await
            .map(|_| ()),
        ),
        (
            "max_setting_bytes",
            f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "T", "x": big})))
                .await
                .map(|_| ()),
        ),
        (
            "max_setting_bytes",
            f.ws.set_setting(
                &f.core,
                &none(),
                "skippedUpdateVersion",
                Some("x".repeat(300 * 1024)),
            )
            .await
            .map(|_| ()),
        ),
        (
            "max_name_bytes",
            f.ws.create_dashboard(&f.core, &none(), dash("p1", &"n".repeat(2000)))
                .await
                .map(|_| ()),
        ),
    ];
    for (limit, r) in cases {
        let e = r.unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{limit}: {e:?}");
        assert!(e.message.contains(limit), "{limit}: {}", e.message);
    }
    // A view state past a limit reads the window's own row first (a state
    // stored before the limits stays saveable); with none, it's refused,
    // and nothing is written.
    let f = fx_with(web_state_limits(), false).await;
    let cases: Vec<(&str, Result<(), CoreError>)> = vec![
        (
            "max_view_state_bytes",
            f.ws.save_window_state(
                &f.core,
                &WriteOrigin::new(Some("w")),
                "w",
                "p1",
                1,
                rv({
                    let mut s = common_state("p1");
                    s["pad"] = json!("x".repeat(9 * 1024 * 1024));
                    s
                }),
            )
            .await
            .map(|_| ()),
        ),
        (
            "max_tab_text_bytes",
            f.ws.save_window_state(
                &f.core,
                &WriteOrigin::new(Some("w")),
                "w",
                "p1",
                1,
                rv({
                    let mut s = common_state("p1");
                    s["queryTabs"][0]["query"] = json!("x".repeat(3 * 1024 * 1024));
                    s
                }),
            )
            .await
            .map(|_| ()),
        ),
        (
            "max_tabs",
            f.ws.save_window_state(
                &f.core,
                &WriteOrigin::new(Some("w")),
                "w",
                "p1",
                1,
                rv({
                    let mut s = common_state("p1");
                    s["queryTabs"] = Value::Array(
                        (0..501)
                            .map(|i| json!({"id": format!("t{i}"), "name": "Q", "query": ""}))
                            .collect(),
                    );
                    s
                }),
            )
            .await
            .map(|_| ()),
        ),
    ];
    for (limit, r) in cases {
        let e = r.unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{limit}: {e:?}");
        assert!(e.message.contains(limit), "{limit}: {}", e.message);
    }
    assert!(rows(&f.ws, "window_state", "window_id").await.is_empty());
}

#[tokio::test]
async fn counts_are_checked_inside_the_transaction() {
    let limits = StateLimits {
        max_dashboards: Some(2),
        max_workflows: Some(1),
        max_chats: Some(1),
        max_user_themes: Some(1),
        max_ai_providers: Some(1),
        max_messages_per_chat: Some(2),
        ..StateLimits::default()
    };
    let f = fx_with(limits, true).await;
    f.ws.create_dashboard(&f.core, &none(), dash("p1", "A"))
        .await
        .unwrap();
    f.ws.create_dashboard(&f.core, &none(), dash("p2", "B"))
        .await
        .unwrap();
    let refused = |e: CoreError, name: &str| {
        assert_eq!(e.code, "INVALID_ARGUMENT", "{name}");
        assert!(e.message.contains(name), "{}", e.message);
    };
    refused(
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "C"))
            .await
            .unwrap_err(),
        "max_dashboards",
    );
    f.ws.create_workflow(
        &f.core,
        &none(),
        j(json!({"projectId": "p1", "workflow": {"name": "W"}})),
    )
    .await
    .unwrap();
    refused(
        f.ws.create_workflow(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "workflow": {"name": "W"}})),
        )
        .await
        .unwrap_err(),
        "max_workflows",
    );
    let c = chat(&f).await;
    refused(
        f.ws.create_chat(
            &f.core,
            &none(),
            j(json!({"connectionId": "c1", "title": ""})),
        )
        .await
        .unwrap_err(),
        "max_chats",
    );
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "user", "a", T0), msg("m2", "user", "b", T0)],
    )
    .await
    .unwrap();
    // Replacing is fine; a third message isn't.
    f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m2", "user", "c", T0)])
        .await
        .unwrap();
    refused(
        f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m3", "user", "d", T0)])
            .await
            .unwrap_err(),
        "max_messages_per_chat",
    );
    f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "T"})))
        .await
        .unwrap();
    refused(
        f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "U"})))
            .await
            .unwrap_err(),
        "max_user_themes",
    );
    f.ws.create_ai_provider(
        &f.core,
        &none(),
        j(json!({"name": "A", "type": "anthropic"})),
        None,
    )
    .await
    .unwrap();
    refused(
        f.ws.create_ai_provider(
            &f.core,
            &none(),
            j(json!({"name": "B", "type": "anthropic"})),
            Some(Some("k".into())),
        )
        .await
        .unwrap_err(),
        "max_ai_providers",
    );
    assert!(
        f.store.entries().is_empty(),
        "a refused provider never touches the keychain"
    );
}

// ── Events ──

async fn next_changes(
    events: &mut futures::stream::BoxStream<'static, WorkspaceEvent>,
) -> Vec<seaquel_core::StorageChange> {
    let mut out = Vec::new();
    while let Ok(Some(e)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), events.next()).await
    {
        if let WorkspaceEvent::StorageChanged(c) = e {
            out.push(c);
        }
    }
    out
}

#[tokio::test]
async fn every_state_write_emits_one_event_after_commit() {
    let f = fx().await;
    let mut events = f.ws.events();
    let o = WriteOrigin::new(Some("tab-a"));
    let main = WriteOrigin::new(Some("main"));
    let mut expect: Vec<(StoredKind, Option<String>)> = Vec::new();
    let d =
        f.ws.create_dashboard(&f.core, &o, dash("p1", "D"))
            .await
            .unwrap();
    expect.push((StoredKind::Dashboard, Some("p1".into())));
    f.ws.update_dashboard(
        &f.core,
        &o,
        &d.value.id,
        j(json!({"name": "E", "captureVersion": true})),
    )
    .await
    .unwrap();
    expect.push((StoredKind::Dashboard, Some("p1".into())));
    let w =
        f.ws.create_workflow(
            &f.core,
            &o,
            j(json!({"projectId": "p1", "workflow": {"name": "W"}})),
        )
        .await
        .unwrap();
    expect.push((StoredKind::Workflow, Some("p1".into())));
    let wid: Value = serde_json::from_str(w.value.get()).unwrap();
    f.ws.remove_workflow(&f.core, &o, wid["id"].as_str().unwrap())
        .await
        .unwrap();
    expect.push((StoredKind::Workflow, Some("p1".into())));
    let c =
        f.ws.create_chat(&f.core, &o, j(json!({"connectionId": "c1", "title": "T"})))
            .await
            .unwrap();
    expect.push((StoredKind::Chat, Some("c1".into())));
    f.ws.put_chat_messages(&f.core, &o, &c.value.id, vec![msg("m1", "user", "q", T0)])
        .await
        .unwrap();
    expect.push((StoredKind::ChatMessages, Some(c.value.id.clone())));
    f.ws.set_setting(&f.core, &o, "editorKeybindingMode", Some("vim".into()))
        .await
        .unwrap();
    expect.push((StoredKind::Setting, None));
    f.ws.patch_ai_settings(&f.core, &o, j(json!({"enabled": false})))
        .await
        .unwrap();
    expect.push((StoredKind::AiSettings, None));
    f.ws.create_user_theme(&f.core, &o, rv(json!({"name": "T"})))
        .await
        .unwrap();
    expect.push((StoredKind::Theme, None));
    f.ws.patch_onboarding(&f.core, &o, rv(json!({"learnEnabled": false})))
        .await
        .unwrap();
    expect.push((StoredKind::Onboarding, None));
    f.ws.save_tutorial(
        &f.core,
        &o,
        j(json!({"lessonId": "l", "challengeId": "c", "state": null})),
    )
    .await
    .unwrap();
    expect.push((StoredKind::Tutorial, None));
    f.ws.save_import_state(
        &f.core,
        &o,
        "dbeaver",
        j(json!({"hasOfferedImport": true, "lastCheckTimestamp": null})),
    )
    .await
    .unwrap();
    expect.push((StoredKind::ImportState, None));
    f.ws.set_project_sidebar(&f.core, &o, "p1", vec!["c1".into()])
        .await
        .unwrap();
    expect.push((StoredKind::Project, None));
    f.ws.save_window_state(&f.core, &main, "main", "p1", 1, rv(common_state("p1")))
        .await
        .unwrap();
    expect.push((StoredKind::ProjectState, Some("p1".into())));
    f.ws.activate_window(&f.core, &main, "main", "p2")
        .await
        .unwrap();
    expect.push((StoredKind::ProjectState, Some("p2".into())));

    let got = next_changes(&mut events).await;
    let kinds: Vec<(StoredKind, Option<String>)> =
        got.iter().map(|c| (c.kind, c.scope.clone())).collect();
    assert_eq!(kinds, expect);
    let ns: Vec<u64> = got.iter().map(|c| c.seq.n).collect();
    assert!(ns.windows(2).all(|w| w[0] < w[1]), "{ns:?}");
    // The event comes after the commit: its seq is published.
    assert!(f.ws.change_seq().n >= *ns.last().unwrap());
    assert!(got[..got.len() - 2]
        .iter()
        .all(|c| c.origin.as_deref() == Some("tab-a")));
}

#[tokio::test]
async fn a_refused_write_emits_nothing() {
    let f = fx().await;
    let mut events = f.ws.events();
    let o = none();
    let _ =
        f.ws.create_dashboard(&f.core, &o, dash("p1", ""))
            .await
            .unwrap_err();
    let _ =
        f.ws.update_dashboard(&f.core, &o, "dashboard-x", j(json!({})))
            .await
            .unwrap_err();
    let _ =
        f.ws.set_setting(&f.core, &o, "nope", None)
            .await
            .unwrap_err();
    let _ =
        f.ws.set_setting(&f.core, &o, "editorKeybindingMode", Some("hyper".into()))
            .await
            .unwrap_err();
    let _ =
        f.ws.remove_ai_provider(&f.core, &o, "prov-x")
            .await
            .unwrap_err();
    let _ =
        f.ws.remove_user_theme(&f.core, &o, "theme-x")
            .await
            .unwrap_err();
    let _ =
        f.ws.put_chat_messages(&f.core, &o, "none", vec![msg("m", "user", "", T0)])
            .await
            .unwrap_err();
    let _ =
        f.ws.save_window_state(
            &f.core,
            &WriteOrigin::new(Some("a")),
            "b",
            "p1",
            1,
            rv(common_state("p1")),
        )
        .await
        .unwrap_err();
    // A stale save writes nothing and emits nothing either.
    let main = WriteOrigin::new(Some("main"));
    f.ws.save_window_state(&f.core, &main, "main", "p1", 3, rv(common_state("p1")))
        .await
        .unwrap();
    let before = next_changes(&mut events).await;
    assert_eq!(before.len(), 1, "{before:?}");
    let stale =
        f.ws.save_window_state(&f.core, &main, "main", "p1", 2, rv(common_state("p1")))
            .await
            .unwrap();
    assert!(stale.value.stale);
    assert!(next_changes(&mut events).await.is_empty());
}

#[tokio::test]
async fn message_events_are_scoped_to_the_chat_and_bounded() {
    let f = fx().await;
    let c = chat(&f).await;
    let mut events = f.ws.events();
    f.ws.put_chat_messages(&f.core, &none(), &c, vec![msg("m1", "user", "q", T0)])
        .await
        .unwrap();
    let msgs: Vec<ChatMessageDraft> = (0..6000)
        .map(|i| msg(&format!("n{i}"), "user", "x", T0))
        .collect();
    f.ws.put_chat_messages(&f.core, &none(), &c, msgs)
        .await
        .unwrap();
    let got = next_changes(&mut events).await;
    assert_eq!(got.len(), 2);
    assert!(got
        .iter()
        .all(|e| e.kind == StoredKind::ChatMessages && e.scope.as_deref() == Some(c.as_str())));
    assert_eq!(got[0].ids, Some(vec!["m1".to_string()]));
    assert_eq!(got[1].ids, None, "6,000 ids are a reload of the chat");
}

#[tokio::test(flavor = "multi_thread")]
async fn seq_follows_commit_order_across_library_and_state_writes() {
    let f = Arc::new(fx().await);
    let mut events = f.ws.events();
    let mut tasks = Vec::new();
    for i in 0..20 {
        let f = f.clone();
        tasks.push(tokio::spawn(async move {
            if i % 2 == 0 {
                f.ws.create_dashboard(&f.core, &none(), dash("p1", &format!("D{i}")))
                    .await
                    .unwrap()
                    .seq
            } else {
                f.ws.create_project(&f.core, &none(), j(json!({"name": format!("P{i}")})))
                    .await
                    .unwrap()
                    .seq
            }
        }));
    }
    let mut seqs = Vec::new();
    for t in tasks {
        seqs.push(t.await.unwrap().n);
    }
    seqs.sort();
    seqs.dedup();
    assert_eq!(seqs.len(), 20, "every write has its own number");
    let got = next_changes(&mut events).await;
    assert_eq!(got.len(), 20);
    // The rows' commit order is the numbers' order: each write read the
    // count inside its lock, so dashboards were created one after another.
    let mut ns: Vec<u64> = got.iter().map(|c| c.seq.n).collect();
    ns.sort();
    assert_eq!(ns, seqs);
}

// ── Logs ──

#[tokio::test]
async fn no_text_names_json_or_keys_in_logs() {
    let capture = common::capture_logs();
    let f = fx().await;
    let win = "CANARYwin7f3a";
    let origin = WriteOrigin::new(Some(win));
    let canary = |what: &str| format!("CANARY-{what}-7f3a");
    let mut state = common_state("p1");
    state["queryTabs"][0]["query"] = json!(canary("tabtext"));
    f.ws.save_window_state(&f.core, &origin, win, "p1", 1, rv(state))
        .await
        .unwrap();
    f.ws.activate_window(&f.core, &origin, win, "p1")
        .await
        .unwrap();
    f.ws.load_window_state(&f.core, &origin, win, "p2")
        .await
        .unwrap();
    let d = f
        .ws
        .create_dashboard(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "name": canary("dashname"), "widgets": [{"q": canary("widget")}], "viewport": {}})),
        )
        .await
        .unwrap();
    f.ws.update_dashboard(
        &f.core,
        &none(),
        &d.value.id,
        j(json!({"description": canary("desc"), "captureVersion": true})),
    )
    .await
    .unwrap();
    let c = chat(&f).await;
    f.ws.put_chat_messages(
        &f.core,
        &none(),
        &c,
        vec![msg("m1", "user", &canary("message"), T0)],
    )
    .await
    .unwrap();
    f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": canary("theme")})))
        .await
        .unwrap();
    f.ws.set_setting(
        &f.core,
        &none(),
        "skippedUpdateVersion",
        Some(canary("setting")),
    )
    .await
    .unwrap();
    f.ws.create_ai_provider(
        &f.core,
        &none(),
        j(json!({"name": canary("provider"), "type": "anthropic"})),
        Some(Some(canary("apikey"))),
    )
    .await
    .unwrap();
    f.store
        .fail_set
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let _ =
        f.ws.create_ai_provider(
            &f.core,
            &none(),
            j(json!({"name": "x", "type": "anthropic"})),
            Some(Some(canary("apikey2"))),
        )
        .await;
    f.store
        .fail_set
        .store(false, std::sync::atomic::Ordering::SeqCst);
    // Refused (not a background the GUI has), and still not logged.
    f.ws.patch_onboarding(
        &f.core,
        &none(),
        rv(json!({"userBackground": canary("background")})),
    )
    .await
    .unwrap_err();
    f.ws.save_tutorial(
        &f.core,
        &none(),
        j(json!({"lessonId": canary("lesson"), "challengeId": "c", "state": canary("tstate")})),
    )
    .await
    .unwrap();
    let logs = capture.0.lock().unwrap().join("\n");
    assert!(
        logs.contains("library.dashboardCreate"),
        "the capture works"
    );
    assert!(
        !logs.contains("CANARY"),
        "no text, names, JSON, keys or window ids:\n{logs}"
    );
}

#[test]
fn debug_shows_no_text_names_or_values() {
    let canary = "CANARY-9d2e";
    let d: DashboardDraft = j(
        json!({"projectId": "p1", "name": canary, "description": canary,
        "widgets": [canary], "viewport": {"c": canary}, "dateFilter": {"c": canary}}),
    );
    let p: DashboardPatch = j(
        json!({"name": canary, "description": canary, "widgets": [canary],
        "viewport": {}, "dateFilter": null, "captureVersion": true}),
    );
    let w: WorkflowDraft = j(json!({"projectId": "p1", "workflow": {"name": canary}}));
    let c: ChatDraft = j(json!({"connectionId": "c1", "title": canary}));
    let cp: ChatPatch = j(json!({"title": canary}));
    let m = msg("m1", "user", canary, canary);
    let a: AiProviderDraft = j(json!({"name": canary, "type": "anthropic", "baseUrl": canary}));
    let ap: AiProviderPatch = j(json!({"name": canary, "baseUrl": canary}));
    let out = format!("{d:?}{p:?}{w:?}{c:?}{cp:?}{m:?}{a:?}{ap:?}");
    assert!(!out.contains("CANARY"), "{out}");
    assert!(out.contains("p1") && out.contains("m1"), "{out}");
}

fn is_uuid_v4_prefixed(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(is_uuid_v4)
}

#[tokio::test]
async fn ids_have_their_prefixes() {
    let f = fx().await;
    let d =
        f.ws.create_dashboard(&f.core, &none(), dash("p1", "D"))
            .await
            .unwrap()
            .value;
    assert!(is_uuid_v4_prefixed(&d.id, "dashboard-"));
    let t =
        f.ws.create_user_theme(&f.core, &none(), rv(json!({"name": "T"})))
            .await
            .unwrap()
            .value;
    assert!(is_uuid_v4_prefixed(&t.id, "theme-"));
    let stored: Value = serde_json::from_str(t.themes.user_themes[0].get()).unwrap();
    assert_eq!(stored["id"], t.id.as_str(), "the JSON's id is the row's");
    let p =
        f.ws.create_ai_provider(
            &f.core,
            &none(),
            j(json!({"name": "A", "type": "anthropic"})),
            None,
        )
        .await
        .unwrap()
        .value;
    assert!(is_uuid_v4(&p.id));
}

#[tokio::test]
async fn a_web_workspace_never_takes_an_api_key() {
    let f = fx_with(web_state_limits(), false).await;
    for key in [Some(Some("sk".to_string())), Some(None)] {
        let e =
            f.ws.create_ai_provider(
                &f.core,
                &none(),
                j(json!({"name": "A", "type": "anthropic"})),
                key.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "NOT_SUPPORTED");
    }
    let settings = rows(&f.ws, "app_state", "key").await;
    assert!(
        settings.iter().all(|r| r["key"] != "aiSettings"),
        "nothing written"
    );
}

// ── Phase 5d-2 review: over-limit rows stay editable ──

#[tokio::test]
async fn an_over_limit_workflow_stays_saveable_and_can_only_shrink() {
    let limits = StateLimits {
        max_workflow_bytes: Some(2000),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let body = |n: usize| json!({"name": "W", "nodes": [{"rows": "x".repeat(n)}]});
    let stored = {
        let mut v = body(3000);
        v["id"] = json!("workflow-old");
        v["projectId"] = json!("p1");
        v["createdAt"] = json!(T0);
        v["updatedAt"] = json!(T0);
        v.to_string()
    };
    insert_rows(
        f.ws.storage(),
        "saved_canvases",
        &[json!({"id": "workflow-old", "project_id": "p1", "data": stored})],
    )
    .await;
    f.ws.update_workflow(&f.core, &none(), "workflow-old", rv(body(3000)))
        .await
        .unwrap();
    f.ws.update_workflow(&f.core, &none(), "workflow-old", rv(body(2500)))
        .await
        .unwrap();
    let e =
        f.ws.update_workflow(&f.core, &none(), "workflow-old", rv(body(2600)))
            .await
            .unwrap_err();
    assert!(e.message.contains("max_workflow_bytes"), "{e:?}");
    let e =
        f.ws.create_workflow(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "workflow": body(2500)})),
        )
        .await
        .unwrap_err();
    assert!(
        e.message.contains("max_workflow_bytes"),
        "a new one gets no allowance: {e:?}"
    );
}

#[tokio::test]
async fn an_over_limit_dashboard_stays_editable_and_can_only_shrink() {
    let limits = StateLimits {
        max_dashboard_bytes: Some(2000),
        ..web_state_limits()
    };
    let f = fx_with(limits, false).await;
    let widgets = |n: usize| json!([{"id": "w", "query": "x".repeat(n)}]);
    insert_rows(
        f.ws.storage(),
        "dashboards",
        &[
            json!({"id": "dash-1", "project_id": "p1", "name": "D", "viewport": "{}",
                 "widgets": widgets(3000).to_string(), "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    f.ws.update_dashboard(
        &f.core,
        &none(),
        "dash-1",
        j(json!({"name": "E", "captureVersion": true})),
    )
    .await
    .unwrap();
    f.ws.update_dashboard(
        &f.core,
        &none(),
        "dash-1",
        j(json!({"widgets": widgets(2500)})),
    )
    .await
    .unwrap();
    let e =
        f.ws.update_dashboard(
            &f.core,
            &none(),
            "dash-1",
            j(json!({"widgets": widgets(2600)})),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("max_dashboard_bytes"), "{e:?}");
}
