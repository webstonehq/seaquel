import type { ActiveViewType, Pane, PaneLayout } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages split pane layouts for the tab area.
 * Each pane has its own set of tabs and active tab.
 * Keeps global activeView and per-type active IDs in sync
 * so existing view components continue working.
 */
export class PaneManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (projectId: string | null) => void,
  ) {}

  /**
   * Get the pane layout for the active project,
   * creating a default single-pane layout if none exists.
   */
  ensureLayout(): PaneLayout {
    const projectId = this.state.activeProjectId;
    if (!projectId) {
      return { panes: [], activePaneId: "" };
    }

    const existing = this.state.paneLayoutByProject[projectId];
    if (existing && existing.panes.length > 0) {
      return existing;
    }

    // Create default single pane from existing tab order
    const tabOrder = this.state.tabOrderByProject[projectId] ?? [];
    const activeTabId = this.findCurrentActiveTabId();
    const pane: Pane = {
      id: `pane-${crypto.randomUUID()}`,
      tabIds: [...tabOrder],
      activeTabId,
    };

    const layout: PaneLayout = {
      panes: [pane],
      activePaneId: pane.id,
    };

    this.state.paneLayoutByProject = {
      ...this.state.paneLayoutByProject,
      [projectId]: layout,
    };

    return layout;
  }

  /**
   * Set which pane has focus and sync global state.
   */
  setActivePane(paneId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.ensureLayout();
    if (layout.activePaneId === paneId) return;

    const pane = layout.panes.find((p) => p.id === paneId);
    if (!pane) return;

    this.updateLayout(projectId, { ...layout, activePaneId: paneId });

    // Sync global active state from the newly focused pane
    if (pane.activeTabId) {
      this.syncGlobalActiveState(pane.activeTabId);
    }
    this.schedulePersistence(projectId);
  }

  /**
   * Set the active tab within a pane and sync global state.
   */
  setActiveTab(paneId: string, tabId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.ensureLayout();
    const pane = layout.panes.find((p) => p.id === paneId);
    if (!pane) return;

    // Update pane's active tab
    const updatedPanes = layout.panes.map((p) =>
      p.id === paneId ? { ...p, activeTabId: tabId } : p,
    );

    this.updateLayout(projectId, {
      ...layout,
      panes: updatedPanes,
      activePaneId: paneId,
    });

    // Sync global state
    this.syncGlobalActiveState(tabId);
    this.schedulePersistence(projectId);
  }

  /**
   * Add a tab to the active pane's tab list.
   * Called by TabOrderingManager.add().
   */
  addTabToActivePane(tabId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.ensureLayout();
    const activePane = layout.panes.find((p) => p.id === layout.activePaneId);
    if (!activePane) return;

    const alreadyInPane = activePane.tabIds.includes(tabId);

    const updatedPanes = layout.panes.map((p) =>
      p.id === layout.activePaneId
        ? {
            ...p,
            tabIds: alreadyInPane ? p.tabIds : [...p.tabIds, tabId],
            activeTabId: tabId,
          }
        : p,
    );

    this.updateLayout(projectId, { ...layout, panes: updatedPanes });
  }

  /**
   * Remove a tab from whichever pane contains it.
   * Auto-closes the pane if it becomes empty.
   */
  removeTabFromPane(tabId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.state.paneLayoutByProject[projectId];
    if (!layout) return;

    const pane = layout.panes.find((p) => p.tabIds.includes(tabId));
    if (!pane) return;

    const newTabIds = pane.tabIds.filter((id) => id !== tabId);

    if (newTabIds.length === 0) {
      // Auto-close empty pane
      this.removePane(projectId, layout, pane.id);
    } else {
      // Update tab list and pick new active tab if needed
      let newActiveTabId = pane.activeTabId;
      if (pane.activeTabId === tabId) {
        const oldIndex = pane.tabIds.indexOf(tabId);
        const newIndex = Math.min(oldIndex, newTabIds.length - 1);
        newActiveTabId = newTabIds[newIndex] ?? null;
      }

      const updatedPanes = layout.panes.map((p) =>
        p.id === pane.id ? { ...p, tabIds: newTabIds, activeTabId: newActiveTabId } : p,
      );

      this.updateLayout(projectId, { ...layout, panes: updatedPanes });

      // Sync global state if this is the active pane
      if (layout.activePaneId === pane.id && newActiveTabId) {
        this.syncGlobalActiveState(newActiveTabId);
      }
    }
  }

  /**
   * Split: move a tab to a new pane to the right of the target pane.
   * @param targetPaneId - The pane whose edge was hit (new pane appears next to this one)
   * @param tabId - The tab to move
   * @param fromPaneId - The pane the tab is currently in (if different from targetPaneId)
   */
  splitRight(targetPaneId: string, tabId: string, fromPaneId?: string): void {
    this.split(targetPaneId, tabId, "right", fromPaneId);
  }

  /**
   * Split: move a tab to a new pane to the left of the target pane.
   * @param targetPaneId - The pane whose edge was hit (new pane appears next to this one)
   * @param tabId - The tab to move
   * @param fromPaneId - The pane the tab is currently in (if different from targetPaneId)
   */
  splitLeft(targetPaneId: string, tabId: string, fromPaneId?: string): void {
    this.split(targetPaneId, tabId, "left", fromPaneId);
  }

  /**
   * Move a tab from one pane to another.
   */
  moveTab(tabId: string, fromPaneId: string, toPaneId: string, index?: number): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;
    if (fromPaneId === toPaneId) return;

    const layout = this.ensureLayout();
    const fromPane = layout.panes.find((p) => p.id === fromPaneId);
    const toPane = layout.panes.find((p) => p.id === toPaneId);
    if (!fromPane || !toPane) return;

    // Remove from source
    const fromTabIds = fromPane.tabIds.filter((id) => id !== tabId);
    let fromActiveTabId = fromPane.activeTabId;
    if (fromActiveTabId === tabId) {
      const oldIndex = fromPane.tabIds.indexOf(tabId);
      const newIndex = Math.min(oldIndex, fromTabIds.length - 1);
      fromActiveTabId = fromTabIds[newIndex] ?? null;
    }

    // Add to target
    const toTabIds = [...toPane.tabIds];
    if (index !== undefined && index >= 0 && index <= toTabIds.length) {
      toTabIds.splice(index, 0, tabId);
    } else {
      toTabIds.push(tabId);
    }

    let updatedPanes = layout.panes.map((p) => {
      if (p.id === fromPaneId) {
        return { ...p, tabIds: fromTabIds, activeTabId: fromActiveTabId };
      }
      if (p.id === toPaneId) {
        return { ...p, tabIds: toTabIds, activeTabId: tabId };
      }
      return p;
    });

    // Auto-close empty source pane
    if (fromTabIds.length === 0) {
      updatedPanes = updatedPanes.filter((p) => p.id !== fromPaneId);
    }

    const newActivePaneId = updatedPanes.find((p) => p.id === toPaneId)
      ? toPaneId
      : (updatedPanes[0]?.id ?? "");

    this.updateLayout(projectId, {
      panes: updatedPanes,
      activePaneId: newActivePaneId,
    });

    this.syncGlobalActiveState(tabId);
    this.schedulePersistence(projectId);
  }

  /**
   * Reorder tabs within a pane.
   */
  reorderPane(paneId: string, newTabIds: string[]): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.state.paneLayoutByProject[projectId];
    if (!layout) return;

    const updatedPanes = layout.panes.map((p) =>
      p.id === paneId ? { ...p, tabIds: newTabIds } : p,
    );

    this.updateLayout(projectId, { ...layout, panes: updatedPanes });

    // Also sync the global tabOrder to be the concatenation of all pane tabIds
    this.syncGlobalTabOrder(projectId, updatedPanes);
    this.schedulePersistence(projectId);
  }

  /**
   * Find which pane contains a given tab.
   */
  findPaneForTab(tabId: string): Pane | null {
    const projectId = this.state.activeProjectId;
    if (!projectId) return null;

    const layout = this.state.paneLayoutByProject[projectId];
    if (!layout) return null;

    return layout.panes.find((p) => p.tabIds.includes(tabId)) ?? null;
  }

  /**
   * Resolve a tab ID to its view type by checking which tab collection it belongs to.
   */
  getTabViewType(tabId: string): ActiveViewType | null {
    const projectId = this.state.activeProjectId;
    if (!projectId) return null;

    if ((this.state.queryTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "query";
    if ((this.state.schemaTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "schema";
    if ((this.state.explainTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "explain";
    if ((this.state.erdTabsByProject[projectId] ?? []).some((t) => t.id === tabId)) return "erd";
    if ((this.state.statisticsTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "statistics";
    if ((this.state.workflowTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "workflow";
    if ((this.state.visualizeTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "visualize";
    if ((this.state.connectionTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "connection";
    if ((this.state.dashboardTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "dashboard";
    if ((this.state.starterTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "starter";
    if ((this.state.settingsTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "settings";
    if ((this.state.createTableTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "createTable";
    if ((this.state.dataTabsByProject[projectId] ?? []).some((t) => t.id === tabId)) return "data";
    if ((this.state.extensionsDuckdbTabsByProject[projectId] ?? []).some((t) => t.id === tabId))
      return "extensionsDuckdb";

    return null;
  }

  // === PRIVATE METHODS ===

  private split(
    targetPaneId: string,
    tabId: string,
    direction: "left" | "right",
    fromPaneId?: string,
  ): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const layout = this.ensureLayout();
    const targetPane = layout.panes.find((p) => p.id === targetPaneId);
    if (!targetPane) return;

    // Find the pane that actually contains the tab
    const actualFromPaneId = fromPaneId ?? targetPaneId;
    const fromPane = layout.panes.find((p) => p.id === actualFromPaneId);
    if (!fromPane) return;

    // Remove tab from its current pane
    const newFromTabIds = fromPane.tabIds.filter((id) => id !== tabId);
    let newFromActiveTabId = fromPane.activeTabId;
    if (newFromActiveTabId === tabId) {
      const oldIndex = fromPane.tabIds.indexOf(tabId);
      const newIndex = Math.min(oldIndex, newFromTabIds.length - 1);
      newFromActiveTabId = newFromTabIds[newIndex] ?? null;
    }

    // Create new pane
    const newPane: Pane = {
      id: `pane-${crypto.randomUUID()}`,
      tabIds: [tabId],
      activeTabId: tabId,
    };

    // Build updated pane list — update the from-pane's tab list
    const targetIndex = layout.panes.findIndex((p) => p.id === targetPaneId);
    let updatedPanes = layout.panes.map((p) =>
      p.id === actualFromPaneId
        ? { ...p, tabIds: newFromTabIds, activeTabId: newFromActiveTabId }
        : p,
    );

    // Auto-close empty from-pane
    const fromEmpty = newFromTabIds.length === 0;
    if (fromEmpty) {
      updatedPanes = updatedPanes.filter((p) => p.id !== actualFromPaneId);
    }

    // Insert new pane at the right position relative to the target pane
    // Recalculate target index after potential removal of from-pane
    const adjustedTargetIndex = updatedPanes.findIndex((p) => p.id === targetPaneId);
    const insertIndex =
      adjustedTargetIndex === -1
        ? Math.min(targetIndex, updatedPanes.length)
        : direction === "right"
          ? adjustedTargetIndex + 1
          : adjustedTargetIndex;

    updatedPanes.splice(insertIndex, 0, newPane);

    this.updateLayout(projectId, {
      panes: updatedPanes,
      activePaneId: newPane.id,
    });

    this.syncGlobalActiveState(tabId);
    this.syncGlobalTabOrder(projectId, updatedPanes);
    this.schedulePersistence(projectId);
  }

  private removePane(projectId: string, layout: PaneLayout, paneId: string): void {
    const updatedPanes = layout.panes.filter((p) => p.id !== paneId);

    if (updatedPanes.length === 0) {
      // All panes gone — create empty default
      const emptyPane: Pane = {
        id: `pane-${crypto.randomUUID()}`,
        tabIds: [],
        activeTabId: null,
      };
      this.updateLayout(projectId, {
        panes: [emptyPane],
        activePaneId: emptyPane.id,
      });
      return;
    }

    // Pick new active pane if needed
    let newActivePaneId = layout.activePaneId;
    if (newActivePaneId === paneId) {
      const oldIndex = layout.panes.findIndex((p) => p.id === paneId);
      const newIndex = Math.min(oldIndex, updatedPanes.length - 1);
      newActivePaneId = updatedPanes[newIndex]?.id ?? updatedPanes[0]?.id ?? "";
    }

    this.updateLayout(projectId, {
      panes: updatedPanes,
      activePaneId: newActivePaneId,
    });

    // Sync global state from the new active pane
    const newActivePane = updatedPanes.find((p) => p.id === newActivePaneId);
    if (newActivePane?.activeTabId) {
      this.syncGlobalActiveState(newActivePane.activeTabId);
    }
  }

  private updateLayout(projectId: string, layout: PaneLayout): void {
    this.state.paneLayoutByProject = {
      ...this.state.paneLayoutByProject,
      [projectId]: layout,
    };
  }

  /**
   * Sync global activeView and per-type active tab IDs from a given tab ID.
   * This ensures existing view components (QueryEditor, TableViewer, etc.)
   * continue to work without changes.
   */
  syncGlobalActiveState(tabId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const viewType = this.getTabViewType(tabId);
    if (!viewType) return;

    this.state.activeView = viewType;
    this.state.activeViewByProject[projectId] = viewType;

    // Update the corresponding per-type active ID
    const setters: Record<ActiveViewType, (id: string) => void> = {
      query: (id) => {
        this.state.activeQueryTabIdByProject = {
          ...this.state.activeQueryTabIdByProject,
          [projectId]: id,
        };
      },
      schema: (id) => {
        this.state.activeSchemaTabIdByProject = {
          ...this.state.activeSchemaTabIdByProject,
          [projectId]: id,
        };
      },
      explain: (id) => {
        this.state.activeExplainTabIdByProject = {
          ...this.state.activeExplainTabIdByProject,
          [projectId]: id,
        };
      },
      erd: (id) => {
        this.state.activeErdTabIdByProject = {
          ...this.state.activeErdTabIdByProject,
          [projectId]: id,
        };
      },
      statistics: (id) => {
        this.state.activeStatisticsTabIdByProject = {
          ...this.state.activeStatisticsTabIdByProject,
          [projectId]: id,
        };
      },
      workflow: (id) => {
        this.state.activeWorkflowTabIdByProject = {
          ...this.state.activeWorkflowTabIdByProject,
          [projectId]: id,
        };
      },
      visualize: (id) => {
        this.state.activeVisualizeTabIdByProject = {
          ...this.state.activeVisualizeTabIdByProject,
          [projectId]: id,
        };
      },
      connection: (id) => {
        this.state.activeConnectionTabIdByProject = {
          ...this.state.activeConnectionTabIdByProject,
          [projectId]: id,
        };
      },
      dashboard: (id) => {
        this.state.activeDashboardTabIdByProject = {
          ...this.state.activeDashboardTabIdByProject,
          [projectId]: id,
        };
      },
      starter: (id) => {
        this.state.activeStarterTabIdByProject = {
          ...this.state.activeStarterTabIdByProject,
          [projectId]: id,
        };
      },
      settings: (id) => {
        this.state.activeSettingsTabIdByProject = {
          ...this.state.activeSettingsTabIdByProject,
          [projectId]: id,
        };
      },
      createTable: (id) => {
        this.state.activeCreateTableTabIdByProject = {
          ...this.state.activeCreateTableTabIdByProject,
          [projectId]: id,
        };
      },
      data: (id) => {
        this.state.activeDataTabIdByProject = {
          ...this.state.activeDataTabIdByProject,
          [projectId]: id,
        };
      },
      extensionsDuckdb: (id) => {
        this.state.activeExtensionsDuckdbTabIdByProject = {
          ...this.state.activeExtensionsDuckdbTabIdByProject,
          [projectId]: id,
        };
      },
    };

    setters[viewType](tabId);
  }

  /**
   * Keep global tabOrder in sync with pane tab order.
   */
  private syncGlobalTabOrder(projectId: string, panes: Pane[]): void {
    const allTabIds = panes.flatMap((p) => p.tabIds);
    this.state.tabOrderByProject = {
      ...this.state.tabOrderByProject,
      [projectId]: allTabIds,
    };
  }

  /**
   * Find the currently active tab ID from global state.
   */
  private findCurrentActiveTabId(): string | null {
    const projectId = this.state.activeProjectId;
    if (!projectId) return null;

    const view = this.state.activeView;
    const idMap: Record<ActiveViewType, string | null> = {
      query: this.state.activeQueryTabIdByProject[projectId] ?? null,
      schema: this.state.activeSchemaTabIdByProject[projectId] ?? null,
      explain: this.state.activeExplainTabIdByProject[projectId] ?? null,
      erd: this.state.activeErdTabIdByProject[projectId] ?? null,
      statistics: this.state.activeStatisticsTabIdByProject[projectId] ?? null,
      workflow: this.state.activeWorkflowTabIdByProject[projectId] ?? null,
      visualize: this.state.activeVisualizeTabIdByProject[projectId] ?? null,
      connection: this.state.activeConnectionTabIdByProject[projectId] ?? null,
      dashboard: this.state.activeDashboardTabIdByProject[projectId] ?? null,
      starter: this.state.activeStarterTabIdByProject[projectId] ?? null,
      settings: this.state.activeSettingsTabIdByProject[projectId] ?? null,
      createTable: this.state.activeCreateTableTabIdByProject[projectId] ?? null,
      data: this.state.activeDataTabIdByProject[projectId] ?? null,
      extensionsDuckdb: this.state.activeExtensionsDuckdbTabIdByProject[projectId] ?? null,
    };

    return idMap[view] ?? null;
  }
}
