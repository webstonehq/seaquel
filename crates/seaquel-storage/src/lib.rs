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

pub use data_dir::{data_dir, DATA_DIR_ENV};
pub use data_steps::DATA_STEPS_TABLE;
pub use error::{
    StorageError, LEGACY_JSON_FILES, LEGACY_STORAGE, NO_DATA_DIR, STORAGE_CORRUPT, STORAGE_ERROR,
};
pub use open::{Storage, StorageOptions};

pub use connection_string::strip_connection_string_password;
/// The typed queries, one module per table group (see `queries/mod.rs`).
pub use queries::{
    ai_chats, app_state, connection_overrides, connections, dashboard_versions, dashboards,
    import_state, license, onboarding, project_state, projects, query_history, query_versions,
    saved_queries, shared_repos, themes, tutorial, user_credentials, vault_state,
};
