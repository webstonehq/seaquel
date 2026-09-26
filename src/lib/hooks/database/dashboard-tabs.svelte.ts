import type { ActiveViewType } from "$lib/types/persisted";
import type { DashboardTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";

/**
 * Manages Dashboard tabs.
 * Tabs are organized per-project.
 */
export class DashboardTabManager extends BaseTabManager<DashboardTab> {
  /** Called with the dashboard id when its tab closes (stops its widget runs). */
  private onClose: (dashboardId: string) => void = () => {};

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<DashboardTab> {
    return {
      getTabs: () => this.state.dashboardTabsByProject,
      setTabs: (r) => (this.state.dashboardTabsByProject = r),
      getActiveId: () => this.state.activeDashboardTabIdByProject,
      setActiveId: (r) => (this.state.activeDashboardTabIdByProject = r),
    };
  }

  /**
   * Open a dashboard tab. If a tab for this dashboard already exists, focus it.
   * If no dashboardId is provided, creates a new dashboard first.
   */
  add(dashboardId?: string, dashboardName?: string): string | null {
    if (!this.state.activeProjectId) return null;

    // Check if a tab already exists for this dashboard
    if (dashboardId) {
      const tabs = this.getProjectTabs();
      const existingTab = tabs.find((t) => t.dashboardId === dashboardId);
      if (existingTab) {
        this.setActive(existingTab.id);
        this.viewFallbackFn!("dashboard");
        return existingTab.id;
      }
    }

    const newTab: DashboardTab = {
      id: `dash-${crypto.randomUUID()}`,
      name: dashboardName ?? "New Dashboard",
      dashboardId: dashboardId ?? "",
    };

    this.appendTab(newTab);
    this.viewFallbackFn!("dashboard");

    return newTab.id;
  }

  /**
   * Update the dashboardId on a tab (used when creating a new dashboard).
   */
  setDashboardId(tabId: string, dashboardId: string): void {
    this.updateTab(tabId, (t) => ({ ...t, dashboardId }));
    this.schedulePersistence(this.state.activeProjectId);
  }

  setOnClose(fn: (dashboardId: string) => void): void {
    this.onClose = fn;
  }

  /** Close a tab, then stop its dashboard's auto-refresh and widget runs. */
  override remove(id: string): void {
    const dashboardId = this.getProjectTabs().find((t) => t.id === id)?.dashboardId;
    super.remove(id);
    if (dashboardId && !this.getProjectTabs().some((t) => t.dashboardId === dashboardId)) {
      this.onClose(dashboardId);
    }
  }

  /**
   * Rename a dashboard tab.
   */
  rename(tabId: string, name: string): void {
    this.updateTab(tabId, (t) => ({ ...t, name }));
    this.schedulePersistence(this.state.activeProjectId);
  }
}
