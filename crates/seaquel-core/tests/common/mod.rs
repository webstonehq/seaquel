//! Helpers shared by the library, change-event and string-secrets tests:
//! a test secret store that can list, fail and block, a log capture, a Core
//! with a clock, and raw row access to the metadata file.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use seaquel_core::secrets::{SecretError, SecretOp, SecretStore};
use seaquel_core::storage::Storage;
use seaquel_core::{Core, LibraryLimits, StateLimits, Workspace, WorkspaceSpec};
use serde_json::{Map, Value};
use sqlx::{Column, Row, TypeInfo, ValueRef};
use tokio::sync::Notify;

/// A secret store in memory that can list its entries, fail its writes,
/// and hold a `set` until released. Never the real keychain.
#[derive(Default)]
pub struct TestStore {
    entries: Mutex<BTreeMap<String, String>>,
    pub fail_set: AtomicBool,
    /// Fail `set` for this key only.
    pub fail_set_key: Mutex<Option<String>>,
    pub fail_get: AtomicBool,
    /// Hold every `set` until [`TestStore::release`].
    pub block_set: AtomicBool,
    pub entered_set: Notify,
    released: Notify,
    pub sets: AtomicUsize,
    pub gets: AtomicUsize,
    pub deletes: AtomicUsize,
}

impl TestStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn with(entries: &[(&str, &str)]) -> Arc<Self> {
        let store = Self::new();
        for (k, v) in entries {
            store
                .entries
                .lock()
                .unwrap()
                .insert(k.to_string(), v.to_string());
        }
        store
    }

    pub fn put(&self, key: &str, value: &str) {
        self.entries
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub fn entries(&self) -> BTreeMap<String, String> {
        self.entries.lock().unwrap().clone()
    }

    pub fn release(&self) {
        self.block_set.store(false, Ordering::SeqCst);
        self.released.notify_waiters();
    }

    fn failure(op: SecretOp, key: &str) -> SecretError {
        SecretError::Store {
            op,
            key: key.to_string(),
            message: "keychain locked".to_string(),
        }
    }
}

#[seaquel_runtime::async_trait]
impl SecretStore for TestStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        seaquel_core::secrets::validate_key(key)?;
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(Self::failure(SecretOp::Get, key));
        }
        Ok(self.entries.lock().unwrap().get(key).cloned())
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        seaquel_core::secrets::validate_key(key)?;
        self.sets.fetch_add(1, Ordering::SeqCst);
        if self.block_set.load(Ordering::SeqCst) {
            let released = self.released.notified();
            self.entered_set.notify_one();
            released.await;
        }
        if self.fail_set.load(Ordering::SeqCst)
            || self.fail_set_key.lock().unwrap().as_deref() == Some(key)
        {
            return Err(Self::failure(SecretOp::Set, key));
        }
        self.entries
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        seaquel_core::secrets::validate_key(key)?;
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.entries.lock().unwrap().remove(key);
        Ok(())
    }
}

/// A desktop-like Core: every engine, a clock, no limits.
pub fn core() -> Core {
    core_with(LibraryLimits::default())
}

pub fn core_with(limits: LibraryLimits) -> Core {
    seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .library_limits(limits)
        .build()
}

/// The web server's engines only (Postgres, MySQL, MSSQL).
pub fn web_core(limits: LibraryLimits) -> Core {
    seaquel_core::with_plugins(|id| ["postgres", "mysql", "mssql"].contains(&id))
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .library_limits(limits)
        .build()
}

/// Insert `rows` (column → JSON value) into `table` with plain SQL.
pub async fn insert_rows(st: &Storage, table: &str, rows: &[Value]) {
    for row in rows {
        let obj = row.as_object().expect("a seed row is an object");
        let cols: Vec<&String> = obj.keys().collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            cols.iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            vec!["?"; cols.len()].join(", ")
        );
        let mut q = sqlx::query(&sql);
        for c in &cols {
            q = match &obj[*c] {
                Value::Null => q.bind(None::<String>),
                Value::Bool(b) => q.bind(i64::from(*b)),
                Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
                Value::Number(n) => q.bind(n.as_f64()),
                Value::String(s) => q.bind(s.clone()),
                other => q.bind(other.to_string()),
            };
        }
        q.execute(st.pool())
            .await
            .unwrap_or_else(|e| panic!("seeding {table}: {e}"));
    }
}

/// Every row of `table` as JSON (numbers as numbers, NULL as null), in
/// `order`.
pub async fn dump(st: &Storage, table: &str, order: &str) -> Vec<Value> {
    let rows = sqlx::query(&format!("SELECT * FROM {table} ORDER BY {order}"))
        .fetch_all(st.pool())
        .await
        .unwrap_or_else(|e| panic!("reading {table}: {e}"));
    rows.iter()
        .map(|row| {
            let mut obj = Map::new();
            for (i, col) in row.columns().iter().enumerate() {
                let raw = row.try_get_raw(i).unwrap();
                let v = if raw.is_null() {
                    Value::Null
                } else {
                    match raw.type_info().name() {
                        "INTEGER" => Value::from(row.get::<i64, _>(i)),
                        "REAL" => Value::from(row.get::<f64, _>(i)),
                        "BLOB" => Value::String(format!("{:?}", row.get::<Vec<u8>, _>(i))),
                        _ => Value::String(row.get::<String, _>(i)),
                    }
                };
                obj.insert(col.name().to_string(), v);
            }
            Value::Object(obj)
        })
        .collect()
}

/// The link columns migrations `0004_shared_links.sql` and
/// `0005_shared_connection_origin.sql` (phase 5e) added, by table. The 5d
/// replays were recorded before them and never link a row.
pub const LINK_COLUMNS: &[(&str, &[&str])] = &[
    ("projects", &["shared_dir"]),
    (
        "saved_queries",
        &["shared_path", "shared_base", "shared_file_id"],
    ),
    (
        "dashboards",
        &["shared_path", "shared_base", "shared_file_id"],
    ),
    (
        "connections",
        &["shared_base", "shared_file_id", "shared_origin"],
    ),
];

/// Takes `0004`'s and `0005`'s link columns out of `table`'s dumped rows, after checking
/// that each is there and NULL: a 5d call never links a row.
pub fn drop_link_columns(table: &str, rows: &mut [Value]) {
    let Some((_, columns)) = LINK_COLUMNS.iter().find(|(t, _)| *t == table) else {
        return;
    };
    for row in rows {
        let obj = row.as_object_mut().unwrap();
        for column in *columns {
            assert_eq!(
                obj.remove(*column),
                Some(Value::Null),
                "{table}.{column} in {obj:?}"
            );
        }
    }
}

/// Reads the whole file at `path`, for canary scans.
pub fn file_bytes(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            out.extend(std::fs::read(path).unwrap_or_default());
        }
    }
    out
}

/// Every log record this test binary makes, message and key-values.
pub struct KvCapture(pub Mutex<Vec<String>>);

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

pub fn capture_logs() -> &'static KvCapture {
    static CAPTURE: std::sync::OnceLock<&'static KvCapture> = std::sync::OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture: &'static KvCapture = Box::leak(Box::new(KvCapture(Mutex::default())));
        log::set_logger(capture).expect("another logger is installed");
        log::set_max_level(log::LevelFilter::Trace);
        capture
    })
}

pub fn logged() -> String {
    capture_logs().0.lock().unwrap().join("\n")
}

/// A clock for the state tests: the wall clock at its start, then one
/// millisecond more on every reading, so "most recently used" never ties
/// within a test, and a test can move it (`advance`) or stop it
/// (`freeze`, for writes in one millisecond).
pub struct TickClock {
    start: std::time::Duration,
    ticks: std::sync::atomic::AtomicU64,
    frozen: std::sync::atomic::AtomicBool,
}

impl TickClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            start: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap(),
            ticks: std::sync::atomic::AtomicU64::new(0),
            frozen: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// From now on every reading is the same millisecond.
    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }

    /// Moves the clock `ms` milliseconds on.
    pub fn advance(&self, ms: u64) {
        self.ticks.fetch_add(ms, Ordering::SeqCst);
    }
}

impl seaquel_runtime::Executor for TickClock {
    fn spawn(&self, future: futures::future::BoxFuture<'static, ()>) {
        seaquel_runtime::TokioExecutor.spawn(future)
    }

    fn sleep(&self, duration: std::time::Duration) -> futures::future::BoxFuture<'static, ()> {
        seaquel_runtime::TokioExecutor.sleep(duration)
    }

    fn unix_time(&self) -> std::time::Duration {
        let step = u64::from(!self.frozen.load(Ordering::SeqCst));
        let n = self.ticks.fetch_add(step, Ordering::SeqCst);
        self.start + std::time::Duration::from_millis(n)
    }

    fn monotonic(&self) -> std::time::Duration {
        seaquel_runtime::TokioExecutor.monotonic()
    }
}

/// A desktop-like Core on `clock`, with these state limits.
pub fn state_core(clock: Arc<TickClock>, limits: seaquel_core::StateLimits) -> Core {
    state_core_with(clock, LibraryLimits::default(), limits)
}

/// [`state_core`] with library limits too (a web Core's names and fields).
pub fn state_core_with(
    clock: Arc<TickClock>,
    library: LibraryLimits,
    limits: seaquel_core::StateLimits,
) -> Core {
    seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(clock)
        .library_limits(library)
        .state_limits(limits)
        .build()
}

/// The web server's library limits (`WEB_LIBRARY_LIMITS`), for the state
/// tests' web Cores.
pub fn web_library_limits() -> LibraryLimits {
    LibraryLimits {
        max_name_bytes: Some(1024),
        max_field_bytes: Some(64 * 1024),
        max_query_bytes: Some(2 * 1024 * 1024),
        max_list_items: Some(1_000),
        max_connections: Some(10_000),
        max_projects: Some(1_000),
        max_saved_queries: Some(50_000),
        max_version_bytes: Some(16 * 1024 * 1024),
    }
}

// ── The state tests' workspace ──

pub const T0: &str = "2024-01-01T00:00:00.000Z";

/// A workspace with projects `p1` and `p2` and a connection `c1` in `p1`.
pub struct Fx {
    _dir: tempfile::TempDir,
    pub core: Core,
    pub ws: Arc<Workspace>,
    pub store: Arc<TestStore>,
    pub clock: Arc<TickClock>,
}

pub async fn fx_with(limits: StateLimits, with_store: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = TestStore::new();
    let clock = TickClock::new();
    // A workspace without a store is the web's: its library limits too.
    let library = if with_store {
        seaquel_core::LibraryLimits::default()
    } else {
        web_library_limits()
    };
    let core = state_core_with(clock.clone(), library, limits);
    let spec = WorkspaceSpec::new(dir.path());
    let spec = if with_store {
        spec.with_secrets(store.clone())
    } else {
        spec
    };
    let ws = core.open_workspace(spec).await.unwrap();
    for (id, name) in [("p1", "Main"), ("p2", "Other")] {
        insert_rows(
            ws.storage(),
            "projects",
            &[
                serde_json::json!({"id": id, "name": name, "description": null, "created_at": T0,
                     "updated_at": T0, "git_repo_path": null}),
            ],
        )
        .await;
    }
    insert_rows(
        ws.storage(),
        "connections",
        &[
            serde_json::json!({"id": "c1", "project_id": "p1", "name": "C1", "type": "postgres",
                 "host": "h", "port": 5432, "database_name": "d", "username": "u"}),
        ],
    )
    .await;
    Fx {
        _dir: dir,
        core,
        ws,
        store,
        clock,
    }
}

/// A web Core's state limits (`WEB_STATE_LIMITS` in `seaquel-server`).
pub fn web_state_limits() -> StateLimits {
    StateLimits {
        max_view_state_bytes: Some(8 * 1024 * 1024),
        max_tab_text_bytes: Some(2 * 1024 * 1024),
        max_tabs: Some(500),
        max_windows: 50,
        max_window_states_per_project: 20,
        spare_main_window: false,
        max_workflow_bytes: Some(16 * 1024 * 1024),
        max_workflows: Some(1_000),
        max_dashboard_bytes: Some(4 * 1024 * 1024),
        max_dashboards: Some(1_000),
        max_dashboard_version_bytes: Some(16 * 1024 * 1024),
        max_message_bytes: Some(1024 * 1024),
        max_messages_per_chat: Some(5_000),
        max_chat_bytes: Some(64 * 1024 * 1024),
        max_chats: Some(10_000),
        max_setting_bytes: Some(256 * 1024),
        max_user_themes: Some(200),
        max_ai_providers: Some(50),
    }
}
