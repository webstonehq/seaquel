//! Task 6 against a real Core: runs through `db.run` (the whole text, the
//! statement at the UTF-16 cursor, parameters, the editor's destructive
//! prompt and Core's `CONFIRM_REQUIRED`), history from `done.history`,
//! paging through `db.page`, "stream all" stopped at the cap, cancel, Explain
//! (plain on SQLite, ANALYZE on Postgres, whose plan is recorded as the
//! Explain tab's fixture) and saving through the library. SQLite always;
//! Postgres behind `SEAQUEL_TEST_POSTGRES`.

use std::sync::Arc;

use crossterm::event::KeyCode;
use seaquel_core::domain::run::RunTarget;
use seaquel_core::secrets::{MemoryStore, SecretStore};
use seaquel_core::{ConnectRequest, Value};

use super::browse_tests::{open_shop, open_shop_named, seed};
use crate::state::app::{Modal, Model, Msg};
use crate::state::editor::Normal;
use crate::state::query::{self, ConfirmKind, ExplainView, RunCall, RunMsg, Status, ROW_CAP};
use crate::testing::core::{project, Seed};
use crate::testing::harness::Harness;
use crate::testing::keys::{alt, ctrl, key};

/// A query tab holding `text`, the cursor at its end.
fn tab_with(h: &mut Harness, text: &str) {
    h.send(key('Q'));
    let tab = h.model.query.active_mut().unwrap();
    tab.editor.set_text(text);
    tab.editor.normal(Normal::Bottom);
    tab.editor.normal(Normal::LineEnd);
    query::sync(&mut h.model);
}

fn idle(m: &Model) -> bool {
    !m.running()
}

fn tab(m: &Model) -> &query::QueryTab {
    m.query.active().unwrap()
}

fn cells(m: &Model, statement: usize) -> Vec<Vec<String>> {
    tab(m).statements[statement]
        .page
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .map(|r| r.iter().map(crate::state::grid::display).collect())
        .collect()
}

#[tokio::test]
async fn whole_text_and_statement_at_the_cursor_run_with_history() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    let conn = h.model.conn.id().unwrap().to_string();
    let text =
        "SELECT id, customer FROM invoices WHERE id <= 3 ORDER BY id;\nSELECT '😀' AS e, 2 AS b";
    tab_with(&mut h, text);
    h.send(ctrl('r'));
    h.until("the run", idle).await;
    let t = tab(&h.model);
    assert_eq!(t.statements.len(), 2);
    assert_eq!(t.finished, Some((2, true)));
    assert_eq!(cells(&h.model, 0), [["1", "c1"], ["2", "c2"], ["3", "c3"]]);
    assert_eq!(t.shown, Some(1), "the last statement with rows");
    // History: Core's row, in panel 3 at once, and stored.
    assert_eq!(h.model.history_items[0].sql, text);
    let stored = h.session.history(&conn).await.unwrap();
    assert_eq!(stored[0].sql, text);
    // Alt+R: the statement at the cursor, past the emoji.
    h.send(alt('r'));
    h.until("the current statement", idle).await;
    let t = tab(&h.model);
    assert_eq!(t.statements.len(), 1);
    assert_eq!(t.statements[0].page.as_ref().unwrap().columns, ["e", "b"]);
    assert_eq!(cells(&h.model, 0), [["😀", "2"]]);
    h.close().await;
}

// Review M6: the cursor mid-line after a 😀 (two UTF-16 units) and an
// `e` with a combining accent (one char each), right before and right
// after a `;`: Core picks the statement the cursor is in.
#[tokio::test]
async fn the_utf16_cursor_picks_the_statement_core_runs() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    let line = "SELECT '😀e\u{301}' AS a; SELECT 'x' AS b";
    tab_with(&mut h, &format!("-- first line\n{line}"));
    let semicolon = line.chars().position(|c| c == ';').unwrap();
    for (chars_in, column) in [(semicolon, "a"), (semicolon + 1, "b"), (10, "a")] {
        let t = h.model.query.active_mut().unwrap();
        t.editor.normal(Normal::Bottom);
        t.editor.normal(Normal::LineStart);
        for _ in 0..chars_in {
            t.editor.normal(Normal::Right);
        }
        h.send(alt('r'));
        h.until("the statement", idle).await;
        let t = tab(&h.model);
        assert_eq!(t.statements.len(), 1, "at {chars_in}");
        assert_eq!(
            t.statements[0].page.as_ref().unwrap().columns,
            [column],
            "at char {chars_in}"
        );
    }
    h.close().await;
}

#[tokio::test]
async fn parameters_and_a_confirmed_destructive_run() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    tab_with(&mut h, "SELECT {{n}} AS v, {{m}} AS w");
    h.send(ctrl('r'));
    h.keys("42");
    h.press(KeyCode::Tab);
    h.keys("NULL");
    h.press(KeyCode::Enter);
    h.until("the run", idle).await;
    assert_eq!(cells(&h.model, 0), [["42", "NULL"]]);

    // The editor's own check asks; Enter sends it with `confirmed`.
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text("DELETE FROM events");
    h.send(ctrl('r'));
    assert!(matches!(h.model.modal, Some(Modal::RunConfirm(_))));
    h.press(KeyCode::Enter);
    h.until("the delete", idle).await;
    assert!(
        matches!(
            tab(&h.model).statements[0].status,
            Status::Done {
                rows_affected: Some(1),
                ..
            }
        ),
        "{:?}",
        tab(&h.model).statements
    );
    h.close().await;
}

// Core's own check: a run sent without `confirmed` is refused with its
// list, and the question reopens with it.
#[tokio::test]
async fn core_refuses_an_unconfirmed_destructive_run_and_the_question_reopens() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    tab_with(&mut h, "DROP TABLE events");
    h.send(ctrl('r'));
    let Some(Modal::RunConfirm(confirm)) = h.model.modal.take() else {
        panic!("{:?}", h.model.modal)
    };
    let query::Confirmed::Run(pending) = confirm.then else {
        panic!()
    };
    // Send it without `confirmed`, as a caller that skipped the dialog would.
    let t = h.model.query.active_mut().unwrap();
    t.op = Some(query::Op {
        op: 99,
        stream_id: "tui-run-test-99".into(),
        kind: query::OpKind::Run,
        page_size: 100,
        pending: Some(pending.clone()),
        connection_id: None,
        core_id: h.model.conn.core_id().unwrap().into(),
    });
    let call = RunCall {
        tab: t.id,
        op: 99,
        stream_id: "tui-run-test-99".into(),
        core_id: h.model.conn.core_id().unwrap().into(),
        text: pending.text.clone(),
        target: RunTarget::All,
        params: None,
        page_size: 100,
        confirmed: false,
        history: None,
    };
    let mut events = Vec::new();
    h.session.run(&call, |m| events.push(m)).await;
    assert!(
        matches!(events.last(), Some(RunMsg::Refused { error, destructive: Some((list, 1)) }) if error.code == "CONFIRM_REQUIRED" && list[0].reason == "drops a table"),
        "{events:?}"
    );
    for event in events {
        h.send(Msg::Run {
            tab: call.tab,
            op: 99,
            event,
        });
    }
    let Some(Modal::RunConfirm(confirm)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert!(matches!(
        confirm.kind,
        ConfirmKind::Destructive {
            from_core: true,
            ..
        }
    ));
    h.close().await;
}

#[tokio::test]
async fn results_page_with_db_page() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    tab_with(&mut h, "SELECT id FROM invoices ORDER BY id");
    h.send(ctrl('r'));
    h.until("page 1", idle).await;
    let page = tab(&h.model).statements[0].page.clone().unwrap();
    assert_eq!(
        (page.rows.len(), page.total_rows, page.total_pages),
        (100, 150, 2)
    );
    h.send(ctrl('w'));
    h.keys("n");
    assert!(h.model.running());
    h.until("page 2", idle).await;
    let page = tab(&h.model).statements[0].page.clone().unwrap();
    assert_eq!((page.page, page.rows.len()), (2, 50));
    assert_eq!(page.rows[0][0], Value::Int(101));
    h.close().await;
}

#[tokio::test]
async fn stream_all_streams_every_row_unpaged() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    tab_with(&mut h, "SELECT id FROM invoices ORDER BY id");
    h.press(KeyCode::Esc);
    h.keys(":all");
    h.press(KeyCode::Enter);
    h.until("every row", idle).await;
    let s = &tab(&h.model).statements[0];
    assert!(!s.capped);
    assert_eq!(s.page.as_ref().unwrap().rows.len(), 150, "not paged");
    h.close().await;
}

#[tokio::test]
async fn a_plain_explain_on_sqlite() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    tab_with(&mut h, "SELECT * FROM invoices WHERE customer = 'c1'");
    h.send(ctrl('x'));
    h.until("the plan", |m| {
        !matches!(tab(m).explain, Some(ExplainView::Loading { .. }))
    })
    .await;
    let Some(ExplainView::Loaded(plan)) = &tab(&h.model).explain else {
        panic!("{:?}", tab(&h.model).explain)
    };
    assert!(!plan.is_analyze);
    let rows = crate::state::explain::rows(plan);
    assert!(
        rows.iter().any(|r| r.label.contains("invoices")),
        "{:?}",
        rows.iter().map(|r| &r.label).collect::<Vec<_>>()
    );
    h.close().await;
}

#[tokio::test]
async fn save_creates_updates_and_names_a_taken_name() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    let project_id = h.model.project.clone().unwrap();
    tab_with(&mut h, "SELECT 42 AS answer");
    h.send(ctrl('s'));
    h.keys("answer");
    h.press(KeyCode::Enter);
    h.until("saved", |m| tab(m).saved.is_some()).await;
    assert!(!tab(&h.model).modified);
    assert!(h.model.saved_items.iter().any(|s| s.name == "answer"));
    let stored = h.session.saved(&project_id).await.unwrap();
    let row = stored.iter().find(|s| s.name == "answer").unwrap();
    assert_eq!(row.sql, "SELECT 42 AS answer");
    // An edit, then Ctrl+S: an update of the same row.
    h.keys(" -- v2");
    assert!(tab(&h.model).modified);
    h.send(ctrl('s'));
    h.until("updated", |m| !tab(m).modified).await;
    let stored = h.session.saved(&project_id).await.unwrap();
    assert_eq!(
        stored.iter().find(|s| s.id == row.id).unwrap().sql,
        "SELECT 42 AS answer -- v2"
    );
    // A new tab saved under the same name: Core's NAME_TAKEN, worded.
    h.press(KeyCode::Esc);
    h.keys("+");
    h.keys("SELECT 1");
    h.send(ctrl('s'));
    h.keys("answer");
    h.press(KeyCode::Enter);
    h.until(
        "refused",
        |m| matches!(&m.modal, Some(Modal::SaveAs(s)) if s.error.is_some()),
    )
    .await;
    let Some(Modal::SaveAs(save)) = &h.model.modal else {
        unreachable!()
    };
    assert!(
        save.error.as_deref().unwrap().contains("\"answer\""),
        "{:?}",
        save.error
    );
    h.close().await;
}

/// A Postgres connection behind `SEAQUEL_TEST_POSTGRES`, opened in the TUI.
async fn postgres() -> Option<(Seed, Harness)> {
    let config = super::app_tests::live("POSTGRES")?;
    let url = config["connection_string"].as_str().unwrap().to_string();
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store: store.clone(),
    };
    let conn = seed
        .with(|core, ws| async move {
            let p = project(&core, &ws, "Pg").await;
            let conn = crate::testing::core::connection(
                &core,
                &ws,
                serde_json::json!({"projectId": p, "name": "pg", "type": "postgres",
                                   "connectionString": url, "savePassword": true}),
            )
            .await;
            ws.connect(&core, ConnectRequest::saved(&conn))
                .await
                .unwrap();
            conn
        })
        .await;
    store.set(&format!("db:{conn}"), "unused").await.ok();
    let h = open_shop_named(&seed, "pg").await;
    Some((seed, h))
}

/// The join the Explain fixture records: `generate_series` only, so it
/// needs no table.
const EXPLAINED: &str = "SELECT g % 10 AS k, count(*) FROM generate_series(1, 20000) g \
     JOIN generate_series(1, 100) h ON h = g % 100 GROUP BY 1 ORDER BY 1";

#[tokio::test]
async fn postgres_cancel_reaches_the_server_and_analyze_explains() {
    let Some((_seed, mut h)) = postgres().await else {
        return;
    };
    let marker = format!("tui_cancel_{}", std::process::id());
    tab_with(&mut h, &format!("SELECT pg_sleep(30), '{marker}'"));
    h.send(ctrl('r'));
    h.until("started", |m| !tab(m).statements.is_empty()).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    h.send(ctrl('c'));
    h.until("cancelled", idle).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(tab(&h.model).statements[0].status, Status::Cancelled);
    // No backend still runs it.
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let mut still = 1;
    for _ in 0..40 {
        let r = h
            .session
            .ws
            .query(
                &h.session.core,
                &core_id,
                &format!(
                    "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' AND query LIKE '%{marker}%' AND query NOT LIKE '%pg_stat_activity%'"
                ),
                Vec::new(),
            )
            .await
            .unwrap();
        still = match r.rows[0][0] {
            Value::Int(n) => n,
            ref other => panic!("{other:?}"),
        };
        if still == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(still, 0, "the backend stopped");
    // A new run cancels the old one: the second answers.
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text("SELECT pg_sleep(30)");
    h.send(ctrl('r'));
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text("SELECT 7 AS seven");
    h.send(ctrl('r'));
    h.until("the second run", idle).await;
    assert_eq!(cells(&h.model, 0), [["7"]]);

    // ANALYZE of a SELECT runs without asking; the tree adds up.
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text(EXPLAINED);
    h.send(alt('x'));
    assert!(h.model.modal.is_none());
    h.until("the plan", |m| {
        !matches!(tab(m).explain, Some(ExplainView::Loading { .. }))
    })
    .await;
    let Some(ExplainView::Loaded(plan)) = &tab(&h.model).explain else {
        panic!("{:?}", tab(&h.model).explain)
    };
    assert!(plan.is_analyze);
    let rows = crate::state::explain::rows(plan);
    let shares: f64 = rows.iter().map(|r| r.share.unwrap()).sum();
    assert!((shares - 100.0).abs() < 1e-6, "{shares}");
    // ANALYZE of a write asks first.
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text("CREATE TEMP TABLE tui_never (x int)");
    h.send(alt('x'));
    assert!(matches!(h.model.modal, Some(Modal::RunConfirm(_))));
    h.press(KeyCode::Esc);
    h.close().await;
}

// Decision 15: "stream all" keeps at most ROW_CAP rows and stops the
// stream there (Core's own cap would fail the statement one row later).
#[tokio::test]
async fn postgres_stream_all_stops_at_the_cap() {
    let Some((_seed, mut h)) = postgres().await else {
        return;
    };
    tab_with(&mut h, "SELECT g FROM generate_series(1, 150000) g");
    h.press(KeyCode::Esc);
    h.keys(":all");
    h.press(KeyCode::Enter);
    h.until("the cap", idle).await;
    let s = &tab(&h.model).statements[0];
    assert!(s.capped, "{:?}", s.status);
    assert_eq!(s.page.as_ref().unwrap().rows.len(), ROW_CAP);
    // The tab runs again at once.
    h.model
        .query
        .active_mut()
        .unwrap()
        .editor
        .set_text("SELECT 1 AS one");
    h.send(ctrl('r'));
    h.until("the next run", idle).await;
    assert_eq!(cells(&h.model, 0), [["1"]]);
    h.close().await;
}

/// Records `tests/fixtures/explain/postgres_analyze.json`: Core's EXPLAIN
/// ANALYZE of [`EXPLAINED`] on the compose container. Runs only with
/// `SEAQUEL_TUI_RECORD_EXPLAIN=1`.
#[tokio::test]
async fn record_the_postgres_explain_fixture() {
    if std::env::var("SEAQUEL_TUI_RECORD_EXPLAIN").as_deref() != Ok("1") {
        return;
    }
    let Some((_seed, h)) = postgres().await else {
        panic!("SEAQUEL_TEST_POSTGRES is needed to record")
    };
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let plan = h
        .session
        .ws
        .engine(&h.session.core, &core_id)
        .unwrap()
        .explain(EXPLAINED, Vec::new(), true)
        .await
        .unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/explain/postgres_analyze.json");
    std::fs::write(&path, serde_json::to_string_pretty(&plan).unwrap() + "\n").unwrap();
    h.close().await;
}

/// Probe F1, live on SQLite: `schema_tables` lists no columns (as on every
/// engine), so `i.` reads the table's through Core (`table_metadata`),
/// keeps them, and opens the popup when they arrive.
#[tokio::test]
async fn alias_completion_reads_the_columns_through_core() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    assert!(
        h.model.schema.iter().all(|t| t.columns.is_empty()),
        "schema_tables listed columns"
    );
    tab_with(&mut h, "SELECT * FROM invoices i WHERE i");
    h.send(key('.'));
    h.until("the popup", |m| {
        m.query
            .active()
            .is_some_and(|t| t.editor.completion.is_some())
    })
    .await;
    let popup = tab(&h.model).editor.completion.clone().unwrap();
    let labels: Vec<&str> = popup.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["id", "customer", "total", "note"]);
    let invoices = h
        .model
        .schema
        .iter()
        .find(|t| t.name == "invoices")
        .unwrap();
    assert_eq!(invoices.columns.len(), 4, "kept for the next time");
    h.close().await;
}

/// Review I2, live on SQLite: `ALTER TABLE … ADD COLUMN`, then `r` in
/// panel 2, and `i.` lists the new column.
#[tokio::test]
async fn a_reload_after_ddl_shows_the_new_column() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    let popup_labels = |m: &Model| -> Vec<String> {
        tab(m)
            .editor
            .completion
            .as_ref()
            .map(|p| p.items.iter().map(|i| i.label.clone()).collect())
            .unwrap_or_default()
    };
    tab_with(&mut h, "SELECT * FROM invoices i WHERE i");
    h.send(key('.'));
    h.until("the popup", |m| !popup_labels(m).is_empty()).await;
    assert!(!popup_labels(&h.model).contains(&"extra".to_string()));
    let core_id = h.model.conn.core_id().unwrap().to_string();
    h.session
        .ws
        .execute(
            &h.session.core,
            &core_id,
            "ALTER TABLE invoices ADD COLUMN extra TEXT",
            Vec::new(),
        )
        .await
        .unwrap();
    h.model.focus_panel(crate::state::app::Panel::Tables);
    h.send(key('r'));
    h.until("listed again", |m| {
        m.schema_load == crate::state::app::Load::Loaded && m.column_loads.is_empty()
    })
    .await;
    tab_with(&mut h, "SELECT * FROM invoices i WHERE i");
    h.send(key('.'));
    h.until("the new column", |m| {
        popup_labels(m).contains(&"extra".to_string())
    })
    .await;
    h.close().await;
}
