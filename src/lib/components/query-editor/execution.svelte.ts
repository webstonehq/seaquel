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
  /** The run waiting on the parameter dialog was confirmed first. */
  let paramRunConfirmed = false;

  /**
   * A run Core refused with `CONFIRM_REQUIRED` (a rerun, a file drop, or a
   * statement the editor's own check didn't flag), for the active tab only.
   */
  function corePending() {
    const pending = db.queries.pendingConfirm;
    return pending && pending.tabId === ctx.getActiveTabId() ? pending : null;
  }

  /**
   * The statement split or a check failed (the SQL module threw), so the
   * query isn't run: the lenient versions' `[]`/`null` would skip the
   * destructive check or run the whole buffer.
   */
  function reportCheckFailure(error: unknown) {
    errorToast(m.destructive_check_failed({ error: extractErrorMessage(error) }));
  }

  function proceedWithExecute(query: string, tabId: string, confirmed: boolean) {
    if (hasParameters(query)) {
      paramDialog.params = paramDialog.getParameterDefinitions(query);
      paramDialog.action = "query";
      paramRunConfirmed = confirmed;
      paramDialog.show = true;
    } else {
      void db.queries.execute(tabId, { confirmed });
    }
  }

  function proceedWithExecuteCurrent(
    currentStatement: ParsedStatement | null,
    tabId: string,
    cursorOffset: number,
    confirmed: boolean,
  ) {
    if (currentStatement && hasParameters(currentStatement.sql)) {
      paramDialog.params = paramDialog.getParameterDefinitions(currentStatement.sql);
      paramDialog.action = { type: "query-current", cursorOffset };
      paramRunConfirmed = confirmed;
      paramDialog.show = true;
    } else {
      void db.queries.executeCurrent(tabId, cursorOffset, { confirmed });
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
      pendingDestructiveAction = () => proceedWithExecute(query, activeTabId, true);
      showDestructiveConfirm = true;
      return;
    }

    proceedWithExecute(query, activeTabId, false);
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
          proceedWithExecuteCurrent(statement, activeTabId, cursorOffset, true);
        showDestructiveConfirm = true;
        return;
      }
    }

    proceedWithExecuteCurrent(currentStatement, activeTabId, cursorOffset, false);
  }

  function handleDestructiveConfirm() {
    const action = pendingDestructiveAction;
    showDestructiveConfirm = false;
    pendingDestructiveAction = null;
    destructiveStatements = [];
    if (action) {
      action();
      return;
    }
    // Core asked: run it again, confirmed.
    const pending = corePending();
    if (pending) void db.queries.confirmPending(pending.tabId);
  }

  function handleParamExecute(values: ParameterValue[]) {
    const activeTabId = ctx.getActiveTabId();
    const resultKey = ctx.getResultKey();
    if (!activeTabId) return;

    const action = paramDialog.action;

    if (action === "query") {
      void db.queries.execute(activeTabId, { params: values, confirmed: paramRunConfirmed });
      paramRunConfirmed = false;
    } else if (action && typeof action === "object" && action.type === "query-current") {
      void db.queries.executeCurrent(activeTabId, action.cursorOffset, {
        params: values,
        confirmed: paramRunConfirmed,
      });
      paramRunConfirmed = false;
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
    paramRunConfirmed = false;
  }

  return {
    /** The editor's own prompt, or a run Core refused on the active tab. */
    get showDestructiveConfirm() {
      return showDestructiveConfirm || corePending() !== null;
    },
    set showDestructiveConfirm(v: boolean) {
      showDestructiveConfirm = v;
      if (!v) {
        // Cancelled: nothing runs.
        pendingDestructiveAction = null;
        if (corePending()) db.queries.clearPendingConfirm();
      }
    },
    get destructiveStatements() {
      return showDestructiveConfirm ? destructiveStatements : (corePending()?.statements ?? []);
    },
    /** How many destructive statements there are; Core lists only the first. */
    get destructiveTotal() {
      if (showDestructiveConfirm) return destructiveStatements.length;
      const pending = corePending();
      return pending ? (pending.total ?? pending.statements.length) : 0;
    },

    handleExecute,
    handleExecuteCurrent,
    handleDestructiveConfirm,
    handleParamExecute,
    handleParamCancel,
  };
}

export type Execution = ReturnType<typeof createExecution>;
