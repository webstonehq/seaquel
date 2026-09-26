import { getStatementAtOffsetOrThrow, hasParameters, type ParsedStatement } from "$lib/sql";
import { extractErrorMessage } from "$lib/errors";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import type { QueryEditorContext } from "./types.js";
import type { ParamDialog, PendingAction } from "./param-dialog.svelte.js";
import type { ViewState } from "./view-state.svelte.js";

export function createExplainVisualize(
  ctx: QueryEditorContext,
  paramDialog: ParamDialog,
  viewState: ViewState,
) {
  const { db } = ctx;

  /**
   * Shared helper: check for parameters before executing.
   * If parameters exist, open the param dialog; otherwise execute directly.
   */
  function withParamCheck(queryToCheck: string, action: PendingAction, executeFn: () => void) {
    if (hasParameters(queryToCheck)) {
      paramDialog.params = paramDialog.getParameterDefinitions(queryToCheck);
      paramDialog.action = action;
      paramDialog.show = true;
    } else {
      executeFn();
    }
  }

  function getQueryContext() {
    const activeTab = ctx.getActiveTab();
    const activeTabId = ctx.getActiveTabId();
    if (!activeTabId || !activeTab) return null;

    const query = activeTab.query;
    const cursorOffset = ctx.getMonacoRef()?.getCursorOffset() ?? 0;
    const dbType = db.state.activeConnection?.type ?? "postgres";
    // Strict: EXPLAIN ANALYZE runs the statement, and a failed lookup must not
    // fall back to the whole buffer.
    let currentStatement: ParsedStatement | null;
    try {
      currentStatement = getStatementAtOffsetOrThrow(query, cursorOffset, dbType);
    } catch (error) {
      errorToast(m.statement_at_cursor_failed({ error: extractErrorMessage(error) }));
      return null;
    }
    const queryToCheck = currentStatement?.sql ?? query;

    return { activeTabId, query, cursorOffset, queryToCheck };
  }

  function handleExplain(analyze: boolean) {
    const qctx = getQueryContext();
    if (!qctx) return;

    viewState.syncVisualBuilderSql();

    withParamCheck(
      qctx.queryToCheck,
      { type: "explain", analyze, cursorOffset: qctx.cursorOffset },
      () => {
        void db.explainTabs.executeEmbedded(qctx.activeTabId, analyze, qctx.cursorOffset);
        viewState.handleViewModeChange("explain");
      },
    );
  }

  function handleVisualize() {
    const qctx = getQueryContext();
    if (!qctx) return;

    viewState.syncVisualBuilderSql();

    withParamCheck(
      qctx.queryToCheck,
      { type: "visualize", cursorOffset: qctx.cursorOffset },
      () => {
        const success = db.visualizeTabs.visualizeEmbedded(qctx.activeTabId, qctx.cursorOffset);
        if (success) {
          viewState.handleViewModeChange("visualize");
        }
      },
    );
  }

  function handleRefreshExplain(analyze: boolean) {
    const qctx = getQueryContext();
    if (!qctx) return;

    withParamCheck(
      qctx.queryToCheck,
      { type: "explain", analyze, cursorOffset: qctx.cursorOffset },
      () => {
        void db.explainTabs.executeEmbedded(qctx.activeTabId, analyze, qctx.cursorOffset);
      },
    );
  }

  function handleRefreshVisualize() {
    const qctx = getQueryContext();
    if (!qctx) return;

    withParamCheck(
      qctx.queryToCheck,
      { type: "visualize", cursorOffset: qctx.cursorOffset },
      () => {
        db.visualizeTabs.visualizeEmbedded(qctx.activeTabId, qctx.cursorOffset);
      },
    );
  }

  function handleCloseExplain() {
    const activeTabId = ctx.getActiveTabId();
    if (!activeTabId) return;
    db.queryTabs.clearExplainResult(activeTabId);
    viewState.handleViewModeChange("table");
  }

  function handleCloseVisualize() {
    const activeTabId = ctx.getActiveTabId();
    if (!activeTabId) return;
    db.queryTabs.clearVisualizeResult(activeTabId);
    viewState.handleViewModeChange("table");
  }

  return {
    handleExplain,
    handleVisualize,
    handleRefreshExplain,
    handleRefreshVisualize,
    handleCloseExplain,
    handleCloseVisualize,
  };
}
