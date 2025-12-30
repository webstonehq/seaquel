import type { ErdTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { setMapValue } from "./map-utils.js";

/**
 * Manages ERD (Entity Relationship Diagram) tabs.
 */
export class ErdTabManager {
  constructor(
    private state: DatabaseState,
    private tabOrdering: TabOrderingManager,
    private schedulePersistence: (connectionId: string | null) => void,
    private setActiveView: (view: "query" | "schema" | "explain" | "erd") => void
  ) {}

  /**
   * Add an ERD tab for the current connection.
   * Returns the tab ID or null if no active connection.
   */
  add(): string | null {
    if (!this.state.activeConnectionId || !this.state.activeConnection) return null;

    const tabs = this.state.erdTabsByConnection.get(this.state.activeConnectionId) || [];

    // Check if an ERD tab already exists for this connection
    const existingTab = tabs.find((t) => t.name === `ERD: ${this.state.activeConnection!.name}`);
    if (existingTab) {
      // Just switch to the existing tab
      setMapValue(
        () => this.state.activeErdTabIdByConnection,
        (m) => (this.state.activeErdTabIdByConnection = m),
        this.state.activeConnectionId,
        existingTab.id
      );
      this.setActiveView("erd");
      return existingTab.id;
    }

    const erdTabId = `erd-${Date.now()}`;
    const newErdTab: ErdTab = {
      id: erdTabId,
      name: `ERD: ${this.state.activeConnection.name}`,
    };

    const newErdTabs = new Map(this.state.erdTabsByConnection);
    newErdTabs.set(this.state.activeConnectionId, [...tabs, newErdTab]);
    this.state.erdTabsByConnection = newErdTabs;

    this.tabOrdering.add(erdTabId);

    setMapValue(
      () => this.state.activeErdTabIdByConnection,
      (m) => (this.state.activeErdTabIdByConnection = m),
      this.state.activeConnectionId,
      erdTabId
    );

    this.setActiveView("erd");
    this.schedulePersistence(this.state.activeConnectionId);

    return erdTabId;
  }

  /**
   * Remove an ERD tab by ID.
   */
  remove(id: string): void {
    this.tabOrdering.removeTabGeneric(
      () => this.state.erdTabsByConnection,
      (m) => (this.state.erdTabsByConnection = m),
      () => this.state.activeErdTabIdByConnection,
      (m) => (this.state.activeErdTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);

    // If no more ERD tabs, switch back to query view
    const remainingTabs = this.state.erdTabsByConnection.get(this.state.activeConnectionId!) || [];
    if (remainingTabs.length === 0) {
      this.setActiveView("query");
    }
  }

  /**
   * Set the active ERD tab by ID.
   */
  setActive(id: string): void {
    if (!this.state.activeConnectionId) return;
    setMapValue(
      () => this.state.activeErdTabIdByConnection,
      (m) => (this.state.activeErdTabIdByConnection = m),
      this.state.activeConnectionId,
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
