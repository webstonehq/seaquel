//! Core with the remote DuckDB engine:
//! `with_plugins(|id| id != "duckdb").duckdb_helper(…)`, as the terminal
//! binaries build it, connects a `duckdb` target through
//! `Workspace::connect`, and the editor's run and page, the data tab's
//! table page and a grid edit go through the helper. The SQL is the native
//! cases' (`run_live.rs`, `edits_live.rs`).
//!
//! The helper is `SEAQUEL_TEST_DUCKDB_HELPER`, else the one built beside
//! the test binary (`common/duckdb.rs`); without either these tests are
//! skipped, and with `SEAQUEL_TEST_REQUIRE_ENGINES` set they fail. It is
//! installed into a folder of the test's own under `CARGO_TARGET_TMPDIR`.
#![cfg(all(
    feature = "workspace",
    feature = "storage",
    feature = "engine-duckdb-remote"
))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::edits::{ApplyChangesParams, TablePageParams};
use seaquel_core::domain::run::{PageParams, RunParams};
use seaquel_core::{
    ConnectRequest, ConnectionForm, Core, DuckdbHelper, SuppliedSecrets, Workspace, WorkspaceSpec,
};
use serde_json::{json, Value as Json};

const LIMIT: Duration = Duration::from_secs(60);

#[path = "common/duckdb.rs"]
mod duckdb_helper;

use duckdb_helper::{built_helper, install};

struct Live {
    core: Core,
    ws: Arc<Workspace>,
    id: String,
    _install: tempfile::TempDir,
    _data: tempfile::TempDir,
}

async fn live() -> Option<Live> {
    let bin = built_helper()?;
    let (install, helper) = install(&bin);
    let data = tempfile::tempdir().unwrap();
    let core = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(helper)
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(data.path()))
        .await
        .unwrap();
    let form: ConnectionForm = serde_json::from_value(
        json!({"name": "live", "type": "duckdb", "databaseName": ":memory:"}),
    )
    .unwrap();
    let req = ConnectRequest::form(form)
        .with_secrets(SuppliedSecrets::none())
        .with_create_if_missing(true);
    let id = ws.connect(&core, req).await.expect("connect");
    Some(Live {
        core,
        ws,
        id,
        _install: install,
        _data: data,
    })
}

impl Live {
    async fn exec(&self, sql: &str) {
        self.ws
            .query(&self.core, &self.id, sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    }

    async fn run(&self, fields: Json) -> Vec<Json> {
        let mut p = json!({"connectionId": self.id, "streamId": uuid::Uuid::new_v4().to_string(),
                           "text": "", "target": {"type": "all"}, "pageSize": 100});
        for (k, v) in fields.as_object().unwrap() {
            p[k] = v.clone();
        }
        let params: RunParams = serde_json::from_value(p).unwrap();
        let events =
            tokio::time::timeout(LIMIT, self.ws.run(&self.core, params).collect::<Vec<_>>())
                .await
                .expect("the run didn't end");
        events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    }

    async fn page(&self, source: &Json, page: u32, page_size: u32) -> Vec<Json> {
        let params: PageParams = serde_json::from_value(json!({
            "connectionId": self.id, "streamId": uuid::Uuid::new_v4().to_string(),
            "source": source, "page": page, "pageSize": page_size,
        }))
        .unwrap();
        let events =
            tokio::time::timeout(LIMIT, self.ws.page(&self.core, params).collect::<Vec<_>>())
                .await
                .expect("the page didn't end");
        events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    }

    async fn table_page(&self, query: Json, page: u32, page_size: u32) -> Vec<Json> {
        let params: TablePageParams = serde_json::from_value(json!({
            "connectionId": self.id, "streamId": uuid::Uuid::new_v4().to_string(),
            "query": query, "page": page, "pageSize": page_size,
        }))
        .unwrap();
        tokio::time::timeout(
            LIMIT,
            self.ws.table_page(&self.core, params).collect::<Vec<_>>(),
        )
        .await
        .expect("the page didn't end")
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
    }

    async fn apply(&self, changes: Json) -> Json {
        let params: ApplyChangesParams = serde_json::from_value(json!({
            "connectionId": self.id, "changes": changes, "confirmed": true,
        }))
        .unwrap();
        let out = tokio::time::timeout(LIMIT, self.ws.apply_changes(&self.core, params))
            .await
            .expect("the apply didn't end")
            .unwrap_or_else(|e| panic!("{e:?}"));
        serde_json::to_value(&out).unwrap()
    }
}

fn types(events: &[Json]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

fn rows_of(events: &[Json]) -> Vec<Json> {
    events
        .iter()
        .find(|e| e["type"] == "batch")
        .and_then(|b| b["rows"].as_array().cloned())
        .unwrap_or_default()
}

fn done(events: &[Json]) -> Json {
    events
        .iter()
        .find(|e| e["type"] == "statementDone")
        .cloned()
        .unwrap_or_else(|| panic!("{events:?}"))
}

fn edit(id: &str, edit: Json) -> Json {
    json!({"type": "edit", "id": id, "edit": edit})
}

/// The builder registers the remote engine once, under `duckdb`.
#[test]
fn the_helper_is_the_duckdb_engine() {
    let core = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(DuckdbHelper {
            dir: PathBuf::from("/nonexistent/bin/duckdb"),
            version: "2026.1.1".to_string(),
        })
        .build();
    assert_eq!(
        core.engine_ids()
            .iter()
            .filter(|id| **id == "duckdb")
            .count(),
        1
    );
    assert_eq!(core.duckdb_helper().unwrap().version, "2026.1.1");
}

/// The default plugins are the four engines without DuckDB (the native
/// driver is gone); DuckDB comes only from the helper's locator, once.
#[test]
fn default_plugins_have_no_duckdb_until_the_helper_is_added() {
    let mut ids = seaquel_core::with_default_plugins().build().engine_ids();
    ids.sort();
    assert_eq!(ids, ["mssql", "mysql", "postgres", "sqlite"]);
    let mut ids = seaquel_core::with_default_plugins()
        .duckdb_helper(DuckdbHelper {
            dir: PathBuf::from("/nonexistent/bin/duckdb"),
            version: "2026.1.1".to_string(),
        })
        .build()
        .engine_ids();
    ids.sort();
    assert_eq!(ids, ["duckdb", "mssql", "mysql", "postgres", "sqlite"]);
}

/// No helper installed: the connect fails at once, not after the connect's
/// timeout.
#[tokio::test]
async fn a_connect_without_the_helper_is_not_installed() {
    let data = tempfile::tempdir().unwrap();
    let core = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(DuckdbHelper {
            dir: data.path().join("bin").join("duckdb"),
            version: "2026.1.1".to_string(),
        })
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(data.path()))
        .await
        .unwrap();
    let form: ConnectionForm = serde_json::from_value(
        json!({"name": "live", "type": "duckdb", "databaseName": ":memory:"}),
    )
    .unwrap();
    let started = tokio::time::Instant::now();
    let e = ws
        .connect(&core, ConnectRequest::form(form))
        .await
        .unwrap_err();
    assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e:?}");
    assert!(started.elapsed() < Duration::from_millis(100));
}

/// `run_live.rs`'s DuckDB cases: status results stay utility results, and
/// a paged SELECT counts, pages and streams.
#[tokio::test]
async fn runs_and_pages() {
    let Some(l) = live().await else { return };
    for sql in [
        "SET threads = 2",
        "CREATE TABLE st (a int)",
        "CREATE TABLE st2 AS SELECT 1 AS a",
        "CREATE VIEW sv AS SELECT 1 AS a",
        "CHECKPOINT",
        "ATTACH ':memory:' AS m2",
        "PRAGMA threads = 2",
    ] {
        let ev = l.run(json!({"text": sql})).await;
        assert_eq!(
            types(&ev),
            ["statementStart", "statementDone", "done"],
            "{sql}: {ev:#?}"
        );
        assert_eq!(ev[1]["totalRows"], 0, "{sql}");
    }

    let sql = "SELECT range AS g FROM range(1, 251) ORDER BY g";
    let ev = l.run(json!({"text": sql})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"],
        "{ev:#?}"
    );
    assert_eq!(ev[0]["kind"], "page");
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 100);
    assert_eq!(ev[2]["totalRows"], 250);
    assert_eq!(ev[2]["totalPages"], 3);
    assert_eq!(ev[2]["countEstimated"], false);
    let ev = l.page(&ev[0]["source"], 3, 100).await;
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 50);
    assert_eq!(ev[1]["rows"][0][0], 201);
    assert_eq!(ev[2]["totalRows"], 250);
    // Page size 0 streams all of it.
    let ev = l.run(json!({"text": sql, "pageSize": 0})).await;
    assert_eq!(ev[0]["kind"], "stream");
    let rows: usize = ev
        .iter()
        .filter(|e| e["type"] == "batch")
        .map(|e| e["rows"].as_array().unwrap().len())
        .sum();
    assert_eq!(rows, 250);
    assert_eq!(ev[ev.len() - 2]["totalRows"], 250);
}

/// `edits_live.rs`'s table page (a sort, the count, two filters) and grid
/// edits on a composite key.
#[tokio::test]
async fn table_pages_and_grid_edits() {
    let Some(l) = live().await else { return };
    l.exec("CREATE TABLE tp (id INT PRIMARY KEY, name VARCHAR(20), tag VARCHAR(20) NULL)")
        .await;
    let values: Vec<String> = (1..=250)
        .map(|i| {
            let tag = if i % 2 == 0 {
                "NULL".to_string()
            } else {
                format!("'t{i}'")
            };
            format!("({i}, 'r{i}', {tag})")
        })
        .collect();
    l.exec(&format!(
        "INSERT INTO tp (id, name, tag) VALUES {}",
        values.join(", ")
    ))
    .await;
    let target = json!({"schema": "main", "table": "tp"});
    let query = |filters: Json, sort: Json| json!({"target": target, "filters": filters, "logic": "AND", "sort": sort});
    let sort = json!([{"column": "id", "direction": "DESC"}]);
    let ev = l.table_page(query(json!([]), sort.clone()), 1, 100).await;
    assert_eq!(rows_of(&ev).len(), 100, "{ev:?}");
    assert_eq!(rows_of(&ev)[0][0], 250);
    assert_eq!(done(&ev)["totalRows"], 250);
    assert_eq!(done(&ev)["totalPages"], 3);
    let ev = l.table_page(query(json!([]), sort), 3, 100).await;
    assert_eq!(rows_of(&ev).len(), 50);
    for (filters, want) in [
        (
            json!([{"column": "name", "op": "LIKE", "value": "r1%"}]),
            111,
        ),
        (json!([{"column": "tag", "op": "IS NULL"}]), 125),
    ] {
        let ev = l
            .table_page(query(filters.clone(), json!([])), 1, 300)
            .await;
        assert_eq!(rows_of(&ev).len(), want, "{filters}: {ev:?}");
        assert_eq!(done(&ev)["totalRows"], want, "{filters}");
    }

    l.exec(
        "CREATE TABLE ge (id INT NOT NULL, k VARCHAR(20) NOT NULL, v VARCHAR(50) DEFAULT 'dflt', \
         n INT, PRIMARY KEY (id, k))",
    )
    .await;
    let target = json!({"schema": "main", "table": "ge"});
    let key = json!([["id", 1], ["k", "a"]]);
    let got = l
        .apply(json!([edit(
            "c1",
            json!({"type": "insertRow", "target": target,
                   "values": [["id", 1], ["k", "a"], ["v", "one"], ["n", 5]]})
        )]))
        .await;
    assert_eq!(got["applied"], 1, "insert: {got}");
    // Two edits are one atomic batch: one transaction in the helper.
    let got = l
        .apply(json!([
            edit(
                "c2",
                json!({"type": "updateCell", "target": target,
                       "key": [["k", "a"], ["id", 1]], "column": "v", "value": "two"})
            ),
            edit(
                "c3",
                json!({"type": "updateCell", "target": target,
                       "key": key, "column": "n", "value": null})
            ),
        ]))
        .await;
    assert_eq!(got["applied"], 2, "update: {got}");
    let r =
        l.ws.query(&l.core, &l.id, "SELECT v, n FROM ge", vec![])
            .await
            .unwrap();
    assert_eq!(
        r.rows,
        vec![vec![
            seaquel_core::Value::Text("two".into()),
            seaquel_core::Value::Null
        ]]
    );
    let got = l
        .apply(json!([edit(
            "c4",
            json!({"type": "deleteRow", "target": target, "key": key})
        )]))
        .await;
    assert_eq!(got["applied"], 1, "delete: {got}");
    // The key is stale now.
    let got = l
        .apply(json!([edit(
            "c5",
            json!({"type": "updateCell", "target": target,
                   "key": key, "column": "v", "value": "x"})
        )]))
        .await;
    assert_eq!(got["failed"]["code"], "NO_ROWS_AFFECTED", "{got}");
}

impl Live {
    /// The pids of this test's helpers (started from its own install).
    #[cfg(unix)]
    fn helper_pids(&self) -> Vec<u32> {
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(self._install.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect()
    }
}

/// The `ConnectionClosed` events that arrive within `within` (the tests
/// that use it kill or watch helpers by pid: Unix only).
#[cfg(unix)]
async fn closed_events(
    events: &mut futures::stream::BoxStream<'static, seaquel_core::WorkspaceEvent>,
    within: Duration,
) -> Vec<(String, String, String)> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let seaquel_core::WorkspaceEvent::ConnectionClosed {
            connection_id,
            code,
            message,
        } = event
        {
            seen.push((connection_id, code, message));
        }
    }
    seen
}

/// A helper killed under a
/// connection makes Core take the connection out and announce it once as
/// `ConnectionClosed` with `CONNECTION_CLOSED` and the helper's message
/// (which the GUI's `handleConnectionClosed` shows); `db.alive` then leaves
/// it out and the next call is `CONNECTION_NOT_FOUND`.
#[cfg(unix)]
#[tokio::test]
async fn a_killed_helper_closes_its_connection_once() {
    let Some(live) = live().await else { return };
    let mut events = live.ws.events();
    live.exec("SELECT 1").await;
    let pids = live.helper_pids();
    assert_eq!(pids.len(), 1, "{pids:?}");
    assert!(std::process::Command::new("kill")
        .args(["-9", &pids[0].to_string()])
        .status()
        .unwrap()
        .success());
    let seen = closed_events(&mut events, Duration::from_secs(5)).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    let (id, code, message) = &seen[0];
    assert_eq!(id, &live.id);
    assert_eq!(code, "CONNECTION_CLOSED");
    assert!(message.contains("signal 9"), "{message}");
    assert!(live
        .ws
        .alive(&live.core, std::slice::from_ref(&live.id))
        .is_empty());
    let e = live
        .ws
        .query(&live.core, &live.id, "SELECT 1", vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, "CONNECTION_NOT_FOUND", "{e:?}");
}

/// A disconnect (and the helper's clean exit after it) announces nothing.
#[cfg(unix)]
#[tokio::test]
async fn a_disconnect_announces_nothing() {
    let Some(live) = live().await else { return };
    let mut events = live.ws.events();
    live.exec("SELECT 1").await;
    live.ws.disconnect(&live.core, &live.id).await.unwrap();
    assert!(closed_events(&mut events, Duration::from_secs(1))
        .await
        .is_empty());
    assert!(
        live.helper_pids().is_empty(),
        "the helper outlived its close"
    );
}

/// Review I1 (b): a window that connects the same saved DuckDB file again
/// (a reload's reconnect) gets the new connection: the old one, which
/// holds the file, is closed first and announced `CONNECTION_REPLACED`
/// once, and one connection is left.
#[cfg(unix)]
#[tokio::test]
async fn a_window_reconnecting_a_duckdb_file_replaces_its_old_connection() {
    let Some(live) = live().await else { return };
    let file = live._data.path().join("saved.duckdb");
    // A form connect doesn't create a DuckDB file: made through ATTACH.
    live.exec(&format!(
        "ATTACH '{}' AS f; CREATE TABLE f.t AS SELECT 5 AS a; DETACH f",
        file.display()
    ))
    .await;
    let form: ConnectionForm = serde_json::from_value(json!({
        "name": "saved", "type": "duckdb", "databaseName": file.to_str().unwrap()
    }))
    .unwrap();
    let request = || {
        ConnectRequest::form(form.clone())
            .with_secrets(SuppliedSecrets::none())
            .with_saved_connection_id(Some("conn-saved".to_string()))
            .with_origin(seaquel_core::WriteOrigin::new(Some("win-1")))
    };
    let mut events = live.ws.events();
    let old = live.ws.connect(&live.core, request()).await.unwrap();
    let new = match live.ws.connect(&live.core, request()).await {
        Ok(id) => id,
        Err(e) => panic!("the reconnect failed: {e:?}"),
    };
    let seen = closed_events(&mut events, Duration::from_millis(500)).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, old);
    assert_eq!(seen[0].1, "CONNECTION_REPLACED");
    let mut ids = live.ws.connection_ids(&live.core);
    ids.sort();
    let mut want = vec![live.id.clone(), new.clone()];
    want.sort();
    assert_eq!(ids, want);
    let r = live
        .ws
        .query(&live.core, &new, "SELECT a FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(r.rows.len(), 1);
}
