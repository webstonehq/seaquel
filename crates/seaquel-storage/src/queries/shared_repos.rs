//! `sharedReposRepo`: `shared_repos`, each repo stored as JSON, and the
//! active repo in `app_state['activeRepoId']`.

use seaquel_types::storage::SharedReposState;
use serde_json::value::RawValue;

use super::app_state;
use super::codec::{begin, bind_json_id, is_null, parse_json, Result};
use crate::Storage;

const ACTIVE_REPO_ID: &str = "activeRepoId";

/// Every stored repo, as its stored JSON, in rowid order. Rows that don't
/// parse or hold `null` are skipped.
pub async fn load_all(st: &Storage) -> Result<SharedReposState> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as("SELECT data FROM shared_repos")
        .fetch_all(st.pool())
        .await?;
    let repos = rows
        .into_iter()
        .filter_map(|(data,)| parse_json(&data?))
        .filter(|r| !is_null(r))
        .collect();
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
    sqlx::query("DELETE FROM shared_repos")
        .execute(&mut *tx)
        .await?;
    for repo in repos {
        let insert = sqlx::query("INSERT INTO shared_repos (id, data) VALUES (?, ?)");
        let insert = bind_json_id(insert, repo, None)?;
        insert.bind(repo.get()).execute(&mut *tx).await?;
    }
    app_state::set_with(&mut *tx, ACTIVE_REPO_ID, active_repo_id).await?;
    tx.commit().await?;
    Ok(())
}
