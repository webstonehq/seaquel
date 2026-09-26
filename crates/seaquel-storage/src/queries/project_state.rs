//! `projectStateRepo`: a project's `project_state` row, its `tabs` and its
//! `saved_canvases` (saved workflows).
//!
//! The quirks it keeps (fixtures README, quirk 10): schema and data tabs
//! store `tableName` in `name` too; workflow tabs are `tab_type = 'canvas'`;
//! a dashboard tab keeps its dashboard id, and a table-editor tab its
//! definition, in `source_query`; `active_visualize_tab_id` is always
//! written NULL; tabs of other types load as nothing.

use seaquel_types::storage::{
    PersistedCreateTableTab, PersistedDashboardTab, PersistedDataTab, PersistedErdTab,
    PersistedExplainTab, PersistedProjectState, PersistedQueryTab, PersistedSchemaTab,
    PersistedStarterTab, PersistedStatisticsTab, PersistedWorkflowTab,
};
use serde_json::value::RawValue;
use sqlx::sqlite::SqliteRow;

use super::codec::{
    begin, bind_json_id, flag, is_null, json, json_or, opt_text, parse_json, text,
    truthy_json_text, Result,
};
use crate::Storage;

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
pub async fn load(st: &Storage, project_id: &str) -> Result<Option<PersistedProjectState>> {
    let Some(state) = sqlx::query("SELECT * FROM project_state WHERE project_id = ?")
        .bind(project_id)
        .fetch_optional(st.pool())
        .await?
    else {
        return Ok(None);
    };

    let tabs = sqlx::query("SELECT * FROM tabs WHERE project_id = ?")
        .bind(project_id)
        .fetch_all(st.pool())
        .await?;
    let tabs = tabs.iter().map(tab).collect::<Result<Vec<_>>>()?;

    let canvases: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT data FROM saved_canvases WHERE project_id = ?")
            .bind(project_id)
            .fetch_all(st.pool())
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

    for t in &s.query_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, query, saved_query_id) \
             VALUES (?, ?, 'query', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.query)
        .bind(&t.query_id)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.schema_tabs {
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
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.explain_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, source_query) \
             VALUES (?, ?, 'explain', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.source_query)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.erd_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'erd', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.statistics_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'statistics', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.workflow_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id) \
             VALUES (?, ?, 'canvas', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.starter_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, starter_type, closable) \
             VALUES (?, ?, 'starter', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.ty)
        .bind(i64::from(t.closable))
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.dashboard_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, source_query) \
             VALUES (?, ?, 'dashboard', ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.dashboard_id)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.create_table_tabs {
        sqlx::query(
            "INSERT INTO tabs (id, project_id, tab_type, name, connection_id, source_query) \
             VALUES (?, ?, 'create_table', ?, ?, ?)",
        )
        .bind(&t.id)
        .bind(pid)
        .bind(&t.name)
        .bind(&t.connection_id)
        .bind(&t.table_definition)
        .execute(&mut *tx)
        .await?;
    }
    for t in &s.data_tabs {
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
        .execute(&mut *tx)
        .await?;
    }

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
