//! Core with the remote DuckDB engine (the DuckDB helper plan, Task 3):
//! `with_plugins(|id| id != "duckdb").duckdb_helper(…)`, as the terminal
//! binaries build it, connects a `duckdb` target through
//! `Workspace::connect`, and the editor's run and page, the data tab's
//! table page and a grid edit go through the helper. The SQL is the native
//! cases' (`run_live.rs`, `edits_live.rs`).
//!
//! `SEAQUEL_TEST_DUCKDB_HELPER` names a built helper (`cargo build -p
//! seaquel-duckdb`); without it these tests are skipped, and with
//! `SEAQUEL_TEST_REQUIRE_ENGINES` set they fail. The helper is installed
//! into a folder of the test's own under `CARGO_TARGET_TMPDIR`.
#![cfg(all(
    feature = "workspace",
    feature = "storage",
    feature = "engine-duckdb-remote"
))]

use std::path::{Path, PathBuf};
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

fn built_helper() -> Option<PathBuf> {
    match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            None
        }
    }
}

/// The app version the helper reports.
fn helper_version(bin: &Path) -> String {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("seaquel-duckdb ")
        .unwrap()
        .to_string()
}

/// `bin/duckdb/<version>/seaquel-duckdb` in a folder of its own, 0700.
fn install(bin: &Path) -> (tempfile::TempDir, DuckdbHelper) {
    let dir = tempfile::Builder::new()
        .prefix("duckdb-remote-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let version = helper_version(bin);
    let root = dir.path().join("bin").join("duckdb");
    let folder = root.join(&version);
    std::fs::create_dir_all(&folder).unwrap();
    #[cfg(unix)]
    for d in [root.parent().unwrap(), &root, &folder] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let to = folder.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(bin, &to).is_err() {
        std::fs::copy(bin, &to).unwrap();
    }
    (dir, DuckdbHelper { dir: root, version })
}

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
