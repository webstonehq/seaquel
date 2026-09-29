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
 *   another, with a toast).
 * - `label`: the project that holds the label, and every connection (a
 *   removed label is stripped from connections in any project).
 * - `savedQuery`: the project's saved queries and versions, if the page
 *   has them loaded; another project reads them when it's opened.
 * - `history`: a connection's history, if the page shows it. An event
 *   without a scope (a favourite set) is matched to its connection by the
 *   row id.
 * - `storage`: nothing yet (phase 5d-2 moves those writes).
 *
 * Every list is reloaded when events may have been missed: a new epoch,
 * and each (re)subscription of the event channel. One that comes while the
 * page is still loading reloads once the load is done.
 */
import { log } from "$lib/utils/logger";
import type { DatabaseState } from "../state.svelte.js";
import type { ChangeFeed, ReloadReason, StorageChange } from "./change-feed";

/** What `LibrarySync` refreshes through. */
export interface LibraryViews {
  connections: { refreshFromLibrary(ids: readonly string[] | null): Promise<void> };
  projects: { refreshFromLibrary(ids: readonly string[] | null): Promise<void> };
  savedQueries: {
    refreshFromLibrary(projectId: string, ids: readonly string[] | null): Promise<void>;
  };
  history: { reloadHistory(connectionId: string, replace: boolean): Promise<void> };
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
      feed.subscribe("history", (c) => this.run(c, () => this.refreshHistory(c))),
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
