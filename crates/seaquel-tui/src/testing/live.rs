//! Live-database test support: a table name no other run uses,
//! and a guard that drops the table when the test ends, passed or failed.

use std::time::Duration;

use seaquel_types::ConnectConfig;

/// `<prefix>_<pid>_<nanos>_<n>`: unique across runs and within one.
pub fn unique_table(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!(
        "{prefix}_{}_{nanos}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Runs `DROP TABLE IF EXISTS <table>` when dropped, on a connection of its
/// own (opened from the live test's `SEAQUEL_TEST_*` config on a thread
/// with its own runtime, so it works while a test's runtime unwinds a
/// panic). Make it before the table, so a failure anywhere after leaves
/// nothing behind.
pub struct TableGuard {
    config: serde_json::Value,
    table: String,
}

impl TableGuard {
    /// `table` as the engine names it (`public.t`, or `t`).
    pub fn new(config: &serde_json::Value, table: &str) -> TableGuard {
        TableGuard {
            config: config.clone(),
            table: table.to_string(),
        }
    }
}

/// How long the drop may take before the guard gives up.
const DROP_WITHIN: Duration = Duration::from_secs(20);

impl Drop for TableGuard {
    fn drop(&mut self) {
        let config = self.config.clone();
        let sql = format!("DROP TABLE IF EXISTS {}", self.table);
        let dropped = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            runtime.block_on(async {
                tokio::time::timeout(DROP_WITHIN, async {
                    let config: ConnectConfig = serde_json::from_value(config).ok()?;
                    let core = super::core::app_core();
                    let id = core.connect(&config).await.ok()?.connection_id;
                    let done = core.execute(&id, &sql, Vec::new()).await.is_ok();
                    let _ = core.disconnect(&id).await;
                    done.then_some(())
                })
                .await
                .ok()
                .flatten()
            })
        })
        .join();
        if !matches!(dropped, Ok(Some(()))) {
            eprintln!("a live test's table wasn't dropped: {}", self.table);
        }
    }
}

/// Whether `table` exists, read on a connection of its own.
pub async fn table_exists(config: &serde_json::Value, table: &str) -> bool {
    let core = super::core::app_core();
    let config: ConnectConfig = serde_json::from_value(config.clone()).unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    let found = core
        .query(
            &id,
            &format!("SELECT 1 FROM {table} WHERE 1 = 0"),
            Vec::new(),
        )
        .await
        .is_ok();
    let _ = core.disconnect(&id).await;
    found
}
