//! `auth.db`, opened next to Node's better-sqlite3 handle.
//!
//! The pool is lazy and never creates the file: Node creates it and applies
//! migrations 006–012 on its first auth call. Each connection sets WAL, a
//! 5 s busy timeout and foreign keys, as Node's does, so the two processes
//! share the file. Until the license tables exist every call is
//! `NOT_READY`; the check runs until it passes once.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use super::{Result, ServerError, ServerErrorCode};

/// Tables and columns the license code reads or writes (migrations
/// 006–012). `user` is joined for the member list.
const REQUIRED: &[(&str, &[&str])] = &[
    ("user", &["id", "email"]),
    (
        "member_license",
        &[
            "user_id",
            "license_key",
            "bound_at",
            "control_member_id",
            "last_validated_at",
            "cached_status",
            "grace_until",
            "is_owner",
            "revoked_at",
        ],
    ),
    ("install", &["id", "install_id", "created_at"]),
    (
        "install_cache",
        &[
            "id",
            "tenant_id",
            "slug",
            "status",
            "tier",
            "seat_limit",
            "current_period_end",
            "last_validated_at",
            "grace_until",
            "mode",
        ],
    ),
    (
        "airgap_bundle",
        &[
            "id",
            "raw_envelope",
            "verified_payload",
            "pubkey_fingerprint",
            "imported_at",
            "not_after",
            "payload_sha256",
            "issued_at",
        ],
    ),
];

pub(crate) struct AuthDb {
    pool: SqlitePool,
    ready: tokio::sync::OnceCell<()>,
}

impl AuthDb {
    pub(crate) fn new(path: &Path) -> Self {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .idle_timeout(Some(Duration::from_secs(60)))
            .connect_lazy_with(options);
        Self {
            pool,
            ready: tokio::sync::OnceCell::new(),
        }
    }

    /// The pool once the license tables exist; `on_ready` runs the first
    /// time they do.
    pub(crate) async fn ready<'a, F, Fut>(&'a self, on_ready: F) -> Result<&'a SqlitePool>
    where
        F: FnOnce(&'a SqlitePool) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        self.ready
            .get_or_try_init(|| async {
                check_tables(&self.pool).await?;
                on_ready(&self.pool).await
            })
            .await?;
        Ok(&self.pool)
    }
}

async fn check_tables(pool: &SqlitePool) -> Result<()> {
    let not_ready = |why: String| ServerError::new(ServerErrorCode::NotReady, why);
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| not_ready(format!("auth.db can't be opened yet: {e}")))?;
    for (table, columns) in REQUIRED {
        let found: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info(?)")
            .bind(table)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| not_ready(format!("auth.db can't be read yet: {e}")))?;
        if found.is_empty() {
            return Err(not_ready(format!(
                "auth.db has no {table} table yet (Node applies its migrations on first use)"
            )));
        }
        if let Some(missing) = columns.iter().find(|c| !found.iter().any(|(n,)| n == *c)) {
            return Err(not_ready(format!(
                "auth.db's {table} table has no {missing} column yet"
            )));
        }
    }
    Ok(())
}
