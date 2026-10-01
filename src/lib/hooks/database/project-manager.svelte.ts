import type {
  Dashboard,
  Project,
  ConnectionLabel,
  DatabaseConnection,
  Query,
  SharedProject,
  SharedConnection,
} from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { WindowStateManager } from "./window-state.svelte.js";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import { SEAQUEL_DIR, type SharedRepoManager } from "./shared-repo-manager.svelte.js";
import type { SharedQueryManager } from "./shared-query-manager.svelte.js";
import type { SharedDashboardManager } from "./shared-dashboard-manager.svelte.js";
import type { StarterTabManager } from "./starter-tabs.svelte.js";
import { isTauri } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import type { ConnectionManager } from "./connection-manager.svelte.js";
import { connectionDraft } from "./connection-manager.svelte.js";
import {
  NEW,
  getLibrary,
  rowKey,
  type ChangeSeq,
  type ProjectPatch,
  type WireProject,
} from "./library/index.js";
import {
  projectFromWire,
  savedQueryFromWire,
  savedQueryPatch,
  workflowSummaryFromWire,
} from "./library/convert.js";
import { LAST_PROJECT } from "./library/types.js";
import { libraryError, libraryErrorMessage } from "./library/messages.js";
import {
  applyConnectionRow,
  bumpRevisions,
  libraryNameOf,
  refreshConnectionOrder,
  refreshQueryVersions,
  storeConnectionOrder,
} from "./library/view.js";
import { errorToast } from "$lib/utils/toast";
import { errorCode } from "$lib/core/client";
import { mkdir, rename as renameFs, exists, writeTextFile } from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { nameToFilename, serializeProjectFile } from "$lib/services/config-file-parser";
import type { PersistedWorkflowTab } from "$lib/types/persisted";
import type { PersistedProjectState } from "$lib/types/project";
import { toPersistedDashboard } from "./dashboard-serialize.js";

/** A dashboard as stored, for telling which ones a change touched. */
function storedForm(dashboard: Dashboard): string {
  return JSON.stringify(toPersistedDashboard(dashboard));
}

/** Legacy persisted state from before canvas→workflow rename */
export interface LegacyPersistedProjectState extends PersistedProjectState {
  canvasTabs?: PersistedWorkflowTab[];
  activeCanvasTabId?: string | null;
}

/**
 * Manages projects and their lifecycle.
 * Projects group connections and provide organization.
 */
export class ProjectManager {
  private removeConnection:
    | ((connectionId: string, options?: { skipUnshare?: boolean }) => Promise<void>)
    | null = null;
  private starterTabManager: StarterTabManager | null = null;
  private sharedRepos: SharedRepoManager | null = null;
  private sharedQueryManager: SharedQueryManager | null = null;
  private sharedDashboardManager: SharedDashboardManager | null = null;
  private connectionManager: ConnectionManager | null = null;

  /** Told when a project is deleted (its tabs' runs are cancelled) and when the active one changes. */
  private lifecycle: {
    removed?: (id: string) => void;
    activated?: (id: string | null) => void;
    reloading?: (id: string) => void;
  } = {};

  constructor(
    private state: DatabaseState,
    private windowState: WindowStateManager,
    private stateRestoration: StateRestorationManager,
    /** The reconcile's dashboard saves (Core calls, phase 5d-2). */
    private dashboardStore?: Pick<DashboardManager, "storeReconciled">,
  ) {
    // A stale save with nothing changed here reads the window's state again.
    windowState.setReloader((projectId) => this.reloadViewState(projectId));
  }

  setLifecycleListener(listener: {
    removed?: (id: string) => void;
    activated?: (id: string | null) => void;
    /** A project's tabs are about to be replaced from storage: cancel their runs. */
    reloading?: (id: string) => void;
  }): void {
    this.lifecycle = listener;
  }

  /**
   * Set the shared repo manager reference.
   * Called by the main database class after SharedRepoManager is created.
   */
  setSharedRepoManager(manager: SharedRepoManager): void {
    this.sharedRepos = manager;
  }

  /**
   * Set the shared query manager reference.
   * Called by the main database class after SharedQueryManager is created.
   */
  setSharedQueryManager(manager: SharedQueryManager): void {
    this.sharedQueryManager = manager;
  }

  /**
   * Set the shared dashboard manager reference.
   * Called by the main database class after SharedDashboardManager is created.
   */
  setSharedDashboardManager(manager: SharedDashboardManager): void {
    this.sharedDashboardManager = manager;
  }

  /**
   * Set the callback for removing connections.
   * This is called by the main database class after ConnectionManager is created.
   */
  setRemoveConnectionCallback(
    callback: (connectionId: string, options?: { skipUnshare?: boolean }) => Promise<void>,
  ): void {
    this.removeConnection = callback;
  }

  /**
   * Set the connection manager reference: a removed project's connections
   * are disconnected and forgotten through it.
   */
  setConnectionManager(manager: ConnectionManager): void {
    this.connectionManager = manager;
  }

  /**
   * Set the starter tab manager reference.
   * Called by the main database class after StarterTabManager is created.
   */
  setStarterTabManager(manager: StarterTabManager): void {
    this.starterTabManager = manager;
  }

  /**
   * Initialize projects on app startup: `projectEnsureDefault` makes the
   * default project on a file with none, and lists them all. The window
   * opens on its own active project (`windowGet`: this window's, else the
   * most recently used window's, else `lastActiveProjectId`), checked
   * against the projects listed.
   */
  async initialize(): Promise<void> {
    try {
      const { value, seq } = await getLibrary().ensureDefaultProject();
      this.applyProjects(value, seq, null, false);
      this.windowState.setProjectsLoaded(true);
    } catch (error) {
      void log.error("Failed to load projects:", error);
      // The projects couldn't be read. Work in an in-memory default project,
      // which isn't stored: it isn't recorded as the window's active project,
      // and writes naming it are refused by Core.
      this.windowState.setProjectsLoaded(false);
      this.state.projects = [this.createDefaultProject()];
    }

    // Set active project
    const storedActive = await this.windowState.activeProject();
    const validProjectId = this.state.projects.find((p) => p.id === storedActive)?.id;
    this.state.activeProjectId = validProjectId || this.state.projects[0]?.id || null;

    // Load project state if there's an active project
    if (this.state.activeProjectId) {
      await this.windowState.activate(this.state.activeProjectId);
      await this.loadProjectState(this.state.activeProjectId);
    }

    this.state.projectsLoading = false;
  }

  // -------- The library's rows (phase 5d-1) --------

  private nameOf = (id: string) => libraryNameOf(this.state, id);

  /** Show one of this page's write answers for project `row`. */
  private applyOwnProject(row: WireProject, seq: ChangeSeq): Project {
    this.state.librarySeqs.note(rowKey("project", row.id), seq);
    const project = projectFromWire(row);
    this.state.projects = this.state.projects.some((p) => p.id === row.id)
      ? this.state.projects.map((p) => (p.id === row.id ? project : p))
      : [...this.state.projects, project];
    return project;
  }

  /**
   * Apply a `projectsList` taken at `seq` to the projects `ids` names (all
   * when `null`), each only if `seq` is newer (Decision 17). A project the
   * list lacks was removed: its connections go from the page, and if it
   * was the active one the page switches to another, with a toast.
   */
  applyProjects(
    rows: readonly WireProject[],
    seq: ChangeSeq,
    ids: readonly string[] | null,
    remote: boolean,
  ): void {
    const seqs = this.state.librarySeqs;
    const byId = new Map(rows.map((r) => [r.id, r]));
    const scope = new Set(ids ?? [...this.state.projects.map((p) => p.id), ...byId.keys()]);
    let next = [...this.state.projects];
    const removed: Project[] = [];
    const revisions: string[] = [];
    for (const id of scope) {
      const key = rowKey("project", id);
      if (!seqs.take(key, seq)) continue;
      const row = byId.get(id);
      const current = next.find((p) => p.id === id);
      if (row) {
        const project = projectFromWire(row);
        if (current) {
          if (remote && projectDiffers(current, project)) revisions.push(key);
          next = next.map((p) => (p.id === id ? project : p));
        } else {
          next.push(project);
        }
      } else if (current) {
        removed.push(current);
      }
    }
    this.state.projects = next;
    bumpRevisions(this.state, revisions);
    for (const project of removed) void this.forgetRemovedProject(project, remote);
  }

  /**
   * Refetch the projects another window changed (`ids`, or all) and apply
   * them, and their connection order (`projectsList` rows don't hold it;
   * Decision 22) for the ones this page has loaded.
   */
  async refreshFromLibrary(ids: readonly string[] | null): Promise<void> {
    await this.state.librarySeqs.settled("project:");
    const { value, seq } = await getLibrary().listProjects();
    this.applyProjects(value, seq, ids, true);
    const loaded = Object.keys(this.state.connectionOrderByProject);
    const orders = ids === null ? loaded : ids.filter((id) => loaded.includes(id));
    await Promise.all(orders.map((id) => refreshConnectionOrder(this.state, id)));
  }

  /**
   * Read this window's view state of `projectId` again: after a stale save
   * with nothing changed here (another page of this window saved later),
   * and for Decision 22's own-id `projectState` event (`LibrarySync`; a
   * no-op in practice, see there). The active project is read again;
   * another is read when it's next opened.
   */
  async reloadViewState(projectId: string): Promise<void> {
    if (projectId === this.state.activeProjectId) {
      await this.loadProjectState(projectId, { reload: true });
    }
  }

  /**
   * A project that is gone from storage: its connections go from the page,
   * its tabs' runs stop, and if it was active the page switches to another
   * (`notify`: with a toast, for another window's removal).
   */
  private async forgetRemovedProject(project: Project, notify: boolean): Promise<void> {
    for (const connection of this.state.connections.filter((c) => c.projectId === project.id)) {
      this.connectionManager?.forgetRemoved(connection, false);
    }
    // After the connections: forgetting them schedules the project's save,
    // which would now fail on the removed project.
    this.windowState.forgetProject(project.id);
    this.state.projects = this.state.projects.filter((p) => p.id !== project.id);
    this.lifecycle.removed?.(project.id);
    if (this.state.activeProjectId === project.id) {
      // Cleared first, so switching doesn't save state for the removed project.
      this.state.activeProjectId = null;
      if (notify) toast.info(m.library_project_removed_elsewhere({ name: project.name }));
      await this.setActive(this.state.projects[0]?.id ?? null);
    }
  }

  /**
   * Create a new project. Core assigns its id and refuses a taken name
   * (`NAME_TAKEN`, thrown worded for the user). The new project is made
   * active.
   */
  async add(name: string, description?: string, options?: { renameIfTaken?: boolean }) {
    const project = await this.create(name, description, options);
    // Automatically make the new project active
    await this.setActive(project.id);
    return project;
  }

  /** Store a new project and list it, without switching to it. */
  private async create(
    name: string,
    description?: string,
    options?: { renameIfTaken?: boolean },
  ): Promise<Project> {
    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("project", NEW)], () =>
        getLibrary().createProject({
          name,
          ...(description !== undefined ? { description } : {}),
          ...(options?.renameIfTaken ? { renameIfTaken: true } : {}),
        }),
      );
      return this.applyOwnProject(value, seq);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
  }

  /**
   * Update an existing project: only the fields `updates` has are sent
   * (`undefined` clears a description or git path). A refusal throws,
   * worded for the user, and changes nothing. A shared project's directory
   * in its repo follows a rename, after the rename is stored.
   */
  async update(
    id: string,
    updates: Partial<Pick<Project, "name" | "description" | "gitRepoPath">>,
  ): Promise<void> {
    const project = this.state.projects.find((p) => p.id === id);
    const patch: ProjectPatch = {};
    if ("name" in updates && updates.name !== undefined) patch.name = updates.name;
    if ("description" in updates) patch.description = updates.description ?? null;
    if ("gitRepoPath" in updates) patch.gitRepoPath = updates.gitRepoPath ?? null;

    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("project", id)], () =>
        getLibrary().updateProject(id, patch),
      );
      this.applyOwnProject(value, seq);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }

    // Rename git repo project directory and update project.yaml if the name changed
    if (updates.name && project && project.name !== updates.name && project.gitRepoPath) {
      const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
      if (repo) {
        try {
          const oldDirName = nameToFilename(project.name);
          const newDirName = nameToFilename(updates.name);
          const projectsDir = await join(repo.path, SEAQUEL_DIR, "projects");
          const oldDir = await join(projectsDir, oldDirName);

          if (await exists(oldDir)) {
            // Rename directory if the filename changed
            const targetDir =
              oldDirName !== newDirName ? await join(projectsDir, newDirName) : oldDir;
            if (oldDirName !== newDirName) {
              await renameFs(oldDir, targetDir);
            }

            // Update name in project.yaml
            const projectYamlPath = await join(targetDir, "project.yaml");
            if (await exists(projectYamlPath)) {
              const yaml = serializeProjectFile({
                id: `${repo.id}:${SEAQUEL_DIR}/projects/${newDirName}`,
                repoId: repo.id,
                name: updates.name,
                description: updates.description ?? project.description,
                dirName: newDirName,
                connections: [],
              });
              await writeTextFile(projectYamlPath, yaml);
            }

            // Reload shared state so in-memory paths reflect the renamed directory
            if (this.sharedRepos) {
              await this.sharedRepos.loadQueriesFromRepo(repo.id);
            }

            // Tab queryIds are stable SQLite IDs — no path updates needed on rename
          }
        } catch {
          // Directory may not exist yet
        }
      }
    }
  }

  /**
   * Set the git repo path for a project and trigger scanning.
   * When set for the first time, auto-links the project to the repo.
   */
  async setGitRepoPath(projectId: string, path: string | undefined): Promise<void> {
    const project = this.state.projects.find((p) => p.id === projectId);
    if (!project) return;

    const hadGitPath = !!project.gitRepoPath;
    const oldGitRepoPath = project.gitRepoPath;

    // Update the project
    await this.update(projectId, { gitRepoPath: path });

    if (path && this.sharedRepos) {
      // Create .seaquel directory structure while dialog scope is active
      try {
        const dirName = nameToFilename(project.name);
        const projectDir = await join(path, SEAQUEL_DIR, "projects", dirName);
        await mkdir(await join(projectDir, "connections"), { recursive: true });
        await mkdir(await join(projectDir, "queries"), { recursive: true });
        await mkdir(await join(projectDir, "dashboards"), { recursive: true });
      } catch {
        // Directory may already exist
      }

      // Check if a SharedQueryRepo entry already exists for this path
      const existingRepo = this.state.sharedRepos.find((r) => r.path === path);

      let activeRepoId: string;

      if (existingRepo) {
        activeRepoId = existingRepo.id;
        // Reuse existing repo - set as active when this project is active
        if (this.state.activeProjectId === projectId) {
          this.state.activeRepoId = existingRepo.id;
        }
        // Reload queries/configs
        await this.sharedRepos.loadQueriesFromRepo(existingRepo.id);
      } else {
        // Register the path as a new repo (without cloning)
        activeRepoId = await this.sharedRepos.initRepo(project.name, path);

        // Set as active when this project is active
        if (this.state.activeProjectId === projectId) {
          this.state.activeRepoId = activeRepoId;
        }

        // Load existing queries and shared configs from the repo
        await this.sharedRepos.loadQueriesFromRepo(activeRepoId);
      }

      // Auto-detect remote URL from the git repo
      const linkedRepo = this.state.sharedRepos.find((r) => r.path === path);
      if (linkedRepo && !linkedRepo.remoteUrl && isTauri()) {
        try {
          const { getRemoteUrl } = await import("$lib/services/git");
          const remoteUrl = await getRemoteUrl(path);
          if (remoteUrl) {
            await this.sharedRepos.setRemoteUrl(linkedRepo.id, remoteUrl);
          }
        } catch {
          // No remote configured — that's fine
        }
      }

      // If first-time setup, export existing non-local-only connections to git
      if (!hadGitPath) {
        const projectConnections = this.state.connections.filter(
          (c) => c.projectId === projectId && !c.isLocalOnly,
        );
        if (projectConnections.length > 0) {
          const activeRepo = this.state.sharedRepos.find((r) => r.path === path);
          if (activeRepo) {
            await this.sharedRepos.exportProject(
              activeRepo.id,
              project.name,
              projectConnections,
              {},
            );
          }
        }
      }
    } else if (!path) {
      // Clearing git path - clean up shared state for the old repo
      const oldRepo = oldGitRepoPath
        ? this.state.sharedRepos.find((r) => r.path === oldGitRepoPath)
        : null;

      if (oldRepo) {
        // Remove local connections that were imported from this repo's shared connections
        const repoId = oldRepo.id;
        const sharedProjects = this.state.sharedProjectsByRepo[repoId] ?? [];
        const sharedConnectionIds = new Set(
          sharedProjects.flatMap((sp) =>
            (this.state.sharedConnectionsByProject[sp.id] ?? []).map((sc) => sc.id),
          ),
        );
        if (sharedConnectionIds.size > 0) {
          const importedConnections = this.state.connections.filter(
            (c) =>
              c.projectId === projectId &&
              c.sharedConnectionId &&
              sharedConnectionIds.has(c.sharedConnectionId),
          );
          for (const conn of importedConnections) {
            await this.removeStoredConnection(conn.id);
          }
          // Out of the project's connection order too, stored at once.
          const removed = new Set(importedConnections.map((c) => c.id));
          const order = this.state.connectionOrderByProject[projectId] ?? [];
          if (order.some((id) => removed.has(id))) {
            this.state.connectionOrderByProject = {
              ...this.state.connectionOrderByProject,
              [projectId]: order.filter((id) => !removed.has(id)),
            };
            await storeConnectionOrder(this.state, projectId);
          }
          this.state.connections = this.state.connections.filter(
            (c) =>
              !(
                c.projectId === projectId &&
                c.sharedConnectionId &&
                sharedConnectionIds.has(c.sharedConnectionId)
              ),
          );
        }

        // Remove the repo and all its shared state (queries, configs, etc.)
        if (this.sharedRepos) {
          this.sharedRepos.removeRepo(repoId);
        }
      }

      if (this.state.activeProjectId === projectId) {
        this.state.activeRepoId = null;
      }
    }
  }

  /**
   * Delete a project and everything in it. Cannot delete the last project
   * (returns false). Core removes it in one transaction, with its
   * connections (and their secrets), saved queries, dashboards and saved
   * workflows (Decision 9); a failure throws, worded for the user, and the
   * project stays. Its connections' shared files stay in the repo: removing
   * a project here doesn't remove it from the team.
   */
  async remove(id: string): Promise<boolean> {
    // Cannot delete the last project
    if (this.state.projects.length <= 1) {
      void log.warn("Cannot delete the last project");
      return false;
    }
    const project = this.state.projects.find((p) => p.id === id);
    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("project", id)], () =>
        getLibrary().removeProject(id),
      );
      this.state.librarySeqs.note(rowKey("project", id), seq);
    } catch (error) {
      if (errorCode(error) === LAST_PROJECT) return false;
      throw libraryError(error, this.nameOf);
    }
    if (project) await this.forgetRemovedProject(project, false);
    return true;
  }

  /**
   * Set the active project.
   */
  async setActive(id: string | null): Promise<void> {
    if (id === this.state.activeProjectId) return;

    void log.info(`Project changed: from=${this.state.activeProjectId} to=${id}`);

    // Save current project state before switching, with the view it's left on
    if (this.state.activeProjectId) {
      this.state.activeViewByProject[this.state.activeProjectId] = this.state.activeView;
      await this.windowState.saveNow(this.state.activeProjectId, { leaving: true });
    }

    // Preload saved queries/dashboards before changing activeProjectId so that
    // $derived values (e.g. projectQueries) see the data immediately. Read
    // once per activation (bug 18): `loadProjectState` then skips it.
    let dataLoaded = false;
    if (id && !(id in this.state.queriesByProject)) {
      await this.stateRestoration.loadProjectData(id);
      dataLoaded = true;
    }

    this.state.activeProjectId = id;
    this.lifecycle.activated?.(id);

    // Load new project state
    if (id) {
      // This window's active project (and `lastActiveProjectId`, which Core
      // writes with it for older releases).
      await this.windowState.activate(id);
      await this.loadProjectState(id, { dataLoaded });

      // Auto-link repo when project has gitRepoPath
      const project = this.state.projects.find((p) => p.id === id);
      if (project?.gitRepoPath && this.sharedRepos) {
        const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
        if (repo) {
          this.state.activeRepoId = repo.id;
          await this.reconcileGitState(id);
        }
      }
    }
  }

  /**
   * Add a custom label to a project. Core assigns its id and refuses a
   * taken name or a bad colour (thrown, worded for the user). The project's
   * `updatedAt` stays (Decision 10).
   */
  async addCustomLabel(
    projectId: string,
    label: Omit<ConnectionLabel, "id" | "isPredefined">,
  ): Promise<ConnectionLabel> {
    try {
      const { value, seq } = await this.state.librarySeqs.write(
        [rowKey("project", projectId)],
        () => getLibrary().createLabel(projectId, { name: label.name, color: label.color }),
      );
      // Shown at once; the project is then read again at a `seq` at least
      // this new, so its `seq` is recorded only with its whole value (C1).
      this.state.projects = this.state.projects.map((p) =>
        p.id === projectId ? { ...p, customLabels: [...p.customLabels, value] } : p,
      );
      await this.resyncAfterLabelWrite(projectId, seq, []);
      return value;
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
  }

  /**
   * Remove a custom label from a project. Core strips it from every
   * connection that had it in the same transaction (Decision 10), and the
   * page does the same.
   */
  async removeCustomLabel(projectId: string, labelId: string): Promise<void> {
    let connectionIds: string[];
    let removedAt: ChangeSeq;
    try {
      const result = await this.state.librarySeqs.write(
        [rowKey("project", projectId), "connection:"],
        () => getLibrary().removeLabel(projectId, labelId),
      );
      connectionIds = result.value.connectionIds;
      removedAt = result.seq;
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
    this.state.projects = this.state.projects.map((p) =>
      p.id === projectId
        ? { ...p, customLabels: p.customLabels.filter((l) => l.id !== labelId) }
        : p,
    );
    const stripped = new Set(connectionIds);
    this.state.connections = this.state.connections.map((c) =>
      stripped.has(c.id) || c.labelIds.includes(labelId)
        ? { ...c, labelIds: c.labelIds.filter((id) => id !== labelId) }
        : c,
    );
    await this.resyncAfterLabelWrite(projectId, removedAt, connectionIds);
  }

  /**
   * After a label write at `seq`: read the project (and the connections the
   * write stripped) again and apply them whole, at a `seq` at least that
   * new. Recording `seq` for rows this page only patched would hide another
   * window's earlier change to them (a rename at n-1 under a label at n).
   * A failed read is logged: the page shows the write, and the next change
   * or reload brings the rest.
   */
  private async resyncAfterLabelWrite(
    projectId: string,
    seq: ChangeSeq,
    connectionIds: readonly string[],
  ): Promise<void> {
    this.state.librarySeqs.observe(seq);
    try {
      const projects = await getLibrary().listProjects();
      this.applyProjects(projects.value, projects.seq, [projectId], false);
      if (connectionIds.length > 0) {
        await this.connectionManager?.refreshFromLibrary(connectionIds, { remote: false });
      }
    } catch (error) {
      void log.warn("Reading the project again after a label change failed:", error);
    }
  }

  /**
   * Update a custom label's name or colour. A refusal throws, worded for
   * the user, and changes nothing.
   */
  async updateCustomLabel(
    projectId: string,
    labelId: string,
    updates: Partial<Pick<ConnectionLabel, "name" | "color">>,
  ): Promise<void> {
    let written: ChangeSeq;
    try {
      const { value, seq } = await this.state.librarySeqs.write(
        [rowKey("project", projectId)],
        () =>
          getLibrary().updateLabel(projectId, labelId, {
            ...(updates.name !== undefined ? { name: updates.name } : {}),
            ...(updates.color !== undefined ? { color: updates.color } : {}),
          }),
      );
      this.state.projects = this.state.projects.map((p) =>
        p.id === projectId
          ? { ...p, customLabels: p.customLabels.map((l) => (l.id === labelId ? value : l)) }
          : p,
      );
      written = seq;
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
    await this.resyncAfterLabelWrite(projectId, written, []);
  }

  /**
   * Import shared projects from a git repo folder.
   * Creates local projects, links them to the repo, and imports their connections.
   */
  async importFromGitRepo(repoPath: string, selectedProjects: SharedProject[]): Promise<string[]> {
    const createdIds: string[] = [];

    for (const sharedProject of selectedProjects) {
      // A taken name becomes the first free "<name> (2)": Core picks it
      // (Decision 13).
      const project = await this.add(sharedProject.name, undefined, { renameIfTaken: true });
      createdIds.push(project.id);

      // Link to git repo (registers repo, loads configs, exports existing connections)
      await this.setGitRepoPath(project.id, repoPath);

      // Import shared connections as local entries
      await this.importSharedConnections(project.id);

      // Reconcile git queries and dashboards into local state
      await this.reconcileGitState(project.id);
    }

    // Switch to first created project
    if (createdIds.length > 0) {
      await this.setActive(createdIds[0]);
    }

    return createdIds;
  }

  // === PRIVATE METHODS ===

  /**
   * Reconcile git .sql and .json files with local saved queries and dashboards.
   * Called after git repo is linked and queries/dashboards are scanned.
   */
  async reconcileGitState(projectId: string): Promise<void> {
    // Reconcile queries
    if (this.sharedQueryManager) {
      const queries = this.state.queriesByProject[projectId] ?? [];
      const reconciled = this.sharedQueryManager.reconcileWithGitFiles(projectId, queries);
      if (reconciled !== queries) await this.storeReconciledQueries(projectId, queries, reconciled);
    }

    // Reconcile dashboards
    if (this.sharedDashboardManager) {
      const dashboards = this.state.dashboardsByProject[projectId] ?? [];
      const reconciled = this.sharedDashboardManager.reconcileWithGitFiles(projectId, dashboards);
      if (reconciled !== dashboards) {
        // Only what the reconcile changed, compared in stored form; each is
        // shown as Core stores it (a new file's placeholder never is).
        const before = new Map(dashboards.map((d) => [d.id, storedForm(d)]));
        await this.dashboardStore?.storeReconciled(
          projectId,
          dashboards,
          reconciled.filter((d) => before.get(d.id) !== storedForm(d)),
        );
      }
    }
  }

  /**
   * Store what the reconcile changed, one call per query: a new `.sql`
   * file is a new saved query (Core's id), a changed one a patch of the
   * fields that differ. Each result is shown as it lands; a failed one is
   * logged and the rest go on.
   */
  private async storeReconciledQueries(
    projectId: string,
    before: readonly Query[],
    after: readonly Query[],
  ): Promise<void> {
    const library = getLibrary();
    const seqs = this.state.librarySeqs;
    const byId = new Map(before.map((q) => [q.id, q]));
    const show = (query: Query) => {
      const list = this.state.queriesByProject[projectId] ?? [];
      this.state.queriesByProject = {
        ...this.state.queriesByProject,
        [projectId]: list.some((q) => q.id === query.id)
          ? list.map((q) => (q.id === query.id ? query : q))
          : [...list, query],
      };
    };
    let versionsChanged = false;
    for (const query of after) {
      const old = byId.get(query.id);
      try {
        if (!old) {
          const { value, seq } = await seqs.write([rowKey("savedQuery", NEW)], () =>
            library.createSavedQuery({
              projectId,
              name: query.name,
              query: query.query,
              ...(query.parameters ? { parameters: query.parameters } : {}),
              ...(query.description ? { description: query.description } : {}),
              ...(query.databaseType ? { databaseType: query.databaseType } : {}),
              ...(query.tags ? { tags: query.tags } : {}),
              ...(query.folder ? { folder: query.folder } : {}),
              shared: query.shared,
            }),
          );
          seqs.note(rowKey("savedQuery", value.id), seq);
          show(savedQueryFromWire(value));
        } else if (old !== query) {
          const patch = savedQueryPatch(old, query);
          if (Object.keys(patch).length === 0) continue;
          const { value, seq } = await seqs.write([rowKey("savedQuery", query.id)], () =>
            library.updateSavedQuery(query.id, patch),
          );
          seqs.note(rowKey("savedQuery", query.id), seq);
          show(savedQueryFromWire(value.query));
          if (value.version || value.prunedVersionIds.length > 0) versionsChanged = true;
        }
      } catch (error) {
        // A shared file whose name another saved query already has, say.
        // Renaming it would part it from its file, so it's said, not fixed.
        void log.warn(`Storing a reconciled shared query failed:`, error);
        errorToast(
          m.shared_query_reconcile_failed({
            name: query.name,
            message: libraryErrorMessage(error, this.nameOf),
          }),
        );
      }
    }
    // The versions the updates appended, read whole.
    if (versionsChanged) await refreshQueryVersions(this.state, projectId);
  }

  /**
   * Import shared connections from the linked repo as local DatabaseConnection entries.
   * Skips connections that are already imported (matched by sharedConnectionId).
   * Called explicitly (e.g. on project settings save), not automatically on folder selection.
   */
  async importSharedConnections(projectId: string): Promise<void> {
    const project = this.state.projects.find((p) => p.id === projectId);
    if (!project?.gitRepoPath) return;

    const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
    if (!repo) return;

    const repoId = repo.id;
    const sharedProjects = this.state.sharedProjectsByRepo[repoId] ?? [];

    for (const sharedProject of sharedProjects) {
      const sharedConnections = this.state.sharedConnectionsByProject[sharedProject.id] ?? [];

      for (const sharedConn of sharedConnections) {
        // Check if already imported
        const alreadyImported = this.state.connections.some(
          (c) => c.sharedConnectionId === sharedConn.id,
        );
        if (alreadyImported) continue;

        await this.addImportedConnection(sharedConn, projectId);
      }
    }
  }

  /**
   * Import a single shared connection template into a specific project.
   * Used by deep links to import individual connections.
   */
  async importSingleSharedConnection(
    sharedConn: SharedConnection,
    projectId: string,
  ): Promise<void> {
    const alreadyImported = this.state.connections.some(
      (c) => c.sharedConnectionId === sharedConn.id,
    );
    if (alreadyImported) return;

    await this.addImportedConnection(sharedConn, projectId);
  }

  /**
   * Store a connection made from a shared template (Core's id; a taken name
   * becomes "<name> (2)"), then list it like a new one: in memory with its
   * maps, at the end of its project's order. A refusal throws, worded for
   * the user, and adds nothing.
   */
  private async addImportedConnection(
    sharedConn: SharedConnection,
    projectId: string,
  ): Promise<void> {
    const draft = {
      ...connectionDraft(
        {
          name: sharedConn.name,
          type: sharedConn.type,
          host: sharedConn.host,
          port: sharedConn.port,
          databaseName: sharedConn.databaseName,
          username: "",
          sslMode: sharedConn.sslMode,
          sharedConnectionId: sharedConn.id,
          sshTunnel: sharedConn.sshTunnel
            ? {
                enabled: true,
                host: sharedConn.sshTunnel.host,
                port: sharedConn.sshTunnel.port,
                username: "",
                authMethod: "key",
              }
            : undefined,
          // Shared templates belong to the repo: not local-only.
          isLocalOnly: false,
        },
        projectId,
      ),
      connected: false,
      renameIfTaken: true,
    };
    let connection: DatabaseConnection;
    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("connection", NEW)], () =>
        getLibrary().createConnection(draft),
      );
      connection = applyConnectionRow(this.state, value, seq);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
    this.stateRestoration.initializeConnectionMaps(connection.id);
    const order = this.state.connectionOrderByProject[connection.projectId] ?? [];
    if (!order.includes(connection.id)) {
      this.state.connectionOrderByProject = {
        ...this.state.connectionOrderByProject,
        [connection.projectId]: [...order, connection.id],
      };
      // The order is shared by the project's windows: stored at once.
      await storeConnectionOrder(this.state, connection.projectId);
    }
  }

  /** Delete a stored connection (the ones a cleared git path imported). */
  private async removeStoredConnection(id: string): Promise<void> {
    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("connection", id)], () =>
        getLibrary().removeConnection(id),
      );
      this.state.librarySeqs.note(rowKey("connection", id), seq);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
  }

  /** The in-memory stand-in project when the projects couldn't be read. */
  private createDefaultProject(): Project {
    const now = new Date();
    return {
      id: DEFAULT_PROJECT_ID,
      name: DEFAULT_PROJECT_NAME,
      createdAt: now,
      updatedAt: now,
      customLabels: [],
    };
  }

  /**
   * The connections a restored tab may name, or `null` while they aren't
   * read yet (startup restores the project before the connections load):
   * then a tab is kept whichever connection it names.
   */
  private knownConnections(): ReadonlySet<string> | null {
    return this.connectionManager?.loaded ? new Set(this.state.connections.map((c) => c.id)) : null;
  }

  /**
   * Read the project's connection order, shared by its windows, and show it
   * by the `seq` rule. It is left as shown when the read fails.
   */
  private async loadConnectionOrder(projectId: string): Promise<void> {
    await refreshConnectionOrder(this.state, projectId);
    // Known as loaded from now on (a `project` event refetches it).
    if (!(projectId in this.state.connectionOrderByProject)) {
      this.state.connectionOrderByProject[projectId] = [];
    }
  }

  /**
   * Read the project's saved workflows (Decision 23: no longer in the view
   * state) as the sidebar lists them, without their bodies (5d-2 Task 7):
   * opening one reads it (`WorkflowManager.loadWorkflow`), and one whose
   * body won't decode says so then. A failed read leaves the page's list.
   */
  private async loadSavedWorkflows(projectId: string): Promise<void> {
    try {
      const { value, seq } = await getLibrary().listWorkflows(projectId);
      const workflows = value.map(workflowSummaryFromWire);
      // A later `workflow` event's refetch applies only what is newer.
      for (const w of workflows) this.state.librarySeqs.note(rowKey("workflow", w.id), seq);
      this.state.savedWorkflowsByProject[projectId] = workflows;
    } catch (error) {
      void log.error(`Failed to load the saved workflows of project ${projectId}:`, error);
      this.state.savedWorkflowsByProject[projectId] ??= [];
    }
  }

  /**
   * Load and restore a project's view state. `reload`: the project is shown
   * already and is read again (`reloadViewState`); its tabs are left alone
   * when the read fails or the project is no longer the active one by then.
   */
  private async loadProjectState(
    projectId: string,
    { reload = false, dataLoaded = false } = {},
  ): Promise<void> {
    // Its tabs are replaced below: a run still going on one of them would
    // lose its results, so cancel it cleanly first (a reload does it once
    // it knows it will restore).
    if (!reload) this.lifecycle.reloading?.(projectId);
    // Load saved queries and dashboards FIRST, before any state assignments
    // that trigger UI re-renders via $derived. This ensures queriesByProject
    // is populated when projectQueries recomputes after activeProjectId changes.
    if (!dataLoaded) await this.stateRestoration.loadProjectData(projectId);

    // This window's view state (Decision 22), and what the project's windows
    // share: the connection order and the saved workflows.
    const [loaded] = await Promise.all([
      this.windowState.load(projectId, { reload }),
      this.loadConnectionOrder(projectId),
      this.loadSavedWorkflows(projectId),
    ]);
    if (reload) {
      // A failed read leaves what the page shows, still saveable.
      if (!loaded) return;
      if (projectId !== this.state.activeProjectId) {
        this.windowState.dropLoad(projectId);
        return;
      }
      this.lifecycle.reloading?.(projectId);
    }
    const persistedState = (loaded?.state ?? null) as LegacyPersistedProjectState | null;
    const known = await this.connectionsNamedBy(persistedState);
    let restored = false;
    try {
      this.restoreViewState(projectId, persistedState, known);
      restored = true;
    } finally {
      // Saveable only now: a save between the load's answer and here would
      // store what the page showed before the restore.
      if (loaded) this.windowState.markLoaded(projectId, restored);
    }
  }

  /**
   * The connections a restored tab may name (see `knownConnections`). When
   * the saved state names one this page doesn't list, the connections are
   * read again first: another window may just have made it.
   */
  private async connectionsNamedBy(
    saved: LegacyPersistedProjectState | null,
  ): Promise<ReadonlySet<string> | null> {
    const known = this.knownConnections();
    if (!known || !saved || !this.connectionManager) return known;
    const named = [
      saved.schemaTabs,
      saved.erdTabs,
      saved.statisticsTabs,
      saved.workflowTabs ?? saved.canvasTabs,
      saved.createTableTabs,
      saved.dataTabs,
      saved.extensionsDuckdbTabs,
    ].flatMap((tabs) => (tabs ?? []).map((t) => t.connectionId).filter((id) => !!id) as string[]);
    const missing = [...new Set(named)].filter((id) => !known.has(id));
    if (missing.length === 0) return known;
    try {
      await this.connectionManager.refreshFromLibrary(missing);
    } catch (error) {
      void log.warn("Reading the connections again before a restore failed:", error);
    }
    return this.knownConnections();
  }

  /** Put a project's loaded view state (or none) in memory. */
  private restoreViewState(
    projectId: string,
    persistedState: LegacyPersistedProjectState | null,
    known: ReadonlySet<string> | null,
  ): void {
    const keep = <T extends { connectionId?: string }>(tabs: readonly T[] | undefined) =>
      (tabs ?? []).filter((t) => keepsConnectionTab(t, known));
    if (!persistedState) {
      // Initialize empty state for this project
      this.state.queryTabsByProject[projectId] = [];
      this.state.schemaTabsByProject[projectId] = [];
      this.state.explainTabsByProject[projectId] = [];
      this.state.erdTabsByProject[projectId] = [];
      this.state.statisticsTabsByProject[projectId] = [];
      this.state.workflowTabsByProject[projectId] = [];
      this.state.dashboardTabsByProject[projectId] = [];
      this.state.tabOrderByProject[projectId] = [];
      this.state.activeQueryTabIdByProject[projectId] = null;
      this.state.activeSchemaTabIdByProject[projectId] = null;
      this.state.activeExplainTabIdByProject[projectId] = null;
      this.state.activeErdTabIdByProject[projectId] = null;
      this.state.activeStatisticsTabIdByProject[projectId] = null;
      this.state.activeWorkflowTabIdByProject[projectId] = null;
      this.state.activeDashboardTabIdByProject[projectId] = null;
      this.state.activeConnectionIdByProject[projectId] = null;
      this.state.connectionTabsByProject[projectId] = [];
      this.state.activeConnectionTabIdByProject[projectId] = null;
      this.state.createTableTabsByProject[projectId] = [];
      this.state.activeCreateTableTabIdByProject[projectId] = null;
      this.state.dataTabsByProject[projectId] = [];
      this.state.activeDataTabIdByProject[projectId] = null;
      this.state.extensionsDuckdbTabsByProject[projectId] = [];
      this.state.activeExtensionsDuckdbTabIdByProject[projectId] = null;
      // Initialize starter tabs for new projects
      this.starterTabManager?.initializeDefaults(projectId);
      return;
    }

    // Restore tabs - query tabs
    this.state.queryTabsByProject[projectId] = persistedState.queryTabs.map((t) => ({
      id: t.id,
      name: t.name,
      query: t.query,
      queryId: t.queryId,
      isExecuting: false,
    }));

    // Restore schema tabs (we'll need to look up the table info later)
    // For now, create placeholder tabs that will be populated when the connection loads
    this.state.schemaTabsByProject[projectId] = keep(persistedState.schemaTabs).map((t) => ({
      id: t.id,
      connectionId: t.connectionId!,
      table: {
        schema: t.schemaName,
        name: t.tableName,
        type: "table" as const, // Default to table, will be updated when metadata loads
        columns: [],
        indexes: [],
      },
    }));

    // Restore explain tabs
    this.state.explainTabsByProject[projectId] = persistedState.explainTabs.map((t) => ({
      id: t.id,
      name: t.name,
      sourceQuery: t.sourceQuery,
      isExecuting: false,
    }));

    // Restore ERD tabs (connectionId may be missing in old persisted data)
    this.state.erdTabsByProject[projectId] = keep(persistedState.erdTabs).map((t) => ({
      id: t.id,
      name: t.name,
      connectionId: t.connectionId!,
    }));

    // Restore statistics tabs
    this.state.statisticsTabsByProject[projectId] = keep(persistedState.statisticsTabs).map(
      (t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connectionId,
        isLoading: false,
      }),
    );

    // Restore workflow tabs (`canvasTabs` before the workflow rename)
    this.state.workflowTabsByProject[projectId] = keep(
      persistedState.workflowTabs ?? persistedState.canvasTabs,
    ).map((t) => ({
      id: t.id,
      name: t.name,
      connectionId: t.connectionId,
    }));

    // Restore tab order and active IDs
    this.state.tabOrderByProject[projectId] = persistedState.tabOrder ?? [];
    this.state.activeQueryTabIdByProject[projectId] = persistedState.activeQueryTabId;
    this.state.activeSchemaTabIdByProject[projectId] = persistedState.activeSchemaTabId;
    this.state.activeExplainTabIdByProject[projectId] = persistedState.activeExplainTabId;
    this.state.activeErdTabIdByProject[projectId] = persistedState.activeErdTabId;
    this.state.activeStatisticsTabIdByProject[projectId] =
      persistedState.activeStatisticsTabId ?? null;
    this.state.activeWorkflowTabIdByProject[projectId] =
      persistedState.activeWorkflowTabId ?? persistedState.activeCanvasTabId ?? null;
    // The window's own active connection (Q14), if the connection exists
    // (even if not yet reconnected). Auto-reconnect runs after restore and
    // will establish providerConnectionId.
    const restoredConnectionExists = persistedState.activeConnectionId
      ? this.state.connections.some((c) => c.id === persistedState.activeConnectionId)
      : false;
    this.state.activeConnectionIdByProject[projectId] = restoredConnectionExists
      ? persistedState.activeConnectionId
      : null;
    this.state.activeViewByProject[projectId] = persistedState.activeView;
    if (projectId === this.state.activeProjectId) this.state.activeView = persistedState.activeView;

    // Restore starter tabs
    if (persistedState.starterTabs && persistedState.starterTabs.length > 0) {
      this.state.starterTabsByProject[projectId] = persistedState.starterTabs.map((t) => ({
        id: t.id,
        type: t.type,
        name: t.name,
        closable: t.closable,
      }));
      this.state.activeStarterTabIdByProject[projectId] = persistedState.activeStarterTabId ?? null;
      // Register restored starter tabs with tab ordering (add to tabOrder if not already present)
      const tabOrder = this.state.tabOrderByProject[projectId] ?? [];
      const tabOrderSet = new Set(tabOrder);
      const newIds = persistedState.starterTabs
        .map((t) => t.id)
        .filter((id) => !tabOrderSet.has(id));
      if (newIds.length > 0) {
        this.state.tabOrderByProject[projectId] = [...newIds, ...tabOrder];
      }
    } else {
      // Only show starter tabs if the project has no other open tabs: read
      // from what was saved and kept (the tab lists below aren't restored
      // yet), over every saved tab type, plus the settings tabs, which
      // aren't saved.
      const hasOtherTabs =
        hasSavedTabs(persistedState, known) ||
        (this.state.settingsTabsByProject[projectId]?.length ?? 0) > 0;
      if (!hasOtherTabs) {
        this.starterTabManager?.initializeDefaults(projectId);
      }
    }

    // Restore dashboard tabs
    this.state.dashboardTabsByProject[projectId] = (persistedState.dashboardTabs ?? [])
      .filter((t) => t.dashboardId)
      .map((t) => ({
        id: t.id,
        name: t.name,
        dashboardId: t.dashboardId,
      }));
    this.state.activeDashboardTabIdByProject[projectId] =
      persistedState.activeDashboardTabId ?? null;

    // Starred shared IDs are now on the Query/Dashboard objects (migrated at load time)

    // Restore pane layout (if saved); otherwise it will be auto-created on first access
    if (persistedState.paneLayout && persistedState.paneLayout.panes.length > 0) {
      this.state.paneLayoutByProject = {
        ...this.state.paneLayoutByProject,
        [projectId]: persistedState.paneLayout,
      };
    }

    // Restore create table tabs
    this.state.createTableTabsByProject[projectId] = keep(persistedState.createTableTabs).map(
      (t) => ({
        id: t.id,
        connectionId: t.connectionId,
        name: t.name,
        tableDefinition:
          typeof t.tableDefinition === "string"
            ? JSON.parse(t.tableDefinition)
            : { tableName: "", schemaName: "public", columns: [], indexes: [], foreignKeys: [] },
      }),
    );
    this.state.activeCreateTableTabIdByProject[projectId] =
      persistedState.activeCreateTableTabId ?? null;

    // Restore data tabs (results will be fetched fresh on activation)
    this.state.dataTabsByProject[projectId] = keep(persistedState.dataTabs).map((t) => ({
      id: t.id,
      connectionId: t.connectionId,
      tableName: t.tableName,
      schemaName: t.schemaName,
      filters: [],
      filterLogic: "AND" as const,
      sortColumns: [],
      page: 1,
      pageSize: 100,
      isLoading: false,
      pendingNewRows: [],
    }));
    this.state.activeDataTabIdByProject[projectId] = persistedState.activeDataTabId ?? null;

    // Restore DuckDB extensions tabs (kept in the window's row since 5d-2)
    this.state.extensionsDuckdbTabsByProject[projectId] = keep(
      persistedState.extensionsDuckdbTabs,
    ).map((t) => ({
      id: t.id,
      name: t.name,
      connectionId: t.connectionId,
      isLoading: false,
    }));
    this.state.activeExtensionsDuckdbTabIdByProject[projectId] =
      persistedState.activeExtensionsDuckdbTabId ?? null;

    // Connection tabs are transient - always initialize empty
    this.state.connectionTabsByProject[projectId] = [];
    this.state.activeConnectionTabIdByProject[projectId] = null;

    // Clear any pane layout created prematurely by $effect (before tab data was loaded)
    if (!persistedState.paneLayout || persistedState.paneLayout.panes.length === 0) {
      this.clearStalePaneLayout(projectId);
    }
  }

  /**
   * Remove a pane layout that was created before tab data was available.
   * The $effect in pane-container.svelte will recreate it with correct tab data.
   */
  private clearStalePaneLayout(projectId: string): void {
    const layout = this.state.paneLayoutByProject[projectId];
    if (layout && layout.panes.length > 0 && layout.panes.every((p) => p.tabIds.length === 0)) {
      const { [projectId]: _, ...rest } = this.state.paneLayoutByProject;
      this.state.paneLayoutByProject = rest;
    }
  }
}

/**
 * Whether a restored tab bound to a connection is kept: it names one, and
 * (once the connections are known) one that still exists.
 */
function keepsConnectionTab(
  tab: { connectionId?: string },
  known: ReadonlySet<string> | null,
): boolean {
  return !!tab.connectionId && (known === null || known.has(tab.connectionId));
}

/**
 * Whether a saved project state holds any open tab the restore keeps,
 * besides the starter tabs: every saved tab type counts, the old
 * `canvasTabs` name for workflow tabs too, but not a tab the restore drops
 * (a connection-bound tab with no connection, or one naming a connection
 * that's gone once `known` lists them; a dashboard tab with no dashboard).
 * Starter tabs are added to a project only when this is false.
 */
export function hasSavedTabs(
  saved: LegacyPersistedProjectState,
  known: ReadonlySet<string> | null = null,
): boolean {
  const bound = (tabs: readonly { connectionId?: string }[] | undefined) =>
    (tabs ?? []).some((t) => keepsConnectionTab(t, known));
  return (
    (saved.queryTabs?.length ?? 0) > 0 ||
    (saved.explainTabs?.length ?? 0) > 0 ||
    (saved.dashboardTabs ?? []).some((t) => !!t.dashboardId) ||
    bound(saved.schemaTabs) ||
    bound(saved.erdTabs) ||
    bound(saved.statisticsTabs) ||
    bound(saved.workflowTabs ?? saved.canvasTabs) ||
    bound(saved.createTableTabs) ||
    bound(saved.dataTabs) ||
    bound(saved.extensionsDuckdbTabs)
  );
}

/** Whether another window's refetch changed what the project settings edit. */
function projectDiffers(a: Project, b: Project): boolean {
  return (
    a.name !== b.name ||
    (a.description ?? "") !== (b.description ?? "") ||
    (a.gitRepoPath ?? "") !== (b.gitRepoPath ?? "")
  );
}
