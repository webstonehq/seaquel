import type { ActiveViewType } from "$lib/types/persisted";
import { withErrorHandling } from "$lib/errors";
import type { ExplainTab, ExplainResult, ParameterValue } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getEngineClient } from "$lib/engine";
import { resolveQuery } from "./resolve-query.js";

/**
 * Callback for setting explain result on a query tab.
 */
export type SetExplainResultCallback = (
  tabId: string,
  result: ExplainResult,
  sourceQuery: string,
  isAnalyze: boolean,
) => void;

/**
 * Callback for setting explain executing state on a query tab.
 */
export type SetExplainExecutingCallback = (
  tabId: string,
  isExecuting: boolean,
  isAnalyze: boolean,
) => void;

/**
 * Manages EXPLAIN/ANALYZE tabs: execute, remove, set active.
 * Tabs are organized per-project.
 */
export class ExplainTabManager extends BaseTabManager<ExplainTab> {
  private setExplainResult?: SetExplainResultCallback;
  private setExplainExecuting?: SetExplainExecutingCallback;

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<ExplainTab> {
    return {
      getTabs: () => this.state.explainTabsByProject,
      setTabs: (r) => (this.state.explainTabsByProject = r),
      getActiveId: () => this.state.activeExplainTabIdByProject,
      setActiveId: (r) => (this.state.activeExplainTabIdByProject = r),
    };
  }

  /**
   * Set callbacks for embedded explain results (stored on QueryTab).
   */
  setEmbeddedCallbacks(
    setResult: SetExplainResultCallback,
    setExecuting: SetExplainExecutingCallback,
  ): void {
    this.setExplainResult = setResult;
    this.setExplainExecuting = setExecuting;
  }

  private resolveQuery(tabId: string, cursorOffset?: number, parameterValues?: ParameterValue[]) {
    return resolveQuery(this.state, tabId, cursorOffset, parameterValues);
  }

  /**
   * Core explain execution logic shared by all four public methods.
   * Runs EXPLAIN (or EXPLAIN ANALYZE) and returns the parsed result.
   */
  private async performExplain(
    queryToExplain: string,
    analyze: boolean,
    bindValues?: unknown[],
  ): Promise<ExplainResult> {
    // SQLite timing and the MSSQL/DuckDB bind skipping live in TsEngineClient.explain.
    const client = getEngineClient(this.state.activeConnection!, this.state);
    return client.explain(queryToExplain, bindValues, analyze);
  }

  /**
   * Execute EXPLAIN or EXPLAIN ANALYZE and store result embedded in the query tab.
   * This is the new approach where results appear below the editor instead of in a separate tab.
   */
  async executeEmbedded(
    tabId: string,
    analyze: boolean = false,
    cursorOffset?: number,
  ): Promise<void> {
    return this.executeEmbeddedWithParams(tabId, undefined, analyze, cursorOffset);
  }

  /**
   * Execute EXPLAIN or EXPLAIN ANALYZE with optional parameter substitution (embedded version).
   */
  async executeEmbeddedWithParams(
    tabId: string,
    parameterValues?: ParameterValue[],
    analyze: boolean = false,
    cursorOffset?: number,
  ): Promise<void> {
    if (
      !this.state.activeProjectId ||
      !this.state.activeConnectionId ||
      !this.state.activeConnection
    )
      return;
    if (!this.setExplainResult || !this.setExplainExecuting) {
      console.warn("Embedded callbacks not set, falling back to tab-based explain");
      return this.execute(tabId, analyze, cursorOffset, parameterValues);
    }

    const resolved = this.resolveQuery(tabId, cursorOffset, parameterValues);
    if (!resolved) return;

    this.setExplainExecuting(tabId, true, analyze);

    const result = await withErrorHandling(
      () => this.performExplain(resolved.query, analyze, resolved.bindValues),
      "QUERY_FAILED",
      "Explain failed",
    );

    if (result.ok) {
      this.setExplainResult(tabId, result.value, resolved.tab.query, analyze);
    } else {
      this.setExplainExecuting(tabId, false, analyze);
    }
  }

  /**
   * Execute EXPLAIN or EXPLAIN ANALYZE on a query tab (tab-based version).
   * If cursorOffset is provided, explains only the statement at that cursor position.
   */
  async execute(
    tabId: string,
    analyze: boolean = false,
    cursorOffset?: number,
    parameterValues?: ParameterValue[],
  ): Promise<void> {
    if (
      !this.state.activeProjectId ||
      !this.state.activeConnectionId ||
      !this.state.activeConnection
    )
      return;

    const resolved = this.resolveQuery(tabId, cursorOffset, parameterValues);
    if (!resolved) return;

    // For display, use the original (unsubstituted) query
    const displayQuery = parameterValues
      ? (this.resolveQuery(tabId, cursorOffset)?.query ?? resolved.query)
      : resolved.query;

    // Create a new explain tab
    const explainTabId = `explain-${crypto.randomUUID()}`;
    const queryPreview = displayQuery.substring(0, 30).replace(/\s+/g, " ").trim();
    const newExplainTab: ExplainTab = $state({
      id: explainTabId,
      name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
      sourceQuery: displayQuery,
      result: undefined,
      isExecuting: true,
    });

    this.appendTab(newExplainTab);
    this.viewFallbackFn!("explain");

    const result = await withErrorHandling(
      () => this.performExplain(resolved.query, analyze, resolved.bindValues),
      "QUERY_FAILED",
      "Explain failed",
    );

    if (result.ok) {
      newExplainTab.result = result.value;
      newExplainTab.isExecuting = false;

      const currentTabs = this.getProjectTabs();
      this.setProjectTabs([...currentTabs]);
    } else {
      const currentTabs = this.getProjectTabs();
      this.setProjectTabs(currentTabs.filter((t) => t.id !== explainTabId));

      this.viewFallbackFn!("query");
    }
  }

  /**
   * @deprecated Use execute with parameterValues param
   */
  async executeWithParams(
    tabId: string,
    parameterValues: ParameterValue[],
    analyze: boolean = false,
    cursorOffset?: number,
  ): Promise<void> {
    return this.execute(tabId, analyze, cursorOffset, parameterValues);
  }
}
