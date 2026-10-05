//! `Workspace::apply_changes` and `table_page` against real databases:
//! grid edits on composite and typed keys (Postgres casts), an atomic batch
//! whose third statement fails, MySQL/MariaDB DDL in an in-order batch, a
//! table page with each filter, a sort and its count, and SQL Server's
//! `sql_variant` pages and hand-opened transactions.
//!
//! Live: `SEAQUEL_TEST_POSTGRES`, `_MYSQL`, `_MARIADB` and `_MSSQL` as for
//! the engine smoke tests (each skipped when unset unless
//! `SEAQUEL_TEST_REQUIRE_ENGINES` is set). SQLite and DuckDB run on a file
//! and in memory, DuckDB through the helper (`common/duckdb.rs`; skipped
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
use seaquel_core::domain::edits::{ApplyChangesParams, TablePageParams};
use seaquel_core::{
    ConnectRequest, ConnectionForm, Core, SuppliedSecrets, Workspace, WorkspaceSpec,
};
use seaquel_engine::{ConnectConfig, Value};
use serde_json::{json, Value as Json};

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

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Live {
    _serial: tokio::sync::MutexGuard<'static, ()>,
    db: Db,
    core: Core,
    ws: Arc<Workspace>,
    id: String,
    /// A connection of Core's own, to look from outside the workspace's.
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
    let file = dir.path().join("live.db").to_str().unwrap().to_string();
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
            json!({"type": "sqlite", "databaseName": file}),
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
    /// The schema the connection's tables are listed under.
    fn schema(&self) -> &'static str {
        match self.db {
            Db::Postgres => "public",
            Db::Mysql | Db::Mariadb => "seaquel_test",
            Db::Mssql => "dbo",
            Db::Sqlite | Db::Duckdb => "main",
        }
    }

    fn target(&self, table: &str) -> Json {
        json!({"schema": self.schema(), "table": table})
    }

    async fn exec(&self, sql: &str) {
        self.ws
            .query(&self.core, &self.id, sql, vec![])
            .await
            .unwrap_or_else(|e| panic!("{:?} {sql}: {e:?}", self.db));
    }

    /// Rows as a fresh connection sees them (the workspace's own for SQLite
    /// and DuckDB, which have no second one here).
    async fn observe(&self, sql: &str) -> Vec<Vec<Value>> {
        let r = match &self.observer {
            Some(o) => self.core.query(o, sql, vec![]).await,
            None => self.ws.query(&self.core, &self.id, sql, vec![]).await,
        };
        r.unwrap_or_else(|e| panic!("{:?} {sql}: {e:?}", self.db))
            .rows
    }

    async fn apply(&self, changes: Json) -> Json {
        let params: ApplyChangesParams = serde_json::from_value(json!({
            "connectionId": self.id, "changes": changes, "confirmed": true,
        }))
        .unwrap();
        let out = tokio::time::timeout(LIMIT, self.ws.apply_changes(&self.core, params))
            .await
            .expect("the apply didn't end")
            .unwrap_or_else(|e| panic!("{:?}: {e:?}", self.db));
        serde_json::to_value(&out).unwrap()
    }

    async fn page(&self, query: Json, page: u32, page_size: u32) -> Vec<Json> {
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

    async fn drop_table(&self, t: &str) {
        let _ = self
            .ws
            .query(&self.core, &self.id, &format!("DROP TABLE {t}"), vec![])
            .await;
    }
}

fn table() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("seaquel_edit_{}", &id[..8])
}

fn edit(id: &str, edit: Json) -> Json {
    json!({"type": "edit", "id": id, "edit": edit})
}

fn int(rows: &[Vec<Value>]) -> i64 {
    rows[0][0].as_i64().unwrap_or_else(|| panic!("{rows:?}"))
}

#[tokio::test]
async fn grid_edits_on_a_composite_key() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!(
            "CREATE TABLE {t} (id INT NOT NULL, k VARCHAR(20) NOT NULL, v VARCHAR(50) DEFAULT 'dflt', \
             n INT, PRIMARY KEY (id, k))"
        ))
        .await;
        let target = l.target(&t);
        let key = json!([["id", 1], ["k", "a"]]);
        let got = l
            .apply(json!([edit(
                "c1",
                json!({"type": "insertRow", "target": target,
                "values": [["id", 1], ["k", "a"], ["v", "one"], ["n", 5]]})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?} insert: {got}");
        // The key in another order than the primary key's still edits.
        let got = l
            .apply(json!([edit(
                "c2",
                json!({"type": "updateCell", "target": target,
                "key": [["k", "a"], ["id", 1]], "column": "v", "value": "two"})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?} update: {got}");
        let rows = l.observe(&format!("SELECT v FROM {t}")).await;
        assert_eq!(rows, vec![vec![Value::Text("two".into())]], "{db:?}");
        let got = l
            .apply(json!([edit(
                "c3",
                json!({"type": "setDefault", "target": target,
                "key": key, "column": "v"})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?} set default: {got}");
        let got = l
            .apply(json!([edit(
                "c4",
                json!({"type": "updateCell", "target": target,
                "key": key, "column": "n", "value": null})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?} null: {got}");
        let rows = l.observe(&format!("SELECT v, n FROM {t}")).await;
        assert_eq!(
            rows,
            vec![vec![Value::Text("dflt".into()), Value::Null]],
            "{db:?}"
        );
        let got = l
            .apply(json!([edit(
                "c5",
                json!({"type": "deleteRow", "target": target, "key": key})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?} delete: {got}");
        // The key is stale now.
        let got = l
            .apply(json!([edit(
                "c6",
                json!({"type": "updateCell", "target": target,
                "key": key, "column": "v", "value": "x"})
            )]))
            .await;
        assert_eq!(got["failed"]["code"], "NO_ROWS_AFFECTED", "{db:?}: {got}");
        // A key that isn't the primary key runs nothing.
        let got = l
            .apply(json!([edit(
                "c7",
                json!({"type": "deleteRow", "target": target,
                "key": [["v", "x"]]})
            )]))
            .await;
        assert_eq!(got["failed"]["code"], "NOT_EDITABLE", "{db:?}: {got}");
        l.drop_table(&t).await;
    }
}

#[tokio::test]
async fn postgres_uuid_and_date_keys_are_cast() {
    let Some(l) = live(Db::Postgres).await else {
        return;
    };
    let t = table();
    l.exec(&format!(
        "CREATE TABLE {t} (id uuid, d date, note text DEFAULT 'x', meta jsonb, PRIMARY KEY (id, d))"
    ))
    .await;
    let target = l.target(&t);
    let key = json!([
        ["id", "0b9d2f5e-8c1a-4f6e-9d3b-2a7c5e1f4b60"],
        ["d", "2024-03-01"]
    ]);
    let got = l
        .apply(json!([edit(
            "c1",
            json!({"type": "insertRow", "target": target,
            "values": [["id", "0b9d2f5e-8c1a-4f6e-9d3b-2a7c5e1f4b60"], ["d", "2024-03-01"],
                       ["note", "a"], ["meta", [1, 2]]]})
        )]))
        .await;
    assert_eq!(got["applied"], 1, "{got}");
    // A JSON cell's array and number bind as JSON (Decision 19).
    for (i, value) in [
        json!([3]),
        json!(5),
        json!({"$sq": "json", "v": {"a": 1}}),
        json!("{\"b\": 2}"),
    ]
    .into_iter()
    .enumerate()
    {
        let got = l
            .apply(json!([edit(
                &format!("j{i}"),
                json!({"type": "updateCell", "target": target,
                "key": key, "column": "meta", "value": value})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{value}: {got}");
    }
    let rows = l.observe(&format!("SELECT meta->>'b' FROM {t}")).await;
    assert_eq!(rows, vec![vec![Value::Text("2".into())]]);
    for change in [
        json!({"type": "updateCell", "target": target, "key": key, "column": "note", "value": "b"}),
        json!({"type": "setDefault", "target": target, "key": key, "column": "note"}),
    ] {
        let got = l.apply(json!([edit("c", change)])).await;
        assert_eq!(got["applied"], 1, "{got}");
    }
    let rows = l.observe(&format!("SELECT note FROM {t}")).await;
    assert_eq!(rows, vec![vec![Value::Text("x".into())]]);
    let got = l
        .apply(json!([edit(
            "c",
            json!({"type": "deleteRow", "target": target, "key": key})
        )]))
        .await;
    assert_eq!(got["applied"], 1, "{got}");
    assert_eq!(
        int(&l.observe(&format!("SELECT COUNT(*) FROM {t}")).await),
        0
    );
    l.drop_table(&t).await;
}

/// Decision 19 on each engine with a JSON type: a JSON cell's array and
/// number bind as JSON and store as JSON; typed JSON text binds as text and
/// the database parses it. MariaDB's JSON is `longtext` with a
/// `json_valid` check, which its metadata reports as `json`.
#[tokio::test]
async fn json_cells_bind_as_json_and_typed_text_stays_text() {
    for db in [Db::Postgres, Db::Mysql, Db::Mariadb] {
        let Some(l) = live(db).await else { continue };
        let t = table();
        let json_type = if db == Db::Postgres { "jsonb" } else { "JSON" };
        l.exec(&format!(
            "CREATE TABLE {t} (id INT PRIMARY KEY, doc {json_type})"
        ))
        .await;
        if db == Db::Mariadb {
            let (columns, _) =
                l.ws.engine(&l.core, &l.id)
                    .unwrap()
                    .table_metadata(l.schema(), &t)
                    .await
                    .unwrap();
            assert_eq!(columns[1].ty, "json", "{columns:?}");
        }
        let target = l.target(&t);
        let insert = |id: i64, value: Json| {
            edit(
                &format!("c{id}"),
                json!({"type": "insertRow", "target": target, "values": [["id", id], ["doc", value]]}),
            )
        };
        for (id, value) in [
            (1, json!([1, "a"])),
            (2, json!(5)),
            (3, json!("{\"b\": 2}")),
            (4, json!(true)),
        ] {
            let got = l.apply(json!([insert(id, value.clone())])).await;
            assert_eq!(got["applied"], 1, "{db:?} {value}: {got}");
        }
        // An update of an array too.
        let got = l
            .apply(json!([edit(
                "u",
                json!({"type": "updateCell", "target": target,
                "key": [["id", 1]], "column": "doc", "value": [7]})
            )]))
            .await;
        assert_eq!(got["applied"], 1, "{db:?}: {got}");
        let type_of = if db == Db::Postgres {
            format!("SELECT jsonb_typeof(doc) FROM {t} ORDER BY id")
        } else {
            format!("SELECT JSON_TYPE(doc) FROM {t} ORDER BY id")
        };
        let types: Vec<String> = l
            .observe(&type_of)
            .await
            .into_iter()
            .map(|r| r[0].as_str().unwrap().to_ascii_lowercase())
            .collect();
        let number = if db == Db::Postgres {
            "number"
        } else {
            "integer"
        };
        assert_eq!(types, ["array", number, "object", "boolean"], "{db:?}");
        l.drop_table(&t).await;
    }
}

#[tokio::test]
async fn an_atomic_batch_whose_third_statement_fails_applies_nothing() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!(
            "CREATE TABLE {t} (id INT PRIMARY KEY, v VARCHAR(10))"
        ))
        .await;
        let target = l.target(&t);
        let insert = |id: &str, n: i64| {
            edit(
                id,
                json!({"type": "insertRow", "target": target, "values": [["id", n], ["v", "x"]]}),
            )
        };
        let got = l
            .apply(json!([insert("c1", 1), insert("c2", 2), insert("c3", 1)]))
            .await;
        assert_eq!(got["mode"], "atomic", "{db:?}: {got}");
        assert_eq!(got["applied"], 0, "{db:?}: {got}");
        assert_eq!(got["failed"]["id"], "c3", "{db:?}: {got}");
        assert_eq!(got["failed"]["index"], 2, "{db:?}: {got}");
        assert_eq!(
            int(&l.observe(&format!("SELECT COUNT(*) FROM {t}")).await),
            0,
            "{db:?}"
        );
        // And a batch that holds commits all of it.
        let got = l.apply(json!([insert("c1", 1), insert("c2", 2)])).await;
        assert_eq!(got["applied"], 2, "{db:?}: {got}");
        assert_eq!(
            int(&l.observe(&format!("SELECT COUNT(*) FROM {t}")).await),
            2,
            "{db:?}"
        );
        l.drop_table(&t).await;
    }
}

#[tokio::test]
async fn a_batch_with_ddl_on_mysql_keeps_what_ran() {
    for db in [Db::Mysql, Db::Mariadb] {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!("CREATE TABLE {t} (id INT PRIMARY KEY)"))
            .await;
        let typed = |id: &str, sql: String| json!({"type": "sql", "id": id, "sql": sql});
        let got = l
            .apply(json!([
                typed("c1", format!("INSERT INTO {t} (id) VALUES (1)")),
                typed("c2", format!("ALTER TABLE {t} ADD COLUMN note TEXT")),
                typed("c3", format!("INSERT INTO {t} (id) VALUES (1)")),
            ]))
            .await;
        assert_eq!(got["mode"], "inOrder", "{db:?}: {got}");
        assert_eq!(got["applied"], 2, "{db:?}: {got}");
        assert_eq!(got["ddl"], true);
        assert_eq!(got["failed"]["id"], "c3");
        let rows = l.observe(&format!("SELECT id, note FROM {t}")).await;
        assert_eq!(rows, vec![vec![Value::Int(1), Value::Null]], "{db:?}");
        l.drop_table(&t).await;
    }
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

#[tokio::test]
async fn a_table_page_with_each_filter_a_sort_and_the_count() {
    for db in ALL {
        let Some(l) = live(db).await else { continue };
        let t = table();
        l.exec(&format!(
            "CREATE TABLE {t} (id INT PRIMARY KEY, name VARCHAR(20), tag VARCHAR(20) NULL)"
        ))
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
            "INSERT INTO {t} (id, name, tag) VALUES {}",
            values.join(", ")
        ))
        .await;
        let query = |filters: Json, logic: &str, sort: Json| json!({"target": l.target(&t), "filters": filters, "logic": logic, "sort": sort});
        // Three pages of 100: a full one counts, the last is partial.
        let sort = json!([{"column": "id", "direction": "DESC"}]);
        let ev = l.page(query(json!([]), "AND", sort.clone()), 1, 100).await;
        assert_eq!(rows_of(&ev).len(), 100, "{db:?}: {ev:?}");
        assert_eq!(rows_of(&ev)[0][0], 250, "{db:?}");
        let d = done(&ev);
        assert_eq!(
            (d["totalRows"].clone(), d["totalPages"].clone()),
            (json!(250), json!(3)),
            "{db:?}"
        );
        assert_eq!(d["countEstimated"], false);
        let ev = l.page(query(json!([]), "AND", sort.clone()), 3, 100).await;
        assert_eq!(rows_of(&ev).len(), 50, "{db:?}");
        assert_eq!(rows_of(&ev)[49][0], 1, "{db:?}");
        assert_eq!(done(&ev)["totalRows"], 250);
        // Each operator; ranges compare text (a known quirk).
        for (filters, logic, want) in [
            (
                json!([{"column": "name", "op": "=", "value": "r7"}]),
                "AND",
                1,
            ),
            (
                json!([{"column": "name", "op": "!=", "value": "r7"}]),
                "AND",
                249,
            ),
            (
                json!([{"column": "name", "op": "LIKE", "value": "r1%"}]),
                "AND",
                111,
            ),
            (
                json!([{"column": "name", "op": "NOT LIKE", "value": "r1%"}]),
                "AND",
                139,
            ),
            (
                json!([{"column": "id", "op": "IN", "value": " 1, 2 ,3 ,"}]),
                "AND",
                3,
            ),
            (
                json!([{"column": "id", "op": "NOT IN", "value": "1,2,3"}]),
                "AND",
                247,
            ),
            (json!([{"column": "tag", "op": "IS NULL"}]), "AND", 125),
            (
                json!([{"column": "tag", "op": "IS NOT NULL", "value": "x"}]),
                "AND",
                125,
            ),
            (
                json!([{"column": "name", "op": ">=", "value": "r99"}, {"column": "name", "op": "<=", "value": "r99"}]),
                "AND",
                1,
            ),
            (
                json!([{"column": "name", "op": ">", "value": "r98"}, {"column": "name", "op": "<", "value": "r990"}]),
                "AND",
                1,
            ),
            (
                json!([{"column": "name", "op": "=", "value": "r1"}, {"column": "name", "op": "=", "value": "r2"}]),
                "OR",
                2,
            ),
        ] {
            let ev = l
                .page(query(filters.clone(), logic, json!([])), 1, 300)
                .await;
            assert_eq!(rows_of(&ev).len(), want, "{db:?} {filters}: {ev:?}");
            assert_eq!(done(&ev)["totalRows"], want, "{db:?} {filters}");
        }
        l.drop_table(&t).await;
    }
}

#[tokio::test]
async fn mssql_sql_variant_pages_and_a_hand_opened_transaction_refuses_a_batch() {
    let Some(l) = live(Db::Mssql).await else {
        return;
    };
    let t = table();
    l.exec(&format!(
        "CREATE TABLE {t} (id INT PRIMARY KEY, v sql_variant, g geography, name NVARCHAR(10))"
    ))
    .await;
    l.exec(&format!(
        "INSERT INTO {t} (id, v, g, name) VALUES (1, CAST(12 AS sql_variant), \
         geography::Point(1, 2, 4326), N'a'), (2, CAST(N'x' AS sql_variant), NULL, N'b')"
    ))
    .await;
    let query = json!({"target": l.target(&t), "filters": [{"column": "name", "op": "LIKE", "value": "%"}]});
    let ev = l.page(query, 1, 1).await;
    let rows = rows_of(&ev);
    assert_eq!(rows.len(), 1, "{ev:?}");
    assert_eq!(rows[0][1], "12");
    assert_eq!(done(&ev)["totalRows"], 2, "{ev:?}");

    // A transaction opened by hand on the held session: an atomic batch is
    // refused before it starts, and names no change. Through
    // `sp_executesql` the BEGIN reports error 266 (the count changed inside
    // EXECUTE) but leaves the transaction open.
    let _ =
        l.ws.query(&l.core, &l.id, "BEGIN TRANSACTION", vec![])
            .await;
    let trancount =
        l.ws.query(&l.core, &l.id, "SELECT @@TRANCOUNT", vec![])
            .await
            .unwrap()
            .rows;
    assert_eq!(int(&trancount), 1, "BEGIN left no transaction open");
    let insert = |id: &str, n: i64| {
        edit(
            id,
            json!({"type": "insertRow", "target": l.target(&t), "values": [["id", n]]}),
        )
    };
    let got = l.apply(json!([insert("c1", 10), insert("c2", 11)])).await;
    assert_eq!(got["applied"], 0, "{got}");
    assert_eq!(got["failed"]["code"], "TRANSACTION_OPEN", "{got}");
    assert!(got["failed"].get("id").is_none(), "{got}");
    assert!(got["failed"].get("index").is_none(), "{got}");
    // ROLLBACK reports 266 the same way, and ends it.
    let _ =
        l.ws.query(&l.core, &l.id, "ROLLBACK TRANSACTION", vec![])
            .await;
    let trancount =
        l.ws.query(&l.core, &l.id, "SELECT @@TRANCOUNT", vec![])
            .await
            .unwrap()
            .rows;
    assert_eq!(int(&trancount), 0);
    assert_eq!(
        int(&l
            .observe(&format!("SELECT COUNT(*) FROM {t} WHERE id >= 10"))
            .await),
        0
    );
    l.drop_table(&t).await;
}
