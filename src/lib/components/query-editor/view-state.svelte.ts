import type { ResultViewMode, ChartConfig, ResolvedQueryVersion } from "$lib/types";
import { createDefaultChartConfig } from "$lib/components/charts/index.js";
import { DEFAULT_LAYOUT_OPTIONS, type QueryLayoutOptions } from "$lib/utils/query-visual-layout";
import { splitSqlStatements } from "$lib/db/sql-parser.js";
import { schemaToQueryBuilder } from "$lib/utils/schema-adapter";
import { editorQualifiedTable, getEngineClient } from "$lib/engine";
import { sampleQueries } from "$lib/config/sample-queries.js";
import type { QueryEditorContext } from "./types.js";
import { cellKey } from "$lib/values";

export interface DiffModeState {
  original: string;
  modified: string;
  leftLabel: string;
  rightLabel: string;
  restoreLeft?: string;
  restoreRight?: string;
}

export function createViewState(ctx: QueryEditorContext, onCloseDiff?: () => void) {
  const { db } = ctx;

  // View mode & chart config (per result, keyed by tab-result combination)
  let viewModeByResult = $state<Record<string, ResultViewMode>>({});
  let chartConfigByResult = $state<Record<string, ChartConfig>>({});

  // Visualize layout options
  let visualizeLayoutOptions = $state<QueryLayoutOptions>({ ...DEFAULT_LAYOUT_OPTIONS });

  // Diff mode
  let diffMode = $state<DiffModeState | null>(null);
  let diffOriginalWidth = $state(0);

  // Visual query builder panel
  let visualPanelOpen = $state(false);
  let visualPanelGetSql: (() => string) | undefined = $state(undefined);

  // Track query content for live statement count
  let currentQuery = $derived(ctx.getActiveTab()?.query ?? "");

  // Derived state
  const explainResult = $derived(ctx.getActiveTab()?.explainResult);
  const visualizeResult = $derived(ctx.getActiveTab()?.visualizeResult);
  const allResults = $derived(ctx.getActiveTab()?.results ?? []);

  const savedQueryVersions = $derived(
    ctx.getActiveTab()?.queryId
      ? db.savedQueries.getResolvedVersionsForQuery(ctx.getActiveTab()!.queryId!)
      : [],
  );

  const isExplainStale = $derived(
    explainResult?.sourceQuery && currentQuery
      ? explainResult.sourceQuery.trim() !== currentQuery.trim()
      : false,
  );

  const isVisualizeStale = $derived(
    visualizeResult?.sourceQuery && currentQuery
      ? visualizeResult.sourceQuery.trim() !== currentQuery.trim()
      : false,
  );

  const liveStatementCount = $derived.by(() => {
    if (!currentQuery?.trim()) return 0;
    const dbType = db.state.activeConnection?.type ?? "postgres";
    return splitSqlStatements(currentQuery, dbType).length;
  });

  const isEditable = $derived.by(() => {
    const result = ctx.getActiveResult();
    return result?.sourceTable && result.sourceTable.primaryKeys.length > 0 && !result.isError;
  });

  const qePendingChangesForTable = $derived.by(() => {
    const st = ctx.getActiveResult()?.sourceTable;
    if (!st) return [];
    return db.state.activePendingChanges.filter(
      (c) => c.target?.schema === st.schema && c.target?.table === st.name,
    );
  });

  const qePendingCellEdits = $derived.by(() => {
    const result = ctx.getActiveResult();
    const st = result?.sourceTable;
    const rows = result?.rows;
    const resultColumns = result?.columns ?? [];
    if (!st || !rows || st.primaryKeys.length === 0) return undefined;
    // Rows are columnar — resolve each PK column name to its position once.
    const pkIdx = st.primaryKeys.map((pk) => resultColumns.indexOf(pk));
    const edits = new Map<string, unknown>();
    for (const change of qePendingChangesForTable) {
      if (
        (change.origin === "inline-edit" || change.origin === "set-default") &&
        change.target?.primaryKeyValues &&
        change.target.column
      ) {
        const rowIdx = rows.findIndex((row) =>
          st.primaryKeys.every(
            (pk, i) =>
              pkIdx[i] !== -1 &&
              cellKey(row[pkIdx[i]]) === cellKey(change.target!.primaryKeyValues![pk]),
          ),
        );
        if (rowIdx >= 0) {
          edits.set(`${rowIdx}:${change.target.column}`, change.target.newValue);
        }
      }
    }
    return edits.size > 0 ? edits : undefined;
  });

  const qePendingRowDeletes = $derived.by(() => {
    const result = ctx.getActiveResult();
    const st = result?.sourceTable;
    const rows = result?.rows;
    const resultColumns = result?.columns ?? [];
    if (!st || !rows || st.primaryKeys.length === 0) return undefined;
    const pkIdx = st.primaryKeys.map((pk) => resultColumns.indexOf(pk));
    const deletes = new Set<number>();
    for (const change of qePendingChangesForTable) {
      if (change.origin === "delete-row" && change.target?.primaryKeyValues) {
        const rowIdx = rows.findIndex((row) =>
          st.primaryKeys.every(
            (pk, i) =>
              pkIdx[i] !== -1 &&
              cellKey(row[pkIdx[i]]) === cellKey(change.target!.primaryKeyValues![pk]),
          ),
        );
        if (rowIdx >= 0) {
          deletes.add(rowIdx);
        }
      }
    }
    return deletes.size > 0 ? deletes : undefined;
  });

  const queryBuilderSchema = $derived.by(() => {
    const connection = db.state.activeConnection;
    if (!db.state.activeSchema) return [];
    // FROM and JOIN name tables `schema.table`, quoted where needed.
    if (!connection) return schemaToQueryBuilder(db.state.activeSchema);
    const client = getEngineClient(connection, db.state);
    const quoted = (s: string, t: string) => client.qualifiedTable(s, t);
    return schemaToQueryBuilder(db.state.activeSchema, (schema, table) =>
      editorQualifiedTable(connection.type, quoted, schema, table),
    );
  });

  const activeSampleQueries = $derived(
    sampleQueries[db.state.activeConnection?.type ?? "postgres"]?.slice(0, 2) ?? [],
  );

  // View mode
  const currentViewMode = $derived<ResultViewMode>(
    ctx.getResultKey() ? (viewModeByResult[ctx.getResultKey()!] ?? "table") : "table",
  );

  const currentChartConfig = $derived<ChartConfig | undefined>(
    ctx.getResultKey() && ctx.getActiveResult()
      ? (chartConfigByResult[ctx.getResultKey()!] ??
          createDefaultChartConfig(ctx.getActiveResult()!.columns, ctx.getActiveResult()!.rows))
      : undefined,
  );

  function handleViewModeChange(mode: ResultViewMode) {
    const key = ctx.getResultKey();
    if (key) {
      viewModeByResult[key] = mode;
    }
  }

  function handleChartConfigChange(config: ChartConfig) {
    const key = ctx.getResultKey();
    if (key) {
      chartConfigByResult[key] = config;
    }
  }

  // Diff mode
  function closeDiff() {
    diffMode = null;
    onCloseDiff?.();
  }

  // Visual builder
  function syncVisualBuilderSql() {
    if (visualPanelGetSql && ctx.getActiveTabId()) {
      const sql = visualPanelGetSql();
      db.queryTabs.updateContent(ctx.getActiveTabId()!, sql);
      // Update current query tracking - we need to reassign to trigger derived updates
      // Note: currentQuery is $derived from activeTab.query, which updateContent updates
    }
  }

  function toggleVisualPanel() {
    if (visualPanelOpen) {
      syncVisualBuilderSql();
    }
    visualPanelOpen = !visualPanelOpen;
  }

  function handleTrySampleQuery(query: string) {
    const tabId = ctx.getActiveTabId();
    if (tabId) {
      db.queryTabs.updateContent(tabId, query);
    } else {
      db.queryTabs.add(undefined, query);
    }
  }

  function handleDiffVersions(selected: ResolvedQueryVersion[]) {
    const activeTab = ctx.getActiveTab();
    if (selected.length === 0) {
      diffMode = null;
    } else if (selected.length === 1) {
      diffMode = {
        original: selected[0].query,
        modified: activeTab?.query ?? "",
        leftLabel: `Version ${selected[0].version}`,
        rightLabel: "Current",
        restoreLeft: selected[0].query,
      };
    } else if (selected.length === 2) {
      diffMode = {
        original: selected[0].query,
        modified: selected[1].query,
        leftLabel: `Version ${selected[0].version}`,
        rightLabel: `Version ${selected[1].version}`,
        restoreLeft: selected[0].query,
        restoreRight: selected[1].query,
      };
    }
  }

  function restoreVersion(query: string) {
    const tabId = ctx.getActiveTabId();
    const activeTab = ctx.getActiveTab();
    if (!tabId || !activeTab) return;
    db.queryTabs.updateContent(tabId, query);
    activeTab.query = query;
    closeDiff();
  }

  return {
    // View mode
    get currentViewMode() {
      return currentViewMode;
    },
    get currentChartConfig() {
      return currentChartConfig;
    },
    handleViewModeChange,
    handleChartConfigChange,

    // Visualize layout
    get visualizeLayoutOptions() {
      return visualizeLayoutOptions;
    },
    set visualizeLayoutOptions(v: QueryLayoutOptions) {
      visualizeLayoutOptions = v;
    },

    // Diff mode
    get diffMode() {
      return diffMode;
    },
    set diffMode(v: DiffModeState | null) {
      diffMode = v;
    },
    get diffOriginalWidth() {
      return diffOriginalWidth;
    },
    set diffOriginalWidth(v: number) {
      diffOriginalWidth = v;
    },
    closeDiff,
    handleDiffVersions,
    restoreVersion,

    // Visual builder
    get visualPanelOpen() {
      return visualPanelOpen;
    },
    set visualPanelOpen(v: boolean) {
      visualPanelOpen = v;
    },
    get visualPanelGetSql() {
      return visualPanelGetSql;
    },
    set visualPanelGetSql(v: (() => string) | undefined) {
      visualPanelGetSql = v;
    },
    syncVisualBuilderSql,
    toggleVisualPanel,

    // Derived state
    get currentQuery() {
      return currentQuery;
    },
    get explainResult() {
      return explainResult;
    },
    get visualizeResult() {
      return visualizeResult;
    },
    get isExplainStale() {
      return isExplainStale;
    },
    get isVisualizeStale() {
      return isVisualizeStale;
    },
    get allResults() {
      return allResults;
    },
    get liveStatementCount() {
      return liveStatementCount;
    },
    get isEditable() {
      return isEditable;
    },
    get qePendingCellEdits() {
      return qePendingCellEdits;
    },
    get qePendingRowDeletes() {
      return qePendingRowDeletes;
    },
    get savedQueryVersions() {
      return savedQueryVersions;
    },
    get queryBuilderSchema() {
      return queryBuilderSchema;
    },
    get activeSampleQueries() {
      return activeSampleQueries;
    },
    handleTrySampleQuery,
  };
}

export type ViewState = ReturnType<typeof createViewState>;
