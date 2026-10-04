//! The column codecs every query module shares, ported from
//! `src/lib/storage/create-repo.ts` (`col`, `nullable`, `bool`, `optBool`,
//! `json`, `safeJsonParse`), plus the SQL builders `createRepo` made.
//!
//! Values are read the way better-sqlite3 handed them to the TypeScript:
//! columns by name, and flags compared with `=== 1`, so a stored 2 or `'1'`
//! is false.

use serde_json::value::RawValue;

use crate::db::{self, Cell, Row, SqliteRow};

use crate::{Storage, StorageError, WriteTx};

pub(crate) type Result<T> = std::result::Result<T, StorageError>;

/// A write transaction ([`Storage::write`]): this process's writers queue
/// on the storage's write mutex, and `BEGIN IMMEDIATE` takes SQLite's write
/// lock up front, so a writer in another process waits (busy_timeout)
/// instead of failing on upgrade.
pub(crate) async fn begin(st: &Storage) -> Result<WriteTx> {
    st.write().await
}

/// A value in a stored row that can't be read as the TypeScript read it
/// (`vault_state.kdf_params` that isn't JSON).
pub(crate) fn decode_error(msg: impl Into<String>) -> StorageError {
    StorageError::Sqlx(db::Error::Decode(msg.into().into()))
}

/// An argument storage can't bind, where the TypeScript's driver threw.
pub(crate) fn encode_error(msg: impl Into<String>) -> StorageError {
    StorageError::Sqlx(db::Error::Encode(msg.into().into()))
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// `col(...)` for a text column. SQLite converts a number to its text; a
/// NULL, which only a hand-edited file could hold here, reads as `""`.
pub(crate) fn text(row: &SqliteRow, col: &str) -> Result<String> {
    Ok(opt_text(row, col)?.unwrap_or_default())
}

/// `nullable(...)` for a text column: NULL is `None`.
pub(crate) fn opt_text(row: &SqliteRow, col: &str) -> Result<Option<String>> {
    Ok(row.try_get_unchecked::<Option<String>, _>(col)?)
}

/// `col(...)` for a number column.
pub(crate) fn number(row: &SqliteRow, col: &str) -> Result<f64> {
    Ok(opt_number(row, col)?.unwrap_or_default())
}

/// `nullable(...)` for a number column.
pub(crate) fn opt_number(row: &SqliteRow, col: &str) -> Result<Option<f64>> {
    Ok(row.try_get_unchecked::<Option<f64>, _>(col)?)
}

/// `value === 1`: only an INTEGER 1 (or REAL 1.0) is true.
fn is_one(v: Cell) -> bool {
    match v {
        Cell::Integer(n) => n == 1,
        Cell::Real(n) => n == 1.0,
        Cell::Null | Cell::Other => false,
    }
}

/// `bool(...)`: `value === 1`, never absent.
pub(crate) fn flag(row: &SqliteRow, col: &str) -> Result<bool> {
    Ok(is_one(db::cell(row, col)?))
}

/// `optBool(...)`: NULL is `None`, anything else `value === 1`.
pub(crate) fn opt_flag(row: &SqliteRow, col: &str) -> Result<Option<bool>> {
    Ok(match db::cell(row, col)? {
        Cell::Null => None,
        v => Some(is_one(v)),
    })
}

/// `JSON.parse(text)`, keeping the JSON as text. `None` when it doesn't
/// parse.
pub(crate) fn parse_json(text: &str) -> Option<Box<RawValue>> {
    serde_json::from_str::<Box<RawValue>>(text).ok()
}

/// Stored JSON read as bytes (`CAST(col AS BLOB)`), as today's loads read
/// it: `None` for NULL, text that isn't UTF-8, text that doesn't parse, and
/// JSON `null`. So a row a user can't fix is skipped, never a failed read.
pub(crate) fn stored_json(bytes: Option<Vec<u8>>) -> Option<Box<RawValue>> {
    let text = String::from_utf8(bytes?).ok()?;
    parse_json(&text).filter(|v| !is_null(v))
}

/// A literal JSON value (a fallback such as `[]`).
pub(crate) fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_string()).expect("literal JSON")
}

/// Whether a raw value is JSON `null`.
pub(crate) fn is_null(v: &RawValue) -> bool {
    v.get() == "null"
}

/// `safeJsonParse(value, undefined)`, and `json(col, undefined)`: NULL, an
/// empty string and text that isn't JSON are `None`. Stored `null` is
/// `Some(null)`, which loads as `null`, not as absent.
pub(crate) fn json(row: &SqliteRow, col: &str) -> Result<Option<Box<RawValue>>> {
    Ok(opt_text(row, col)?
        .filter(|t| !t.is_empty())
        .and_then(|t| parse_json(&t)))
}

/// `safeJsonParse(value, fallback)` with a JSON fallback such as `[]`.
pub(crate) fn json_or(row: &SqliteRow, col: &str, fallback: &str) -> Result<Box<RawValue>> {
    Ok(json(row, col)?.unwrap_or_else(|| raw(fallback)))
}

/// `JSON.stringify(value)` for a nullable JSON column: `None` is NULL.
pub(crate) fn json_text(v: &Option<Box<RawValue>>) -> Option<&str> {
    v.as_deref().map(RawValue::get)
}

/// `value ? JSON.stringify(value) : null`: JavaScript's falsy values
/// (`null`, `false`, `0`, `""`) are NULL.
pub(crate) fn truthy_json_text(v: &Option<Box<RawValue>>) -> Option<&str> {
    json_text(v).filter(|t| {
        let falsy = matches!(*t, "null" | "false" | "\"\"")
            || t.parse::<f64>().is_ok_and(|n| n == 0.0 || n.is_nan());
        !falsy
    })
}

/// `0/1` for a flag.
pub(crate) fn bit(b: bool) -> i64 {
    i64::from(b)
}

/// `optBool` on write: `None` is NULL.
pub(crate) fn opt_bit(b: Option<bool>) -> Option<i64> {
    b.map(bit)
}

// ---------------------------------------------------------------------------
// Ids taken out of stored JSON
// ---------------------------------------------------------------------------

/// A JSON value's `id` as better-sqlite3 would have bound `value.id`.
enum JsonId {
    Text(String),
    Number(f64),
    Missing,
}

fn json_id(v: &RawValue) -> Result<JsonId> {
    #[derive(serde::Deserialize)]
    struct WithId {
        id: Option<serde_json::Value>,
    }
    let id = match serde_json::from_str::<serde_json::Value>(v.get()) {
        Ok(serde_json::Value::Object(_)) => serde_json::from_str::<WithId>(v.get())
            .map_err(|e| encode_error(e.to_string()))?
            .id
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    };
    match id {
        serde_json::Value::Null => Ok(JsonId::Missing),
        serde_json::Value::String(s) => Ok(JsonId::Text(s)),
        serde_json::Value::Number(n) => Ok(JsonId::Number(n.as_f64().unwrap_or(f64::NAN))),
        other => Err(encode_error(format!("an id must be a string, not {other}"))),
    }
}

pub(crate) use crate::db::SqliteQuery;

/// Binds the `id` of a JSON value stored whole (a user theme, a shared repo,
/// a saved workflow), the way the TypeScript bound `value.id`: a string as
/// text, a number as a number (INTEGER when it's whole), and a missing or
/// `null` id (or a value that isn't an object) as `missing`. Any other id
/// fails, as it did in the TypeScript.
pub(crate) fn bind_json_id<'q>(
    query: SqliteQuery<'q>,
    value: &RawValue,
    missing: Option<String>,
) -> Result<SqliteQuery<'q>> {
    Ok(match json_id(value)? {
        JsonId::Text(id) => query.bind(id),
        JsonId::Number(id) if id.fract() == 0.0 && id.abs() < 9_007_199_254_740_992.0 => {
            query.bind(id as i64)
        }
        JsonId::Number(id) => query.bind(id),
        JsonId::Missing => query.bind(missing),
    })
}

// ---------------------------------------------------------------------------
// Singleton JSON rows (license_state, onboarding_state)
// ---------------------------------------------------------------------------

/// The JSON in `table`'s one row (`id = 1`). `None` (JSON `null`) when
/// there's no row or the stored text doesn't parse; stored `null` loads as
/// `null` too.
pub(crate) async fn load_singleton_json(
    st: &Storage,
    table: &str,
) -> Result<Option<Box<RawValue>>> {
    let row: Option<(Option<String>,)> =
        db::query_as(&format!("SELECT data FROM {table} WHERE id = 1"))
            .fetch_optional(st.pool())
            .await?;
    Ok(row.and_then(|(data,)| parse_json(&data?)))
}

/// Stores `data` in `table`'s one row as the JSON given (`null` is stored
/// as `'null'`).
pub(crate) async fn save_singleton_json(st: &Storage, table: &str, data: &RawValue) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query(&format!(
        "INSERT OR REPLACE INTO {table} (id, data) VALUES (1, ?)"
    ))
    .bind(data.get())
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SQL builders (createRepo)
// ---------------------------------------------------------------------------

/// `INSERT INTO table (cols) VALUES (?, …)`.
pub(crate) fn insert_sql(table: &str, cols: &[&str]) -> String {
    let placeholders = vec!["?"; cols.len()].join(", ");
    format!(
        "INSERT INTO {table} ({}) VALUES ({placeholders})",
        cols.join(", ")
    )
}

/// `INSERT … ON CONFLICT(id) DO UPDATE SET` every other column.
pub(crate) fn upsert_sql(table: &str, cols: &[&str], id: &str) -> String {
    let updates: Vec<String> = cols
        .iter()
        .filter(|c| **c != id)
        .map(|c| format!("{c} = excluded.{c}"))
        .collect();
    format!(
        "{} ON CONFLICT({id}) DO UPDATE SET {}",
        insert_sql(table, cols),
        updates.join(", ")
    )
}

/// `SELECT cols FROM table`, with an optional `WHERE`.
pub(crate) fn select_sql(table: &str, cols: &[&str], filter: &str) -> String {
    let base = format!("SELECT {} FROM {table}", cols.join(", "));
    if filter.is_empty() {
        base
    } else {
        format!("{base} WHERE {filter}")
    }
}
