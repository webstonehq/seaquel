//! `Workspace::run` and `Workspace::page` against real databases: a run of
//! several statements, row-returning `other` statements (Decision 18), a
//! paged SELECT with its count, a stream, bound and inlined parameters,
//! cancel on the server, and what a hand-typed transaction does on a pooled
//! connection (Decision 16: recorded, not asserted).
//!
//! Live: `SEAQUEL_TEST_POSTGRES`, `_MYSQL`, `_MARIADB` and `_MSSQL` as for
//! the engine smoke tests (each skipped when unset unless
//! `SEAQUEL_TEST_REQUIRE_ENGINES` is set). SQLite and DuckDB run on files
//! in a temp dir, DuckDB through the helper (`common/duckdb.rs`; skipped
//! without one unless `SEAQUEL_TEST_REQUIRE_ENGINES` is set).
#![cfg(all(
    feature = "workspace",
    feature = "storage",
    feature = "engine-postgres",
    feature = "engine-mysql",
    feature = "engine-sqlite",
    feature = "engine-mssql",
    feature = "engine-duckdb-remote"
))]

#[path = "common/duckdb.rs"]
mod duckdb_helper;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::run::{PageParams, RunEvent, RunParams};
use seaquel_core::{
    ConnectRequest, ConnectionForm, Core, SuppliedSecrets, Workspace, WorkspaceSpec,
};
use seaquel_engine::ConnectConfig;
use serde_json::{json, Value as Json};
use tokio::time::Instant;

const LIMIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Db {
    Postgres,
    Mysql,
    Mariadb,
    Mssql,
    Sqlite,
    Duckdb,
}

const ALL: [Db; 6] = [
    Db::Postgres,
    Db::Mysql,
    Db::Mariadb,
    Db::Mssql,
    Db::Sqlite,
    Db::Duckdb,
];

fn var(db: Db) -> Option<&'static str> {
    match db {
        Db::Postgres => Some("SEAQUEL_TEST_POSTGRES"),
        Db::Mysql => Some("SEAQUEL_TEST_MYSQL"),
        Db::Mariadb => Some("SEAQUEL_TEST_MARIADB"),
        Db::Mssql => Some("SEAQUEL_TEST_MSSQL"),
        Db::Sqlite | Db::Duckdb => None,
    }
}

fn config(db: Db) -> Option<Option<ConnectConfig>> {
    let Some(var) = var(db) else {
        return Some(None);
    };
    match std::env::var(var) {
        Ok(raw) => Some(Some(serde_json::from_str(&raw).expect(var))),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("{var} is not set")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

/// The tests here run one at a time, so the Decision 16 record and the
/// cancel check see only their own sessions on the shared servers.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Live {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    db: Db,
    core: Core,
    ws: Arc<Workspace>,
    id: String,
    /// A connection of Core's own, for looking at the server from outside
    /// the run.
    observer: Option<String>,
    _dir: tempfile::TempDir,
    /// The DuckDB helper's install, for as long as Core.
    _helper: Option<tempfile::TempDir>,
}

async fn live(db: Db) -> Option<Live> {
    let config = config(db)?;
    let serial = SERIAL.lock().await;
    let (plugins, helper) = duckdb_helper::default_plugins();
    if db == Db::Duckdb && helper.is_none() {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let core = plugins
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let file = |ext: &str| {
        dir.path()
            .join(format!("live.{ext}"))
            .to_str()
            .unwrap()
            .to_string()
    };
    let (form, secrets) = match db {
        Db::Postgres | Db::Mysql | Db::Mariadb => {
            let ty = match db {
                Db::Postgres => "postgres",
                Db::Mysql => "mysql",
                _ => "mariadb",
            };
            let s = config.as_ref().unwrap().connection_string.clone().unwrap();
            (
                json!({"type": ty, "connectionString": s}),
                SuppliedSecrets::none(),
            )
        }
        Db::Mssql => {
            let c = config.as_ref().unwrap();
            (
                json!({"type": "mssql", "host": c.host, "port": c.port, "username": c.username,
                       "databaseName": "master"}),
                SuppliedSecrets::db(c.password.clone().unwrap()),
            )
        }
        Db::Sqlite => (
            json!({"type": "sqlite", "databaseName": file("db")}),
            SuppliedSecrets::none(),
        ),
        Db::Duckdb => (
            json!({"type": "duckdb", "databaseName": ":memory:"}),
            SuppliedSecrets::none(),
        ),
    };
    let mut f = json!({"name": "live"});
    for (k, v) in form.as_object().unwrap() {
        f[k] = v.clone();
    }
    let form: ConnectionForm = serde_json::from_value(f).unwrap();
    let req = ConnectRequest::form(form)
        .with_secrets(secrets)
        .with_create_if_missing(true);
    let id = ws.connect(&core, req).await.expect("connect");
    let observer = match &config {
        Some(c) => Some(core.connect(c).await.expect("observer").connection_id),
        None => None,
    };
    Some(Live {
        _serial: serial,
        db,
        core,
        ws,
        id,
        observer,
        _dir: dir,
        _helper: helper,
    })
}

impl Live {
    fn params(&self, fields: Json) -> RunParams {
        let mut p = json!({"connectionId": self.id, "streamId": uuid::Uuid::new_v4().to_string(),
                           "text": "", "target": {"type": "all"}, "pageSize": 100});
        for (k, v) in fields.as_object().unwrap() {
            p[k] = v.clone();
        }
        serde_json::from_value(p).unwrap()
    }

    async fn run(&self, fields: Json) -> Vec<Json> {
        let events = tokio::time::timeout(
            LIMIT,
            self.ws
                .run(&self.core, self.params(fields))
                .collect::<Vec<_>>(),
        )
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

    async fn exec(&self, sql: &str) {
        self.ws
            .query(&self.core, &self.id, sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{:?} {sql}: {e:?}", self.db));
    }

    async fn observe(&self, sql: &str) -> Vec<Vec<seaquel_engine::Value>> {
        self.core
            .query(self.observer.as_ref().unwrap(), sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
            .rows
    }
}

fn types(events: &[Json]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

fn table() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("seaquel_run_{}", &id[..8])
}

#[tokio::test]
async fn run_all_with_a_select_an_insert_a_utility_and_a_failure() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!("CREATE TABLE {t} (a int)")).await;
        let utility = match db {
            Db::Postgres => "SET search_path TO public",
            Db::Mysql | Db::Mariadb => "SET @seaquel_x = 1",
            Db::Mssql => "SET NOCOUNT OFF",
            Db::Sqlite => "PRAGMA foreign_keys = ON",
            Db::Duckdb => "SET threads = 2",
        };
        let text = format!(
            "SELECT 1 AS a;\nINSERT INTO {t} VALUES (1);\n{utility};\nSELECT nope_column FROM {t};\nSELECT a FROM {t};"
        );
        let ev = l.run(json!({"text": text})).await;
        assert_eq!(
            types(&ev),
            [
                "statementStart",
                "batch",
                "statementDone",
                "statementStart",
                "statementDone",
                "statementStart",
                "statementDone",
                "statementStart",
                "statementError",
                "statementStart",
                "batch",
                "statementDone",
                "done"
            ],
            "{db:?}: {ev:#?}"
        );
        assert_eq!(ev[1]["rows"][0][0], 1, "{db:?}");
        assert_eq!(ev[4]["rowsAffected"], 1, "{db:?}");
        assert_eq!(ev[10]["rows"], json!([[1]]), "{db:?}");
        assert_eq!(ev.last().unwrap()["succeeded"], false);
        l.exec(&format!("DROP TABLE {t}")).await;
    }
}

#[tokio::test]
async fn row_returning_other_statements_show_their_rows() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let sqls: Vec<&str> = match db {
            Db::Postgres => vec![
                "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
                "EXPLAIN SELECT 1",
                "SHOW search_path",
            ],
            Db::Mysql | Db::Mariadb => vec![
                "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
                "SHOW TABLES",
                "EXPLAIN SELECT 1",
            ],
            Db::Mssql => vec!["WITH x AS (SELECT 1 AS a) SELECT a FROM x"],
            Db::Sqlite => vec![
                "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
                "PRAGMA database_list",
                "VALUES (1), (2)",
            ],
            Db::Duckdb => vec![
                "FROM range(3)",
                "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
                "PRAGMA database_list",
                "DESCRIBE SELECT 1 AS a",
            ],
        };
        for sql in sqls {
            let ev = l.run(json!({"text": sql})).await;
            assert_eq!(
                types(&ev),
                ["statementStart", "batch", "statementDone", "done"],
                "{db:?} {sql}: {ev:#?}"
            );
            assert_eq!(ev[0]["queryType"], "other", "{db:?} {sql}");
            assert_eq!(ev[0]["kind"], "utility");
            assert!(
                !ev[1]["columns"].as_array().unwrap().is_empty(),
                "{db:?} {sql}"
            );
            assert_eq!(
                ev[2]["totalRows"].as_u64().unwrap() as usize,
                ev[1]["rows"].as_array().unwrap().len()
            );
        }
    }
}

/// DuckDB answers statements without rows with a status column (`Success`
/// or `Count`); they stay hidden utility results.
#[tokio::test]
async fn duckdb_status_results_stay_utility_results() {
    let Some(l) = live(Db::Duckdb).await else {
        return;
    };
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
    // A query whose column happens to be named like a status keeps its rows.
    for sql in [
        "WITH x AS (SELECT 1 AS a) SELECT count(*) AS Count FROM x",
        "FROM range(3) SELECT count(*) AS Count",
    ] {
        let ev = l.run(json!({"text": sql})).await;
        assert_eq!(
            types(&ev),
            ["statementStart", "batch", "statementDone", "done"],
            "{sql}: {ev:#?}"
        );
        assert_eq!(ev[1]["columns"], json!(["Count"]), "{sql}");
        assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 1, "{sql}");
    }
}

#[tokio::test]
async fn a_paged_select_counts_and_a_stream_sends_everything() {
    for db in [Db::Postgres, Db::Duckdb, Db::Sqlite] {
        let Some(l) = live(db).await else { continue };
        let sql = match db {
            Db::Postgres => "SELECT g FROM generate_series(1, 250) g ORDER BY g",
            Db::Duckdb => "SELECT range AS g FROM range(1, 251) ORDER BY g",
            _ => {
                l.exec("CREATE TABLE s (g int)").await;
                l.exec("INSERT INTO s WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 250) SELECT x FROM c").await;
                "SELECT g FROM s ORDER BY g"
            }
        };
        let ev = l.run(json!({"text": sql})).await;
        assert_eq!(
            types(&ev),
            ["statementStart", "batch", "statementDone", "done"],
            "{db:?}: {ev:#?}"
        );
        assert_eq!(ev[0]["kind"], "page");
        assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 100);
        assert_eq!(ev[2]["totalRows"], 250, "{db:?}");
        assert_eq!(ev[2]["totalPages"], 3);
        assert_eq!(ev[2]["countEstimated"], false);
        let ev = l.page(&ev[0]["source"], 3, 100).await;
        assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 50, "{db:?}");
        assert_eq!(ev[1]["rows"][0][0], 201, "{db:?}");
        assert_eq!(ev[2]["totalRows"], 250);
        // Page size 0 streams all of it.
        let ev = l.run(json!({"text": sql, "pageSize": 0})).await;
        assert_eq!(ev[0]["kind"], "stream");
        let rows: usize = ev
            .iter()
            .filter(|e| e["type"] == "batch")
            .map(|e| e["rows"].as_array().unwrap().len())
            .sum();
        assert_eq!(rows, 250, "{db:?}");
        assert_eq!(ev[ev.len() - 2]["totalRows"], 250);
    }
}

/// Decision 5: an empty page carries its column names, on every engine
/// (the sqlx engines read them from the prepared statement when no row
/// comes back).
#[tokio::test]
async fn an_empty_page_carries_its_columns() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        for sql in [
            "SELECT 1 AS a, 2 AS b WHERE 1 = 0",
            "SELECT 1 AS a, 2 AS b WHERE 1 = 0 -- note",
        ] {
            for page_size in [100, 0] {
                let ev = l.run(json!({"text": sql, "pageSize": page_size})).await;
                assert_eq!(
                    types(&ev),
                    ["statementStart", "batch", "statementDone", "done"],
                    "{db:?}: {ev:#?}"
                );
                assert_eq!(
                    ev[1]["columns"],
                    json!(["a", "b"]),
                    "{db:?} {sql} {page_size}"
                );
                assert_eq!(ev[1]["rows"], json!([]));
                assert_eq!(ev[2]["totalRows"], 0);
            }
        }
    }
}

/// A trailing `--` comment doesn't swallow the page's LIMIT: a million
/// rows page as 100, counted.
#[tokio::test]
async fn a_trailing_comment_pages_on_postgres() {
    let Some(l) = live(Db::Postgres).await else {
        return;
    };
    let sql = "SELECT g FROM generate_series(1, 1000000) g -- note";
    let ev = l.run(json!({"text": sql})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"],
        "{ev:#?}"
    );
    assert_eq!(ev[0]["kind"], "page");
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 100);
    assert_eq!(ev[2]["totalRows"], 1_000_000);
    let ev = l.page(&ev[0]["source"], 3, 100).await;
    assert_eq!(ev[1]["rows"][0][0], 201);
}

#[tokio::test]
async fn bound_and_inlined_parameters() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let sql = match db {
            Db::Postgres => "SELECT CAST({{a}} AS int) + 1 AS b, {{s}} AS s",
            _ => "SELECT {{a}} + 1 AS b, {{s}} AS s",
        };
        let ev = l
            .run(json!({"text": sql, "params": [{"name": "a", "value": 5}, {"name": "s", "value": "it's"}]}))
            .await;
        assert_eq!(
            types(&ev),
            ["statementStart", "batch", "statementDone", "done"],
            "{db:?}: {ev:#?}"
        );
        let binds = ev[0]["source"]["params"].as_array().unwrap().len();
        match db {
            Db::Mssql | Db::Duckdb => assert_eq!(binds, 0, "{db:?} inlines"),
            _ => assert_eq!(binds, 2, "{db:?} binds"),
        }
        let b = &ev[1]["rows"][0][0];
        assert!(
            b == 6 || b == "6" || b == &json!({"$sq": "decimal", "v": "6"}),
            "{db:?}: {b}"
        );
        assert_eq!(ev[1]["rows"][0][1], "it's", "{db:?}");
    }
}

#[tokio::test]
async fn cancel_stops_the_statement_on_the_server_and_the_rest_never_run() {
    for db in [Db::Postgres, Db::Mysql] {
        let Some(l) = live(db).await else { continue };
        let marker = format!(
            "seaquel_run_cancel_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let (sleep, running) = match db {
            Db::Postgres => (
                format!("SELECT pg_sleep(30) AS {marker}"),
                format!("SELECT count(*) FROM pg_stat_activity WHERE state = 'active' AND pid <> pg_backend_pid() AND query LIKE '%{marker}%'"),
            ),
            _ => (
                format!("SELECT SLEEP(30) AS {marker}"),
                format!("SELECT COUNT(*) FROM information_schema.PROCESSLIST WHERE COMMAND IN ('Query', 'Execute') AND ID <> CONNECTION_ID() AND INFO LIKE '%{marker}%'"),
            ),
        };
        let params = l.params(json!({"text": format!("{sleep};\nSELECT 42 AS after_{marker}")}));
        let stream_id = params.stream_id.clone();
        let count = |rows: Vec<Vec<seaquel_engine::Value>>| rows[0][0].as_i64().unwrap();
        let drive = l.ws.run(&l.core, params).collect::<Vec<RunEvent>>();
        let control = async {
            let deadline = Instant::now() + Duration::from_secs(10);
            while count(l.observe(&running).await) != 1 {
                assert!(Instant::now() < deadline, "{db:?}: the sleep never started");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            l.ws.cancel(&l.core, &stream_id);
            Instant::now()
        };
        let (events, cancelled) =
            tokio::time::timeout(LIMIT, futures::future::join(drive, control))
                .await
                .expect("the run never ended");
        let events: Vec<Json> = events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect();
        assert_eq!(types(&events), ["statementStart"], "{db:?}: {events:#?}");
        let deadline = cancelled + Duration::from_secs(2);
        loop {
            if count(l.observe(&running).await) == 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{db:?}: still running 2 s after the cancel"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Decision 16: a hand-typed transaction on a pooled connection. Records
/// what happens for the follow-up; asserts only that the run finishes.
#[tokio::test]
async fn a_typed_transaction_on_a_pool_is_recorded() {
    for db in [Db::Postgres, Db::Mysql] {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!("CREATE TABLE {t} (a int)")).await;
        l.exec(&format!("INSERT INTO {t} VALUES (1)")).await;
        let text = format!("BEGIN;\nUPDATE {t} SET a = 2 WHERE a = 1;\nCOMMIT;");
        let ev = l.run(json!({"text": text})).await;
        assert_eq!(ev.last().unwrap()["type"], "done", "{db:?}: {ev:#?}");
        let visible = l.observe(&format!("SELECT a FROM {t}")).await;
        let idle = match db {
            Db::Postgres => l
                .observe("SELECT count(*) FROM pg_stat_activity WHERE state = 'idle in transaction' AND pid <> pg_backend_pid()")
                .await,
            _ => l
                .observe("SELECT COUNT(*) FROM information_schema.INNODB_TRX")
                .await,
        };
        eprintln!(
            "DECISION 16 {db:?}: events {:?} {:?}; a fresh connection sees a = {:?}; open transactions left: {:?}",
            types(&ev),
            ev.iter().filter(|e| e["type"] == "statementError").map(|e| e["message"].clone()).collect::<Vec<_>>(),
            visible,
            idle
        );
        l.exec(&format!("DROP TABLE {t}")).await;
    }
}
