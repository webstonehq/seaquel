//! `Workspace::run` and `Workspace::page` on a scripted mock driver: the run
//! fixtures (`crates/seaquel-workspace/tests/fixtures/run`, recorded from
//! today's TypeScript runner) replayed through Core, and the run's
//! lifecycle (cancel, disconnect, eviction, ownership, history, timing).

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::run::{
    PageParams, PageSource, ParamValue, RunEvent, RunParams, RunTarget, CONFIRM_REQUIRED,
};
use seaquel_core::storage::{connections, projects, query_history};
use seaquel_core::{
    ConnectRequest, Core, Executor, QueryOptions, RunLimits, StreamEvent, SuppliedSecrets,
    Workspace, WorkspaceSpec,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Dialect, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch, Value,
};
use seaquel_runtime::BoxFuture;
use serde_json::{json, Value as Json};
use tokio::sync::Notify;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);
const SAVED: &str = "saved-1";

// ── The scripted driver ──

/// One call the script expects, and its answer (the fixture's `answer`).
#[derive(Debug, Clone)]
struct Expect {
    call: &'static str,
    sql: String,
    params: Json,
    answer: Json,
}

/// How a stream call behaves beyond its script.
#[derive(Clone, Copy, PartialEq)]
enum Hold {
    /// Answer from the script.
    No,
    /// Yield one batch, then wait for the cancel token, holding a "pooled
    /// connection" until the stream is dropped (as the sqlx drivers do).
    Connection,
    /// `query` and `execute` take their call and never answer, like a slow
    /// statement; streams answer from the script.
    Calls,
    /// A stream yields up to 10,000 one-row batches, whatever the script
    /// says, counting them in `pulled`.
    Endless,
}

#[derive(Default)]
struct Script {
    expected: VecDeque<Expect>,
    problems: Vec<String>,
    calls: Vec<(&'static str, String)>,
}

struct ScriptedDriver {
    script: Mutex<Script>,
    hold: Mutex<Hold>,
    released: AtomicBool,
    release: Notify,
    pulled: AtomicU64,
}

impl ScriptedDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            hold: Mutex::new(Hold::No),
            released: AtomicBool::new(true),
            release: Notify::new(),
            pulled: AtomicU64::new(0),
        })
    }

    fn load(&self, expected: Vec<Expect>) {
        let mut s = self.script.lock().unwrap();
        *s = Script {
            expected: expected.into(),
            ..Script::default()
        };
    }

    fn set_hold(&self, hold: Hold) {
        *self.hold.lock().unwrap() = hold;
    }

    /// Unexpected calls, and answers left over.
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

    /// The next expected call's answer, or an error when this call isn't it.
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

fn names(columns: &Json) -> Option<Vec<String>> {
    columns
        .as_array()
        .map(|c| c.iter().map(|n| n.as_str().unwrap().to_string()).collect())
}

fn answer_error(answer: &Json) -> Option<DbError> {
    answer.get("error").map(|e| DbError {
        code: e["code"].as_str().unwrap().to_string(),
        message: e["message"].as_str().unwrap().to_string(),
    })
}

/// Marks the driver's connection as released when dropped.
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
        let answer = self.take("query", sql, &params)?;
        if *self.hold.lock().unwrap() == Hold::Calls {
            futures::future::pending::<()>().await;
        }
        if let Some(e) = answer_error(&answer) {
            return Err(e);
        }
        Ok(QueryResult {
            columns: names(&answer["columns"]).unwrap_or_default(),
            rows: values(&answer["rows"]),
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let answer = self.take("execute", sql, &params)?;
        if *self.hold.lock().unwrap() == Hold::Calls {
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

    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        let hold = *self.hold.lock().unwrap();
        let answer = self.take("stream", &sql, &params);
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
            if hold == Hold::Connection {
                self.released.store(false, Ordering::SeqCst);
                let _checkout = Checkout(self);
                yield Ok(StreamBatch {
                    columns: Some(vec!["n".into()]),
                    rows: vec![vec![Value::Int(1)]],
                    is_final: false,
                    truncated: false,
                });
                cancel.cancelled().await;
                return;
            }
            let answer = match answer {
                Ok(answer) => answer,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            let batches: Vec<Json> = match answer.get("batches") {
                Some(b) => b.as_array().unwrap().clone(),
                None if answer.get("columns").is_some() => vec![answer.clone()],
                None => vec![],
            };
            let n = batches.len();
            for (i, b) in batches.iter().enumerate() {
                yield Ok(StreamBatch {
                    columns: names(&b["columns"]),
                    rows: values(&b["rows"]),
                    is_final: i + 1 == n && answer.get("error").is_none(),
                    truncated: false,
                });
            }
            if let Some(e) = answer_error(&answer) {
                yield Err(e);
            }
        })
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

/// A mock engine under a real engine's id, with that engine's dialect, so
/// pages are the real pagination.
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
        seaquel_engine_duckdb::engine(),
    ]
}

fn real_engine(ty: &str) -> Arc<dyn Engine> {
    let id = if ty == "mariadb" { "mysql" } else { ty };
    real_engines().into_iter().find(|e| e.id() == id).unwrap()
}

/// A clock that moves 5 ms each time it's read.
#[derive(Default)]
struct SteppingClock(AtomicU64);

impl Executor for SteppingClock {
    fn spawn(&self, _future: BoxFuture<'static, ()>) {
        unimplemented!("a run spawns nothing")
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

async fn env_with(ty: &str, executor: Option<Arc<dyn Executor>>) -> Env {
    env_limited(ty, executor, RunLimits::default()).await
}

/// The web server's run limits (`seaquel_server::WEB_RUN_LIMITS`).
const WEB_RUN_LIMITS: RunLimits = RunLimits {
    max_text_bytes: Some(2 * 1024 * 1024),
    max_statements: Some(10_000),
    max_param_values: Some(1_000),
    max_param_bytes: Some(1024 * 1024),
};

async fn env_limited(ty: &str, executor: Option<Arc<dyn Executor>>, limits: RunLimits) -> Env {
    let driver = ScriptedDriver::new();
    let mut builder = Core::builder()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .run_limits(limits);
    for real in real_engines() {
        builder = builder.engine(Arc::new(MockEngine {
            real,
            driver: driver.clone(),
        }));
    }
    if let Some(executor) = executor {
        builder = builder.executor(executor);
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
    env_with(ty, Some(Arc::new(SteppingClock::default()))).await
}

/// The saved connection history rows belong to (the foreign key).
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

/// A history context whose name and labels hold a canary.
fn canary_ctx(connection_id: &str) -> Json {
    json!({"connectionId": connection_id, "connectionName": "canary-name",
           "connectionLabels": [{"id": "l1", "name": "canary-label", "color": "red"}]})
}

fn history_ctx(connection_id: &str) -> Json {
    json!({"connectionId": connection_id, "connectionName": "Saved",
           "connectionLabels": [{"id": "l1", "name": "prod", "color": "red"}]})
}

/// `db.run`'s params from JSON, with the env's connection and defaults.
fn run_params(e: &Env, fields: Json) -> RunParams {
    let mut p = json!({
        "connectionId": e.connection_id, "streamId": "run-1", "text": "",
        "target": {"type": "all"}, "pageSize": 100,
    });
    for (k, v) in fields.as_object().unwrap() {
        p[k] = v.clone();
    }
    serde_json::from_value(p).unwrap()
}

async fn collect(stream: BoxStream<'_, RunEvent>) -> Vec<Json> {
    timeout(LIMIT, stream.collect::<Vec<_>>())
        .await
        .expect("the run didn't end")
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}

async fn run(e: &Env, fields: Json) -> Vec<Json> {
    collect(e.ws.run(&e.core, run_params(e, fields))).await
}

async fn page(e: &Env, fields: Json) -> Vec<Json> {
    let mut p = json!({
        "connectionId": e.connection_id, "streamId": "page-1", "page": 1, "pageSize": 100,
    });
    for (k, v) in fields.as_object().unwrap() {
        p[k] = v.clone();
    }
    let params: PageParams = serde_json::from_value(p).unwrap();
    collect(e.ws.page(&e.core, params)).await
}

fn types(events: &[Json]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap()).collect()
}

fn expect(call: &'static str, sql: &str, params: Json, answer: Json) -> Expect {
    Expect {
        call,
        sql: sql.to_string(),
        params,
        answer,
    }
}

fn paginate(ty: &str, sql: &str, limit: u64, offset: u64) -> String {
    real_engine(ty)
        .dialect()
        .unwrap()
        .paginate(sql, limit, offset)
}

fn last(events: &[Json]) -> &Json {
    events.last().expect("no events")
}

// ── The fixture replay ──

const FILES: [&str; 10] = [
    "plan-postgres",
    "plan-mysql",
    "plan-mariadb",
    "plan-sqlite",
    "plan-mssql",
    "plan-duckdb",
    "cursor",
    "execute",
    "history",
    "pending",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../seaquel-workspace/tests/fixtures/run")
}

fn read(file: &str) -> Json {
    let path = fixtures().join(format!("{file}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The fixture's `driver` as the calls Core makes: a page is `query_stream`
/// of the dialect's pagination (Decision 5).
fn script(ty: &str, driver: &Json) -> Vec<Expect> {
    driver
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let sql = d["sql"].as_str().unwrap();
            let (call, sql) = match d["op"].as_str().unwrap() {
                "page" => {
                    let p = &d["paginate"];
                    let sql = paginate(
                        ty,
                        sql,
                        p["limit"].as_u64().unwrap(),
                        p["offset"].as_u64().unwrap(),
                    );
                    ("stream", sql)
                }
                "stream" => ("stream", sql.to_string()),
                "count" | "utility" => ("query", sql.to_string()),
                "write" => ("execute", sql.to_string()),
                op => panic!("op {op}"),
            };
            expect(call, &sql, d["params"].clone(), d["answer"].clone())
        })
        .collect()
}

/// The GUI's `dedupeColumnNames`, display work the replay redoes to compare.
fn dedupe(columns: &[String]) -> Vec<String> {
    let originals: HashSet<&String> = columns.iter().collect();
    let mut used: HashSet<String> = HashSet::new();
    columns
        .iter()
        .map(|name| {
            if used.insert(name.clone()) {
                return name.clone();
            }
            let mut n = 2;
            loop {
                let candidate = format!("{name}_{n}");
                if !originals.contains(&candidate) && !used.contains(&candidate) {
                    used.insert(candidate.clone());
                    return candidate;
                }
                n += 1;
            }
        })
        .collect()
}

/// What the grid would hold after a run's events: one entry per statement
/// that produced a result, in the fixture's `results` shape.
#[derive(Default)]
struct View {
    results: Vec<Json>,
    deferred: Vec<Json>,
    history: Json,
    toasts: Vec<Json>,
    problems: Vec<String>,
}

fn new_result(start: &Json) -> Json {
    json!({
        "index": start["index"], "sql": start["sql"], "source": start["source"],
        "kind": start["kind"], "queryType": start["queryType"],
        "columns": [], "rows": [], "rowCount": 0, "totalRows": 0,
        "page": start["page"], "pageSize": start["pageSize"], "totalPages": 1,
        "error": null, "affectedRows": null, "lastInsertId": null,
        "table": start.get("table").cloned().unwrap_or(Json::Null),
        "columnRefs": start.get("columnRefs").cloned().unwrap_or(Json::Null),
        "countEstimated": null, "_planned": false, "_batch": false,
    })
}

fn apply(view: &mut View, event: &Json) {
    let current = |view: &mut View| -> Option<usize> { view.results.len().checked_sub(1) };
    match event["type"].as_str().unwrap() {
        "statementStart" => view.results.push(new_result(event)),
        "batch" => {
            let Some(i) = current(view) else {
                view.problems.push("batch before any statementStart".into());
                return;
            };
            let r = &mut view.results[i];
            if let Some(cols) = event["columns"].as_array() {
                let cols: Vec<String> = cols
                    .iter()
                    .map(|c| c.as_str().unwrap().to_string())
                    .collect();
                r["columns"] = json!(dedupe(&cols));
            }
            for row in event["rows"].as_array().unwrap() {
                r["rows"].as_array_mut().unwrap().push(row.clone());
            }
            r["rowCount"] = json!(r["rows"].as_array().unwrap().len());
            r["_batch"] = json!(true);
        }
        "statementDone" => {
            let Some(i) = current(view) else {
                view.problems.push("statementDone without a start".into());
                return;
            };
            let r = &mut view.results[i];
            r["totalRows"] = event["totalRows"].clone();
            r["totalPages"] = event["totalPages"].clone();
            if r["kind"] == "page" {
                r["countEstimated"] = event["countEstimated"].clone();
            }
            if let Some(n) = event.get("rowsAffected") {
                r["affectedRows"] = n.clone();
            }
            if let Some(n) = event.get("lastInsertId") {
                r["lastInsertId"] = n.clone();
            }
        }
        "statementError" => {
            if let Some(sql) = event.get("sql") {
                // A planned failure: no statementStart.
                view.results.push(json!({
                    "index": event["index"], "sql": sql, "source": null, "kind": null,
                    "queryType": null, "error": event["message"], "_planned": true,
                }));
                return;
            }
            let Some(i) = current(view) else {
                view.problems.push("statementError without a start".into());
                return;
            };
            let r = &mut view.results[i];
            r["error"] = json!(format!(
                "{}: {}",
                event["code"].as_str().unwrap(),
                event["message"].as_str().unwrap()
            ));
            // A failed stream keeps the rows it got.
            r["totalRows"] = r["rowCount"].clone();
        }
        "statementDeferred" => view.deferred.push(json!({
            "index": event["index"], "sql": event["sql"], "source": event["source"],
            "queryType": event["queryType"],
        })),
        "done" => {
            if event["statements"] == 0 {
                view.toasts.push(json!({"kind": "info",
                    "message": "No executable statements found (only comments)"}));
            }
            view.history = match event.get("history") {
                Some(h) => json!({"query": h["query"], "rowCount": h["rowCount"]}),
                None => Json::Null,
            };
        }
        "error" => {
            if event["code"] == "INVALID_PARAMETERS" {
                view.toasts
                    .push(json!({"kind": "error", "message": event["message"]}));
            } else {
                view.problems.push(format!("run failed: {event}"));
            }
        }
        other => view.problems.push(format!("unknown event {other}")),
    }
}

/// A utility result the grid hides: no rows, no error.
fn is_utility(r: &Json) -> bool {
    r["kind"] == "utility" && r["error"].is_null() && r["_batch"] == false
}

fn view_of(events: &[Json], current_target: bool) -> View {
    let mut view = View::default();
    for event in events {
        apply(&mut view, event);
    }
    let all_utility = view.results.iter().all(is_utility);
    for r in &mut view.results {
        let hidden = is_utility(r) && !all_utility;
        r["shown"] = json!(!hidden);
    }
    let n = view.deferred.len();
    if n > 0 {
        let message = if current_target {
            "Statement added to pending changes".to_string()
        } else if n == 1 {
            "1 statement added to pending changes".to_string()
        } else {
            format!("{n} statements added to pending changes")
        };
        view.toasts
            .push(json!({"kind": "info", "message": message}));
    }
    view
}

fn num_eq(a: &Json, b: &Json) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// `core` against the fixture's result `fix`, under the README's Replay
/// rules.
fn compare_result(at: &str, core: &Json, fix: &Json, out: &mut Vec<String>) {
    let mut check = |field: &str, ok: bool| {
        if !ok {
            out.push(format!(
                "{at}.{field}: core {} vs fixture {}",
                core[field], fix[field]
            ));
        }
    };
    check("index", core["index"] == fix["index"]);
    check("sql", core["sql"] == fix["sql"]);
    check("shown", core["shown"] == fix["shown"]);
    check("kind", core["kind"] == fix["kind"]);
    if !fix["source"].is_null() {
        check("source", core["source"] == fix["source"]);
    }
    if !fix["queryType"].is_null() {
        check("queryType", core["queryType"] == fix["queryType"]);
    }
    check("error", core["error"] == fix["error"]);
    for field in ["table", "columnRefs", "countEstimated"] {
        if fix.get(field).is_some() {
            check(field, core[field] == fix[field]);
        }
    }
    let error = !fix["error"].is_null();
    let write = fix["kind"] == "write";
    if write {
        check("affectedRows", core["affectedRows"] == fix["affectedRows"]);
        check("lastInsertId", core["lastInsertId"] == fix["lastInsertId"]);
    }
    // A write's and a non-streamed error's grid rows are the view's.
    if !write && (!error || fix["kind"] == "stream") {
        for field in [
            "columns",
            "rows",
            "rowCount",
            "totalRows",
            "page",
            "pageSize",
            "totalPages",
        ] {
            check(field, num_eq(&core[field], &fix[field]));
        }
    }
}

/// Every difference between Core's run of `case` and the case.
async fn replay(case: &Json) -> Vec<String> {
    let ty = case["engine"].as_str().unwrap();
    let input = &case["input"];
    let e = env(ty).await;
    e.driver.load(script(ty, &case["driver"]));
    let mut fields = json!({
        "text": input["text"], "target": input["target"], "pageSize": input["pageSize"],
        "confirmed": input["confirmed"], "deferWrites": input["deferWrites"],
        "history": history_ctx(SAVED),
    });
    if !input["params"].is_null() {
        fields["params"] = input["params"].clone();
    }
    let events = run(&e, fields).await;
    let current_target = input["target"]["type"] == "current";
    let mut view = view_of(&events, current_target);
    let mut out = std::mem::take(&mut view.problems);
    out.extend(
        e.driver
            .problems()
            .into_iter()
            .map(|p| format!("driver: {p}")),
    );

    let fix_results = case["results"].as_array().unwrap();
    if view.results.len() != fix_results.len() {
        out.push(format!(
            "{} results vs {}: {:?}",
            view.results.len(),
            fix_results.len(),
            types(&events)
        ));
    }
    for (i, (core, fix)) in view.results.iter().zip(fix_results).enumerate() {
        compare_result(&format!("results[{i}]"), core, fix, &mut out);
    }
    if Json::Array(view.deferred.clone()) != case["deferred"] {
        out.push(format!(
            "deferred: {:?} vs {}",
            view.deferred, case["deferred"]
        ));
    }
    let history_ok = match (&view.history, &case["history"]) {
        (Json::Null, Json::Null) => true,
        (core, fix) if !core.is_null() && !fix.is_null() => {
            core["query"] == fix["query"] && num_eq(&core["rowCount"], &fix["rowCount"])
        }
        _ => false,
    };
    if !history_ok {
        out.push(format!("history: {} vs {}", view.history, case["history"]));
    }
    if Json::Array(view.toasts.clone()) != case["toasts"] {
        out.push(format!("toasts: {:?} vs {}", view.toasts, case["toasts"]));
    }

    // Paging afterwards, through db.page with the result's source.
    let mut results = view.results;
    for (n, p) in case
        .get("pages")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let action = &p["action"];
        let at = action["resultIndex"].as_u64().unwrap_or(0) as usize;
        let Some(before) = results.get(at).cloned() else {
            out.push(format!("pages[{n}]: no result {at}"));
            continue;
        };
        let (page_no, page_size) = match action["type"].as_str().unwrap() {
            "goToPage" => (action["page"].clone(), before["pageSize"].clone()),
            _ => (json!(1), action["pageSize"].clone()),
        };
        e.driver.load(script(ty, &p["driver"]));
        let events = page(
            &e,
            json!({"source": before["source"], "page": page_no, "pageSize": page_size}),
        )
        .await;
        let mut pv = view_of(&events, false);
        out.extend(pv.problems.drain(..).map(|x| format!("pages[{n}]: {x}")));
        out.extend(
            e.driver
                .problems()
                .into_iter()
                .map(|x| format!("pages[{n}] driver: {x}")),
        );
        if !pv.history.is_null() {
            out.push(format!("pages[{n}]: a page recorded history"));
        }
        match pv.results.pop() {
            Some(mut r) if pv.results.is_empty() => {
                r["index"] = before["index"].clone();
                r["sql"] = before["sql"].clone();
                r["shown"] = before["shown"].clone();
                compare_result(&format!("pages[{n}]"), &r, &p["result"], &mut out);
                results[at] = r;
            }
            _ => out.push(format!("pages[{n}]: not one result: {:?}", types(&events))),
        }
    }
    out
}

#[tokio::test]
async fn replays_the_execute_and_history_fixtures() {
    let changes = read("changes");
    let listed: HashSet<String> = changes.as_object().unwrap().keys().cloned().collect();
    let mut failures = Vec::new();
    let mut differ = HashSet::new();
    let mut count = 0;
    for file in FILES {
        for case in read(file).as_array().unwrap() {
            count += 1;
            let name = case["name"].as_str().unwrap().to_string();
            let mut changed = case.clone();
            if let Some(change) = changes.get(&name) {
                for (k, v) in change["expected"].as_object().unwrap() {
                    changed[k] = v.clone();
                }
            }
            let diffs = replay(&changed).await;
            if !diffs.is_empty() {
                failures.push(format!("{name}:\n  {}", diffs.join("\n  ")));
            }
            if !replay(case).await.is_empty() {
                differ.insert(name);
            }
        }
    }
    assert!(count >= 95, "{count} cases");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // Exactly the cases changes.json lists differ from the recording.
    let mut unlisted: Vec<_> = differ.difference(&listed).collect();
    let mut unchanged: Vec<_> = listed.difference(&differ).collect();
    unlisted.sort();
    unchanged.sort();
    assert!(
        unlisted.is_empty() && unchanged.is_empty(),
        "differ without an entry: {unlisted:?}; listed but the same: {unchanged:?}"
    );
}

// ── Paging and counting ──

#[tokio::test]
async fn counts_only_when_the_page_is_full() {
    let e = env("postgres").await;
    let rows = |n: i64| json!((1..=n).map(|i| json!([i])).collect::<Vec<_>>());
    // A partial page: no count, the total is what came back.
    e.driver.load(vec![expect(
        "stream",
        &paginate("postgres", "SELECT a FROM t", 4, 0),
        json!([]),
        json!({"columns": ["a"], "rows": rows(2)}),
    )]);
    let ev = run(&e, json!({"text": "SELECT a FROM t", "pageSize": 3})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
    assert_eq!(ev[2]["totalRows"], 2);
    assert_eq!(ev[2]["countEstimated"], false);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
    // A full page: the extra row is dropped and the count runs, with the
    // same binds.
    e.driver.load(vec![
        expect(
            "stream",
            &paginate("postgres", "SELECT a FROM t WHERE b = $1", 4, 0),
            json!([7]),
            json!({"columns": ["a"], "rows": rows(4)}),
        ),
        expect(
            "query",
            "SELECT COUNT(*) as total FROM (SELECT a FROM t WHERE b = $1) AS count_query",
            json!([7]),
            json!({"columns": ["total"], "rows": [[10]]}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SELECT a FROM t WHERE b = {{b}}", "pageSize": 3,
               "params": [{"name": "b", "value": 7}]}),
    )
    .await;
    assert_eq!(ev[1]["rows"], rows(3));
    assert_eq!(ev[1]["is_final"], true);
    assert_eq!(ev[2]["totalRows"], 10);
    assert_eq!(ev[2]["totalPages"], 4);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

#[tokio::test]
async fn a_failed_count_is_estimated_and_flagged() {
    let e = env("postgres").await;
    let count_sql = "SELECT COUNT(*) as total FROM (SELECT a FROM t) AS count_query";
    for count_answer in [
        json!({"error": {"code": "QUERY_ERROR", "message": "timeout"}}),
        json!({"columns": ["total"], "rows": [["n/a"]]}),
        json!({"columns": ["total"], "rows": [[-1]]}),
        json!({"columns": ["total"], "rows": [[1.5]]}),
        json!({"columns": ["total"], "rows": []}),
    ] {
        e.driver.load(vec![
            expect(
                "stream",
                &paginate("postgres", "SELECT a FROM t", 3, 2),
                json!([]),
                json!({"columns": ["a"], "rows": [[1], [2], [3]]}),
            ),
            expect("query", count_sql, json!([]), count_answer.clone()),
        ]);
        let ev = page(
            &e,
            json!({"source": {"sql": "SELECT a FROM t", "params": []}, "page": 2, "pageSize": 2}),
        )
        .await;
        assert_eq!(
            types(&ev),
            ["statementStart", "batch", "statementDone", "done"],
            "{count_answer}"
        );
        assert_eq!(ev[2]["totalRows"], 5, "{count_answer}");
        assert_eq!(ev[2]["totalPages"], 3);
        assert_eq!(ev[2]["countEstimated"], true);
        assert_eq!(last(&ev)["succeeded"], true);
    }
    // Text and big integers count.
    for (cell, total) in [
        (json!("7"), 7),
        (
            json!({"$sq": "bigint", "v": "9007199254740993"}),
            9_007_199_254_740_993u64,
        ),
    ] {
        e.driver.load(vec![
            expect(
                "stream",
                &paginate("postgres", "SELECT a FROM t", 3, 0),
                json!([]),
                json!({"columns": ["a"], "rows": [[1], [2], [3]]}),
            ),
            expect(
                "query",
                count_sql,
                json!([]),
                json!({"columns": ["total"], "rows": [[cell]]}),
            ),
        ]);
        let ev = run(&e, json!({"text": "SELECT a FROM t", "pageSize": 2})).await;
        assert_eq!(ev[2]["totalRows"], total);
        assert_eq!(ev[2]["countEstimated"], false);
    }
}

/// Probe M3: a page past the end comes back empty, and its offset says
/// nothing about the total, so the count runs (estimated when it fails).
#[tokio::test]
async fn an_empty_page_past_the_start_counts() {
    let e = env("postgres").await;
    let count_sql = "SELECT COUNT(*) as total FROM (SELECT a FROM t) AS count_query";
    e.driver.load(vec![
        expect(
            "stream",
            &paginate("postgres", "SELECT a FROM t", 3, 20),
            json!([]),
            json!({"columns": ["a"], "rows": []}),
        ),
        expect(
            "query",
            count_sql,
            json!([]),
            json!({"columns": ["total"], "rows": [[5]]}),
        ),
    ]);
    let ev = page(
        &e,
        json!({"source": {"sql": "SELECT a FROM t", "params": []}, "page": 11, "pageSize": 2}),
    )
    .await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
    assert_eq!(ev[2]["totalRows"], 5);
    assert_eq!(ev[2]["totalPages"], 3);
    assert_eq!(ev[2]["countEstimated"], false);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());

    // The count fails: the total is flagged as an estimate.
    e.driver.load(vec![
        expect(
            "stream",
            &paginate("postgres", "SELECT a FROM t", 3, 20),
            json!([]),
            json!({"columns": ["a"], "rows": []}),
        ),
        expect(
            "query",
            count_sql,
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "timeout"}}),
        ),
    ]);
    let ev = page(
        &e,
        json!({"source": {"sql": "SELECT a FROM t", "params": []}, "page": 11, "pageSize": 2}),
    )
    .await;
    assert_eq!(ev[2]["countEstimated"], true, "{ev:?}");
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());

    // The first page, empty: nothing to count.
    e.driver.load(vec![expect(
        "stream",
        &paginate("postgres", "SELECT a FROM t", 3, 0),
        json!([]),
        json!({"columns": ["a"], "rows": []}),
    )]);
    let ev = run(&e, json!({"text": "SELECT a FROM t", "pageSize": 2})).await;
    assert_eq!(ev[2]["totalRows"], 0);
    assert_eq!(ev[2]["countEstimated"], false);
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
}

#[tokio::test]
async fn page_refuses_a_non_select() {
    let e = env("postgres").await;
    for (source, page_no, size) in [
        ("DELETE FROM t", 1, 10),
        ("INSERT INTO t VALUES (1)", 1, 10),
        ("CREATE TABLE x (a int)", 1, 10),
        ("SELECT 1", 0, 10),
        ("SELECT 1", 1, u32::MAX),
        // A page fetches one row more, which must stay within
        // max_query_rows() (100,000).
        ("SELECT 1", 1, 100_000),
        // One statement only.
        ("SELECT 1; DELETE FROM t", 1, 10),
        ("SELECT 1;\nSELECT 2", 1, 10),
    ] {
        let ev = page(
            &e,
            json!({"source": {"sql": source, "params": []}, "page": page_no, "pageSize": size}),
        )
        .await;
        assert_eq!(types(&ev), ["error"], "{source}");
        assert_eq!(ev[0]["code"], "INVALID_ARGUMENT", "{source}");
    }
    assert!(e.driver.calls().is_empty());
}

/// Phase 5b probe (I1): a page's SQL is bounded like a run's text, before
/// anything scans or parses it.
#[tokio::test]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)] // Instant, in a native-only test
async fn page_refuses_text_past_the_run_limit() {
    let e = env_limited(
        "postgres",
        Some(Arc::new(SteppingClock::default())),
        WEB_RUN_LIMITS,
    )
    .await;
    let sql = format!("SELECT {}1 FROM t", "1,".repeat(4 * 1024 * 1024));
    let start = std::time::Instant::now();
    let ev = page(&e, json!({"source": {"sql": sql, "params": []}})).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], "INVALID_ARGUMENT");
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
    assert!(e.driver.calls().is_empty());
}

/// The run limits are per interface (owner, 2026-10-02): a Core built as
/// the desktop's (no limits) runs a 3 MiB, 20,000-statement script; one
/// built as the web server's refuses it before anything runs. Deferred
/// writes keep the driver out of it.
#[tokio::test]
async fn run_limits_are_the_interfaces() {
    let text = (0..20_000)
        .map(|i| {
            format!(
                "INSERT INTO t (a, b) VALUES ({i}, '{}');\n",
                "x".repeat(120)
            )
        })
        .collect::<String>();
    assert!(text.len() > 3 * 1024 * 1024, "{}", text.len());
    let params = json!({"text": text, "deferWrites": true});

    let desktop = env("postgres").await;
    let ev = run(&desktop, params.clone()).await;
    assert_eq!(ev.len(), 20_001);
    assert!(ev[..20_000]
        .iter()
        .all(|e| e["type"] == "statementDeferred"));
    assert_eq!(last(&ev)["type"], "done");
    assert_eq!(last(&ev)["statements"], 20_000);

    let web = env_limited(
        "postgres",
        Some(Arc::new(SteppingClock::default())),
        WEB_RUN_LIMITS,
    )
    .await;
    let ev = run(&web, params).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], "INVALID_ARGUMENT");
    assert!(ev[0]["message"].as_str().unwrap().contains("2 MiB"));
    // Under the text cap, the statement count is the web's limit.
    let text = "INSERT INTO t VALUES (1);".repeat(10_001);
    let ev = run(&web, json!({"text": text, "deferWrites": true})).await;
    assert_eq!(types(&ev), ["error"]);
    assert!(
        ev[0]["message"]
            .as_str()
            .unwrap()
            .contains("at most 10,000"),
        "{}",
        ev[0]["message"]
    );
    assert!(web.driver.calls().is_empty() && desktop.driver.calls().is_empty());
}

/// A page reads the row past the page and no more, whatever the SQL did
/// with the limit, and drops the driver's stream (which stops the rest on
/// the server).
#[tokio::test]
async fn a_page_reads_one_row_past_the_page_and_stops() {
    let e = env("postgres").await;
    e.driver.set_hold(Hold::Endless);
    let sql = "SELECT n FROM big -- note";
    e.driver.load(vec![
        expect(
            "stream",
            &paginate("postgres", sql, 101, 0),
            json!([]),
            json!({}),
        ),
        expect(
            "query",
            &format!("SELECT COUNT(*) as total FROM ({sql}\n) AS count_query"),
            json!([]),
            json!({"columns": ["total"], "rows": [[10000]]}),
        ),
    ]);
    let ev = run(&e, json!({"text": sql})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
    assert_eq!(ev[1]["rows"].as_array().unwrap().len(), 100);
    assert_eq!(ev[2]["totalRows"], 10_000);
    assert_eq!(e.driver.pulled.load(Ordering::SeqCst), 101);
    assert!(e.driver.released.load(Ordering::SeqCst));
    assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
    // The limit is on its own line, out of the comment's reach.
    assert!(paginate("postgres", sql, 101, 0).ends_with("-- note\nLIMIT 101 OFFSET 0"));
}

/// An executable comment is code to the split, so a page can't carry a
/// second statement inside one.
#[tokio::test]
async fn page_refuses_a_second_statement_in_an_executable_comment() {
    for (ty, sql) in [
        ("mysql", "SELECT 1 /*!; DELETE FROM t */"),
        ("mariadb", "SELECT 1 /*M!; DELETE FROM t */"),
    ] {
        let e = env(ty).await;
        let ev = page(&e, json!({"source": {"sql": sql, "params": []}})).await;
        assert_eq!(types(&ev), ["error"], "{ty}");
        assert_eq!(ev[0]["code"], "INVALID_ARGUMENT", "{ty}");
        assert!(e.driver.calls().is_empty());
    }
}

/// Cancel while a write, a utility statement or a page's count is in
/// flight: the run ends with nothing more, and the rest never runs.
#[tokio::test]
async fn cancel_during_a_write_a_utility_or_a_count() {
    let full_page = json!({"columns": ["a"], "rows": [[1], [2]]});
    for (text, script, calls) in [
        (
            "INSERT INTO t VALUES (1); SELECT 2",
            vec![expect(
                "execute",
                "INSERT INTO t VALUES (1)",
                json!([]),
                json!({}),
            )],
            1,
        ),
        (
            "CREATE TABLE u (a int); SELECT 2",
            vec![expect(
                "query",
                "CREATE TABLE u (a int)",
                json!([]),
                json!({}),
            )],
            1,
        ),
        (
            "SELECT a FROM t; SELECT 2",
            vec![
                expect(
                    "stream",
                    &paginate("postgres", "SELECT a FROM t", 2, 0),
                    json!([]),
                    full_page.clone(),
                ),
                expect(
                    "query",
                    "SELECT COUNT(*) as total FROM (SELECT a FROM t) AS count_query",
                    json!([]),
                    json!({}),
                ),
            ],
            2,
        ),
    ] {
        let e = env("postgres").await;
        e.driver.set_hold(Hold::Calls);
        e.driver.load(script);
        let stream = e.ws.run(
            &e.core,
            run_params(
                &e,
                json!({"text": text, "pageSize": 1, "history": history_ctx(SAVED)}),
            ),
        );
        let control = async {
            while e.driver.calls().len() < calls {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            e.ws.cancel(&e.core, "run-1");
        };
        let (events, ()) = timeout(LIMIT, async {
            tokio::join!(stream.collect::<Vec<_>>(), control)
        })
        .await
        .expect("the run didn't end");
        assert!(
            !events.iter().any(RunEvent::is_terminal),
            "{text}: {events:?}"
        );
        assert!(
            !events.iter().any(|ev| matches!(
                ev,
                RunEvent::StatementDone { .. } | RunEvent::StatementError { .. }
            )),
            "{text}: {events:?}"
        );
        assert_eq!(
            e.driver.calls().len(),
            calls,
            "{text}: later statements ran"
        );
        assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
        assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn page_streams_when_the_page_size_is_zero() {
    let e = env("postgres").await;
    e.driver.load(vec![expect(
        "stream",
        "SELECT a FROM t WHERE b = $1",
        json!([1]),
        json!({"batches": [{"columns": ["a"], "rows": [[1], [2]]}, {"rows": [[3]]}]}),
    )]);
    let ev = page(
        &e,
        json!({"source": {"sql": "SELECT a FROM t WHERE b = $1", "params": [1]}, "page": 3, "pageSize": 0}),
    )
    .await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "batch", "statementDone", "done"]
    );
    assert_eq!(ev[0]["kind"], "stream");
    assert_eq!(ev[3]["totalRows"], 3);
    assert_eq!(ev[3]["totalPages"], 1);
    assert!(last(&ev).get("history").is_none());
    assert!(e.driver.problems().is_empty());
}

// ── Runs of several statements ──

#[tokio::test]
async fn continues_after_a_statement_error() {
    let e = env("sqlite").await;
    e.driver.load(vec![
        expect(
            "execute",
            "INSERT INTO t VALUES (1)",
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "UNIQUE"}}),
        ),
        expect(
            "query",
            "CREATE TABLE u (a int)",
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "exists"}}),
        ),
        expect(
            "execute",
            "UPDATE t SET a = 1 WHERE b",
            json!([]),
            json!({"rowsAffected": 2}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "INSERT INTO t VALUES (1); CREATE TABLE u (a int); UPDATE t SET a = 1 WHERE b",
               "history": history_ctx(SAVED)}),
    )
    .await;
    assert_eq!(
        types(&ev),
        [
            "statementStart",
            "statementError",
            "statementStart",
            "statementError",
            "statementStart",
            "statementDone",
            "done"
        ]
    );
    assert_eq!(ev[1]["code"], "QUERY_ERROR");
    assert_eq!(ev[5]["rowsAffected"], 2);
    assert_eq!(last(&ev)["statements"], 3);
    assert_eq!(last(&ev)["succeeded"], false);
    assert!(last(&ev).get("history").is_none());
}

/// Phase 5b probe (N3): the refusal lists the first 100 destructive
/// statements and says how many there are, so one frame can't carry
/// thousands of them.
#[tokio::test]
async fn confirm_required_lists_the_first_hundred() {
    let e = env("postgres").await;
    let text = (0..250)
        .map(|i| format!("DROP TABLE t{i};"))
        .collect::<String>();
    let ev = run(&e, json!({"text": text})).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], CONFIRM_REQUIRED);
    let listed = ev[0]["destructive"].as_array().unwrap();
    assert_eq!(listed.len(), 100);
    assert_eq!(listed[0]["sql"], "DROP TABLE t0");
    assert_eq!(listed[99]["index"], 99);
    assert_eq!(ev[0]["destructiveTotal"], 250);
    assert!(ev[0]["message"].as_str().unwrap().starts_with("250 "));
    assert!(e.driver.calls().is_empty());
    // Under the cap the total is the list's length.
    let ev = run(&e, json!({"text": "DROP TABLE a; DROP TABLE b"})).await;
    assert_eq!(ev[0]["destructive"].as_array().unwrap().len(), 2);
    assert_eq!(ev[0]["destructiveTotal"], 2);
}

#[tokio::test]
async fn confirm_required_runs_nothing() {
    let e = env("postgres").await;
    let text = "SELECT 1;\nDROP TABLE t;\nDELETE FROM u";
    let ev = run(&e, json!({"text": text, "history": history_ctx(SAVED)})).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], CONFIRM_REQUIRED);
    assert_eq!(
        ev[0]["destructive"],
        json!([
            {"index": 1, "sql": "DROP TABLE t", "reason": "drop_table"},
            {"index": 2, "sql": "DELETE FROM u", "reason": "delete_no_where"},
        ])
    );
    assert!(e.driver.calls().is_empty());
    // Deferred statements are checked too, and at the cursor only its own.
    let ev = run(&e, json!({"text": text, "deferWrites": true})).await;
    assert_eq!(ev[0]["code"], CONFIRM_REQUIRED);
    let ev = run(
        &e,
        json!({"text": text, "target": {"type": "current", "cursor": 11}}),
    )
    .await;
    assert_eq!(
        ev[0]["destructive"],
        json!([{"index": 1, "sql": "DROP TABLE t", "reason": "drop_table"}])
    );
    assert!(e.driver.calls().is_empty());
    assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn confirmed_runs_the_destructive_statements() {
    let e = env("postgres").await;
    e.driver.load(vec![
        expect(
            "query",
            "DROP TABLE t",
            json!([]),
            json!({"columns": [], "rows": []}),
        ),
        expect(
            "execute",
            "DELETE FROM u",
            json!([]),
            json!({"rowsAffected": 3}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "DROP TABLE t; DELETE FROM u", "confirmed": true}),
    )
    .await;
    assert_eq!(
        types(&ev),
        [
            "statementStart",
            "statementDone",
            "statementStart",
            "statementDone",
            "done"
        ]
    );
    assert_eq!(last(&ev)["succeeded"], true);
    assert!(e.driver.problems().is_empty());
}

#[tokio::test]
async fn nothing_to_run_is_done_with_no_statements() {
    let e = env("postgres").await;
    for text in ["-- nothing\n/* ; */", "  ", ""] {
        for target in [
            json!({"type": "all"}),
            json!({"type": "current", "cursor": 1}),
        ] {
            let ev = run(
                &e,
                json!({"text": text, "target": target, "history": history_ctx(SAVED)}),
            )
            .await;
            assert_eq!(types(&ev), ["done"]);
            assert_eq!(ev[0]["statements"], 0);
            assert_eq!(ev[0]["succeeded"], false);
        }
    }
    assert!(e.driver.calls().is_empty());
}

// ── Cancel, disconnect, eviction, ownership ──

#[tokio::test]
async fn cancel_ends_the_run_and_drops_the_statement_in_flight() {
    let e = env("postgres").await;
    e.driver.set_hold(Hold::Connection);
    e.driver.load(vec![expect(
        "stream",
        "SELECT a FROM t LIMIT 5",
        json!([]),
        json!({}),
    )]);
    let mut stream = e.ws.run(
        &e.core,
        run_params(
            &e,
            json!({"text": "SELECT a FROM t LIMIT 5; SELECT 2; INSERT INTO t VALUES (1)",
                              "history": history_ctx(SAVED)}),
        ),
    );
    let first: Vec<Json> = vec![
        serde_json::to_value(stream.next().await.unwrap()).unwrap(),
        serde_json::to_value(stream.next().await.unwrap()).unwrap(),
    ];
    assert_eq!(types(&first), ["statementStart", "batch"]);
    assert!(!e.driver.released.load(Ordering::SeqCst));
    e.ws.cancel(&e.core, "run-1");
    let rest = timeout(LIMIT, stream.collect::<Vec<_>>())
        .await
        .expect("run didn't end");
    assert!(rest.is_empty(), "{rest:?}");
    assert!(e.driver.released.load(Ordering::SeqCst));
    assert_eq!(e.driver.calls().len(), 1, "later statements ran");
    assert_eq!(e.ws.stream_count(&e.core), 0);
    assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_cancel_before_the_start_runs_nothing() {
    let e = env("postgres").await;
    e.ws.cancel(&e.core, "run-1");
    let ev = run(&e, json!({"text": "SELECT 1"})).await;
    assert!(ev.is_empty(), "{ev:?}");
    assert!(e.driver.calls().is_empty());
}

#[tokio::test]
async fn disconnect_mid_run_ends_with_connection_closed() {
    let e = env("postgres").await;
    e.driver.set_hold(Hold::Connection);
    e.driver.load(vec![expect(
        "stream",
        "SELECT a FROM t LIMIT 5",
        json!([]),
        json!({}),
    )]);
    let mut stream = e.ws.run(
        &e.core,
        run_params(&e, json!({"text": "SELECT a FROM t LIMIT 5; SELECT 2"})),
    );
    assert!(matches!(
        stream.next().await,
        Some(RunEvent::StatementStart { .. })
    ));
    assert!(matches!(stream.next().await, Some(RunEvent::Batch(_))));
    let (rest, closed) = timeout(LIMIT, async {
        tokio::join!(
            stream.collect::<Vec<_>>(),
            e.ws.disconnect(&e.core, &e.connection_id)
        )
    })
    .await
    .expect("disconnect didn't finish");
    closed.unwrap();
    assert_eq!(rest.len(), 1, "{rest:?}");
    match &rest[0] {
        RunEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_CLOSED"),
        other => panic!("{other:?}"),
    }
    assert_eq!(e.driver.calls().len(), 1);
}

#[tokio::test]
async fn close_all_ends_a_run() {
    let e = env("postgres").await;
    e.driver.set_hold(Hold::Connection);
    e.driver.load(vec![expect(
        "stream",
        "SELECT a FROM t LIMIT 5",
        json!([]),
        json!({}),
    )]);
    let mut stream = e.ws.run(
        &e.core,
        run_params(&e, json!({"text": "SELECT a FROM t LIMIT 5; SELECT 2"})),
    );
    assert!(matches!(
        stream.next().await,
        Some(RunEvent::StatementStart { .. })
    ));
    assert!(matches!(stream.next().await, Some(RunEvent::Batch(_))));
    let (rest, ()) = timeout(LIMIT, async {
        tokio::join!(stream.collect::<Vec<_>>(), e.ws.close_all(&e.core))
    })
    .await
    .expect("close_all didn't finish");
    assert!(
        rest.iter()
            .all(|ev| matches!(ev, RunEvent::Error { code, .. } if code == "CONNECTION_CLOSED")),
        "{rest:?}"
    );
    assert!(!rest.iter().any(|ev| matches!(ev, RunEvent::Done { .. })));
    assert!(
        rest.iter().filter(|ev| ev.is_terminal()).count() <= 1,
        "{rest:?}"
    );
    assert_eq!(e.driver.calls().len(), 1);
    assert_eq!(e.core.connection_count(), 0);
}

#[tokio::test]
async fn another_workspace_cannot_run_page_or_cancel() {
    let e = env("postgres").await;
    let dir = tempfile::tempdir().unwrap();
    let other = e
        .core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let params = run_params(&e, json!({"text": "SELECT 1"}));
    let ev = collect(other.run(&e.core, params)).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], "CONNECTION_NOT_FOUND");
    let params: PageParams = serde_json::from_value(json!({
        "connectionId": e.connection_id, "streamId": "p", "page": 1, "pageSize": 10,
        "source": {"sql": "SELECT 1", "params": []},
    }))
    .unwrap();
    let ev = collect(other.page(&e.core, params)).await;
    assert_eq!(ev[0]["code"], "CONNECTION_NOT_FOUND");
    assert!(e.driver.calls().is_empty());

    // Its cancel of the same stream id doesn't reach this workspace's run.
    e.driver.set_hold(Hold::Connection);
    e.driver.load(vec![expect(
        "stream",
        "SELECT a FROM t LIMIT 5",
        json!([]),
        json!({}),
    )]);
    let mut stream = e.ws.run(
        &e.core,
        run_params(&e, json!({"text": "SELECT a FROM t LIMIT 5"})),
    );
    assert!(matches!(
        stream.next().await,
        Some(RunEvent::StatementStart { .. })
    ));
    assert!(matches!(stream.next().await, Some(RunEvent::Batch(_))));
    other.cancel(&e.core, "run-1");
    assert!(
        timeout(Duration::from_millis(100), stream.next())
            .await
            .is_err(),
        "another workspace's cancel ended the run"
    );
    e.ws.cancel(&e.core, "run-1");
    assert!(timeout(LIMIT, stream.next()).await.unwrap().is_none());
}

// ── History ──

#[tokio::test]
async fn history_is_appended_once_when_the_run_succeeds() {
    let e = env("postgres").await;
    e.driver.load(vec![
        expect(
            "stream",
            &paginate("postgres", "SELECT 1 AS a", 101, 0),
            json!([]),
            json!({"columns": ["a"], "rows": [[1]]}),
        ),
        expect(
            "execute",
            "INSERT INTO t VALUES ($1)",
            json!(["x"]),
            json!({"rowsAffected": 1}),
        ),
    ]);
    let text = "SELECT 1 AS a;\nINSERT INTO t VALUES ({{v}});";
    let ev = run(
        &e,
        json!({"text": text, "params": [{"name": "v", "value": "x"}], "history": history_ctx(SAVED)}),
    )
    .await;
    let done = last(&ev);
    assert_eq!(done["succeeded"], true);
    let h = &done["history"];
    assert_eq!(h["query"], text);
    assert_eq!(h["rowCount"], 1);
    assert_eq!(h["connectionId"], SAVED);
    assert_eq!(h["connectionNameSnapshot"], "Saved");
    assert_eq!(
        h["connectionLabelsSnapshot"],
        json!([{"id": "l1", "name": "prod", "color": "red"}])
    );
    assert_eq!(h["favorite"], false);
    assert_eq!(h["timestamp"], "2026-09-21T14:13:20.123Z");
    assert!(h["id"].as_str().unwrap().starts_with("hist-"));
    let stored = query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(serde_json::to_value(&stored[0]).unwrap(), *h);
}

#[tokio::test]
async fn history_is_skipped_on_error_cancel_page_and_without_context() {
    let e = env("postgres").await;
    let one = || {
        expect(
            "stream",
            &paginate("postgres", "SELECT 1 AS a", 101, 0),
            json!([]),
            json!({"columns": ["a"], "rows": [[1]]}),
        )
    };
    // A failed statement.
    e.driver.load(vec![
        one(),
        expect(
            "stream",
            &paginate("postgres", "SELECT nope", 101, 0),
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "nope"}}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SELECT 1 AS a; SELECT nope", "history": history_ctx(SAVED)}),
    )
    .await;
    assert!(last(&ev).get("history").is_none());
    // No context.
    e.driver.load(vec![one()]);
    let ev = run(&e, json!({"text": "SELECT 1 AS a"})).await;
    assert_eq!(last(&ev)["succeeded"], true);
    assert!(last(&ev).get("history").is_none());
    // A page.
    e.driver.load(vec![one()]);
    let ev = page(
        &e,
        json!({"source": {"sql": "SELECT 1 AS a", "params": []}}),
    )
    .await;
    assert!(last(&ev).get("history").is_none());
    // Only deferred statements: nothing ran.
    let ev = run(
        &e,
        json!({"text": "INSERT INTO t VALUES (1)", "deferWrites": true,
                            "history": history_ctx(SAVED)}),
    )
    .await;
    assert_eq!(types(&ev), ["statementDeferred", "done"]);
    assert_eq!(last(&ev)["succeeded"], false);
    assert!(last(&ev).get("history").is_none());
    // A cancelled run: cancel_ends_the_run_and_drops_the_statement_in_flight.
    assert!(query_history::load_by_connection(e.ws.storage(), SAVED)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_failed_append_does_not_fail_the_run() {
    let e = env("postgres").await;
    e.driver.load(vec![expect(
        "stream",
        &paginate("postgres", "SELECT 1 AS a", 101, 0),
        json!([]),
        json!({"columns": ["a"], "rows": [[1]]}),
    )]);
    let ev = run(
        &e,
        json!({"text": "SELECT 1 AS a", "history": history_ctx("not-saved")}),
    )
    .await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
    assert_eq!(last(&ev)["succeeded"], true);
    assert!(last(&ev).get("history").is_none());
}

#[tokio::test]
async fn history_row_uses_the_first_non_utility_statement() {
    let e = env("postgres").await;
    e.driver.load(vec![
        expect(
            "query",
            "SET x = 1",
            json!([]),
            json!({"columns": [], "rows": []}),
        ),
        expect(
            "execute",
            "UPDATE t SET a = 1 WHERE b",
            json!([]),
            json!({"rowsAffected": 4}),
        ),
        expect(
            "stream",
            &paginate("postgres", "SELECT 1 AS a", 101, 0),
            json!([]),
            json!({"columns": ["a"], "rows": [[1]]}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SET x = 1; UPDATE t SET a = 1 WHERE b; SELECT 1 AS a",
                            "history": history_ctx(SAVED)}),
    )
    .await;
    let h = &last(&ev)["history"];
    assert_eq!(h["rowCount"], 4);
    // 5 ms per clock reading: the UPDATE is readings 3 and 4.
    assert_eq!(h["executionTime"], 5);
    // Only utility statements: the first one.
    e.driver.load(vec![
        expect(
            "query",
            "SET x = 1",
            json!([]),
            json!({"columns": [], "rows": []}),
        ),
        expect(
            "query",
            "SET y = 2",
            json!([]),
            json!({"columns": [], "rows": []}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SET x = 1; SET y = 2", "history": history_ctx(SAVED)}),
    )
    .await;
    assert_eq!(last(&ev)["history"]["rowCount"], 0);
}

// ── Decision 18: `other` statements that return rows ──

#[tokio::test]
async fn a_row_returning_other_statement_shows_its_rows() {
    let e = env("postgres").await;
    e.driver.load(vec![
        expect(
            "query",
            "SET x = 1",
            json!([]),
            json!({"columns": [], "rows": []}),
        ),
        expect(
            "query",
            "WITH x AS (SELECT 1 AS a) SELECT a FROM x",
            json!([]),
            json!({"columns": ["a"], "rows": [[1], [2]]}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SET x = 1; WITH x AS (SELECT 1 AS a) SELECT a FROM x",
                            "history": history_ctx(SAVED)}),
    )
    .await;
    assert_eq!(
        types(&ev),
        [
            "statementStart",
            "statementDone",
            "statementStart",
            "batch",
            "statementDone",
            "done"
        ]
    );
    assert_eq!(ev[2]["kind"], "utility");
    assert_eq!(ev[2]["queryType"], "other");
    assert_eq!(ev[3]["columns"], json!(["a"]));
    assert_eq!(ev[3]["rows"], json!([[1], [2]]));
    assert_eq!(ev[3]["is_final"], true);
    assert_eq!(ev[4]["totalRows"], 2);
    // History counts it, not the SET before it.
    assert_eq!(last(&ev)["history"]["rowCount"], 2);
}

#[tokio::test]
async fn an_other_statement_with_no_columns_stays_a_utility() {
    let e = env("duckdb").await;
    e.driver.load(vec![expect(
        "query",
        "SET threads = 1",
        json!([]),
        json!({"columns": [], "rows": []}),
    )]);
    let ev = run(&e, json!({"text": "SET threads = 1"})).await;
    assert_eq!(types(&ev), ["statementStart", "statementDone", "done"]);
    assert_eq!(ev[1]["totalRows"], 0);
}

/// DuckDB answers `SET`, `CREATE`, … with one status column (`Success`,
/// or `Count` for DDL); that isn't a result to show.
#[tokio::test]
async fn a_duckdb_status_column_is_no_result() {
    let e = env("duckdb").await;
    e.driver.load(vec![
        expect(
            "query",
            "SET threads = 1",
            json!([]),
            json!({"columns": ["Success"], "rows": []}),
        ),
        expect(
            "query",
            "CREATE TABLE u AS SELECT 1",
            json!([]),
            json!({"columns": ["Count"], "rows": [[1]]}),
        ),
        expect(
            "query",
            "FROM range(1)",
            json!([]),
            json!({"columns": ["range"], "rows": [[0]]}),
        ),
    ]);
    let ev = run(
        &e,
        json!({"text": "SET threads = 1; CREATE TABLE u AS SELECT 1; FROM range(1)"}),
    )
    .await;
    assert_eq!(
        types(&ev),
        [
            "statementStart",
            "statementDone",
            "statementStart",
            "statementDone",
            "statementStart",
            "batch",
            "statementDone",
            "done"
        ]
    );
    // A query whose answer looks like a status keeps its rows: it starts
    // like a query, or its shape isn't a status (two rows, a text cell).
    for (sql, answer) in [
        (
            "WITH x AS (SELECT 1) SELECT count(*) AS Count FROM x",
            json!({"columns": ["Count"], "rows": [[1]]}),
        ),
        (
            "FROM range(3) SELECT count(*) AS Count",
            json!({"columns": ["Count"], "rows": [[3]]}),
        ),
        (
            "SELECT 1 AS Success WHERE false",
            json!({"columns": ["Success"], "rows": []}),
        ),
        ("CALL f()", json!({"columns": ["Success"], "rows": []})),
        (
            "CREATE TABLE v AS SELECT 1",
            json!({"columns": ["Count"], "rows": [[1], [2]]}),
        ),
        (
            "CREATE TABLE w AS SELECT 1",
            json!({"columns": ["Count"], "rows": [["1"]]}),
        ),
    ] {
        let kind = if sql.starts_with("SELECT") {
            "stream"
        } else {
            "query"
        };
        let script_sql = if kind == "stream" {
            paginate("duckdb", sql, 101, 0)
        } else {
            sql.to_string()
        };
        e.driver
            .load(vec![expect(kind, &script_sql, json!([]), answer)]);
        let ev = run(&e, json!({"text": sql})).await;
        assert_eq!(
            types(&ev),
            ["statementStart", "batch", "statementDone", "done"],
            "{sql}: {ev:?}"
        );
        assert!(e.driver.problems().is_empty(), "{:?}", e.driver.problems());
    }
    // A PRAGMA setting answers with a status and stays hidden; a PRAGMA
    // that lists something has its own columns and shows.
    for (sql, answer, shown) in [
        (
            "PRAGMA threads = 2",
            json!({"columns": ["Success"], "rows": []}),
            false,
        ),
        (
            "PRAGMA x",
            json!({"columns": ["Count"], "rows": [[1]]}),
            false,
        ),
        (
            "PRAGMA database_list",
            json!({"columns": ["seq", "name", "file"], "rows": [[0, "memory", null]]}),
            true,
        ),
    ] {
        e.driver.load(vec![expect("query", sql, json!([]), answer)]);
        let ev = run(&e, json!({"text": sql})).await;
        let want: &[&str] = if shown {
            &["statementStart", "batch", "statementDone", "done"]
        } else {
            &["statementStart", "statementDone", "done"]
        };
        assert_eq!(types(&ev), want, "{sql}: {ev:?}");
    }
    // Elsewhere a column named `Success` is a result like any other.
    let e = env("postgres").await;
    e.driver.load(vec![expect(
        "query",
        "SHOW x",
        json!([]),
        json!({"columns": ["Success"], "rows": []}),
    )]);
    let ev = run(&e, json!({"text": "SHOW x"})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
}

#[tokio::test]
async fn an_other_statement_over_the_row_cap_is_result_too_large() {
    let e = env("mysql").await;
    e.driver.load(vec![
        expect(
            "query",
            "SHOW TABLES",
            json!([]),
            json!({"error": {"code": "RESULT_TOO_LARGE", "message": "more than 100000 rows"}}),
        ),
        expect(
            "stream",
            &paginate("mysql", "SELECT 1 AS a", 101, 0),
            json!([]),
            json!({"columns": ["a"], "rows": [[1]]}),
        ),
    ]);
    let ev = run(&e, json!({"text": "SHOW TABLES; SELECT 1 AS a"})).await;
    assert_eq!(
        types(&ev),
        [
            "statementStart",
            "statementError",
            "statementStart",
            "batch",
            "statementDone",
            "done"
        ]
    );
    assert_eq!(ev[1]["code"], "RESULT_TOO_LARGE");
    assert!(e.driver.problems().is_empty());
}

// ── The clock and the engine ──

#[tokio::test]
async fn elapsed_comes_from_the_executor() {
    let e = env("postgres").await;
    e.driver.load(vec![
        expect(
            "execute",
            "INSERT INTO t VALUES (1)",
            json!([]),
            json!({"rowsAffected": 1}),
        ),
        expect(
            "query",
            "SET x = 1",
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "x"}}),
        ),
    ]);
    let ev = run(&e, json!({"text": "INSERT INTO t VALUES (1); SET x = 1"})).await;
    // One reading before and one after each statement, 5 ms apart.
    assert_eq!(ev[1]["elapsedMs"], 5.0);
    assert_eq!(ev[3]["elapsedMs"], 5.0);
}

#[tokio::test]
async fn without_an_executor_run_is_not_supported() {
    let e = env_with("postgres", None).await;
    let ev = run(&e, json!({"text": "SELECT 1"})).await;
    assert_eq!(types(&ev), ["error"]);
    assert_eq!(ev[0]["code"], "NOT_SUPPORTED");
    let ev = page(&e, json!({"source": {"sql": "SELECT 1", "params": []}})).await;
    assert_eq!(ev[0]["code"], "NOT_SUPPORTED");
    assert!(e.driver.calls().is_empty());
}

#[tokio::test]
async fn mariadb_connection_scans_as_mariadb() {
    let text = "SELECT 1 AS a /*M! ; DELETE FROM t */";
    // MariaDB: the executable comment is code, so its DELETE needs a
    // confirmation.
    let e = env("mariadb").await;
    let ev = run(&e, json!({"text": text})).await;
    assert_eq!(ev[0]["code"], CONFIRM_REQUIRED);
    assert_eq!(ev[0]["destructive"][0]["sql"], "DELETE FROM t */");
    // And the read-only check refuses it, as before.
    let events: Vec<StreamEvent> =
        e.ws.query_stream(
            &e.core,
            "ro".into(),
            e.connection_id.clone(),
            text.into(),
            vec![],
            QueryOptions::default().with_read_only(true),
        )
        .collect()
        .await;
    assert!(
        matches!(&events[..], [StreamEvent::Error { code, .. }] if code == "READ_ONLY"),
        "{events:?}"
    );
    // MySQL: one statement with a comment, run without asking.
    let e = env("mysql").await;
    e.driver.load(vec![expect(
        "stream",
        &paginate("mysql", text, 101, 0),
        json!([]),
        json!({"columns": ["a"], "rows": [[1]]}),
    )]);
    let ev = run(&e, json!({"text": text})).await;
    assert_eq!(
        types(&ev),
        ["statementStart", "batch", "statementDone", "done"]
    );
}

/// Every log record this test binary makes, message and key-values, as
/// `LEVEL target: message {key=value …}`. testkit's `capture_logs` prints the
/// message only, and Core puts most of what it logs in key-values.
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
        let line = format!(
            "{} {}: {}{}",
            record.level(),
            record.target(),
            record.args(),
            kvs.0
        );
        self.0.lock().unwrap().push(line);
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
async fn no_sql_or_values_in_logs() {
    let capture = capture_logs();
    let e = env("postgres").await;
    let count_sql = "SELECT COUNT(*) as total FROM (SELECT 'canary-sql' AS a FROM t WHERE b = $1) AS count_query";
    e.driver.load(vec![
        expect(
            "stream",
            &paginate(
                "postgres",
                "SELECT 'canary-sql' AS a FROM t WHERE b = $1",
                2,
                0,
            ),
            json!(["canary-value"]),
            json!({"columns": ["a"], "rows": [["x"], ["y"]]}),
        ),
        expect(
            "query",
            count_sql,
            json!(["canary-value"]),
            json!({"error": {"code": "QUERY_ERROR", "message": "canary-error"}}),
        ),
        expect(
            "execute",
            "DELETE FROM canary_table",
            json!([]),
            json!({"error": {"code": "QUERY_ERROR", "message": "canary-error"}}),
        ),
    ]);
    let text = "SELECT 'canary-sql' AS a FROM t WHERE b = {{b}};\nDELETE FROM canary_table";
    let ev = run(
        &e,
        json!({"text": text, "pageSize": 1, "confirmed": true,
                            "params": [{"name": "b", "value": "canary-value"}],
                            "history": canary_ctx("canary-unsaved")}),
    )
    .await;
    assert_eq!(last(&ev)["type"], "done");
    // A successful run recorded in history (its name and labels hold canaries).
    e.driver.load(vec![expect(
        "query",
        "SET canary = 1",
        json!([]),
        json!({"columns": [], "rows": []}),
    )]);
    let ev = run(
        &e,
        json!({"text": "SET canary = 1", "history": canary_ctx(SAVED)}),
    )
    .await;
    assert_eq!(last(&ev)["succeeded"], true);
    // A page, and a refused run.
    e.driver.load(vec![expect(
        "stream",
        "SELECT 'canary-sql' LIMIT 1",
        json!(["canary-value"]),
        json!({"columns": ["a"], "rows": [["x"]]}),
    )]);
    page(
        &e,
        json!({"source": {"sql": "SELECT 'canary-sql' LIMIT 1", "params": ["canary-value"]}}),
    )
    .await;
    run(&e, json!({"text": "DROP TABLE canary_table"})).await;
    let lines = capture.0.lock().unwrap().clone();
    for activity in [
        "activity=db.run ",
        "activity=db.run.statement",
        "activity=db.run.count",
        "activity=db.page",
    ] {
        assert!(
            lines.iter().any(|l| l.contains(activity)),
            "no {activity} logged: {lines:#?}"
        );
    }
    for line in lines {
        // sqlparser (the table and column refs) logs its tokens, literals
        // included, at DEBUG. Every interface keeps that target below DEBUG
        // (the desktop and CLI turn it off; the server logs at INFO).
        if line.starts_with("DEBUG sqlparser") || line.starts_with("TRACE sqlparser") {
            continue;
        }
        assert!(!line.contains("canary"), "{line}");
    }
}

#[test]
fn page_source_debug_shows_no_sql() {
    let source = PageSource {
        sql: "SELECT 'canary'".into(),
        params: vec![Value::Text("canary".into())],
    };
    assert!(!format!("{source:?}").contains("canary"));
    let _ = (
        ParamValue {
            name: "a".into(),
            value: Value::Null,
        },
        RunTarget::All,
    );
}
