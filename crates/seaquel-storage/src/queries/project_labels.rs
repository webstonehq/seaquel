//! `project_labels`: a project's custom connection labels, one row each.
//! The three predefined labels aren't stored; their
//! ids appear only in `connection_labels`.
//!
//! `connection_labels` has no foreign key to this table, so removing a label
//! strips it from every connection separately ([`strip_from_connections`]),
//! in the same transaction.

use crate::db;
use db::SqliteConnection;
use seaquel_types::storage::ConnectionLabel;

use super::codec::{bit, flag, text, Result};
use crate::{Reader, WriteTx};

/// A project's labels, in the order `projects::load_all` gives them.
pub async fn list(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Vec<ConnectionLabel>> {
    let mut conn = r.into().conn().await?;
    of_project(&mut conn, project_id).await
}

pub(crate) async fn of_project(
    conn: &mut SqliteConnection,
    project_id: &str,
) -> Result<Vec<ConnectionLabel>> {
    let rows =
        db::query("SELECT id, name, is_predefined, color FROM project_labels WHERE project_id = ?")
            .bind(project_id)
            .fetch_all(&mut *conn)
            .await?;
    rows.iter()
        .map(|l| {
            Ok(ConnectionLabel {
                id: text(l, "id")?,
                name: text(l, "name")?,
                is_predefined: flag(l, "is_predefined")?,
                color: text(l, "color")?,
            })
        })
        .collect()
}

/// Adds a label to the project. A label id is unique across projects, so
/// one that exists anywhere fails.
pub async fn insert(tx: &mut WriteTx, project_id: &str, label: &ConnectionLabel) -> Result<()> {
    insert_row(tx.conn(), project_id, label).await
}

pub(crate) async fn insert_row(
    conn: &mut SqliteConnection,
    project_id: &str,
    label: &ConnectionLabel,
) -> Result<()> {
    db::query(
        "INSERT INTO project_labels (id, project_id, name, is_predefined, color) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&label.id)
    .bind(project_id)
    .bind(&label.name)
    .bind(bit(label.is_predefined))
    .bind(&label.color)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Makes `labels` the project's labels: the replace-all of
/// `projects::save`/`save_all`, which the frozen repo fixtures still pin.
pub(crate) async fn replace_all(
    conn: &mut SqliteConnection,
    project_id: &str,
    labels: &[ConnectionLabel],
) -> Result<()> {
    db::query("DELETE FROM project_labels WHERE project_id = ?")
        .bind(project_id)
        .execute(&mut *conn)
        .await?;
    for label in labels {
        insert_row(conn, project_id, label).await?;
    }
    Ok(())
}

/// Renames and recolours one of the project's labels. `false` when the
/// project has no label with that id (another project's is left alone).
pub async fn update(tx: &mut WriteTx, project_id: &str, label: &ConnectionLabel) -> Result<bool> {
    let done =
        db::query("UPDATE project_labels SET name = ?, color = ? WHERE id = ? AND project_id = ?")
            .bind(&label.name)
            .bind(&label.color)
            .bind(&label.id)
            .bind(project_id)
            .execute(tx.conn())
            .await?;
    Ok(done.rows_affected() > 0)
}

/// Deletes one of the project's labels. `false` when the project has no
/// label with that id. It doesn't touch connections: see
/// [`strip_from_connections`].
pub async fn delete(tx: &mut WriteTx, project_id: &str, label_id: &str) -> Result<bool> {
    let done = db::query("DELETE FROM project_labels WHERE id = ? AND project_id = ?")
        .bind(label_id)
        .bind(project_id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Takes `label_id` off every connection that has it, in any project (phase
/// No row may point at a removed label), and returns their
/// ids in rowid order. Custom label ids are unique across projects, so only
/// the removed label's rows go.
pub async fn strip_from_connections(tx: &mut WriteTx, label_id: &str) -> Result<Vec<String>> {
    let conn = tx.conn();
    let had: Vec<(String,)> = db::query_as(
        "SELECT c.id FROM connections c \
         JOIN connection_labels l ON l.connection_id = c.id \
         WHERE l.label_id = ? ORDER BY c.rowid",
    )
    .bind(label_id)
    .fetch_all(&mut *conn)
    .await?;
    db::query("DELETE FROM connection_labels WHERE label_id = ?")
        .bind(label_id)
        .execute(&mut *conn)
        .await?;
    Ok(had.into_iter().map(|(id,)| id).collect())
}
