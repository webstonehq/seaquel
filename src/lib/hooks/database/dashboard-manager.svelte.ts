import type {
  Dashboard,
  DashboardWidget,
  DashboardVersion,
  ResolvedDashboardVersion,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import { getDatabase, dashboardsRepo } from "$lib/storage";
import { log } from "$lib/utils/logger";
import {
  createDashboardVersionEntry,
  resolveDashboardVersions,
} from "$lib/utils/dashboard-versions";
import { toPersistedDashboard } from "./dashboard-serialize.js";

export { stripWidgetRuntimeState } from "./dashboard-serialize.js";

/**
 * Manages dashboard CRUD operations, widget execution, and auto-refresh.
 * Dashboards are per-project.
 */
export class DashboardManager {
  private autoRefreshTimers = new Map<string, ReturnType<typeof setInterval>>();
  private writeDashboardFile: ((dashboard: Dashboard) => Promise<void>) | null = null;
  private deleteDashboardFile: ((dashboard: Dashboard) => Promise<void>) | null = null;

  constructor(
    private state: DatabaseState,
    private executeQuery: (query: string) => Promise<Record<string, unknown>[]>,
    private scheduleProjectPersistence: (projectId: string | null) => void,
    private persistence?: PersistenceManager,
  ) {}

  setFileProjection(fns: {
    writeDashboardFile: (dashboard: Dashboard) => Promise<void>;
    deleteDashboardFile: (dashboard: Dashboard) => Promise<void>;
  }) {
    this.writeDashboardFile = fns.writeDashboardFile;
    this.deleteDashboardFile = fns.deleteDashboardFile;
  }

  // === CRUD ===

  async createDashboard(name: string): Promise<Dashboard | null> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return null;

    const now = new Date();
    const dashboard: Dashboard = {
      id: `dashboard-${crypto.randomUUID()}`,
      name,
      projectId,
      widgets: [],
      viewport: { x: 0, y: 0, zoom: 1 },
      dateFilter: null,
      createdAt: now,
      updatedAt: now,
      shared: false,
    };

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: [...dashboards, dashboard],
    };

    await this.persistDashboard(dashboard);
    return dashboard;
  }

  async deleteDashboard(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    // Stop any auto-refresh timers
    const dashboard = this.getDashboard(id);
    if (dashboard) {
      for (const widget of dashboard.widgets) {
        this.stopAutoRefresh(id, widget.id);
      }
      // Clean up git file for shared dashboards
      if (dashboard.shared) {
        await this.deleteDashboardFile?.(dashboard);
      }
    }

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.filter((d) => d.id !== id),
    };

    try {
      const db = await getDatabase();
      await dashboardsRepo.remove(db, id);
    } catch (error) {
      void log.error("Failed to delete dashboard:", error);
    }
  }

  async renameDashboard(id: string, name: string): Promise<void> {
    const before = this.getDashboard(id);
    if (before) this.captureVersion(before);
    this.updateDashboard(id, (d) => ({ ...d, name, updatedAt: new Date() }));
    const dashboard = this.getDashboard(id);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  // === WIDGET MANAGEMENT ===

  async addWidget(dashboardId: string, widget: DashboardWidget): Promise<void> {
    const before = this.getDashboard(dashboardId);
    if (before) this.captureVersion(before);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: [...d.widgets, widget],
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  async updateWidget(
    dashboardId: string,
    widgetId: string,
    updates: Partial<DashboardWidget>,
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    if (before) this.captureVersion(before);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...updates } : w)),
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  async removeWidget(dashboardId: string, widgetId: string): Promise<void> {
    const before = this.getDashboard(dashboardId);
    if (before) this.captureVersion(before);
    this.stopAutoRefresh(dashboardId, widgetId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.filter((w) => w.id !== widgetId),
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  async moveWidget(
    dashboardId: string,
    widgetId: string,
    position: { x: number; y: number },
  ): Promise<void> {
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...position } : w)),
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  async resizeWidget(
    dashboardId: string,
    widgetId: string,
    size: { width: number; height: number },
  ): Promise<void> {
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...size } : w)),
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  // === VIEWPORT ===

  async updateViewport(
    dashboardId: string,
    viewport: { x: number; y: number; zoom: number },
  ): Promise<void> {
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      viewport,
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) await this.persistDashboard(dashboard);
  }

  // === WIDGET EXECUTION ===

  async executeWidget(dashboardId: string, widgetId: string): Promise<void> {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard) return;

    const widget = dashboard.widgets.find((w) => w.id === widgetId);
    if (!widget) return;

    // Resolve query
    let query = widget.query;
    if (widget.querySource === "saved" && widget.savedQueryId) {
      const projectId = dashboard.projectId;
      const savedQueries = this.state.queriesByProject[projectId] ?? [];
      const savedQuery = savedQueries.find((sq) => sq.id === widget.savedQueryId);
      if (savedQuery) {
        query = savedQuery.query;
      }
    }

    if (!query) return;

    // Inject date filter placeholders (validate and escape to prevent SQL injection)
    if (dashboard.dateFilter) {
      const isValidDate = (val: string) => /^[\d\-T:.Z ]+$/.test(val);
      const escapeDate = (val: string) => `'${val.replace(/'/g, "''")}'`;
      if (isValidDate(dashboard.dateFilter.start) && isValidDate(dashboard.dateFilter.end)) {
        query = query
          .replace(/\{\{start_date\}\}/g, escapeDate(dashboard.dateFilter.start))
          .replace(/\{\{end_date\}\}/g, escapeDate(dashboard.dateFilter.end));
      }
    }

    // Set loading state
    this.updateWidgetState(dashboardId, widgetId, { isLoading: true, error: undefined });

    try {
      const result = await this.executeQuery(query);
      this.updateWidgetState(dashboardId, widgetId, {
        result,
        isLoading: false,
        lastRefreshed: new Date(),
      });
    } catch (error) {
      this.updateWidgetState(dashboardId, widgetId, {
        isLoading: false,
        error: error instanceof Error ? error.message : "Query execution failed",
      });
    }
  }

  async executeAllWidgets(dashboardId: string): Promise<void> {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard) return;

    await Promise.all(
      dashboard.widgets.map((widget) => this.executeWidget(dashboardId, widget.id)),
    );
  }

  // === AUTO-REFRESH ===

  startAutoRefresh(dashboardId: string, widgetId: string): void {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard) return;

    const widget = dashboard.widgets.find((w) => w.id === widgetId);
    if (!widget?.autoRefreshSeconds || widget.autoRefreshSeconds <= 0) return;

    const timerKey = `${dashboardId}:${widgetId}`;
    this.stopAutoRefresh(dashboardId, widgetId);

    const timer = setInterval(() => {
      void this.executeWidget(dashboardId, widgetId);
    }, widget.autoRefreshSeconds * 1000);

    this.autoRefreshTimers.set(timerKey, timer);
  }

  stopAutoRefresh(dashboardId: string, widgetId: string): void {
    const timerKey = `${dashboardId}:${widgetId}`;
    const timer = this.autoRefreshTimers.get(timerKey);
    if (timer) {
      clearInterval(timer);
      this.autoRefreshTimers.delete(timerKey);
    }
  }

  stopAllAutoRefresh(): void {
    for (const timer of this.autoRefreshTimers.values()) {
      clearInterval(timer);
    }
    this.autoRefreshTimers.clear();
  }

  // === DATE FILTER ===

  async setDateFilter(
    dashboardId: string,
    range: { start: string; end: string } | null,
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    if (before) this.captureVersion(before);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      dateFilter: range,
      updatedAt: new Date(),
    }));
    const dashboard = this.getDashboard(dashboardId);
    if (dashboard) {
      await this.persistDashboard(dashboard);
      await this.executeAllWidgets(dashboardId);
    }
  }

  // === DATA LOADING ===

  async loadDashboards(projectId: string): Promise<void> {
    this.stopAllAutoRefresh();
    try {
      const db = await getDatabase();
      const rows = await dashboardsRepo.loadByProject(db, projectId);

      const dashboards: Dashboard[] = rows.map((r) => ({
        id: r.id,
        name: r.name,
        projectId: r.projectId,
        widgets: JSON.parse(r.widgets),
        viewport: JSON.parse(r.viewport),
        dateFilter: r.dateFilter ? JSON.parse(r.dateFilter) : null,
        starred: r.starred,
        shared: r.shared ?? false,
        description: r.description,
        createdAt: new Date(r.createdAt),
        updatedAt: new Date(r.updatedAt),
      }));

      this.state.dashboardsByProject = {
        ...this.state.dashboardsByProject,
        [projectId]: dashboards,
      };
    } catch (error) {
      void log.error("Failed to load dashboards:", error);
    }
  }

  // === STARRING ===

  toggleDashboardStarred(id: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.map((d) => (d.id === id ? { ...d, starred: !d.starred } : d)),
    };
    this.scheduleProjectPersistence(projectId);
  }

  /** @deprecated Use toggleDashboardStarred instead */
  toggleSharedDashboardStarred(id: string): void {
    this.toggleDashboardStarred(id);
  }

  // === SHARE / UNSHARE ===

  async shareDashboardById(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    const dashboard = dashboards.find((d) => d.id === id);
    if (!dashboard || dashboard.shared) return;

    const updated = { ...dashboard, shared: true, updatedAt: new Date() };
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.map((d) => (d.id === id ? updated : d)),
    };

    await this.writeDashboardFile?.(updated);
    await this.persistDashboard(updated);
    this.scheduleProjectPersistence(projectId);
  }

  async unshareDashboardById(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    const dashboard = dashboards.find((d) => d.id === id);
    if (!dashboard || !dashboard.shared) return;

    await this.deleteDashboardFile?.(dashboard);

    const updated = { ...dashboard, shared: false, updatedAt: new Date() };
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.map((d) => (d.id === id ? updated : d)),
    };

    await this.persistDashboard(updated);
    this.scheduleProjectPersistence(projectId);
  }

  // === HELPERS ===

  getDashboard(id: string): Dashboard | undefined {
    for (const dashboards of Object.values(this.state.dashboardsByProject)) {
      const found = dashboards.find((d) => d.id === id);
      if (found) return found;
    }
    return undefined;
  }

  private updateDashboard(id: string, updater: (d: Dashboard) => Dashboard): void {
    for (const [projectId, dashboards] of Object.entries(this.state.dashboardsByProject)) {
      const index = dashboards.findIndex((d) => d.id === id);
      if (index !== -1) {
        const updated = [...dashboards];
        updated[index] = updater(updated[index]);
        this.state.dashboardsByProject = {
          ...this.state.dashboardsByProject,
          [projectId]: updated,
        };
        return;
      }
    }
  }

  private updateWidgetState(
    dashboardId: string,
    widgetId: string,
    updates: Partial<DashboardWidget>,
  ): void {
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...updates } : w)),
    }));
  }

  // === VERSION HISTORY ===

  getVersionsForDashboard(dashboardId: string): DashboardVersion[] {
    for (const versions of Object.values(this.state.dashboardVersionsByProject)) {
      const filtered = versions.filter((v) => v.dashboardId === dashboardId);
      if (filtered.length > 0) return filtered;
    }
    return [];
  }

  getResolvedVersionsForDashboard(dashboardId: string): ResolvedDashboardVersion[] {
    const versions = this.getVersionsForDashboard(dashboardId);
    return resolveDashboardVersions(versions);
  }

  /**
   * Restore a dashboard to a previous version's snapshot state.
   */
  async restoreVersion(dashboardId: string, version: ResolvedDashboardVersion): Promise<void> {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard) return;

    // Capture current state as a version before restoring
    this.captureVersion(dashboard);

    const snapshot = version.dashboard;
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      name: snapshot.name,
      description: snapshot.description,
      widgets: snapshot.widgets as DashboardWidget[],
      viewport: snapshot.viewport,
      dateFilter: snapshot.dateFilter,
      updatedAt: new Date(),
    }));

    const updated = this.getDashboard(dashboardId);
    if (updated) await this.persistDashboard(updated);
  }

  private captureVersion(dashboard: Dashboard): void {
    if (!this.persistence) return;
    const projectId = dashboard.projectId;

    const versions = this.getVersionsForDashboard(dashboard.id);
    const nextVersion = versions.length > 0 ? Math.max(...versions.map((v) => v.version)) + 1 : 1;

    const newVersion = createDashboardVersionEntry(dashboard.id, nextVersion, dashboard);

    // Add to state
    const projectVersions = this.state.dashboardVersionsByProject[projectId] ?? [];
    this.state.dashboardVersionsByProject = {
      ...this.state.dashboardVersionsByProject,
      [projectId]: [...projectVersions, newVersion],
    };

    // Persist immediately
    this.persistence
      .persistDashboardVersion({
        id: newVersion.id,
        dashboardId: newVersion.dashboardId,
        version: newVersion.version,
        snapshot: newVersion.snapshot,
        createdAt: newVersion.createdAt.toISOString(),
      })
      .catch((err) =>
        console.error("[dashboard-manager] Failed to persist dashboard version:", err),
      );

    // Prune old versions if over limit
    const DEFAULT_VERSION_LIMIT = 100;
    const allVersions = this.state.dashboardVersionsByProject[projectId] ?? [];
    const dashVersions = allVersions.filter((v) => v.dashboardId === dashboard.id);
    if (dashVersions.length > DEFAULT_VERSION_LIMIT) {
      const sorted = [...dashVersions].sort((a, b) => b.version - a.version);
      const keepIds = new Set(sorted.slice(0, DEFAULT_VERSION_LIMIT).map((v) => v.id));
      this.state.dashboardVersionsByProject = {
        ...this.state.dashboardVersionsByProject,
        [projectId]: allVersions.filter((v) => v.dashboardId !== dashboard.id || keepIds.has(v.id)),
      };
    }

    this.persistence
      .pruneDashboardVersions(dashboard.id, DEFAULT_VERSION_LIMIT)
      .catch((err) =>
        console.error("[dashboard-manager] Failed to prune dashboard versions:", err),
      );
  }

  // === PERSISTENCE ===

  private async persistDashboard(dashboard: Dashboard): Promise<void> {
    try {
      const db = await getDatabase();
      await dashboardsRepo.save(db, toPersistedDashboard(dashboard));
    } catch (error) {
      void log.error("Failed to persist dashboard:", error);
    }
  }
}
