//! `tutorialRepo`: `tutorial_progress`.

use seaquel_types::storage::TutorialProgress;

use super::codec::{opt_text, text, Result};
use crate::Storage;

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
