//! sqlx logs every statement at DEBUG and each one slower than a second at
//! WARN, with its whole SQL. Storage turns both off on every connection it
//! opens (the pool, the probe and the read-only checks).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Mutex, OnceLock, PoisonError};

use seaquel_storage::{Storage, StorageOptions};

struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!(
                "{} {}: {}",
                record.level(),
                record.target(),
                record.args()
            ));
    }
    fn flush(&self) {}
}

fn capture() -> &'static Capture {
    static CAPTURE: OnceLock<&'static Capture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let c: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        log::set_logger(c).expect("logger");
        log::set_max_level(log::LevelFilter::Trace);
        c
    })
}

#[tokio::test]
async fn storage_never_logs_statements() {
    let capture = capture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    // Created, migrated, then opened again: the probe runs on an existing
    // file, and the read-only open runs its checks.
    let storage = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    seaquel_storage::connections::load_all(&storage)
        .await
        .unwrap();
    storage.close().await;
    let storage = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    storage.close().await;
    let read_only = StorageOptions {
        read_only: true,
        ..StorageOptions::default()
    };
    let storage = Storage::open(&path, read_only).await.unwrap();
    seaquel_storage::connections::load_all(&storage)
        .await
        .unwrap();
    storage.close().await;

    let lines = capture
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let logged: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains(" sqlx::query:"))
        .collect();
    assert!(logged.is_empty(), "statements reached the log: {logged:#?}");
}
