//! Task 4 against a real Core on SQLite: a table opened from panel 2 reads
//! its page and metadata through Core (`table_page`, `table_metadata`, the
//! dialect's DDL), pages, filters and sorts with typed `TableQuery`s, and
//! every staging is planned by Core (`plan_edits`): a keyless table and a
//! view are refused with Core's `NOT_EDITABLE` message.

use crossterm::event::KeyCode;
use seaquel_core::ConnectRequest;

use crate::state::app::{Conn, Load, Model, Panel, TablesTab};
use crate::state::browse::Meta;
use crate::state::log::Tag;
use crate::state::panels::Row;
use crate::state::pending::Plan;
use crate::testing::core::{connection, project, Seed};
use crate::testing::harness::{Harness, HarnessOptions};

/// A SQLite file with 150 invoices, a table without a primary key and a
/// view.
pub(super) async fn seed() -> Seed {
    let seed = Seed::new().await;
    let file = seed.path().join("shop.db").to_string_lossy().into_owned();
    seed.with(|core, ws| async move {
        let project_id = project(&core, &ws, "Shop").await;
        let conn = connection(
            &core,
            &ws,
            serde_json::json!({"projectId": project_id, "name": "shop", "type": "sqlite",
                               "databaseName": file}),
        )
        .await;
        let id = ws
            .connect(
                &core,
                ConnectRequest::saved(&conn).with_create_if_missing(true),
            )
            .await
            .unwrap();
        for sql in [
            "CREATE TABLE invoices (id INTEGER PRIMARY KEY, customer TEXT NOT NULL, \
             total REAL DEFAULT 0, note TEXT)",
            "CREATE INDEX invoices_customer ON invoices (customer)",
            "CREATE TABLE events (at TEXT, what TEXT)",
            "CREATE VIEW big AS SELECT * FROM invoices WHERE total > 100",
            "INSERT INTO events VALUES ('2026-10-01', 'boot')",
        ] {
            ws.execute(&core, &id, sql, Vec::new()).await.unwrap();
        }
        for i in 1..=150 {
            ws.execute(
                &core,
                &id,
                &format!("INSERT INTO invoices (id, customer, total) VALUES ({i}, 'c{i}', {i})"),
                Vec::new(),
            )
            .await
            .unwrap();
        }
    })
    .await;
    seed
}

pub(super) async fn open_shop(seed: &Seed) -> Harness {
    open_shop_named(seed, "shop").await
}

pub(super) async fn open_shop_named(seed: &Seed, name: &str) -> Harness {
    let mut h = Harness::open(HarnessOptions {
        connection: Some(name),
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    h.until("connected and listed", |m| {
        matches!(m.conn, Conn::Connected { .. }) && m.schema_load == Load::Loaded
    })
    .await;
    h
}

/// Selects `name` in panel 2's current tab and presses Enter.
pub(super) fn open_table(h: &mut Harness, name: &str) {
    h.model.focus_panel(Panel::Tables);
    let index = h
        .model
        .table_rows()
        .iter()
        .position(|r| matches!(r, Row::Item(i) if h.model.schema[*i].name == name))
        .unwrap_or_else(|| panic!("{name} is listed"));
    h.model.list_mut(Panel::Tables).unwrap().selected = index;
    h.press(KeyCode::Enter);
}

pub(super) fn loaded(m: &Model) -> bool {
    m.browse.loading.is_none()
        && m.browse.page.is_some()
        && matches!(m.browse.meta, Meta::Loaded(_))
}

fn first_id(m: &Model) -> String {
    let page = m.browse.page.as_ref().unwrap();
    crate::state::grid::display(&page.rows[0][0])
}

#[tokio::test]
async fn a_table_pages_filters_and_sorts_through_core() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    assert_eq!(h.model.focus, Panel::Main);
    let page = h.model.browse.page.clone().unwrap();
    assert_eq!(page.columns, ["id", "customer", "total", "note"]);
    assert_eq!(page.rows.len(), 100);
    assert_eq!((page.total_rows, page.total_pages), (150, 2));
    assert_eq!(crate::state::grid::range_text(&page), "rows 1–100 of 150");
    assert!(
        h.model.log.last(5).any(|l| l.text.starts_with("SELECT")),
        "the command log shows Core's page query"
    );
    let Meta::Loaded(meta) = &h.model.browse.meta else {
        unreachable!()
    };
    assert_eq!(meta.primary_key(), ["id"]);
    let ddl = meta.ddl.as_ref().unwrap();
    assert!(
        ddl.contains("CREATE TABLE") && ddl.contains("invoices"),
        "{ddl}"
    );
    assert!(meta.indexes.iter().any(|i| i.name == "invoices_customer"));

    // n: the second page.
    h.keys("n");
    h.until("page 2", |m| {
        loaded(m) && m.browse.page.as_ref().is_some_and(|p| p.page == 2)
    })
    .await;
    assert_eq!(h.model.browse.page.as_ref().unwrap().rows.len(), 50);

    // F: customer LIKE 'c14%' (c14, c140–c149), page 1 again. (Core
    // compares as text, so a range operator on a number would too.)
    h.model.browse.col = 0;
    h.keys("F");
    h.press(KeyCode::Right);
    h.press(KeyCode::Tab);
    for _ in 0..6 {
        h.press(KeyCode::Right);
    }
    h.press(KeyCode::Tab);
    h.keys("c14%");
    h.press(KeyCode::Enter);
    h.until("filtered", |m| {
        loaded(m) && m.browse.page.as_ref().is_some_and(|p| p.rows.len() == 11)
    })
    .await;
    assert_eq!(h.model.browse.page.as_ref().unwrap().page, 1);

    // s twice on id: descending.
    h.model.browse.col = 0;
    h.keys("ss");
    h.until("sorted", |m| loaded(m) && first_id(m) == "149")
        .await;
    // Esc clears the filter (and keeps the sort).
    h.press(KeyCode::Esc);
    h.until("unfiltered", |m| {
        loaded(m) && m.browse.page.as_ref().is_some_and(|p| p.total_rows == 150)
    })
    .await;
    assert_eq!(first_id(&h.model), "150");
    h.close().await;
}

#[tokio::test]
async fn every_staging_is_planned_by_core() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    // An update of `customer` on id 1.
    h.model.browse.col = 1;
    h.keys("e");
    h.press(KeyCode::Backspace);
    h.keys("X");
    h.press(KeyCode::Enter);
    // Set default on `total` of id 2, a delete of id 3, and an insert.
    h.model.browse.row = 1;
    h.model.browse.col = 2;
    h.keys("D");
    h.model.browse.row = 2;
    h.keys("d");
    h.keys("a");
    h.model.browse.col = 1;
    h.keys("enew");
    h.press(KeyCode::Enter);
    let staged_lines = |m: &Model| {
        m.log
            .last(10)
            .filter(|l| matches!(l.tag, Some(Tag::Staged | Tag::StagedDelete)))
            .count()
    };
    h.until("four plans, logged", |m| {
        m.queue.entries().len() == 4
            && m.queue
                .entries()
                .iter()
                .all(|e| matches!(e.plan, Plan::Planned(_)))
            && staged_lines(m) == 4
    })
    .await;
    let sql: Vec<String> = h
        .model
        .queue
        .entries()
        .iter()
        .map(|e| match &e.plan {
            Plan::Planned(p) => p.sql.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert!(sql[0].starts_with("UPDATE"), "{sql:?}");
    assert!(
        sql[1].starts_with("UPDATE") && sql[1].contains("total"),
        "{sql:?}"
    );
    assert!(sql[2].starts_with("DELETE"), "{sql:?}");
    assert!(sql[3].starts_with("INSERT"), "{sql:?}");
    let mut staged: Vec<_> = h
        .model
        .log
        .last(10)
        .filter(|l| matches!(l.tag, Some(Tag::Staged | Tag::StagedDelete)))
        .map(|l| l.text.clone())
        .collect();
    // The plans answer in any order.
    let mut expected = sql.clone();
    staged.sort();
    expected.sort();
    assert_eq!(staged, expected, "the log shows Core's SQL");
    assert_eq!(h.model.staged.total(), 4);
    h.close().await;
}

/// A table with no primary key and a view: edits go to Core, which
/// refuses them with `NOT_EDITABLE`; its message is what the TUI says.
#[tokio::test]
async fn a_keyless_table_and_a_view_say_core_s_not_editable_message() {
    let seed = seed().await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "events");
    h.until("events", loaded).await;
    h.model.browse.col = 1;
    h.keys("e");
    h.keys("!");
    h.press(KeyCode::Enter);
    let refusals = |m: &Model| {
        m.log
            .last(20)
            .filter(|l| l.tag == Some(Tag::ReadOnly) && l.text.contains("no primary key"))
            .count()
    };
    h.until("refused", |m| refusals(m) == 1).await;
    assert!(h.model.queue.is_empty());
    h.keys("d");
    h.until("delete refused", |m| refusals(m) == 2).await;
    assert!(h.model.queue.is_empty());

    h.model.focus_panel(Panel::Tables);
    h.model.tables_tab = TablesTab::Views;
    h.model.refresh_lists();
    open_table(&mut h, "big");
    h.until("the view", |m| {
        loaded(m)
            && m.browse
                .opened
                .as_ref()
                .is_some_and(|o| o.target.table == "big")
    })
    .await;
    assert_eq!(h.model.browse.page.as_ref().unwrap().rows.len(), 50);
    h.model.browse.col = 1;
    h.keys("D");
    h.until("view refused", |m| refusals(m) == 3).await;
    let said = h.model.log.last(1).next().unwrap().text.clone();
    assert!(
        said.contains("big"),
        "Core's message names the view: {said}"
    );
    assert!(h.model.queue.is_empty());
    h.close().await;
}

/// Postgres (behind `SEAQUEL_TEST_POSTGRES`): a two-column primary key
/// keys an edit in the metadata's order, and a `jsonb` cell's typed text
/// is planned by Core with its cast.
#[tokio::test]
async fn postgres_keys_by_a_two_column_primary_key_and_casts_json() {
    use seaquel_core::domain::edits::Edit;
    use seaquel_core::secrets::{MemoryStore, SecretStore};
    use std::sync::Arc;

    let Some(config) = super::app_tests::live("POSTGRES") else {
        return;
    };
    let url = config["connection_string"].as_str().unwrap().to_string();
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store: store.clone(),
    };
    let table = crate::testing::live::unique_table("tui_browse");
    // Dropped when the test ends, failed or not.
    let _guard = crate::testing::live::TableGuard::new(&config, &format!("public.{table}"));
    let setup = table.clone();
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
            let id = ws
                .connect(&core, ConnectRequest::saved(&conn))
                .await
                .unwrap();
            for sql in [
                format!(
                    "CREATE TABLE public.{setup} (b text, a int, doc jsonb, PRIMARY KEY (a, b))"
                ),
                format!(
                    "INSERT INTO public.{setup} VALUES ('x', 1, '{{\"k\": 1}}'), ('y', 2, NULL)"
                ),
            ] {
                ws.execute(&core, &id, &sql, Vec::new()).await.unwrap();
            }
            conn
        })
        .await;
    store.set(&format!("db:{conn}"), "unused").await.ok();
    let mut h = open_shop_named(&seed, "pg").await;
    open_table(&mut h, &table);
    h.until("the page", loaded).await;
    let page = h.model.browse.page.clone().unwrap();
    assert_eq!(page.columns, ["b", "a", "doc"]);
    // The JSON cell shows as compact JSON.
    let row = page
        .rows
        .iter()
        .position(|r| crate::state::grid::display(&r[0]) == "x")
        .unwrap();
    assert_eq!(crate::state::grid::display(&page.rows[row][2]), "{\"k\":1}");
    h.model.browse.row = row;
    h.model.browse.col = 2;
    h.keys("e");
    for _ in 0..7 {
        h.press(KeyCode::Backspace);
    }
    h.keys("[1,2]");
    h.press(KeyCode::Enter);
    h.keys("d");
    h.until("planned", |m| {
        m.queue.entries().len() == 2
            && m.queue
                .entries()
                .iter()
                .all(|e| matches!(e.plan, Plan::Planned(_)))
    })
    .await;
    let entries = h.model.queue.entries();
    let Some(Edit::UpdateCell { key, .. }) = entries[0].edit() else {
        panic!("an update")
    };
    let names: Vec<_> = key.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(names, ["b", "a"], "the primary key in the metadata's order");
    let Plan::Planned(update) = &entries[0].plan else {
        unreachable!()
    };
    assert!(
        update.sql.contains("jsonb"),
        "Core casts the value: {}",
        update.sql
    );
    let Plan::Planned(delete) = &entries[1].plan else {
        unreachable!()
    };
    assert!(delete.sql.starts_with("DELETE"), "{}", delete.sql);
    h.close().await;
}

/// Applies the queue through Core (Task 5 will do this from the commit
/// dialog), clears it, and reads the page again.
async fn apply_and_reload(h: &mut Harness) {
    use seaquel_core::domain::edits::{ApplyChangesParams, ApplyOutcome};
    let changes = h
        .model
        .queue
        .entries()
        .iter()
        .filter_map(crate::state::pending::Entry::change)
        .collect::<Vec<_>>();
    let count = changes.len() as u32;
    let core_id = h.model.conn.core_id().unwrap().to_string();
    let outcome = h
        .session
        .ws
        .apply_changes(
            &h.session.core,
            ApplyChangesParams {
                connection_id: core_id,
                changes,
                confirmed: true,
                history: None,
            },
        )
        .await
        .unwrap();
    let ApplyOutcome::Applied {
        applied, failed, ..
    } = outcome
    else {
        panic!("confirm required")
    };
    assert!(failed.is_none(), "{failed:?}");
    assert_eq!(applied, count);
    h.model.queue.clear();
    let gen = h.model.browse.page_gen;
    h.keys("r");
    h.until("reloaded", |m| loaded(m) && m.browse.page_gen > gen)
        .await;
}

/// Edits the cell under the cursor to `text` (from empty) and waits for
/// its plan.
async fn stage(h: &mut Harness, row: usize, col: usize, text: &str) {
    h.model.browse.row = row;
    h.model.browse.col = col;
    h.keys("e");
    while !h.model.browse.editing.as_ref().unwrap().text.is_empty() {
        h.press(KeyCode::Backspace);
    }
    h.keys(text);
    h.press(KeyCode::Enter);
    h.until("planned", |m| {
        m.queue
            .entries()
            .iter()
            .all(|e| matches!(e.plan, Plan::Planned(_)))
    })
    .await;
}

fn cell(m: &Model, row: usize, column: &str) -> seaquel_core::Value {
    m.browse
        .page
        .as_ref()
        .unwrap()
        .value(row, column)
        .unwrap()
        .clone()
}

/// I2 on SQLite: a bytes cell edited as hex is saved as bytes, and a
/// bigint past 2^53 round-trips.
#[tokio::test]
async fn sqlite_round_trips_bytes_and_a_bigint() {
    use seaquel_core::Value;
    let seed = Seed::new().await;
    let file = seed.path().join("b.db").to_string_lossy().into_owned();
    seed.with(|core, ws| async move {
        let p = project(&core, &ws, "B").await;
        let conn = connection(
            &core,
            &ws,
            serde_json::json!({"projectId": p, "name": "shop", "type": "sqlite",
                               "databaseName": file}),
        )
        .await;
        let id = ws
            .connect(
                &core,
                ConnectRequest::saved(&conn).with_create_if_missing(true),
            )
            .await
            .unwrap();
        for sql in [
            "CREATE TABLE blobs (id INTEGER PRIMARY KEY, data BLOB, big INTEGER)",
            "INSERT INTO blobs VALUES (1, x'cafe', 9007199254740993)",
        ] {
            ws.execute(&core, &id, sql, Vec::new()).await.unwrap();
        }
    })
    .await;
    let mut h = open_shop(&seed).await;
    open_table(&mut h, "blobs");
    h.until("blobs", loaded).await;
    assert_eq!(cell(&h.model, 0, "data"), Value::Bytes(vec![0xca, 0xfe]));
    h.model.browse.col = 1;
    h.keys("e");
    assert_eq!(h.model.browse.editing.as_ref().unwrap().text, "\\xcafe");
    h.press(KeyCode::Esc);
    stage(&mut h, 0, 1, "\\xbeef").await;
    stage(&mut h, 0, 2, "9007199254740995").await;
    apply_and_reload(&mut h).await;
    assert_eq!(cell(&h.model, 0, "data"), Value::Bytes(vec![0xbe, 0xef]));
    assert_eq!(cell(&h.model, 0, "big"), Value::Int(9_007_199_254_740_995));
    h.close().await;
}

/// I2 on MySQL (behind `SEAQUEL_TEST_MYSQL`): hex into a bytes cell and
/// into a NULL `VARBINARY` cell is saved as bytes; a bigint and a decimal
/// round-trip exactly.
#[tokio::test]
async fn mysql_round_trips_bytes_a_bigint_and_a_decimal() {
    use seaquel_core::secrets::{MemoryStore, SecretStore};
    use seaquel_core::Value;
    use std::sync::Arc;

    let Some(config) = super::app_tests::live("MYSQL") else {
        return;
    };
    let url = config["connection_string"].as_str().unwrap().to_string();
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store: store.clone(),
    };
    let table = crate::testing::live::unique_table("tui_bytes");
    // Dropped when the test ends, failed or not.
    let _guard = crate::testing::live::TableGuard::new(&config, &table);
    let setup = table.clone();
    let conn = seed
        .with(|core, ws| async move {
            let p = project(&core, &ws, "My").await;
            let conn = connection(
                &core,
                &ws,
                serde_json::json!({"projectId": p, "name": "my", "type": "mysql",
                                   "connectionString": url, "savePassword": true}),
            )
            .await;
            let id = ws
                .connect(&core, ConnectRequest::saved(&conn))
                .await
                .unwrap();
            for sql in [
                format!(
                    "CREATE TABLE {setup} (id INT PRIMARY KEY, data VARBINARY(16), \
                     big BIGINT, amount DECIMAL(20,4))"
                ),
                format!(
                    "INSERT INTO {setup} VALUES (1, x'cafe', 9007199254740993, \
                     12345678901234.5678), (2, NULL, 1, 1)"
                ),
            ] {
                ws.execute(&core, &id, &sql, Vec::new()).await.unwrap();
            }
            conn
        })
        .await;
    // root has no password: a saved empty one.
    store.set(&format!("db:{conn}"), "").await.ok();
    let mut h = open_shop_named(&seed, "my").await;
    open_table(&mut h, &table);
    h.until("the table", loaded).await;
    let row = |m: &Model, id: i64| {
        m.browse
            .page
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| r[0] == Value::Int(id))
            .unwrap()
    };
    let (one, two) = (row(&h.model, 1), row(&h.model, 2));
    stage(&mut h, one, 1, "\\xbeef").await;
    stage(&mut h, two, 1, "\\x0102").await;
    stage(&mut h, one, 2, "9007199254740995").await;
    stage(&mut h, one, 3, "98765432109876.1234").await;
    apply_and_reload(&mut h).await;
    let (one, two) = (row(&h.model, 1), row(&h.model, 2));
    assert_eq!(cell(&h.model, one, "data"), Value::Bytes(vec![0xbe, 0xef]));
    assert_eq!(cell(&h.model, two, "data"), Value::Bytes(vec![1, 2]));
    assert_eq!(
        cell(&h.model, one, "big"),
        Value::Int(9_007_199_254_740_995)
    );
    assert_eq!(
        cell(&h.model, one, "amount"),
        Value::Decimal("98765432109876.1234".into())
    );
    h.close().await;
}

/// Probe F9 (behind `SEAQUEL_TEST_POSTGRES` and `SEAQUEL_TEST_MYSQL`): a
/// live test that fails still drops its table, through its guard, while
/// the panic unwinds.
#[tokio::test]
async fn a_failing_live_test_leaves_no_table_behind() {
    use crate::testing::live::{table_exists, unique_table, TableGuard};
    use seaquel_types::ConnectConfig;

    for (engine, qualify) in [("POSTGRES", "public."), ("MYSQL", "")] {
        let Some(config) = super::app_tests::live(engine) else {
            continue;
        };
        let table = format!("{qualify}{}", unique_table("tui_guard"));
        let core = crate::testing::core::app_core();
        let parsed: ConnectConfig = serde_json::from_value(config.clone()).unwrap();
        let id = core.connect(&parsed).await.unwrap().connection_id;
        core.execute(&id, &format!("CREATE TABLE {table} (x INT)"), Vec::new())
            .await
            .unwrap();
        core.disconnect(&id).await.unwrap();
        assert!(table_exists(&config, &table).await, "{engine}: made");
        let guard = TableGuard::new(&config, &table);
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("a live test failing");
        }));
        assert!(failed.is_err());
        assert!(!table_exists(&config, &table).await, "{engine}: dropped");
    }
}
