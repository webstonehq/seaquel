//! Task 1's TypeScript baseline replayed through Core (phase 6 Task 4):
//! every case of `turns.json`, `page.json`, `generate.json` and
//! `models.json` runs against the mock provider and the scripted database,
//! and what Core sends, runs, stores, answers and logs must equal the
//! recording with `changes.json`'s `expected` fields in its place.
//!
//! What isn't compared, and why:
//! - the page's own view (`messages`, `allowAllAfter`, `dashboardCalls`,
//!   the inline prompt's `executed`, `notice`, `error` and `toasts`):
//!   Task 7's, which words a failed turn and runs the page's tools;
//! - `chunks`: `*` rule 2 compares their concatenation, `text`;
//! - `providerClosed` is checked as the mock seeing the client go;
//! - a case's `logs.$allowed` is documentation: only `$forbidden` is
//!   enforced (`ai_turn.rs`'s log test checks usage lines hold numbers
//!   only).
//!
//! The harness plays the page: it answers each approval from the case's
//! script, answers a client tool with the result the expected next
//! request carries for that call, presses Stop where the case does, and
//! keeps "Allow all" per connection.

#![cfg(all(feature = "ai", feature = "ai-native"))]
// Native-only tests: the shared helpers read the system clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod ai_support;
mod common;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ai_support::*;
use seaquel_ai::testing::{Reply, SseEnd, TEST_KEY};
use seaquel_ai::wire::ProviderKind;
use seaquel_core::ai::{AiDecision, AiEvent, Approval, ApprovalDecision, ClientResult};
use seaquel_core::{StoredKind, WorkspaceEvent};
use seaquel_types::SchemaTable;
use serde_json::{json, Value as Json};

const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-ai/tests/fixtures/ts-baseline"
);
const SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-ai/tests/fixtures/tool-schemas.json"
);

fn read(path: &str) -> Json {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn cases(file: &str) -> Vec<Json> {
    read(&format!("{DIR}/{file}.json"))
        .as_array()
        .unwrap()
        .clone()
}

fn changes() -> &'static serde_json::Map<String, Json> {
    static CHANGES: OnceLock<serde_json::Map<String, Json>> = OnceLock::new();
    CHANGES.get_or_init(|| {
        read(&format!("{DIR}/changes.json"))
            .as_object()
            .unwrap()
            .clone()
    })
}

fn inputs() -> &'static Json {
    static INPUTS: OnceLock<Json> = OnceLock::new();
    INPUTS.get_or_init(|| read(&format!("{DIR}/inputs.json")))
}

/// A case's field as Rust must produce it: `changes.json`'s value when it
/// lists one, else the recording's.
fn expected(file: &str, case: &Json, field: &str) -> Json {
    let key = format!("{file}/{}", case["name"].as_str().unwrap());
    if let Some(v) = changes()
        .get(&key)
        .and_then(|c| c["expected"].as_object())
        .and_then(|e| e.get(field))
    {
        return v.clone();
    }
    case[field].clone()
}

fn is_absent(v: &Json) -> bool {
    v.get("$absent").is_some()
}

/// `*` rule 6: `{"$toolSchemas": …}` as the provider's tool list.
fn expand(v: &Json) -> Json {
    match v {
        Json::Object(map) if map.contains_key("$toolSchemas") => {
            let spec = &map["$toolSchemas"];
            let all = read(SCHEMAS);
            let defs = all[spec["profile"].as_str().unwrap()].as_array().unwrap();
            let shape = spec["shape"].as_str().unwrap();
            Json::Array(
                spec["names"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| {
                        let d = defs.iter().find(|d| d["name"] == *n).unwrap();
                        match shape {
                            "anthropic" => d.clone(),
                            _ => json!({"type": "function", "function": {
                                "name": d["name"], "description": d["description"],
                                "parameters": d["input_schema"]}}),
                        }
                    })
                    .collect(),
            )
        }
        Json::Object(map) => {
            Json::Object(map.iter().map(|(k, v)| (k.clone(), expand(v))).collect())
        }
        Json::Array(items) => Json::Array(items.iter().map(expand).collect()),
        other => other.clone(),
    }
}

fn sent_json(sent: &[Sent]) -> Json {
    Json::Array(
        sent.iter()
            .map(
                |s| json!({"method": s.method, "url": s.url, "headers": s.headers, "body": s.body}),
            )
            .collect(),
    )
}

fn schema_of(v: &Json) -> Vec<SchemaTable> {
    let items = match v {
        Json::String(name) => inputs()[name.as_str()].clone(),
        Json::Null => json!([]),
        other => other.clone(),
    };
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let mut t = t.clone();
            if t.get("indexes").is_none() {
                t["indexes"] = json!([]);
            }
            serde_json::from_value(t).unwrap()
        })
        .collect()
}

fn kind_of(provider: &Json) -> ProviderKind {
    match provider["type"].as_str() {
        Some("openai-compatible") => ProviderKind::OpenAiCompatible,
        _ => ProviderKind::Anthropic,
    }
}

/// Queue a case's scripted responses on the world's client and mock.
fn script(w: &World, responses: &Json, kind: ProviderKind) {
    for r in responses.as_array().unwrap() {
        if r["networkError"] == json!(true) {
            w.http.script(Action::NetworkError, None);
            continue;
        }
        let body = renamed_round(r["body"].as_str().unwrap_or(""), kind);
        let status = r["status"].as_u64().unwrap_or(200) as u16;
        let content_type = r["contentType"].as_str().unwrap_or("text/event-stream");
        let reply = if r["stall"] == json!(true) {
            sse(&body, SseEnd::Stall)
        } else if status == 200 && content_type == "text/event-stream" {
            sse(&body, SseEnd::Finish)
        } else {
            Reply::Raw {
                status,
                headers: vec![("content-type".into(), content_type.into())],
                body: body.into_bytes(),
            }
        };
        w.http.script(Action::Forward, Some(reply));
    }
}

/// `$forbidden` against every log line this test binary made.
fn check_logs(file: &str, case: &Json) {
    let want = expected(file, case, "logs");
    let logged = common::logged();
    for f in want["$forbidden"].as_array().unwrap() {
        let f = f.as_str().unwrap().replace("$TEST_KEY", TEST_KEY);
        assert!(
            !logged.contains(&f),
            "{file}/{}: a log line holds {f:?}",
            case["name"]
        );
    }
}

/// The result a client tool's call got, from the next request the case
/// expects: `(content, is_error)`.
fn client_answer(requests: &Json, call_id: &str) -> (String, bool) {
    for req in requests.as_array().unwrap() {
        for m in req["body"]["messages"].as_array().into_iter().flatten() {
            // Anthropic: a user message of tool_result blocks.
            for block in m["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result" && block["tool_use_id"] == call_id {
                    return (
                        block["content"].as_str().unwrap().to_string(),
                        block["is_error"] == json!(true),
                    );
                }
            }
            // OpenAI: one tool message each.
            if m["role"] == "tool" && m["tool_call_id"] == call_id {
                let text = m["content"].as_str().unwrap();
                return match text.strip_prefix("Error: ") {
                    Some(rest) => (rest.to_string(), true),
                    None => (text.to_string(), false),
                };
            }
        }
    }
    panic!("no expected request carries {call_id}'s result")
}

/// What the page answers a waiting turn with.
struct Page<'a> {
    approvals: Vec<String>,
    requests: &'a Json,
    stop_at_first_text: bool,
    seen: Vec<String>,
    allow_all: bool,
}

impl Page<'_> {
    fn answer(&mut self, event: &AiEvent) -> Answer {
        match event {
            AiEvent::Text { .. } if self.stop_at_first_text => {
                self.stop_at_first_text = false;
                Answer::Cancel
            }
            AiEvent::ApprovalRequired { sql, .. } => {
                self.seen.push(sql.clone());
                let next = if self.approvals.is_empty() {
                    "allow".to_string()
                } else {
                    self.approvals.remove(0)
                };
                match next.as_str() {
                    "allow" => Answer::Decide(AiDecision::Approval(ApprovalDecision::Allow)),
                    "deny" => Answer::Decide(AiDecision::Approval(ApprovalDecision::Deny)),
                    "allowAll" => {
                        self.allow_all = true;
                        Answer::Decide(AiDecision::Approval(ApprovalDecision::AllowAll))
                    }
                    "stop" => Answer::Cancel,
                    other => panic!("approval {other}"),
                }
            }
            AiEvent::ClientTool { call_id, .. } => {
                let (result, is_error) = client_answer(self.requests, call_id);
                Answer::Decide(AiDecision::Client(ClientResult { result, is_error }))
            }
            _ => Answer::Nothing,
        }
    }
}

/// `outcome` as the recording writes it.
fn outcome(events: &[AiEvent]) -> Json {
    match terminal(events) {
        Some(AiEvent::Done { stop, .. }) => json!({"kind": "done", "stop": stop}),
        Some(AiEvent::Error { code, message, .. }) => {
            json!({"kind": "error", "code": code, "message": message})
        }
        _ => json!({"kind": "cancelled"}),
    }
}

fn queries_json(ran: &[Ran], with_connection: bool) -> Json {
    Json::Array(
        ran.iter()
            .map(|r| {
                let mut q = json!({"sql": r.sql, "maxRows": r.max_rows,
                                   "maxBytes": r.max_bytes, "timeoutMs": r.timeout_ms});
                if with_connection {
                    q["connectionId"] = json!(r.saved);
                }
                q
            })
            .collect(),
    )
}

// ── turns.json ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn turns_replay() {
    common::capture_logs();
    let all = cases("turns");
    assert_eq!(all.len(), 73);
    for case in &all {
        replay_turn(case).await;
    }
}

async fn replay_turn(case: &Json) {
    let name = case["name"].as_str().unwrap();
    let input = &case["input"];
    let provider = &input["provider"];
    let kind = kind_of(provider);
    let mut conn = Conn::new("conn-1", input["engine"].as_str().unwrap());
    conn.share_schema = input["shareSchema"].as_bool();
    conn.share_data = input["shareData"].as_bool();
    conn.model = input["model"].as_str().map(String::from);
    let w = world(Setup {
        conns: vec![conn],
        providers: if provider.is_null() {
            vec![]
        } else {
            vec![provider.clone()]
        },
        key: Some(
            input["key"]
                .as_bool()
                .unwrap()
                .then(|| TEST_KEY.to_string()),
        ),
        ..Setup::default()
    })
    .await;
    *w.db.schema.lock().unwrap() = schema_of(&input["schema"]);
    if let Some(answers) = input["answers"].as_object() {
        w.db.answers
            .lock()
            .unwrap()
            .extend(answers.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let id = w.connect("conn-1").await;
    w.chat("chat-1", "conn-1").await;
    let messages = input["messages"].as_array().unwrap();
    for (i, m) in messages[..messages.len() - 1].iter().enumerate() {
        w.seed_message(
            "chat-1",
            &format!("h{i}"),
            m["role"].as_str().unwrap(),
            m["content"].as_str().unwrap(),
            i as u32,
        )
        .await;
    }
    script(&w, &case["responses"], kind);
    let requests = expand(&expected("turns", case, "requests"));
    let mut p = params(
        "s1",
        &id,
        messages.last().unwrap()["content"].as_str().unwrap(),
    );
    p.client_tools = input["dashboards"] == json!(true);
    if input["allowAll"] == json!(true) {
        p.approval = Approval::AllowAll;
    }
    let mut page = Page {
        approvals: input["approvals"]
            .as_array()
            .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
            .unwrap_or_default(),
        requests: &requests,
        stop_at_first_text: input["stopAtFirstChunk"] == json!(true),
        seen: Vec::new(),
        allow_all: false,
    };
    let events = run_turn(&w, p, |e| page.answer(e)).await;

    assert_eq!(
        sent_json(&w.http.sent()),
        requests,
        "turns/{name}: requests"
    );
    assert_eq!(
        outcome(&events),
        expected("turns", case, "outcome"),
        "turns/{name}: outcome"
    );
    assert_eq!(
        json!(text_of(&events)),
        expected("turns", case, "text"),
        "turns/{name}: text"
    );
    assert_eq!(
        json!(page.seen),
        expected("turns", case, "approvals"),
        "turns/{name}: approvals"
    );
    assert_eq!(
        queries_json(&w.db.ran(), false),
        expected("turns", case, "queries"),
        "turns/{name}: queries"
    );
    if case["providerClosed"] == json!([true]) {
        assert!(
            w.mock.client_gone(Duration::from_secs(5)).await,
            "turns/{name}: the provider's response is dropped"
        );
    }
    check_logs("turns", case);
}

// ── page.json ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn page_replay() {
    common::capture_logs();
    let all = cases("page");
    assert_eq!(all.len(), 21);
    for case in &all {
        replay_page(case).await;
    }
}

/// The page's rows as `stored` writes them.
fn stored_json(rows: &[seaquel_types::storage::PersistedAIMessage]) -> Json {
    Json::Array(
        rows.iter()
            .map(|m| {
                let mut v = json!({"role": m.role, "content": m.content});
                if let Some(d) = &m.dashboard_id {
                    v["dashboardId"] = json!(d);
                }
                if let Some(p) = &m.parts {
                    v["parts"] = p.clone();
                }
                v
            })
            .collect(),
    )
}

async fn replay_page(case: &Json) {
    let name = case["name"].as_str().unwrap();
    let input = &case["input"];
    let provider = match input["provider"].as_str() {
        Some("openai") => json!({"id": "prov-1", "name": "OpenAI", "type": "openai-compatible"}),
        _ => json!({"id": "prov-1", "name": "Anthropic", "type": "anthropic"}),
    };
    let kind = kind_of(&provider);
    let conns: Vec<Conn> = input["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Conn {
            id: c["id"].as_str().unwrap().into(),
            name: c["name"].as_str().unwrap().into(),
            ty: c["type"].as_str().unwrap().into(),
            share_schema: c["aiShareSchema"].as_bool(),
            share_data: c["aiShareData"].as_bool(),
            provider: c["activeAIProviderId"].as_str().map(String::from),
            model: c["activeAIModel"].as_str().map(String::from),
        })
        .collect();
    let names: HashMap<String, String> = conns
        .iter()
        .map(|c| (c.id.clone(), c.name.clone()))
        .collect();
    let w = world(Setup {
        conns: conns.clone(),
        providers: vec![provider],
        share_schema_globally: input["global"]["shareSchemaGlobally"].as_bool().unwrap(),
        share_data_globally: input["global"]["shareDataGlobally"].as_bool().unwrap(),
        key: Some((input["noKey"] != json!(true)).then(|| TEST_KEY.to_string())),
        ..Setup::default()
    })
    .await;
    *w.db.schema.lock().unwrap() = schema_of(&input["schema"]);
    if let Some(answers) = input["answers"].as_object() {
        w.db.answers
            .lock()
            .unwrap()
            .extend(answers.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    seed_library(&w).await;
    if input["dataOffAfterFirstQuery"] == json!(true) {
        let ws = w.ws.clone();
        *w.db.after_query.lock().unwrap() = Some(Arc::new(move |_ran: &Ran| {
            let ws = ws.clone();
            Box::pin(async move {
                sqlx::query("UPDATE connections SET ai_share_data = 0")
                    .execute(ws.storage().pool())
                    .await
                    .unwrap();
            })
        }));
    }
    // One chat per connection, each open.
    let mut open: HashMap<String, String> = HashMap::new();
    for c in &conns {
        open.insert(c.id.clone(), w.connect(&c.id).await);
        w.chat(&format!("chat-{}", c.id), &c.id).await;
    }
    let mut active = input["active"].as_str().unwrap().to_string();
    // The seeded history: only what a completed send stored (a pending
    // model selection and the question before it never were).
    let history = input["history"].as_array().cloned().unwrap_or_default();
    let mut n = 0;
    for (i, m) in history.iter().enumerate() {
        let pending = |j: usize| {
            history
                .get(j)
                .is_some_and(|h| h.get("pendingModelSelection").is_some())
        };
        if pending(i) || pending(i + 1) {
            continue;
        }
        let id = format!("h{i}");
        w.seed_message(
            &format!("chat-{active}"),
            &id,
            m["role"].as_str().unwrap(),
            m["content"].as_str().unwrap(),
            n,
        )
        .await;
        if let Some(d) = m["dashboardId"].as_str() {
            sqlx::query("UPDATE ai_messages SET dashboard_id = ? WHERE id = ?")
                .bind(d)
                .bind(&id)
                .execute(w.ws.storage().pool())
                .await
                .unwrap();
        }
        n += 1;
    }
    if input["removeConnection"] == json!(true) {
        sqlx::query("DELETE FROM connections WHERE id = ?")
            .bind(&active)
            .execute(w.ws.storage().pool())
            .await
            .unwrap();
    }
    script(&w, &case["responses"], kind);
    let requests = expand(&expected("page", case, "requests"));
    let mut events_rx = w.ws.events();
    let mut allow_all: BTreeMap<String, bool> = BTreeMap::new();
    let mut approvals = Vec::new();
    let mut last_error = Json::Null;
    for (i, step) in input["steps"].as_array().unwrap().iter().enumerate() {
        if let Some(c) = step["activate"].as_str() {
            active = c.to_string();
            continue;
        }
        let content = step["send"].as_str().unwrap();
        let mut p = params(&format!("s{i}"), &open[&active], content);
        p.chat_id = format!("chat-{active}");
        p.client_tools = true;
        if allow_all.get(&active).copied().unwrap_or(false) {
            p.approval = Approval::AllowAll;
        }
        let mut page = Page {
            approvals: step["approvals"]
                .as_array()
                .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
                .unwrap_or_default(),
            requests: &requests,
            stop_at_first_text: step["stopAtFirstChunk"] == json!(true),
            seen: Vec::new(),
            allow_all: false,
        };
        let events = run_turn(&w, p, |e| page.answer(e)).await;
        if page.allow_all {
            allow_all.insert(active.clone(), true);
        }
        approvals.extend(
            page.seen
                .into_iter()
                .map(|q| json!({"query": q, "connectionName": names[&active]})),
        );
        last_error = match terminal(&events) {
            Some(AiEvent::Error { code, message, .. }) => json!({"code": code, "message": message}),
            _ => Json::Null,
        };
    }

    assert_eq!(sent_json(&w.http.sent()), requests, "page/{name}: requests");
    assert_eq!(
        queries_json(&w.db.ran(), true),
        expected("page", case, "queries"),
        "page/{name}: queries"
    );
    assert_eq!(
        json!(approvals),
        expected("page", case, "approvals"),
        "page/{name}: approvals"
    );
    let want_stored = expected("page", case, "stored");
    let stored = stored_json(&w.messages(&format!("chat-{active}")).await);
    if want_stored.is_null() {
        assert_eq!(stored, json!([]), "page/{name}: stored");
    } else {
        assert_eq!(stored, want_stored, "page/{name}: stored");
    }
    let mut store_calls = 0;
    for e in drain(&mut events_rx) {
        if let WorkspaceEvent::StorageChanged(c) = e {
            if c.kind == StoredKind::ChatMessages {
                store_calls += 1;
            }
        }
    }
    assert_eq!(
        json!(store_calls),
        expected("page", case, "storeCalls"),
        "page/{name}: storeCalls"
    );
    let want_error = expected("page", case, "error");
    if !want_error.is_null() {
        assert_eq!(last_error, want_error, "page/{name}: error");
    }
    check_logs("page", case);
}

/// The project's saved queries and dashboards (`SAVED`, `DASHBOARDS`).
async fn seed_library(w: &World) {
    for q in inputs()["SAVED"].as_array().unwrap() {
        common::insert_rows(
            w.ws.storage(),
            "saved_queries",
            &[
                json!({"id": q["id"], "project_id": "p1", "name": q["name"], "query": q["query"],
                     "created_at": T0, "updated_at": T0}),
            ],
        )
        .await;
    }
    for d in inputs()["DASHBOARDS"].as_array().unwrap() {
        common::insert_rows(
            w.ws.storage(),
            "dashboards",
            &[json!({"id": d["id"], "project_id": "p1", "name": d["name"],
                     "widgets": d["widgets"].to_string(), "created_at": T0, "updated_at": T0})],
        )
        .await;
    }
}

// ── generate.json ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn generate_replay() {
    common::capture_logs();
    let all = cases("generate");
    assert_eq!(all.len(), 21);
    for case in &all {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let provider = &input["provider"];
        let mut conn = Conn::new("conn-1", input["engine"].as_str().unwrap());
        conn.share_schema = input["shareSchema"].as_bool();
        conn.model = input["model"].as_str().map(String::from);
        // The recording's connection named a provider only when one was
        // configured (the README's "The chat's provider when none is
        // configured"; `turns/none/no-provider` names one that's gone).
        conn.provider = (!provider.is_null()).then(|| "prov-1".to_string());
        let w = world(Setup {
            conns: vec![conn],
            providers: if provider.is_null() {
                vec![]
            } else {
                vec![provider.clone()]
            },
            key: Some(
                input["key"]
                    .as_bool()
                    .unwrap()
                    .then(|| TEST_KEY.to_string()),
            ),
            ..Setup::default()
        })
        .await;
        *w.db.schema.lock().unwrap() = schema_of(&input["schema"]);
        w.connect("conn-1").await;
        script(&w, &case["responses"], kind_of(provider));
        let result =
            w.ws.ai_generate(
                &w.core,
                seaquel_core::ai::GenerateParams {
                    connection_id: "conn-1".into(),
                    request: input["request"].as_str().unwrap().into(),
                    existing_query: input["existingQuery"].as_str().unwrap().into(),
                    api_key: None,
                    provider_id: None,
                },
            )
            .await;
        let requests = expand(&expected("generate", case, "requests"));
        assert_eq!(
            sent_json(&w.http.sent()),
            requests,
            "generate/{name}: requests"
        );
        match result {
            Ok(sql) => {
                assert_eq!(
                    json!([sql]),
                    expected("generate", case, "inserted"),
                    "generate/{name}"
                );
            }
            Err(e) => {
                assert_eq!(
                    json!([]),
                    expected("generate", case, "inserted"),
                    "generate/{name}"
                );
                assert_eq!(
                    json!(e.code),
                    expected("generate", case, "code"),
                    "generate/{name}"
                );
                assert_eq!(
                    json!(e.message),
                    expected("generate", case, "message"),
                    "generate/{name}"
                );
            }
        }
        check_logs("generate", case);
    }
}

// ── models.json ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn models_replay() {
    common::capture_logs();
    let all = cases("models");
    assert_eq!(all.len(), 18);
    for case in &all {
        let name = case["name"].as_str().unwrap();
        let input = &case["input"];
        let provider = &input["provider"];
        let w = world(Setup {
            providers: vec![provider.clone()],
            key: Some(
                input["key"]
                    .as_bool()
                    .unwrap()
                    .then(|| TEST_KEY.to_string()),
            ),
            ..Setup::default()
        })
        .await;
        script(&w, &case["responses"], kind_of(provider));
        let (result, error) = if name.starts_with("models/") {
            match w.ws.ai_models(&w.core, "prov-1", None).await {
                Ok(list) => (json!(list), Json::Null),
                Err(e) => (Json::Null, json!({"code": e.code, "message": e.message})),
            }
        } else {
            match w.ws.ai_test(&w.core, "prov-1", None).await {
                Ok(()) => (Json::Null, Json::Null),
                Err(e) => (Json::Null, json!({"code": e.code, "message": e.message})),
            }
        };
        let requests = expand(&expected("models", case, "requests"));
        assert_eq!(
            sent_json(&w.http.sent()),
            requests,
            "models/{name}: requests"
        );
        let want_result = expected("models", case, "result");
        let want_error = expected("models", case, "error");
        if is_absent(&want_result) {
            assert_eq!(error, want_error, "models/{name}: error");
        } else {
            assert_eq!(result, want_result, "models/{name}: result");
            assert!(error.is_null(), "models/{name}: {error}");
        }
        assert_eq!(
            json!(w.http.unused()),
            expected("models", case, "unusedResponses"),
            "models/{name}: unused responses"
        );
        check_logs("models", case);
    }
}

#[test]
fn every_change_for_these_files_names_a_replayed_case() {
    for file in ["turns", "page", "generate", "models"] {
        let names: Vec<String> = cases(file)
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        for key in changes().keys() {
            if let Some(case) = key.strip_prefix(&format!("{file}/")) {
                assert!(names.iter().any(|n| n == case), "{key} names no case");
            }
        }
    }
}
