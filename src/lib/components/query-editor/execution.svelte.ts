import {
  findDestructiveStatements,
  getStatementAtOffsetOrThrow,
  hasParameters,
  isDestructiveStatement,
  splitSqlStatementsOrThrow,
  type DestructiveStatement,
  type ParsedStatement,
} from "$lib/sql";
import { extractErrorMessage } from "$lib/errors";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import type { ParameterValue } from "$lib/types";
import type { QueryEditorContext } from "./types.js";
import type { ParamDialog } from "./param-dialog.svelte.js";

export function createExecution(
  ctx: QueryEditorContext,
  paramDialog: ParamDialog,
  syncVisualBuilder: () => void,
) {
  const { db } = ctx;

  let showDestructiveConfirm = $state(false);
  let destructiveStatements = $state<DestructiveStatement[]>([]);
  let pendingDestructiveAction = $state<(() => void) | null>(null);

  /**
   * The statement split or a check failed (the SQL module threw), so the
   * query isn't run: the lenient versions' `[]`/`null` would skip the
   * destructive check or run the whole buffer.
   */
  function reportCheckFailure(error: unknown) {
    errorToast(m.destructive_check_failed({ error: extractErrorMessage(error) }));
  }

  function proceedWithExecute(query: string, tabId: string) {
    if (hasParameters(query)) {
      paramDialog.params = paramDialog.getParameterDefinitions(query);
      paramDialog.action = "query";
      paramDialog.show = true;
    } else {
      void db.queries.execute(tabId);
    }
  }

  function proceedWithExecuteCurrent(
    currentStatement: ParsedStatement | null,
    tabId: string,
    cursorOffset: number,
  ) {
    if (currentStatement && hasParameters(currentStatement.sql)) {
      paramDialog.params = paramDialog.getParameterDefinitions(currentStatement.sql);
      paramDialog.action = { type: "query-current", cursorOffset };
      paramDialog.show = true;
    } else {
      void db.queries.executeCurrent(tabId, cursorOffset);
    }
  }

  function handleExecute() {
    const activeTab = ctx.getActiveTab();
    const activeTabId = ctx.getActiveTabId();
    if (!activeTabId || !activeTab) return;

    syncVisualBuilder();

    const query = activeTab.query;
    const dbType = db.state.activeConnection?.type ?? "postgres";

    let dangerous: DestructiveStatement[];
    try {
      const statements = splitSqlStatementsOrThrow(query, dbType);
      dangerous = findDestructiveStatements(statements, dbType);
    } catch (error) {
      // The check couldn't run: don't run the script unconfirmed.
      reportCheckFailure(error);
      return;
    }

    if (dangerous.length > 0) {
      destructiveStatements = dangerous;
      pendingDestructiveAction = () => proceedWithExecute(query, activeTabId);
      showDestructiveConfirm = true;
      return;
    }

    proceedWithExecute(query, activeTabId);
  }

  function handleExecuteCurrent() {
    const activeTab = ctx.getActiveTab();
    const activeTabId = ctx.getActiveTabId();
    if (!activeTabId || !activeTab) return;

    syncVisualBuilder();

    const query = activeTab.query;
    const cursorOffset = ctx.getMonacoRef()?.getCursorOffset() ?? 0;
    const dbType = db.state.activeConnection?.type ?? "postgres";

    let currentStatement: ParsedStatement | null;
    let reason: DestructiveStatement["reason"] | null = null;
    try {
      currentStatement = getStatementAtOffsetOrThrow(query, cursorOffset, dbType);
      if (currentStatement) reason = isDestructiveStatement(currentStatement.sql, dbType);
    } catch (error) {
      reportCheckFailure(error);
      return;
    }
    if (currentStatement) {
      if (reason) {
        destructiveStatements = [
          { sql: currentStatement.sql, index: currentStatement.index, reason },
        ];
        const statement = currentStatement;
        pendingDestructiveAction = () =>
          proceedWithExecuteCurrent(statement, activeTabId, cursorOffset);
        showDestructiveConfirm = true;
        return;
      }
    }

    proceedWithExecuteCurrent(currentStatement, activeTabId, cursorOffset);
  }

  function handleDestructiveConfirm() {
    showDestructiveConfirm = false;
    pendingDestructiveAction?.();
    pendingDestructiveAction = null;
    destructiveStatements = [];
  }

  function handleParamExecute(values: ParameterValue[]) {
    const activeTabId = ctx.getActiveTabId();
    const resultKey = ctx.getResultKey();
    if (!activeTabId) return;

    const action = paramDialog.action;

    if (action === "query") {
      void db.queries.executeWithParams(activeTabId, values);
    } else if (action && typeof action === "object" && action.type === "query-current") {
      void db.queries.executeCurrentWithParams(activeTabId, action.cursorOffset, values);
    } else if (action && typeof action === "object" && action.type === "explain") {
      void db.explainTabs.executeEmbeddedWithParams(
        activeTabId,
        values,
        action.analyze,
        action.cursorOffset,
      );
      if (resultKey) {
        return { switchViewMode: "explain" as const };
      }
    } else if (action && typeof action === "object" && action.type === "visualize") {
      const success = db.visualizeTabs.visualizeEmbeddedWithParams(
        activeTabId,
        values,
        action.cursorOffset,
      );
      if (success && resultKey) {
        return { switchViewMode: "visualize" as const };
      }
    }

    paramDialog.action = null;
    return undefined;
  }

  function handleParamCancel() {
    paramDialog.action = null;
  }

  return {
    get showDestructiveConfirm() {
      return showDestructiveConfirm;
    },
    set showDestructiveConfirm(v: boolean) {
      showDestructiveConfirm = v;
    },
    get destructiveStatements() {
      return destructiveStatements;
    },

    handleExecute,
    handleExecuteCurrent,
    handleDestructiveConfirm,
    handleParamExecute,
    handleParamCancel,
  };
}

export type Execution = ReturnType<typeof createExecution>;
