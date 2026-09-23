import type { SqliteDatabase } from "../sqlite-types";
import { safeJsonParse } from "../create-repo";
import type { PersistedProjectState } from "$lib/types";
import type { SavedWorkflow } from "$lib/types/workflow";

export const projectStateRepo = {
  async load(db: SqliteDatabase, projectId: string): Promise<PersistedProjectState | null> {
    const rows = await db.query<{
      active_view: string;
      active_connection_id: string | null;
      active_query_tab_id: string | null;
      active_schema_tab_id: string | null;
      active_explain_tab_id: string | null;
      active_erd_tab_id: string | null;
      active_statistics_tab_id: string | null;
      active_workflow_tab_id: string | null;
      active_visualize_tab_id: string | null;
      active_starter_tab_id: string | null;
      active_create_table_tab_id: string | null;
      active_data_tab_id: string | null;
      tab_order: string;
      connection_order: string | null;
    }>("SELECT * FROM project_state WHERE project_id = ?", [projectId]);

    if (rows.length === 0) return null;
    const state = rows[0];

    // Load tabs
    const tabs = await db.query<{
      id: string;
      tab_type: string;
      name: string;
      query: string | null;
      saved_query_id: string | null;
      shared_query_id: string | null;
      table_name: string | null;
      schema_name: string | null;
      source_query: string | null;
      connection_id: string | null;
      starter_type: string | null;
      closable: number | null;
    }>("SELECT * FROM tabs WHERE project_id = ?", [projectId]);

    const queryTabs = tabs
      .filter((t) => t.tab_type === "query")
      .map((t) => ({
        id: t.id,
        name: t.name,
        query: t.query ?? "",
        queryId: t.saved_query_id ?? t.shared_query_id ?? undefined,
      }));

    const schemaTabs = tabs
      .filter((t) => t.tab_type === "schema")
      .map((t) => ({
        id: t.id,
        tableName: t.table_name ?? "",
        schemaName: t.schema_name ?? "",
        connectionId: t.connection_id ?? undefined,
      }));

    const explainTabs = tabs
      .filter((t) => t.tab_type === "explain")
      .map((t) => ({
        id: t.id,
        name: t.name,
        sourceQuery: t.source_query ?? "",
      }));

    const erdTabs = tabs
      .filter((t) => t.tab_type === "erd")
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connection_id ?? undefined,
      }));

    const statisticsTabs = tabs
      .filter((t) => t.tab_type === "statistics")
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connection_id ?? "",
      }));

    const workflowTabs = tabs
      .filter((t) => t.tab_type === "canvas")
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connection_id ?? "",
      }));

    const starterTabs = tabs
      .filter((t) => t.tab_type === "starter")
      .map((t) => ({
        id: t.id,
        type: (t.starter_type ?? "getting-started") as "getting-started" | "migration-tips",
        name: t.name,
        closable: t.closable === 1,
      }));

    // Load saved workflows
    const dashboardTabs = tabs
      .filter((t) => t.tab_type === "dashboard")
      .map((t) => ({
        id: t.id,
        name: t.name,
        dashboardId: t.source_query ?? "",
      }));

    const createTableTabs = tabs
      .filter((t) => t.tab_type === "create_table")
      .map((t) => ({
        id: t.id,
        connectionId: t.connection_id ?? "",
        name: t.name,
        tableDefinition: t.source_query ?? "{}",
      }));

    const dataTabs = tabs
      .filter((t) => t.tab_type === "data")
      .map((t) => ({
        id: t.id,
        connectionId: t.connection_id ?? "",
        tableName: t.table_name ?? "",
        schemaName: t.schema_name ?? "",
      }));

    const workflowRows = await db.query<{ id: string; data: string }>(
      "SELECT id, data FROM saved_canvases WHERE project_id = ?",
      [projectId],
    );
    const savedWorkflows: SavedWorkflow[] = workflowRows
      .map((r) => safeJsonParse<SavedWorkflow | null>(r.data, null))
      .filter((w): w is SavedWorkflow => w !== null);

    // Read active_dashboard_tab_id if the column exists
    let activeDashboardTabId: string | null = null;
    try {
      const dashRow = await db.query<{ active_dashboard_tab_id: string | null }>(
        "SELECT active_dashboard_tab_id FROM project_state WHERE project_id = ?",
        [projectId],
      );
      if (dashRow.length > 0) {
        activeDashboardTabId = dashRow[0].active_dashboard_tab_id;
      }
    } catch {
      // Column doesn't exist yet (pre-migration)
    }

    // Read starred_shared_query_ids if the column exists
    let starredSharedQueryIds: string[] = [];
    try {
      const starredRow = await db.query<{ starred_shared_query_ids: string }>(
        "SELECT starred_shared_query_ids FROM project_state WHERE project_id = ?",
        [projectId],
      );
      if (starredRow.length > 0) {
        starredSharedQueryIds = safeJsonParse(starredRow[0].starred_shared_query_ids, []);
      }
    } catch {
      // Column doesn't exist yet (pre-migration)
    }

    // Read starred_shared_dashboard_ids if the column exists
    let starredSharedDashboardIds: string[] = [];
    try {
      const starredDashRow = await db.query<{ starred_shared_dashboard_ids: string }>(
        "SELECT starred_shared_dashboard_ids FROM project_state WHERE project_id = ?",
        [projectId],
      );
      if (starredDashRow.length > 0) {
        starredSharedDashboardIds = safeJsonParse(
          starredDashRow[0].starred_shared_dashboard_ids,
          [],
        );
      }
    } catch {
      // Column doesn't exist yet (pre-migration)
    }

    // Read pane_layout if the column exists
    let paneLayout: PersistedProjectState["paneLayout"] | undefined;
    try {
      const paneRow = await db.query<{ pane_layout: string | null }>(
        "SELECT pane_layout FROM project_state WHERE project_id = ?",
        [projectId],
      );
      if (paneRow.length > 0 && paneRow[0].pane_layout) {
        paneLayout = safeJsonParse(paneRow[0].pane_layout, undefined);
      }
    } catch {
      // Column doesn't exist yet (pre-v4 migration)
    }

    return {
      projectId,
      queryTabs,
      schemaTabs,
      explainTabs,
      erdTabs,
      statisticsTabs,
      workflowTabs,
      tabOrder: safeJsonParse(state.tab_order, []),
      connectionOrder: safeJsonParse(state.connection_order ?? "[]", []),
      activeQueryTabId: state.active_query_tab_id,
      activeSchemaTabId: state.active_schema_tab_id,
      activeExplainTabId: state.active_explain_tab_id,
      activeErdTabId: state.active_erd_tab_id,
      activeStatisticsTabId: state.active_statistics_tab_id,
      activeWorkflowTabId: state.active_workflow_tab_id,
      activeView: state.active_view as PersistedProjectState["activeView"],
      activeConnectionId: state.active_connection_id,
      starterTabs,
      activeStarterTabId: state.active_starter_tab_id,
      savedWorkflows,
      dashboardTabs,
      activeDashboardTabId,
      createTableTabs,
      activeCreateTableTabId: state.active_create_table_tab_id ?? null,
      dataTabs,
      activeDataTabId: state.active_data_tab_id ?? null,
      starredSharedQueryIds,
      starredSharedDashboardIds,
      paneLayout,
    };
  },

  async save(db: SqliteDatabase, state: PersistedProjectState): Promise<void> {
    // Build all statements and execute in a single transaction to avoid
    // "database is locked" errors from concurrent writes.
    const statements: Array<{ sql: string; params?: unknown[] }> = [];

    statements.push({
      sql: `INSERT OR REPLACE INTO project_state
       (project_id, active_view, active_connection_id, active_query_tab_id, active_schema_tab_id,
        active_explain_tab_id, active_erd_tab_id, active_statistics_tab_id, active_workflow_tab_id,
        active_visualize_tab_id, active_starter_tab_id, tab_order, connection_order,
        active_dashboard_tab_id, starred_shared_query_ids, starred_shared_dashboard_ids, pane_layout,
        active_create_table_tab_id, active_data_tab_id)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      params: [
        state.projectId,
        state.activeView,
        state.activeConnectionId,
        state.activeQueryTabId,
        state.activeSchemaTabId,
        state.activeExplainTabId,
        state.activeErdTabId,
        state.activeStatisticsTabId ?? null,
        state.activeWorkflowTabId ?? null,
        null, // activeVisualizeTabId
        state.activeStarterTabId ?? null,
        JSON.stringify(state.tabOrder),
        JSON.stringify(state.connectionOrder ?? []),
        state.activeDashboardTabId ?? null,
        JSON.stringify(state.starredSharedQueryIds ?? []),
        JSON.stringify(state.starredSharedDashboardIds ?? []),
        state.paneLayout ? JSON.stringify(state.paneLayout) : null,
        state.activeCreateTableTabId ?? null,
        state.activeDataTabId ?? null,
      ],
    });

    statements.push({ sql: "DELETE FROM tabs WHERE project_id = ?", params: [state.projectId] });

    for (const tab of state.queryTabs) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, query, saved_query_id) VALUES (?, ?, 'query', ?, ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.query, tab.queryId ?? null],
      });
    }

    for (const tab of state.schemaTabs) {
      // connection_id matters: restore drops schema tabs that don't have one.
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, table_name, schema_name, connection_id) VALUES (?, ?, 'schema', ?, ?, ?, ?)`,
        params: [
          tab.id,
          state.projectId,
          tab.tableName,
          tab.tableName,
          tab.schemaName,
          tab.connectionId ?? null,
        ],
      });
    }

    for (const tab of state.explainTabs) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, source_query) VALUES (?, ?, 'explain', ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.sourceQuery],
      });
    }

    for (const tab of state.erdTabs) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, connection_id) VALUES (?, ?, 'erd', ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.connectionId ?? null],
      });
    }

    for (const tab of state.statisticsTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, connection_id) VALUES (?, ?, 'statistics', ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.connectionId],
      });
    }

    for (const tab of state.workflowTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, connection_id) VALUES (?, ?, 'canvas', ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.connectionId],
      });
    }

    for (const tab of state.starterTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, starter_type, closable) VALUES (?, ?, 'starter', ?, ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.type, tab.closable ? 1 : 0],
      });
    }

    for (const tab of state.dashboardTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, source_query) VALUES (?, ?, 'dashboard', ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.dashboardId],
      });
    }

    for (const tab of state.createTableTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, connection_id, source_query) VALUES (?, ?, 'create_table', ?, ?, ?)`,
        params: [tab.id, state.projectId, tab.name, tab.connectionId, tab.tableDefinition],
      });
    }

    for (const tab of state.dataTabs ?? []) {
      statements.push({
        sql: `INSERT INTO tabs (id, project_id, tab_type, name, connection_id, table_name, schema_name) VALUES (?, ?, 'data', ?, ?, ?, ?)`,
        params: [
          tab.id,
          state.projectId,
          tab.tableName,
          tab.connectionId,
          tab.tableName,
          tab.schemaName,
        ],
      });
    }

    statements.push({
      sql: "DELETE FROM saved_canvases WHERE project_id = ?",
      params: [state.projectId],
    });
    for (const workflow of state.savedWorkflows ?? []) {
      statements.push({
        sql: "INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)",
        params: [
          (workflow as { id?: string }).id ?? `workflow-${crypto.randomUUID()}`,
          state.projectId,
          JSON.stringify(workflow),
        ],
      });
    }

    await db.transaction(statements);
  },

  async remove(db: SqliteDatabase, projectId: string): Promise<void> {
    await db.execute("DELETE FROM project_state WHERE project_id = ?", [projectId]);
    await db.execute("DELETE FROM tabs WHERE project_id = ?", [projectId]);
    await db.execute("DELETE FROM saved_canvases WHERE project_id = ?", [projectId]);
  },
};
