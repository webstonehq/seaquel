//! `tutorialRepo`: `tutorial_progress`.

use seaquel_types::storage::TutorialProgress;

use super::codec::{opt_text, text, Result};
use crate::{Reader, Storage, WriteTx};

/// Every row, in rowid order (an overwritten pair moves to the end, since
/// `INSERT OR REPLACE` gives it a new rowid).
pub async fn load_all(st: &Storage) -> Result<Vec<TutorialProgress>> {
    let rows = sqlx::query(
        "SELECT lesson_id as lessonId, challenge_id as challengeId, state FROM tutorial_progress",
    )
    .fetch_all(st.pool())
    .await?;
    rows.iter()
        .map(|row| {
            Ok(TutorialProgress {
                lesson_id: text(row, "lessonId")?,
                challenge_id: text(row, "challengeId")?,
                state: opt_text(row, "state")?,
            })
        })
        .collect()
}

pub async fn save(
    st: &Storage,
    lesson_id: &str,
    challenge_id: &str,
    state: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO tutorial_progress (lesson_id, challenge_id, state) VALUES (?, ?, ?)")
        .bind(lesson_id)
        .bind(challenge_id)
        .bind(state)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes one lesson's rows.
pub async fn remove_lesson(st: &Storage, lesson_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM tutorial_progress WHERE lesson_id = ?")
        .bind(lesson_id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes every row.
pub async fn remove_all(st: &Storage) -> Result<()> {
    sqlx::query("DELETE FROM tutorial_progress")
        .execute(st.pool())
        .await?;
    Ok(())
}

/// [`load_all`] on the pool or inside a write, skipping a row with a
/// value that isn't UTF-8 (which [`load_all`] fails on). `state` stays
/// text; nothing here parses it.
pub async fn list(r: impl Into<Reader<'_>>) -> Result<Vec<TutorialProgress>> {
    let mut conn = r.into().conn().await?;
    type Raw = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
    let rows: Vec<Raw> = sqlx::query_as(
        "SELECT CAST(lesson_id AS BLOB), CAST(challenge_id AS BLOB), CAST(state AS BLOB) \
         FROM tutorial_progress ORDER BY rowid",
    )
    .fetch_all(&mut *conn)
    .await?;
    let utf8 = |b: Option<Vec<u8>>| -> Option<Option<String>> {
        match b {
            None => Some(None),
            Some(b) => String::from_utf8(b).ok().map(Some),
        }
    };
    Ok(rows
        .into_iter()
        .filter_map(|(lesson, challenge, state)| {
            Some(TutorialProgress {
                lesson_id: utf8(lesson)?.unwrap_or_default(),
                challenge_id: utf8(challenge)?.unwrap_or_default(),
                state: utf8(state)?,
            })
        })
        .collect())
}

/// [`save`] inside a write transaction.
pub async fn save_in(
    tx: &mut WriteTx,
    lesson_id: &str,
    challenge_id: &str,
    state: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO tutorial_progress (lesson_id, challenge_id, state) VALUES (?, ?, ?)")
        .bind(lesson_id)
        .bind(challenge_id)
        .bind(state)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// [`remove_lesson`] inside a write transaction; returns how many rows went.
pub async fn remove_lesson_in(tx: &mut WriteTx, lesson_id: &str) -> Result<u64> {
    let done = sqlx::query("DELETE FROM tutorial_progress WHERE lesson_id = ?")
        .bind(lesson_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}

/// [`remove_all`] inside a write transaction; returns how many rows went.
pub async fn remove_all_in(tx: &mut WriteTx) -> Result<u64> {
    let done = sqlx::query("DELETE FROM tutorial_progress")
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
