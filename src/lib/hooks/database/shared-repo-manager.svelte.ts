import type { GitCredentials, RepoSyncStatus, SharedQueryRepo, SyncState } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import * as gitService from "$lib/services/git";
import { log } from "$lib/utils/logger";
import { extractErrorMessage } from "$lib/errors";
import { errorCode } from "$lib/core/client";
import { errorToast } from "$lib/utils/toast";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { deserializeRepo } from "$lib/types";
import { getShared, type RepoPreview, type SyncReport } from "./shared/index.js";
import { sayReport } from "./shared/notices.js";

const DEFAULT_SYNC_STATE: SyncState = {
  isSyncing: false,
  pendingChanges: 0,
  aheadBy: 0,
  behindBy: 0,
  conflictFiles: [],
};

/** A repo path without trailing separators, for comparing the page's copies. */
function pathKey(path: string): string {
  return path.replace(/[/\\]+$/, "");
}

/** How the manager shows what a sync or publish changed. */
export interface SharedViews {
  /**
   * Read a project's rows again (saved queries, dashboards, connections,
   * the project and its connection order). A sync's own events carry this
   * page's origin, which the change feed skips, so the page reads them.
   */
  refreshProjectRows(projectId: string): Promise<void>;
}

/**
 * The repo list, git and the sync as the GUI shows them (phase 5e).
 *
 * Core owns the projection: the files, their pairing with rows, the
 * three-way sync, the repo lock and the repo list's storage (the `shared`
 * group). This keeps what the page shows: the repos as Core lists them, the
 * git status of each (the sync button's counts and state, held in memory
 * only: a status refresh writes nothing, bug 20), and the conflict dialog's
 * state. A pull, a commit and a conflict resolution sync every project
 * linked to the repo; the git calls take Core's repo lock and
 * record `lastSyncAt` themselves.
 */
export class SharedRepoManager {
  /** Interval ID for background refresh */
  private refreshIntervalId: ReturnType<typeof setInterval> | null = null;

  /** Per-repo lock: the sync button's state (Core keeps its own lock for the files). */
  private repoLocks = new Map<string, Promise<void>>();

  /** The status each repo last showed, to sync when the background refresh sees it change. */
  private lastStatus = new Map<string, string>();

  /** Default refresh interval in milliseconds (5 minutes) */
  private static readonly DEFAULT_REFRESH_INTERVAL = 5 * 60 * 1000;

  private views: SharedViews | null = null;

  constructor(private state: DatabaseState) {}

  setViews(views: SharedViews): void {
    this.views = views;
  }

  // -------- The repo list --------

  /**
   * Read the repo list from Core. Each repo keeps the status the page shows
   * (`syncStatus` is the page's view, derived from git status). A failed
   * read is logged and leaves the list shown.
   */
  async loadRepos(): Promise<void> {
    try {
      const { value } = await getShared().listRepos();
      const shown = new Map(this.state.sharedRepos.map((r) => [r.id, r]));
      this.state.sharedRepos = value.map((row) => {
        const repo = deserializeRepo(row);
        const before = shown.get(repo.id);
        return before ? { ...repo, syncStatus: before.syncStatus } : repo;
      });
      for (const repo of this.state.sharedRepos) {
        if (!this.state.syncStateByRepo[repo.id]) this.updateSyncState(repo.id, {});
      }
    } catch (error) {
      void log.warn(`Reading the shared repos failed (${errorCode(error) ?? "unknown"})`);
    }
  }

  /**
   * Another window wrote repo `ids` (all when `null`): its row or a file of
   * its projects. Read the list again and each named repo's status. With
   * `status: false` (another process wrote the file, phase 7a: no repo is
   * named, and a `git status` per repo every second would be wasted) only
   * the list is read.
   */
  async refreshRepos(
    ids: readonly string[] | null,
    { status = true }: { status?: boolean } = {},
  ): Promise<void> {
    await this.loadRepos();
    if (!status) return;
    const repos = this.state.sharedRepos.filter((r) => ids === null || ids.includes(r.id));
    await Promise.all(repos.map((r) => this.refreshRepoStatus(r.id)));
  }

  /** Show `row` (a register or update answer) in the list. */
  private showRepo(row: Parameters<typeof deserializeRepo>[0]): SharedQueryRepo {
    const repo = deserializeRepo(row);
    const before = this.state.sharedRepos.find((r) => r.id === repo.id);
    const shown = before ? { ...repo, syncStatus: before.syncStatus } : repo;
    this.state.sharedRepos = before
      ? this.state.sharedRepos.map((r) => (r.id === repo.id ? shown : r))
      : [...this.state.sharedRepos, shown];
    if (!this.state.syncStateByRepo[repo.id]) this.updateSyncState(repo.id, {});
    return shown;
  }

  /**
   * The repo a project is linked to (its git path), or `null` for a project
   * without one or whose repo isn't listed. Everything for a project
   * resolves its repo here, never from another project's (bug 5).
   */
  repoForProject(projectId: string | null | undefined): SharedQueryRepo | null {
    if (!projectId) return null;
    const project = this.state.projects.find((p) => p.id === projectId);
    if (!project?.gitRepoPath) return null;
    const key = pathKey(project.gitRepoPath);
    return this.state.sharedRepos.find((r) => pathKey(r.path) === key) ?? null;
  }

  /** The projects linked to repo `repoId`. */
  private projectsOf(repo: SharedQueryRepo): string[] {
    const key = pathKey(repo.path);
    return this.state.projects
      .filter((p) => p.gitRepoPath && pathKey(p.gitRepoPath) === key)
      .map((p) => p.id);
  }

  /**
   * Serialize async operations per-repo to prevent concurrent mutations.
   */
  private async withRepoLock<T>(repoId: string, fn: () => Promise<T>): Promise<T> {
    const existing = this.repoLocks.get(repoId) ?? Promise.resolve();
    let resolve: () => void;
    const next = new Promise<void>((r) => {
      resolve = r;
    });
    this.repoLocks.set(repoId, next);

    await existing;
    try {
      return await fn();
    } finally {
      resolve!();
      if (this.repoLocks.get(repoId) === next) {
        this.repoLocks.delete(repoId);
      }
    }
  }

  /**
   * Find a repo by ID or throw a descriptive error.
   */
  private getRepoOrThrow(repoId: string): SharedQueryRepo {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) throw new Error(`Repository not found: ${repoId}`);
    return repo;
  }

  /**
   * Clone a repository from a remote URL and register it (Core makes its id).
   */
  async cloneRepo(
    name: string,
    remoteUrl: string,
    localPath: string,
    credentials?: GitCredentials,
  ): Promise<string> {
    await gitService.cloneRepo(remoteUrl, localPath, credentials);
    const { value } = await getShared().registerRepo(localPath, { name, remoteUrl });
    const repo = this.showRepo(value);
    await this.refreshRepoStatus(repo.id);
    return repo.id;
  }

  /**
   * Set the remote URL for a repository: git's remote, then the stored row.
   */
  async setRemoteUrl(repoId: string, url: string): Promise<void> {
    const repo = this.getRepoOrThrow(repoId);
    await gitService.setRemote(repo.path, url);
    const { value } = await getShared().updateRepo(repoId, { remoteUrl: url });
    this.showRepo(value);
  }

  // -------- Git --------

  /**
   * Pull changes from remote, then sync every project linked to the repo.
   * Conflicts open the conflict dialog. `"updated"` when the
   * pull went through, `"conflicted"` when it left conflicts (the caller
   * then says nothing about an update).
   */
  async pullRepo(
    repoId: string,
    credentials?: GitCredentials,
  ): Promise<"updated" | "conflicted" | "unchanged"> {
    const repo = this.getRepoOrThrow(repoId);

    return this.withRepoLock(repoId, async () => {
      this.updateSyncState(repoId, { isSyncing: true, lastError: undefined });

      try {
        const result = await gitService.pullRepo(repo.path, credentials);

        if (result.success) {
          await this.syncRepo(repoId);
          // Core recorded `lastSyncAt`.
          await this.loadRepos();
          await this.refreshRepoStatus(repoId);
          return "updated";
        }
        if (result.conflicts.length > 0) {
          this.updateSyncState(repoId, { conflictFiles: result.conflicts });
          this.setSyncStatus(repoId, "diverged");
          this.openConflict(repoId, result.conflicts);
          return "conflicted";
        }
        return "unchanged";
      } catch (error) {
        const message = extractErrorMessage(error);
        this.updateSyncState(repoId, { lastError: message });
        this.setSyncStatus(repoId, "error");
        throw error;
      } finally {
        this.updateSyncState(repoId, { isSyncing: false });
      }
    });
  }

  /**
   * Push local changes to remote.
   */
  async pushRepo(repoId: string, credentials?: GitCredentials): Promise<void> {
    const repo = this.getRepoOrThrow(repoId);

    return this.withRepoLock(repoId, async () => {
      this.updateSyncState(repoId, { isSyncing: true, lastError: undefined });

      try {
        const result = await gitService.pushRepo(repo.path, credentials);

        if (result.success) {
          await this.loadRepos();
          await this.refreshRepoStatus(repoId);
        }
      } catch (error) {
        const message = extractErrorMessage(error);
        this.updateSyncState(repoId, { lastError: message });
        // seaquel-git's PUSH_REJECTED_NON_FAST_FORWARD: pull first. Any other
        // refusal (a protected branch, a hook) is an error with its reason.
        if (message.includes("rejected (non-fast-forward)")) {
          this.setSyncStatus(repoId, "behind");
        } else {
          this.setSyncStatus(repoId, "error");
        }
        throw error;
      } finally {
        this.updateSyncState(repoId, { isSyncing: false });
      }
    });
  }

  /**
   * Commit all pending changes, then sync the repo's projects. A failure
   * is recorded on the repo and thrown.
   */
  async commitChanges(repoId: string, message: string): Promise<string> {
    const repo = this.getRepoOrThrow(repoId);

    return this.withRepoLock(repoId, async () => {
      try {
        const commitId = await gitService.commitChanges(repo.path, message);
        await this.syncRepo(repoId);
        await this.refreshRepoStatus(repoId);
        return commitId;
      } catch (error) {
        this.updateSyncState(repoId, { lastError: extractErrorMessage(error) });
        throw error;
      }
    });
  }

  /**
   * Resolve one conflicted file with `resolution`; once none is left, the
   * repo's projects are synced.
   */
  /** `resolution` `null` keeps a side that deleted the file. */
  async resolveConflict(
    repoId: string,
    filePath: string,
    resolution: string | null,
  ): Promise<void> {
    const repo = this.getRepoOrThrow(repoId);
    await gitService.resolveConflict(repo.path, filePath, resolution);
    await this.refreshRepoStatus(repoId);
    if ((this.state.syncStateByRepo[repoId]?.conflictFiles.length ?? 0) === 0) {
      await this.syncRepo(repoId);
    }
  }

  // -------- The sync --------

  /**
   * Sync a linked project with its files. Nothing for a project without a
   * link. A refusal is said; the answer is `null` then.
   */
  async syncProject(projectId: string): Promise<SyncReport | null> {
    const repo = this.repoForProject(projectId);
    if (!repo) return null;
    let report: SyncReport;
    try {
      ({ value: report } = await getShared().sync({ projectId }));
    } catch (error) {
      this.syncFailed(error);
      return null;
    }
    await this.showReport(repo, [projectId], report);
    return report;
  }

  /** Sync every project linked to repo `repoId`. A refusal is said. */
  async syncRepo(repoId: string): Promise<SyncReport | null> {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return null;
    let report: SyncReport;
    try {
      ({ value: report } = await getShared().sync({ repoId }));
    } catch (error) {
      this.syncFailed(error);
      return null;
    }
    await this.showReport(repo, this.projectsOf(repo), report);
    return report;
  }

  private syncFailed(error: unknown): void {
    void log.warn(`A shared sync failed (${errorCode(error) ?? "unknown"})`);
    errorToast(m.shared_sync_project_failed({ message: extractErrorMessage(error) }));
  }

  /**
   * Show a sync's outcome for the projects it covered: a conflicted repo
   * opens the conflict dialog (nothing was read or written); otherwise the
   * rows it changed are read again, then its notices are said.
   */
  async showReport(repo: SharedQueryRepo, projectIds: string[], report: SyncReport): Promise<void> {
    this.markSkipped(projectIds, report);
    if (report.conflicted) {
      await this.refreshRepoStatus(repo.id);
      const files = this.state.syncStateByRepo[repo.id]?.conflictFiles ?? [];
      toast.warning(m.shared_sync_conflicted({ name: repo.name }));
      this.openConflict(repo.id, files);
      return;
    }
    if (report.rowsChanged > 0 || report.filesWritten > 0) {
      await Promise.all(projectIds.map((id) => this.refreshRows(id)));
    }
    if (report.filesWritten > 0) await this.refreshRepoStatus(repo.id);
    sayReport(this.state, report);
  }

  /**
   * The projects this sync skipped whole are marked (their
   * sync status shows it); the others it read are cleared. A conflicted
   * sync read nothing and changes nothing.
   */
  private markSkipped(projectIds: string[], report: SyncReport): void {
    if (report.conflicted) return;
    const next = { ...this.state.sharedSyncSkipped };
    for (const id of projectIds) delete next[id];
    for (const s of report.skippedProjects ?? []) next[s.projectId] = s.why;
    this.state.sharedSyncSkipped = next;
  }

  private async refreshRows(projectId: string): Promise<void> {
    try {
      await this.views?.refreshProjectRows(projectId);
    } catch (error) {
      void log.warn("Reading a project's rows again after a sync failed:", error);
    }
  }

  /** A publish synced the project (`FILE_CHANGED`): its rows and its repo's status. */
  async refreshAfterProjection(projectId: string): Promise<void> {
    await this.refreshRows(projectId);
    await this.refreshProjectStatus(projectId);
  }

  /** A publish wrote or deleted a file of the project: its repo's status. */
  async refreshProjectStatus(projectId: string): Promise<void> {
    const repo = this.repoForProject(projectId);
    if (repo) await this.refreshRepoStatus(repo.id);
  }

  /** What the repo at `path` holds (the import dialog's preview). */
  scan(path: string): Promise<RepoPreview> {
    return getShared().scan(path);
  }

  // -------- The conflict dialog --------

  /** Open the conflict dialog for `repoId`'s conflicted files. */
  openConflict(repoId: string, files: string[]): void {
    this.state.sharedConflict = { repoId, files: [...files] };
  }

  closeConflict(): void {
    this.state.sharedConflict = null;
  }

  // -------- Status --------

  /**
   * Refresh the Git status for a repository. A status that can't be read is
   * shown on the repo (`statusUnreadable`, the error, `syncStatus` error)
   * instead of passing silently (M4).
   */
  async refreshRepoStatus(repoId: string): Promise<void> {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    try {
      const status = await gitService.getRepoStatus(repo.path);

      this.updateSyncState(repoId, {
        pendingChanges: status.pendingChanges,
        aheadBy: status.aheadBy,
        behindBy: status.behindBy,
        conflictFiles: status.hasConflicts ? status.conflictFiles : [],
        statusUnreadable: false,
      });

      let syncStatus: RepoSyncStatus = "synced";
      if (status.hasConflicts) {
        syncStatus = "diverged";
      } else if (status.aheadBy > 0 && status.behindBy > 0) {
        syncStatus = "diverged";
      } else if (status.aheadBy > 0) {
        syncStatus = "ahead";
      } else if (status.behindBy > 0) {
        syncStatus = "behind";
      }
      this.setSyncStatus(repoId, syncStatus);
    } catch (error) {
      void log.warn(`A repo's git status couldn't be read (${errorCode(error) ?? "unknown"})`);
      this.updateSyncState(repoId, {
        lastError: extractErrorMessage(error),
        statusUnreadable: true,
      });
      this.setSyncStatus(repoId, "error");
    }
  }

  /** The page's view of a repo's state: in memory only (bug 20). */
  private setSyncStatus(repoId: string, syncStatus: RepoSyncStatus): void {
    this.state.sharedRepos = this.state.sharedRepos.map((r) =>
      r.id === repoId && r.syncStatus !== syncStatus ? { ...r, syncStatus } : r,
    );
  }

  /**
   * Update sync state for a repository.
   */
  private updateSyncState(repoId: string, updates: Partial<SyncState>): void {
    const current = this.state.syncStateByRepo[repoId] ?? { ...DEFAULT_SYNC_STATE };

    this.state.syncStateByRepo = {
      ...this.state.syncStateByRepo,
      [repoId]: { ...current, ...updates },
    };
  }

  /**
   * Start background refresh of repo statuses.
   * @param intervalMs - Refresh interval in milliseconds (default: 5 minutes)
   */
  startBackgroundRefresh(intervalMs?: number): void {
    this.stopBackgroundRefresh();

    const interval = intervalMs ?? SharedRepoManager.DEFAULT_REFRESH_INTERVAL;

    this.refreshIntervalId = setInterval(() => {
      void this.refreshAllRepoStatuses();
    }, interval);

    void this.refreshAllRepoStatuses();
  }

  /** Whether the background refresh is running. */
  get refreshing(): boolean {
    return this.refreshIntervalId !== null;
  }

  /** Start the background refresh unless it runs (after a link or an import). */
  ensureBackgroundRefresh(): void {
    if (!this.refreshing) this.startBackgroundRefresh();
  }

  /**
   * Stop background refresh.
   */
  stopBackgroundRefresh(): void {
    if (this.refreshIntervalId !== null) {
      clearInterval(this.refreshIntervalId);
      this.refreshIntervalId = null;
    }
  }

  /**
   * Refresh every repo's status; a repo whose status changed since the last
   * refresh (someone edited its files, committed or fetched) is synced.
   * The first refresh only records the status: startup
   * syncs the active project itself.
   */
  async refreshAllRepoStatuses(): Promise<void> {
    const repos = this.state.sharedRepos;
    if (repos.length === 0) return;

    await Promise.allSettled(
      repos.map(async (repo) => {
        if (this.state.syncStateByRepo[repo.id]?.isSyncing) return;
        await this.refreshRepoStatus(repo.id);
        const s = this.state.syncStateByRepo[repo.id];
        if (!s || s.statusUnreadable) return;
        const key = `${s.pendingChanges}:${s.aheadBy}:${s.behindBy}:${s.conflictFiles.length}`;
        const before = this.lastStatus.get(repo.id);
        this.lastStatus.set(repo.id, key);
        if (before !== undefined && before !== key && s.conflictFiles.length === 0) {
          await this.syncRepo(repo.id);
        }
      }),
    );
  }
}
