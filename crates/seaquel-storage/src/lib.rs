//! Seaquel's metadata storage. It owns the SQLite file that holds connections,
//! projects, saved queries, history, dashboards and settings: the schema, the
//! baseline that upgrades every released file, the numbered migrations after
//! it, and one typed function per query. Interfaces reach it only through
//! Core's workspace, so no SQL for this database crosses from the UI.

mod connection_string;
#[cfg(not(target_arch = "wasm32"))]
mod data_dir;
mod data_steps;
mod db;
mod error;
mod lock;
#[cfg(any(target_arch = "wasm32", test))]
mod migrations;
mod open;
#[cfg(target_arch = "wasm32")]
mod open_mem;
mod queries;
pub mod schema;
mod write;

#[cfg(not(target_arch = "wasm32"))]
pub use data_dir::{data_dir, data_local_dir, DATA_DIR_ENV};
pub use data_steps::DATA_STEPS_TABLE;
/// The SQL layer's error and migration error: sqlx's on native targets,
/// the in-memory executor's (same variants, codes and messages) on wasm32.
pub use db::{Error as DbError, MigrateError};
pub use error::{
    StorageError, LEGACY_JSON_FILES, LEGACY_STORAGE, NO_DATA_DIR, STORAGE_CORRUPT, STORAGE_ERROR,
    STORAGE_FULL, STORAGE_NEEDS_UPGRADE, STORAGE_NOT_FOUND, STORAGE_READ_ONLY,
};
#[cfg(target_arch = "wasm32")]
pub use open::Image;
pub use open::{SchemaPolicy, Storage, StorageOptions, CAP_PAGE_SIZE, WRITE_WAIT};
pub use write::{Reader, WriteTx};

/// Refills the `name_key` of every row that has a text name and no key
/// (phase 5d-1 probe fix; dashboards since 5d-2): the rows an older release wrote or renamed
/// after `backfill_name_keys` ran, for instance on a downgrade and
/// re-upgrade. Core runs it on each writable open. It only reads when
/// there's nothing to fill (a name that isn't UTF-8 can't have a key, so it
/// never counts as something to fill), is never recorded, and does nothing on
/// read-only storage. Returns whether it wrote.
pub async fn refill_name_keys(st: &Storage) -> Result<bool, StorageError> {
    if st.is_read_only() {
        return Ok(false);
    }
    if !data_steps::name_keys_pending(st.pool()).await? {
        return Ok(false);
    }
    let mut tx = st.write().await?;
    data_steps::fill_name_keys(tx.conn(), &data_steps::ALL_NAME_KEY_TABLES).await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether any workflow lacks its `meta` or any version its
/// `widget_count` (migration `0003`): two lookups in partial indexes, no
/// body read.
pub const LIST_META_PENDING: &str =
    "SELECT EXISTS (SELECT 1 FROM saved_canvases WHERE meta IS NULL) \
     OR EXISTS (SELECT 1 FROM dashboard_versions WHERE widget_count IS NULL)";

/// Refills the list metadata of the rows an older release wrote without
/// it (5d-2 Task 7 review): its replace-all save of a project's saved
/// workflows inserts whole rows with no `meta`, and its versions have no
/// `widget_count`. Core runs it on each writable open, after
/// [`refill_name_keys`], and a capped (web) open runs it too. Like that
/// one, it only reads when there's nothing to fill ([`LIST_META_PENDING`]),
/// is never recorded as a data step, and does nothing on read-only
/// storage. Returns whether it wrote.
pub async fn refill_list_meta(st: &Storage) -> Result<bool, StorageError> {
    if st.is_read_only() {
        return Ok(false);
    }
    let pending: bool = db::query_scalar(LIST_META_PENDING)
        .fetch_one(st.pool())
        .await?;
    if !pending {
        return Ok(false);
    }
    let mut tx = st.write().await?;
    db::query(&queries::saved_canvases::refill_sql())
        .execute(tx.conn())
        .await?;
    db::query(&queries::dashboard_versions::refill_sql())
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub use connection_string::{
    is_legacy_built_string, legacy_built_string, split_connection_string_secret,
    strip_connection_string_password, strip_connection_string_secrets, SecretSplit, StringFields,
};
pub use queries::{
    ai_chats, app_state, connection_overrides, connections, dashboard_versions, dashboards,
    import_state, license, onboarding, project_labels, project_state, projects, query_history,
    query_versions, saved_canvases, saved_queries, shared_repos, themes, tutorial,
    user_credentials, vault_state, window_state, windows,
};
/// The typed queries, one module per table group (see `queries/mod.rs`).
pub use queries::{IdName, RowLink, SharedLink};
