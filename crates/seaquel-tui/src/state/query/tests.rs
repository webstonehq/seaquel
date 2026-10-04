//! Query in `update` (Task 6): tabs, the editor's modes, completion, runs
//! (whole text, statement at the cursor, parameters, the destructive
//! check, Core's `CONFIRM_REQUIRED`), their events, paging, cancel,
//! Explain, saving and `$EDITOR`. The GUI's rules where it has them: one
//! run per tab (`QueryExecutionManager`), history from Core only
//! (`QueryHistoryManager.insertRecorded`), the editor's destructive prompt
//! then `confirmed` (`query-editor.svelte`).

use crossterm::event::KeyCode;
use seaquel_core::domain::run::{PageSource, RunTarget, StatementKind};
use seaquel_core::Value;

use super::*;
use crate::state::app::{update, Effect, Modal, Model, Msg, Panel, SavedTab};
use crate::state::commit::Destructive;
use crate::state::editor::Mode;
use crate::state::keymap::BarContext;
use crate::state::log::Tag;
use crate::state::panels::{HistoryItem, SavedItem};
use crate::testing::fixtures::{connected, querying, running};
use crate::testing::keys::{alt, ctrl, key, press};

fn typed(m: &mut Model, text: &str) -> Vec<Effect> {
    text.chars().flat_map(|c| update(m, key(c))).collect()
}

fn text(m: &Model) -> String {
    m.query.active().unwrap().editor.text()
}

fn run_call(effects: &[Effect]) -> RunCall {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Run(call) => Some(call.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a run: {effects:?}"))
}

fn tab_id(m: &Model) -> u64 {
    m.query.active().unwrap().id
}

fn send(m: &mut Model, call: &RunCall, event: RunMsg) -> Vec<Effect> {
    update(
        m,
        Msg::Run {
            tab: call.tab,
            op: call.op,
            event,
        },
    )
}

fn start(index: u32, sql: &str, kind: StatementKind, page: u32, page_size: u32) -> RunMsg {
    RunMsg::Start {
        index,
        sql: sql.into(),
        source: PageSource {
            sql: sql.into(),
            params: Vec::new(),
        },
        kind,
        page,
        page_size,
    }
}

fn batch(columns: &[&str], rows: Vec<Vec<Value>>) -> RunMsg {
    RunMsg::Batch {
        columns: Some(columns.iter().map(|c| c.to_string()).collect()),
        rows,
    }
}

fn done(index: u32, total_rows: u64, total_pages: u32, rows_affected: Option<u64>) -> RunMsg {
    RunMsg::Done {
        index,
        elapsed_ms: 3.2,
        total_rows,
        total_pages,
        count_estimated: false,
        rows_affected,
    }
}

fn finished(history: Option<HistoryItem>) -> RunMsg {
    RunMsg::Finished {
        statements: 1,
        succeeded: true,
        history,
    }
}

fn ints(range: std::ops::Range<i64>) -> Vec<Vec<Value>> {
    range.map(|i| vec![Value::Int(i)]).collect()
}

#[test]
fn q_and_plus_open_tabs_over_the_main_view() {
    let mut m = connected(148, 42);
    typed(&mut m, "Q");
    assert!(m.query.shown);
    assert_eq!(m.focus, Panel::Main);
    assert_eq!(m.query.tabs.len(), 1);
    assert_eq!(m.query.tabs[0].title, "untitled-1");
    assert_eq!(m.query.tabs[0].editor.mode, Mode::Insert);
    assert_eq!(m.bar_context(), BarContext::QueryInsert);
    // `+` is text in Insert mode; in Normal mode it's a new tab.
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, "+");
    assert_eq!(m.query.tabs.len(), 2);
    assert_eq!(
        (m.query.active, m.query.tabs[1].title.as_str()),
        (1, "untitled-2")
    );
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, "]");
    assert_eq!(m.query.active, 0, "] wraps to the first tab");
    typed(&mut m, "[");
    assert_eq!(m.query.active, 1);
    // A panel takes the main view back; `Q` shows the same tabs again.
    typed(&mut m, "2");
    assert!(!m.query.shown);
    assert_eq!(m.bar_context(), BarContext::Tables);
    typed(&mut m, "Q");
    assert!(m.query.shown);
    assert_eq!(m.query.tabs.len(), 2);
}

// Decision 9: a text input takes every printable key; Task 5's `c` (global
// Commit) and `q` are text here.
#[test]
fn insert_mode_takes_every_printable_key() {
    let mut m = querying(148, 42, "", false);
    let effects = typed(&mut m, "c1q[?:Q+u");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(text(&m), "c1q[?:Q+u");
    assert_eq!((m.focus, m.modal.clone()), (Panel::Main, None));
    update(&mut m, press(KeyCode::Enter));
    update(&mut m, press(KeyCode::Backspace));
    update(&mut m, press(KeyCode::Backspace));
    assert_eq!(text(&m), "c1q[?:Q+");
    assert!(m.query.active().unwrap().modified);
}

#[test]
fn normal_mode_edits_and_u_undoes_the_text_not_the_staging() {
    let mut m = querying(148, 42, "SELECT 1\nSELECT 2", false);
    update(&mut m, press(KeyCode::Esc));
    assert_eq!(m.bar_context(), BarContext::QueryNormal);
    typed(&mut m, "ggdd");
    assert_eq!(text(&m), "SELECT 2");
    let effects = typed(&mut m, "u");
    assert!(effects.is_empty(), "no staging undo: {effects:?}");
    assert_eq!(text(&m), "SELECT 1\nSELECT 2");
    typed(&mut m, "ggo");
    assert_eq!(m.query.active().unwrap().editor.mode, Mode::Insert);
    typed(&mut m, "-- x");
    assert_eq!(text(&m), "SELECT 1\n-- x\nSELECT 2");
}

#[test]
fn ctrl_r_sends_the_whole_text_with_its_history_context() {
    let mut m = querying(148, 42, "SELECT 1;\nSELECT 2", false);
    let effects = update(&mut m, ctrl('r'));
    let call = run_call(&effects);
    assert_eq!(call.text, "SELECT 1;\nSELECT 2");
    assert_eq!(call.target, RunTarget::All);
    assert_eq!((call.page_size, call.confirmed), (100, false));
    assert_eq!(call.params, None);
    assert_eq!(call.core_id, "core-1");
    assert!(call.stream_id.starts_with("tui-run-"), "{}", call.stream_id);
    let history = call.history.as_ref().unwrap();
    assert_eq!(
        (
            history.connection_id.as_str(),
            history.connection_name.as_str()
        ),
        ("conn-saved", "prod-analytics")
    );
    assert!(m.running());
    // F5 is an alias; a new run cancels the tab's last one first.
    let effects = update(&mut m, press(KeyCode::F(5)));
    assert!(
        matches!(&effects[0], Effect::CancelRun { tab, stream_id: Some(id) } if *tab == call.tab && *id == call.stream_id),
        "{effects:?}"
    );
    let second = run_call(&effects);
    assert_ne!(second.op, call.op);
    // A late event of the first run is dropped.
    send(
        &mut m,
        &call,
        start(0, "SELECT 1", StatementKind::Page, 1, 100),
    );
    assert!(m.query.active().unwrap().statements.is_empty());
}

// The emoji is two UTF-16 units: Core picks the statement with the cursor
// the editor sends.
#[test]
fn alt_r_and_capital_r_send_the_utf16_cursor() {
    let sql = "SELECT '😀' AS a;\nSELECT 2 AS b";
    let mut m = querying(148, 42, sql, false);
    let call = run_call(&update(&mut m, alt('r')));
    let cursor = seaquel_core::sql::offsets::utf16_len(sql) as u64;
    assert_eq!(call.target, RunTarget::Current { cursor });
    assert_eq!(
        call.text, sql,
        "the whole text goes; Core picks the statement"
    );
    update(&mut m, press(KeyCode::Esc));
    let call = run_call(&typed(&mut m, "R"));
    assert_eq!(call.target, RunTarget::Current { cursor });
}

#[test]
fn a_run_needs_a_connection() {
    let mut m = querying(148, 42, "SELECT 1", false);
    m.conn = crate::state::app::Conn::None;
    let effects = update(&mut m, ctrl('r'));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run(_))));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Log(l) if l.tag == Some(Tag::Error))),
        "{effects:?}"
    );
    assert!(!m.running());
    let effects = update(&mut m, ctrl('r'));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run(_))));
}

#[test]
fn parameters_open_a_form_first() {
    let mut m = querying(148, 42, "SELECT {{n}} AS v, {{m}} AS w", false);
    assert!(update(&mut m, ctrl('r')).is_empty());
    let Some(Modal::Params(form)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert_eq!(form.names, ["n", "m"]);
    assert_eq!(m.bar_context(), BarContext::Params);
    typed(&mut m, "42");
    update(&mut m, press(KeyCode::Tab));
    typed(&mut m, "NULL");
    let call = run_call(&update(&mut m, press(KeyCode::Enter)));
    assert_eq!(
        call.params,
        Some(vec![
            ("n".to_string(), Value::Text("42".into())),
            ("m".to_string(), Value::Null)
        ])
    );
    // The values are remembered for the next run.
    update(&mut m, ctrl('r'));
    let Some(Modal::Params(form)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert_eq!(form.values, ["42", "NULL"]);
    update(&mut m, press(KeyCode::Esc));
    // The statement at the cursor has none: no form.
    let mut m = querying(148, 42, "SELECT {{a}};\nSELECT 2", false);
    let call = run_call(&update(&mut m, alt('r')));
    assert_eq!(call.params, None);
}

// GUI: the editor's synchronous destructive check prompts, and `confirmed`
// goes only after the user agrees.
#[test]
fn the_editor_s_destructive_check_asks_then_sends_confirmed() {
    let mut m = querying(148, 42, "DELETE FROM invoices", false);
    assert!(update(&mut m, ctrl('r')).is_empty());
    let Some(Modal::RunConfirm(confirm)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    let ConfirmKind::Destructive {
        list,
        total,
        from_core,
    } = &confirm.kind
    else {
        panic!("{:?}", confirm.kind)
    };
    assert_eq!((list.len(), *total, *from_core), (1, 1, false));
    assert_eq!(list[0].reason, "DELETE without WHERE");
    assert!(update(&mut m, press(KeyCode::Esc)).is_empty());
    assert_eq!(m.modal, None);
    assert!(!m.running());
    update(&mut m, ctrl('r'));
    let call = run_call(&update(&mut m, press(KeyCode::Enter)));
    assert!(call.confirmed);
    // Nothing destructive: no question and no `confirmed`.
    let mut m = querying(148, 42, "DELETE FROM invoices WHERE id = 1", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    assert!(!call.confirmed);
}

// Q11 A: on a connection tagged `prod`, the run waits for `prod` typed.
#[test]
fn on_prod_a_destructive_run_needs_prod_typed() {
    let mut m = querying(148, 42, "TRUNCATE invoices", true);
    update(&mut m, ctrl('r'));
    assert!(prod(&m));
    assert!(update(&mut m, press(KeyCode::Enter)).is_empty());
    assert!(matches!(m.modal, Some(Modal::RunConfirm(_))));
    typed(&mut m, "prd");
    update(&mut m, press(KeyCode::Backspace));
    update(&mut m, press(KeyCode::Backspace));
    typed(&mut m, "rod");
    let call = run_call(&update(&mut m, press(KeyCode::Enter)));
    assert!(call.confirmed);
}

#[test]
fn core_s_confirm_required_reopens_the_question_with_its_list() {
    let mut m = querying(148, 42, "SELECT 1", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    let list = vec![Destructive {
        sql: "DROP TABLE t".into(),
        reason: "drops a table".into(),
    }];
    send(
        &mut m,
        &call,
        RunMsg::Refused {
            error: CallError::new("CONFIRM_REQUIRED", "1 destructive statement"),
            destructive: Some((list.clone(), 3)),
        },
    );
    assert!(!m.running());
    let Some(Modal::RunConfirm(confirm)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert_eq!(
        confirm.kind,
        ConfirmKind::Destructive {
            list,
            total: 3,
            from_core: true
        }
    );
    let again = run_call(&update(&mut m, press(KeyCode::Enter)));
    assert!(again.confirmed);
    assert_eq!(
        (again.text.as_str(), again.target),
        ("SELECT 1", RunTarget::All)
    );
    assert_ne!(again.op, call.op);
}

#[test]
fn events_fill_the_results_messages_log_and_history() {
    let mut m = querying(
        148,
        42,
        "SELECT id FROM t;\nUPDATE t SET x = 1 WHERE id = 1",
        false,
    );
    let call = run_call(&update(&mut m, ctrl('r')));
    let mut effects = Vec::new();
    effects.extend(send(
        &mut m,
        &call,
        start(0, "SELECT id FROM t", StatementKind::Page, 1, 100),
    ));
    effects.extend(send(&mut m, &call, batch(&["id"], ints(1..3))));
    effects.extend(send(&mut m, &call, done(0, 2, 1, None)));
    effects.extend(send(
        &mut m,
        &call,
        start(
            1,
            "UPDATE t SET x = 1 WHERE id = 1",
            StatementKind::Write,
            1,
            100,
        ),
    ));
    effects.extend(send(&mut m, &call, done(1, 0, 1, Some(2))));
    let row = HistoryItem {
        id: "h-new".into(),
        when: "12:04:31".into(),
        sql: "SELECT id FROM t;\nUPDATE t SET x = 1 WHERE id = 1".into(),
        elapsed_ms: 6.4,
        rows: 2.0,
    };
    send(&mut m, &call, finished(Some(row.clone())));
    assert!(!m.running());
    let tab = m.query.active().unwrap();
    assert_eq!(tab.statements.len(), 2);
    assert_eq!(tab.shown, Some(0), "the last statement with rows");
    assert_eq!(tab.statements[0].page.as_ref().unwrap().rows.len(), 2);
    assert_eq!(
        tab.statements[1].status,
        Status::Done {
            elapsed_ms: 3.2,
            rows_affected: Some(2)
        }
    );
    assert_eq!(tab.result_tab, ResultTab::Results);
    // History from Core's answer only, newest first and once.
    assert_eq!(m.history_items[0].id, "h-new");
    let before = m.history_items.len();
    send(&mut m, &call, finished(Some(row)));
    assert_eq!(m.history_items.len(), before);
    // The command log: each statement as typed, with its time.
    let lines: Vec<(String, Option<String>)> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Log(l) => Some((l.text.clone(), l.elapsed.clone())),
            _ => None,
        })
        .collect();
    assert!(
        lines.contains(&("SELECT id FROM t".to_string(), Some("3 ms".to_string()))),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|(l, _)| l == "UPDATE t SET x = 1 WHERE id = 1"),
        "{lines:?}"
    );
}

#[test]
fn a_failed_statement_and_a_refused_run_show_in_messages() {
    let mut m = querying(148, 42, "SELEC 1", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELEC 1", StatementKind::Utility, 1, 100),
    );
    send(
        &mut m,
        &call,
        RunMsg::Failed {
            index: 0,
            error: CallError::new("SYNTAX_ERROR", "syntax error at or near \"SELEC\""),
            elapsed_ms: 1.0,
            sql: None,
        },
    );
    send(
        &mut m,
        &call,
        RunMsg::Finished {
            statements: 1,
            succeeded: false,
            history: None,
        },
    );
    let tab = m.query.active().unwrap();
    assert!(matches!(&tab.statements[0].status, Status::Failed(e) if e.code == "SYNTAX_ERROR"));
    assert_eq!(tab.result_tab, ResultTab::Messages, "no rows: Messages");
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        RunMsg::Refused {
            error: CallError::new("CONNECTION_NOT_FOUND", "gone"),
            destructive: None,
        },
    );
    let tab = m.query.active().unwrap();
    assert_eq!(tab.run_error.as_ref().unwrap().code, "CONNECTION_NOT_FOUND");
    assert!(!m.running());
}

#[test]
fn ctrl_c_and_esc_in_the_results_cancel() {
    let mut m = running(148, 42);
    let call_stream = m
        .query
        .active()
        .unwrap()
        .op
        .as_ref()
        .unwrap()
        .stream_id
        .clone();
    let effects = update(&mut m, ctrl('c'));
    assert_eq!(
        effects[0],
        Effect::CancelRun {
            tab: tab_id(&m),
            stream_id: Some(call_stream)
        }
    );
    assert!(
        matches!(&effects[1], Effect::Log(l) if l.text == "cancelled"),
        "{effects:?}"
    );
    assert!(!m.running());
    assert_eq!(m.modal, None);
    // Esc in the results cancels; with nothing running it goes back.
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELECT 1", StatementKind::Page, 1, 100),
    );
    update(&mut m, ctrl('w'));
    assert_eq!(m.bar_context(), BarContext::Results);
    let effects = update(&mut m, press(KeyCode::Esc));
    assert!(
        matches!(effects.first(), Some(Effect::CancelRun { .. })),
        "{effects:?}"
    );
    assert_eq!(
        m.query.active().unwrap().statements[0].status,
        Status::Cancelled
    );
    assert!(update(&mut m, press(KeyCode::Esc)).is_empty());
    assert_eq!(m.query.pane, Pane::Editor);
}

#[test]
fn n_and_p_page_with_db_page_and_the_statement_s_source() {
    let mut m = querying(148, 42, "SELECT * FROM big", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELECT * FROM big", StatementKind::Page, 1, 100),
    );
    send(&mut m, &call, batch(&["id"], ints(0..100)));
    send(&mut m, &call, done(0, 250, 3, None));
    send(&mut m, &call, finished(None));
    update(&mut m, ctrl('w'));
    let effects = update(&mut m, key('n'));
    let Some(Effect::PageRun(page)) = effects.first() else {
        panic!("{effects:?}")
    };
    assert_eq!((page.page, page.page_size), (2, 100));
    assert_eq!(page.source.sql, "SELECT * FROM big");
    assert!(m.running(), "a page is the tab's one operation");
    let page = page.clone();
    let page_call = RunCall {
        op: page.op,
        ..call.clone()
    };
    send(
        &mut m,
        &page_call,
        start(0, "SELECT * FROM big", StatementKind::Page, 2, 100),
    );
    send(&mut m, &page_call, batch(&["id"], ints(100..200)));
    send(&mut m, &page_call, done(0, 250, 3, None));
    send(&mut m, &page_call, finished(None));
    let tab = m.query.active().unwrap();
    let shown = tab.statements[0].page.as_ref().unwrap();
    assert_eq!((shown.page, shown.rows.len()), (2, 100));
    assert_eq!(shown.rows[0][0], Value::Int(100));
    assert!(!m.running());
    let effects = update(&mut m, key('p'));
    assert!(
        matches!(effects.first(), Some(Effect::PageRun(p)) if p.page == 1),
        "{effects:?}"
    );
}

#[test]
fn stream_all_keeps_at_most_the_row_cap() {
    let mut m = querying(148, 42, "SELECT * FROM huge", false);
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, ":all");
    assert_eq!(m.bar_context(), BarContext::QueryCommand);
    let call = run_call(&update(&mut m, press(KeyCode::Enter)));
    assert_eq!(call.page_size, 0);
    send(
        &mut m,
        &call,
        start(0, "SELECT * FROM huge", StatementKind::Stream, 1, 0),
    );
    let effects = send(
        &mut m,
        &call,
        batch(&["id"], ints(0..(ROW_CAP as i64 + 10))),
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelRun { .. })),
        "{effects:?}"
    );
    let tab = m.query.active().unwrap();
    assert_eq!(tab.statements[0].page.as_ref().unwrap().rows.len(), ROW_CAP);
    assert!(tab.statements[0].capped);
    assert!(!m.running());
}

/// Probe F4: a stream stopped at the cap shows the time it ran, from its
/// `statementStart` to the tick the cap was reached in, not "0 ms".
#[test]
fn a_capped_stream_shows_the_time_it_ran() {
    let mut m = querying(148, 42, "SELECT * FROM huge", false);
    let t0 = std::time::Instant::now();
    update(&mut m, Msg::Tick(t0));
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, ":all");
    let call = run_call(&update(&mut m, press(KeyCode::Enter)));
    update(&mut m, Msg::Tick(t0 + std::time::Duration::from_millis(20)));
    send(
        &mut m,
        &call,
        start(0, "SELECT * FROM huge", StatementKind::Stream, 1, 0),
    );
    update(
        &mut m,
        Msg::Tick(t0 + std::time::Duration::from_millis(360)),
    );
    send(
        &mut m,
        &call,
        batch(&["id"], ints(0..(ROW_CAP as i64 + 10))),
    );
    let tab = m.query.active().unwrap();
    assert!(tab.statements[0].capped);
    match tab.statements[0].status {
        Status::Done { elapsed_ms, .. } => assert_eq!(elapsed_ms, 340.0),
        ref other => panic!("{other:?}"),
    }
}

#[test]
fn enter_opens_a_cell_full_size() {
    let mut m = querying(148, 42, "SELECT doc FROM t", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELECT doc FROM t", StatementKind::Page, 1, 100),
    );
    send(
        &mut m,
        &call,
        batch(
            &["doc"],
            vec![vec![Value::Json(serde_json::json!({"a": 1}))]],
        ),
    );
    send(&mut m, &call, done(0, 1, 1, None));
    update(&mut m, ctrl('w'));
    update(&mut m, press(KeyCode::Enter));
    let Some(Modal::Cell(cell)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert_eq!(cell.column, "doc");
    assert_eq!(cell.text, "{\n  \"a\": 1\n}");
    update(&mut m, press(KeyCode::Esc));
    assert_eq!(m.modal, None);
}

#[test]
fn explain_and_analyze_the_statement_at_the_cursor() {
    let mut m = querying(148, 42, "SELECT 1;\nUPDATE t SET x = 1", false);
    let effects = update(&mut m, ctrl('x'));
    let Some(Effect::Explain(call)) = effects.first() else {
        panic!("{effects:?}")
    };
    assert_eq!(
        (call.sql.as_str(), call.analyze),
        ("UPDATE t SET x = 1", false)
    );
    let tab = m.query.active().unwrap();
    assert_eq!(tab.result_tab, ResultTab::Explain);
    assert_eq!(tab.explain, Some(ExplainView::Loading { analyze: false }));
    // ANALYZE runs it: anything but a SELECT asks, naming its kind.
    assert!(update(&mut m, alt('x')).is_empty());
    let Some(Modal::RunConfirm(confirm)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert_eq!(
        confirm.kind,
        ConfirmKind::Analyze {
            verb: "UPDATE".into()
        }
    );
    let effects = update(&mut m, press(KeyCode::Enter));
    assert!(
        matches!(effects.first(), Some(Effect::Explain(c)) if c.analyze),
        "{effects:?}"
    );
    // A SELECT doesn't ask.
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, "gg");
    let effects = update(&mut m, alt('x'));
    let Some(Effect::Explain(call)) = effects.first() else {
        panic!("{effects:?}")
    };
    assert_eq!((call.sql.as_str(), call.analyze), ("SELECT 1", true));
    let call = call.clone();
    let plan = crate::state::explain::tests::design_plan();
    update(
        &mut m,
        Msg::Explained {
            tab: call.tab,
            op: call.op,
            result: Ok(Box::new(plan.clone())),
        },
    );
    assert_eq!(
        m.query.active().unwrap().explain,
        Some(ExplainView::Loaded(Box::new(plan)))
    );
    // `{{params}}` aren't explained.
    let mut m = querying(148, 42, "SELECT {{x}}", false);
    let effects = update(&mut m, ctrl('x'));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Explain(_))),
        "{effects:?}"
    );
}

#[test]
fn ctrl_s_creates_with_a_name_then_updates() {
    let mut m = querying(148, 42, "SELECT 42", false);
    assert!(update(&mut m, ctrl('s')).is_empty());
    assert!(matches!(m.modal, Some(Modal::SaveAs(_))));
    typed(&mut m, "answer");
    let effects = update(&mut m, press(KeyCode::Enter));
    let Some(Effect::SaveQuery(call)) = effects.first() else {
        panic!("{effects:?}")
    };
    assert_eq!(
        call.save,
        SaveKind::Create {
            project_id: "project-a".into(),
            name: "answer".into()
        }
    );
    assert_eq!(call.text, "SELECT 42");
    let tab = call.tab;
    update(
        &mut m,
        Msg::QuerySaved {
            tab,
            text: "SELECT 42".into(),
            result: Ok(SavedItem {
                id: "saved-new".into(),
                name: "answer".into(),
                folder: None,
                shared: false,
                sql: "SELECT 42".into(),
            }),
            taken_by: None,
            detached: false,
        },
    );
    let t = m.query.active().unwrap();
    assert_eq!(t.title, "answer");
    assert_eq!(t.saved.as_ref().unwrap().id, "saved-new");
    assert!(!t.modified);
    assert!(m.saved_items.iter().any(|s| s.id == "saved-new"));
    typed(&mut m, "1");
    assert!(m.query.active().unwrap().modified);
    let effects = update(&mut m, ctrl('s'));
    assert!(
        matches!(effects.first(), Some(Effect::SaveQuery(c)) if c.save == SaveKind::Update { id: "saved-new".into() } && c.text == "SELECT 421"),
        "{effects:?}"
    );
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, ":w");
    let effects = update(&mut m, press(KeyCode::Enter));
    assert!(
        matches!(effects.first(), Some(Effect::SaveQuery(_))),
        "{effects:?}"
    );
}

#[test]
fn name_taken_names_the_row_core_points_at() {
    let mut m = querying(148, 42, "SELECT 42", false);
    update(&mut m, ctrl('s'));
    typed(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    let tab = tab_id(&m);
    update(
        &mut m,
        Msg::QuerySaved {
            tab,
            text: "SELECT 42".into(),
            result: Err(CallError::new("NAME_TAKEN", "the name is taken")),
            taken_by: Some("saved-top_customers.sql".into()),
            detached: false,
        },
    );
    let Some(Modal::SaveAs(save)) = &m.modal else {
        panic!("{:?}", m.modal)
    };
    assert!(
        save.error.as_deref().unwrap().contains("top_customers.sql"),
        "{:?}",
        save.error
    );
    assert!(m.query.active().unwrap().saved.is_none());
}

#[test]
fn saved_queries_and_history_rows_open_in_tabs() {
    let mut m = connected(148, 42);
    m.focus_panel(Panel::Saved);
    typed(&mut m, "o");
    assert!(m.query.shown);
    let tab = m.query.active().unwrap();
    assert_eq!(tab.title, "revenue_by_month.sql");
    assert_eq!(tab.saved.as_ref().unwrap().id, "saved-revenue_by_month.sql");
    assert!(text(&m).starts_with("SELECT c.name"));
    assert!(!tab.modified);
    // Again: the same tab.
    m.focus_panel(Panel::Saved);
    typed(&mut m, "o");
    assert_eq!(m.query.tabs.len(), 1);
    // A history row opens a new tab; nothing runs.
    m.focus_panel(Panel::Saved);
    typed(&mut m, "]");
    assert_eq!(m.saved_tab, SavedTab::History);
    let effects = typed(&mut m, "o");
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(m.query.tabs.len(), 2);
    assert!(text(&m).starts_with("SELECT * FROM public.invoices"));
    // Enter in the main view over panel 3 opens it too.
    m.focus_panel(Panel::Saved);
    typed(&mut m, "[");
    m.saved.selected = 1;
    update(&mut m, press(KeyCode::Enter));
    assert_eq!((m.focus, m.query.shown), (Panel::Main, false));
    update(&mut m, press(KeyCode::Enter));
    assert!(m.query.shown);
    assert_eq!(m.query.active().unwrap().title, "top_customers.sql");
}

#[test]
fn a_dot_opens_completion_and_tab_inserts_the_item() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    typed(&mut m, ".");
    let popup = m.query.active().unwrap().editor.completion.clone().unwrap();
    let labels: Vec<&str> = popup.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["id", "customer", "total"]);
    assert_eq!(m.bar_context(), BarContext::Completion);
    update(&mut m, press(KeyCode::Down));
    update(&mut m, press(KeyCode::Tab));
    assert_eq!(text(&m), "SELECT * FROM invoices i WHERE i.customer");
    assert!(m.query.active().unwrap().editor.completion.is_none());
    // Typing on refilters; Esc closes.
    typed(&mut m, " AND i.t");
    let popup = m.query.active().unwrap().editor.completion.clone().unwrap();
    assert_eq!(popup.items.len(), 1);
    update(&mut m, press(KeyCode::Esc));
    assert!(m.query.active().unwrap().editor.completion.is_none());
    assert_eq!(m.query.active().unwrap().editor.mode, Mode::Insert);
    // Tab asks for it; Enter inserts the first.
    let mut m = querying(148, 42, "SELECT * FROM inv", false);
    update(&mut m, press(KeyCode::Tab));
    update(&mut m, press(KeyCode::Enter));
    assert_eq!(text(&m), "SELECT * FROM invoice_line_items");
    // Nothing loaded: Tab indents.
    let mut m = querying(148, 42, "SELECT", false);
    m.schema_load = crate::state::app::Load::Idle;
    update(&mut m, press(KeyCode::Tab));
    assert_eq!(text(&m), "SELECT  ");
}

#[test]
fn no_sql_text_in_the_debug_of_effects_and_messages() {
    let mut m = querying(148, 42, "SELECT 'sql-marker'", false);
    let effects = update(&mut m, ctrl('o'));
    let tab = tab_id(&m);
    let msgs = [
        Msg::Edited {
            tab,
            result: Ok("SELECT 'sql-marker'".into()),
        },
        Msg::QuerySaved {
            tab,
            text: "SELECT 'sql-marker'".into(),
            result: Err(CallError::new("X", "y")),
            taken_by: None,
            detached: false,
        },
    ];
    let call = run_call(&update(&mut m, ctrl('r')));
    let text = format!("{effects:?} {msgs:?} {call:?} {:?} {m:?}", call.history);
    assert!(!text.contains("sql-marker"), "{text}");
    assert!(
        !text.contains("prod-analytics"),
        "no connection name: {text}"
    );
}

#[test]
fn ctrl_o_hands_the_text_to_the_external_editor() {
    let mut m = querying(148, 42, "SELECT 1", false);
    let effects = update(&mut m, ctrl('o'));
    assert_eq!(
        effects,
        [Effect::ExternalEditor {
            tab: tab_id(&m),
            text: "SELECT 1".into()
        }]
    );
    let tab = tab_id(&m);
    update(
        &mut m,
        Msg::Edited {
            tab,
            result: Ok("SELECT 2\nFROM t".into()),
        },
    );
    assert_eq!(text(&m), "SELECT 2\nFROM t");
    assert!(m.query.active().unwrap().modified);
    update(
        &mut m,
        Msg::Edited {
            tab,
            result: Err("vi: not found".into()),
        },
    );
    assert_eq!(text(&m), "SELECT 2\nFROM t", "unchanged");
    assert!(matches!(&m.modal, Some(Modal::Notice(n)) if n.0.contains("vi: not found")));
}

// Review: the tab's operation is cancelled when the tab closes.
#[test]
fn closing_a_tab_cancels_its_run() {
    let mut m = running(148, 42);
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, ":q");
    let effects = update(&mut m, press(KeyCode::Enter));
    assert_eq!(m.query.tabs.len(), 1, "a modified tab stays: {effects:?}");
    typed(&mut m, ":q!");
    let effects = update(&mut m, press(KeyCode::Enter));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelRun { .. })),
        "{effects:?}"
    );
    assert!(m.query.tabs.is_empty());
    assert!(!m.query.shown);
    assert!(!m.running());
}

#[test]
fn the_bar_s_mode_names_the_line_and_column() {
    let mut m = querying(148, 42, "SELECT 1\nFROM t", false);
    assert_eq!(m.mode().as_deref(), Some("INSERT · Ln 2, Col 7"));
    update(&mut m, press(KeyCode::Esc));
    assert_eq!(m.mode().as_deref(), Some("NORMAL · Ln 2, Col 7"));
    update(&mut m, ctrl('w'));
    assert_eq!(m.bar_context(), BarContext::Results);
}

// Q4 A: the open tabs and their text survive a restart through the state
// file; a saved query's tab takes its name (and, unchanged, its text) from
// the library. Review M1: an unchanged saved tab keeps only a hash, a text
// over 1 MiB isn't kept and the tab says so.
#[test]
fn tabs_are_remembered_and_restored() {
    use crate::state::picker::text_hash;
    let mut m = connected(148, 42);
    m.focus_panel(Panel::Saved);
    typed(&mut m, "o");
    typed(&mut m, " -- edited");
    m.focus_panel(Panel::Saved);
    m.saved.selected = 1;
    typed(&mut m, "o");
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, "+");
    typed(&mut m, "SELECT 2");
    update(&mut m, press(KeyCode::Esc));
    typed(&mut m, "+");
    let big = "x".repeat(crate::state::picker::MAX_REMEMBERED_TEXT + 1);
    update(&mut m, Msg::Paste(big));
    assert!(m.remember_dirty.is_some(), "a changed tab is written later");
    remember(&mut m);
    let tabs = m.remembered.query_tabs.clone();
    assert_eq!(tabs.len(), 4);
    let revenue = &m.saved_items[0].sql;
    assert_eq!(
        tabs[0].saved_id.as_deref(),
        Some("saved-revenue_by_month.sql")
    );
    assert!(tabs[0].text.as_deref().unwrap().starts_with(" -- edited"));
    assert_eq!(
        tabs[0].stored_hash.as_deref(),
        Some(text_hash(revenue).as_str())
    );
    // Unchanged: no text, the hash only.
    assert_eq!(tabs[1].saved_id.as_deref(), Some("saved-top_customers.sql"));
    assert_eq!(tabs[1].text, None);
    assert!(tabs[1].stored_hash.is_some());
    assert_eq!(
        (tabs[2].saved_id.as_deref(), tabs[2].text.as_deref()),
        (None, Some("SELECT 2"))
    );
    assert_eq!((tabs[3].text.as_deref(), tabs[3].omitted), (None, true));
    assert_eq!(m.remembered.query_active, 3);
    assert!(
        !format!("{:?}", m.remembered).contains("edited"),
        "no text in Debug"
    );

    let mut fresh = connected(148, 42);
    let library = std::mem::take(&mut fresh.saved_items);
    restore(&mut fresh, &tabs, 2);
    sync(&mut fresh);
    assert!(!fresh.query.shown);
    assert_eq!(fresh.query.active, 2);
    // Before panel 3's list arrives: the saved tabs wait for it.
    assert_eq!(fresh.query.tabs[1].editor.text(), "");
    fresh.saved_items = library;
    sync(&mut fresh);
    let t = &fresh.query.tabs[0];
    assert_eq!(t.title, "revenue_by_month.sql");
    assert!(t.modified);
    assert_eq!(t.editor.text(), tabs[0].text.as_deref().unwrap());
    let t = &fresh.query.tabs[1];
    assert_eq!(t.title, "top_customers.sql");
    assert_eq!(t.editor.text(), "SELECT 1;");
    assert!(!t.modified);
    assert_eq!(fresh.query.tabs[2].title, "untitled-1");
    let t = &fresh.query.tabs[3];
    assert_eq!(t.editor.text(), "");
    assert!(t.notice.is_some(), "the omitted tab says why it's empty");
    typed(&mut fresh, "Q");
    assert_eq!(text(&fresh), "SELECT 2");
}

// Review I1: a paste is inserted as received, never through completion.
#[test]
fn a_paste_goes_in_verbatim_and_closes_the_popup() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    typed(&mut m, ".");
    assert!(m.query.active().unwrap().editor.completion.is_some());
    update(&mut m, Msg::Paste("('a','b')\nORDER BY".into()));
    assert!(m.query.active().unwrap().editor.completion.is_none());
    update(&mut m, Msg::Paste(" DESC\nLIMIT".into()));
    assert_eq!(
        text(&m),
        "SELECT * FROM invoices i WHERE i.('a','b')\nORDER BY DESC\nLIMIT"
    );
    // A paste in Normal mode or outside the editor changes nothing.
    update(&mut m, press(KeyCode::Esc));
    update(&mut m, Msg::Paste("x".into()));
    assert!(text(&m).ends_with("LIMIT"));
}

// Review I1: typing after the popup opened doesn't rewrite what's typed.
#[test]
fn an_empty_prefix_closes_the_popup_except_right_after_a_dot() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    typed(&mut m, ".");
    assert!(
        m.query.active().unwrap().editor.completion.is_some(),
        "after ."
    );
    typed(&mut m, "t");
    assert!(m.query.active().unwrap().editor.completion.is_some());
    update(&mut m, press(KeyCode::Backspace));
    assert!(
        m.query.active().unwrap().editor.completion.is_some(),
        "straight after `.` again"
    );
    typed(&mut m, "(");
    assert!(m.query.active().unwrap().editor.completion.is_none());
    update(&mut m, press(KeyCode::Enter));
    typed(&mut m, "DESC");
    update(&mut m, press(KeyCode::Enter));
    assert_eq!(text(&m), "SELECT * FROM invoices i WHERE i.(\nDESC\n");
}

#[test]
fn tab_indents_with_no_prefix() {
    let mut m = querying(148, 42, "SELECT *\nFROM invoices", false);
    update(&mut m, press(KeyCode::Home));
    update(&mut m, press(KeyCode::Tab));
    assert_eq!(text(&m), "SELECT *\n    FROM invoices");
    assert!(m.query.active().unwrap().editor.completion.is_none());
    // After a space, too.
    let mut m = querying(148, 42, "SELECT ", false);
    update(&mut m, press(KeyCode::Tab));
    assert_eq!(text(&m), "SELECT  ", "to the next stop");
}

#[test]
fn no_popup_inside_a_string_or_comment() {
    for sql in [
        "SELECT * FROM invoices i WHERE note = 'see i",
        "SELECT * FROM invoices i -- i",
        "SELECT * FROM invoices i /* i",
    ] {
        let mut m = querying(148, 42, sql, false);
        typed(&mut m, ".");
        assert!(
            m.query.active().unwrap().editor.completion.is_none(),
            "{sql}"
        );
        update(&mut m, press(KeyCode::Tab));
        assert!(
            m.query.active().unwrap().editor.completion.is_none(),
            "Tab: {sql}"
        );
    }
    // Right after a closed string, it opens.
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE 'x' = i", false);
    typed(&mut m, ".");
    assert!(m.query.active().unwrap().editor.completion.is_some());
}

// Review I2: results page only on the saved connection they came from, with
// its Core id as it is now (a reconnect is followed).
#[test]
fn paging_follows_the_results_connection() {
    let mut m = querying(148, 42, "SELECT * FROM big", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELECT * FROM big", StatementKind::Page, 1, 100),
    );
    send(&mut m, &call, batch(&["id"], ints(0..100)));
    send(&mut m, &call, done(0, 250, 3, None));
    send(&mut m, &call, finished(None));
    assert_eq!(
        m.query.active().unwrap().results_connection.as_deref(),
        Some("conn-saved")
    );
    update(&mut m, ctrl('w'));
    // Reconnected: the same saved connection, a new Core id.
    m.conn = crate::state::app::Conn::Connected {
        id: "conn-saved".into(),
        core_id: "core-2".into(),
    };
    let effects = update(&mut m, key('n'));
    assert!(
        matches!(effects.first(), Some(Effect::PageRun(p)) if p.core_id == "core-2"),
        "{effects:?}"
    );
    let page_call = RunCall {
        op: match &effects[0] {
            Effect::PageRun(p) => p.op,
            _ => unreachable!(),
        },
        ..call.clone()
    };
    send(&mut m, &page_call, RunMsg::Ended);
    // Another saved connection: refused, with a message.
    m.conn = crate::state::app::Conn::Connected {
        id: "conn-ask".into(),
        core_id: "core-3".into(),
    };
    let effects = update(&mut m, key('n'));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::PageRun(_))),
        "{effects:?}"
    );
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Log(l) if l.tag == Some(Tag::Error) && l.text.contains("prod-analytics"))),
        "{effects:?}"
    );
    assert!(!m.running());
}

// Review M4 (decision): ANALYZE runs without asking only a plain SELECT:
// `query_type` SELECT, no destructive reason, and no `INTO`, `FOR UPDATE`,
// `FOR SHARE`, `nextval` or `setval` among the scanner's tokens (words in
// strings, comments and quoted names don't count).
#[test]
fn analyze_asks_unless_a_plain_select() {
    for (sql, asks) in [
        ("SELECT * FROM t", false),
        ("SELECT 'into', \"nextval\" FROM t -- for update", false),
        ("SELECT * INTO copy FROM t", true),
        ("SELECT * FROM t FOR UPDATE", true),
        ("SELECT * FROM t FOR NO KEY UPDATE", true),
        ("SELECT * FROM t for share", true),
        ("SELECT nextval('s')", true),
        ("SELECT SetVal('s', 1)", true),
        (
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            true,
        ),
    ] {
        let mut m = querying(148, 42, sql, false);
        let effects = update(&mut m, alt('x'));
        let asked = matches!(m.modal, Some(Modal::RunConfirm(_)));
        assert_eq!(asked, asks, "{sql}: {effects:?}");
        assert_eq!(
            effects.iter().any(|e| matches!(e, Effect::Explain(_))),
            !asks,
            "{sql}"
        );
    }
}

// Review M5: a cancelled explain says the plan may still finish.
#[test]
fn a_cancelled_explain_says_so() {
    let mut m = querying(148, 42, "SELECT 1", false);
    update(&mut m, ctrl('x'));
    let effects = update(&mut m, ctrl('c'));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelExplain { .. })),
        "{effects:?}"
    );
    let Some(ExplainView::Failed(e)) = &m.query.active().unwrap().explain else {
        panic!("{:?}", m.query.active().unwrap().explain)
    };
    assert_eq!(
        e.message,
        "Stopped waiting; the plan may still finish on the server"
    );
}

// Review M7: the cap applies whenever a statement streams (its own LIMIT,
// say), not only under `:all`; review M8: the cut says no history row was
// recorded.
#[test]
fn the_row_cap_applies_to_any_streamed_statement() {
    let mut m = querying(148, 42, "SELECT * FROM huge LIMIT 500000", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    assert_eq!(call.page_size, 100);
    send(
        &mut m,
        &call,
        start(
            0,
            "SELECT * FROM huge LIMIT 500000",
            StatementKind::Stream,
            1,
            100,
        ),
    );
    let effects = send(&mut m, &call, batch(&["id"], ints(0..(ROW_CAP as i64 + 5))));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelRun { .. })),
        "{effects:?}"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Log(l) if l.text.contains("no history row was recorded"))),
        "{effects:?}"
    );
    let s = &m.query.active().unwrap().statements[0];
    assert!(s.capped);
    assert_eq!(s.page.as_ref().unwrap().rows.len(), ROW_CAP);
    // A paged statement isn't cut (Core pages it).
    let mut m = querying(148, 42, "SELECT * FROM huge", false);
    let call = run_call(&update(&mut m, ctrl('r')));
    send(
        &mut m,
        &call,
        start(0, "SELECT * FROM huge", StatementKind::Page, 1, 100),
    );
    let effects = send(&mut m, &call, batch(&["id"], ints(0..100)));
    assert!(!effects
        .iter()
        .any(|e| matches!(e, Effect::CancelRun { .. })));
}

/// Probe F2: Esc and the next key in one read. A legacy terminal (and tmux
/// within its `escape-time`) sends `ESC :`, which crossterm reads as
/// Alt+`:`; with the kitty protocol Esc is `CSI 27 u`, a key of its own.
/// Both must leave Insert mode and then take the key, as vim does in a
/// terminal, whenever that Alt binding doesn't exist here.
#[test]
fn esc_and_a_key_in_one_read_are_esc_then_the_key() {
    let state = |m: &Model| {
        let e = &m.query.active().unwrap().editor;
        (e.mode, e.command.clone(), e.text())
    };
    // `:` after Esc: the command line, in both encodings.
    let mut legacy = querying(148, 42, "SELECT 1", false);
    assert_eq!(legacy.query.active().unwrap().editor.mode, Mode::Insert);
    update(&mut legacy, alt(':'));
    let mut kitty = querying(148, 42, "SELECT 1", false);
    update(&mut kitty, press(KeyCode::Esc));
    update(&mut kitty, key(':'));
    assert_eq!(
        state(&legacy),
        (Mode::Normal, Some(String::new()), "SELECT 1".into())
    );
    assert_eq!(state(&legacy), state(&kitty));

    // `dd` after Esc deletes the line, in both encodings.
    let mut legacy = querying(148, 42, "SELECT 1", false);
    update(&mut legacy, alt('d'));
    update(&mut legacy, key('d'));
    let mut kitty = querying(148, 42, "SELECT 1", false);
    update(&mut kitty, press(KeyCode::Esc));
    typed(&mut kitty, "dd");
    assert_eq!(state(&legacy), (Mode::Normal, None, String::new()));
    assert_eq!(state(&legacy), state(&kitty));

    // An Alt binding that exists keeps its meaning: Alt+R runs the
    // statement at the cursor (the same as `R` after Esc).
    let mut m = querying(148, 42, "SELECT 1", false);
    run_call(&update(&mut m, alt('r')));
}

/// The same outside the editor: in a panel, `ESC 3` focuses panel 3 (Esc
/// there goes back, which is nothing to undo).
#[test]
fn esc_and_a_key_in_one_read_work_in_the_panels_too() {
    let mut m = connected(148, 42);
    m.focus_panel(Panel::Tables);
    update(&mut m, alt('3'));
    assert_eq!(m.focus, Panel::Saved);
}

/// What every engine's `schema_tables` really gives (phase 6's F3): no
/// columns. Completion and the preview must not lean on them.
fn without_columns(m: &mut Model) {
    for t in &mut m.schema {
        t.columns.clear();
    }
}

fn load_columns(effects: &[Effect]) -> Vec<(String, seaquel_core::domain::edits::TableTarget)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::LoadColumns { core_id, target } => Some((core_id.clone(), target.clone())),
            _ => None,
        })
        .collect()
}

fn invoices_target() -> seaquel_core::domain::edits::TableTarget {
    seaquel_core::domain::edits::TableTarget {
        schema: "public".into(),
        table: "invoices".into(),
    }
}

/// Probe F1: `alias.` reads that table's columns from Core on first need,
/// keeps them for the connection, and opens the popup when they arrive.
#[test]
fn alias_completion_loads_the_table_s_columns_on_first_need() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    without_columns(&mut m);
    let effects = typed(&mut m, ".");
    assert_eq!(
        load_columns(&effects),
        [("core-1".to_string(), invoices_target())]
    );
    assert!(m.query.active().unwrap().editor.completion.is_none());
    // Typing on while they load asks nothing more.
    assert!(load_columns(&typed(&mut m, "t")).is_empty());
    assert!(load_columns(&update(&mut m, ctrl(' '))).is_empty());
    // They arrive: kept on the table, and the popup opens for `i.t`.
    let effects = update(
        &mut m,
        Msg::Columns {
            core_id: "core-1".into(),
            target: invoices_target(),
            result: Ok(vec![
                ("id".into(), "int8".into()),
                ("total".into(), "numeric".into()),
                ("tax".into(), "numeric".into()),
            ]),
        },
    );
    assert!(effects.is_empty(), "{effects:?}");
    let popup = m.query.active().unwrap().editor.completion.clone().unwrap();
    let labels: Vec<&str> = popup.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["total", "tax"]);
    let invoices = m.schema.iter().find(|t| t.name == "invoices").unwrap();
    assert_eq!(invoices.columns.len(), 3);
    // Cached: the next `i.` opens at once and asks nothing.
    update(&mut m, press(KeyCode::Esc));
    update(&mut m, key('a'));
    let effects = typed(&mut m, " AND i.");
    assert!(load_columns(&effects).is_empty());
    assert!(m.query.active().unwrap().editor.completion.is_some());
}

#[test]
fn alias_columns_are_never_asked_for_inside_strings_or_comments() {
    for text in [
        "SELECT 'x FROM invoices i WHERE i",
        "SELECT 1 FROM invoices i -- i",
        "SELECT 1 FROM invoices i /* i",
    ] {
        let mut m = querying(148, 42, text, false);
        without_columns(&mut m);
        assert!(load_columns(&typed(&mut m, ".")).is_empty(), "{text}");
        assert!(
            load_columns(&update(&mut m, ctrl(' '))).is_empty(),
            "{text}"
        );
    }
}

/// A late answer for another connection, or a failed read, opens nothing,
/// and a failure isn't asked again at once.
#[test]
fn alias_columns_from_another_connection_or_a_failure_open_nothing() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    without_columns(&mut m);
    typed(&mut m, ".");
    update(
        &mut m,
        Msg::Columns {
            core_id: "core-old".into(),
            target: invoices_target(),
            result: Ok(vec![("id".into(), "int8".into())]),
        },
    );
    assert!(m.query.active().unwrap().editor.completion.is_none());
    assert!(m.schema.iter().all(|t| t.columns.is_empty()));
    update(
        &mut m,
        Msg::Columns {
            core_id: "core-1".into(),
            target: invoices_target(),
            result: Err(CallError::new("NOT_SUPPORTED", "no")),
        },
    );
    assert!(m.query.active().unwrap().editor.completion.is_none());
    assert!(load_columns(&update(&mut m, ctrl(' '))).is_empty());
}

/// Review I2: a schema read again (`r`, a commit's DDL, another process)
/// may follow DDL, so the columns read before are dropped with it and
/// read again on the next `alias.`.
#[test]
fn a_schema_reload_drops_the_columns_read_before() {
    let mut m = querying(148, 42, "SELECT * FROM invoices i WHERE i", false);
    without_columns(&mut m);
    typed(&mut m, ".");
    update(
        &mut m,
        Msg::Columns {
            core_id: "core-1".into(),
            target: invoices_target(),
            result: Ok(vec![("id".into(), "int8".into())]),
        },
    );
    let mut fresh = m.schema.clone();
    without_columns_in(&mut fresh);
    update(
        &mut m,
        Msg::Schema {
            core_id: "core-1".into(),
            result: Ok(fresh),
            stamp: crate::state::app::Stamp::default(),
        },
    );
    assert!(m.schema.iter().all(|t| t.columns.is_empty()));
    assert!(m.column_loads.is_empty());
    update(&mut m, press(KeyCode::Esc));
    update(&mut m, key('a'));
    let effects = typed(&mut m, " AND i.");
    assert_eq!(
        load_columns(&effects),
        [("core-1".to_string(), invoices_target())]
    );
}

fn without_columns_in(tables: &mut [crate::state::panels::TableItem]) {
    for t in tables {
        t.columns.clear();
    }
}

/// Review M1: only a character is split. Alt+Left, Alt+Backspace and
/// Alt+Enter are keys a terminal sends as one (word moves, word deletes),
/// never Esc and then a key, so they leave Insert mode alone. And with the
/// kitty flags pushed, Esc is never merged, so nothing is split.
#[test]
fn only_a_character_is_split_and_only_without_the_kitty_flags() {
    use crossterm::event::{KeyEvent, KeyModifiers};
    let alt_key = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::ALT));
    for code in [KeyCode::Left, KeyCode::Backspace, KeyCode::Enter] {
        let mut m = querying(148, 42, "SELECT 1", false);
        update(&mut m, alt_key(code));
        assert_eq!(
            m.query.active().unwrap().editor.mode,
            Mode::Insert,
            "{code:?}"
        );
    }
    let mut m = querying(148, 42, "SELECT 1", false);
    m.kitty_keys = true;
    update(&mut m, alt(':'));
    assert_eq!(m.query.active().unwrap().editor.mode, Mode::Insert);
}
