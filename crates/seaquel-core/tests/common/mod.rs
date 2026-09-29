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
use seaquel_core::{Core, LibraryLimits};
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
