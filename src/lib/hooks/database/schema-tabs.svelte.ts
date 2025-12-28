import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages schema tabs: remove, set active.
 * Note: addSchemaTab is in UseDatabase as it requires database access for metadata fetching.
 */
export class SchemaTabManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void,
    private removeTabGeneric: <T extends { id: string }>(
      tabsGetter: () => Map<string, T[]>,
      tabsSetter: (m: Map<string, T[]>) => void,
      activeIdGetter: () => Map<string, string | null>,
      activeIdSetter: (m: Map<string, string | null>) => void,
      tabId: string
    ) => void
  ) {}

  removeSchemaTab(id: string) {
    this.removeTabGeneric(
      () => this.state.schemaTabsByConnection,
      (m) => (this.state.schemaTabsByConnection = m),
      () => this.state.activeSchemaTabIdByConnection,
      (m) => (this.state.activeSchemaTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }

  setActiveSchemaTab(id: string) {
    if (!this.state.activeConnectionId) return;

    const newActiveSchemaIds = new Map(this.state.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.state.activeConnectionId, id);
    this.state.activeSchemaTabIdByConnection = newActiveSchemaIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
