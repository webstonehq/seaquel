//! sqlx logs every statement at DEBUG and each one slower than a second at
//! WARN, with its whole SQL. The license server's `auth.db` pool turns both
//! off.

mod common;

use std::sync::{Mutex, OnceLock, PoisonError};

use common::Env;

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
async fn auth_db_never_logs_statements() {
    let capture = capture();
    // The harness's own connections (creating auth.db) may log; only what
    // the server's calls log counts.
    let env = Env::new(1_750_000_000).await;
    let before = capture
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len();
    env.server.install_status().await.unwrap();
    env.server.gate(Some("nobody")).await.ok();
    let lines = capture.0.lock().unwrap_or_else(PoisonError::into_inner)[before..].to_vec();
    let logged: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains(" sqlx::query:"))
        .collect();
    assert!(logged.is_empty(), "statements reached the log: {logged:#?}");
}
