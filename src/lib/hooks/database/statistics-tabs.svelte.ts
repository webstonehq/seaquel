import type { ActiveViewType } from "$lib/types/persisted";
import type { StatisticsTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getEngineClient } from "$lib/engine";

/**
 * Manages Statistics dashboard tabs.
 * Tabs are organized per-project.
 */
export class StatisticsTabManager extends BaseTabManager<StatisticsTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<StatisticsTab> {
    return {
      getTabs: () => this.state.statisticsTabsByProject,
      setTabs: (r) => (this.state.statisticsTabsByProject = r),
      getActiveId: () => this.state.activeStatisticsTabIdByProject,
      setActiveId: (r) => (this.state.activeStatisticsTabIdByProject = r),
    };
  }

  /**
   * Add a Statistics tab for the current connection.
   * Returns the tab ID or null if no active project/connection.
   */
  async add(): Promise<string | null> {
    if (
      !this.state.activeProjectId ||
      !this.state.activeConnectionId ||
      !this.state.activeConnection
    )
      return null;

    const tabs = this.getProjectTabs();

    // Check if a Statistics tab already exists for this connection
    const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
    if (existingTab) {
      // Just switch to the existing tab
      this.setActive(existingTab.id);
      this.viewFallbackFn!("statistics");
      // Refresh the data
      await this.refresh(existingTab.id);
      return existingTab.id;
    }

    const newTab: StatisticsTab = {
      id: `stats-${crypto.randomUUID()}`,
      name: `Stats: ${this.state.activeConnection.name}`,
      connectionId: this.state.activeConnectionId,
      isLoading: true,
    };

    this.appendTab(newTab);
    this.viewFallbackFn!("statistics");

    // Load the statistics data
    await this.loadStatistics(newTab.id);

    return newTab.id;
  }

  /**
   * Refresh statistics data for a tab.
   */
  async refresh(tabId: string): Promise<void> {
    await this.loadStatistics(tabId);
  }

  /**
   * Load statistics data for a tab.
   */
  private async loadStatistics(tabId: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const tabs = this.getProjectTabs();
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    const connection = this.state.connections.find((c) => c.id === tab.connectionId);
    if (!connection) return;

    // Set loading state
    this.updateTab(tabId, (t) => ({ ...t, isLoading: true, error: undefined }));

    try {
      // The tab's own connection, not the active one.
      const client = getEngineClient(connection, this.state);
      const statistics = await client.statistics();

      this.updateTab(tabId, (t) => ({
        ...t,
        data: statistics,
        isLoading: false,
        lastRefreshed: new Date(),
      }));
    } catch (error) {
      this.updateTab(tabId, (t) => ({
        ...t,
        isLoading: false,
        error: error instanceof Error ? error.message : "Failed to load statistics",
      }));
    }
  }
}
