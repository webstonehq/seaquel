//! One module per TypeScript repository in `src/lib/storage/repos/`, with the
//! same methods in snake_case and the same SQL. Row types live in
//! `seaquel_types::storage`. A method the TypeScript ran as a batch
//! (`db.transaction`) runs in one transaction, and so do the multi-statement
//! saves it ran one statement at a time (`projects::save`,
//! `connections::save`, `shared_repos::save_all`, `project_state::remove`),
//! so a failure leaves nothing half-written.

mod codec;

pub mod ai_chats;
pub mod app_state;
pub mod connection_overrides;
pub mod connections;
pub mod dashboard_versions;
pub mod dashboards;
pub mod import_state;
pub mod license;
pub mod onboarding;
pub mod project_state;
pub mod projects;
pub mod query_history;
pub mod query_versions;
pub mod saved_queries;
pub mod shared_repos;
pub mod themes;
pub mod tutorial;
pub mod user_credentials;
pub mod vault_state;
