//! Task 5 against a real Core on SQLite: the commit dialog applies the
//! queue through Core's `apply_changes` with a history context, and the
//! outcome rules are the GUI's: a full success clears the queue, records
//! one history row per change and reads the page again; an atomic failure
//! rolls everything back and marks the change (`NO_ROWS_AFFECTED` names
//! its table and key); an in-order batch keeps what it didn't apply; a
//! `confirmRequired` reopens the dialog with Core's list.

use crossterm::event::KeyCode;
use seaquel_core::domain::edits::Change;
use seaquel_core::Value;

use super::browse_tests::{loaded, open_shop, open_table, seed};
use crate::state::app::{update, Effect, Modal, Model, Msg, Stamp};
use crate::state::commit::ApplyCall;
use crate::state::pending::Plan;
use crate::state::text;
use crate::testing::harness::Harness;
use crate::testing::keys::{key, press};

/// Edits `customer` of page rows `rows` (appending `X`), then waits for
/// every plan.
async fn stage_customers(h: &mut Harness, rows: &[usize]) {
    for &row in rows {
        h.model.browse.row = row;
        h.model.browse.col = 1;
        h.keys("eX");
        h.press(KeyCode::Enter);
    }
    h.until("planned", |m| all_planned(m, rows.len())).await;
}

fn all_planned(m: &Model, n: usize) -> bool {
    m.queue.entries().len() == n
        && m.queue
            .entries()
            .iter()
            .all(|e| matches!(e.plan, Plan::Planned(_)))
}

/// A column of the shop's invoices, read through the TUI's own Core.
async fn customers(h: &Harness) -> Vec<(i64, String)> {
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let result = h
        .session
        .ws
        .query(
            &h.session.core,
            &core_id,
            "SELECT id, customer FROM invoices WHERE id <= 4 ORDER BY id",
            Vec::new(),
        )
        .await
        .unwrap();
    result
        .rows
        .into_iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::Int(id), Value::Text(c)) => (*id, c.clone()),
            other => panic!("{other:?}"),
        })
        .collect()
}

async fn execute(h: &Harness, sql: &str) {
    let core_id = h.model.conn.core_id().unwrap().to_string();
    h.session
        .ws
        .execute(&h.session.core, &core_id, sql, Vec::new())
        .await
        .unwrap();
}

/// `c` then Enter without carrying the apply out: its call.
fn commit_call(h: &mut Harness) -> ApplyCall {
    update(&mut h.model, key('c'));
    let effects = update(&mut h.model, press(KeyCode::Enter));
    effects
        .into_iter()
        .find_map(|e| match e {
            Effect::Apply(call) => Some(call),
            _ => None,
        })
        .expect("an apply")
}

/// Runs `call` through the TUI's Core and hands the answer to `update`.
async fn apply(h: &mut Harness, call: &ApplyCall) {
    let result = h.session.apply(call).await;
    h.send(Msg::Applied {
        op: call.op,
        result,
        stamp: Stamp::default(),
    });
}

#[tokio::test]
async fn two_updates_and_a_delete_apply_atomically_with_history() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    stage_customers(&mut h, &[0, 1]).await;
    h.model.browse.row = 2;
    h.keys("d");
    h.until("planned", |m| all_planned(m, 3)).await;
    let gen = h.model.browse.page_gen;
    h.keys("c");
    h.press(KeyCode::Enter);
    h.until("applied and the page read again", |m| {
        m.committing.is_none() && m.queue.is_empty() && m.browse.page_gen > gen && loaded(m)
    })
    .await;
    assert_eq!(
        customers(&h).await,
        [
            (1, "c1X".to_string()),
            (2, "c2X".to_string()),
            (4, "c4".to_string())
        ]
    );
    let page = h.model.browse.page.as_ref().unwrap();
    assert_eq!(crate::state::grid::display(&page.rows[2][0]), "4", "3 went");
    // One history row per change, from Core's answer, newest first; and
    // the same rows stored, under the saved connection.
    assert_eq!(h.model.history_items.len(), 3);
    assert!(h.model.history_items[0].sql.starts_with("DELETE"));
    let conn = h.model.conn.id().unwrap().to_string();
    let stored = h.session.history(&conn).await.unwrap();
    let ids: Vec<_> = stored.iter().map(|r| r.id.clone()).collect();
    let shown: Vec<_> = h.model.history_items.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, shown);
    let rows =
        seaquel_core::storage::query_history::load_by_connection(h.session.ws.storage(), &conn)
            .await
            .unwrap();
    assert_eq!(rows[0].connection_name_snapshot, "shop");
    let texts: Vec<String> = h.model.log.last(12).map(|l| l.text.clone()).collect();
    assert!(texts.contains(&text::BEGIN.to_string()), "{texts:?}");
    assert!(texts.contains(&text::committed(3)), "{texts:?}");
    h.close().await;
}

#[tokio::test]
async fn a_failing_middle_change_rolls_everything_back_and_is_marked() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    stage_customers(&mut h, &[0, 1, 2]).await;
    // Row 2 goes under the staged edit.
    execute(&h, "DELETE FROM invoices WHERE id = 2").await;
    let failed = h.model.queue.entries()[1].id.clone();
    h.keys("c");
    h.press(KeyCode::Enter);
    h.until("answered", |m| m.committing.is_none()).await;
    assert_eq!(h.model.queue.entries().len(), 3, "everything kept");
    let mark = h.model.queue.failure(&failed).expect("marked");
    assert_eq!(mark.code, "NO_ROWS_AFFECTED");
    assert_eq!(mark.message, text::no_row("main.invoices", "id 2"));
    assert_eq!(
        customers(&h).await,
        [
            (1, "c1".to_string()),
            (3, "c3".to_string()),
            (4, "c4".to_string())
        ],
        "rolled back"
    );
    assert!(h.model.history_items.is_empty(), "nothing recorded");
    h.close().await;
}

#[tokio::test]
async fn a_mixed_ddl_and_dml_batch_applies_in_order_and_keeps_the_rest() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    stage_customers(&mut h, &[0, 1, 2]).await;
    execute(&h, "DELETE FROM invoices WHERE id = 2").await;
    let ids: Vec<String> = h
        .model
        .queue
        .entries()
        .iter()
        .map(|e| e.id.clone())
        .collect();
    // A typed DDL change after the first edit (the GUI's queue holds such
    // changes; the TUI's doesn't make them) turns the batch in-order.
    let mut call = commit_call(&mut h);
    call.changes.insert(
        1,
        Change::Sql {
            id: "ddl-1".into(),
            sql: "CREATE TABLE extra (x INTEGER)".into(),
            params: Vec::new(),
        },
    );
    apply(&mut h, &call).await;
    let left: Vec<String> = h
        .model
        .queue
        .entries()
        .iter()
        .map(|e| e.id.clone())
        .collect();
    assert_eq!(left, ids[1..], "the applied prefix left");
    assert_eq!(
        h.model.queue.failure(&ids[1]).map(|e| e.code.as_str()),
        Some("NO_ROWS_AFFECTED")
    );
    assert_eq!(
        customers(&h).await,
        [
            (1, "c1X".to_string()),
            (3, "c3".to_string()),
            (4, "c4".to_string())
        ]
    );
    assert!(
        h.effects
            .iter()
            .any(|e| matches!(e, Effect::LoadSchema { .. })),
        "a DDL change reads the tables again"
    );
    h.until("the new table listed", |m| {
        m.schema.iter().any(|t| t.name == "extra")
    })
    .await;
    assert_eq!(h.model.history_items.len(), 2, "the two that ran");
    h.close().await;
}

#[tokio::test]
async fn confirm_required_reopens_the_dialog_with_core_s_list() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    stage_customers(&mut h, &[0]).await;
    let mut call = commit_call(&mut h);
    assert!(!call.confirmed);
    call.changes.push(Change::Sql {
        id: "wide-delete".into(),
        sql: "DELETE FROM events".into(),
        params: Vec::new(),
    });
    apply(&mut h, &call).await;
    let Some(Modal::Commit(dialog)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    let (list, total) = dialog.from_core.clone().unwrap();
    assert_eq!(total, 1);
    assert_eq!(list[0].sql, "DELETE FROM events");
    assert_eq!(list[0].reason, "DELETE without WHERE");
    assert_eq!(customers(&h).await[0].1, "c1", "nothing ran");
    // Enter now confirms.
    let effects = update(&mut h.model, press(KeyCode::Enter));
    let again = effects
        .iter()
        .find_map(|e| match e {
            Effect::Apply(call) => Some(call.clone()),
            _ => None,
        })
        .unwrap();
    assert!(again.confirmed);
    h.close().await;
}
