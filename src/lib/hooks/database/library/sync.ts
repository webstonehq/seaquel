/**
 * `LibrarySync`: other windows' and tabs' library changes, applied to this
 * page (phase 5d-1, Decision 18). The `ChangeFeed` groups Core's
 * `storageChanged` events; this refetches what each group names and hands
 * it to the view model that shows it, which applies it by the `seq` rule
 * after this page's own writes to those rows have answered.
 *
 * - `connection`: the connections the event names (a deleted one is
 *   disconnected, with a toast).
 * - `project`: the projects (a deleted active one switches the page to
 *   another, with a toast), and the connection order of the ones loaded
 *   (`projectSidebarSet` announces a `project` change).
 * - `projectState`: a window's view state. Every other window ignores it
 *   (Decision 22: no cross-window reload); only an event naming this
 *   window's own id that the feed didn't skip as its own would reload it,
 *   which doesn't happen in practice (see `refreshViewState`).
 * - `label`: the project that holds the label, and every connection (a
 *   removed label is stripped from connections in any project).
 * - `savedQuery`: the project's saved queries and versions, if the page
 *   has them loaded; another project reads them when it's opened.
 * - `history`: a connection's history, if the page shows it. An event
 *   without a scope (a favourite set) is matched to its connection by the
 *   row id.
 * - `storage`: nothing (the vault, license and shared repos aren't shown live).
 * - `dashboard` (5d-2): the project's dashboards and versions, if the page
 *   holds them; one deleted elsewhere closes its tabs in every project, and
 *   one with an edit this page couldn't save shows the banner instead.
 * - `workflow`: the project's saved workflows; a deleted one unlinks the
 *   canvas showing it, which keeps its nodes.
 * - `chat`: the connection's chats; a deleted open chat stops its turn and
 *   another becomes active. `chatMessages`: the chat's messages, except
 *   the chat streaming here, which reads them once its turn is stored.
 * - `setting`, `aiSettings`, `theme`, `onboarding`, `tutorial`,
 *   `importState`: the settings stores read their record again and apply
 *   it (a theme at once, Q18).
 *
 * Every list is reloaded when events may have been missed: a new epoch,
 * and each (re)subscription of the event channel. One that comes while the
 * page is still loading reloads once the load is done.
 */
import { log } from "$lib/utils/logger";
import type { DatabaseState } from "../state.svelte.js";
import type { ChangeFeed, ReloadReason, StorageChange } from "./change-feed";
import type { StoredKind } from "./types";

/** The settings kinds, which the settings stores follow. */
const SETTINGS_KINDS = [
  "setting",
  "aiSettings",
  "theme",
  "onboarding",
  "tutorial",
  "importState",
] as const satisfies readonly StoredKind[];
export type SettingsKind = (typeof SETTINGS_KINDS)[number];

/** What `LibrarySync` refreshes through. */
export interface LibraryViews {
  connections: { refreshFromLibrary(ids: readonly string[] | null): Promise<void> };
  projects: { refreshFromLibrary(ids: readonly string[] | null): Promise<void> };
  savedQueries: {
    refreshFromLibrary(projectId: string, ids: readonly string[] | null): Promise<void>;
  };
  history: { reloadHistory(connectionId: string, replace: boolean): Promise<void> };
  projectsViewState?: { reloadViewState(projectId: string): Promise<void> };
  /** This page's window id (`windowId()`), for `projectState` events. */
  windowId?: () => string | null;
  /** Phase 5d-2: a project's dashboards (and their versions). */
  dashboards?: {
    refreshFromLibrary(projectId: string, ids: readonly string[] | null): Promise<void>;
  };
  /** A project's saved workflows. */
  workflows?: {
    refreshFromLibrary(projectId: string, ids: readonly string[] | null): Promise<void>;
  };
  /** A connection's chats, and a chat's messages. */
  chats?: {
    refreshChats(connectionId: string): Promise<void>;
    refreshMessages(chatId: string): Promise<void>;
  };
  /** The settings stores (`applyStoredChange`). */
  settings?: (kind: SettingsKind, ids: readonly string[] | null) => Promise<void>;
}

export class LibrarySync {
  private stops: Array<() => void> = [];
  private loaded = false;
  private reloadWhenLoaded = false;

  constructor(
    private readonly state: DatabaseState,
    private readonly feed: ChangeFeed,
    private readonly views: LibraryViews,
  ) {}

  /** Subscribe the feed. Call before the page's first library list. */
  start(): void {
    if (this.stops.length > 0) return;
    const { feed } = this;
    this.stops.push(
      feed.onReload((reason) => this.requestReload(reason)),
      feed.onStatus((reason) => {
        this.state.libraryUpdatesUnavailable = reason;
      }),
      feed.subscribe("connection", (c) =>
        this.run(c, () => this.views.connections.refreshFromLibrary(c.ids)),
      ),
      feed.subscribe("project", (c) =>
        this.run(c, () => this.views.projects.refreshFromLibrary(c.ids)),
      ),
      feed.subscribe("label", (c) =>
        this.run(c, async () => {
          await this.views.projects.refreshFromLibrary(c.scope ? [c.scope] : null);
          await this.views.connections.refreshFromLibrary(null);
        }),
      ),
      feed.subscribe("savedQuery", (c) => this.run(c, () => this.refreshSavedQueries(c))),
      feed.subscribe("projectState", (c) => this.run(c, () => this.refreshViewState(c))),
      feed.subscribe("history", (c) => this.run(c, () => this.refreshHistory(c))),
      feed.subscribe("dashboard", (c) => this.run(c, () => this.refreshDashboards(c))),
      feed.subscribe("workflow", (c) => this.run(c, () => this.refreshWorkflows(c))),
      feed.subscribe("chat", (c) => this.run(c, () => this.refreshChats(c))),
      feed.subscribe("chatMessages", (c) => this.run(c, () => this.refreshMessages(c))),
      ...SETTINGS_KINDS.map((kind) =>
        feed.subscribe(kind, (c) =>
          this.run(c, async () => {
            await this.views.settings?.(kind, c.ids);
          }),
        ),
      ),
    );
    feed.start();
    this.stops.push(() => feed.stop());
  }

  stop(): void {
    for (const stop of this.stops.splice(0)) stop();
  }

  /** The page's first load is done: a reload asked for during it runs now. */
  markLoaded(): void {
    this.loaded = true;
    if (this.reloadWhenLoaded) {
      this.reloadWhenLoaded = false;
      void this.reloadAll();
    }
  }

  private requestReload(reason: ReloadReason): void {
    void log.info(`Reloading the library lists (${reason.reason})`);
    if (!this.loaded) {
      this.reloadWhenLoaded = true;
      return;
    }
    void this.reloadAll();
  }

  /** Reload every list this page holds. */
  async reloadAll(): Promise<void> {
    const steps: Promise<void>[] = [
      this.views.projects.refreshFromLibrary(null),
      this.views.connections.refreshFromLibrary(null),
    ];
    for (const projectId of Object.keys(this.state.queriesByProject)) {
      steps.push(this.views.savedQueries.refreshFromLibrary(projectId, null));
    }
    for (const connectionId of Object.keys(this.state.queryHistoryByConnection)) {
      steps.push(this.views.history.reloadHistory(connectionId, true));
    }
    const { dashboards, workflows, chats, settings } = this.views;
    for (const projectId of Object.keys(this.state.dashboardsByProject)) {
      if (dashboards) steps.push(dashboards.refreshFromLibrary(projectId, null));
    }
    for (const projectId of Object.keys(this.state.savedWorkflowsByProject)) {
      if (workflows) steps.push(workflows.refreshFromLibrary(projectId, null));
    }
    if (chats) {
      for (const connectionId of Object.keys(this.state.aiChatsByConnection)) {
        steps.push(chats.refreshChats(connectionId));
      }
      // An open chat streaming here reads its messages once its turn is stored.
      for (const chatId of Object.keys(this.state.aiMessagesByChat)) {
        steps.push(chats.refreshMessages(chatId));
      }
    }
    if (settings) for (const kind of SETTINGS_KINDS) steps.push(settings(kind, null));
    const results = await Promise.allSettled(steps);
    for (const r of results) {
      if (r.status === "rejected") void log.warn("Reloading a library list failed:", r.reason);
    }
  }

  private async refreshSavedQueries(change: StorageChange): Promise<void> {
    const projects = change.scope ? [change.scope] : Object.keys(this.state.queriesByProject);
    await Promise.all(
      projects.map((p) => this.views.savedQueries.refreshFromLibrary(p, change.ids)),
    );
  }

  /**
   * Only this window's own view state, written by another page under its
   * id (Decision 22's rule). A no-op in practice: Core refuses a `ui` write
   * whose window id isn't the caller's origin, so such an event always
   * carries this window's id as its origin too, and the feed skips events
   * with this page's origin before they get here. Kept so the rule holds if
   * either check changes.
   */
  private async refreshViewState(change: StorageChange): Promise<void> {
    const own = this.views.windowId?.();
    if (!own || !change.scope || !change.ids?.includes(own)) return;
    await this.views.projectsViewState?.reloadViewState(change.scope);
  }

  private async refreshDashboards(change: StorageChange): Promise<void> {
    const view = this.views.dashboards;
    if (!view) return;
    const projects = change.scope ? [change.scope] : Object.keys(this.state.dashboardsByProject);
    await Promise.all(projects.map((p) => view.refreshFromLibrary(p, change.ids)));
  }

  private async refreshWorkflows(change: StorageChange): Promise<void> {
    const view = this.views.workflows;
    if (!view) return;
    const projects = change.scope
      ? [change.scope]
      : Object.keys(this.state.savedWorkflowsByProject);
    await Promise.all(projects.map((p) => view.refreshFromLibrary(p, change.ids)));
  }

  private async refreshChats(change: StorageChange): Promise<void> {
    const view = this.views.chats;
    if (!view) return;
    const connections = change.scope ? [change.scope] : Object.keys(this.state.aiChatsByConnection);
    await Promise.all(connections.map((c) => view.refreshChats(c)));
  }

  private async refreshMessages(change: StorageChange): Promise<void> {
    const view = this.views.chats;
    if (!view) return;
    const chats = change.scope ? [change.scope] : Object.keys(this.state.aiMessagesByChat);
    await Promise.all(chats.map((c) => view.refreshMessages(c)));
  }

  private async refreshHistory(change: StorageChange): Promise<void> {
    if (change.scope) {
      await this.views.history.reloadHistory(change.scope, change.ids === null);
      return;
    }
    // No scope (a favourite set): find the connections holding the rows.
    const ids = new Set(change.ids ?? []);
    const connections = Object.entries(this.state.queryHistoryByConnection)
      .filter(([, items]) => change.ids === null || items.some((h) => ids.has(h.id)))
      .map(([connectionId]) => connectionId);
    await Promise.all(connections.map((c) => this.views.history.reloadHistory(c, false)));
  }

  private run(change: StorageChange, refresh: () => Promise<void>): void {
    refresh().catch((error) => {
      void log.warn(`Applying another window's ${change.kind} change failed:`, error);
    });
  }
}
