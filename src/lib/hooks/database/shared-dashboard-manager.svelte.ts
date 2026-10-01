import type { SharedDashboard, Dashboard } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import {
  SEAQUEL_DIR,
  parseCompositeId,
  type SharedRepoManager,
} from "./shared-repo-manager.svelte.js";
import { stripWidgetRuntimeState } from "./dashboard-manager.svelte.js";
import {
  serializeDashboardFile,
  dashboardNameToFilename,
} from "$lib/services/dashboard-file-parser";
import { nameToFilename } from "$lib/services/config-file-parser";
import { writeTextFile, remove, mkdir, exists } from "@tauri-apps/plugin-fs";
import { join, dirname } from "@tauri-apps/api/path";

export class SharedDashboardManager {
  constructor(
    private state: DatabaseState,
    private repoManager: SharedRepoManager,
  ) {}

  private getDashboardsBasePath(): string | null {
    const project = this.state.projects.find((p) => p.id === this.state.activeProjectId);
    if (!project) return null;
    const dirName = nameToFilename(project.name);
    return `${SEAQUEL_DIR}/projects/${dirName}/dashboards`;
  }

  async createDashboard(
    name: string,
    widgets: Dashboard["widgets"],
    viewport: Dashboard["viewport"],
    options?: {
      description?: string;
      dateFilter?: Dashboard["dateFilter"];
    },
  ): Promise<string | null> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return null;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return null;

    const dashboardsBase = this.getDashboardsBasePath();
    if (!dashboardsBase) return null;

    const filename = dashboardNameToFilename(name);
    const filePath = `${dashboardsBase}/${filename}`;

    const sharedDashboard: SharedDashboard = {
      id: `${repoId}:${filePath}`,
      repoId,
      filePath,
      name,
      description: options?.description,
      widgets,
      viewport,
      dateFilter: options?.dateFilter ?? null,
      updatedAt: new Date(),
    };

    const content = serializeDashboardFile(sharedDashboard);
    const fullPath = await join(repo.path, filePath);
    const folderPath = await dirname(fullPath);

    if (!(await exists(folderPath))) {
      await mkdir(folderPath, { recursive: true });
    }

    await writeTextFile(fullPath, content);

    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    this.state.sharedDashboardsByRepo = {
      ...this.state.sharedDashboardsByRepo,
      [repoId]: [...dashboards, sharedDashboard],
    };

    await this.repoManager.refreshRepoStatus(repoId);
    return sharedDashboard.id;
  }

  async deleteDashboard(dashboardId: string): Promise<boolean> {
    const { repoId, filePath } = parseCompositeId(dashboardId);

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return false;

    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    const dashboard = dashboards.find((d) => d.id === dashboardId);
    if (!dashboard) return false;

    const fullPath = await join(repo.path, filePath);
    await remove(fullPath);

    this.state.sharedDashboardsByRepo = {
      ...this.state.sharedDashboardsByRepo,
      [repoId]: dashboards.filter((d) => d.id !== dashboardId),
    };

    await this.repoManager.refreshRepoStatus(repoId);
    return true;
  }

  getDashboard(dashboardId: string): SharedDashboard | null {
    const { repoId } = parseCompositeId(dashboardId);
    const dashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    return dashboards.find((d) => d.id === dashboardId) ?? null;
  }

  /**
   * Write a dashboard as a .json file in the git repo.
   */
  async writeDashboardFile(dashboard: Dashboard): Promise<void> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const dashboardsBase = this.getDashboardsBasePath();
    if (!dashboardsBase) return;

    const filename = dashboardNameToFilename(dashboard.name);
    const filePath = `${dashboardsBase}/${filename}`;

    const content = serializeDashboardFile(dashboard);
    const fullPath = await join(repo.path, filePath);
    const folderPath = await dirname(fullPath);

    if (!(await exists(folderPath))) {
      await mkdir(folderPath, { recursive: true });
    }

    await writeTextFile(fullPath, content);
    await this.repoManager.refreshRepoStatus(repoId);
  }

  /**
   * Delete the .json file for a dashboard from the git repo.
   */
  async deleteDashboardFile(dashboard: Dashboard): Promise<void> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const dashboardsBase = this.getDashboardsBasePath();
    if (!dashboardsBase) return;

    const filename = dashboardNameToFilename(dashboard.name);
    const filePath = `${dashboardsBase}/${filename}`;
    const fullPath = await join(repo.path, filePath);

    try {
      await remove(fullPath);
    } catch {
      // File may not exist
    }

    await this.repoManager.refreshRepoStatus(repoId);
  }

  async shareDashboard(dashboard: Dashboard): Promise<string | null> {
    // Strip runtime state from widgets
    const widgets = dashboard.widgets.map(stripWidgetRuntimeState);
    return this.createDashboard(dashboard.name, widgets, dashboard.viewport, {
      dateFilter: dashboard.dateFilter,
    });
  }

  async unshareDashboard(dashboardId: string): Promise<boolean> {
    return this.deleteDashboard(dashboardId);
  }

  /**
   * Reconcile .json files from the scan cache with SQLite dashboards.
   * - New .json files → create Dashboard with shared=true
   * - Missing .json files for shared dashboards → set shared=false
   * - Updated .json files → update dashboard content
   * Returns the list of dashboards after reconciliation: the same array when
   * nothing changed, and an unchanged dashboard as the same object.
   */
  reconcileWithGitFiles(projectId: string, dashboards: Dashboard[]): Dashboard[] {
    const repoId = this.state.activeRepoId;
    if (!repoId) return dashboards;

    const dashboardsBase = this.getDashboardsBasePath();
    if (!dashboardsBase) return dashboards;

    const allScannedDashboards = this.state.sharedDashboardsByRepo[repoId] ?? [];
    const gitDashboards = allScannedDashboards.filter((d) =>
      d.filePath?.startsWith(dashboardsBase + "/"),
    );

    const result = [...dashboards];
    /** The dashboards (by index) a file matched: the rest that are shared get unshared. */
    const matched = new Set<number>();
    let changed = false;

    // Pair each file with a shared dashboard in two passes:
    // 1. by the file's `name` field, case-insensitively, so dashboards whose
    //    names slug to one path (non-Latin names, punctuation) each keep
    //    their own file;
    // 2. for files and shared dashboards still unpaired, by the path each
    //    dashboard would be written to (`dashboardNameToFilename`, as
    //    `writeDashboardFile` and `deleteDashboardFile` build it), so a
    //    case-only rename on either side keeps the pair.
    // A file still unpaired is a new one; if a local dashboard has its
    // name, Core refuses it and `storeReconciled` says so.
    const pairs = new Map<SharedDashboard, number>();
    for (const gitDashboard of gitDashboards) {
      const key = gitDashboard.name.toLowerCase();
      const idx = result.findIndex(
        (d, i) => !matched.has(i) && d.shared && d.name.toLowerCase() === key,
      );
      if (idx !== -1) {
        matched.add(idx);
        pairs.set(gitDashboard, idx);
      }
    }
    for (const gitDashboard of gitDashboards) {
      if (pairs.has(gitDashboard)) continue;
      const file = gitDashboard.filePath?.split("/").pop();
      const idx = result.findIndex(
        (d, i) => !matched.has(i) && d.shared && dashboardNameToFilename(d.name) === file,
      );
      if (idx !== -1) {
        matched.add(idx);
        pairs.set(gitDashboard, idx);
      }
    }

    for (const gitDashboard of gitDashboards) {
      const existingIdx = pairs.get(gitDashboard) ?? -1;

      const updatedAt =
        gitDashboard.updatedAt instanceof Date
          ? gitDashboard.updatedAt
          : gitDashboard.updatedAt
            ? new Date(gitDashboard.updatedAt)
            : new Date();

      if (existingIdx !== -1) {
        const existing = result[existingIdx];
        // The file's content is already the dashboard's: nothing to store.
        if (sameContent(existing, gitDashboard)) continue;
        changed = true;
        result[existingIdx] = {
          ...existing,
          widgets: gitDashboard.widgets,
          viewport: gitDashboard.viewport,
          description: gitDashboard.description,
          dateFilter: gitDashboard.dateFilter,
          updatedAt,
        };
      } else {
        changed = true;
        // New .json file → a new shared Dashboard. Its id is a placeholder
        // Core replaces (`storeReconciled` creates it and shows Core's row;
        // this list itself is never shown).
        const newDashboard: Dashboard = {
          id: `file:${gitDashboard.filePath ?? gitDashboard.name}`,
          name: gitDashboard.name,
          projectId,
          widgets: gitDashboard.widgets,
          viewport: gitDashboard.viewport,
          dateFilter: gitDashboard.dateFilter,
          createdAt: new Date(),
          updatedAt,
          shared: true,
          starred: false,
          description: gitDashboard.description,
        };
        result.push(newDashboard);
        matched.add(result.length - 1);
      }
    }

    // Shared dashboards in SQLite with no matching .json file → mark as unshared
    for (let i = 0; i < result.length; i++) {
      const d = result[i];
      if (d.shared && !matched.has(i)) {
        result[i] = { ...d, shared: false };
        changed = true;
      }
    }

    return changed ? result : dashboards;
  }
}

/** Whether a git file holds what the dashboard already has (the fields the reconcile copies). */
function sameContent(dashboard: Dashboard, file: SharedDashboard): boolean {
  const content = (d: Pick<Dashboard, "widgets" | "viewport" | "description" | "dateFilter">) =>
    JSON.stringify([
      d.widgets.map(stripWidgetRuntimeState),
      d.viewport,
      d.description ?? null,
      d.dateFilter ?? null,
    ]);
  return content(dashboard) === content(file);
}
