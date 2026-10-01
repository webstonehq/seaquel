//! `projectStateRepo`: a project's `project_state` row, its `tabs` and its
//! `saved_canvases` (saved workflows).
//!
//! The quirks it keeps (fixtures README, quirk 10): schema and data tabs
//! store `tableName` in `name` too; workflow tabs are `tab_type = 'canvas'`;
//! a dashboard tab keeps its dashboard id, and a table-editor tab its
//! definition, in `source_query`; `active_visualize_tab_id` is always
//! written NULL; tabs of other types load as nothing.
//!
//! From phase 5d-2 (Decision 22) a window's view state lives in
//! `window_state`, and every view-state save writes this module's rows as a
//! legacy mirror ([`write_legacy_mirror`]) so older releases see the most
//! recently saved window's tabs. The connection order stays here, shared
//! by the project's windows ([`sidebar`], [`set_connection_order`]).
//! [`save`] and [`remove`] stay for their frozen fixtures.

use seaquel_types::storage::{
    PersistedCreateTableTab, PersistedDashboardTab, PersistedDataTab, PersistedErdTab,
    PersistedExplainTab, PersistedProjectState, PersistedQueryTab, PersistedSchemaTab,
    PersistedStarterTab, PersistedStatisticsTab, PersistedWorkflowTab,
};
use serde_json::value::RawValue;
use sqlx::sqlite::SqliteRow;

use std::collections::HashSet;

use sqlx::SqliteConnection;

use super::codec::{
    begin, bind_json_id, encode_error, flag, is_null, json, json_or, opt_text, parse_json, text,
    truthy_json_text, Result,
};
use crate::{Reader, Storage, WriteTx};

/// One `tabs` row, with the columns every tab type reads.
struct Tab {
    id: String,
    tab_type: String,
    name: String,
    query: Option<String>,
    saved_query_id: Option<String>,
    shared_query_id: Option<String>,
    table_name: Option<String>,
    schema_name: Option<String>,
    source_query: Option<String>,
    connection_id: Option<String>,
    starter_type: Option<String>,
    closable: bool,
}

fn tab(row: &SqliteRow) -> Result<Tab> {
    Ok(Tab {
        id: text(row, "id")?,
        tab_type: text(row, "tab_type")?,
        name: text(row, "name")?,
        query: opt_text(row, "query")?,
        saved_query_id: opt_text(row, "saved_query_id")?,
        shared_query_id: opt_text(row, "shared_query_id")?,
        table_name: opt_text(row, "table_name")?,
        schema_name: opt_text(row, "schema_name")?,
        source_query: opt_text(row, "source_query")?,
        connection_id: opt_text(row, "connection_id")?,
        starter_type: opt_text(row, "starter_type")?,
        closable: flag(row, "closable")?,
    })
}

fn of_type<'a>(tabs: &'a [Tab], tab_type: &'a str) -> impl Iterator<Item = &'a Tab> {
    tabs.iter().filter(move |t| t.tab_type == tab_type)
}

fn or_empty(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

/// The project's state, or `None` when it has no `project_state` row.
///
/// Saved workflows come back as their stored JSON, in rowid order, without
/// the rows that don't parse or hold `null`.
///
/// It reads on the pool (`&storage`) or inside a write (`&mut tx`): Core's
/// first load of a project in a window with no view state falls back to it
/// (Decision 22).
pub async fn load(
    r: impl Into<Reader<'_>>,
    project_id: &str,
) -> Result<Option<PersistedProjectState>> {
    let mut conn = r.into().conn().await?;
    let Some(state) = sqlx::query("SELECT * FROM project_state WHERE project_id = ?")
        .bind(project_id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };

    let tabs = sqlx::query("SELECT * FROM tabs WHERE project_id = ?")
        .bind(project_id)
        .fetch_all(&mut *conn)
        .await?;
    let tabs = tabs.iter().map(tab).collect::<Result<Vec<_>>>()?;

    let canvases: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT data FROM saved_canvases WHERE project_id = ?")
            .bind(project_id)
            .fetch_all(&mut *conn)
            .await?;
    let saved_workflows: Vec<Box<RawValue>> = canvases
        .into_iter()
        .filter_map(|(data,)| parse_json(&data?))
        .filter(|w| !is_null(w))
        .collect();

    // `safeJsonParse(state.connection_order ?? "[]", [])`
    let connection_order = Some(json_or(&state, "connection_order", "[]")?);
    // `if (pane_layout) paneLayout = safeJsonParse(pane_layout, undefined)`
    let pane_layout = json(&state, "pane_layout")?;

    Ok(Some(PersistedProjectState {
        project_id: project_id.to_string(),
        query_tabs: of_type(&tabs, "query")
            .map(|t| PersistedQueryTab {
                id: t.id.clone(),
                name: t.name.clone(),
                query: or_empty(&t.query),
                query_id: t
                    .saved_query_id
                    .clone()
                    .or_else(|| t.shared_query_id.clone()),
            })
            .collect(),
        schema_tabs: of_type(&tabs, "schema")
            .map(|t| PersistedSchemaTab {
                id: t.id.clone(),
                table_name: or_empty(&t.table_name),
                schema_name: or_empty(&t.schema_name),
                connection_id: t.connection_id.clone(),
            })
            .collect(),
        explain_tabs: of_type(&tabs, "explain")
            .map(|t| PersistedExplainTab {
                id: t.id.clone(),
                name: t.name.clone(),
                source_query: or_empty(&t.source_query),
            })
            .collect(),
        erd_tabs: of_type(&tabs, "erd")
            .map(|t| PersistedErdTab {
                id: t.id.clone(),
                name: t.name.clone(),
                connection_id: t.connection_id.clone(),
            })
            .collect(),
        statistics_tabs: of_type(&tabs, "statistics")
            .map(|t| PersistedStatisticsTab {
                id: t.id.clone(),
                name: t.name.clone(),
                connection_id: or_empty(&t.connection_id),
            })
            .collect(),
        workflow_tabs: of_type(&tabs, "canvas")
            .map(|t| PersistedWorkflowTab {
                id: t.id.clone(),
                name: t.name.clone(),
                connection_id: or_empty(&t.connection_id),
            })
            .collect(),
        tab_order: json_or(&state, "tab_order", "[]")?,
        connection_order,
        active_query_tab_id: opt_text(&state, "active_query_tab_id")?,
        active_schema_tab_id: opt_text(&state, "active_schema_tab_id")?,
        active_explain_tab_id: opt_text(&state, "active_explain_tab_id")?,
        active_erd_tab_id: opt_text(&state, "active_erd_tab_id")?,
        active_statistics_tab_id: opt_text(&state, "active_statistics_tab_id")?,
        active_workflow_tab_id: opt_text(&state, "active_workflow_tab_id")?,
        active_view: text(&state, "active_view")?,
        active_connection_id: opt_text(&state, "active_connection_id")?,
        starter_tabs: of_type(&tabs, "starter")
            .map(|t| PersistedStarterTab {
                id: t.id.clone(),
                ty: t
                    .starter_type
                    .clone()
                    .unwrap_or_else(|| "getting-started".to_string()),
                name: t.name.clone(),
                closable: t.closable,
            })
            .collect(),
        active_starter_tab_id: opt_text(&state, "active_starter_tab_id")?,
        saved_workflows,
        connection_tabs: None,
        active_connection_tab_id: None,
        dashboard_tabs: of_type(&tabs, "dashboard")
            .map(|t| PersistedDashboardTab {
                id: t.id.clone(),
                name: t.name.clone(),
                dashboard_id: or_empty(&t.source_query),
            })
            .collect(),
        active_dashboard_tab_id: opt_text(&state, "active_dashboard_tab_id")?,
        starred_shared_query_ids: Some(json_or(&state, "starred_shared_query_ids", "[]")?),
        starred_shared_dashboard_ids: Some(json_or(&state, "starred_shared_dashboard_ids", "[]")?),
        create_table_tabs: of_type(&tabs, "create_table")
            .map(|t| PersistedCreateTableTab {
                id: t.id.clone(),
                connection_id: or_empty(&t.connection_id),
                name: t.name.clone(),
                table_definition: t.source_query.clone().unwrap_or_else(|| "{}".to_string()),
            })
            .collect(),
        active_create_table_tab_id: opt_text(&state, "active_create_table_tab_id")?,
        data_tabs: of_type(&tabs, "data")
            .map(|t| PersistedDataTab {
                id: t.id.clone(),
                connection_id: or_empty(&t.connection_id),
                table_name: or_empty(&t.table_name),
                schema_name: or_empty(&t.schema_name),
            })
            .collect(),
        active_data_tab_id: opt_text(&state, "active_data_tab_id")?,
        extensions_duckdb_tabs: None,
        active_extensions_duckdb_tab_id: None,
        pane_layout,
    }))
}

/// `JSON.stringify(value ?? [])`.
fn json_or_empty_list(v: &Option<Box<RawValue>>) -> String {
    v.as_deref().map_or("[]", RawValue::get).to_string()
}

/// Replaces the project's state row (`INSERT OR REPLACE`), all its tabs and
/// all its saved workflows, in one transaction. A saved workflow without an
/// `id` is stored as `workflow-<random uuid>`.
pub async fn save(st: &Storage, s: &PersistedProjectState) -> Result<()> {
    let pid = &s.project_id;
    let mut tx = begin(st).await?;

    sqlx::query(
        "INSERT OR REPLACE INTO project_state \
         (project_id, active_view, active_connection_id, active_query_tab_id, active_schema_tab_id, \
          active_explain_tab_id, active_erd_tab_id, active_statistics_tab_id, active_workflow_tab_id, \
          active_visualize_tab_id, active_starter_tab_id, tab_order, connection_order, \
          active_dashboard_tab_id, starred_shared_query_ids, starred_shared_dashboard_ids, pane_layout, \
          active_create_table_tab_id, active_data_tab_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(pid)
    .bind(&s.active_view)
    .bind(&s.active_connection_id)
    .bind(&s.active_query_tab_id)
    .bind(&s.active_schema_tab_id)
    .bind(&s.active_explain_tab_id)
    .bind(&s.active_erd_tab_id)
    .bind(&s.active_statistics_tab_id)
    .bind(&s.active_workflow_tab_id)
    .bind(None::<String>) // active_visualize_tab_id
    .bind(&s.active_starter_tab_id)
    .bind(s.tab_order.get())
    .bind(json_or_empty_list(&s.connection_order))
    .bind(&s.active_dashboard_tab_id)
    .bind(json_or_empty_list(&s.starred_shared_query_ids))
    .bind(json_or_empty_list(&s.starred_shared_dashboard_ids))
    .bind(truthy_json_text(&s.pane_layout))
    .bind(&s.active_create_table_tab_id)
    .bind(&s.active_data_tab_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM tabs WHERE project_id = ?")
        .bind(pid)
        .execute(&mut *tx)
        .await?;
    insert_tabs(&mut tx, s, false).await?;

    sqlx::query("DELETE FROM saved_canvases WHERE project_id = ?")
        .bind(pid)
        .execute(&mut *tx)
        .await?;
    for workflow in &s.saved_workflows {
        let insert =
            sqlx::query("INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)");
        // `workflow.id ?? "workflow-" + crypto.randomUUID()`
        let random = format!("workflow-{}", uuid::Uuid::new_v4());
        let insert = bind_json_id(insert, workflow, Some(random))?;
        insert
            .bind(pid)
            .bind(workflow.get())
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// The project's connection order, as its stored JSON (`[]` for NULL or
/// text that isn't JSON, as [`load`] reads it), or `None` when the project
/// has no `project_state` row. Shared by the project's windows: it's how
/// the sidebar looks, not what a window has open (Decision 22).
pub async fn sidebar(r: impl Into<Reader<'_>>, project_id: &str) -> Result<Option<Box<RawValue>>> {
    let mut conn = r.into().conn().await?;
    let row = sqlx::query("SELECT connection_order FROM project_state WHERE project_id = ?")
        .bind(project_id)
        .fetch_optional(&mut *conn)
        .await?;
    row.map(|row| json_or(&row, "connection_order", "[]"))
        .transpose()
}

/// Sets the project's connection order, adding its `project_state` row
/// (with the columns' defaults) if it has none. Nothing else in the row
/// changes.
pub async fn set_connection_order(
    tx: &mut WriteTx,
    project_id: &str,
    connection_order: &[String],
) -> Result<()> {
    let order = serde_json::to_string(connection_order).map_err(|e| encode_error(e.to_string()))?;
    sqlx::query(
        "INSERT INTO project_state (project_id, connection_order) VALUES (?, ?) \
         ON CONFLICT(project_id) DO UPDATE SET connection_order = excluded.connection_order",
    )
    .bind(project_id)
    .bind(order)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The legacy mirror of one window's view-state save (Decision 22): the
/// `project_state` row and the `tabs` rows today's [`save`] writes for
/// `s`, so an older release opening the file sees the most recently saved
/// window's tabs. Unlike [`save`]:
/// - the stored `connection_order` and starred-shared lists are kept (a
///   project with no row gets the columns' defaults); `s`'s are ignored,
///   since a window's view state doesn't carry them;
/// - `saved_canvases` is never touched (`s.saved_workflows` is ignored);
/// - a tab whose id an earlier tab took is skipped instead of failing the
///   save (`tabs`' key is `(id, project_id)`), and the count of skipped
///   tabs is returned.
///
/// `active_connection_id` is `s`'s, the saving window's (Q14). DuckDB
/// extensions tabs have no column and aren't mirrored.
pub async fn write_legacy_mirror(tx: &mut WriteTx, s: &PersistedProjectState) -> Result<u32> {
    let conn = tx.conn();
    sqlx::query(
        "INSERT INTO project_state \
         (project_id, active_view, active_connection_id, active_query_tab_id, active_schema_tab_id, \
          active_explain_tab_id, active_erd_tab_id, active_statistics_tab_id, active_workflow_tab_id, \
          active_visualize_tab_id, active_starter_tab_id, tab_order, active_dashboard_tab_id, \
          pane_layout, active_create_table_tab_id, active_data_tab_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(project_id) DO UPDATE SET \
           active_view = excluded.active_view, \
           active_connection_id = excluded.active_connection_id, \
           active_query_tab_id = excluded.active_query_tab_id, \
           active_schema_tab_id = excluded.active_schema_tab_id, \
           active_explain_tab_id = excluded.active_explain_tab_id, \
           active_erd_tab_id = excluded.active_erd_tab_id, \
           active_statistics_tab_id = excluded.active_statistics_tab_id, \
           active_workflow_tab_id = excluded.active_workflow_tab_id, \
           active_visualize_tab_id = excluded.active_visualize_tab_id, \
           active_starter_tab_id = excluded.active_starter_tab_id, \
           tab_order = excluded.tab_order, \
           active_dashboard_tab_id = excluded.active_dashboard_tab_id, \
           pane_layout = excluded.pane_layout, \
           active_create_table_tab_id = excluded.active_create_table_tab_id, \
           active_data_tab_id = excluded.active_data_tab_id",
    )
    .bind(&s.project_id)
    .bind(&s.active_view)
    .bind(&s.active_connection_id)
    .bind(&s.active_query_tab_id)
    .bind(&s.active_schema_tab_id)
    .bind(&s.active_explain_tab_id)
    .bind(&s.active_erd_tab_id)
    .bind(&s.active_statistics_tab_id)
    .bind(&s.active_workflow_tab_id)
    .bind(None::<String>) // active_visualize_tab_id
    .bind(&s.active_starter_tab_id)
    .bind(s.tab_order.get())
    .bind(&s.active_dashboard_tab_id)
    .bind(truthy_json_text(&s.pane_layout))
    .bind(&s.active_create_table_tab_id)
    .bind(&s.active_data_tab_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM tabs WHERE project_id = ?")
        .bind(&s.project_id)
        .execute(&mut *conn)
        .await?;
    insert_tabs(conn, s, true).await
}

/// Inserts the state's tabs as today's save does (the quirks in the
/// module doc), in its order: query, schema, explain, ERD, statistics,
/// workflow (`canvas`), starter, dashboard, table editor, data. Connection
/// and DuckDB extensions tabs have no rows.
///
/// With `skip_repeats`, a tab whose id an earlier tab took is skipped (the
/// table's key is `(id, project_id)`), and the count of skipped tabs is
/// returned; without it a repeat fails, as today's save did.
async fn insert_tabs(
    conn: &mut SqliteConnection,
    s: &PersistedProjectState,
    skip_repeats: bool,
) -> Result<u32> {
    let pid = &s.project_id;
    let mut seen: HashSet<String> = HashSet::new();
    let mut skipped = 0u32;
    let mut keep = |id: &str| -> bool {
        if !skip_repeats || seen.insert(id.to_string()) {
            return true;
        }
        skipped += 1;
        false
    };
    for t in &s.query_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, query, saved_query_id) \
             VALUES (?, ?, 'query', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.query)
        .bind(&t.query_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.schema_tabs {
        if !keep(&t.id) {
            continue;
        }
        // connection_id matters: restore drops schema tabs that don't have one.
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, table_name, schema_name, connection_id) \
             VALUES (?, ?, 'schema', ?, ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.table_name)
        .bind(&t.table_name)
        .bind(&t.schema_name)
        .bind(&t.connection_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.explain_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, source_query) \
             VALUES (?, ?, 'explain', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.source_query)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.erd_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'erd', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.statistics_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'statistics', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.workflow_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'canvas', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.starter_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, starter_type, closable) \
             VALUES (?, ?, 'starter', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.ty)
        .bind(i64::from(t.closable))
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.dashboard_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, source_query) \
             VALUES (?, ?, 'dashboard', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.dashboard_id)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.create_table_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id, source_query) \
             VALUES (?, ?, 'create_table', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .bind(&t.table_definition)
        .execute(&mut *conn)
        .await?;
    }
    for t in &s.data_tabs {
        if !keep(&t.id) {
            continue;
        }
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id, table_name, schema_name) \
             VALUES (?, ?, 'data', ?, ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.table_name)
        .bind(&t.connection_id)
        .bind(&t.table_name)
        .bind(&t.schema_name)
        .execute(&mut *conn)
        .await?;
    }
    Ok(skipped)
}

/// Deletes the project's state row, tabs and saved workflows, in one
/// transaction.
pub async fn remove(st: &Storage, project_id: &str) -> Result<()> {
    let mut tx = begin(st).await?;
    for table in ["project_state", "tabs", "saved_canvases"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE project_id = ?"))
            .bind(project_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::codec::raw;
    use super::*;

    #[test]
    fn missing_lists_are_stored_as_empty_lists() {
        assert_eq!(json_or_empty_list(&None), "[]");
        assert_eq!(json_or_empty_list(&Some(raw(r#"["a"]"#))), r#"["a"]"#);
    }
}
