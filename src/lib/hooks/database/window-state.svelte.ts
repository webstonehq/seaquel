import type {
  PersistedCreateTableTab,
  PersistedDashboardTab,
  PersistedDataTab,
  PersistedErdTab,
  PersistedExplainTab,
  PersistedProjectState,
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedStarterTab,
  PersistedStatisticsTab,
  PersistedWorkflowTab,
} from "$lib/types";
import { errorCode, wellFormedJson } from "$lib/core/client";
import { windowId as pageWindowId } from "$lib/core/window-id";
import { m } from "$lib/paraglide/messages.js";
import { skipUnloadedSave } from "$lib/storage/load-guard";
import { log } from "$lib/utils/logger";
import { errorToast } from "$lib/utils/toast";
import { getUi } from "./library/index.js";
import { storeConnectionOrderIfChanged } from "./library/view.js";
import type { UiService, ViewState, ViewStateLoaded } from "./library/types.js";
import type { DatabaseState } from "./state.svelte.js";

/** The load guard's key for one project's view state in this window. */
export type WindowStateLoadKey = `windowState:${string}`;

export interface WindowStateOptions {
  /** Defaults to the page's `UiService` (`getUi()`), read per call. */
  ui?: () => UiService;
  /** This window's id (`windowId()`); `null` while it isn't settled. */
  windowId?: () => string | null;
  /**
   * False for a window that keeps no view state (the desktop's theme editor):
   * it reads nothing and saves nothing, so it never gets a row of its own.
   */
  enabled?: boolean;
}

/** The limit a Core refusal names (`… (max_tabs: 500).`), if it names one. */
function limitNamed(error: unknown): string | null {
  const message = error instanceof Error ? error.message : String(error);
  return /\((max_[a-z_]+):/.exec(message)?.[1] ?? null;
}

/** Core's message without its `"CODE: "` prefix. */
function plainMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  const code = errorCode(error);
  return code && message.startsWith(`${code}: `) ? message.slice(code.length + 2) : message;
}

/**
 * This window's view state (phase 5d-2, Decision 22): its open tabs with
 * their text, pane layout, active ids and active view, per project, stored
 * by Core's `ui` group under the window's id. Another window of the same
 * project keeps its own; a window's first load of a project copies the most
 * recently used window's (or, the first time after the upgrade, today's
 * rows), so a new tab starts with what the user last had.
 *
 * - **Debounced**, 500 ms per project, as before.
 * - **Load before save.** A project's state is saved only after this
 *   window's `windowStateLoad` of it answered. Before, what's in memory is
 *   empty or another load's (a switch sets the active project before it
 *   loads it, and so does startup), and a save would write it over the
 *   stored one.
 * - **`rev`.** Each save carries the window's counter for the project, one
 *   more than the last: counted up from the `rev` the load answered, and past
 *   the stored `rev` a `stale` answer names (a save from another page of
 *   this window got there first; this page's state is saved again after
 *   it). Core stores a save only when its `rev` is higher, so the `pagehide`
 *   save, which leaves outside the write queue, can't be overwritten by an
 *   older save still queued.
 * - **Flush** saves only projects with a pending save, the active one first.
 * - **Page hide (web)**: the active project's pending save leaves as one
 *   `keepalive` request when it fits the browser's budget; the rest are
 *   flushed as far as they get.
 * - A save refused for a web limit (`max_view_state_bytes`, `max_tabs`,
 *   `max_tab_text_bytes`) is shown once per project and limit; later saves
 *   keep going (the user trims the tab to get under it).
 *
 * The saved workflows and the connection order aren't view state: they are
 * shared by the project's windows and stored through the library.
 */
export class WindowStateManager {
  readonly PERSISTENCE_DEBOUNCE_MS = 500;

  private readonly ui: () => UiService;
  private readonly windowId: () => string | null;
  private readonly enabled: boolean;

  private timers = new Map<string, ReturnType<typeof setTimeout>>();
  /** The last `rev` this page used or saw, per project. */
  private revs = new Map<string, number>();
  /** Projects whose view state this page has read (a state or none). */
  private loadedProjects = new Set<string>();
  /** Loads that failed or are still running: their saves are refused. */
  private failedLoads = new Set<WindowStateLoadKey>();
  private pendingLoads = new Set<WindowStateLoadKey>();
  /** Projects changed in memory since their restore (a save was scheduled). */
  private changed = new Set<string>();
  /** Each project's save on its way to Core, for `flush` to wait on. */
  private inflight = new Map<string, Promise<void>>();
  /**
   * Reads the window's view state of a project again (a stale save with
   * nothing changed here: another page of this window saved later).
   */
  private reload: (projectId: string) => Promise<void> = async () => {};
  /** Limit refusals already shown, as `project\u0001limit`. */
  private shownRefusals = new Set<string>();
  /**
   * Whether the projects were read at startup. Without them the active
   * project is an in-memory stand-in, whose state can't be stored and whose
   * id must not become the window's active project.
   */
  private projectsLoaded = true;

  constructor(
    private state: DatabaseState,
    options: WindowStateOptions = {},
  ) {
    this.ui = options.ui ?? getUi;
    this.windowId = options.windowId ?? pageWindowId;
    this.enabled = options.enabled ?? true;
  }

  /** Set by `ProjectManager.initialize`. */
  setProjectsLoaded(loaded: boolean): void {
    this.projectsLoaded = loaded;
  }

  /** How a project's view state is read again and restored (`ProjectManager`). */
  setReloader(reload: (projectId: string) => Promise<void>): void {
    this.reload = reload;
  }

  /** True if the last load of `projectId`'s view state failed (or is running). */
  loadFailed(projectId: string): boolean {
    return this.failedLoads.has(`windowState:${projectId}`);
  }

  // -------- Scheduling --------

  /** Save `projectId`'s view state after the debounce. */
  scheduleProject(projectId: string | null): void {
    if (!projectId) return;
    if (this.loadedProjects.has(projectId)) this.changed.add(projectId);
    const existing = this.timers.get(projectId);
    if (existing) clearTimeout(existing);
    this.timers.set(
      projectId,
      setTimeout(() => {
        this.timers.delete(projectId);
        void this.saveNow(projectId);
      }, this.PERSISTENCE_DEBOUNCE_MS),
    );
  }

  /** Drop every pending save. */
  cancelPending(): void {
    for (const timer of this.timers.values()) clearTimeout(timer);
    this.timers.clear();
  }

  /** Drop `projectId`'s pending save, leaving other projects' alone. */
  cancelPendingFor(projectId: string): void {
    const timer = this.timers.get(projectId);
    if (timer) {
      clearTimeout(timer);
      this.timers.delete(projectId);
    }
  }

  /**
   * A removed project: its pending save is dropped, and it counts as not
   * loaded, so nothing saves it again (the save would fail on the removed
   * project, and on web could use up the one request that leaves on unload).
   */
  forgetProject(projectId: string): void {
    this.cancelPendingFor(projectId);
    this.loadedProjects.delete(projectId);
    this.changed.delete(projectId);
    this.revs.delete(projectId);
    delete this.state.activeViewByProject[projectId];
  }

  /**
   * Save now every project with a pending save, the active one first, and
   * wait for the saves already on their way. A stale answer schedules its
   * project again; those are saved too, for at most two more rounds.
   */
  async flush(): Promise<void> {
    for (let round = 0; round < 3; round++) {
      const active = this.state.activeProjectId;
      const pending = [...this.timers.keys()].sort(
        (a, b) => Number(b === active) - Number(a === active),
      );
      this.cancelPending();
      for (const projectId of pending) await this.saveNow(projectId);
      await Promise.allSettled(this.inflight.values());
      if (this.timers.size === 0) return;
    }
  }

  /**
   * The page is going away (web, `pagehide`). The active project's pending
   * save leaves as one `keepalive` request, outside the write queue, when
   * it's small enough for the browser to carry after the page is gone;
   * everything else is flushed the usual way, as far as it gets.
   */
  saveOnPageHide(): void {
    const projectId = this.state.activeProjectId;
    const windowId = this.enabled ? this.windowId() : null;
    if (projectId && windowId && this.timers.has(projectId) && this.canSave(projectId)) {
      try {
        const rev = this.nextRev(projectId);
        if (
          this.ui().windowStateSaveKeepalive(windowId, projectId, rev, this.sendable(projectId))
        ) {
          this.cancelPendingFor(projectId);
        }
      } catch (error) {
        void log.error(
          `Failed to send the view state of project ${projectId} on page hide:`,
          error,
        );
      }
    }
    void this.flush();
  }

  // -------- Loading and saving --------

  /**
   * This window's view state of `projectId` (`windowStateLoad`): its own,
   * or on its first load a copy (Decision 22). `null` when the load failed:
   * the project's saves are then refused until a later load succeeds.
   *
   * An answered load still leaves the project unsaveable, as pending, until
   * the caller has put the state in memory and calls `markLoaded`: a save
   * in between would store what the page showed before the restore.
   *
   * `reload`: the project is already restored here and is read again (a
   * stale save with nothing changed). A failed reload leaves it saveable at
   * its rev, with what the page shows.
   */
  async load(projectId: string, { reload = false } = {}): Promise<ViewStateLoaded | null> {
    const key: WindowStateLoadKey = `windowState:${projectId}`;
    if (!this.enabled) return null;
    const windowId = this.windowId();
    this.failedLoads.add(key);
    this.pendingLoads.add(key);
    try {
      if (windowId === null) throw new Error("the window id isn't settled");
      const { value } = await this.ui().windowStateLoad(windowId, projectId);
      // Count this window's saves on from the stored one.
      this.revs.set(projectId, Math.max(this.revs.get(projectId) ?? 0, value.rev));
      if (value.copiedFrom && value.copiedFrom !== "empty") {
        void log.info(`View state of project ${projectId} copied from ${value.copiedFrom}`);
      }
      // Still pending until `markLoaded`.
      return value;
    } catch (error) {
      void log.error(`Failed to load the view state of project ${projectId}:`, error);
      this.pendingLoads.delete(key);
      if (reload && this.loadedProjects.has(projectId)) this.failedLoads.delete(key);
      return null;
    }
  }

  /**
   * A reload whose state the page won't apply (the project is no longer the
   * active one): it counts as not loaded, without a failure, so nothing
   * saves the page's older copy over the stored state, and the next switch
   * to it reads it again.
   */
  dropLoad(projectId: string): void {
    const key: WindowStateLoadKey = `windowState:${projectId}`;
    this.pendingLoads.delete(key);
    this.failedLoads.delete(key);
    this.loadedProjects.delete(projectId);
  }

  /**
   * The state `load` answered is in memory (`restored`), so the project's
   * saves may go out from now on; or the restore failed, and they stay
   * refused until a later load succeeds.
   */
  markLoaded(projectId: string, restored = true): void {
    const key: WindowStateLoadKey = `windowState:${projectId}`;
    this.pendingLoads.delete(key);
    if (!restored) return;
    this.failedLoads.delete(key);
    this.loadedProjects.add(projectId);
    this.changed.delete(projectId);
  }

  /**
   * Save `projectId`'s view state now (clearing its pending save).
   * `leaving`: the page is switching away from it, so a stale answer never
   * reads it again.
   */
  async saveNow(projectId: string, { leaving = false } = {}): Promise<void> {
    // Saved now: a timer still pending would save it again later, after a
    // switch, from whatever is in memory then.
    this.cancelPendingFor(projectId);
    if (!this.canSave(projectId)) return;
    const windowId = this.windowId();
    if (windowId === null) return;
    const rev = this.nextRev(projectId);
    const saving = this.save(windowId, projectId, rev, leaving);
    this.inflight.set(projectId, saving);
    try {
      await saving;
    } finally {
      if (this.inflight.get(projectId) === saving) this.inflight.delete(projectId);
    }
  }

  private async save(
    windowId: string,
    projectId: string,
    rev: number,
    leaving: boolean,
  ): Promise<void> {
    try {
      // The order is the project's (not view state): stored first when a
      // connection was appended to it since it was read.
      await storeConnectionOrderIfChanged(this.state, projectId);
      const { value } = await this.ui().windowStateSave(
        windowId,
        projectId,
        rev,
        this.sendable(projectId),
      );
      if (value.stale) this.onStale(projectId, value.rev, leaving);
    } catch (error) {
      this.reportSaveFailure(projectId, error);
    }
  }

  /**
   * Another page of this window stored a later save (`storedRev`). Count on
   * past it. If this page changed the project since its restore, save what
   * it shows again; if not, the stored state is newer than what the page
   * shows (the old page's `pagehide` save landed after this page's load),
   * so read it again instead of saving over it: only for the active
   * project, and not for the save a switch makes on its way out (that would
   * cancel the old project's runs and race the new one's load). Another
   * project counts as not loaded, and its next switch reads it again.
   *
   * Only a save with no change since the restore is covered: a change here
   * (the user's edit, auto-reconnect's active connection) still counts past
   * the stored rev and overwrites the other page's newer save (accepted).
   */
  private onStale(projectId: string, storedRev: number, leaving: boolean): void {
    this.revs.set(projectId, Math.max(this.revs.get(projectId) ?? 0, storedRev));
    void log.info(`View state save of project ${projectId} was stale (stored rev ${storedRev})`);
    if (!this.loadedProjects.has(projectId)) return;
    if (this.changed.has(projectId)) {
      this.scheduleProject(projectId);
      return;
    }
    if (leaving || projectId !== this.state.activeProjectId) {
      this.dropLoad(projectId);
      return;
    }
    this.reload(projectId).catch((error: unknown) => {
      void log.error(`Failed to read the view state of project ${projectId} again:`, error);
    });
  }

  /** The next `rev` for `projectId`: one more than the last used or seen. */
  private nextRev(projectId: string): number {
    const rev = (this.revs.get(projectId) ?? 0) + 1;
    this.revs.set(projectId, rev);
    return rev;
  }

  /** Whether `projectId`'s view state may be saved, reporting why not. */
  private canSave(projectId: string): boolean {
    if (!this.enabled) return false;
    if (!this.projectsLoaded) {
      skipUnloadedSave(`the view state of project ${projectId}`, { pending: false });
      return false;
    }
    const key: WindowStateLoadKey = `windowState:${projectId}`;
    if (this.failedLoads.has(key)) {
      skipUnloadedSave(`the view state of project ${projectId}`, {
        pending: this.pendingLoads.has(key),
      });
      return false;
    }
    if (!this.loadedProjects.has(projectId)) {
      // Not loaded by this page yet (a switch or startup still loading it,
      // or a removed project): what's in memory isn't its state.
      void log.debug(`Not saving the view state of project ${projectId}: not loaded yet`);
      return false;
    }
    return true;
  }

  /**
   * A failed save is logged. One refused for a web limit is also shown, once
   * per project and limit, naming the limit (and the tab, for a tab's text).
   */
  private reportSaveFailure(projectId: string, error: unknown): void {
    void log.error(`Failed to save the view state of project ${projectId}:`, error);
    const limit = errorCode(error) === "INVALID_ARGUMENT" ? limitNamed(error) : null;
    if (!limit) return;
    const key = `${projectId}\u0001${limit}`;
    if (this.shownRefusals.has(key)) return;
    this.shownRefusals.add(key);
    errorToast(m.view_state_save_refused({ message: plainMessage(error) }));
  }

  // -------- The window's active project --------

  /**
   * The project this window shows first (`windowGet`): its own, else the
   * most recently used window's, else `lastActiveProjectId`. `null` when
   * there is none, the read failed, or this window keeps no view state.
   */
  async activeProject(): Promise<string | null> {
    if (!this.enabled) return null;
    const windowId = this.windowId();
    if (windowId === null) return null;
    try {
      return (await this.ui().windowGet(windowId)).value.activeProjectId;
    } catch (error) {
      void log.error("Failed to read the window's active project:", error);
      return null;
    }
  }

  /**
   * Record `projectId` as this window's active project (`windowActivate`,
   * which also writes `lastActiveProjectId` for older releases). Skipped
   * when the projects couldn't be read, since the active one is then an
   * in-memory stand-in.
   */
  async activate(projectId: string): Promise<void> {
    if (!this.enabled) return;
    if (!this.projectsLoaded) {
      skipUnloadedSave("the active project", { pending: false });
      return;
    }
    const windowId = this.windowId();
    if (windowId === null) return;
    try {
      await this.ui().windowActivate(windowId, projectId);
    } catch (error) {
      void log.error(`Failed to record project ${projectId} as the window's active one:`, error);
    }
  }

  // -------- Serializing --------

  /** The view last shown in `projectId`: live for the active project, else as it was left. */
  private activeViewOf(projectId: string): PersistedProjectState["activeView"] {
    return (
      this.state.activeViewByProject[projectId] ??
      (projectId === this.state.activeProjectId ? this.state.activeView : "query")
    );
  }

  /** The view state of `projectId` as this window shows it. */
  /**
   * The view state as sent: `buildState` with every string well-formed,
   * as run text is, since Core refuses a lone surrogate (which a tab's
   * text can hold) and would then refuse every save of the project (5d-2
   * Task 7 probe). The page keeps showing what it has.
   */
  private sendable(projectId: string): ViewState {
    return wellFormedJson(this.buildState(projectId));
  }

  buildState(projectId: string): ViewState {
    const s = this.state;
    return {
      projectId,
      queryTabs: this.serializeQueryTabs(projectId),
      schemaTabs: this.serializeSchemaTabs(projectId),
      explainTabs: this.serializeExplainTabs(projectId),
      erdTabs: this.serializeErdTabs(projectId),
      statisticsTabs: this.serializeStatisticsTabs(projectId),
      workflowTabs: this.serializeWorkflowTabs(projectId),
      tabOrder: s.tabOrderByProject[projectId] ?? [],
      activeQueryTabId: s.activeQueryTabIdByProject[projectId] ?? null,
      activeSchemaTabId: s.activeSchemaTabIdByProject[projectId] ?? null,
      activeExplainTabId: s.activeExplainTabIdByProject[projectId] ?? null,
      activeErdTabId: s.activeErdTabIdByProject[projectId] ?? null,
      activeStatisticsTabId: s.activeStatisticsTabIdByProject[projectId] ?? null,
      activeWorkflowTabId: s.activeWorkflowTabIdByProject[projectId] ?? null,
      activeView: this.activeViewOf(projectId),
      // Per window (Q14).
      activeConnectionId: s.activeConnectionIdByProject[projectId] ?? null,
      starterTabs: this.serializeStarterTabs(projectId),
      activeStarterTabId: s.activeStarterTabIdByProject[projectId] ?? null,
      connectionTabs: [],
      activeConnectionTabId: null,
      dashboardTabs: this.serializeDashboardTabs(projectId),
      activeDashboardTabId: s.activeDashboardTabIdByProject[projectId] ?? null,
      createTableTabs: this.serializeCreateTableTabs(projectId),
      activeCreateTableTabId: s.activeCreateTableTabIdByProject[projectId] ?? null,
      dataTabs: this.serializeDataTabs(projectId),
      activeDataTabId: s.activeDataTabIdByProject[projectId] ?? null,
      // Kept in the window's row (the legacy mirror has no column for them).
      extensionsDuckdbTabs: this.serializeExtensionsDuckdbTabs(projectId),
      activeExtensionsDuckdbTabId: s.activeExtensionsDuckdbTabIdByProject[projectId] ?? null,
      paneLayout: this.serializePaneLayout(projectId),
    };
  }

  serializeQueryTabs(projectId: string): PersistedQueryTab[] {
    return (this.state.queryTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      query: tab.query,
      queryId: tab.queryId,
    }));
  }

  serializeSchemaTabs(projectId: string): PersistedSchemaTab[] {
    return (this.state.schemaTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      tableName: tab.table.name,
      schemaName: tab.table.schema,
      connectionId: tab.connectionId,
    }));
  }

  serializeExplainTabs(projectId: string): PersistedExplainTab[] {
    return (this.state.explainTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      sourceQuery: tab.sourceQuery,
    }));
  }

  serializeErdTabs(projectId: string): PersistedErdTab[] {
    return (this.state.erdTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeStatisticsTabs(projectId: string): PersistedStatisticsTab[] {
    return (this.state.statisticsTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeWorkflowTabs(projectId: string): PersistedWorkflowTab[] {
    return (this.state.workflowTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeStarterTabs(projectId: string): PersistedStarterTab[] {
    return (this.state.starterTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      type: tab.type,
      name: tab.name,
      closable: tab.closable,
    }));
  }

  serializeDashboardTabs(projectId: string): PersistedDashboardTab[] {
    return (this.state.dashboardTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      dashboardId: tab.dashboardId,
    }));
  }

  serializeCreateTableTabs(projectId: string): PersistedCreateTableTab[] {
    return (this.state.createTableTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      connectionId: tab.connectionId,
      name: tab.name,
      tableDefinition: JSON.stringify(tab.tableDefinition),
    }));
  }

  serializeDataTabs(projectId: string): PersistedDataTab[] {
    return (this.state.dataTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      connectionId: tab.connectionId,
      tableName: tab.tableName,
      schemaName: tab.schemaName,
    }));
  }

  serializeExtensionsDuckdbTabs(
    projectId: string,
  ): { id: string; name: string; connectionId: string }[] {
    return (this.state.extensionsDuckdbTabsByProject[projectId] ?? []).map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializePaneLayout(projectId: string): PersistedProjectState["paneLayout"] | undefined {
    const layout = this.state.paneLayoutByProject[projectId];
    if (!layout || layout.panes.length <= 1) return undefined;
    return {
      panes: layout.panes.map((p) => ({
        id: p.id,
        tabIds: p.tabIds,
        activeTabId: p.activeTabId,
      })),
      activePaneId: layout.activePaneId,
    };
  }
}
