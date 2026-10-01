import type { StarterTab, StarterTabType } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";

/**
 * Manages starter tabs using the standard BaseTabManager pattern.
 * Starter tabs are shown when no database connection is active,
 * providing quick actions and migration guidance.
 */
export class StarterTabManager extends BaseTabManager<StarterTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
  ) {
    super(state, tabOrdering, schedulePersistence);
  }

  protected get accessors(): TabStateAccessors<StarterTab> {
    return {
      getTabs: () => this.state.starterTabsByProject,
      setTabs: (r) => (this.state.starterTabsByProject = r),
      getActiveId: () => this.state.activeStarterTabIdByProject,
      setActiveId: (r) => (this.state.activeStarterTabIdByProject = r),
    };
  }

  /**
   * Add a starter tab by type.
   */
  add(tabType?: StarterTabType): string | null {
    if (!this.state.activeProjectId) return null;

    const config: Record<StarterTabType, { id: string; name: string }> = {
      "getting-started": { id: "getting-started", name: "Getting Started" },
      "migration-tips": { id: "migration-tips", name: "Migration Tips" },
    };

    const type = tabType ?? "getting-started";
    const { id, name } = config[type];

    // Don't add duplicate
    const existing = this.getProjectTabs();
    if (existing.some((t) => t.id === id)) {
      this.setActive(id);
      return id;
    }

    const newTab: StarterTab = {
      id,
      type,
      name,
      closable: true,
    };

    this.appendTab(newTab);
    return newTab.id;
  }

  /**
   * Initialize default starter tabs for a new project.
   * Adds both Getting Started and Migration Tips tabs.
   */
  initializeDefaults(projectId: string): void {
    const existing = this.state.starterTabsByProject[projectId];
    if (existing && existing.length > 0) return;

    if (projectId === this.state.activeProjectId) {
      this.add("getting-started");
      this.add("migration-tips");

      // Set the first tab as active
      const tabs = this.state.starterTabsByProject[projectId] ?? [];
      if (tabs.length > 0) {
        this.setActive(tabs[0].id);
      }
      return;
    }

    // Another project (its load finished after a switch): write its own
    // records, never borrowing the active project, whose tabs, layout and
    // view would otherwise take the starter tabs' side effects.
    const tabs: StarterTab[] = [
      { id: "getting-started", type: "getting-started", name: "Getting Started", closable: true },
      { id: "migration-tips", type: "migration-tips", name: "Migration Tips", closable: true },
    ];
    const ids = tabs.map((t) => t.id);
    this.state.starterTabsByProject = { ...this.state.starterTabsByProject, [projectId]: tabs };
    this.state.activeStarterTabIdByProject = {
      ...this.state.activeStarterTabIdByProject,
      [projectId]: ids[0],
    };
    const order = this.state.tabOrderByProject[projectId] ?? [];
    this.state.tabOrderByProject = {
      ...this.state.tabOrderByProject,
      [projectId]: [...order, ...ids.filter((id) => !order.includes(id))],
    };
    const layout = this.state.paneLayoutByProject[projectId];
    const pane = layout?.panes.find((p) => p.id === layout.activePaneId);
    if (layout && pane) {
      this.state.paneLayoutByProject = {
        ...this.state.paneLayoutByProject,
        [projectId]: {
          ...layout,
          panes: layout.panes.map((p) =>
            p === pane
              ? {
                  ...p,
                  tabIds: [...p.tabIds, ...ids.filter((id) => !p.tabIds.includes(id))],
                  activeTabId: ids[0],
                }
              : p,
          ),
        },
      };
    }
    this.schedulePersistence(projectId);
  }

  /**
   * Reset to default starter tabs.
   * Removes all existing starter tabs and re-adds defaults.
   */
  reset(): void {
    if (!this.state.activeProjectId) return;
    const projectId = this.state.activeProjectId;

    // Remove existing starter tabs
    const existing = this.getProjectTabs();
    for (const tab of existing) {
      this.remove(tab.id);
    }

    // Re-add defaults
    this.add("getting-started");
    this.add("migration-tips");

    // Set the first tab as active
    const tabs = this.state.starterTabsByProject[projectId] ?? [];
    if (tabs.length > 0) {
      this.setActive(tabs[0].id);
    }
  }
}
