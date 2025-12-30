import { toast } from "svelte-sonner";
import type { ExplainTab, ExplainResult, ExplainPlanNode } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type ExplainNode } from "$lib/db";
import { setMapValue } from "./map-utils.js";

/**
 * Manages EXPLAIN/ANALYZE tabs: execute, remove, set active.
 */
export class ExplainTabManager {
  constructor(
    private state: DatabaseState,
    private tabOrdering: TabOrderingManager,
    private schedulePersistence: (connectionId: string | null) => void,
    private setActiveView: (view: "query" | "schema" | "explain" | "erd") => void
  ) {}

  /**
   * Convert ExplainNode from adapter to ExplainResult for rendering.
   */
  private convertExplainNodeToResult(node: ExplainNode, isAnalyze: boolean): ExplainResult {
    let nodeCounter = 0;

    const convertNode = (n: ExplainNode): ExplainPlanNode => {
      const id = `node-${nodeCounter++}`;

      return {
        id,
        nodeType: n.type,
        relationName: undefined,
        alias: undefined,
        startupCost: 0,
        totalCost: n.cost || 0,
        planRows: n.rows || 0,
        planWidth: 0,
        actualStartupTime: undefined,
        actualTotalTime: n.actualTime,
        actualRows: n.actualRows,
        actualLoops: undefined,
        filter: n.label !== n.type ? n.label : undefined,
        indexName: undefined,
        indexCond: undefined,
        joinType: undefined,
        hashCond: undefined,
        sortKey: undefined,
        children: (n.children || []).map((child) => convertNode(child)),
      };
    };

    return {
      plan: convertNode(node),
      planningTime: 0,
      executionTime: undefined,
      isAnalyze,
    };
  }

  /**
   * Execute EXPLAIN or EXPLAIN ANALYZE on a query tab.
   */
  async execute(tabId: string, analyze: boolean = false): Promise<void> {
    if (!this.state.activeConnectionId || !this.state.activeConnection) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab || !tab.query.trim()) return;

    // Create a new explain tab
    const explainTabs = this.state.explainTabsByConnection.get(this.state.activeConnectionId) || [];
    const explainTabId = `explain-${Date.now()}`;
    const queryPreview = tab.query.substring(0, 30).replace(/\s+/g, " ").trim();
    const newExplainTab: ExplainTab = $state({
      id: explainTabId,
      name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
      sourceQuery: tab.query,
      result: undefined,
      isExecuting: true,
    });

    const newExplainTabs = new Map(this.state.explainTabsByConnection);
    newExplainTabs.set(this.state.activeConnectionId, [...explainTabs, newExplainTab]);
    this.state.explainTabsByConnection = newExplainTabs;

    this.tabOrdering.add(explainTabId);

    // Set as active and switch view
    const newActiveExplainIds = new Map(this.state.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.state.activeConnectionId, explainTabId);
    this.state.activeExplainTabIdByConnection = newActiveExplainIds;
    this.setActiveView("explain");
    this.schedulePersistence(this.state.activeConnectionId);

    try {
      const adapter = getAdapter(this.state.activeConnection.type);
      const explainQuery = adapter.getExplainQuery(tab.query, analyze);
      const result = (await this.state.activeConnection.database!.select(explainQuery)) as unknown[];

      // Use adapter to parse the results into common format
      const parsedNode = adapter.parseExplainResult(result, analyze);

      // Convert to ExplainResult format for rendering
      const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

      // Update the explain tab with results
      newExplainTab.result = explainResult;
      newExplainTab.isExecuting = false;

      // Trigger reactivity
      const updatedExplainTabs = new Map(this.state.explainTabsByConnection);
      updatedExplainTabs.set(
        this.state.activeConnectionId!,
        [...(this.state.explainTabsByConnection.get(this.state.activeConnectionId!) || [])]
      );
      this.state.explainTabsByConnection = updatedExplainTabs;
    } catch (error) {
      // Remove failed explain tab
      const updatedExplainTabs = new Map(this.state.explainTabsByConnection);
      const currentTabs = updatedExplainTabs.get(this.state.activeConnectionId!) || [];
      updatedExplainTabs.set(
        this.state.activeConnectionId!,
        currentTabs.filter((t) => t.id !== explainTabId)
      );
      this.state.explainTabsByConnection = updatedExplainTabs;

      // Switch back to query view
      this.setActiveView("query");
      toast.error(`Explain failed: ${error}`);
    }
  }

  /**
   * Remove an explain tab by ID.
   */
  remove(id: string): void {
    this.tabOrdering.removeTabGeneric(
      () => this.state.explainTabsByConnection,
      (m) => (this.state.explainTabsByConnection = m),
      () => this.state.activeExplainTabIdByConnection,
      (m) => (this.state.activeExplainTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
    // Switch to query view if no explain tabs left
    if (this.state.activeConnectionId && this.state.explainTabs.length === 0) {
      this.setActiveView("query");
    }
  }

  /**
   * Set the active explain tab by ID.
   */
  setActive(id: string): void {
    if (!this.state.activeConnectionId) return;

    const newActiveExplainIds = new Map(this.state.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.state.activeConnectionId, id);
    this.state.activeExplainTabIdByConnection = newActiveExplainIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
