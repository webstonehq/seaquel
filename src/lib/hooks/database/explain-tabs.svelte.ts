import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages explain tabs: remove, set active.
 * Note: executeExplain is in UseDatabase as it requires database access.
 */
export class ExplainTabManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void,
    private removeTabGeneric: <T extends { id: string }>(
      tabsGetter: () => Map<string, T[]>,
      tabsSetter: (m: Map<string, T[]>) => void,
      activeIdGetter: () => Map<string, string | null>,
      activeIdSetter: (m: Map<string, string | null>) => void,
      tabId: string
    ) => void,
    private setActiveView: (view: "query" | "schema" | "explain") => void
  ) {}

  removeExplainTab(id: string) {
    this.removeTabGeneric(
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

  setActiveExplainTab(id: string) {
    if (!this.state.activeConnectionId) return;

    const newActiveExplainIds = new Map(this.state.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.state.activeConnectionId, id);
    this.state.activeExplainTabIdByConnection = newActiveExplainIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
