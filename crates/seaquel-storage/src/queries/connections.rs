//! `connectionsRepo`: `connections` and their `connection_labels`.

use seaquel_types::storage::PersistedConnection;
use sqlx::sqlite::SqliteRow;
use sqlx::SqliteConnection;

use super::codec::{
    begin, bit, flag, json, number, opt_bit, opt_flag, opt_text, select_sql, text,
    truthy_json_text, upsert_sql, Result,
};
use super::{IdName, RowLink, SharedLink};
use crate::{
    split_connection_string_secret, strip_connection_string_password,
    strip_connection_string_secrets, Reader, Storage, WriteTx,
};

const TABLE: &str = "connections";
const COLUMNS: [&str; 21] = [
    "id",
    "project_id",
    "name",
    "type",
    "host",
    "port",
    "database_name",
    "username",
    "ssl_mode",
    "connection_string",
    "last_connected",
    "ssh_tunnel",
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
    "is_local_only",
    "shared_connection_id",
    "ai_share_schema",
    "ai_share_data",
    "active_ai_provider_id",
    "active_ai_model",
];

/// [`COLUMNS`] and the link's origin (migration `0005`), which the reads
/// carry and no write of a whole row names: only [`set_origin`] and
/// [`set_link`] write it.
const READ_COLUMNS: [&str; 22] = [
    "id",
    "project_id",
    "name",
    "type",
    "host",
    "port",
    "database_name",
    "username",
    "ssl_mode",
    "connection_string",
    "last_connected",
    "ssh_tunnel",
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
    "is_local_only",
    "shared_connection_id",
    "ai_share_schema",
    "ai_share_data",
    "active_ai_provider_id",
    "active_ai_model",
    "shared_origin",
];

/// A linked connection this project shared (`connections.shared_origin`).
pub const ORIGIN_EXPORTED: &str = "exported";
/// A linked connection a sync made from a teammate's template.
pub const ORIGIN_IMPORTED: &str = "imported";

/// The label ids of one connection, in the order of `connection_labels`'
/// primary key.
async fn label_ids(conn: &mut SqliteConnection, id: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT label_id FROM connection_labels WHERE connection_id = ?")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows.into_iter().map(|l| l.0).collect())
}

fn map_row(row: &SqliteRow, id: String, label_ids: Vec<String>) -> Result<PersistedConnection> {
    Ok(PersistedConnection {
        id,
        project_id: text(row, "project_id")?,
        name: text(row, "name")?,
        ty: text(row, "type")?,
        host: text(row, "host")?,
        port: number(row, "port")?,
        database_name: text(row, "database_name")?,
        username: text(row, "username")?,
        ssl_mode: opt_text(row, "ssl_mode")?,
        connection_string: opt_text(row, "connection_string")?,
        // `v ? new Date(v) : undefined`: the TypeScript makes the Date.
        last_connected: opt_text(row, "last_connected")?.filter(|t| !t.is_empty()),
        ssh_tunnel: json(row, "ssh_tunnel")?,
        save_password: flag(row, "save_password")?,
        save_ssh_password: flag(row, "save_ssh_password")?,
        save_ssh_key_passphrase: flag(row, "save_ssh_key_passphrase")?,
        label_ids,
        // `v === 1 ? true : undefined`
        is_local_only: flag(row, "is_local_only")?.then_some(true),
        shared_connection_id: opt_text(row, "shared_connection_id")?,
        ai_share_schema: opt_flag(row, "ai_share_schema")?,
        ai_share_data: opt_flag(row, "ai_share_data")?,
        active_ai_provider_id: opt_text(row, "active_ai_provider_id")?,
        active_ai_model: opt_text(row, "active_ai_model")?,
        shared_origin: opt_text(row, "shared_origin")?,
    })
}

/// Every connection with its label ids, in rowid order. The label ids come
/// in the order of `connection_labels`' primary key, not the order they were
/// saved in.
pub async fn load_all(st: &Storage) -> Result<Vec<PersistedConnection>> {
    let rows = sqlx::query(&select_sql(TABLE, &READ_COLUMNS, ""))
        .fetch_all(st.pool())
        .await?;
    let mut conn = st.pool().acquire().await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = text(row, "id")?;
        let labels = label_ids(&mut conn, &id).await?;
        out.push(map_row(row, id, labels)?);
    }
    Ok(out)
}

/// One connection, as [`load_all`] gives it, or `None`.
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<PersistedConnection>> {
    let mut conn = r.into().conn().await?;
    let Some(row) = sqlx::query(&select_sql(TABLE, &READ_COLUMNS, "id = ?"))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };
    let labels = label_ids(&mut conn, id).await?;
    map_row(&row, id.to_string(), labels).map(Some)
}

/// Binds every column of [`COLUMNS`] but `id` and `project_id`, in order,
/// with `connection_string` (already stripped) for the string.
fn bind_fields<'q>(
    q: super::codec::SqliteQuery<'q>,
    c: &'q PersistedConnection,
    connection_string: Option<String>,
) -> super::codec::SqliteQuery<'q> {
    q.bind(&c.name)
        .bind(&c.ty)
        .bind(&c.host)
        .bind(c.port)
        .bind(&c.database_name)
        .bind(&c.username)
        .bind(&c.ssl_mode)
        .bind(connection_string)
        .bind(&c.last_connected)
        .bind(truthy_json_text(&c.ssh_tunnel))
        .bind(bit(c.save_password))
        .bind(bit(c.save_ssh_password))
        .bind(bit(c.save_ssh_key_passphrase))
        .bind(bit(c.is_local_only == Some(true)))
        .bind(&c.shared_connection_id)
        .bind(opt_bit(c.ai_share_schema))
        .bind(opt_bit(c.ai_share_data))
        .bind(&c.active_ai_provider_id)
        .bind(&c.active_ai_model)
}

/// Makes `labels` the connection's labels. A label id listed twice is saved
/// once (a repeat would fail the primary key).
async fn replace_labels(conn: &mut SqliteConnection, id: &str, labels: &[String]) -> Result<()> {
    sqlx::query("DELETE FROM connection_labels WHERE connection_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let mut seen = std::collections::HashSet::new();
    for label_id in labels.iter().filter(|id| seen.insert(id.as_str())) {
        sqlx::query("INSERT INTO connection_labels (connection_id, label_id) VALUES (?, ?)")
            .bind(id)
            .bind(label_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Upserts the connection and replaces its labels, in one transaction. A
/// label id listed twice is saved once.
///
/// The connection string is saved without its password
/// ([`strip_connection_string_password`]), so a row an older build saved
/// with one is cleaned the next time it's saved.
pub async fn save(st: &Storage, c: &PersistedConnection) -> Result<()> {
    let mut tx = begin(st).await?;
    let sql = upsert_sql(TABLE, &COLUMNS, "id");
    let connection_string = c
        .connection_string
        .as_deref()
        .map(strip_connection_string_password);
    bind_fields(
        sqlx::query(&sql).bind(&c.id).bind(&c.project_id),
        c,
        connection_string,
    )
    .execute(&mut *tx)
    .await?;
    replace_labels(&mut tx, &c.id, &c.label_ids).await?;
    tx.commit().await?;
    Ok(())
}

/// The string [`insert`] and [`update`] store: the TypeScript's
/// [`strip_connection_string_secrets`]. NULL for none or `""`; a string the
/// strip can't read safely (a URL the parser rejects, a fragment, an
/// unclosed quote, a leftover `password=`) is blanked and stored as `""`.
fn stored_string(c: &PersistedConnection) -> Option<String> {
    c.connection_string
        .as_deref()
        .and_then(strip_connection_string_secrets)
}

/// Inserts a new connection and its labels. An id that exists fails (the
/// primary key) rather than overwriting. The connection string is stored
/// with no secret in it ([`strip_connection_string_secrets`]).
pub async fn insert(tx: &mut WriteTx, c: &PersistedConnection) -> Result<()> {
    let conn = tx.conn();
    let sql = super::codec::insert_sql(TABLE, &COLUMNS);
    bind_fields(
        sqlx::query(&sql).bind(&c.id).bind(&c.project_id),
        c,
        stored_string(c),
    )
    .execute(&mut *conn)
    .await?;
    super::set_name_key(conn, TABLE, &c.id, &c.name).await?;
    replace_labels(conn, &c.id, &c.label_ids).await?;
    Ok(())
}

/// Writes every field of an existing connection and replaces its labels.
/// `project_id` isn't written: a connection never moves to another project.
/// The string is stored as [`insert`] stores it.
/// `false` when there's no connection with that id (nothing is written).
pub async fn update(tx: &mut WriteTx, c: &PersistedConnection) -> Result<bool> {
    let conn = tx.conn();
    let sets: Vec<String> = COLUMNS[2..].iter().map(|c| format!("{c} = ?")).collect();
    let sql = format!("UPDATE {TABLE} SET {} WHERE id = ?", sets.join(", "));
    let done = bind_fields(sqlx::query(&sql), c, stored_string(c))
        .bind(&c.id)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    super::set_name_key(conn, TABLE, &c.id, &c.name).await?;
    replace_labels(conn, &c.id, &c.label_ids).await?;
    Ok(true)
}

/// Deletes the connection. Its labels, history and AI chats cascade.
pub async fn remove(st: &Storage, connection_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM connections WHERE id = ?")
        .bind(connection_id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// [`remove`] inside a write transaction. `false` when there was no such
/// connection.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let done = sqlx::query("DELETE FROM connections WHERE id = ?")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// The ids and names of a project's connections, in rowid order.
pub async fn names_in_project(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<IdName>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, name FROM connections WHERE project_id = ? ORDER BY rowid")
            .bind(project_id)
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| IdName {
            id: id.unwrap_or_default(),
            name: name.unwrap_or_default(),
        })
        .collect())
}

/// The `name_key` lookup behind [`with_name_key`]: two searches of
/// `idx_connections_name_key` (`?1` the project, `?2` the key).
pub const NAME_KEY_LOOKUP: &str = "\
    SELECT rowid, id, name, name_key FROM connections WHERE project_id = ?1 AND name_key = ?2 \
    UNION ALL \
    SELECT rowid, id, name, name_key FROM connections WHERE project_id = ?1 AND name_key IS NULL \
    ORDER BY 1";

/// The ids and names of a project's connections whose name has `key`
/// (`seaquel_types::names::name_key`), in rowid order: Core's duplicate
/// check, an index search however many connections the project holds.
/// Rows with no stored key (an older release wrote or renamed them) are
/// read too and compared by their name.
pub async fn with_name_key(
    r: impl Into<Reader<'_>>,
    project_id: &str,
    key: &str,
) -> Result<Vec<IdName>> {
    let mut conn = r.into().conn().await?;
    let rows = sqlx::query_as(NAME_KEY_LOOKUP)
        .bind(project_id)
        .bind(key)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::matching_key(rows, key))
}

/// The ids of a project's connections, in rowid order.
pub async fn ids_in_project(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<String>> {
    let mut conn = r.into().conn().await?;
    ids_of_project(&mut conn, project_id).await
}

pub(crate) async fn ids_of_project(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT id FROM connections WHERE project_id = ? AND id IS NOT NULL ORDER BY rowid",
    )
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().filter_map(|(id,)| id).collect())
}

/// [`set_link`]'s statement: by primary key. A connection's link path is
/// its `shared_connection_id`.
pub const SET_LINK: &str = "UPDATE connections SET shared_connection_id = ?1, \
     shared_base = ?2, shared_file_id = ?3, \
     shared_origin = CASE WHEN ?1 IS NULL THEN NULL ELSE shared_origin END WHERE id = ?4";

/// Stores a connection's [`SharedLink`], and nothing else: `path` goes to
/// `shared_connection_id` as given (`<repoId>:<path>`, the form older
/// releases match an imported template by), `base` and `file_id` to the
/// columns of migration `0004`. `None` clears one; clearing `path` (the
/// link) clears the origin too (migration `0005`). `false` when there's no
/// connection with that id.
pub async fn set_link(tx: &mut WriteTx, id: &str, link: &SharedLink) -> Result<bool> {
    super::set_link(tx.conn(), SET_LINK, id, link).await
}

/// [`set_origin`]'s statement: by primary key.
pub const SET_ORIGIN: &str = "UPDATE connections SET shared_origin = ?1 WHERE id = ?2";

/// Records where a linked connection came from ([`ORIGIN_EXPORTED`] or
/// [`ORIGIN_IMPORTED`]; `None` clears it). `false` when there's no
/// connection with that id.
pub async fn set_origin(tx: &mut WriteTx, id: &str, origin: Option<&str>) -> Result<bool> {
    let done = sqlx::query(SET_ORIGIN)
        .bind(origin)
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Where a linked connection came from, or `None` (not known, no link, or
/// no such row).
pub async fn origin(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<String>> {
    let mut conn = r.into().conn().await?;
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT shared_origin FROM connections WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(v.flatten())
}

/// [`link`]'s query: by primary key.
pub const LINK: &str =
    "SELECT shared_connection_id, shared_base, shared_file_id FROM connections WHERE id = ?1";

/// One connection's [`SharedLink`] (`path` is `shared_connection_id`), or
/// `None` when there's no such row.
pub async fn link(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<SharedLink>> {
    let mut conn = r.into().conn().await?;
    super::link_of(&mut conn, LINK, id).await
}

/// A project's connections with their label ids, in rowid order, read
/// through `r` (inside a write, the transaction).
pub async fn list_in_project(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Vec<PersistedConnection>> {
    let mut conn = r.into().conn().await?;
    let rows = sqlx::query(&select_sql(TABLE, &READ_COLUMNS, "project_id = ?"))
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = text(row, "id")?;
        let labels = label_ids(&mut conn, &id).await?;
        out.push(map_row(row, id, labels)?);
    }
    Ok(out)
}

/// [`links`]' query: `idx_connections_project` (`?1` the project).
pub const LINKS: &str = "SELECT id, shared_connection_id, shared_base, shared_file_id \
     FROM connections WHERE project_id = ?1 ORDER BY rowid";

/// The link of every connection in a project, local-only or not, in rowid
/// order; `path` is `shared_connection_id`.
pub async fn links(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<RowLink>> {
    let mut conn = r.into().conn().await?;
    let rows = sqlx::query_as(LINKS)
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(super::row_links(rows))
}

/// How many connections the file holds.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM connections")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// A connection whose stored string still holds a secret: its id and
/// engine only, never the string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInString {
    pub id: String,
    pub ty: String,
}

/// The connections whose stored `connection_string` still holds a secret
/// ([`split_connection_string_secret`] finds one, movable or not),
/// in rowid order: the rows the one-time upgrade of phase 5d Decision 12a
/// moves to the keychain and strips. Rows written before phase 5a and not
/// saved since can hold one.
///
/// Rows with a NULL id, or a value that isn't UTF-8, are skipped.
pub async fn with_secret_in_string(r: impl Into<Reader<'_>>) -> Result<Vec<SecretInString>> {
    let mut conn = r.into().conn().await?;
    let rows = sqlx::query(
        "SELECT id, type, connection_string FROM connections \
         WHERE id IS NOT NULL AND typeof(connection_string) = 'text' \
         AND connection_string <> '' ORDER BY rowid",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let id = opt_text(row, "id").ok()??;
            let ty = text(row, "type").ok()?;
            let string = opt_text(row, "connection_string").ok()??;
            let secret = split_connection_string_secret(&string).is_some();
            secret.then_some(SecretInString { id, ty })
        })
        .collect())
}
