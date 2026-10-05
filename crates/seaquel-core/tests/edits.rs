//! `Workspace::plan_edits`, `apply_changes`, `table_page` and
//! `duckdb_extension` on a scripted mock driver: the edit fixtures
//! (`crates/seaquel-workspace/tests/fixtures/edits`, recorded from today's
//! TypeScript; see their README, "The Rust replay") replayed through Core,
//! and the edits service's lifecycle (validation before execution, the
//! apply modes, confirmation, history, cancel, ownership, limits, logs).

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::edits::{
    ApplyChangesParams, ApplyOutcome, Change, Edit, EditLimits, ExtensionAction, PlanEditsParams,
    TablePageParams,
};
use seaquel_core::domain::run::RunEvent;
use seaquel_core::storage::{connections, projects, query_history};
use seaquel_core::{ConnectRequest, Core, Executor, SuppliedSecrets, Workspace, WorkspaceSpec};
use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, ConnectConfig, DbError, Dialect, Driver, Engine,
    ExecuteResult, QueryResult, SchemaColumn, SchemaIndex, StreamBatch, TransactionError, Value,
};
use seaquel_runtime::BoxFuture;
use serde_json::{json, Value as Json};
use tokio::sync::Notify;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);
const SAVED: &str = "saved-1";

// ── The scripted driver ──

#[derive(Debug, Clone)]
struct Expect {
    call: &'static str,
    sql: String,
    params: Json,
    answer: Json,
}

fn expect(call: &'static str, sql: &str, params: Json, answer: Json) -> Expect {
    Expect {
        call,
        sql: sql.to_string(),
        params,
        answer,
    }
}

/// How the driver behaves beyond its script.
#[derive(Clone, Copy, PartialEq)]
enum Hold {
    No,
    /// `query` and `execute` never answer; streams hold a "pooled
    /// connection" until dropped.
    Calls,
    /// A stream yields up to 10,000 one-row batches, counting them.
    Endless,
}

#[derive(Default)]
struct Script {
    expected: VecDeque<Expect>,
    problems: Vec<String>,
    calls: Vec<(&'static str, String)>,
}

/// A table page's answers: the page from `matching`, the count from
/// `count`, checked against the SQL Core must send.
#[derive(Clone)]
struct PageScript {
    page_sql: String,
    count_sql: String,
    params: Json,
    columns: Vec<String>,
    matching: Vec<Vec<Value>>,
    offset: usize,
    limit: usize,
    count: Json,
    page_error: Option<Json>,
}

struct ScriptedDriver {
    script: Mutex<Script>,
    page: Mutex<Option<PageScript>>,
    /// `schema.table` → the metadata answer (`{"columns"}` or `{"error"}`).
    metadata: Mutex<HashMap<(String, String), Json>>,
    metadata_reads: Mutex<Vec<(String, String)>>,
    hold: Mutex<Hold>,
    released: AtomicBool,
    release: Notify,
    pulled: AtomicU64,
}

impl ScriptedDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            page: Mutex::default(),
            metadata: Mutex::default(),
            metadata_reads: Mutex::default(),
            hold: Mutex::new(Hold::No),
            released: AtomicBool::new(true),
            release: Notify::new(),
            pulled: AtomicU64::new(0),
        })
    }

    fn load(&self, expected: Vec<Expect>) {
        *self.script.lock().unwrap() = Script {
            expected: expected.into(),
            ..Script::default()
        };
    }

    fn set_metadata(&self, entries: &Json) {
        let mut m = self.metadata.lock().unwrap();
        m.clear();
        for e in entries.as_array().into_iter().flatten() {
            m.insert(
                (
                    e["schema"].as_str().unwrap().into(),
                    e["table"].as_str().unwrap().into(),
                ),
                json!({"columns": e["columns"]}),
            );
        }
        self.metadata_reads.lock().unwrap().clear();
    }

    fn set_metadata_answer(&self, schema: &str, table: &str, answer: Json) {
        self.metadata
            .lock()
            .unwrap()
            .insert((schema.into(), table.into()), answer);
    }

    fn metadata_reads(&self) -> Vec<(String, String)> {
        self.metadata_reads.lock().unwrap().clone()
    }

    fn set_hold(&self, hold: Hold) {
        *self.hold.lock().unwrap() = hold;
    }

    fn problems(&self) -> Vec<String> {
        let s = self.script.lock().unwrap();
        let mut out = s.problems.clone();
        for left in &s.expected {
            out.push(format!(
                "expected but not called: {} {:?}",
                left.call, left.sql
            ));
        }
        out
    }

    fn calls(&self) -> Vec<(&'static str, String)> {
        self.script.lock().unwrap().calls.clone()
    }

    fn take(&self, call: &'static str, sql: &str, params: &[Value]) -> Result<Json, DbError> {
        let mut s = self.script.lock().unwrap();
        s.calls.push((call, sql.to_string()));
        let params = serde_json::to_value(params).unwrap();
        match s.expected.pop_front() {
            Some(e) if e.call == call && e.sql == sql && e.params == params => Ok(e.answer),
            other => {
                let problem = format!("unexpected {call} {sql:?} {params}; expected {other:?}");
                s.problems.push(problem.clone());
                Err(DbError::query_error(problem))
            }
        }
    }

    fn record(&self, call: &'static str, sql: &str) {
        self.script
            .lock()
            .unwrap()
            .calls
            .push((call, sql.to_string()));
    }

    fn problem(&self, problem: String) -> DbError {
        self.script.lock().unwrap().problems.push(problem.clone());
        DbError::query_error(problem)
    }
}

fn values(rows: &Json) -> Vec<Vec<Value>> {
    rows.as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .unwrap()
                        .iter()
                        .map(|v| Value::from_wire(v.clone()).unwrap())
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn names(columns: &Json) -> Vec<String> {
    columns
        .as_array()
        .map(|c| c.iter().map(|n| n.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn answer_error(answer: &Json) -> Option<DbError> {
    answer.get("error").map(|e| DbError {
        code: e["code"].as_str().unwrap().to_string(),
        message: e["message"].as_str().unwrap().to_string(),
    })
}

/// Records `("dropped", call)` when a held call's future is dropped.
struct Dropped<'a>(&'a ScriptedDriver, &'static str);

impl Drop for Dropped<'_> {
    fn drop(&mut self) {
        self.0.record("dropped", self.1);
    }
}

struct Checkout<'a>(&'a ScriptedDriver);

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}

#[seaquel_runtime::async_trait]
impl Driver for ScriptedDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let page = self.page.lock().unwrap().clone();
        if let Some(page) = page {
            self.record("count", sql);
            if *self.hold.lock().unwrap() == Hold::Calls {
                futures::future::pending::<()>().await;
            }
            if sql != page.count_sql || serde_json::to_value(&params).unwrap() != page.params {
                return Err(self.problem(format!("unexpected count {sql:?}")));
            }
            if let Some(e) = answer_error(&page.count) {
                return Err(e);
            }
            return Ok(QueryResult {
                columns: vec!["total".into()],
                rows: vec![vec![Value::from_wire(page.count.clone()).unwrap()]],
            });
        }
        let answer = self.take("query", sql, &params)?;
        if *self.hold.lock().unwrap() == Hold::Calls {
            futures::future::pending::<()>().await;
        }
        if let Some(e) = answer_error(&answer) {
            return Err(e);
        }
        Ok(QueryResult {
            columns: names(&answer["columns"]),
            rows: values(&answer["rows"]),
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let answer = self.take("execute", sql, &params)?;
        if *self.hold.lock().unwrap() == Hold::Calls {
            let _dropped = Dropped(self, "execute");
            futures::future::pending::<()>().await;
        }
        if let Some(e) = answer_error(&answer) {
            return Err(e);
        }
        Ok(ExecuteResult {
            rows_affected: answer["rowsAffected"].as_u64().unwrap_or(0),
            last_insert_id: answer["lastInsertId"].as_i64(),
        })
    }

    /// Answers each statement from the script (as `tx`), stopping at the
    /// first error or `expect_rows` shortfall, like the real drivers.
    async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        self.record("begin", "");
        if *self.hold.lock().unwrap() == Hold::Calls {
            // Dropping the call mid-transaction is its rollback.
            let _dropped = Dropped(self, "tx");
            futures::future::pending::<()>().await;
        }
        let mut counts = Vec::new();
        for (index, s) in statements.iter().enumerate() {
            let answer = self
                .take("tx", &s.sql, &s.params)
                .map_err(|e| TransactionError::at(index, e))?;
            if let Some(e) = answer_error(&answer) {
                return Err(TransactionError::at(index, e));
            }
            let rows = answer["rowsAffected"].as_u64().unwrap_or(0);
            s.check_affected(index, rows)
                .map_err(|e| TransactionError::at(index, e))?;
            counts.push(rows);
        }
        Ok(counts)
    }

    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        let hold = *self.hold.lock().unwrap();
        let page = self.page.lock().unwrap().clone();
        Box::pin(async_stream::stream! {
            if hold == Hold::Endless {
                self.released.store(false, Ordering::SeqCst);
                let _checkout = Checkout(self);
                for i in 0..10_000i64 {
                    self.pulled.fetch_add(1, Ordering::SeqCst);
                    yield Ok(StreamBatch {
                        columns: (i == 0).then(|| vec!["n".into()]),
                        rows: vec![vec![Value::Int(i)]],
                        is_final: i == 9_999,
                        truncated: false,
                    });
                }
                return;
            }
            if hold == Hold::Calls {
                self.released.store(false, Ordering::SeqCst);
                let _checkout = Checkout(self);
                self.record("stream", &sql);
                cancel.cancelled().await;
                return;
            }
            let Some(page) = page else {
                yield Err(self.problem(format!("unexpected stream {sql:?}")));
                return;
            };
            self.record("stream", &sql);
            if sql != page.page_sql || serde_json::to_value(&params).unwrap() != page.params {
                yield Err(self.problem(format!("unexpected page {sql:?} {params:?}; expected {:?}", page.page_sql)));
                return;
            }
            if let Some(e) = page.page_error.as_ref().and_then(answer_error) {
                yield Err(e);
                return;
            }
            let end = (page.offset + page.limit).min(page.matching.len());
            let rows = page.matching.get(page.offset..end).map(<[_]>::to_vec).unwrap_or_default();
            yield Ok(StreamBatch {
                columns: Some(page.columns.clone()),
                rows,
                is_final: true,
                truncated: false,
            });
        })
    }

    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.metadata_reads
            .lock()
            .unwrap()
            .push((schema.to_string(), table.to_string()));
        let answer = self
            .metadata
            .lock()
            .unwrap()
            .get(&(schema.to_string(), table.to_string()))
            .cloned();
        match answer {
            Some(a) => match answer_error(&a) {
                Some(e) => Err(e),
                None => Ok((
                    serde_json::from_value(a["columns"].clone()).unwrap(),
                    vec![],
                )),
            },
            None => Ok((vec![], vec![])),
        }
    }

    async fn close(&self) -> Result<(), DbError> {
        loop {
            let released = self.release.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            released.await;
        }
        Ok(())
    }
}

struct MockEngine {
    real: Arc<dyn Engine>,
    driver: Arc<ScriptedDriver>,
}

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        self.real.id()
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.driver.clone())
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        self.real.dialect()
    }
}

fn real_engines() -> Vec<Arc<dyn Engine>> {
    vec![
        seaquel_engine_postgres::engine(),
        seaquel_engine_mysql::engine(),
        seaquel_engine_sqlite::engine(),
        seaquel_engine_mssql::engine(),
        // For its dialect only, which needs no helper.
        seaquel_engine_duckdb::remote_engine(seaquel_engine_duckdb::HelperLocator {
            dir: std::path::PathBuf::from("/nonexistent/bin/duckdb"),
            version: "0.0.0".into(),
        }),
    ]
}

fn real_engine(ty: &str) -> Arc<dyn Engine> {
    let id = if ty == "mariadb" { "mysql" } else { ty };
    real_engines().into_iter().find(|e| e.id() == id).unwrap()
}

#[derive(Default)]
struct SteppingClock(AtomicU64);

impl Executor for SteppingClock {
    fn spawn(&self, _future: BoxFuture<'static, ()>) {
        unimplemented!("nothing spawns")
    }

    fn sleep(&self, _duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(std::future::ready(()))
    }

    fn unix_time(&self) -> Duration {
        Duration::from_millis(1_790_000_000_123)
    }

    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.0.fetch_add(5, Ordering::SeqCst) + 5)
    }
}

// ── The environment ──

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    driver: Arc<ScriptedDriver>,
    connection_id: String,
    _dir: tempfile::TempDir,
}

fn form(ty: &str, dir: &Path) -> seaquel_core::ConnectionForm {
    let file = dir.join("db.file");
    let database = if ty == "sqlite" || ty == "duckdb" {
        file.to_str().unwrap().to_string()
    } else {
        "db".to_string()
    };
    serde_json::from_value(json!({
        "name": "Mock", "type": ty, "host": "localhost", "port": 0,
        "databaseName": database, "username": "u", "connectionString": "",
        "sshEnabled": false, "sshHost": "", "sshPort": 22, "sshUsername": "",
        "sshAuthMethod": "password", "sshKeyPath": "",
        "savePassword": false, "saveSshPassword": false, "saveSshKeyPassphrase": false,
    }))
    .unwrap()
}

async fn env_with(ty: &str, executor: bool, limits: EditLimits) -> Env {
    let driver = ScriptedDriver::new();
    let mut builder = Core::builder()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .edit_limits(limits);
    for real in real_engines() {
        builder = builder.engine(Arc::new(MockEngine {
            real,
            driver: driver.clone(),
        }));
    }
    if executor {
        builder = builder.executor(Arc::new(SteppingClock::default()));
    }
    let core = builder.build();
    let dir = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    save_connection(&ws).await;
    let req = ConnectRequest::form(form(ty, dir.path()))
        .with_secrets(SuppliedSecrets::db("pw"))
        .with_create_if_missing(true);
    let connection_id = ws.connect(&core, req).await.unwrap();
    Env {
        core,
        ws,
        driver,
        connection_id,
        _dir: dir,
    }
}

async fn env(ty: &str) -> Env {
    env_with(ty, true, EditLimits::default()).await
}

async fn save_connection(ws: &Workspace) {
    let project = serde_json::from_value(json!({
        "id": "p1", "name": "P", "customLabels": [],
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
    }))
    .unwrap();
    projects::save(ws.storage(), &project).await.unwrap();
    let row = serde_json::from_value(json!({
        "id": SAVED, "projectId": "p1", "name": "Saved", "type": "postgres",
        "host": "localhost", "port": 5432, "databaseName": "db", "username": "u",
        "labelIds": [], "savePassword": false,
    }))
    .unwrap();
    connections::save(ws.storage(), &row).await.unwrap();
}

fn history_ctx(connection_id: &str) -> Json {
    json!({"connectionId": connection_id, "connectionName": "Saved",
           "connectionLabels": [{"id": "l1", "name": "prod", "color": "red"}]})
}

fn canary_ctx(connection_id: &str) -> Json {
    json!({"connectionId": connection_id, "connectionName": "canary-name",
           "connectionLabels": [{"id": "l1", "name": "canary-label", "color": "red"}]})
}

async fn apply(e: &Env, changes: Json, confirmed: bool, history: Option<Json>) -> Json {
    let mut p =
        json!({"connectionId": e.connection_id, "changes": changes, "confirmed": confirmed});
    if let Some(h) = history {
        p["history"] = h;
    }
    let params: ApplyChangesParams = serde_json::from_value(p).unwrap();
    let out = timeout(LIMIT, e.ws.apply_changes(&e.core, params))
        .await
        .expect("the apply didn't end");
    match out {
        Ok(o) => serde_json::to_value(&o).unwrap(),
        Err(err) => json!({"err": err.code, "message": err.message}),
    }
}

async fn plan(e: &Env, edits: Json) -> Result<Json, (String, String)> {
    let params: PlanEditsParams =
        serde_json::from_value(json!({"connectionId": e.connection_id, "edits": edits})).unwrap();
    e.ws.plan_edits(&e.core, params)
        .await
        .map(|p| serde_json::to_value(&p).unwrap())
        .map_err(|e| (e.code, e.message))
}

async fn collect(stream: BoxStream<'_, RunEvent>) -> Vec<Json> {
    timeout(LIMIT, stream.collect::<Vec<_>>())
        .await
        .expect("the stream didn't end")
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}

fn page_params(e: &Env, query: Json, page: u32, page_size: u32) -> TablePageParams {
    serde_json::from_value(json!({
        "connectionId": e.connection_id, "streamId": "tp-1", "query": query,
        "page": page, "pageSize": page_size,
    }))
    .unwrap()
}

fn types(events: &[Json]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

fn paginate(ty: &str, sql: &str, limit: u64, offset: u64) -> String {
    real_engine(ty)
        .dialect()
        .unwrap()
        .paginate(sql, limit, offset)
}

fn sql_engine(ty: &str) -> seaquel_core::sql::SqlEngine {
    ty.parse().unwrap()
}

// ── Fixtures ──

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../seaquel-workspace/tests/fixtures/edits")
}

fn read(file: &str) -> Json {
    let path = fixtures().join(format!("{file}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn changes() -> serde_json::Map<String, Json> {
    match read("changes") {
        Json::Object(m) => m,
        other => panic!("{other}"),
    }
}

fn expected(case: &Json, changes: &serde_json::Map<String, Json>) -> Json {
    let mut case = case.clone();
    if let Some(entry) = changes.get(case["name"].as_str().unwrap()) {
        for (k, v) in entry["expected"].as_object().unwrap() {
            case[k] = v.clone();
        }
    }
    case
}

/// Replays every case of `files` against its expected fields (no
/// differences allowed) and its recorded ones; the cases that differ from
/// the recording must be in `changes.json`, and are returned.
async fn replay_all<F, Fut>(files: &[&str], replay: F) -> (usize, BTreeSet<String>)
where
    F: Fn(Json) -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    let changes = changes();
    let mut problems = Vec::new();
    let mut differ = BTreeSet::new();
    let mut n = 0;
    for file in files {
        for case in read(file).as_array().unwrap() {
            n += 1;
            let name = case["name"].as_str().unwrap().to_string();
            problems.extend(replay(expected(case, &changes)).await);
            if !replay(case.clone()).await.is_empty() {
                if !changes.contains_key(&name) {
                    problems.push(format!("{name} differs but isn't in changes.json"));
                }
                differ.insert(name);
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    (n, differ)
}

/// Whether a failure's text is compared: a database error's is (`CODE:
/// message`); the no-row-matched and refusal texts are the GUI's.
fn compares_text(code: &str) -> bool {
    !matches!(code, "NO_ROWS_AFFECTED" | "NOT_EDITABLE")
}

// ── The plan files, through Core ──

async fn replay_plan(case: Json) -> Vec<String> {
    let name = case["name"].as_str().unwrap().to_string();
    let ty = case["engine"].as_str().unwrap();
    let e = env(ty).await;
    let pending = case["input"]["pending"] == true;
    let mut out = Vec::new();
    let mut planned: Vec<(Json, Result<Json, String>)> = Vec::new();
    for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
        let edit = step["edit"].clone();
        if edit.is_null() {
            continue;
        }
        e.driver.set_metadata(&case["metadata"]);
        let driver = step["driver"].as_array().unwrap();
        for d in driver.iter().filter(|d| d["op"] == "tableMetadata") {
            e.driver.set_metadata_answer(
                d["schema"].as_str().unwrap(),
                d["table"].as_str().unwrap(),
                d["answer"].clone(),
            );
        }
        let build = driver.iter().find(|d| d["op"] == "build");
        let outcome = &step["outcome"];
        let failed = outcome["success"] == false || outcome["saved"] == false;
        if pending {
            e.driver.load(vec![]);
            let got = plan(&e, json!([edit])).await.map(|p| p[0].clone());
            match &got {
                Ok(p) => {
                    if failed {
                        out.push(format!("{name} step {i}: planned, expected {outcome}"));
                    }
                    if let Some(b) = build {
                        if p["sql"] != b["answer"]["sql"]
                            || p["params"] != b["answer"]["bindValues"]
                        {
                            out.push(format!(
                                "{name} step {i}: {} {} != {}",
                                p["sql"], p["params"], b["answer"]
                            ));
                        }
                    }
                }
                Err((code, _)) => {
                    if !(failed && outcome["code"] == code.as_str()) {
                        out.push(format!(
                            "{name} step {i}: refused {code}, expected {outcome}"
                        ));
                    }
                }
            }
            planned.push((edit, got.map_err(|(c, _)| c)));
        } else {
            let script = driver
                .iter()
                .filter(|d| d["op"] == "execute")
                .map(|d| {
                    expect(
                        "execute",
                        d["sql"].as_str().unwrap(),
                        d["params"].clone(),
                        d["answer"].clone(),
                    )
                })
                .collect();
            e.driver.load(script);
            let got = apply(
                &e,
                json!([{"type": "edit", "id": "s", "edit": edit}]),
                true,
                None,
            )
            .await;
            let f = &got["failed"];
            match (failed, f.is_null()) {
                (false, true) if got["applied"] == 1 => {}
                (true, false) => {
                    let code = f["code"].as_str().unwrap();
                    if outcome["code"] != code {
                        out.push(format!(
                            "{name} step {i}: failed {code}, expected {outcome}"
                        ));
                    } else if compares_text(code) && outcome.get("error").is_some() {
                        let text = format!("{code}: {}", f["message"].as_str().unwrap());
                        if outcome["error"] != text.as_str() {
                            out.push(format!("{name} step {i}: {text:?} != {}", outcome["error"]));
                        }
                    }
                }
                _ => out.push(format!("{name} step {i}: {got} != {outcome}")),
            }
            for p in e.driver.problems() {
                out.push(format!("{name} step {i}: {p}"));
            }
        }
    }
    for q in case["queue"].as_array().unwrap() {
        let id = q["id"].as_str().unwrap();
        match planned
            .iter()
            .rev()
            .find(|(edit, _)| *edit == q["change"]["edit"])
        {
            Some((_, Ok(p))) => {
                for field in ["sql", "params", "queryType", "dml", "summary"] {
                    if p[field] != q[field] {
                        out.push(format!("{name} {id}: {field} {} != {}", p[field], q[field]));
                    }
                }
            }
            _ => out.push(format!("{name} {id}: not planned")),
        }
    }
    out
}

#[tokio::test]
async fn replays_the_plan_fixtures() {
    let files = [
        "plan-postgres",
        "plan-mysql",
        "plan-mariadb",
        "plan-sqlite",
        "plan-mssql",
        "plan-duckdb",
    ];
    let (n, differ) = replay_all(&files, replay_plan).await;
    assert_eq!(n, 49);
    let want: BTreeSet<String> = [
        "duckdb/attached-catalog-queued",
        "mariadb/bigint-key-queued",
        "mariadb/query-tab-aliased-delete",
        "mssql/query-tab-delete-queued",
        "mssql/update-and-set-default-queued",
        "mysql/bytes-key",
        "mysql/insert-queued",
        "mysql/json-top-level-values",
        "mysql/json-typed-text",
        "mysql/update-queued",
        "pg/insert-metadata-fails",
        "pg/json-top-level-values",
        "pg/key-not-primary-key",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(differ, want);
}

// ── Apply ──

async fn replay_apply(case: Json) -> Vec<String> {
    let name = case["name"].as_str().unwrap().to_string();
    let ty = case["engine"].as_str().unwrap();
    let e = env(ty).await;
    e.driver.set_metadata(&case["metadata"]);
    let mode = case["mode"].as_str().unwrap();
    let call = if mode == "atomic" { "tx" } else { "execute" };
    let driver = case["driver"].as_array().unwrap();
    e.driver.load(
        driver
            .iter()
            .map(|d| {
                expect(
                    call,
                    d["sql"].as_str().unwrap(),
                    d["params"].clone(),
                    d["answer"].clone(),
                )
            })
            .collect(),
    );
    let changes: Vec<Json> = case["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["change"].clone())
        .collect();
    let confirmed = case["input"]["confirmed"] == true;
    let got = apply(&e, json!(changes), confirmed, Some(history_ctx(SAVED))).await;
    let want = &case["outcome"];
    let mut out: Vec<String> = e
        .driver
        .problems()
        .into_iter()
        .map(|p| format!("{name}: {p}"))
        .collect();
    let mut check = |what: &str, a: &Json, b: &Json| {
        if a != b {
            out.push(format!("{name}: {what} {a} != {b}"));
        }
    };
    if want["confirmRequired"] == true {
        check("outcome", &got["outcome"], &json!("confirmRequired"));
        check("destructive", &got["destructive"], &want["destructive"]);
        check(
            "destructiveTotal",
            &got["destructiveTotal"],
            &want["destructiveTotal"],
        );
    } else {
        check("outcome", &got["outcome"], &json!("applied"));
        check("mode", &got["mode"], &json!(mode));
        check("applied", &got["applied"], &want["executed"]);
        check("ddl", &got["ddl"], &want["hasDdl"]);
        let f = &got["failed"];
        check("failed", &json!(!f.is_null() as u8), &want["failed"]);
        if !f.is_null() {
            check("failedAt", &f["index"], &want["failedAt"]);
            check("failedChangeId", &f["id"], &want["failedChangeId"]);
            check("code", &f["code"], &want["code"]);
            let code = f["code"].as_str().unwrap_or("");
            if compares_text(code) && want.get("error").is_some() {
                let text = format!("{code}: {}", f["message"].as_str().unwrap_or(""));
                check("error", &json!(text), &want["error"]);
            }
        }
        // Per applied change for single and in order; none for atomic.
        let results = got["results"].as_array().cloned().unwrap_or_default();
        if mode == "atomic" {
            check("results", &json!(results.len()), &json!(0));
        } else {
            check("results", &json!(results.len()), &got["applied"]);
            for (r, d) in results.iter().zip(driver) {
                check(
                    "rowsAffected",
                    &r["rowsAffected"],
                    &d["answer"]["rowsAffected"],
                );
                let last = d["answer"]
                    .get("lastInsertId")
                    .cloned()
                    .unwrap_or(Json::Null);
                check(
                    "lastInsertId",
                    r.get("lastInsertId").unwrap_or(&Json::Null),
                    &last,
                );
            }
        }
    }
    let history: Vec<Json> = got["history"]
        .as_array()
        .map(|h| {
            h.iter()
                .map(|r| json!({"query": r["query"], "rowCount": r["rowCount"]}))
                .collect()
        })
        .unwrap_or_default();
    let want_history: Vec<Json> = case["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| json!({"query": r["query"], "rowCount": r["rowCount"]}))
        .collect();
    check("history", &json!(history), &json!(want_history));
    let stored = query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap();
    check(
        "stored history",
        &json!(stored.len()),
        &json!(want_history.len()),
    );
    out
}

#[tokio::test]
async fn replays_the_apply_fixtures() {
    let (n, differ) = replay_all(&["apply"], replay_apply).await;
    assert_eq!(n, 29);
    // Every apply case in changes.json differs, and nothing else: Decision
    // 5 (atomic), 4 (the key), 7 (confirmation) and 8 (history's rows
    // affected). No apply case is TypeScript-only.
    let listed: BTreeSet<String> = changes()
        .keys()
        .filter(|k| k.starts_with("apply/"))
        .cloned()
        .collect();
    assert_eq!(differ, listed);
}

// ── Table pages ──

const TABLE_PAGE_FILES: [&str; 6] = [
    "table-page-postgres",
    "table-page-mysql",
    "table-page-mariadb",
    "table-page-sqlite",
    "table-page-mssql",
    "table-page-duckdb",
];

async fn replay_table_page(case: Json) -> Vec<String> {
    let name = case["name"].as_str().unwrap().to_string();
    let ty = case["engine"].as_str().unwrap();
    let e = env(ty).await;
    let input = &case["input"];
    let target = &input["tableQuery"]["target"];
    // Core reads the metadata on SQL Server: the case's schema cache.
    if let Some(t) = input["schemaCache"].as_array().and_then(|c| {
        c.iter()
            .find(|t| t["schema"] == target["schema"] && t["name"] == target["table"])
    }) {
        e.driver.set_metadata_answer(
            target["schema"].as_str().unwrap(),
            target["table"].as_str().unwrap(),
            json!({"columns": t["columns"]}),
        );
    }
    let mut out = Vec::new();
    let select = &case["select"];
    let (page, page_size) = (
        input["page"].as_u64().unwrap() as u32,
        input["pageSize"].as_u64().unwrap() as u32,
    );
    if !select.is_null() {
        let sql = select["sql"].as_str().unwrap();
        let (limit, offset) = (
            case["paging"]["limit"].as_u64().unwrap(),
            case["paging"]["offset"].as_u64().unwrap(),
        );
        let count = match (input.get("countError"), input.get("countCell")) {
            (Some(err), _) => json!({"error": err}),
            (None, Some(cell)) => cell.clone(),
            (None, None) => json!(input["matching"].as_array().unwrap().len()),
        };
        *e.driver.page.lock().unwrap() = Some(PageScript {
            page_sql: paginate(ty, sql, limit + 1, offset),
            count_sql: seaquel_core::sql::scan::count_query(sql, sql_engine(ty)),
            params: select["params"].clone(),
            columns: names(&input["columns"]),
            matching: values(&input["matching"]),
            offset: offset as usize,
            limit: limit as usize + 1,
            count,
            page_error: input.get("pageError").map(|err| json!({"error": err})),
        });
    }
    let ev = collect(e.ws.table_page(
        &e.core,
        page_params(&e, input["tableQuery"].clone(), page, page_size),
    ))
    .await;
    for p in e.driver.problems() {
        out.push(format!("{name}: {p}"));
    }
    let counted = e.driver.calls().iter().any(|(c, _)| *c == "count");
    let should_count = !case["count"].is_null();
    if counted != should_count {
        out.push(format!(
            "{name}: counted {counted}, expected {}",
            case["count"]
        ));
    }
    let want = &case["result"];
    let error = ev
        .iter()
        .find(|e| e["type"] == "statementError" || e["type"] == "error");
    match (error, want["error"].as_str()) {
        (Some(err), Some(text)) => {
            if input.get("pageError").is_some() {
                let got = format!(
                    "{}: {}",
                    err["code"].as_str().unwrap(),
                    err["message"].as_str().unwrap()
                );
                if got != text {
                    out.push(format!("{name}: error {got:?} != {text:?}"));
                }
            }
        }
        (None, None) => {
            let batch = ev
                .iter()
                .find(|e| e["type"] == "batch")
                .cloned()
                .unwrap_or(json!({}));
            let done = ev
                .iter()
                .find(|e| e["type"] == "statementDone")
                .cloned()
                .unwrap_or(json!({}));
            let start = ev
                .iter()
                .find(|e| e["type"] == "statementStart")
                .cloned()
                .unwrap_or(json!({}));
            let rows = batch["rows"].as_array().cloned().unwrap_or_default();
            let got = json!({
                "columns": batch["columns"], "rows": rows, "rowCount": rows.len(),
                "totalRows": done["totalRows"], "page": start["page"], "pageSize": start["pageSize"],
                "totalPages": done["totalPages"],
            });
            // Cells as the wire carries them (`Value` has no u64: an
            // unsigned bigint past i64 is a decimal on its way through).
            let mut want = want.clone();
            want["rows"] = serde_json::to_value(values(&want["rows"])).unwrap();
            for field in [
                "columns",
                "rows",
                "rowCount",
                "totalRows",
                "page",
                "pageSize",
                "totalPages",
            ] {
                if got[field] != want[field] {
                    out.push(format!("{name}: {field} {} != {}", got[field], want[field]));
                }
            }
            if let Some(est) = want.get("countEstimated") {
                if done["countEstimated"] != *est {
                    out.push(format!(
                        "{name}: countEstimated {} != {est}",
                        done["countEstimated"]
                    ));
                }
            }
            if ev.last().map(|e| e["type"].clone()) != Some(json!("done")) {
                out.push(format!("{name}: {:?}", types(&ev)));
            }
        }
        (got, want) => out.push(format!("{name}: error {got:?} != {want:?}")),
    }
    out
}

#[tokio::test]
async fn replays_the_table_page_fixtures() {
    let (n, differ) = replay_all(&TABLE_PAGE_FILES, replay_table_page).await;
    assert_eq!(n, 40);
    let listed: BTreeSet<String> = changes()
        .keys()
        .filter(|k| k.starts_with("tp/"))
        .cloned()
        .collect();
    assert_eq!(differ, listed);
}

// ── Apply modes ──

fn users_meta() -> Json {
    json!([{"schema": "public", "table": "users", "columns": [
        {"name": "id", "type": "bigint", "castType": "bigint", "nullable": false, "isPrimaryKey": true, "isForeignKey": false},
        {"name": "name", "type": "text", "castType": "text", "nullable": true, "isPrimaryKey": false, "isForeignKey": false},
        {"name": "email", "type": "text", "castType": "text", "nullable": true, "isPrimaryKey": false, "isForeignKey": false},
    ]}, {"schema": "public", "table": "items", "columns": [
        {"name": "id", "type": "integer", "castType": "integer", "nullable": false, "isPrimaryKey": true, "isForeignKey": false},
    ]}])
}

fn update(id: &str, key: i64, value: &str) -> Json {
    json!({"type": "edit", "id": id, "edit": {"type": "updateCell",
           "target": {"schema": "public", "table": "users"},
           "key": [["id", key]], "column": "name", "value": value}})
}

const UPDATE_SQL: &str =
    "UPDATE \"public\".\"users\" SET \"name\" = CAST($1 AS text) WHERE \"id\" = CAST($2 AS bigint)";

fn typed(id: &str, sql: &str) -> Json {
    json!({"type": "sql", "id": id, "sql": sql, "params": []})
}

#[tokio::test]
async fn a_batch_of_one_runs_through_execute_and_returns_last_insert_id() {
    let e = env("mysql").await;
    e.driver.set_metadata(&json!([{"schema": "shop", "table": "items", "columns": [
        {"name": "id", "type": "int", "nullable": false, "isPrimaryKey": true, "isForeignKey": false},
        {"name": "name", "type": "text", "nullable": true, "isPrimaryKey": false, "isForeignKey": false}]}]));
    e.driver.load(vec![expect(
        "execute",
        "INSERT INTO `shop`.`items` (`name`) VALUES (?)",
        json!(["x"]),
        json!({"rowsAffected": 1, "lastInsertId": 42}),
    )]);
    let got = apply(
        &e,
        json!([{"type": "edit", "id": "c1", "edit": {"type": "insertRow",
                "target": {"schema": "shop", "table": "items"}, "values": [["name", "x"]]}}]),
        false,
        None,
    )
    .await;
    assert_eq!(got["mode"], "single", "{got}");
    assert_eq!(got["applied"], 1);
    assert_eq!(
        got["results"],
        json!([{"id": "c1", "rowsAffected": 1, "lastInsertId": 42}])
    );
    assert!(got.get("failed").is_none());
    assert_eq!(got["ddl"], false);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

#[tokio::test]
async fn a_dml_batch_is_one_transaction() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            "DELETE FROM t WHERE x = 1",
            json!([]),
            json!({"rowsAffected": 4}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            update("c1", 1, "a"),
            typed("c2", "DELETE FROM t WHERE x = 1")
        ]),
        false,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["mode"], "atomic", "{got}");
    assert_eq!(got["applied"], 2);
    assert_eq!(got["results"], json!([]));
    let calls: Vec<&str> = e.driver.calls().iter().map(|(c, _)| *c).collect();
    assert_eq!(calls, ["begin", "tx", "tx"]);
    let rows: Vec<&Json> = got["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| &h["rowCount"])
        .collect();
    assert_eq!(rows, [&json!(1), &json!(4)]);
}

#[tokio::test]
async fn its_failure_applies_nothing_and_names_the_change() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            UPDATE_SQL,
            json!(["b", 2]),
            json!({"error": {"code": "EXECUTE_ERROR", "message": "boom"}}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            update("c1", 1, "a"),
            update("c2", 2, "b"),
            update("c3", 3, "c")
        ]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["applied"], 0, "{got}");
    assert_eq!(
        got["failed"],
        json!({"id": "c2", "index": 1, "code": "EXECUTE_ERROR", "message": "boom"})
    );
    assert_eq!(got["history"], json!([]));
    // A stale key rolls back too, naming its change.
    e.driver.load(vec![expect(
        "tx",
        UPDATE_SQL,
        json!(["a", 1]),
        json!({"rowsAffected": 0}),
    )]);
    let got = apply(
        &e,
        json!([update("c1", 1, "a"), update("c2", 2, "b")]),
        true,
        None,
    )
    .await;
    assert_eq!(got["failed"]["code"], "NO_ROWS_AFFECTED");
    assert_eq!(got["failed"]["id"], "c1");
    assert!(!got["failed"]["message"]
        .as_str()
        .unwrap()
        .contains("canary"));
}

#[tokio::test]
async fn a_batch_with_ddl_applies_in_order_and_stops() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![
        expect(
            "execute",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "execute",
            "CREATE TABLE z (i int)",
            json!([]),
            json!({"rowsAffected": 0}),
        ),
        expect(
            "execute",
            UPDATE_SQL,
            json!(["b", 2]),
            json!({"rowsAffected": 0}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            update("c1", 1, "a"),
            typed("c2", "CREATE TABLE z (i int)"),
            update("c3", 2, "b"),
            update("c4", 3, "c")
        ]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["mode"], "inOrder", "{got}");
    assert_eq!(got["applied"], 2);
    assert_eq!(got["ddl"], true);
    assert_eq!(got["failed"]["id"], "c3");
    assert_eq!(got["failed"]["index"], 2);
    assert_eq!(got["failed"]["code"], "NO_ROWS_AFFECTED");
    assert_eq!(got["results"].as_array().unwrap().len(), 2);
    assert_eq!(got["history"].as_array().unwrap().len(), 2);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

/// Probe M1: the web server evicting the workspace (`close_all`) while an
/// apply runs drops the transaction, which rolls it back, instead of
/// letting it commit; an in-order apply stops at the statement in flight.
#[tokio::test]
async fn close_all_drops_an_apply_in_flight() {
    for (changes, mode, dropped) in [
        (
            json!([update("c1", 1, "a"), update("c2", 2, "b")]),
            "atomic",
            "tx",
        ),
        (
            json!([update("c1", 1, "a"), typed("c2", "CREATE TABLE z (i int)")]),
            "inOrder",
            "execute",
        ),
    ] {
        let e = env("postgres").await;
        e.driver.set_metadata(&users_meta());
        e.driver.load(vec![expect(
            "execute",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        )]);
        e.driver.set_hold(Hold::Calls);
        let params: ApplyChangesParams = serde_json::from_value(json!({
            "connectionId": e.connection_id, "changes": changes, "confirmed": true}))
        .unwrap();
        let applying = e.ws.apply_changes(&e.core, params);
        let closing = async {
            while !e
                .driver
                .calls()
                .iter()
                .any(|(c, _)| *c == "begin" || *c == "execute")
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            e.ws.close_all(&e.core).await;
        };
        let (got, ()) = timeout(LIMIT, futures::future::join(applying, closing))
            .await
            .expect("the apply didn't end when the workspace closed");
        let got = serde_json::to_value(got.unwrap()).unwrap();
        assert_eq!(got["mode"], mode, "{got}");
        assert_eq!(got["applied"], 0, "{got}");
        assert_eq!(got["failed"]["code"], "WORKSPACE_CLOSED", "{got}");
        assert!(
            e.driver.calls().contains(&("dropped", dropped.to_string())),
            "{:?}",
            e.driver.calls()
        );
    }
}

/// Probe M2: a NUL in any name is refused before any metadata read or
/// statement (Postgres refuses the byte with a confusing protocol error).
#[tokio::test]
async fn a_nul_in_a_name_is_refused_before_anything_is_read() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![]);
    let users = json!({"schema": "public", "table": "users"});
    let edits = [
        json!({"type": "updateCell", "target": {"schema": "public", "table": "us\u{0}ers"},
               "key": [["id", 1]], "column": "name", "value": "a"}),
        json!({"type": "updateCell", "target": {"schema": "pub\u{0}lic", "table": "users"},
               "key": [["id", 1]], "column": "name", "value": "a"}),
        json!({"type": "updateCell", "target": users, "key": [["id", 1]],
               "column": "na\u{0}me", "value": "a"}),
        json!({"type": "setDefault", "target": users, "key": [["i\u{0}d", 1]], "column": "name"}),
        json!({"type": "insertRow", "target": users, "values": [["na\u{0}me", "a"]]}),
        json!({"type": "deleteRow", "target": users, "key": [["\u{0}", 1]]}),
        json!({"type": "truncateTable", "target": {"schema": "public", "table": "\u{0}"}}),
        json!({"type": "dropObject", "target": {"schema": "\u{0}", "table": "users"}, "kind": "table"}),
    ];
    for edit in edits {
        let (code, message) = plan(&e, json!([edit])).await.unwrap_err();
        assert_eq!(code, "INVALID_ARGUMENT", "{edit}");
        assert!(!message.contains('\0'), "{message:?}");
        let got = apply(
            &e,
            json!([{"type": "edit", "id": "c1", "edit": edit}]),
            true,
            None,
        )
        .await;
        assert_eq!(got["err"], "INVALID_ARGUMENT", "{got}");
    }
    let mut queries = Vec::new();
    for (field, value) in [
        ("target", json!({"schema": "public", "table": "ite\u{0}ms"})),
        ("target", json!({"schema": "\u{0}", "table": "items"})),
        (
            "filters",
            json!([{"column": "i\u{0}d", "op": "=", "value": "1"}]),
        ),
        (
            "filters",
            json!([{"column": "\u{0}", "op": "IS NULL", "value": ""}]),
        ),
        ("sort", json!([{"column": "i\u{0}d", "direction": "ASC"}])),
    ] {
        let mut q = items_query();
        q[field] = value;
        queries.push(q);
    }
    for q in queries {
        let ev = collect(e.ws.table_page(&e.core, page_params(&e, q.clone(), 1, 10))).await;
        assert_eq!(types(&ev), ["error"], "{q} {ev:?}");
        assert_eq!(ev[0]["code"], "INVALID_ARGUMENT", "{q}");
    }
    // A NUL in a value is the database's business, not a name.
    assert!(e.driver.calls().is_empty(), "{:?}", e.driver.calls());
    assert!(e.driver.metadata_reads().is_empty());
}

#[tokio::test]
async fn validation_refuses_before_any_statement_runs() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![]);
    let bad_key = json!({"type": "edit", "id": "c3", "edit": {"type": "deleteRow",
        "target": {"schema": "public", "table": "users"}, "key": [["email", "canary-key"]]}});
    for (changes, index, code) in [
        (
            json!([
                update("c1", 1, "a"),
                update("c2", 2, "b"),
                bad_key,
                update("c4", 4, "d"),
                update("c5", 5, "e")
            ]),
            2,
            "NOT_EDITABLE",
        ),
        (
            json!([
                update("c1", 1, "a"),
                typed("c2", "UPDATE t SET a = 1; COMMIT"),
                update("c3", 3, "c")
            ]),
            1,
            "INVALID_ARGUMENT",
        ),
        (
            json!([update("c1", 1, "a"), {"type": "edit", "id": "c2", "edit": {"type": "deleteRow",
            "target": {"schema": "public", "table": "missing"}, "key": [["id", 1]]}}]),
            1,
            "NOT_EDITABLE",
        ),
    ] {
        let got = apply(&e, changes, true, Some(history_ctx(SAVED))).await;
        assert_eq!(got["applied"], 0, "{got}");
        assert_eq!(got["failed"]["index"], index, "{got}");
        assert_eq!(got["failed"]["code"], code, "{got}");
        assert!(!got.to_string().contains("canary"), "{got}");
    }
    // A metadata read that fails refuses with its code.
    e.driver.set_metadata_answer(
        "public",
        "users",
        json!({"error": {"code": "QUERY_ERROR", "message": "permission denied"}}),
    );
    let got = apply(
        &e,
        json!([typed("c1", "CREATE TABLE z (i int)"), update("c2", 1, "a")]),
        true,
        None,
    )
    .await;
    assert_eq!(got["failed"]["code"], "QUERY_ERROR", "{got}");
    assert_eq!(got["failed"]["index"], 1);
    assert!(e.driver.calls().is_empty(), "{:?}", e.driver.calls());
}

#[tokio::test]
async fn confirm_required_lists_destructive_statements() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![]);
    let mut changes: Vec<Json> = (0..105)
        .map(|i| typed(&format!("d{i}"), "DELETE FROM k"))
        .collect();
    changes.insert(0, update("c1", 1, "a"));
    changes.push(
        json!({"type": "edit", "id": "t", "edit": {"type": "truncateTable",
        "target": {"schema": "public", "table": "users"}}}),
    );
    let got = apply(&e, json!(changes), false, None).await;
    assert_eq!(got["outcome"], "confirmRequired", "{got}");
    assert_eq!(got["destructiveTotal"], 106);
    let listed = got["destructive"].as_array().unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(
        listed[0],
        json!({"index": 1, "sql": "DELETE FROM k", "reason": "delete_no_where"})
    );
    assert!(e.driver.calls().is_empty());
    // The sidebar's TRUNCATE on its own.
    let got = apply(
        &e,
        json!([{"type": "edit", "id": "t", "edit": {"type": "truncateTable",
        "target": {"schema": "public", "table": "users"}}}]),
        false,
        None,
    )
    .await;
    assert_eq!(
        got["destructive"],
        json!([{"index": 0, "sql": "TRUNCATE TABLE \"public\".\"users\"", "reason": "truncate"}])
    );
}

#[tokio::test]
async fn confirmed_applies_them() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![
        expect(
            "execute",
            "DROP TABLE \"public\".\"items\"",
            json!([]),
            json!({"rowsAffected": 0}),
        ),
        expect(
            "execute",
            "DELETE FROM k",
            json!([]),
            json!({"rowsAffected": 3}),
        ),
    ]);
    let got = apply(
        &e,
        json!([{"type": "edit", "id": "d", "edit": {"type": "dropObject",
            "target": {"schema": "public", "table": "items"}, "kind": "table"}}, typed("k", "DELETE FROM k")]),
        true,
        None,
    )
    .await;
    assert_eq!(got["mode"], "inOrder", "{got}");
    assert_eq!(got["applied"], 2);
    assert_eq!(got["ddl"], true);
    assert!(e.driver.problems().is_empty());
}

// ── History ──

#[tokio::test]
async fn history_is_appended_after_commit_and_only_for_what_ran() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    // Atomic, rolled back: nothing.
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            UPDATE_SQL,
            json!(["b", 2]),
            json!({"rowsAffected": 0}),
        ),
    ]);
    let got = apply(
        &e,
        json!([update("c1", 1, "a"), update("c2", 2, "b")]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["history"], json!([]));
    assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap()
        .is_empty());
    // Committed: all, with rows affected and Core's time.
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            "DELETE FROM t WHERE a = 1",
            json!([]),
            json!({"rowsAffected": 9}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            update("c1", 1, "a"),
            typed("c2", "DELETE FROM t WHERE a = 1")
        ]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    let h = got["history"].as_array().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0]["query"], UPDATE_SQL);
    assert_eq!(h[1]["rowCount"], 9);
    assert_eq!(h[0]["timestamp"], "2026-09-21T14:13:20.123Z");
    assert_eq!(h[0]["connectionId"], SAVED);
    assert!(h[0]["id"].as_str().unwrap().starts_with("hist-"));
    let stored = query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    // In order: the ones that ran.
    e.driver.load(vec![
        expect(
            "execute",
            "CREATE TABLE z (i int)",
            json!([]),
            json!({"rowsAffected": 0}),
        ),
        expect(
            "execute",
            "DROP TABLE q",
            json!([]),
            json!({"error": {"code": "EXECUTE_ERROR", "message": "no q"}}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            typed("a", "CREATE TABLE z (i int)"),
            typed("b", "DROP TABLE q"),
            typed("c", "CREATE TABLE y (i int)")
        ]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["history"].as_array().unwrap().len(), 1);
    assert_eq!(
        query_history::load_by_connection(e.ws.storage(), SAVED)
            .await
            .unwrap()
            .len(),
        3
    );
}

/// Cleanup pass B: each history row keeps the values its change was bound
/// with, in the cell wire format, so the history shows what ran and the
/// row can run again. A change without values stores none.
#[tokio::test]
async fn history_rows_keep_their_values() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    let big = json!({"$sq": "bigint", "v": "9007199254740993"});
    let bytes = json!({"$sq": "bytes", "v": "AAH/"});
    // Atomic: an edit, a typed change with values, one without.
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["Jonson", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            "UPDATE t SET a = $1, b = $2 WHERE c = $3",
            json!([big, bytes, null]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            "DELETE FROM t WHERE a = 1",
            json!([]),
            json!({"rowsAffected": 1}),
        ),
    ]);
    let got = apply(
        &e,
        json!([
            update("c1", 1, "Jonson"),
            {"type": "sql", "id": "c2", "sql": "UPDATE t SET a = $1, b = $2 WHERE c = $3",
             "params": [big, bytes, null]},
            typed("c3", "DELETE FROM t WHERE a = 1")
        ]),
        true,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["mode"], "atomic", "{got}");
    let h = got["history"].as_array().unwrap();
    assert_eq!(h.len(), 3, "{got}");
    assert_eq!(h[0]["params"], json!(["Jonson", 1]));
    assert_eq!(h[1]["params"], json!([big, bytes, null]));
    assert!(h[2].get("params").is_none(), "{}", h[2]);
    // Stored the same: newest first, the last change on top.
    let stored = query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap();
    let stored: Vec<Json> = stored
        .iter()
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    assert_eq!(stored[2]["params"], json!(["Jonson", 1]));
    assert_eq!(stored[1]["params"], json!([big, bytes, null]));
    assert!(stored[0].get("params").is_none());

    // A single change (execute) keeps its values too.
    e.driver.load(vec![expect(
        "execute",
        UPDATE_SQL,
        json!(["Smith", 2]),
        json!({"rowsAffected": 1}),
    )]);
    let got = apply(
        &e,
        json!([update("c4", 2, "Smith")]),
        false,
        Some(history_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["mode"], "single", "{got}");
    assert_eq!(got["history"][0]["params"], json!(["Smith", 2]));
}

#[tokio::test]
async fn a_failed_append_does_not_fail_the_apply() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![expect(
        "execute",
        UPDATE_SQL,
        json!(["a", 1]),
        json!({"rowsAffected": 1}),
    )]);
    let got = apply(
        &e,
        json!([update("c1", 1, "a")]),
        false,
        Some(history_ctx("not-saved")),
    )
    .await;
    assert_eq!(got["applied"], 1, "{got}");
    assert_eq!(got["history"], json!([]));
}

#[tokio::test]
async fn no_context_no_history() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["a", 1]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            UPDATE_SQL,
            json!(["b", 2]),
            json!({"rowsAffected": 1}),
        ),
    ]);
    let got = apply(
        &e,
        json!([update("c1", 1, "a"), update("c2", 2, "b")]),
        false,
        None,
    )
    .await;
    assert_eq!(got["applied"], 2, "{got}");
    assert_eq!(got["history"], json!([]));
    assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn metadata_is_loaded_once_per_table_per_call() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![]);
    let edits: Vec<Json> = (0..5)
        .map(|i| update(&format!("c{i}"), i, "x")["edit"].clone())
        .chain([json!({"type": "deleteRow", "target": {"schema": "public", "table": "items"}, "key": [["id", 1]]}),
                json!({"type": "truncateTable", "target": {"schema": "public", "table": "other"}})])
        .collect();
    let planned = plan(&e, json!(edits)).await.unwrap();
    assert_eq!(planned.as_array().unwrap().len(), 7);
    let reads = e.driver.metadata_reads();
    assert_eq!(
        reads,
        [
            ("public".to_string(), "users".to_string()),
            ("public".into(), "items".into())
        ]
    );
    // A new call reads again (no cache in Core).
    plan(&e, json!([edits[0]])).await.unwrap();
    assert_eq!(e.driver.metadata_reads().len(), 3);
    assert!(e.driver.calls().is_empty());
}

// ── Table pages ──

fn items_query() -> Json {
    json!({"target": {"schema": "public", "table": "items"}, "filters": [], "logic": "AND", "sort": []})
}

fn page_script(ty: &str, matching: usize, page_size: u64, count: Json) -> PageScript {
    let select = "SELECT * FROM \"public\".\"items\"";
    PageScript {
        page_sql: paginate(ty, select, page_size + 1, 0),
        count_sql: seaquel_core::sql::scan::count_query(select, sql_engine(ty)),
        params: json!([]),
        columns: vec!["id".into()],
        matching: (0..matching as i64).map(|i| vec![Value::Int(i)]).collect(),
        offset: 0,
        limit: page_size as usize + 1,
        count,
        page_error: None,
    }
}

#[tokio::test]
async fn table_page_counts_only_a_full_page() {
    let e = env("postgres").await;
    *e.driver.page.lock().unwrap() = Some(page_script("postgres", 3, 5, json!(3)));
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 5))).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
    assert!(!e.driver.calls().iter().any(|(c, _)| *c == "count"));
    assert_eq!(ev[2]["totalRows"], 3);
    assert_eq!(ev[0]["kind"], "page");
    assert_eq!(ev[0]["queryType"], "select");

    let e = env("postgres").await;
    *e.driver.page.lock().unwrap() = Some(page_script("postgres", 12, 5, json!(12)));
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 5))).await;
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 5);
    assert_eq!(ev[2]["totalRows"], 12);
    assert_eq!(ev[2]["totalPages"], 3);
    assert!(e.driver.calls().iter().any(|(c, _)| *c == "count"));
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

#[tokio::test]
async fn a_failed_count_is_estimated() {
    let e = env("postgres").await;
    *e.driver.page.lock().unwrap() = Some(page_script(
        "postgres",
        12,
        5,
        json!({"error": {"code": "QUERY_ERROR", "message": "timeout"}}),
    ));
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 5))).await;
    assert_eq!(ev[2]["countEstimated"], true, "{ev:?}");
    assert_eq!(ev[2]["totalRows"], 6);
}

/// Probe M3: a table page past the end runs the count instead of taking
/// the offset as the total.
#[tokio::test]
async fn table_page_past_the_end_counts() {
    let e = env("postgres").await;
    let mut script = page_script("postgres", 5, 5, json!(5));
    let select = "SELECT * FROM \"public\".\"items\"";
    script.page_sql = paginate("postgres", select, 6, 10);
    script.offset = 10;
    *e.driver.page.lock().unwrap() = Some(script);
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 3, 5))).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"],
        "{ev:?}"
    );
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 0);
    assert_eq!(ev[2]["totalRows"], 5);
    assert_eq!(ev[2]["totalPages"], 1);
    assert_eq!(ev[2]["countEstimated"], false);
    assert!(e.driver.calls().iter().any(|(c, _)| *c == "count"));
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

#[tokio::test]
async fn table_page_stops_after_the_row_past_the_page() {
    let e = env("postgres").await;
    e.driver.set_hold(Hold::Endless);
    *e.driver.page.lock().unwrap() = Some(page_script("postgres", 0, 3, json!(10_000)));
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 3))).await;
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 3, "{ev:?}");
    assert_eq!(e.driver.pulled.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn table_page_is_cancelled_by_db_cancel_and_by_disconnect() {
    for how in ["cancel", "disconnect", "close_all"] {
        let e = env("postgres").await;
        e.driver.set_hold(Hold::Calls);
        *e.driver.page.lock().unwrap() = Some(page_script("postgres", 0, 3, json!(0)));
        let mut stream =
            e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 3));
        let first = timeout(LIMIT, stream.next()).await.unwrap().unwrap();
        assert!(matches!(first, RunEvent::StatementStart { .. }));
        // The page's statement is in flight, holding a pooled connection.
        let (rest, ()) = tokio::join!(collect(stream), async {
            tokio::task::yield_now().await;
            match how {
                "cancel" => e.ws.cancel(&e.core, "tp-1"),
                "disconnect" => e.ws.disconnect(&e.core, &e.connection_id).await.unwrap(),
                _ => e.ws.close_all(&e.core).await,
            }
        });
        match how {
            "cancel" => assert!(rest.is_empty(), "{how}: {rest:?}"),
            "disconnect" => {
                assert_eq!(types(&rest), ["error"], "{how}: {rest:?}");
                assert_eq!(rest[0]["code"], "CONNECTION_CLOSED");
            }
            // Eviction cancels the workspace's streams and closes its
            // connections: either may reach the page first, as for a run.
            _ => assert!(
                rest.is_empty()
                    || (types(&rest) == ["error"] && rest[0]["code"] == "CONNECTION_CLOSED"),
                "{how}: {rest:?}"
            ),
        }
        assert!(
            e.driver.released.load(Ordering::SeqCst),
            "{how}: the page wasn't dropped"
        );
        assert_eq!(e.ws.stream_count(&e.core), 0);
    }
}

#[tokio::test]
async fn table_page_refuses_bad_pages_and_limits() {
    let e = env_with(
        "postgres",
        true,
        EditLimits {
            max_filters: Some(1),
            ..EditLimits::default()
        },
    )
    .await;
    for (page, size) in [(0, 10), (1, 0), (1, 100_000)] {
        let ev =
            collect(e.ws.table_page(&e.core, page_params(&e, items_query(), page, size))).await;
        assert_eq!(types(&ev), ["error"], "{page} {size}");
        assert_eq!(ev[0]["code"], "INVALID_ARGUMENT");
    }
    let mut q = items_query();
    q["filters"] =
        json!([{"column": "a", "op": "=", "value": "1"}, {"column": "b", "op": "=", "value": "2"}]);
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, q, 1, 10))).await;
    assert_eq!(ev[0]["code"], "INVALID_ARGUMENT");
    let mut q = items_query();
    q["filters"] = json!([{"column": "a", "op": "IN", "value": " , "}]);
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, q, 1, 10))).await;
    assert_eq!(ev[0]["code"], "INVALID_ARGUMENT");
    assert!(e.driver.calls().is_empty());
}

// ── Ownership, limits, executor ──

#[tokio::test]
async fn another_workspace_cannot_plan_apply_or_page() {
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    let dir = tempfile::tempdir().unwrap();
    let other = e
        .core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let params: PlanEditsParams = serde_json::from_value(json!({
        "connectionId": e.connection_id, "edits": [update("c", 1, "a")["edit"]]}))
    .unwrap();
    assert_eq!(
        other.plan_edits(&e.core, params).await.unwrap_err().code,
        "CONNECTION_NOT_FOUND"
    );
    let params: ApplyChangesParams = serde_json::from_value(json!({
        "connectionId": e.connection_id, "changes": [update("c", 1, "a")], "confirmed": true}))
    .unwrap();
    assert_eq!(
        other.apply_changes(&e.core, params).await.unwrap_err().code,
        "CONNECTION_NOT_FOUND"
    );
    let ev = collect(other.table_page(&e.core, page_params(&e, items_query(), 1, 10))).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], "CONNECTION_NOT_FOUND");
    let err = other
        .duckdb_extension(&e.core, &e.connection_id, ExtensionAction::List)
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    assert!(e.driver.calls().is_empty());
    assert!(e.driver.metadata_reads().is_empty());

    // Its cancel of the same stream id doesn't reach this workspace's page.
    e.driver.set_hold(Hold::Calls);
    *e.driver.page.lock().unwrap() = Some(page_script("postgres", 0, 3, json!(0)));
    let mut stream =
        e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 3));
    assert!(matches!(
        stream.next().await,
        Some(RunEvent::StatementStart { .. })
    ));
    other.cancel(&e.core, "tp-1");
    assert!(timeout(Duration::from_millis(100), stream.next())
        .await
        .is_err());
    e.ws.cancel(&e.core, "tp-1");
    assert!(timeout(LIMIT, stream.next()).await.unwrap().is_none());
}

#[tokio::test]
async fn edit_limits_are_checked_before_anything_is_read() {
    let limits = EditLimits {
        max_changes: Some(2),
        max_sql_bytes: Some(20),
        max_value_bytes: Some(8),
        ..EditLimits::default()
    };
    let e = env_with("postgres", true, limits).await;
    e.driver.set_metadata(&users_meta());
    e.driver.load(vec![]);
    for changes in [
        json!([
            typed("a", "DELETE FROM t"),
            typed("b", "DELETE FROM t"),
            typed("c", "DELETE FROM t")
        ]),
        json!([typed("a", "DELETE FROM t WHERE a = 1 AND b = 2")]),
        json!([update("a", 1, "a long value")]),
    ] {
        let got = apply(&e, changes, true, None).await;
        assert_eq!(got["err"], "INVALID_ARGUMENT", "{got}");
    }
    let err = plan(
        &e,
        json!([
            update("a", 1, "x")["edit"],
            update("b", 1, "y")["edit"],
            update("c", 1, "z")["edit"]
        ]),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, "INVALID_ARGUMENT");
    assert!(e.driver.calls().is_empty());
    assert!(e.driver.metadata_reads().is_empty());
    // Two tables past a cap of one: refused before either is read.
    let e = env_with(
        "postgres",
        true,
        EditLimits {
            max_tables: Some(1),
            ..EditLimits::default()
        },
    )
    .await;
    e.driver.set_metadata(&users_meta());
    let items = json!({"type": "edit", "id": "i", "edit": {"type": "deleteRow",
        "target": {"schema": "public", "table": "items"}, "key": [["id", 1]]}});
    let got = apply(&e, json!([update("a", 1, "x"), items.clone()]), true, None).await;
    assert_eq!(got["err"], "INVALID_ARGUMENT", "{got}");
    let err = plan(&e, json!([update("a", 1, "x")["edit"], items["edit"]]))
        .await
        .unwrap_err();
    assert_eq!(err.0, "INVALID_ARGUMENT");
    assert!(e.driver.metadata_reads().is_empty());
    // One table many times is fine.
    assert!(plan(
        &e,
        json!([update("a", 1, "x")["edit"], update("b", 2, "y")["edit"]])
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn without_an_executor_apply_and_table_page_are_not_supported() {
    let e = env_with("postgres", false, EditLimits::default()).await;
    let got = apply(
        &e,
        json!([typed("a", "DELETE FROM t WHERE a = 1")]),
        true,
        None,
    )
    .await;
    assert_eq!(got["err"], "NOT_SUPPORTED");
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, items_query(), 1, 10))).await;
    assert_eq!(ev[0]["code"], "NOT_SUPPORTED");
    // Planning needs no clock.
    e.driver.set_metadata(&users_meta());
    assert!(plan(&e, json!([update("a", 1, "x")["edit"]])).await.is_ok());
}

#[tokio::test]
async fn duckdb_extension_refuses_a_bad_name_and_other_engines() {
    let e = env("duckdb").await;
    e.driver.load(vec![
        expect(
            "query",
            "INSTALL 'h3' FROM community",
            json!([]),
            json!({"columns": ["Success"], "rows": []}),
        ),
        expect(
            "query",
            "LOAD 'h3'",
            json!([]),
            json!({"columns": ["Success"], "rows": []}),
        ),
        expect(
            "query",
            "SELECT * FROM duckdb_extensions()",
            json!([]),
            json!({"columns": ["extension_name"], "rows": [["json"]]}),
        ),
    ]);
    let got =
        e.ws.duckdb_extension(
            &e.core,
            &e.connection_id,
            ExtensionAction::InstallCommunity { name: "h3".into() },
        )
        .await
        .unwrap();
    assert!(got.is_none());
    let list =
        e.ws.duckdb_extension(&e.core, &e.connection_id, ExtensionAction::List)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(list.rows, vec![vec![Value::Text("json".into())]]);
    for bad in ["h3'; DROP", "", "a b"] {
        let err =
            e.ws.duckdb_extension(
                &e.core,
                &e.connection_id,
                ExtensionAction::Load { name: bad.into() },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
    }
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
    let pg = env("postgres").await;
    let err = pg
        .ws
        .duckdb_extension(&pg.core, &pg.connection_id, ExtensionAction::List)
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
    assert!(pg.driver.calls().is_empty());
}

// ── Logs ──

/// Every log record, message and key-values (as `tests/run.rs` captures).
struct KvCapture(Mutex<Vec<String>>);

impl log::Log for KvCapture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        struct Kvs(String);
        impl<'kvs> log::kv::VisitSource<'kvs> for Kvs {
            fn visit_pair(
                &mut self,
                key: log::kv::Key<'kvs>,
                value: log::kv::Value<'kvs>,
            ) -> Result<(), log::kv::Error> {
                self.0.push_str(&format!(" {key}={value}"));
                Ok(())
            }
        }
        let mut kvs = Kvs(String::new());
        let _ = record.key_values().visit(&mut kvs);
        self.0.lock().unwrap().push(format!(
            "{} {}: {}{}",
            record.level(),
            record.target(),
            record.args(),
            kvs.0
        ));
    }

    fn flush(&self) {}
}

fn capture_logs() -> &'static KvCapture {
    static CAPTURE: std::sync::OnceLock<&'static KvCapture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture: &'static KvCapture = Box::leak(Box::new(KvCapture(Mutex::default())));
        log::set_logger(capture).expect("another logger is installed");
        log::set_max_level(log::LevelFilter::Trace);
        capture
    })
}

#[tokio::test]
async fn no_sql_keys_or_values_in_logs() {
    let capture = capture_logs();
    let e = env("postgres").await;
    e.driver.set_metadata(&users_meta());
    let canary_update = json!({"type": "edit", "id": "c1", "edit": {"type": "updateCell",
        "target": {"schema": "public", "table": "users"},
        "key": [["id", "canary-key"]], "column": "name", "value": "canary-value"}});
    let typed_canary = json!({"type": "sql", "id": "c2",
        "sql": "UPDATE t SET a = 'canary-sql' WHERE b = $1", "params": ["canary-param"]});
    // A plan, an atomic apply that fails with a message holding a value,
    // an in-order apply that succeeds into history, and a refusal.
    plan(&e, json!([canary_update["edit"]])).await.unwrap();
    e.driver.load(vec![
        expect(
            "tx",
            UPDATE_SQL,
            json!(["canary-value", "canary-key"]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "tx",
            "UPDATE t SET a = 'canary-sql' WHERE b = $1",
            json!(["canary-param"]),
            json!({"error": {"code": "EXECUTE_ERROR", "message": "canary-error"}}),
        ),
    ]);
    let got = apply(
        &e,
        json!([canary_update, typed_canary]),
        true,
        Some(canary_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["failed"]["code"], "EXECUTE_ERROR");
    e.driver.load(vec![
        expect(
            "execute",
            "CREATE TABLE canary_t (i int)",
            json!([]),
            json!({"rowsAffected": 0}),
        ),
        expect(
            "execute",
            UPDATE_SQL,
            json!(["canary-value", "canary-key"]),
            json!({"rowsAffected": 1}),
        ),
    ]);
    let got = apply(
        &e,
        json!([typed("c0", "CREATE TABLE canary_t (i int)"), canary_update]),
        true,
        Some(canary_ctx(SAVED)),
    )
    .await;
    assert_eq!(got["applied"], 2, "{got}");
    apply(
        &e,
        json!([typed("c", "DELETE FROM canary_t; DROP TABLE canary_t")]),
        true,
        None,
    )
    .await;
    // A table page with a filter value, and one whose count fails.
    let mut q = items_query();
    q["filters"] = json!([{"column": "id", "op": "=", "value": "canary-filter"},
                          {"column": "id", "op": "IN", "value": "canary-in,x"}]);
    let select = "SELECT * FROM \"public\".\"items\" WHERE CAST(\"id\" AS TEXT) = $1 AND CAST(\"id\" AS TEXT) IN ($2, $3)";
    *e.driver.page.lock().unwrap() = Some(PageScript {
        page_sql: paginate("postgres", select, 2, 0),
        count_sql: seaquel_core::sql::scan::count_query(select, sql_engine("postgres")),
        params: json!(["canary-filter", "canary-in", "x"]),
        columns: vec!["id".into()],
        matching: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        offset: 0,
        limit: 2,
        count: json!({"error": {"code": "QUERY_ERROR", "message": "canary-count"}}),
        page_error: None,
    });
    let ev = collect(e.ws.table_page(&e.core, page_params(&e, q, 1, 1))).await;
    assert_eq!(ev.last().unwrap()["type"], "done", "{ev:?}");
    let lines = capture.0.lock().unwrap().clone();
    for activity in [
        "activity=db.planEdits",
        "activity=db.applyChanges",
        "activity=db.tablePage",
    ] {
        assert!(
            lines.iter().any(|l| l.contains(activity)),
            "no {activity}: {lines:#?}"
        );
    }
    for line in lines {
        if line.starts_with("DEBUG sqlparser") || line.starts_with("TRACE sqlparser") {
            continue;
        }
        assert!(!line.contains("canary"), "{line}");
    }
}

#[test]
fn debug_shows_no_sql_keys_or_values() {
    let change: Change = serde_json::from_value(json!({"type": "sql", "id": "c",
        "sql": "DELETE FROM canary", "params": ["canary"]}))
    .unwrap();
    let edit: Edit = serde_json::from_value(json!({"type": "updateCell",
        "target": {"schema": "s", "table": "t"}, "key": [["id", "canary"]],
        "column": "c", "value": "canary"}))
    .unwrap();
    let insert: Edit = serde_json::from_value(json!({"type": "insertRow",
        "target": {"schema": "s", "table": "t"}, "values": [["a", "canary"]]}))
    .unwrap();
    let page: TablePageParams =
        serde_json::from_value(json!({"connectionId": "x", "streamId": "s",
        "query": {"target": {"schema": "s", "table": "t"},
                  "filters": [{"column": "a", "op": "LIKE", "value": "canary"}]},
        "page": 1, "pageSize": 10}))
        .unwrap();
    let apply: ApplyChangesParams = serde_json::from_value(json!({"connectionId": "x",
        "changes": [{"type": "edit", "id": "e", "edit": {"type": "deleteRow",
            "target": {"schema": "s", "table": "t"}, "key": [["id", "canary"]]}}],
        "history": canary_ctx("saved")}))
    .unwrap();
    let outcome = ApplyOutcome::Applied {
        mode: seaquel_core::domain::edits::ApplyMode::Single,
        applied: 0,
        results: vec![],
        failed: Some(seaquel_core::domain::edits::ApplyFailure {
            id: Some("c".into()),
            index: Some(0),
            code: "EXECUTE_ERROR".into(),
            message: "canary".into(),
        }),
        ddl: false,
        history: vec![serde_json::from_value(
            json!({"id": "h", "query": "UPDATE canary SET a = $1",
            "timestamp": "t", "executionTime": 1, "rowCount": 1, "connectionId": "c",
            "favorite": false, "connectionNameSnapshot": "canary", "params": ["canary"]}),
        )
        .unwrap()],
    };
    let debug = format!("{change:?} {edit:?} {insert:#?} {page:?} {apply:?} {outcome:?}");
    assert!(!debug.contains("canary"), "{debug}");
    assert!(debug.contains("updateCell"), "{debug}");
}
