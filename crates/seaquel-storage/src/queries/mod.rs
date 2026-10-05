//! One module per TypeScript repository in `src/lib/storage/repos/`, with the
//! same methods in snake_case and the same SQL. Row types live in
//! `seaquel_types::storage`. A method the TypeScript ran as a batch
//! (`db.transaction`) runs in one transaction, and so do the multi-statement
//! saves it ran one statement at a time (`projects::save`,
//! `connections::save`, `shared_repos::save_all`, `project_state::remove`),
//! so a failure leaves nothing half-written.
//!
//! The targeted functions of phase 5d (`get`, `insert`, `update`, `delete`,
//! …) take a [`crate::WriteTx`] for writes, and `impl Into<Reader>` for reads,
//! so Core can read, check and write inside one transaction. They change
//! only what they name: no replace-all.

use crate::db;

pub(crate) mod codec;

/// A row's id and name, for Core's duplicate-name check (`NAME_TAKEN`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdName {
    pub id: String,
    pub name: String,
}

/// Where a shared row's file is and what it last synced (migration `0004_shared_links.sql`).
/// `None` is "not
/// known": the row's file is the slug path of its name, and there is no
/// base. Storage stores and reads these as given; what they mean is Core's.
///
/// - `path`: the repo-relative path of the row's file. For a connection it
///   is `shared_connection_id` (`<repoId>:<path>`), where older releases
///   keep a template's link.
/// - `base`: the hash of the content both sides had at the last sync.
/// - `file_id`: the `id` written into the file.
///
/// Its `Debug` says only which parts are set: a path holds project and
/// query names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SharedLink {
    pub path: Option<String>,
    pub base: Option<String>,
    pub file_id: Option<String>,
}

impl std::fmt::Debug for SharedLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedLink")
            .field("path", &self.path.is_some())
            .field("base", &self.base.is_some())
            .field("file_id", &self.file_id.is_some())
            .finish()
    }
}

/// One row's id and its [`SharedLink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLink {
    pub id: String,
    pub link: SharedLink,
}

/// Reads `(id, link path, base, file id)` rows into [`RowLink`]s, skipping
/// rows with no id (a hand-edited file).
pub(crate) type LinkRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub(crate) fn row_links(rows: Vec<LinkRow>) -> Vec<RowLink> {
    rows.into_iter()
        .filter_map(|(id, path, base, file_id)| {
            Some(RowLink {
                id: id?,
                link: SharedLink {
                    path,
                    base,
                    file_id,
                },
            })
        })
        .collect()
}

/// Runs a `SET_LINK` statement (`?1` path, `?2` base, `?3` file id, `?4`
/// the row's id). Whether a row had that id.
pub(crate) async fn set_link(
    conn: &mut db::SqliteConnection,
    sql: &str,
    id: &str,
    link: &SharedLink,
) -> codec::Result<bool> {
    let done = db::query(sql)
        .bind(&link.path)
        .bind(&link.base)
        .bind(&link.file_id)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// One row's [`SharedLink`] (`sql` selects `path, base, file_id` by `?1`
/// the id), or `None` when there's no such row.
pub(crate) async fn link_of(
    conn: &mut db::SqliteConnection,
    sql: &str,
    id: &str,
) -> codec::Result<Option<SharedLink>> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = db::query_as(sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.map(|(path, base, file_id)| SharedLink {
        path,
        base,
        file_id,
    }))
}

/// Stores `name`'s key in the `name_key` column of `table`'s row `id`
/// (migration `0001_name_keys.sql`), after the write that stored the name.
/// A separate statement, so the stale-key trigger, which NULLs the key when
/// a rename keeps it (a case-only rename), can't leave it NULL.
pub(crate) async fn set_name_key(
    conn: &mut db::SqliteConnection,
    table: &str,
    id: &str,
    name: &str,
) -> codec::Result<()> {
    db::query(&format!("UPDATE {table} SET name_key = ? WHERE id = ?"))
        .bind(seaquel_types::names::name_key(name))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// One row of a `name_key` lookup: rowid, id, name (as bytes, since a
/// hand-edited file can hold a name that isn't UTF-8) and stored key.
pub(crate) type KeyedRow = (i64, Option<String>, Option<Vec<u8>>, Option<String>);

/// The rows a `name_key` lookup found (`id`, `name`, stored key), in rowid
/// order: those whose stored key is `key`, and those with no stored key
/// whose name has it (rows an older release wrote or renamed). A name
/// that isn't UTF-8 has no key and matches nothing; it never fails the
/// lookup.
pub(crate) fn matching_key(rows: Vec<KeyedRow>, key: &str) -> Vec<IdName> {
    rows.into_iter()
        .filter_map(|(_, id, name, stored)| {
            let name = match name {
                Some(bytes) => String::from_utf8(bytes).ok()?,
                None => String::new(),
            };
            let matches = match stored {
                Some(stored) => stored == key,
                None => seaquel_types::names::name_key(&name) == key,
            };
            matches.then(|| IdName {
                id: id.unwrap_or_default(),
                name,
            })
        })
        .collect()
}

pub mod ai_chats;
pub mod app_state;
pub mod connection_overrides;
pub mod connections;
pub mod dashboard_versions;
pub mod dashboards;
pub mod import_state;
pub mod license;
pub mod onboarding;
pub mod project_labels;
pub mod project_state;
pub mod projects;
pub mod query_history;
pub mod query_versions;
pub mod saved_canvases;
pub mod saved_queries;
pub mod shared_repos;
pub mod themes;
pub mod tutorial;
pub mod user_credentials;
pub mod vault_state;
pub mod window_state;
pub mod windows;
