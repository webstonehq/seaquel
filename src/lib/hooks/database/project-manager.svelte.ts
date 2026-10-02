import type { Project, ConnectionLabel, DatabaseConnection } from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { WindowStateManager } from "./window-state.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { SharedRepoManager } from "./shared-repo-manager.svelte.js";
import type { StarterTabManager } from "./starter-tabs.svelte.js";
import { log } from "$lib/utils/logger";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import type { ConnectionManager } from "./connection-manager.svelte.js";
import {
  NEW,
  getLibrary,
  rowKey,
  type ChangeSeq,
  type ProjectPatch,
  type WireProject,
} from "./library/index.js";
import { projectFromWire, workflowSummaryFromWire } from "./library/convert.js";
import { LAST_PROJECT } from "./library/types.js";
import { libraryError } from "./library/messages.js";
import { bumpRevisions, libraryNameOf, refreshConnectionOrder } from "./library/view.js";
import { errorToast } from "$lib/utils/toast";
import { errorCode } from "$lib/core/client";
import { getShared, type UnlinkReport } from "./shared/index.js";
import { reportProjection } from "./shared/projection.js";
import { failureText, sharedError } from "./shared/notices.js";
import type { PersistedWorkflowTab } from "$lib/types/persisted";
import type { PersistedProjectState } from "$lib/types/project";

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
  private starterTabManager: StarterTabManager | null = null;
  private sharedRepos: SharedRepoManager | null = null;
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
   * (`undefined` clears a description). A refusal throws, worded for the
   * user, and changes nothing. A linked project's `project.yaml` follows a
   * rename (Core publishes it; its directory stays, Q25), and a failed
   * write is said.
   */
  async update(id: string, updates: Partial<Pick<Project, "name" | "description">>): Promise<void> {
    const patch: ProjectPatch = {};
    if ("name" in updates && updates.name !== undefined) patch.name = updates.name;
    if ("description" in updates) patch.description = updates.description ?? null;

    try {
      const answer = await this.state.librarySeqs.write([rowKey("project", id)], () =>
        getLibrary().updateProject(id, patch),
      );
      this.applyOwnProject(answer.value, answer.seq);
      reportProjection(answer, id);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
  }

  /**
   * Link a project to the repo at `path` (Decision 40): Core registers the
   * repo, picks the project's directory, exports the connections `share`
   * names as templates (the link dialog's ticked ones, Q30) and syncs. The
   * page then reads the project and the repo list again and shows the
   * sync's outcome. A refusal throws, worded for the user.
   */
  async linkProject(projectId: string, path: string, share: string[]): Promise<void> {
    let report;
    try {
      ({ value: report } = await getShared().linkProject(projectId, path, share));
    } catch (error) {
      throw sharedError(error);
    }
    await this.refreshFromLibrary([projectId]);
    await this.sharedRepos?.loadRepos();
    const repo = this.sharedRepos?.repoForProject(projectId);
    if (repo && this.sharedRepos) {
      await this.sharedRepos.showReport(repo, [projectId], report);
      await this.sharedRepos.refreshRepoStatus(repo.id);
    }
    // The first linked repo starts the status refresh too, not only startup.
    this.sharedRepos?.ensureBackgroundRefresh();
  }

  /**
   * The connections of `projectId` an unlink would remove if the user
   * confirms (Q31): Core answers the list (`shared.unlinkPreview`), so the
   * dialog and the unlink apply one rule (the project's repo and directory,
   * links not shared from here).
   */
  async importedConnectionsOf(projectId: string): Promise<DatabaseConnection[]> {
    let ids: string[];
    try {
      ({ importedConnectionIds: ids } = await getShared().unlinkPreview(projectId));
    } catch (error) {
      throw sharedError(error);
    }
    const wanted = new Set(ids);
    return this.state.connections.filter((c) => wanted.has(c.id));
  }

  /**
   * Unlink a project (Decision 40 with Q31). The user's own connections
   * stay, unlinked and local-only, with their passwords. The ones the repo
   * brought are listed through `ask` first: `"remove"` removes them,
   * `"keep"` keeps them like the others, and `null` (cancel) changes
   * nothing. With none, nothing is asked. True when the project was
   * unlinked. A refusal throws, worded for the user.
   */
  async unlinkWithConfirmation(
    projectId: string,
    ask: (
      projectName: string,
      imported: readonly DatabaseConnection[],
    ) => Promise<"remove" | "keep" | null>,
  ): Promise<boolean> {
    const imported = await this.importedConnectionsOf(projectId);
    let removeImported = false;
    if (imported.length > 0) {
      const name = this.state.projects.find((p) => p.id === projectId)?.name ?? "";
      const choice = await ask(name, imported);
      if (choice === null) return false;
      removeImported = choice === "remove";
    }
    await this.unlinkProject(projectId, removeImported);
    return true;
  }

  /**
   * Unlink a project (Decision 40 with Q31): Core keeps the user's own
   * connections (unlinked, local-only, their secrets kept), removes the
   * ones the repo brought when `removeImported` (else keeps them like the
   * others), clears the links and forgets the repo when no project uses
   * it. The page drops the removed connections and reads the kept ones
   * and the project again. A refusal throws, worded for the user.
   */
  async unlinkProject(projectId: string, removeImported: boolean): Promise<UnlinkReport> {
    let report: UnlinkReport;
    try {
      ({ value: report } = await getShared().unlinkProject(projectId, removeImported));
    } catch (error) {
      throw sharedError(error);
    }
    for (const id of report.removedConnectionIds) {
      const connection = this.state.connections.find((c) => c.id === id);
      if (connection) this.connectionManager?.forgetRemoved(connection, false);
    }
    if (report.keptConnectionIds.length > 0) {
      await this.connectionManager?.refreshFromLibrary(report.keptConnectionIds, { remote: false });
    }
    await this.refreshFromLibrary([projectId]);
    await this.sharedRepos?.loadRepos();
    return report;
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
    }
    // A linked project is synced with its files (Decision 35).
    if (id) await this.sharedRepos?.syncProject(id);
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
   * Import the repo's project directories `dirs` (Decision 40): Core makes
   * one project each (a taken name becomes the first free "<name> (2)"),
   * linked to `path` with that directory, and syncs it, which imports that
   * directory's templates. Each directory is imported whole or not at all;
   * one that failed is said. The first new project is made active. Returns
   * the new projects' ids.
   */
  async importProjects(path: string, dirs: string[]): Promise<string[]> {
    let projectIds: string[];
    let failures;
    try {
      ({
        value: { projectIds, failures },
      } = await getShared().importProjects(path, dirs));
    } catch (error) {
      throw sharedError(error);
    }
    await this.refreshFromLibrary(projectIds);
    await this.sharedRepos?.loadRepos();
    await this.connectionManager?.refreshFromLibrary(null, { remote: false });
    for (const failure of failures ?? []) errorToast(failureText(this.state, failure));
    if (projectIds.length > 0) this.sharedRepos?.ensureBackgroundRefresh();
    const first = projectIds[0];
    if (first !== undefined) await this.setActive(first);
    return projectIds;
  }

  // === PRIVATE METHODS ===

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
