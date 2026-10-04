//! Task 7 against a real Core: Ask AI's `ai_generate` through the TUI's own
//! Core, whose model client is wrapped in `LoopbackOnly` in every test build
//! (`runtime::core::build_core`), answering from `seaquel-ai`'s
//! `MockProvider` on 127.0.0.1. The API key is the fake `test-key-not-real`
//! from a `SEAQUEL_TUI_TEST_SECRETS` file, read through `TestHooks` as the
//! binary reads it. Never a real provider or key.
//!
//! The library's AI flags (Core's sharing rule), the request as typed with
//! Core resolving its `@mentions`, Ctrl+R running only the inserted SELECT
//! (and not an UPDATE), errors worded, Esc dropping the request (the mock
//! sees the client go), and the key, prompt and SQL in no log line or
//! `Debug`.

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::KeyCode;
use seaquel_ai::testing::{MockProvider, Reply, TEST_KEY};
use seaquel_core::domain::library::ConnectionPatch;
use seaquel_core::secrets::SecretStore;
use seaquel_core::WriteOrigin;
use serde_json::json;

use super::browse_tests::seed;
use crate::state::app::{Conn, Effect, Load, Modal, Model};
use crate::state::ask::{self, Stage};
use crate::state::editor::Normal;
use crate::state::query;
use crate::state::text;
use crate::testing::core::Seed;
use crate::testing::harness::{Harness, HarnessOptions};
use crate::testing::keys::{ctrl, key};

/// The shop seed with an OpenAI-compatible provider at the mock, chosen for
/// the `shop` connection with a model, and the connection's data sharing
/// turned on (schema sharing stays the default).
async fn ai_seed(mock: &MockProvider) -> (Seed, String) {
    let seed = seed().await;
    let url = format!("{}/v1", mock.url());
    let provider = seed
        .with(|core, ws| async move {
            let origin = WriteOrigin::new(Some("app-window"));
            let draft = serde_json::from_value(json!({
                "name": "Mock", "type": "openai-compatible", "baseUrl": url
            }))
            .unwrap();
            let provider = ws
                .create_ai_provider(&core, &origin, draft, None)
                .await
                .unwrap()
                .value
                .id;
            let conn = ws.list_connections().await.unwrap().value[0].id.clone();
            let patch = ConnectionPatch {
                active_ai_provider_id: Some(Some(provider.clone())),
                active_ai_model: Some(Some("mock-model".into())),
                ai_share_data: Some(Some(true)),
                ..ConnectionPatch::default()
            };
            let project = ws.list_connections().await.unwrap().value[0]
                .project_id
                .clone();
            ws.update_connection(&core, &origin, &conn, patch, Default::default())
                .await
                .unwrap();
            let board = serde_json::from_value(json!({
                "projectId": project, "name": "Revenue board", "widgets": [],
                "viewport": {"x": 0, "y": 0, "zoom": 1}
            }))
            .unwrap();
            ws.create_dashboard(&core, &origin, board).await.unwrap();
            provider
        })
        .await;
    (seed, provider)
}

/// The test hook's store, from a JSON file as the binary reads it.
async fn hook_store(dir: &std::path::Path, provider: &str) -> Arc<dyn SecretStore> {
    let path = dir.join("secrets.json");
    std::fs::write(
        &path,
        json!({ format!("ai-api-key:{provider}"): TEST_KEY }).to_string(),
    )
    .unwrap();
    let path = path.to_string_lossy().into_owned();
    seaquel_terminal::TestHooks::from_lookup(crate::TEST_HOOKS_PREFIX, |name| {
        (name == "SEAQUEL_TUI_TEST_SECRETS").then(|| path.clone())
    })
    .secret_store()
    .await
    .unwrap()
}

/// The TUI on the seed, connected to `shop`, with a query tab holding
/// `text` (the cursor at its end).
async fn open(seed: &Seed, provider: &str, text: &str) -> Harness {
    let secrets = tempfile::tempdir().unwrap();
    let store = hook_store(secrets.path(), provider).await;
    let mut h = Harness::open(HarnessOptions {
        connection: Some("shop"),
        ..HarnessOptions::new(seed.path(), store)
    })
    .await;
    h.until("connected and listed", |m| {
        matches!(m.conn, Conn::Connected { .. }) && m.schema_load == Load::Loaded
    })
    .await;
    h.send(key('Q'));
    let tab = h.model.query.active_mut().unwrap();
    tab.editor.set_text(text);
    tab.editor.normal(Normal::Bottom);
    tab.editor.normal(Normal::LineEnd);
    query::sync(&mut h.model);
    h
}

fn stage(m: &Model) -> Option<Stage> {
    match &m.modal {
        Some(Modal::Ask(a)) => Some(a.stage),
        _ => None,
    }
}

fn the_ask(m: &Model) -> &ask::Ask {
    match &m.modal {
        Some(Modal::Ask(a)) => a,
        other => panic!("not asking: {other:?}"),
    }
}

fn openai_sql(sql: &str) -> Reply {
    Reply::json(
        200,
        &json!({"choices": [{"index": 0, "message": {"role": "assistant",
                "content": format!("```sql\n{sql}\n```")}}]}),
    )
}

fn idle(m: &Model) -> bool {
    !m.running()
}

/// Asks `request` and waits for the answer (or the error).
async fn ask_for(h: &mut Harness, request: &str) {
    h.send(ctrl('k'));
    h.keys(request);
    h.press(KeyCode::Enter);
    h.until("the answer", |m| {
        matches!(stage(m), Some(Stage::Answer))
            || m.modal
                .as_ref()
                .is_some_and(|_| matches!(&m.modal, Some(Modal::Ask(a)) if a.error.is_some()))
    })
    .await;
}

#[tokio::test]
async fn the_library_carries_core_s_sharing_and_the_model() {
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let mut h = open(&seed, &provider, "").await;
    let conn = h
        .model
        .library
        .connection(h.model.conn.id().unwrap())
        .unwrap();
    assert_eq!(
        (conn.ai.schema, conn.ai.data, conn.ai.model.as_deref()),
        (true, true, Some("mock-model"))
    );
    assert!(!h.model.library.ai_off);
    h.send(ctrl('k'));
    assert_eq!(
        ask::sharing_line(&h.model),
        "schema shared · data shared · read-only"
    );
    // The project's dashboards come for `@`.
    h.until(
        "the dashboards",
        |m| matches!(&m.modal, Some(Modal::Ask(a)) if a.dashboards == ["Revenue board"]),
    )
    .await;
    // Turned off in the app: the line says so after the next library read.
    seed.with(|core, ws| async move {
        let patch = serde_json::from_value(json!({"enabled": false})).unwrap();
        ws.patch_ai_settings(&core, &WriteOrigin::new(Some("app-window")), patch)
            .await
            .unwrap();
    })
    .await;
    h.until("the external change", |m| m.library.ai_off).await;
    assert_eq!(ask::sharing_line(&h.model), text::ASK_AI_OFF);
    h.close().await;
}

#[tokio::test]
async fn ask_sends_the_request_as_typed_and_ctrl_r_runs_only_the_inserted_select() {
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let mut h = open(&seed, &provider, "DELETE FROM events;").await;
    mock.reply(openai_sql("SELECT count(*) AS n FROM invoices"));
    ask_for(&mut h, "how many in @invoices ?").await;
    let a = the_ask(&h.model);
    assert_eq!(
        a.answer.as_ref().map(|x| x.sql.as_str()),
        Some("SELECT count(*) AS n FROM invoices")
    );
    // What reached the mock: the key from the hook's store, the request as
    // typed with Core's mention context, the editor's text as context.
    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(r.path, "/v1/chat/completions");
    assert_eq!(
        r.header("authorization"),
        Some(format!("Bearer {TEST_KEY}").as_str())
    );
    let body = r.json();
    assert_eq!(body["model"], "mock-model");
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(
        user.starts_with("how many in @invoices ?\n\nReferenced context:\n"),
        "{user}"
    );
    assert!(user.contains("Table: main.invoices"), "{user}");
    assert!(
        user.ends_with("Existing query for context:\n```sql\nDELETE FROM events;\n```"),
        "{user}"
    );

    // Ctrl+R: inserted as its own statement, and only it runs.
    h.effects.clear();
    h.send(ctrl('r'));
    assert!(h.model.modal.is_none());
    h.until("the run", idle).await;
    let tab = h.model.query.active().unwrap();
    assert_eq!(
        tab.editor.text(),
        "DELETE FROM events;\n\nSELECT count(*) AS n FROM invoices;"
    );
    assert_eq!(tab.statements.len(), 1);
    let page = tab.statements[0].page.as_ref().unwrap();
    assert_eq!(page.columns, ["n"]);
    assert_eq!(crate::state::grid::display(&page.rows[0][0]), "150");
    // The DELETE before it didn't run.
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let left = h
        .session
        .ws
        .query(
            &h.session.core,
            &core_id,
            "SELECT count(*) FROM events",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(crate::state::grid::display(&left.rows[0][0]), "1");
    h.close().await;
}

#[tokio::test]
async fn an_update_is_inserted_and_not_run() {
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let mut h = open(&seed, &provider, "").await;
    mock.reply(openai_sql("UPDATE invoices SET total = 0"));
    ask_for(&mut h, "zero every total").await;
    h.effects.clear();
    h.send(ctrl('r'));
    assert!(
        !h.effects.iter().any(|e| matches!(e, Effect::Run(_))),
        "{:?}",
        h.effects
    );
    assert_eq!(
        the_ask(&h.model).note.as_deref(),
        Some(text::ask_not_run("UPDATE").as_str())
    );
    assert_eq!(
        h.model.query.active().unwrap().editor.text(),
        "UPDATE invoices SET total = 0"
    );
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let rows = h
        .session
        .ws
        .query(
            &h.session.core,
            &core_id,
            "SELECT count(*) FROM invoices WHERE total = 0",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(crate::state::grid::display(&rows.rows[0][0]), "0");
    h.close().await;
}

#[tokio::test]
async fn errors_are_worded_and_an_echoed_key_is_redacted() {
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let mut h = open(&seed, &provider, "").await;
    mock.reply(Reply::json(
        429,
        &json!({"error": {"message": "slow down"}}),
    ));
    ask_for(&mut h, "anything").await;
    let error = the_ask(&h.model).error.clone().unwrap();
    assert_eq!(error, text::ask_error("RATE_LIMITED", ""));
    // A provider's message that echoes the key: Core redacts it.
    mock.reply(Reply::json(
        400,
        &json!({"error": {"message": format!("bad key {TEST_KEY}")}}),
    ));
    h.press(KeyCode::Enter);
    h.until("the second answer", |m| {
        matches!(&m.modal, Some(Modal::Ask(a)) if a.stage == Stage::Prompt
            && a.error.as_deref() != Some(error.as_str()))
    })
    .await;
    let error = the_ask(&h.model).error.clone().unwrap();
    assert!(error.contains("refused the request"), "{error}");
    assert!(error.contains("<redacted>"), "{error}");
    assert!(!error.contains(TEST_KEY), "{error}");
    h.close().await;
}

#[tokio::test]
async fn esc_while_waiting_drops_the_request() {
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let mut h = open(&seed, &provider, "").await;
    mock.reply(Reply::Hang);
    h.send(ctrl('k'));
    h.keys("never answered");
    h.press(KeyCode::Enter);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while mock.connections() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the request never left"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.press(KeyCode::Esc);
    assert!(
        mock.client_gone(Duration::from_secs(10)).await,
        "the request wasn't dropped"
    );
    assert_eq!(the_ask(&h.model).note.as_deref(), Some(text::ASK_STOPPED));
    h.close().await;
}

/// The key, the prompt and the SQL never reach the log file (at `trace`,
/// Core's lines and the TUI's) or the model's and effects' `Debug`.
#[tokio::test]
async fn the_key_prompt_and_sql_reach_no_log_line_or_debug() {
    const PROMPT: &str = "prompt-marker-7a";
    const SQL: &str = "SELECT 'sql-marker-7a' AS m";
    let mock = MockProvider::start().await;
    let (seed, provider) = ai_seed(&mock).await;
    let logs = tempfile::tempdir().unwrap();
    let log = super::logging::init(logs.path(), seaquel_terminal::LogLevel::Trace)
        .unwrap()
        .unwrap();
    let mut h = open(&seed, &provider, "SELECT 'editor-marker-7a';").await;
    mock.reply(openai_sql(SQL));
    ask_for(&mut h, PROMPT).await;
    h.send(ctrl('r'));
    assert!(h.model.modal.is_none(), "it ran");
    h.until("the run", idle).await;
    // A failure too: its log line names the code only.
    mock.reply(Reply::json(
        500,
        &json!({"error": {"message": format!("echo {PROMPT} {TEST_KEY}")}}),
    ));
    ask_for(&mut h, PROMPT).await;
    let debug = format!("{:?} {:?}", h.model, h.effects);
    h.close().await;
    let logged = std::fs::read_to_string(&log).unwrap();
    // `log`'s records reach the file with their key-values (Task 9).
    assert!(
        logged.contains("Generating SQL activity=ai.generate connection_id="),
        "Core's line is logged with its key-values: {logged}"
    );
    assert!(
        logged.contains("Ask AI failed activity=tui.ask code=PROVIDER_ERROR elapsed_ms="),
        "the TUI's line is logged with its key-values: {logged}"
    );
    for marker in [TEST_KEY, "prompt-marker", "sql-marker", "editor-marker"] {
        assert!(!logged.contains(marker), "{marker} in the log");
        assert!(!debug.contains(marker), "{marker} in Debug");
    }
}
