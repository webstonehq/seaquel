import type { QueryTab, SchemaTab, ExplainTab, ErdTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import { setMapValue } from "./map-utils.js";

/**
 * Manages tab ordering across all tab types (query, schema, explain, ERD).
 * Provides generic tab removal logic and ordered tab computation.
 */
export class TabOrderingManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  /**
   * Generic tab removal helper used by all tab managers.
   * Handles removing from tab list and updating active tab selection.
   */
  removeTabGeneric<T extends { id: string }>(
    tabsGetter: () => Map<string, T[]>,
    tabsSetter: (m: Map<string, T[]>) => void,
    activeIdGetter: () => Map<string, string | null>,
    activeIdSetter: (m: Map<string, string | null>) => void,
    tabId: string
  ): void {
    if (!this.state.activeConnectionId) return;

    const tabs = tabsGetter().get(this.state.activeConnectionId) || [];
    const index = tabs.findIndex((t) => t.id === tabId);
    const newTabs = tabs.filter((t) => t.id !== tabId);

    setMapValue(tabsGetter, tabsSetter, this.state.activeConnectionId, newTabs);

    // Remove from tab order
    this.removeFromTabOrder(tabId);

    const currentActiveId = activeIdGetter().get(this.state.activeConnectionId);
    if (currentActiveId === tabId) {
      let newActiveId: string | null = null;
      if (newTabs.length > 0) {
        const newIndex = Math.min(index, newTabs.length - 1);
        newActiveId = newTabs[newIndex]?.id || null;
      }
      setMapValue(activeIdGetter, activeIdSetter, this.state.activeConnectionId, newActiveId);
    }
  }

  /**
   * Add a tab ID to the ordering array.
   */
  add(tabId: string): void {
    if (!this.state.activeConnectionId) return;
    const order = this.state.tabOrderByConnection.get(this.state.activeConnectionId) || [];
    if (!order.includes(tabId)) {
      setMapValue(
        () => this.state.tabOrderByConnection,
        (m) => (this.state.tabOrderByConnection = m),
        this.state.activeConnectionId,
        [...order, tabId]
      );
    }
  }

  /**
   * Remove a tab ID from the ordering array.
   */
  removeFromTabOrder(tabId: string): void {
    if (!this.state.activeConnectionId) return;
    const order = this.state.tabOrderByConnection.get(this.state.activeConnectionId) || [];
    setMapValue(
      () => this.state.tabOrderByConnection,
      (m) => (this.state.tabOrderByConnection = m),
      this.state.activeConnectionId,
      order.filter((id) => id !== tabId)
    );
  }

  /**
   * Reorder tabs to match the provided order array.
   */
  reorder(newOrder: string[]): void {
    if (!this.state.activeConnectionId) return;
    setMapValue(
      () => this.state.tabOrderByConnection,
      (m) => (this.state.tabOrderByConnection = m),
      this.state.activeConnectionId,
      newOrder
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Extract timestamp from tab ID for default ordering.
   */
  private getTabTimestamp(id: string): number {
    const match = id.match(/\d+$/);
    return match ? parseInt(match[0], 10) : 0;
  }

  /**
   * Get all tabs ordered by user preference or creation time.
   */
  get ordered(): Array<{
    id: string;
    type: "query" | "schema" | "explain" | "erd";
    tab: QueryTab | SchemaTab | ExplainTab | ErdTab;
  }> {
    if (!this.state.activeConnectionId) return [];

    // Ensure we have arrays (defensive against undefined)
    const queryTabs = this.state.queryTabs || [];
    const schemaTabs = this.state.schemaTabs || [];
    const explainTabs = this.state.explainTabs || [];
    const erdTabs = this.state.erdTabs || [];

    const allTabsUnordered: Array<{
      id: string;
      type: "query" | "schema" | "explain" | "erd";
      tab: QueryTab | SchemaTab | ExplainTab | ErdTab;
    }> = [];

    for (const t of queryTabs) {
      allTabsUnordered.push({ id: t.id, type: "query", tab: t });
    }
    for (const t of schemaTabs) {
      allTabsUnordered.push({ id: t.id, type: "schema", tab: t });
    }
    for (const t of explainTabs) {
      allTabsUnordered.push({ id: t.id, type: "explain", tab: t });
    }
    for (const t of erdTabs) {
      allTabsUnordered.push({ id: t.id, type: "erd", tab: t });
    }

    const order = this.state.tabOrderByConnection.get(this.state.activeConnectionId) || [];

    // Sort by order array, falling back to timestamp for new tabs
    return allTabsUnordered.sort((a, b) => {
      const aIndex = order.indexOf(a.id);
      const bIndex = order.indexOf(b.id);

      // Both in order array: use order
      if (aIndex !== -1 && bIndex !== -1) return aIndex - bIndex;

      // Only one in order: ordered comes first
      if (aIndex !== -1) return -1;
      if (bIndex !== -1) return 1;

      // Neither in order: fall back to timestamp
      return this.getTabTimestamp(a.id) - this.getTabTimestamp(b.id);
    });
  }
}
