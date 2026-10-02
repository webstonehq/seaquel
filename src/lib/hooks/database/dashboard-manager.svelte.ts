import type {
  Dashboard,
  DashboardWidget,
  DashboardVersion,
  ResolvedDashboardVersion,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import { log } from "$lib/utils/logger";
import { errorToast } from "$lib/utils/toast";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { stripWidgetRuntimeState } from "./dashboard-serialize.js";
import { errorCode } from "$lib/core/client";
import {
  getLibrary,
  rowKey,
  DASHBOARD_NOT_FOUND,
  DASHBOARD_VERSION_NOT_FOUND,
  NEW,
  type DashboardDraft,
  type DashboardPatch,
  type DashboardUpdated,
} from "./library/index.js";
import {
  dashboardFromWire,
  dashboardVersionFromWire,
  resolvedDashboardVersionFromWire,
} from "./library/convert.js";
import { libraryErrorMessage, limitMessage, limitOf } from "./library/messages.js";
import { closeDashboardTabs } from "./connection-tabs-cleanup.js";
import { reportProjection } from "./shared/projection.js";

export { stripWidgetRuntimeState } from "./dashboard-serialize.js";

function runKey(dashboardId: string, widgetId: string): string {
  return `${dashboardId}:${widgetId}`;
}

/** The widgets as stored: without their run state (rows, loading, error). */
function storedWidgets(widgets: readonly DashboardWidget[]) {
  return widgets.map(stripWidgetRuntimeState);
}

/**
 * Manages dashboard CRUD operations, widget execution, and auto-refresh.
 * Dashboards are per-project.
 *
 * Phase 5d-2 (Decision 21): every change is one `library` call. Core makes
 * the dashboard's id, checks its name (`NAME_TAKEN` within the project),
 * and applies a patch of only the fields the edit changed; the page says
 * which edits are versioned (`captureVersion`: a rename, a widget added,
 * changed or removed, a date filter, a restore; not a move, resize, pan or
 * zoom), and Core numbers and prunes the versions inside the call. The page
 * shows its edit at once and splices the answer's version and prune into
 * its list. A refused edit stays on screen and the dashboard is marked
 * unsaved; another window's change to it then shows the "changed in
 * another window" banner instead of replacing it.
 */
export class DashboardManager {
  private autoRefreshTimers = new Map<string, ReturnType<typeof setInterval>>();
  /**
   * The run in flight per widget (`dashboardId:widgetId`). Auto-refresh skips
   * a widget whose last run hasn't finished, so a query blocked on a lock
   * doesn't queue another waiter every tick; a manual run replaces it.
   * Closing the dashboard or removing the widget aborts it.
   */
  private runs = new Map<string, AbortController>();
  /** Dashboards refused as too large (Decision 27), told once each. */
  private toldTooLarge = new Set<string>();
  /** The pane manager's `syncGlobalActiveState`, for closing a deleted dashboard's tabs. */
  private syncActive?: (tabId: string) => void;

  /**
   * @param runReadOnly `executeReadOnly`: every widget query runs read-only
   *   (the AI writes widgets, and shared dashboards come from a git repo).
   */
  constructor(
    private state: DatabaseState,
    private runReadOnly: (
      connectionId: string,
      sql: string,
      signal?: AbortSignal,
    ) => Promise<Record<string, unknown>[]>,
    private scheduleProjectPersistence: (projectId: string | null) => void,
  ) {}

  setSyncActive(fn: (tabId: string) => void): void {
    this.syncActive = fn;
  }

  // === CRUD ===

  /**
   * Save a new dashboard in the active project. Core gives it its id and
   * checks its name; `renameIfTaken` ("New Dashboard", the AI's) takes the
   * next free `"<name> (n)"` instead of `NAME_TAKEN`. `null` when it wasn't
   * saved (the refusal is shown).
   */
  async createDashboard(
    name: string,
    { renameIfTaken = false }: { renameIfTaken?: boolean } = {},
  ): Promise<Dashboard | null> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return null;

    let dashboard: Dashboard;
    try {
      dashboard = await this.create(projectId, {
        projectId,
        name,
        widgets: [],
        viewport: { x: 0, y: 0, zoom: 1 },
        ...(renameIfTaken ? { renameIfTaken } : {}),
      });
    } catch (error) {
      this.saveFailed(null, name, error);
      return null;
    }
    return dashboard;
  }

  /** One `dashboardCreate`, shown in its project's list (once) as Core answered it. */
  private async create(projectId: string, draft: DashboardDraft): Promise<Dashboard> {
    const seqs = this.state.librarySeqs;
    const answer = await seqs.write([rowKey("dashboard", NEW)], () =>
      getLibrary().createDashboard(draft),
    );
    seqs.note(rowKey("dashboard", answer.value.id), answer.seq);
    const dashboard = dashboardFromWire(answer.value);
    this.show(projectId, dashboard);
    reportProjection(answer, projectId);
    return dashboard;
  }

  /** Puts `dashboard` in its project's list, replacing a copy with its id. */
  private show(projectId: string, dashboard: Dashboard): void {
    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.some((d) => d.id === dashboard.id)
        ? dashboards.map((d) => (d.id === dashboard.id ? dashboard : d))
        : [...dashboards, dashboard],
    };
  }

  async deleteDashboard(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    // One call: Core deletes a shared dashboard's file first (Decision 37);
    // a refusal leaves the dashboard as stored.
    let answer;
    try {
      answer = await this.state.librarySeqs.write([rowKey("dashboard", id)], () =>
        getLibrary().removeDashboard(id),
      );
      this.state.librarySeqs.note(rowKey("dashboard", id), answer.seq);
    } catch (error) {
      void log.error("Failed to delete dashboard:", error);
      errorToast(m.dashboard_delete_failed({ message: libraryErrorMessage(error) }));
      return;
    }

    // Stop any auto-refresh timers
    const dashboard = this.getDashboard(id);
    this.forget(id);
    if (dashboard) this.closeDashboard(id);
    reportProjection(answer, dashboard?.projectId ?? projectId, { removal: true });
  }

  /**
   * Rename a dashboard. A refusal (`NAME_TAKEN`, a limit) is shown and the
   * old name comes back, on the dashboard and on the tabs showing it
   * (which the header renamed first). False when it wasn't saved.
   */
  async renameDashboard(id: string, name: string): Promise<boolean> {
    const before = this.getDashboard(id);
    if (!before) return false;
    this.updateDashboard(id, (d) => ({ ...d, name, updatedAt: new Date() }));
    // A refusal puts the stored name back, on the tabs too (`save`).
    return this.save(id, { name, captureVersion: true }, before);
  }

  /** The dashboard tabs showing `id`, in every project, named `name`. */
  private renameTabs(id: string, name: string): void {
    const byProject = this.state.dashboardTabsByProject ?? {};
    const stale = (t: { dashboardId: string; name: string }) =>
      t.dashboardId === id && t.name !== name;
    const touched = Object.keys(byProject).filter((p) => byProject[p].some(stale));
    if (touched.length === 0) return;
    this.state.dashboardTabsByProject = Object.fromEntries(
      Object.entries(byProject).map(([projectId, tabs]) => [
        projectId,
        tabs.map((t) => (t.dashboardId === id ? { ...t, name } : t)),
      ]),
    );
    // The tab names are view state: saved with it.
    for (const projectId of touched) this.scheduleProjectPersistence(projectId);
  }

  // === WIDGET MANAGEMENT ===

  async addWidget(dashboardId: string, widget: DashboardWidget): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: [...d.widgets, widget],
      updatedAt: new Date(),
    }));
    await this.saveWidgets(dashboardId, true, before);
  }

  async updateWidget(
    dashboardId: string,
    widgetId: string,
    updates: Partial<DashboardWidget>,
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...updates } : w)),
      updatedAt: new Date(),
    }));
    await this.saveWidgets(dashboardId, true, before);
  }

  async removeWidget(dashboardId: string, widgetId: string): Promise<void> {
    this.stopAutoRefresh(dashboardId, widgetId);
    this.abortRuns(dashboardId, widgetId);
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.filter((w) => w.id !== widgetId),
      updatedAt: new Date(),
    }));
    await this.saveWidgets(dashboardId, true, before);
  }

  async moveWidget(
    dashboardId: string,
    widgetId: string,
    position: { x: number; y: number },
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...position } : w)),
      updatedAt: new Date(),
    }));
    // A move isn't versioned.
    await this.saveWidgets(dashboardId, false, before);
  }

  async resizeWidget(
    dashboardId: string,
    widgetId: string,
    size: { width: number; height: number },
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      widgets: d.widgets.map((w) => (w.id === widgetId ? { ...w, ...size } : w)),
      updatedAt: new Date(),
    }));
    await this.saveWidgets(dashboardId, false, before);
  }

  // === VIEWPORT ===

  async updateViewport(
    dashboardId: string,
    viewport: { x: number; y: number; zoom: number },
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      viewport,
      updatedAt: new Date(),
    }));
    // A pan or zoom isn't versioned.
    if (before) await this.save(dashboardId, { viewport }, before);
  }

  // === WIDGET EXECUTION ===

  /**
   * Runs a widget query read-only on the active connection. A widget has no
   * connection of its own; it renders against whichever is active, and the
   * query is checked against that connection's engine. The widget editor's
   * preview and the version diff view run through this too.
   */
  async runWidgetQuery(query: string, signal?: AbortSignal): Promise<Record<string, unknown>[]> {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) throw new Error("Not connected to database");
    return await this.runReadOnly(connectionId, query, signal);
  }

  /**
   * Runs a widget's query and stores the result. A run already in flight for
   * the widget is aborted and its result dropped.
   *
   * @param signal Aborting it cancels the query too (the AI's Stop, for a
   *   widget a tool added).
   */
  async executeWidget(dashboardId: string, widgetId: string, signal?: AbortSignal): Promise<void> {
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

    const key = runKey(dashboardId, widgetId);
    const controller = new AbortController();
    const previous = this.runs.get(key);
    this.runs.set(key, controller);
    previous?.abort();
    const onAbort = () => controller.abort();
    if (signal?.aborted) controller.abort();
    signal?.addEventListener("abort", onAbort, { once: true });
    const current = () => this.runs.get(key) === controller;

    // Set loading state
    this.updateWidgetState(dashboardId, widgetId, { isLoading: true, error: undefined });

    try {
      const result = await this.runWidgetQuery(query, controller.signal);
      if (current()) {
        this.updateWidgetState(dashboardId, widgetId, {
          result,
          isLoading: false,
          lastRefreshed: new Date(),
        });
      }
    } catch (error) {
      if (current()) {
        this.updateWidgetState(dashboardId, widgetId, {
          isLoading: false,
          error: controller.signal.aborted
            ? "Query cancelled"
            : error instanceof Error
              ? error.message
              : "Query execution failed",
        });
      }
    } finally {
      signal?.removeEventListener("abort", onAbort);
      if (current()) this.runs.delete(key);
    }
  }

  /** Aborts the runs in flight for a dashboard, or for one of its widgets. */
  private abortRuns(dashboardId: string, widgetId?: string): void {
    const prefix = widgetId === undefined ? `${dashboardId}:` : runKey(dashboardId, widgetId);
    for (const [key, controller] of this.runs) {
      if (widgetId === undefined ? key.startsWith(prefix) : key === prefix) controller.abort();
    }
  }

  /**
   * The dashboard's tab closed: stop its auto-refresh timers and abort the
   * widget runs in flight.
   */
  closeDashboard(dashboardId: string): void {
    for (const key of this.autoRefreshTimers.keys()) {
      if (key.startsWith(`${dashboardId}:`)) {
        clearInterval(this.autoRefreshTimers.get(key));
        this.autoRefreshTimers.delete(key);
      }
    }
    this.abortRuns(dashboardId);
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

    const timerKey = runKey(dashboardId, widgetId);
    this.stopAutoRefresh(dashboardId, widgetId);

    const timer = setInterval(() => {
      // Still waiting on the last run (a lock, a slow server): skip this tick.
      if (this.runs.has(timerKey)) return;
      void this.executeWidget(dashboardId, widgetId);
    }, widget.autoRefreshSeconds * 1000);

    this.autoRefreshTimers.set(timerKey, timer);
  }

  stopAutoRefresh(dashboardId: string, widgetId: string): void {
    const timerKey = runKey(dashboardId, widgetId);
    const timer = this.autoRefreshTimers.get(timerKey);
    if (timer) {
      clearInterval(timer);
      this.autoRefreshTimers.delete(timerKey);
    }
  }

  /** Stops every timer and aborts every widget run (project switch, app teardown). */
  stopAllAutoRefresh(): void {
    for (const timer of this.autoRefreshTimers.values()) {
      clearInterval(timer);
    }
    this.autoRefreshTimers.clear();
    for (const controller of this.runs.values()) controller.abort();
  }

  // === DATE FILTER ===

  async setDateFilter(
    dashboardId: string,
    range: { start: string; end: string } | null,
  ): Promise<void> {
    const before = this.getDashboard(dashboardId);
    this.updateDashboard(dashboardId, (d) => ({
      ...d,
      dateFilter: range,
      updatedAt: new Date(),
    }));
    if (before) {
      await this.save(dashboardId, { dateFilter: range, captureVersion: true }, before);
      await this.executeAllWidgets(dashboardId);
    }
  }

  // === OTHER WINDOWS ===

  /**
   * Another window changed dashboards of `projectId` (a `dashboard` event):
   * read its dashboards and versions again if the page holds them, and
   * apply each row by the `seq` rule once this page's own writes to it have
   * answered (its widgets keep their rows). A refused edit here was taken
   * back, so the page never holds an unsaved change another window's could
   * clash with. One deleted elsewhere stops its runs and closes its tabs in
   * every project.
   */
  async refreshFromLibrary(
    projectId: string,
    ids: readonly string[] | null,
    { again = true } = {},
  ): Promise<void> {
    if (!(projectId in this.state.dashboardsByProject)) return;
    const seqs = this.state.librarySeqs;
    await Promise.all((ids ?? [""]).map((id) => seqs.settled(rowKey("dashboard", id))));
    const library = getLibrary();
    const [dashboards, versions] = await Promise.all([
      library.listDashboards(projectId),
      library.listDashboardVersions(projectId),
    ]);
    const stored = new Map(dashboards.value.map((d) => [d.id, d]));
    const shown = this.state.dashboardsByProject[projectId] ?? [];
    const wanted = ids === null ? null : new Set(ids);
    const removed: Dashboard[] = [];
    let next = [...shown];
    /** Rows with a write of this page on its way: read again once it answers. */
    const skipped: string[] = [];
    /** Dashboards another window renamed: their tabs here follow. */
    const renamed: [string, string][] = [];
    for (const id of new Set([...shown.map((d) => d.id), ...stored.keys()])) {
      if (wanted && !wanted.has(id)) continue;
      if (seqs.busy(rowKey("dashboard", id))) {
        skipped.push(id);
        continue;
      }
      if (!seqs.take(rowKey("dashboard", id), dashboards.seq)) continue;
      const row = stored.get(id);
      const current = next.find((d) => d.id === id);
      if (!row) {
        if (current) removed.push(current);
        next = next.filter((d) => d.id !== id);
      } else {
        const dashboard = dashboardFromWire(row, current);
        next = current ? next.map((d) => (d.id === id ? dashboard : d)) : [...next, dashboard];
        if (current && current.name !== dashboard.name) renamed.push([id, dashboard.name]);
      }
    }
    this.state.dashboardsByProject = { ...this.state.dashboardsByProject, [projectId]: next };
    if (seqs.take(rowKey("dashboardVersion", projectId), versions.seq)) {
      this.state.dashboardVersionsByProject = {
        ...this.state.dashboardVersionsByProject,
        [projectId]: versions.value.map(dashboardVersionFromWire),
      };
    }
    for (const [id, name] of renamed) this.renameTabs(id, name);
    for (const dashboard of removed) this.removedElsewhere(dashboard);
    if (again && skipped.length > 0) {
      await Promise.all(skipped.map((id) => seqs.settled(rowKey("dashboard", id))));
      await this.refreshFromLibrary(projectId, skipped, { again: false });
    }
  }

  /** A dashboard deleted in another window: its runs stop and its tabs close. */
  private removedElsewhere(dashboard: Dashboard): void {
    this.closeDashboard(dashboard.id);
    const touched = closeDashboardTabs(this.state, dashboard.id, this.syncActive);
    for (const projectId of touched) this.scheduleProjectPersistence(projectId);
    if (touched.length > 0) toast.info(m.dashboard_removed_elsewhere({ name: dashboard.name }));
  }

  // === STARRING ===

  /** Star or unstar a dashboard. The star is on the dashboard's own row. */
  async toggleDashboardStarred(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    const dashboard = dashboards.find((d) => d.id === id);
    if (!dashboard) return;
    const updated = { ...dashboard, starred: !dashboard.starred };
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.map((d) => (d.id === id ? updated : d)),
    };
    // Alone, the star keeps `updated_at` (Core's rule).
    await this.save(id, { starred: !!updated.starred }, dashboard);
  }

  /** @deprecated Use toggleDashboardStarred instead */
  toggleSharedDashboardStarred(id: string): Promise<void> {
    return this.toggleDashboardStarred(id);
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

    // One call: Core writes its file after the row (Decision 36).
    await this.save(id, { shared: true }, dashboard);
    this.scheduleProjectPersistence(projectId);
  }

  async unshareDashboardById(id: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const dashboards = this.state.dashboardsByProject[projectId] ?? [];
    const dashboard = dashboards.find((d) => d.id === id);
    if (!dashboard || !dashboard.shared) return;

    const updated = { ...dashboard, shared: false, updatedAt: new Date() };
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards.map((d) => (d.id === id ? updated : d)),
    };

    // One call: Core deletes its file before the row (Decision 37).
    await this.save(id, { shared: false }, dashboard);
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

  /**
   * One version with its snapshot, for the history's diff and restore
   * (5d-2 Task 7: the list holds no snapshots). `null` when it can't be
   * read, which is shown; a version pruned or a dashboard removed elsewhere
   * also reads the project's versions again, so the history drops it.
   */
  async loadVersion(
    dashboardId: string,
    versionId: string,
  ): Promise<ResolvedDashboardVersion | null> {
    try {
      const { value } = await getLibrary().getDashboardVersion(dashboardId, versionId);
      const resolved = resolvedDashboardVersionFromWire(value);
      if (!resolved) errorToast(m.dashboard_version_unreadable({ version: value.version }));
      return resolved;
    } catch (error) {
      void log.error("Failed to read a dashboard version:", error);
      errorToast(m.dashboard_version_open_failed({ message: libraryErrorMessage(error) }));
      const code = errorCode(error);
      if (code === DASHBOARD_VERSION_NOT_FOUND || code === DASHBOARD_NOT_FOUND) {
        const projectId = this.projectOf(dashboardId);
        if (projectId) void this.refreshFromLibrary(projectId, [dashboardId]).catch(() => {});
      }
      return null;
    }
  }

  /** The project holding the dashboard (or its versions), if the page holds it. */
  private projectOf(dashboardId: string): string | null {
    for (const [projectId, dashboards] of Object.entries(this.state.dashboardsByProject)) {
      if (dashboards.some((d) => d.id === dashboardId)) return projectId;
    }
    for (const [projectId, versions] of Object.entries(this.state.dashboardVersionsByProject)) {
      if (versions.some((v) => v.dashboardId === dashboardId)) return projectId;
    }
    return null;
  }

  /**
   * Restore a dashboard to a previous version's snapshot state. The state
   * it had is versioned first, by Core.
   */
  async restoreVersion(dashboardId: string, version: ResolvedDashboardVersion): Promise<void> {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard) return;

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

    await this.save(
      dashboardId,
      {
        name: snapshot.name,
        description: snapshot.description ?? null,
        widgets: storedWidgets(snapshot.widgets as DashboardWidget[]),
        viewport: snapshot.viewport,
        dateFilter: snapshot.dateFilter ?? null,
        captureVersion: true,
      },
      dashboard,
    );
  }

  // === PERSISTENCE ===

  /** Saves the dashboard's widgets as shown; `version` for an edit that is versioned. */
  private async saveWidgets(
    dashboardId: string,
    version: boolean,
    before: Dashboard | undefined,
  ): Promise<void> {
    const dashboard = this.getDashboard(dashboardId);
    if (!dashboard || !before) return;
    await this.save(
      dashboardId,
      {
        widgets: storedWidgets(dashboard.widgets),
        ...(version ? { captureVersion: true } : {}),
      },
      before,
    );
  }

  /**
   * One `dashboardUpdate` with `patch` (the edit is already shown). The
   * answer's version, and the versions Core pruned, go into the page's list;
   * the dashboard as shown stays (its widgets hold their rows). False when
   * it wasn't saved: the refusal is shown and the fields the patch named go
   * back to `before` (the dashboard before the edit), as the library's
   * refusals do, so what the page shows is what is stored.
   */
  private async save(id: string, patch: DashboardPatch, before: Dashboard): Promise<boolean> {
    const dashboard = this.getDashboard(id);
    if (!dashboard) return false;
    const seqs = this.state.librarySeqs;
    let updated: DashboardUpdated;
    let answer;
    try {
      answer = await seqs.write([rowKey("dashboard", id)], () =>
        getLibrary().updateDashboard(id, patch),
      );
      seqs.note(rowKey("dashboard", id), answer.seq);
      updated = answer.value;
    } catch (error) {
      this.saveFailed(id, before.name, error);
      await this.restoreStored(id, patch, before);
      return false;
    }
    this.toldTooLarge.delete(id);
    this.spliceVersions(dashboard.projectId, updated);
    // Core writes a shared dashboard's file from the stored row inside the
    // call (bug 1; not for a pan or zoom alone, Q24).
    reportProjection(answer, dashboard.projectId, { removal: patch.shared === false });
    // The stored row, which holds other windows' changes to other fields,
    // shows unless a later edit here is still on its way (its answer will).
    if (!seqs.busy(rowKey("dashboard", id))) {
      this.updateDashboard(id, (d) => dashboardFromWire(updated.dashboard, d));
      this.renameTabs(id, updated.dashboard.name);
    }
    return true;
  }

  /**
   * After a refused edit: once this page's writes to the dashboard have
   * answered, the fields the refused patch named show the stored row again
   * (another refused edit in flight may have shown its own change since
   * `before` was taken; none of them is stored). The tabs follow a name.
   * If the row can't be read, they go back to `before`.
   */
  private async restoreStored(id: string, patch: DashboardPatch, before: Dashboard): Promise<void> {
    const seqs = this.state.librarySeqs;
    const key = rowKey("dashboard", id);
    let stored: Dashboard | null = null;
    try {
      await seqs.settled(key);
      const { value, seq } = await getLibrary().listDashboards(before.projectId);
      // An edit made since, still on its way or answered after this read
      // was taken, shows what's newer: this read isn't applied over it.
      const last = seqs.last(key);
      if (seqs.busy(key) || (seq.epoch === seqs.epoch && last !== undefined && seq.n < last)) {
        return;
      }
      const row = value.find((d) => d.id === id);
      stored = row ? dashboardFromWire(row) : null;
    } catch (error) {
      void log.warn("Reading a dashboard again after a refused edit failed:", error);
    }
    this.revert(id, patch, stored ?? before);
    if (patch.name !== undefined) this.renameTabs(id, (stored ?? before).name);
  }

  /** Puts back the fields `patch` named, as they are in `before`. */
  private revert(id: string, patch: DashboardPatch, before: Dashboard): void {
    this.updateDashboard(id, (d) => {
      const next = { ...d };
      if (patch.name !== undefined) next.name = before.name;
      if (patch.description !== undefined) next.description = before.description;
      if (patch.widgets !== undefined) {
        // Widgets keep the run state they have now (a widget added by the
        // refused edit goes, with its runs).
        const live = new Map(d.widgets.map((w) => [w.id, w]));
        next.widgets = before.widgets.map((w) => {
          const now = live.get(w.id);
          return now
            ? {
                ...w,
                result: now.result,
                isLoading: now.isLoading,
                error: now.error,
                lastRefreshed: now.lastRefreshed,
              }
            : w;
        });
        for (const w of d.widgets) {
          if (!before.widgets.some((b) => b.id === w.id)) {
            this.stopAutoRefresh(id, w.id);
            this.abortRuns(id, w.id);
          }
        }
      }
      if (patch.viewport !== undefined) next.viewport = before.viewport;
      if (patch.dateFilter !== undefined) next.dateFilter = before.dateFilter;
      if (patch.starred !== undefined) next.starred = before.starred;
      if (patch.shared !== undefined) next.shared = before.shared;
      next.updatedAt = before.updatedAt;
      return next;
    });
  }

  /** The answer's new version in, the pruned ones out. */
  private spliceVersions(projectId: string, updated: DashboardUpdated): void {
    const { version, prunedVersionIds } = updated;
    if (!version && prunedVersionIds.length === 0) return;
    const pruned = new Set(prunedVersionIds);
    const versions = (this.state.dashboardVersionsByProject[projectId] ?? []).filter(
      (v) => !pruned.has(v.id) && v.id !== version?.id,
    );
    if (version) versions.push(dashboardVersionFromWire(version));
    this.state.dashboardVersionsByProject = {
      ...this.state.dashboardVersionsByProject,
      [projectId]: versions,
    };
  }

  /**
   * A refused save, shown. Past the web's `max_dashboard_bytes` it says so
   * once per dashboard, naming it and the limit.
   */
  private saveFailed(id: string | null, name: string, error: unknown): void {
    void log.error("Failed to save a dashboard:", error);
    const limit = limitOf(error);
    const other = limit ? limitMessage(limit) : null;
    if (other) {
      errorToast(m.dashboard_save_failed({ message: other }));
      return;
    }
    if (limit) {
      const key = id ?? `new:${name}`;
      if (this.toldTooLarge.has(key)) return;
      this.toldTooLarge.add(key);
      errorToast(m.dashboard_too_large({ name, limit }));
      return;
    }
    errorToast(
      m.dashboard_save_failed({
        message: libraryErrorMessage(error, (taken) => this.getDashboard(taken)?.name),
      }),
    );
  }

  /** Drops a dashboard from the page's lists. */
  private forget(id: string): void {
    for (const [projectId, dashboards] of Object.entries(this.state.dashboardsByProject)) {
      if (!dashboards.some((d) => d.id === id)) continue;
      this.state.dashboardsByProject = {
        ...this.state.dashboardsByProject,
        [projectId]: dashboards.filter((d) => d.id !== id),
      };
    }
  }
}
