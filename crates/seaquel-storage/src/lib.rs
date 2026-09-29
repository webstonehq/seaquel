//! Seaquel's metadata storage. It owns the SQLite file that holds connections,
//! projects, saved queries, history, dashboards and settings: the schema, the
//! baseline that upgrades every released file, the numbered migrations after
//! it, and one typed function per query. Interfaces reach it only through
//! Core's workspace, so no SQL for this database crosses from the UI.

mod connection_string;
mod data_dir;
mod data_steps;
mod error;
mod open;
mod queries;
pub mod schema;
mod write;

pub use data_dir::{data_dir, DATA_DIR_ENV};
pub use data_steps::DATA_STEPS_TABLE;
pub use error::{
    StorageError, LEGACY_JSON_FILES, LEGACY_STORAGE, NO_DATA_DIR, STORAGE_CORRUPT, STORAGE_ERROR,
    STORAGE_NEEDS_UPGRADE, STORAGE_NOT_FOUND, STORAGE_READ_ONLY,
};
pub use open::{Storage, StorageOptions, WRITE_WAIT};
pub use write::{Reader, WriteTx};

/// Refills the `name_key` of every row that has a text name and no key
/// (phase 5d-1 probe fix): the rows an older release wrote or renamed
/// after `backfill_name_keys` ran, for instance on a downgrade and
/// re-upgrade. Core runs it on each writable open. It only reads when
/// there's nothing to fill, is never recorded, and does nothing on
/// read-only storage. Returns whether it wrote.
pub async fn refill_name_keys(st: &Storage) -> Result<bool, StorageError> {
    if st.is_read_only() {
        return Ok(false);
    }
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM connections WHERE name_key IS NULL AND typeof(name) = 'text') \
         OR EXISTS (SELECT 1 FROM projects WHERE name_key IS NULL AND typeof(name) = 'text') \
         OR EXISTS (SELECT 1 FROM saved_queries WHERE name_key IS NULL AND typeof(name) = 'text')",
    )
    .fetch_one(st.pool())
    .await?;
    if !pending {
        return Ok(false);
    }
    let mut tx = st.write().await?;
    data_steps::backfill_name_keys(tx.conn()).await?;
    tx.commit().await?;
    Ok(true)
}

pub use connection_string::{
    is_legacy_built_string, legacy_built_string, split_connection_string_secret,
    strip_connection_string_password, strip_connection_string_secrets, SecretSplit, StringFields,
};
/// The typed queries, one module per table group (see `queries/mod.rs`).
pub use queries::IdName;
pub use queries::{
    ai_chats, app_state, connection_overrides, connections, dashboard_versions, dashboards,
    import_state, license, onboarding, project_labels, project_state, projects, query_history,
    query_versions, saved_queries, shared_repos, themes, tutorial, user_credentials, vault_state,
};
