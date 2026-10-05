//! `sharedReposRepo`: `shared_repos`, each repo stored as JSON, and the
//! active repo in `app_state['activeRepoId']`.
//!
//! The targeted functions (phase 5e) read and
//! write one repo at a time inside Core's write transaction, and keep each
//! row's JSON as stored: [`update_json`] changes only the fields it names,
//! so older releases read every other field as they wrote it. They never
//! touch `activeRepoId`, which stays as an older release last wrote it.
//! [`load_all`] and [`save_all`] stay only for the frozen
//! `shared-repos.json` fixture.

use crate::db;
use std::collections::HashMap;

use seaquel_types::storage::SharedReposState;
use serde_json::value::RawValue;

use super::app_state;
use super::codec::{begin, bind_json_id, decode_error, encode_error, is_null, parse_json, Result};
use crate::{Reader, Storage, WriteTx};

const ACTIVE_REPO_ID: &str = "activeRepoId";

/// Every stored repo, as its stored JSON, in rowid order. Rows that don't
/// parse or hold `null` are skipped.
pub async fn load_all(st: &Storage) -> Result<SharedReposState> {
    let repos = list(st).await?;
    let active_repo_id = app_state::get(st, ACTIVE_REPO_ID).await?;
    Ok(SharedReposState {
        repos,
        active_repo_id,
    })
}

/// Replaces every repo with `repos` (stored as the JSON given, under its
/// `id`) and sets the active repo, in one transaction. `None` keeps an
/// `activeRepoId` row whose value is NULL.
pub async fn save_all(
    st: &Storage,
    repos: &[Box<RawValue>],
    active_repo_id: Option<&str>,
) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM shared_repos")
        .execute(&mut *tx)
        .await?;
    for repo in repos {
        let insert = db::query("INSERT INTO shared_repos (id, data) VALUES (?, ?)");
        let insert = bind_json_id(insert, repo, None)?;
        insert.bind(repo.get()).execute(&mut *tx).await?;
    }
    app_state::set_with(&mut *tx, ACTIVE_REPO_ID, active_repo_id).await?;
    tx.commit().await?;
    Ok(())
}

/// The stored rows, `(id, data)`, in rowid order. `data` as bytes, so a
/// row that isn't UTF-8 is skipped rather than failing the read.
async fn rows(r: impl Into<Reader<'_>>) -> Result<Vec<(Option<String>, Option<Vec<u8>>)>> {
    let mut conn = r.into().conn().await?;
    Ok(
        db::query_as("SELECT id, CAST(data AS BLOB) FROM shared_repos ORDER BY rowid")
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// A stored `data` as JSON, or `None` when it isn't UTF-8, doesn't parse
/// or is `null`.
fn stored(data: Option<Vec<u8>>) -> Option<Box<RawValue>> {
    let text = String::from_utf8(data?).ok()?;
    parse_json(&text).filter(|v| !is_null(v))
}

/// Every stored repo, as its stored JSON, in rowid order: what
/// [`load_all`] gives, on the pool or inside a write. Rows that don't
/// parse or hold `null` are skipped.
pub async fn list(r: impl Into<Reader<'_>>) -> Result<Vec<Box<RawValue>>> {
    Ok(rows(r)
        .await?
        .into_iter()
        .filter_map(|(_, data)| stored(data))
        .collect())
}

/// One repo's stored JSON by its id, or `None` (also for a row that
/// [`list`] skips).
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<Box<RawValue>>> {
    let mut conn = r.into().conn().await?;
    let row: Option<(Option<Vec<u8>>,)> =
        db::query_as("SELECT CAST(data AS BLOB) FROM shared_repos WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row.and_then(|(data,)| stored(data)))
}

/// The first repo, in rowid order, whose JSON `path` is `path` (compared
/// exactly: normalising a path is the caller's). A repeated `path` key is
/// read as `JSON.parse` reads it: the last one wins. Core registers a repo
/// by path with this and [`insert`] in one write, so one path has one repo.
/// The list is a handful of rows, so it is read whole.
pub async fn get_by_path(r: impl Into<Reader<'_>>, path: &str) -> Result<Option<Box<RawValue>>> {
    Ok(list(r).await?.into_iter().find(|repo| {
        serde_json::from_str::<HashMap<String, &RawValue>>(repo.get())
            .ok()
            .and_then(|obj| obj.get("path").copied())
            .and_then(|p| serde_json::from_str::<String>(p.get()).ok())
            .is_some_and(|p| p == path)
    }))
}

/// Inserts a repo, stored as the JSON given under its `id`. The JSON must
/// be an object with a non-empty string `id`; an id that exists fails (the
/// primary key) rather than overwriting.
pub async fn insert(tx: &mut WriteTx, repo: &RawValue) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct WithId {
        id: Option<serde_json::Value>,
    }
    let id = match serde_json::from_str::<serde_json::Value>(repo.get()) {
        Ok(serde_json::Value::Object(_)) => serde_json::from_str::<WithId>(repo.get())
            .ok()
            .and_then(|w| w.id),
        _ => None,
    };
    let Some(serde_json::Value::String(id)) = id.filter(|id| id.as_str() != Some("")) else {
        return Err(encode_error(
            "a shared repo must be an object with a string id",
        ));
    };
    db::query("INSERT INTO shared_repos (id, data) VALUES (?, ?)")
        .bind(id)
        .bind(repo.get())
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// Sets `fields` in the stored JSON of repo `id` and keeps every other
/// byte of it: a named field already there has its value
/// replaced in place (the last one, if a hand edit repeated the key, since
/// that's the one `JSON.parse` reads); one that isn't is added before the
/// closing brace, in the order given. A field named twice takes its last
/// value. `false` when there's no repo with that id; a stored value that
/// isn't a JSON object is an error, and nothing is written: that includes
/// a row stored as `null`, which [`get`] and [`list`] treat as absent. So
/// does one the parser refuses, such as a key holding a lone surrogate
/// escape (`"\ud800"`) or nesting deeper than serde_json's 128 levels.
///
/// `id` can't be set (an encode error, nothing written): the JSON's id
/// must stay the row's.
pub async fn update_json(tx: &mut WriteTx, id: &str, fields: &[(&str, &RawValue)]) -> Result<bool> {
    if fields.iter().any(|(k, _)| *k == "id") {
        return Err(encode_error("a shared repo's id can't be changed"));
    }
    let conn = tx.conn();
    let row: Option<(Option<Vec<u8>>,)> =
        db::query_as("SELECT CAST(data AS BLOB) FROM shared_repos WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((data,)) = row else {
        return Ok(false);
    };
    let text = data
        .and_then(|d| String::from_utf8(d).ok())
        .ok_or_else(|| decode_error("a stored shared repo isn't text"))?;
    let patched = patch_object(&text, fields)
        .ok_or_else(|| decode_error("a stored shared repo isn't an object"))?;
    if patched != text {
        db::query("UPDATE shared_repos SET data = ? WHERE id = ?")
            .bind(&patched)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(true)
}

/// Deletes one repo. `false` when there was no such repo.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let done = db::query("DELETE FROM shared_repos WHERE id = ?")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// `text` (a JSON object) with `fields` set, every other byte kept, or
/// `None` when `text` isn't a JSON object serde_json reads: `null`, a key
/// with a lone surrogate escape and nesting past 128 levels are all `None`.
///
/// The stored values are read as borrowed [`RawValue`]s, which are slices
/// of `text`, so each one's place is known exactly and only those bytes
/// are replaced.
fn patch_object(text: &str, fields: &[(&str, &RawValue)]) -> Option<String> {
    // A repeated key keeps its last value, as `JSON.parse` does.
    let parsed: HashMap<String, &RawValue> = serde_json::from_str(text).ok()?;
    let base = text.as_ptr() as usize;
    let span = |v: &RawValue| -> Option<(usize, usize)> {
        let start = (v.get().as_ptr() as usize).checked_sub(base)?;
        let end = start.checked_add(v.get().len())?;
        (text.get(start..end) == Some(v.get())).then_some((start, end))
    };

    // The last value given for each field, in first-given order.
    let mut wanted: Vec<(&str, &RawValue)> = Vec::new();
    for (k, v) in fields {
        match wanted.iter_mut().find(|(w, _)| w == k) {
            Some(slot) => slot.1 = v,
            None => wanted.push((k, v)),
        }
    }

    let mut replace: Vec<(usize, usize, &str)> = Vec::new();
    let mut append = String::new();
    let mut members = parsed.len();
    for (k, v) in wanted {
        match parsed.get(k) {
            Some(old) => {
                let (start, end) = span(old)?;
                replace.push((start, end, v.get()));
            }
            None => {
                if members > 0 {
                    append.push(',');
                }
                append.push_str(&serde_json::to_string(k).ok()?);
                append.push(':');
                append.push_str(v.get());
                members += 1;
            }
        }
    }

    // The closing brace: the object's last non-whitespace byte.
    let close = text.trim_end().len().checked_sub(1)?;
    if text.as_bytes().get(close) != Some(&b'}') {
        return None;
    }
    replace.push((close, close, &append));
    replace.sort_by_key(|r| std::cmp::Reverse(r.0));
    let mut out = text.to_string();
    for (start, end, with) in replace {
        out.replace_range(start..end, with);
    }
    Some(out)
}
