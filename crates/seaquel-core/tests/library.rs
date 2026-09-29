//! The library through Core (phase 5d-1): the replay of every library
//! fixture (`crates/seaquel-workspace/tests/fixtures/library`) under its
//! README's Rust replay rules and `changes.json`, and Core's own rules:
//! ids and times, patches, secrets first, removal, labels, versions, the
//! web's engines and limits.
//!
//! Storage is a temp dir and secrets a `TestStore`; nothing touches the real
//! keychain or data dir.
#![cfg(all(feature = "storage", feature = "secrets"))]
// Native-only tests: tasks on the test runtime and the wall clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use common::{core, core_with, dump, insert_rows, web_core, TestStore};
use seaquel_core::domain::library::{
    ConnectionDraft, ConnectionPatch, LabelDraft, LabelPatch, ProjectDraft, ProjectPatch,
    SavedQueryDraft, SavedQueryPatch, SecretChanges,
};
use seaquel_core::storage::{Storage, StorageOptions};
use seaquel_core::{
    Core, CoreError, LibraryLimits, Workspace, WorkspaceSpec, WriteOrigin,
    SAVED_CONNECTION_NOT_FOUND,
};
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-workspace/tests/fixtures/library"
);
const FILES: [&str; 5] = [
    "connections.json",
    "projects.json",
    "saved-queries.json",
    "imports.json",
    "legacy-strings.json",
];
const BETA_SCHEMA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-storage/tests/fixtures/schemas/v2026.4.5-beta.1.sql"
);

/// The recorder's tables, in seed order, with their sort keys.
const TABLES: [(&str, &str); 15] = [
    ("projects", "id"),
    ("project_labels", "project_id, id"),
    ("connections", "id"),
    ("connection_labels", "connection_id, label_id"),
    ("saved_queries", "id"),
    ("query_versions", "saved_query_id, version"),
    ("query_history", "id"),
    ("ai_chats", "id"),
    ("ai_messages", "id"),
    ("dashboards", "id"),
    ("dashboard_versions", "id"),
    ("saved_canvases", "id"),
    ("project_state", "project_id"),
    ("tabs", "project_id, id"),
    ("app_state", "key"),
];
/// Compared whole after every step.
const LIBRARY_TABLES: [&str; 6] = [
    "projects",
    "project_labels",
    "connections",
    "connection_labels",
    "saved_queries",
    "query_versions",
];
/// Compared by their ids.
const CASCADE_TABLES: [&str; 6] = [
    "query_history",
    "ai_chats",
    "ai_messages",
    "dashboards",
    "dashboard_versions",
    "saved_canvases",
];
const ID_PREFIXES: [&str; 5] = ["conn-", "project-", "label-", "saved-", "ver-"];

fn order_of(table: &str) -> &'static str {
    TABLES.iter().find(|(t, _)| *t == table).unwrap().1
}

// ── Tokens ──

/// A `<id:n>` or `<version:n>` token with the prefix before it, found in
/// `s`: (the token as written, its id prefix).
fn tokens_in(s: &str) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for marker in ["<id:", "<version:"] {
        let mut from = 0;
        while let Some(at) = s[from..].find(marker) {
            let start = from + at;
            let Some(close) = s[start..].find('>') else {
                break;
            };
            let end = start + close + 1;
            if marker == "<version:" {
                out.push((s[start..end].to_string(), "ver-"));
            } else if let Some(prefix) = ID_PREFIXES.iter().find(|p| s[..start].ends_with(**p)) {
                out.push((s[start - prefix.len()..end].to_string(), *prefix));
            }
            from = end;
        }
    }
    out
}

fn collect_tokens(v: &Value, out: &mut Vec<(String, &'static str)>) {
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
    // Longest first, so `conn-<id:1>` isn't cut by a shorter token.
    let mut tokens: Vec<&String> = bound.keys().collect();
    tokens.sort_by_key(|t| std::cmp::Reverse(t.len()));
    for t in tokens {
        if out.contains(t.as_str()) {
            out = out.replace(t.as_str(), &bound[t]);
        }
    }
    out
}

fn substitute_value(v: &Value, bound: &HashMap<String, String>) -> Value {
    match v {
        Value::String(s) => Value::String(substitute(s, bound)),
        Value::Array(a) => Value::Array(a.iter().map(|v| substitute_value(v, bound)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (substitute(k, bound), substitute_value(v, bound)))
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

/// Whether `actual` matches `expected` under the replay rules: tokens bound
/// or (unbound) any id with their prefix, `<now>` any time from the case,
/// numbers by value.
fn matches(expected: &Value, actual: &Value, cx: &Ctx) -> bool {
    match (expected, actual) {
        (Value::String(e), Value::String(a)) => {
            if e == "<now>" {
                return is_iso(a) && a.as_str() >= cx.started;
            }
            let e = substitute(e, cx.bound);
            if e == *a {
                return true;
            }
            // An unbound token stands for any Core id with its prefix.
            let unbound = tokens_in(&e);
            unbound.len() == 1
                && unbound[0].0 == e
                && a.strip_prefix(unbound[0].1).is_some_and(is_uuid_v4)
        }
        (Value::Number(e), Value::Number(a)) => e.as_f64() == a.as_f64(),
        (Value::Null, Value::Null) => true,
        (Value::Bool(e), Value::Bool(a)) => e == a,
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(e, a)| matches(e, a, cx))
        }
        (Value::Object(e), Value::Object(a)) => {
            e.len() == a.len()
                && e.iter()
                    .all(|(k, v)| a.get(k).is_some_and(|av| matches(v, av, cx)))
        }
        _ => false,
    }
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

/// Rows compared as sorted lists (Core's ids sort differently from the
/// recorder's).
fn rows_match(table: &str, expected: &[Value], actual: &[Value], cx: &Ctx) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let order = order_of(table);
    let mut e: Vec<&Value> = expected.iter().collect();
    let mut a: Vec<&Value> = actual.iter().collect();
    e.sort_by_key(|r| sort_key(r, order, cx.bound));
    a.sort_by_key(|r| sort_key(r, order, cx.bound));
    e.iter().zip(&a).all(|(e, a)| matches(e, a, cx))
}

fn ids_of(rows: &[Value]) -> HashSet<String> {
    rows.iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect()
}

/// What a step left: the rows of every table, and the keychain.
struct Snapshot {
    tables: BTreeMap<String, Vec<Value>>,
    secrets: BTreeMap<String, String>,
}

async fn snapshot(st: &Storage, store: &TestStore) -> Snapshot {
    let mut tables = BTreeMap::new();
    for (table, order) in TABLES {
        let mut rows = dump(st, table, order).await;
        // The fixtures were recorded before migration `0001_name_keys`
        // added `name_key` (phase 5d-1 probe fix). Each stored key must be
        // its row's; then the column leaves the comparison.
        for row in &mut rows {
            if let Some(obj) = row.as_object_mut() {
                if let Some(key) = obj.remove("name_key") {
                    if let (Some(key), Some(name)) = (key.as_str(), obj["name"].as_str()) {
                        assert_eq!(
                            key,
                            seaquel_core::domain::library::name_key(name),
                            "{table}"
                        );
                    }
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

/// What a step is expected to leave, recorded or from `changes.json`.
struct Expected<'a> {
    outcome: &'a Value,
    rows: &'a Value,
    secrets: &'a Value,
}

/// `None` when everything matches; otherwise why not.
fn compare(
    exp: &Expected,
    outcome: &Result<Value, CoreError>,
    snap: &Snapshot,
    cx: &Ctx,
) -> Option<String> {
    let mut why = Vec::new();
    // Outcome.
    match (exp.outcome["ok"].as_bool(), outcome) {
        (Some(true), Ok(_)) => {}
        (Some(true), Err(e)) => why.push(format!("expected ok, got {}: {}", e.code, e.message)),
        (_, Ok(_)) => why.push(format!("expected a refusal {}, got ok", exp.outcome)),
        (_, Err(e)) => {
            let code = exp.outcome["code"].as_str().map(|c| {
                if c == "SAVED_CONNECTION_NOT_FOUND" {
                    SAVED_CONNECTION_NOT_FOUND
                } else {
                    c
                }
            });
            if code != Some(e.code.as_str()) {
                why.push(format!("expected {:?}, got {}", exp.outcome, e.code));
            }
            if let Some(t) = exp.outcome.get("takenBy").and_then(Value::as_str) {
                if e.taken_by.as_deref() != Some(substitute(t, cx.bound).as_str()) {
                    why.push(format!("takenBy {t} != {:?}", e.taken_by));
                }
            }
        }
    }
    // Library tables, whole.
    for table in LIBRARY_TABLES {
        let e = exp.rows[table].as_array().cloned().unwrap_or_default();
        let a = &snap.tables[table];
        if !rows_match(table, &e, a, cx) {
            why.push(format!(
                "{table}:\n  expected {}\n  actual   {}",
                Value::Array(e.iter().map(|v| substitute_value(v, cx.bound)).collect()),
                Value::Array(a.clone())
            ));
        }
    }
    // What hangs off them, by id.
    for table in CASCADE_TABLES {
        let e: HashSet<String> = ids_of(exp.rows[table].as_array().map_or(&[][..], |v| v))
            .iter()
            .map(|id| substitute(id, cx.bound))
            .collect();
        let a = ids_of(&snap.tables[table]);
        if e != a {
            why.push(format!("{table} ids: expected {e:?}, actual {a:?}"));
        }
    }
    // No state or tab of a removed project.
    let projects = ids_of(&snap.tables["projects"]);
    for table in ["project_state", "tabs"] {
        for row in &snap.tables[table] {
            let p = row["project_id"].as_str().unwrap_or_default();
            if !projects.contains(p) {
                why.push(format!("{table} row of removed project {p}"));
            }
        }
    }
    // The keychain, whole.
    let e: BTreeMap<String, String> = exp
        .secrets
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    (
                        substitute(k, cx.bound),
                        v.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if e != snap.secrets {
        why.push(format!(
            "secretStore: expected {e:?}, actual keys {:?}",
            snap.secrets.keys().collect::<Vec<_>>()
        ));
    }
    (!why.is_empty()).then(|| why.join("\n"))
}

// ── Running a case ──

struct Replay {
    _dir: tempfile::TempDir,
    core: Core,
    ws: Arc<Workspace>,
    store: Arc<TestStore>,
    bound: HashMap<String, String>,
    started: String,
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
    for (table, _) in TABLES {
        if let Some(rows) = case["seed"][table].as_array() {
            insert_rows(&st, table, rows).await;
        }
    }
    if case["name"] == "legacy/strings-for-each-engine" {
        sqlx::query(
            "DELETE FROM _seaquel_data_steps WHERE name = 'drop_legacy_built_connection_strings'",
        )
        .execute(st.pool())
        .await
        .unwrap();
    }
    st.close().await;

    let store = TestStore::new();
    if let Some(secrets) = case["secrets"].as_object() {
        for (k, v) in secrets {
            store.put(k, v.as_str().unwrap());
        }
    }
    let core = core();
    let spec = WorkspaceSpec::new(dir.path());
    let spec = if case["target"] == "web" {
        spec
    } else {
        spec.with_secrets(store.clone())
    };
    let started = seaquel_core::domain::run::iso_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap(),
    );
    let ws = core.open_workspace(spec).await.unwrap();
    Replay {
        _dir: dir,
        core,
        ws,
        store,
        bound: HashMap::new(),
        started,
    }
}

/// A plain pool on a new file, without `Storage::open`'s schema.
async fn raw_file(path: &Path) -> sqlx::SqlitePool {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    sqlx::SqlitePool::connect_with(opts).await.unwrap()
}

fn param<T: serde::de::DeserializeOwned>(params: &Value, key: &str) -> T {
    serde_json::from_value(params.get(key).cloned().unwrap_or(Value::Null))
        .unwrap_or_else(|e| panic!("params.{key}: {e}\n{params}"))
}

fn opt_param<T: serde::de::DeserializeOwned + Default>(params: &Value, key: &str) -> T {
    match params.get(key) {
        None => T::default(),
        Some(v) => {
            serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("params.{key}: {e}"))
        }
    }
}

fn to_json<T: serde::Serialize>(r: Result<T, CoreError>) -> Result<Value, CoreError> {
    r.map(|v| serde_json::to_value(v).unwrap())
}

impl Replay {
    /// Binds the unbound tokens in `params` to the ids made earlier in this
    /// step (by prefix, in order), then runs the call.
    async fn call(&mut self, call: &Value, fresh: &mut Vec<String>) -> Result<Value, CoreError> {
        let method = call["method"].as_str().unwrap();
        let raw = call.get("params").cloned().unwrap_or(Value::Null);
        let mut tokens = Vec::new();
        collect_tokens(&raw, &mut tokens);
        for (token, prefix) in tokens {
            if self.bound.contains_key(&token) {
                continue;
            }
            let taken: HashSet<&String> = self.bound.values().collect();
            let id = fresh
                .iter()
                .find(|id| id.starts_with(prefix) && !taken.contains(id))
                .unwrap_or_else(|| panic!("{token} in {method} names no id made earlier"))
                .clone();
            self.bound.insert(token, id);
        }
        let p = substitute_value(&raw, &self.bound);
        let (core, ws, o) = (&self.core, &self.ws, &WriteOrigin::new(Some("replay")));
        let result = match method {
            "connectionsList" => to_json(ws.list_connections().await),
            "connectionCreate" => to_json(
                ws.create_connection(
                    core,
                    o,
                    param::<ConnectionDraft>(&p, "connection"),
                    opt_param::<SecretChanges>(&p, "secrets"),
                )
                .await,
            ),
            "connectionUpdate" => to_json(
                ws.update_connection(
                    core,
                    o,
                    &param::<String>(&p, "id"),
                    param::<ConnectionPatch>(&p, "patch"),
                    opt_param::<SecretChanges>(&p, "secrets"),
                )
                .await,
            ),
            "connectionRemove" => to_json(
                ws.remove_connection(core, o, &param::<String>(&p, "id"))
                    .await,
            ),
            "projectCreate" => to_json(
                ws.create_project(core, o, param::<ProjectDraft>(&p, "project"))
                    .await,
            ),
            "projectEnsureDefault" => to_json(ws.ensure_default_project(core, o).await),
            "projectUpdate" => to_json(
                ws.update_project(
                    core,
                    o,
                    &param::<String>(&p, "id"),
                    param::<ProjectPatch>(&p, "patch"),
                )
                .await,
            ),
            "projectRemove" => {
                to_json(ws.remove_project(core, o, &param::<String>(&p, "id")).await)
            }
            "labelCreate" => to_json(
                ws.create_label(
                    core,
                    o,
                    &param::<String>(&p, "projectId"),
                    param::<LabelDraft>(&p, "label"),
                )
                .await,
            ),
            "labelUpdate" => to_json(
                ws.update_label(
                    core,
                    o,
                    &param::<String>(&p, "projectId"),
                    &param::<String>(&p, "labelId"),
                    param::<LabelPatch>(&p, "patch"),
                )
                .await,
            ),
            "labelRemove" => to_json(
                ws.remove_label(
                    core,
                    o,
                    &param::<String>(&p, "projectId"),
                    &param::<String>(&p, "labelId"),
                )
                .await,
            ),
            "savedQueryCreate" => to_json(
                ws.create_saved_query(core, o, param::<SavedQueryDraft>(&p, "query"))
                    .await,
            ),
            "savedQueryUpdate" => to_json(
                ws.update_saved_query(
                    core,
                    o,
                    &param::<String>(&p, "id"),
                    param::<SavedQueryPatch>(&p, "patch"),
                )
                .await,
            ),
            "savedQueryRemove" => to_json(
                ws.remove_saved_query(core, o, &param::<String>(&p, "id"))
                    .await,
            ),
            other => panic!("unknown library method {other}"),
        };
        if let Ok(v) = &result {
            // The ids this call made, in order.
            let made = match method {
                "connectionCreate" | "projectCreate" | "labelCreate" | "savedQueryCreate" => {
                    vec![v["value"]["id"].as_str().unwrap().to_string()]
                }
                "savedQueryUpdate" => v["value"]["version"]["id"]
                    .as_str()
                    .map(|s| vec![s.to_string()])
                    .unwrap_or_default(),
                _ => vec![],
            };
            for id in made {
                let prefix = ID_PREFIXES.iter().find(|p| id.starts_with(**p)).unwrap();
                assert!(
                    is_uuid_v4(&id[prefix.len()..]),
                    "{id} isn't a prefix and a v4 uuid"
                );
                fresh.push(id);
            }
        }
        result
    }
}

/// Binds the tokens still unbound in `expected` to the ids made in the
/// step, trying every assignment within a prefix until the rows match.
fn bind_step(
    bound: &mut HashMap<String, String>,
    expected: &[&Value],
    fresh: &[String],
    fits: impl Fn(&HashMap<String, String>) -> bool,
) {
    let mut tokens = Vec::new();
    for v in expected {
        collect_tokens(v, &mut tokens);
    }
    let mut seen = HashSet::new();
    let tokens: Vec<(String, &str)> = tokens
        .into_iter()
        .filter(|(t, _)| !bound.contains_key(t) && seen.insert(t.clone()))
        .collect();
    let taken: HashSet<String> = bound.values().cloned().collect();
    let free: Vec<&String> = fresh.iter().filter(|id| !taken.contains(*id)).collect();
    // One assignment per prefix; with few ids, try every permutation.
    let mut choices: Vec<Vec<(String, String)>> = vec![vec![]];
    for prefix in ID_PREFIXES {
        let ts: Vec<&String> = tokens
            .iter()
            .filter(|(_, p)| *p == prefix)
            .map(|(t, _)| t)
            .collect();
        let ids: Vec<&String> = free
            .iter()
            .filter(|id| id.starts_with(prefix))
            .copied()
            .collect();
        if ts.is_empty() || ids.len() < ts.len() {
            continue;
        }
        let mut next = Vec::new();
        for perm in permutations(ids.len(), ts.len()) {
            for base in &choices {
                let mut c = base.clone();
                c.extend(
                    ts.iter()
                        .zip(&perm)
                        .map(|(t, &i)| ((*t).clone(), ids[i].clone())),
                );
                next.push(c);
            }
        }
        choices = next;
    }
    for c in &choices {
        let mut trial = bound.clone();
        trial.extend(c.iter().cloned());
        if fits(&trial) {
            *bound = trial;
            return;
        }
    }
    if let Some(c) = choices.first() {
        bound.extend(c.iter().cloned());
    }
}

/// Every ordered choice of `k` of `n` indexes (n is small here).
fn permutations(n: usize, k: usize) -> Vec<Vec<usize>> {
    if k == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for rest in permutations(n, k - 1) {
        for i in 0..n {
            if !rest.contains(&i) {
                let mut p = rest.clone();
                p.push(i);
                out.push(p);
            }
        }
    }
    out.truncate(5040);
    out
}

fn load(file: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
    serde_json::from_str::<Vec<Value>>(&text).unwrap()
}

fn changes() -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(format!("{FIXTURES}/changes.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The rows a step is expected to leave: the recorded ones with the
/// entry's tables replaced.
fn expected_rows(recorded: &Value, entry: Option<&Value>) -> Value {
    let mut rows = recorded.clone();
    if let Some(Value::Object(tables)) = entry.and_then(|e| e.get("rows")) {
        let obj = rows.as_object_mut().unwrap();
        for (t, v) in tables {
            obj.insert(t.clone(), v.clone());
        }
    }
    rows
}

#[tokio::test(flavor = "multi_thread")]
async fn replays_every_fixture() {
    let changes = changes();
    let mut failures = Vec::new();
    let mut listed_seen = HashSet::new();
    let (mut cases, mut steps) = (0, 0);
    for file in FILES {
        for case in load(file) {
            cases += 1;
            let name = case["name"].as_str().unwrap().to_string();
            let mut r = open_case(&case).await;
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                if step["library"].is_null() {
                    continue;
                }
                steps += 1;
                let entry = changes
                    .get(&name)
                    .and_then(|c| c["expected"]["steps"].get(i.to_string()));
                if entry.is_some() {
                    listed_seen.insert(format!("{name}#{i}"));
                }
                let inject = step["inject"].as_str().unwrap_or_default();
                r.store
                    .fail_set
                    .store(inject.starts_with("secret set fails"), Ordering::SeqCst);
                let calls: Vec<Value> = match &step["library"] {
                    Value::Array(a) => a.clone(),
                    one => vec![one.clone()],
                };
                let mut fresh = Vec::new();
                let mut outcome = Ok(Value::Null);
                for call in &calls {
                    outcome = r.call(call, &mut fresh).await;
                    if outcome.is_err() {
                        break;
                    }
                }
                r.store.fail_set.store(false, Ordering::SeqCst);
                let snap = snapshot(r.ws.storage(), &r.store).await;

                let recorded = Expected {
                    outcome: &step["outcome"],
                    rows: &step["rows"],
                    secrets: &step["secretStore"],
                };
                let exp_rows = expected_rows(&step["rows"], entry);
                let expected = Expected {
                    outcome: entry
                        .and_then(|e| e.get("outcome"))
                        .unwrap_or(&step["outcome"]),
                    rows: &exp_rows,
                    secrets: entry
                        .and_then(|e| e.get("secretStore"))
                        .unwrap_or(&step["secretStore"]),
                };
                let started = r.started.clone();
                bind_step(
                    &mut r.bound,
                    &[expected.outcome, expected.rows, expected.secrets],
                    &fresh,
                    |b| {
                        compare(
                            &expected,
                            &outcome,
                            &snap,
                            &Ctx {
                                bound: b,
                                started: &started,
                            },
                        )
                        .is_none()
                    },
                );
                // Every `<id:n>` the step expects names an id Core made; only
                // `<version:n>` may stay a wildcard.
                let mut unbound = Vec::new();
                for v in [expected.outcome, expected.secrets] {
                    collect_tokens(v, &mut unbound);
                }
                // The compared tables only (`project_state` holds ids of
                // connections the TS saved in a refused step).
                for table in LIBRARY_TABLES.iter().chain(&CASCADE_TABLES) {
                    collect_tokens(&expected.rows[*table], &mut unbound);
                }
                unbound.retain(|(t, _)| t.contains("<id:") && !r.bound.contains_key(t));
                unbound.sort();
                unbound.dedup();
                if !unbound.is_empty() {
                    failures.push(format!(
                        "{name} step {i}: {:?} bound to no id Core made",
                        unbound.iter().map(|(t, _)| t).collect::<Vec<_>>()
                    ));
                }
                let cx = Ctx {
                    bound: &r.bound,
                    started: &started,
                };
                if let Some(why) = compare(&expected, &outcome, &snap, &cx) {
                    failures.push(format!(
                        "{name} step {i}{}:\n{why}",
                        if entry.is_some() { " (listed)" } else { "" }
                    ));
                } else if entry.is_some() && compare(&recorded, &outcome, &snap, &cx).is_none() {
                    failures.push(format!(
                        "{name} step {i} is listed in changes.json but matches the recording"
                    ));
                }
            }
            r.ws.close().await;
        }
    }
    // Every listed step exists and was replayed.
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
    assert!(cases >= 116 && steps >= 140, "{cases} cases, {steps} steps");
}

// ── Core's own rules ──

struct Fx {
    _dir: tempfile::TempDir,
    core: Core,
    ws: Arc<Workspace>,
    store: Arc<TestStore>,
}

async fn fx() -> Fx {
    fx_on(core(), true).await
}

async fn fx_on(core: Core, with_store: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = TestStore::new();
    let spec = WorkspaceSpec::new(dir.path());
    let spec = if with_store {
        spec.with_secrets(store.clone())
    } else {
        spec
    };
    let ws = core.open_workspace(spec).await.unwrap();
    let o = WriteOrigin::none();
    for (id, name) in [("p1", "Main"), ("p2", "Other")] {
        insert_rows(
            ws.storage(),
            "projects",
            &[
                json!({"id": id, "name": name, "description": null, "created_at": T0,
                     "updated_at": T0, "git_repo_path": null}),
            ],
        )
        .await;
    }
    let _ = o;
    Fx {
        _dir: dir,
        core,
        ws,
        store,
    }
}

const T0: &str = "2024-01-01T00:00:00.000Z";

fn conn_draft(project: &str, name: &str, extra: Value) -> ConnectionDraft {
    let mut d = json!({
        "projectId": project, "name": name, "type": "postgres", "host": "db.example.com",
        "port": 5432, "databaseName": "app", "username": "alice",
    });
    for (k, v) in extra.as_object().unwrap() {
        d[k] = v.clone();
    }
    serde_json::from_value(d).unwrap()
}

fn j<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

fn none() -> WriteOrigin {
    WriteOrigin::none()
}

impl Fx {
    async fn add(&self, name: &str, extra: Value, secrets: Value) -> Result<String, CoreError> {
        self.ws
            .create_connection(
                &self.core,
                &none(),
                conn_draft("p1", name, extra),
                j(secrets),
            )
            .await
            .map(|s| s.value.id)
    }

    async fn row(&self, id: &str) -> Option<Value> {
        dump(self.ws.storage(), "connections", "id")
            .await
            .into_iter()
            .find(|r| r["id"] == id)
    }
}

#[tokio::test]
async fn create_returns_core_ids_and_times() {
    let f = fx().await;
    let before = seaquel_core::domain::run::iso_timestamp(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap(),
    );
    let c =
        f.ws.create_connection(
            &f.core,
            &none(),
            conn_draft("p1", "A", json!({"connected": true})),
            SecretChanges::default(),
        )
        .await
        .unwrap();
    assert!(c.value.id.starts_with("conn-") && is_uuid_v4(&c.value.id[5..]));
    let at = c.value.last_connected.clone().unwrap();
    assert!(is_iso(&at) && at >= before, "{at}");
    let p =
        f.ws.create_project(&f.core, &none(), j(json!({"name": "New"})))
            .await
            .unwrap();
    assert!(p.value.id.starts_with("project-") && is_uuid_v4(&p.value.id[8..]));
    assert_eq!(p.value.created_at, p.value.updated_at);
    let l =
        f.ws.create_label(
            &f.core,
            &none(),
            "p1",
            j(json!({"name": "L", "color": "#aabbcc"})),
        )
        .await
        .unwrap();
    assert!(l.value.id.starts_with("label-") && is_uuid_v4(&l.value.id[6..]));
    let q =
        f.ws.create_saved_query(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "name": "Q", "query": "SELECT 1"})),
        )
        .await
        .unwrap();
    assert!(q.value.id.starts_with("saved-") && is_uuid_v4(&q.value.id[6..]));
    let u =
        f.ws.update_saved_query(
            &f.core,
            &none(),
            &q.value.id,
            j(json!({"query": "SELECT 2"})),
        )
        .await
        .unwrap();
    let v = u.value.version.unwrap();
    assert!(v.id.starts_with("ver-") && is_uuid_v4(&v.id[4..]));
    assert_eq!(v.snapshot.as_deref(), Some("SELECT 1"));
    assert!(c.seq.n < p.seq.n && p.seq.n < l.seq.n && l.seq.n < q.seq.n && q.seq.n < u.seq.n);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_patches_of_different_fields_both_land() {
    let f = Arc::new(fx().await);
    let id = f.add("A", json!({}), json!({})).await.unwrap();
    let a = {
        let (f, id) = (f.clone(), id.clone());
        tokio::spawn(async move {
            f.ws.update_connection(
                &f.core,
                &none(),
                &id,
                j(json!({"host": "h-from-a"})),
                SecretChanges::default(),
            )
            .await
            .unwrap()
        })
    };
    let b = {
        let (f, id) = (f.clone(), id.clone());
        tokio::spawn(async move {
            f.ws.update_connection(
                &f.core,
                &none(),
                &id,
                j(json!({"databaseName": "db-from-b"})),
                SecretChanges::default(),
            )
            .await
            .unwrap()
        })
    };
    a.await.unwrap();
    b.await.unwrap();
    let row = f.row(&id).await.unwrap();
    assert_eq!(row["host"], "h-from-a");
    assert_eq!(row["database_name"], "db-from-b");
}

#[tokio::test]
async fn a_patch_can_t_move_a_connection() {
    let e = serde_json::from_value::<ConnectionPatch>(json!({"projectId": "p2"}));
    assert!(e.is_err(), "no project in a patch");
}

#[tokio::test]
async fn a_keychain_failure_leaves_the_row_unchanged() {
    let f = fx().await;
    let id = f.add("A", json!({}), json!({})).await.unwrap();
    let before = f.row(&id).await.unwrap();
    f.store.fail_set.store(true, Ordering::SeqCst);
    let err =
        f.ws.update_connection(
            &f.core,
            &none(),
            &id,
            j(json!({"host": "h2", "savePassword": true})),
            j(json!({"db": "pw"})),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_STORE_ERROR");
    assert!(!err.message.contains("pw\""), "{err}");
    assert_eq!(f.row(&id).await.unwrap(), before);
    assert!(f.store.entries().is_empty());
}

#[tokio::test]
async fn a_failed_insert_deletes_the_secret_it_set() {
    // The count limit is checked inside the transaction, after the keychain
    // write: the create fails there, and the secret it set goes.
    let f = fx_on(
        core_with(LibraryLimits {
            max_connections: Some(1),
            ..Default::default()
        }),
        true,
    )
    .await;
    f.add("A", json!({}), json!({})).await.unwrap();
    let err = f
        .add(
            "B",
            json!({"savePassword": true, "saveSshPassword": true}),
            json!({"db": "pw", "ssh": "spw"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert!(err.message.contains("max_connections"), "{err}");
    assert!(
        f.store.sets.load(Ordering::SeqCst) == 2,
        "the secrets were set first"
    );
    assert!(f.store.entries().is_empty(), "{:?}", f.store.entries());
}

/// Phase 5d-1 probe fix: `NAME_TAKEN` is an indexed lookup of the stored
/// `name_key` now, with the same answers. Rows that already share a name
/// (from before the check) name the first as `takenBy` and can still be
/// patched; a row written without a key (an older release, or seeded by
/// hand as `fx` seeds its projects) is still found.
#[tokio::test]
async fn name_taken_keeps_its_answers_with_stored_keys() {
    let f = fx().await;
    let o = none();
    // Two rows that already share a name, written before any check.
    insert_rows(
        f.ws.storage(),
        "connections",
        &[
            json!({"id": "old-1", "project_id": "p1", "name": "Dup", "type": "postgres",
                   "host": "h", "port": 1, "database_name": "d", "username": "u"}),
            json!({"id": "old-2", "project_id": "p1", "name": " dup ", "type": "postgres",
                   "host": "h", "port": 1, "database_name": "d", "username": "u"}),
        ],
    )
    .await;
    let err = f.add("DUP", json!({}), json!({})).await.unwrap_err();
    assert_eq!(err.code, "NAME_TAKEN");
    assert_eq!(
        err.taken_by.as_deref(),
        Some("old-1"),
        "the first in rowid order"
    );
    // Each can still be patched, renamed in case only, or renamed apart.
    f.ws.update_connection(&f.core, &o, "old-2", j(json!({"port": 2})), j(json!({})))
        .await
        .unwrap();
    f.ws.update_connection(
        &f.core,
        &o,
        "old-2",
        j(json!({"name": "DUP"})),
        j(json!({})),
    )
    .await
    .unwrap();
    let err =
        f.ws.update_connection(
            &f.core,
            &o,
            "old-2",
            j(json!({"name": "Dupe"})),
            j(json!({})),
        )
        .await
        .map(|_| ());
    err.unwrap();
    // Renaming into a taken name names the holder.
    let err =
        f.ws.update_connection(
            &f.core,
            &o,
            "old-2",
            j(json!({"name": "dup"})),
            j(json!({})),
        )
        .await
        .unwrap_err();
    assert_eq!(err.taken_by.as_deref(), Some("old-1"));
    // An import takes the first free name.
    let imported =
        f.ws.create_connection(
            &f.core,
            &o,
            conn_draft("p1", "dup", json!({"renameIfTaken": true})),
            j(json!({})),
        )
        .await
        .unwrap();
    assert_eq!(imported.value.name, "dup (2)");
    let again =
        f.ws.create_connection(
            &f.core,
            &o,
            conn_draft("p1", "Dupe", json!({"renameIfTaken": true})),
            j(json!({})),
        )
        .await
        .unwrap();
    assert_eq!(again.value.name, "Dupe (2)");

    // Projects: `fx` seeded "Main" and "Other" without a key.
    let err =
        f.ws.create_project(&f.core, &o, j(json!({"name": " main "})))
            .await
            .unwrap_err();
    assert_eq!(err.taken_by.as_deref(), Some("p1"));
    let err =
        f.ws.update_project(&f.core, &o, "p2", j(json!({"name": "MAIN"})))
            .await
            .unwrap_err();
    assert_eq!(err.taken_by.as_deref(), Some("p1"));
    f.ws.update_project(&f.core, &o, "p1", j(json!({"name": "MAIN"})))
        .await
        .unwrap();

    // Saved queries, per folder (none and "" are one folder).
    let q = |name: &str, folder: Value| {
        j(json!({"projectId": "p1", "name": name, "query": "x", "folder": folder}))
    };
    let first =
        f.ws.create_saved_query(&f.core, &o, q("Q", Value::Null))
            .await
            .unwrap();
    let err =
        f.ws.create_saved_query(&f.core, &o, q("q", json!("")))
            .await
            .unwrap_err();
    assert_eq!(err.taken_by.as_deref(), Some(first.value.id.as_str()));
    f.ws.create_saved_query(&f.core, &o, q("q", json!("f")))
        .await
        .unwrap();
}

/// Phase 5d-1 probe fix: a writable open refills the `name_key` of rows
/// an older release wrote after the backfill step (a downgrade, then this
/// release again); a read-only open leaves them.
#[tokio::test]
async fn opening_refills_null_name_keys() {
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    // `fx`-style seeding writes no key, as an older release would.
    insert_rows(
        ws.storage(),
        "projects",
        &[json!({"id": "p1", "name": "Main", "created_at": T0, "updated_at": T0})],
    )
    .await;
    let key = |rows: Vec<Value>| rows[0]["name_key"].clone();
    assert_eq!(key(dump(ws.storage(), "projects", "id").await), Value::Null);
    ws.close().await;
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(
        key(dump(ws.storage(), "projects", "id").await),
        json!("main")
    );
    ws.close().await;
}

#[tokio::test]
async fn a_refused_create_never_touches_the_keychain() {
    let f = fx().await;
    f.add("Taken", json!({}), json!({})).await.unwrap();
    let err = f
        .add("taken", json!({"savePassword": true}), json!({"db": "pw"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "NAME_TAKEN");
    assert_eq!(f.store.sets.load(Ordering::SeqCst), 0);
    // A secret whose flag is off is refused too.
    let err = f
        .add("C", json!({}), json!({"db": "pw"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert_eq!(f.store.sets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_flag_turned_off_deletes_its_secret() {
    let f = fx().await;
    let id = f
        .add(
            "A",
            json!({"savePassword": true, "saveSshKeyPassphrase": true}),
            json!({"db": "pw", "sshKey": "kp"}),
        )
        .await
        .unwrap();
    assert_eq!(f.store.entries().len(), 2);
    f.ws.update_connection(
        &f.core,
        &none(),
        &id,
        j(json!({"savePassword": false})),
        SecretChanges::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        f.store.entries().keys().cloned().collect::<Vec<_>>(),
        vec![format!("ssh-key:{id}")]
    );
    // An explicit delete, with the flag still on.
    f.ws.update_connection(
        &f.core,
        &none(),
        &id,
        ConnectionPatch::default(),
        j(json!({"sshKey": null})),
    )
    .await
    .unwrap();
    assert!(f.store.entries().is_empty());
}

#[tokio::test]
async fn a_secret_set_on_web_is_not_supported() {
    let f = fx_on(web_core(LibraryLimits::default()), false).await;
    let err = f
        .add("A", json!({"savePassword": true}), json!({"db": "pw"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
    assert!(f.row("A").await.is_none());
    assert!(dump(f.ws.storage(), "connections", "id").await.is_empty());
    // Without secrets the web saves the row.
    f.add("A", json!({"savePassword": true}), json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn remove_deletes_keychain_entries() {
    let f = fx().await;
    let id = f
        .add(
            "A",
            json!({"savePassword": true, "saveSshPassword": true, "saveSshKeyPassphrase": true}),
            json!({"db": "a", "ssh": "b", "sshKey": "c"}),
        )
        .await
        .unwrap();
    f.store.put("db:other", "kept");
    assert_eq!(f.store.entries().len(), 4);
    f.ws.remove_connection(&f.core, &none(), &id).await.unwrap();
    assert_eq!(
        f.store.entries().keys().cloned().collect::<Vec<_>>(),
        vec!["db:other"]
    );
    assert!(f.row(&id).await.is_none());
    let err =
        f.ws.remove_connection(&f.core, &none(), &id)
            .await
            .unwrap_err();
    assert_eq!(err.code, SAVED_CONNECTION_NOT_FOUND);
}

#[tokio::test]
async fn remove_deletes_vault_rows() {
    let f = fx_on(web_core(LibraryLimits::default()), false).await;
    let a = f.add("A", json!({}), json!({})).await.unwrap();
    let b = f.add("B", json!({}), json!({})).await.unwrap();
    let cred = |key: &str, scope: &str| json!({"scope": scope, "key": key, "nonce": "n", "ciphertext": "c", "updated_at": T0});
    insert_rows(
        f.ws.storage(),
        "user_credentials",
        &[cred(&a, "db"), cred(&a, "ssh"), cred(&b, "db")],
    )
    .await;
    f.ws.remove_connection(&f.core, &none(), &a).await.unwrap();
    let left = dump(f.ws.storage(), "user_credentials", "key").await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["key"], b.as_str());
    // A project's removal takes its connections' vault rows.
    f.ws.remove_project(&f.core, &none(), "p1").await.unwrap();
    assert!(dump(f.ws.storage(), "user_credentials", "key")
        .await
        .is_empty());
}

#[tokio::test]
async fn project_remove_refuses_the_last_project() {
    let f = fx().await;
    let a = f
        .add("A", json!({"savePassword": true}), json!({"db": "pw"}))
        .await
        .unwrap();
    let removed = f.ws.remove_project(&f.core, &none(), "p1").await.unwrap();
    assert_eq!(removed.value.connection_ids, vec![a.clone()]);
    assert!(f.store.entries().is_empty(), "its connections' secrets go");
    let err =
        f.ws.remove_project(&f.core, &none(), "p2")
            .await
            .unwrap_err();
    assert_eq!(err.code, "LAST_PROJECT");
    let err =
        f.ws.remove_project(&f.core, &none(), "p1")
            .await
            .unwrap_err();
    assert_eq!(err.code, "PROJECT_NOT_FOUND");
}

#[tokio::test]
async fn label_remove_strips_connections_in_the_same_transaction() {
    let f = fx().await;
    let label =
        f.ws.create_label(
            &f.core,
            &none(),
            "p1",
            j(json!({"name": "Team", "color": "#112233"})),
        )
        .await
        .unwrap()
        .value
        .id;
    let a = f
        .add("A", json!({"labelIds": [label, "prod"]}), json!({}))
        .await
        .unwrap();
    // A row in another project that points at it anyway (a stale id).
    insert_rows(
        f.ws.storage(),
        "connections",
        &[json!({
        "id": "c-other", "project_id": "p2", "name": "O", "type": "postgres", "host": "h",
        "port": 1, "database_name": "d", "username": "u", "save_password": 0,
        "save_ssh_password": 0, "save_ssh_key_passphrase": 0, "is_local_only": 1})],
    )
    .await;
    insert_rows(
        f.ws.storage(),
        "connection_labels",
        &[json!({"connection_id": "c-other", "label_id": label})],
    )
    .await;
    let removed =
        f.ws.remove_label(&f.core, &none(), "p1", &label)
            .await
            .unwrap();
    let mut ids = removed.value.connection_ids.clone();
    ids.sort();
    let mut want = vec![a.clone(), "c-other".to_string()];
    want.sort();
    assert_eq!(ids, want);
    let left = dump(
        f.ws.storage(),
        "connection_labels",
        "connection_id, label_id",
    )
    .await;
    assert_eq!(left, vec![json!({"connection_id": a, "label_id": "prod"})]);
}

#[tokio::test]
async fn label_remove_refuses_a_predefined_id() {
    let f = fx().await;
    let a = f
        .add("A", json!({"labelIds": ["prod", "local"]}), json!({}))
        .await
        .unwrap();
    for id in ["local", "staging", "prod"] {
        let err =
            f.ws.remove_label(&f.core, &none(), "p1", id)
                .await
                .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{id}");
        let err =
            f.ws.update_label(&f.core, &none(), "p1", id, j(json!({"name": "x"})))
                .await
                .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{id}");
    }
    assert_eq!(
        dump(f.ws.storage(), "connection_labels", "label_id")
            .await
            .len(),
        2,
        "{a} keeps both"
    );
}

#[tokio::test]
async fn an_unchanged_text_adds_no_version() {
    let f = fx().await;
    let q =
        f.ws.create_saved_query(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "name": "Q", "query": "SELECT 1"})),
        )
        .await
        .unwrap()
        .value;
    let same =
        f.ws.update_saved_query(
            &f.core,
            &none(),
            &q.id,
            j(json!({"query": "SELECT 1", "name": "R"})),
        )
        .await
        .unwrap();
    assert!(same.value.version.is_none());
    assert!(dump(f.ws.storage(), "query_versions", "version")
        .await
        .is_empty());
    // Starring alone keeps updatedAt.
    let starred =
        f.ws.update_saved_query(&f.core, &none(), &q.id, j(json!({"starred": true})))
            .await
            .unwrap();
    assert_eq!(starred.value.query.updated_at, same.value.query.updated_at);
    assert!(starred.value.query.starred);
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_are_numbered_inside_the_transaction() {
    let f = Arc::new(fx().await);
    let q =
        f.ws.create_saved_query(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "name": "Q", "query": "SELECT 0"})),
        )
        .await
        .unwrap()
        .value
        .id;
    let tasks: Vec<_> = (1..=30)
        .map(|i| {
            let (f, q) = (f.clone(), q.clone());
            tokio::spawn(async move {
                f.ws.update_saved_query(
                    &f.core,
                    &none(),
                    &q,
                    j(json!({"query": format!("SELECT {i}")})),
                )
                .await
                .unwrap()
            })
        })
        .collect();
    for t in tasks {
        t.await.unwrap();
    }
    let versions: Vec<i64> = dump(f.ws.storage(), "query_versions", "version")
        .await
        .iter()
        .map(|v| v["version"].as_i64().unwrap())
        .collect();
    assert_eq!(versions, (1..=30).collect::<Vec<_>>(), "unique and gapless");
}

#[tokio::test]
async fn web_refuses_sqlite_and_duckdb_rows() {
    let f = fx_on(web_core(LibraryLimits::default()), false).await;
    for ty in ["sqlite", "duckdb"] {
        let err = f
            .add("A", json!({"type": ty}), json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, "ENGINE_NOT_AVAILABLE", "{ty}");
    }
    let id = f
        .add("A", json!({"type": "mariadb"}), json!({}))
        .await
        .unwrap();
    let err =
        f.ws.update_connection(
            &f.core,
            &none(),
            &id,
            j(json!({"type": "sqlite"})),
            SecretChanges::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
    // The desktop has them all.
    let f = fx().await;
    for ty in ["sqlite", "duckdb", "mssql", "mysql", "mariadb", "postgres"] {
        f.add(ty, json!({"type": ty}), json!({})).await.unwrap();
    }
}

#[tokio::test]
async fn limits_refuse_before_anything_is_read() {
    let limits = LibraryLimits {
        max_name_bytes: Some(8),
        max_field_bytes: Some(16),
        max_query_bytes: Some(32),
        max_list_items: Some(2),
        ..Default::default()
    };
    let f = fx_on(core_with(limits), true).await;
    // Close the storage: a call that reads would fail with a storage code.
    f.ws.close().await;
    let o = none();
    let cases: Vec<(&str, Result<(), CoreError>)> = vec![
        (
            "name",
            f.add("123456789", json!({}), json!({})).await.map(|_| ()),
        ),
        (
            "field",
            f.add("A", json!({"host": "x".repeat(17)}), json!({}))
                .await
                .map(|_| ()),
        ),
        (
            "labels",
            f.add("A", json!({"labelIds": ["a", "b", "c"]}), json!({}))
                .await
                .map(|_| ()),
        ),
        (
            "query",
            f.ws.create_saved_query(
                &f.core,
                &o,
                j(json!({"projectId": "p1", "name": "Q", "query": "x".repeat(33)})),
            )
            .await
            .map(|_| ()),
        ),
        (
            "tags",
            f.ws.create_saved_query(
                &f.core,
                &o,
                j(json!({"projectId": "p1", "name": "Q", "query": "x", "tags": ["a", "b", "c"]})),
            )
            .await
            .map(|_| ()),
        ),
        (
            "patch",
            f.ws.update_saved_query(&f.core, &o, "q", j(json!({"query": "x".repeat(33)})))
                .await
                .map(|_| ()),
        ),
        (
            "project",
            f.ws.create_project(
                &f.core,
                &o,
                j(json!({"name": "A", "description": "x".repeat(17)})),
            )
            .await
            .map(|_| ()),
        ),
        (
            "label",
            f.ws.create_label(
                &f.core,
                &o,
                "p1",
                j(json!({"name": "123456789", "color": "#000000"})),
            )
            .await
            .map(|_| ()),
        ),
    ];
    for (what, r) in cases {
        let e = r.unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{what}: {e}");
        assert!(e.message.contains("max_"), "{what} names its limit: {e}");
    }
}

/// Phase 5d-1 probe fix: the two lists that take an id check its size
/// like every write does, before anything is read.
#[tokio::test]
async fn list_ids_are_size_checked() {
    let f = fx_on(
        core_with(LibraryLimits {
            max_name_bytes: Some(8),
            ..Default::default()
        }),
        true,
    )
    .await;
    let long = "p".repeat(9);
    f.ws.list_saved_queries(&f.core, "p1").await.unwrap();
    f.ws.list_query_versions(&f.core, "p1").await.unwrap();
    f.ws.close().await;
    let e = f.ws.list_saved_queries(&f.core, &long).await.unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT", "{e}");
    assert!(e.message.contains("max_name_bytes"), "{e}");
    let e = f.ws.list_query_versions(&f.core, &long).await.unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT", "{e}");
    assert!(e.message.contains("max_name_bytes"), "{e}");
}

/// Phase 5d-1 probe fix: with `max_version_bytes`, a saved query's
/// versions are pruned to fit it, keeping at least the newest.
#[tokio::test]
async fn versions_are_pruned_to_the_byte_budget() {
    let f = fx_on(
        core_with(LibraryLimits {
            max_version_bytes: Some(2_500),
            ..Default::default()
        }),
        true,
    )
    .await;
    let o = none();
    let q =
        f.ws.create_saved_query(
            &f.core,
            &o,
            j(json!({"projectId": "p1", "name": "Q", "query": "a".repeat(1000)})),
        )
        .await
        .unwrap()
        .value
        .id;
    let mut pruned = Vec::new();
    for c in ["b", "c", "d", "e"] {
        let done =
            f.ws.update_saved_query(&f.core, &o, &q, j(json!({"query": c.repeat(1000)})))
                .await
                .unwrap();
        pruned.extend(done.value.pruned_version_ids);
    }
    // Four versions of 1,000 bytes were stored; 2,500 bytes hold two.
    let versions = dump(f.ws.storage(), "query_versions", "version").await;
    let kept: Vec<i64> = versions
        .iter()
        .map(|v| v["version"].as_i64().unwrap())
        .collect();
    assert_eq!(kept, [3, 4]);
    assert_eq!(pruned.len(), 2);
    // A single version larger than the budget is still kept.
    let big =
        f.ws.update_saved_query(&f.core, &o, &q, j(json!({"query": "f".repeat(3000)})))
            .await
            .unwrap();
    assert!(big.value.version.is_some());
    let done =
        f.ws.update_saved_query(&f.core, &o, &q, j(json!({"query": "g"})))
            .await
            .unwrap();
    let versions = dump(f.ws.storage(), "query_versions", "version").await;
    assert_eq!(versions.len(), 1, "only the newest (3,000 bytes) stays");
    assert_eq!(done.value.pruned_version_ids.len(), 2);
}

#[tokio::test]
async fn count_limits_hold() {
    let f = fx_on(
        core_with(LibraryLimits {
            max_projects: Some(3),
            max_saved_queries: Some(1),
            ..Default::default()
        }),
        true,
    )
    .await;
    f.ws.create_project(&f.core, &none(), j(json!({"name": "Third"})))
        .await
        .unwrap();
    let e =
        f.ws.create_project(&f.core, &none(), j(json!({"name": "Fourth"})))
            .await
            .unwrap_err();
    assert!(e.message.contains("max_projects"), "{e}");
    f.ws.create_saved_query(
        &f.core,
        &none(),
        j(json!({"projectId": "p1", "name": "A", "query": "x"})),
    )
    .await
    .unwrap();
    let e =
        f.ws.create_saved_query(
            &f.core,
            &none(),
            j(json!({"projectId": "p1", "name": "B", "query": "x"})),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("max_saved_queries"), "{e}");
}

#[tokio::test]
async fn ensure_default_makes_one_project_once() {
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let first = ws.ensure_default_project(&core, &none()).await.unwrap();
    assert_eq!(first.value.len(), 1);
    assert_eq!(first.value[0].id, "default-seaquel");
    assert_eq!(first.value[0].name, "Seaquel");
    let mut events = ws.events();
    let again = ws.ensure_default_project(&core, &none()).await.unwrap();
    assert_eq!(again.value.len(), 1);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(30),
            futures::StreamExt::next(&mut events)
        )
        .await
        .is_err(),
        "nothing written, no event"
    );
}

#[tokio::test]
async fn no_names_hosts_strings_text_or_secrets_in_logs() {
    let _ = common::capture_logs();
    let f = fx().await;
    let o = none();
    let id = f
        .add(
            "canary-name",
            json!({"host": "canary-host", "username": "canary-user", "savePassword": true,
                   "connectionString": "postgres://canary-user:canary-pw@canary-host/db"}),
            json!({"db": "canary-secret"}),
        )
        .await
        .unwrap();
    f.ws.update_connection(
        &f.core,
        &o,
        &id,
        j(json!({"name": "canary-rename"})),
        SecretChanges::default(),
    )
    .await
    .unwrap();
    let _ = f
        .add("CANARY-rename", json!({}), json!({}))
        .await
        .unwrap_err();
    let q =
        f.ws.create_saved_query(
            &f.core,
            &o,
            j(json!({"projectId": "p1", "name": "canary-q", "query": "SELECT 'canary-text'"})),
        )
        .await
        .unwrap()
        .value
        .id;
    f.ws.update_saved_query(&f.core, &o, &q, j(json!({"query": "SELECT 'canary-2'"})))
        .await
        .unwrap();
    f.ws.create_label(
        &f.core,
        &o,
        "p1",
        j(json!({"name": "canary-label", "color": "#000000"})),
    )
    .await
    .unwrap();
    f.store.fail_set.store(true, Ordering::SeqCst);
    let e =
        f.ws.update_connection(
            &f.core,
            &o,
            &id,
            ConnectionPatch::default(),
            j(json!({"db": "canary-secret-2"})),
        )
        .await
        .unwrap_err();
    let draft = conn_draft(
        "p1",
        "canary-name",
        json!({"host": "canary-host", "connectionString": "canary-cs"}),
    );
    let text = format!(
        "{}\n{e:?} {draft:?} {:?} {:?}",
        common::logged(),
        j::<ConnectionPatch>(json!({"name": "canary-n", "host": "canary-h"})),
        j::<SecretChanges>(json!({"db": "canary-s", "ssh": null})),
    );
    assert!(!text.contains("canary"), "{text}");
}

#[tokio::test]
async fn a_partly_failed_secret_update_keeps_the_working_password() {
    let f = fx().await;
    let id = f
        .add(
            "A",
            json!({"savePassword": true}),
            json!({"db": "working-pw"}),
        )
        .await
        .unwrap();
    let before = f.row(&id).await.unwrap();
    *f.store.fail_set_key.lock().unwrap() = Some(format!("ssh:{id}"));
    let err =
        f.ws.update_connection(
            &f.core,
            &none(),
            &id,
            j(json!({"saveSshPassword": true})),
            j(json!({"db": "new-pw", "ssh": "ssh-pw"})),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_STORE_ERROR");
    assert_eq!(
        f.store.entries(),
        [(format!("db:{id}"), "working-pw".to_string())]
            .into_iter()
            .collect(),
        "the db entry set before the failure gets its old value back"
    );
    assert_eq!(f.row(&id).await.unwrap(), before);

    // A secret that wasn't there before is deleted again.
    let fresh = f.add("B", json!({}), json!({})).await.unwrap();
    *f.store.fail_set_key.lock().unwrap() = Some(format!("ssh:{fresh}"));
    f.ws.update_connection(
        &f.core,
        &none(),
        &fresh,
        j(json!({"savePassword": true, "saveSshPassword": true})),
        j(json!({"db": "x", "ssh": "y"})),
    )
    .await
    .unwrap_err();
    assert!(!f.store.entries().contains_key(&format!("db:{fresh}")));
}
