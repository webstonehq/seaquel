//! Ask AI in `update` (Task 7): the popup's title, `@` mentions, sending
//! the request as typed, refining, inserting at the cursor, Ctrl+R's
//! read-only run of the inserted statement only, the error codes worded,
//! Esc while waiting, Ctrl+S on the generated SQL, and no prompt, SQL or key
//! in `Debug`.

use crossterm::event::KeyCode;
use seaquel_core::domain::run::RunTarget;
use seaquel_core::sql::scan::statement_at;
use seaquel_core::sql::SqlEngine;

use super::*;
use crate::state::app::{update, Effect, Modal, Model, Msg};
use crate::state::dialogs::CallError;
use crate::state::keymap::BarContext;
use crate::state::query::{SaveKind, SqlText};
use crate::state::text;
use crate::testing::fixtures::querying;
use crate::testing::keys::{ctrl, key, press};

fn keys(m: &mut Model, text: &str) -> Vec<Effect> {
    text.chars().flat_map(|c| update(m, key(c))).collect()
}

fn ask(m: &Model) -> &Ask {
    match &m.modal {
        Some(Modal::Ask(a)) => a,
        other => panic!("not asking: {other:?}"),
    }
}

/// A query tab holding `text` (the cursor at its end) with the popup open.
fn asking(text: &str) -> Model {
    let mut m = querying(148, 42, text, false);
    let effects = update(&mut m, ctrl('k'));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::LoadMentions { project_id } if project_id == "project-a")),
        "{effects:?}"
    );
    m
}

/// The generate call the last Enter sent.
fn sent(effects: &[Effect]) -> &GenerateCall {
    effects
        .iter()
        .find_map(|e| match e {
            Effect::Generate(call) => Some(call),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no generate: {effects:?}"))
}

/// The answer to the request in flight.
fn answer(m: &mut Model, sql: &str) -> Vec<Effect> {
    let op = ask(m).op;
    update(
        m,
        Msg::Generated {
            op,
            result: Ok(SqlText(sql.into())),
            elapsed_ms: 1_800,
        },
    )
}

fn editor_text(m: &Model) -> String {
    m.query.active().unwrap().editor.text()
}

#[test]
fn ctrl_k_and_colon_ask_open_the_popup_over_the_query_tab() {
    let m = asking("SELECT 1");
    assert_eq!(m.bar_context(), BarContext::AskPrompt);
    let a = ask(&m);
    assert_eq!(a.connection_id, "conn-saved");
    assert_eq!(a.tab, m.query.active().unwrap().id);

    // `:ask` from Normal mode.
    let mut m = querying(148, 42, "SELECT 1", false);
    update(&mut m, press(KeyCode::Esc));
    keys(&mut m, ":ask");
    update(&mut m, press(KeyCode::Enter));
    assert!(matches!(m.modal, Some(Modal::Ask(_))));

    // No saved connection: it says so instead.
    let mut m = querying(148, 42, "SELECT 1", false);
    m.conn = crate::state::app::Conn::None;
    let effects = update(&mut m, ctrl('k'));
    assert!(m.modal.is_none());
    assert!(
        matches!(effects.as_slice(), [Effect::Log(_)]),
        "{effects:?}"
    );
}

#[test]
fn the_title_says_what_each_combination_shares() {
    for (schema, data, off, line) in [
        (
            true,
            false,
            false,
            "schema shared · data not shared · read-only",
        ),
        (true, true, false, "schema shared · data shared · read-only"),
        (
            false,
            false,
            false,
            "schema not shared · data not shared · read-only",
        ),
        (
            false,
            true,
            false,
            "schema not shared · data shared · read-only",
        ),
        (true, false, true, "AI is turned off · read-only"),
    ] {
        let mut m = asking("");
        let conn = &mut m.library.connections[0];
        conn.ai.schema = schema;
        conn.ai.data = data;
        m.library.ai_off = off;
        assert_eq!(sharing_line(&m), line);
    }
}

#[test]
fn at_completes_tables_saved_queries_and_dashboards() {
    let mut m = asking("");
    update(
        &mut m,
        Msg::Mentions {
            project_id: "project-a".into(),
            result: Ok(Names(vec!["Revenue board".into()])),
        },
    );
    keys(&mut m, "top payers in @inv");
    assert_eq!(m.bar_context(), BarContext::AskMention);
    let labels: Vec<String> = mention_items(&m).iter().map(|i| i.label.clone()).collect();
    assert_eq!(
        labels,
        ["public.invoices", "public.invoice_line_items"],
        "tables by name, schema-qualified"
    );
    update(&mut m, press(KeyCode::Tab));
    assert_eq!(ask(&m).prompt, "top payers in @public.invoices ");
    assert_eq!(m.bar_context(), BarContext::AskPrompt);

    // A saved query; a dashboard's name with a space is quoted.
    keys(&mut m, "like @top_c");
    update(&mut m, press(KeyCode::Enter));
    keys(&mut m, "on @rev");
    let items = mention_items(&m);
    assert_eq!(items[0].label, "revenue_by_month.sql");
    assert!(items.iter().any(|i| i.label == "Revenue board"));
    update(&mut m, press(KeyCode::Down));
    let chosen = mention_items(&m)[1].label.clone();
    assert_eq!(chosen, "Revenue board");
    update(&mut m, press(KeyCode::Enter));
    assert_eq!(
        ask(&m).prompt,
        "top payers in @public.invoices like @top_customers.sql on @\"Revenue board\" "
    );

    // An `@` inside a word (an address) isn't a mention; Esc closes the
    // list and keeps what was typed.
    keys(&mut m, "me@x");
    assert_eq!(m.bar_context(), BarContext::AskPrompt);
    keys(&mut m, " @zzz");
    assert!(mention_items(&m).is_empty());
    update(&mut m, press(KeyCode::Esc));
    assert_eq!(m.bar_context(), BarContext::AskPrompt);
    assert!(ask(&m).prompt.ends_with("me@x @zzz"));
}

#[test]
fn enter_sends_the_request_as_typed_with_the_editor_s_text() {
    let mut m = asking("SELECT * FROM invoices");
    // Nothing typed: nothing sent.
    let effects = update(&mut m, press(KeyCode::Enter));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Generate(_))));
    assert_eq!(ask(&m).error.as_deref(), Some(text::ASK_EMPTY));

    keys(&mut m, "only @public.invoices paid  ");
    let effects = update(&mut m, press(KeyCode::Enter));
    let call = sent(&effects);
    assert_eq!(call.connection_id, "conn-saved");
    assert_eq!(call.request, "only @public.invoices paid  ");
    assert_eq!(call.existing, "SELECT * FROM invoices");
    assert_eq!(m.bar_context(), BarContext::AskWaiting);
    // Keys don't edit the request while it's out.
    keys(&mut m, "xyz");
    assert_eq!(ask(&m).prompt, "only @public.invoices paid  ");
}

#[test]
fn an_answer_shows_and_tab_refines_the_shown_sql() {
    let mut m = asking("");
    keys(&mut m, "count invoices");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT count(*) FROM invoices;");
    assert_eq!(m.bar_context(), BarContext::AskAnswer);
    let a = ask(&m);
    assert_eq!(
        a.answer.as_ref().unwrap().sql,
        "SELECT count(*) FROM invoices;"
    );
    assert_eq!(a.answer.as_ref().unwrap().elapsed_ms, 1_800);

    update(&mut m, press(KeyCode::Tab));
    assert_eq!(m.bar_context(), BarContext::AskPrompt);
    assert!(ask(&m).refining);
    assert_eq!(ask(&m).prompt, "");
    keys(&mut m, "only paid ones");
    let effects = update(&mut m, press(KeyCode::Enter));
    let call = sent(&effects);
    assert_eq!(call.request, "only paid ones");
    assert_eq!(call.existing, "SELECT count(*) FROM invoices;");
    // A late answer to an earlier request is dropped.
    let stale = update(
        &mut m,
        Msg::Generated {
            op: 0,
            result: Ok(SqlText("SELECT 'old'".into())),
            elapsed_ms: 1,
        },
    );
    assert!(stale.is_empty());
    assert_eq!(m.bar_context(), BarContext::AskWaiting);
    answer(&mut m, "SELECT count(*) FROM invoices WHERE paid;");
    assert_eq!(
        ask(&m).answer.as_ref().unwrap().sql,
        "SELECT count(*) FROM invoices WHERE paid;"
    );
}

#[test]
fn enter_inserts_at_the_cursor() {
    let mut m = asking("SELECT 1;\n");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT 2;");
    let effects = update(&mut m, press(KeyCode::Enter));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run(_))));
    assert!(m.modal.is_none());
    assert_eq!(editor_text(&m), "SELECT 1;\nSELECT 2;");
    assert!(crate::state::query::typing(&m), "back in the editor");
}

#[test]
fn ctrl_r_inserts_a_select_and_runs_only_that_statement() {
    // A statement before it, ended: it isn't run.
    let mut m = asking("UPDATE invoices SET total = 0;");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT id\nFROM invoices;");
    let effects = update(&mut m, ctrl('r'));
    assert!(m.modal.is_none(), "{:?}", m.modal);
    let run = effects
        .iter()
        .find_map(|e| match e {
            Effect::Run(call) => Some(call),
            _ => None,
        })
        .unwrap_or_else(|| panic!("ran nothing: {effects:?}"));
    let RunTarget::Current { cursor } = run.target else {
        panic!("{:?}", run.target)
    };
    let byte = seaquel_core::sql::offsets::utf16_to_byte(&run.text, cursor as usize);
    let statement = statement_at(&run.text, byte, SqlEngine::Postgres).unwrap();
    assert_eq!(run.text[statement.text].trim(), "SELECT id\nFROM invoices");
    assert_eq!(
        editor_text(&m),
        "UPDATE invoices SET total = 0;\n\nSELECT id\nFROM invoices;"
    );
    assert!(!run.confirmed);
    assert_eq!(statement.index, 1, "the second of two");

    // On the blank line between two statements: they stay statements of
    // their own and only the inserted one runs.
    let mut m = asking("SELECT 1;\n\nSELECT 3;");
    let tab = m.query.active_mut().unwrap();
    tab.editor.normal(crate::state::editor::Normal::G);
    tab.editor.normal(crate::state::editor::Normal::G);
    tab.editor.normal(crate::state::editor::Normal::Down);
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT 2");
    let effects = update(&mut m, ctrl('r'));
    let run = effects
        .iter()
        .find_map(|e| match e {
            Effect::Run(call) => Some(call),
            _ => None,
        })
        .expect("ran");
    let RunTarget::Current { cursor } = run.target else {
        panic!()
    };
    let byte = seaquel_core::sql::offsets::utf16_to_byte(&run.text, cursor as usize);
    let statement = statement_at(&run.text, byte, SqlEngine::Postgres).unwrap();
    assert_eq!(run.text[statement.text].trim(), "SELECT 2");
    assert_eq!(
        seaquel_core::sql::scan::split_statements(&run.text, SqlEngine::Postgres).len(),
        3
    );
}

#[test]
fn ctrl_r_on_a_write_inserts_it_and_says_why_it_didnt_run() {
    for (sql, why) in [
        ("UPDATE invoices SET total = 0", text::ask_not_run("UPDATE")),
        (
            "SELECT 1; DELETE FROM invoices",
            text::ASK_NOT_RUN_SEVERAL.to_string(),
        ),
        ("SELECT nextval('s')", text::ask_not_run("")),
    ] {
        let mut m = asking("");
        keys(&mut m, "x");
        update(&mut m, press(KeyCode::Enter));
        answer(&mut m, sql);
        let effects = update(&mut m, ctrl('r'));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Run(_))),
            "{sql}: {effects:?}"
        );
        assert_eq!(editor_text(&m), sql, "inserted as it came");
        let a = ask(&m);
        assert_eq!(a.note.as_deref(), Some(why.as_str()), "{sql}");
        assert!(a.answer.as_ref().unwrap().inserted);
        assert_eq!(m.bar_context(), BarContext::AskDone);
        // Enter again doesn't insert it twice: it closes.
        update(&mut m, press(KeyCode::Enter));
        assert!(m.modal.is_none());
        assert_eq!(editor_text(&m), sql);
    }
}

#[test]
fn ctrl_r_while_not_connected_inserts_and_says_so() {
    let mut m = asking("");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT 1");
    m.conn = crate::state::app::Conn::Failed {
        id: "conn-saved".into(),
    };
    let effects = update(&mut m, ctrl('r'));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Run(_))));
    assert_eq!(ask(&m).note.as_deref(), Some(text::ASK_NOT_CONNECTED));
}

#[test]
fn each_error_code_is_worded() {
    for (code, needle) in [
        ("NO_PROVIDER", "Add an AI provider"),
        ("NO_API_KEY", "has no API key"),
        ("NO_MODEL", "no AI model chosen"),
        ("AI_DISABLED", "AI is turned off"),
        ("PROVIDER_ERROR", "refused the request: overloaded"),
        ("RATE_LIMITED", "limiting requests"),
        ("TIMEOUT", "took too long"),
    ] {
        let mut m = asking("");
        keys(&mut m, "x");
        update(&mut m, press(KeyCode::Enter));
        let op = ask(&m).op;
        let effects = update(
            &mut m,
            Msg::Generated {
                op,
                result: Err(CallError::new(code, "overloaded")),
                elapsed_ms: 5,
            },
        );
        let a = ask(&m);
        assert!(
            a.error.as_deref().is_some_and(|e| e.contains(needle)),
            "{code}: {:?}",
            a.error
        );
        assert_eq!(
            m.bar_context(),
            BarContext::AskPrompt,
            "the request can be sent again"
        );
        assert_eq!(a.prompt, "x");
        // The log names the code only.
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Log(l) if l.text == text::failed_line("ask", code))),
            "{effects:?}"
        );
    }
    // An empty answer is said too.
    let mut m = asking("");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "  ");
    assert_eq!(ask(&m).error.as_deref(), Some(text::ASK_NO_SQL));
}

#[test]
fn esc_while_waiting_drops_the_request() {
    let mut m = asking("");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    let op = ask(&m).op;
    let effects = update(&mut m, press(KeyCode::Esc));
    assert_eq!(effects, [Effect::CancelGenerate]);
    assert_eq!(m.bar_context(), BarContext::AskPrompt);
    assert_eq!(ask(&m).note.as_deref(), Some(text::ASK_STOPPED));
    // Its answer, if it still comes, is dropped.
    update(
        &mut m,
        Msg::Generated {
            op,
            result: Ok(SqlText("SELECT 1".into())),
            elapsed_ms: 1,
        },
    );
    assert!(ask(&m).answer.is_none());
    // Esc again closes.
    update(&mut m, press(KeyCode::Esc));
    assert!(m.modal.is_none());
}

#[test]
fn the_keychain_box_shows_while_a_request_waits_on_it() {
    let mut m = asking("");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    let now = std::time::Instant::now();
    update(&mut m, Msg::Tick(now));
    update(
        &mut m,
        Msg::Keychain {
            pending: true,
            at: now,
        },
    );
    update(
        &mut m,
        Msg::Tick(now + crate::state::app::KEYCHAIN_BOX_AFTER),
    );
    assert!(m.keychain_box());
    let effects = update(&mut m, press(KeyCode::Esc));
    assert_eq!(effects.first(), Some(&Effect::CancelGenerate));
    assert!(!m.keychain_box());
    assert_eq!(
        ask(&m).error.as_deref(),
        Some(text::ask_keychain_gave_up(m.store))
    );
}

#[test]
fn ctrl_s_saves_the_generated_sql_and_comes_back() {
    let mut m = asking("SELECT 'tab text'");
    keys(&mut m, "x");
    update(&mut m, press(KeyCode::Enter));
    answer(&mut m, "SELECT 42 AS answer;");
    update(&mut m, ctrl('s'));
    assert_eq!(m.bar_context(), BarContext::SaveAs);
    // Esc goes back to the answer.
    update(&mut m, press(KeyCode::Esc));
    assert_eq!(m.bar_context(), BarContext::AskAnswer);
    update(&mut m, ctrl('s'));
    keys(&mut m, "answer");
    let effects = update(&mut m, press(KeyCode::Enter));
    let call = effects
        .iter()
        .find_map(|e| match e {
            Effect::SaveQuery(call) => Some(call),
            _ => None,
        })
        .expect("saved");
    assert_eq!(call.text, "SELECT 42 AS answer;");
    assert!(call.detached);
    assert!(matches!(&call.save, SaveKind::Create { name, .. } if name == "answer"));
    assert_eq!(m.bar_context(), BarContext::AskAnswer, "the answer stays");
    let tab = call.tab;
    update(
        &mut m,
        Msg::QuerySaved {
            tab,
            text: SqlText(call.text.clone()),
            result: Ok(crate::state::panels::SavedItem {
                id: "saved-new".into(),
                name: "answer".into(),
                folder: None,
                shared: false,
                sql: "SELECT 42 AS answer;".into(),
            }),
            taken_by: None,
            detached: true,
        },
    );
    let t = m.query.active().unwrap();
    assert!(t.saved.is_none(), "the tab isn't the saved query");
    assert_eq!(t.editor.text(), "SELECT 'tab text'");
    assert!(m.saved_items.iter().any(|s| s.id == "saved-new"));
    assert_eq!(
        ask(&m).note.as_deref(),
        Some(text::ask_saved("answer").as_str())
    );
}

#[test]
fn no_prompt_sql_or_key_in_debug() {
    const PROMPT: &str = "prompt-marker-7a";
    const SQL: &str = "SELECT 'sql-marker-7a'";
    let mut m = asking("SELECT 'editor-marker-7a'");
    let mut effects = keys(&mut m, PROMPT);
    effects.extend(update(&mut m, press(KeyCode::Enter)));
    let op = ask(&m).op;
    let msg = Msg::Generated {
        op,
        result: Ok(SqlText(SQL.into())),
        elapsed_ms: 3,
    };
    let shown = format!("{msg:?}");
    effects.extend(update(&mut m, msg));
    effects.extend(update(&mut m, ctrl('s')));
    let all = format!("{shown} {:?} {effects:?}", m);
    for marker in [
        "prompt-marker",
        "sql-marker",
        "editor-marker",
        "test-key-not-real",
    ] {
        assert!(!all.contains(marker), "{marker} in {all}");
    }
}

#[test]
fn a_paste_goes_into_the_request_on_one_line() {
    let mut m = asking("SELECT 1");
    keys(&mut m, "top ");
    update(&mut m, Msg::Paste("customers\r\nby revenue".into()));
    assert_eq!(ask(&m).prompt, "top customers by revenue");
    assert_eq!(editor_text(&m), "SELECT 1", "not into the editor");
}

#[test]
fn the_dashboards_names_stay_out_of_debug() {
    let msg = Msg::Mentions {
        project_id: "project-a".into(),
        result: Ok(Names(vec!["board-marker-7a".into()])),
    };
    assert!(!format!("{msg:?}").contains("board-marker"));
}

// Review M6: inside the user's statement (the text before the cursor ends
// mid-statement), Ctrl+R only inserts and leaves that statement as it was:
// no `;` is added to it and nothing runs.
#[test]
fn ctrl_r_inside_a_statement_only_inserts() {
    for before in [
        "SELECT * FROM invoices WHERE ",
        "UPDATE invoices SET total = 0",
    ] {
        let mut m = asking(before);
        keys(&mut m, "x");
        update(&mut m, press(KeyCode::Enter));
        answer(&mut m, "SELECT 1");
        let effects = update(&mut m, ctrl('r'));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Run(_))),
            "{before}: {effects:?}"
        );
        assert_eq!(editor_text(&m), format!("{before}SELECT 1"));
        assert_eq!(ask(&m).note.as_deref(), Some(text::ASK_NOT_RUN_ALONE));
        assert_eq!(m.bar_context(), BarContext::AskDone);
    }
}

// Review M5: a name starting with `"` can't be written as a mention Core
// reads back (DuckDB's quoted catalog parts, `"fx.we""ird".main`), so it
// isn't listed.
#[test]
fn names_starting_with_a_quote_are_left_out_of_the_list() {
    let mut m = asking("");
    let mut odd = m.schema[0].clone();
    odd.schema = "\"fx.we\"\"ird\".main".into();
    odd.name = "events".into();
    m.schema.push(odd);
    m.saved_items[0].name = "\"quoted\" report".into();
    m.saved_items[1].name = "\"solo".into();
    keys(&mut m, "@");
    let labels: Vec<String> = mention_items(&m).into_iter().map(|i| i.label).collect();
    assert!(!labels.is_empty());
    assert!(labels.iter().all(|l| !l.starts_with('"')), "{labels:?}");
    assert!(labels.iter().any(|l| l == "public.invoices"));
}

// Review M8: Ctrl+R's gate is Core's read-only token check with the
// connection's engine (CLAUDE.md's cases): each of these goes in, not run.
#[test]
fn the_read_only_gate_follows_the_engine() {
    for (engine, sql) in [
        ("mysql", "SELECT 1 -- x\rDELETE FROM t"),
        ("duckdb", "SELECT * FROM query('DELETE FROM t')"),
        ("mariadb", "SELECT 1 /*M! , (SELECT 1 FROM t FOR UPDATE) */"),
    ] {
        let mut m = asking("");
        m.library.connections[0].engine = engine.into();
        keys(&mut m, "x");
        update(&mut m, press(KeyCode::Enter));
        answer(&mut m, sql);
        let effects = update(&mut m, ctrl('r'));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Run(_))),
            "{engine}: {effects:?}"
        );
        assert_eq!(editor_text(&m), sql, "{engine}");
        assert_eq!(
            ask(&m).note.as_deref(),
            Some(text::ask_not_run("").as_str()),
            "{engine}"
        );
    }
}
