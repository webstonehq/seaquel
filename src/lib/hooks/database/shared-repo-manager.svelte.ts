import type {
	SharedQueryRepo,
	SharedQuery,
	SyncState,
	GitCredentials,
	RepoSyncStatus
} from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import * as gitService from '$lib/services/git';
import { parseQueryFile } from '$lib/services/query-file-parser';
import { readDir, readTextFile, exists } from '@tauri-apps/plugin-fs';
import { join } from '@tauri-apps/api/path';

/**
 * Manages shared query repositories: clone, sync, status updates.
 */
export class SharedRepoManager {
	/** Interval ID for background refresh */
	private refreshIntervalId: ReturnType<typeof setInterval> | null = null;

	/** Default refresh interval in milliseconds (5 minutes) */
	private static readonly DEFAULT_REFRESH_INTERVAL = 5 * 60 * 1000;

	constructor(
		private state: DatabaseState,
		private schedulePersistence: () => void
	) {}

	/**
	 * Add a new repository by cloning from a remote URL.
	 */
	async cloneRepo(
		name: string,
		remoteUrl: string,
		localPath: string,
		credentials?: GitCredentials
	): Promise<string> {
		// Clone the repository
		await gitService.cloneRepo(remoteUrl, localPath, credentials);

		// Create repo entry
		const repo: SharedQueryRepo = {
			id: `repo-${Date.now()}`,
			name,
			path: localPath,
			remoteUrl,
			branch: 'main',
			lastSyncAt: new Date(),
			syncStatus: 'synced'
		};

		// Add to state
		this.state.sharedRepos = [...this.state.sharedRepos, repo];

		// Initialize sync state
		this.state.syncStateByRepo = {
			...this.state.syncStateByRepo,
			[repo.id]: {
				isSyncing: false,
				pendingChanges: 0,
				aheadBy: 0,
				behindBy: 0,
				conflictFiles: []
			}
		};

		// Load queries from the cloned repo
		await this.loadQueriesFromRepo(repo.id);

		// Set as active if first repo
		if (this.state.sharedRepos.length === 1) {
			this.state.activeRepoId = repo.id;
		}

		this.schedulePersistence();
		return repo.id;
	}

	/**
	 * Initialize a new local repository.
	 */
	async initRepo(name: string, localPath: string): Promise<string> {
		// Initialize the repository
		await gitService.initRepo(localPath);

		// Create repo entry
		const repo: SharedQueryRepo = {
			id: `repo-${Date.now()}`,
			name,
			path: localPath,
			remoteUrl: '',
			branch: 'main',
			lastSyncAt: null,
			syncStatus: 'uninitialized'
		};

		// Add to state
		this.state.sharedRepos = [...this.state.sharedRepos, repo];

		// Initialize sync state
		this.state.syncStateByRepo = {
			...this.state.syncStateByRepo,
			[repo.id]: {
				isSyncing: false,
				pendingChanges: 0,
				aheadBy: 0,
				behindBy: 0,
				conflictFiles: []
			}
		};

		// Initialize empty queries list
		this.state.sharedQueriesByRepo = {
			...this.state.sharedQueriesByRepo,
			[repo.id]: []
		};

		// Set as active if first repo
		if (this.state.sharedRepos.length === 1) {
			this.state.activeRepoId = repo.id;
		}

		this.schedulePersistence();
		return repo.id;
	}

	/**
	 * Remove a repository from the list (does not delete local files).
	 */
	removeRepo(repoId: string): void {
		this.state.sharedRepos = this.state.sharedRepos.filter((r) => r.id !== repoId);

		// Clean up associated state
		const { [repoId]: _queries, ...remainingQueries } = this.state.sharedQueriesByRepo;
		this.state.sharedQueriesByRepo = remainingQueries;

		const { [repoId]: _syncState, ...remainingSyncState } = this.state.syncStateByRepo;
		this.state.syncStateByRepo = remainingSyncState;

		// Update active repo if needed
		if (this.state.activeRepoId === repoId) {
			this.state.activeRepoId = this.state.sharedRepos[0]?.id ?? null;
		}

		this.schedulePersistence();
	}

	/**
	 * Set the active repository.
	 */
	setActiveRepo(repoId: string | null): void {
		this.state.activeRepoId = repoId;
	}

	/**
	 * Pull changes from remote.
	 */
	async pullRepo(repoId: string, credentials?: GitCredentials): Promise<void> {
		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return;

		// Update sync state
		this.updateSyncState(repoId, { isSyncing: true, lastError: undefined });

		try {
			const result = await gitService.pullRepo(repo.path, credentials);

			if (result.success) {
				// Reload queries after pull
				await this.loadQueriesFromRepo(repoId);

				// Update repo status
				this.updateRepo(repoId, {
					lastSyncAt: new Date(),
					syncStatus: 'synced'
				});

				await this.refreshRepoStatus(repoId);
			} else if (result.conflicts.length > 0) {
				this.updateSyncState(repoId, { conflictFiles: result.conflicts });
				this.updateRepo(repoId, { syncStatus: 'diverged' });
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			this.updateSyncState(repoId, { lastError: message });
			this.updateRepo(repoId, { syncStatus: 'error' });
		} finally {
			this.updateSyncState(repoId, { isSyncing: false });
		}
	}

	/**
	 * Push local changes to remote.
	 */
	async pushRepo(repoId: string, credentials?: GitCredentials): Promise<void> {
		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return;

		this.updateSyncState(repoId, { isSyncing: true, lastError: undefined });

		try {
			const result = await gitService.pushRepo(repo.path, credentials);

			if (result.success) {
				this.updateRepo(repoId, {
					lastSyncAt: new Date(),
					syncStatus: 'synced'
				});
				await this.refreshRepoStatus(repoId);
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			this.updateSyncState(repoId, { lastError: message });
			// May need to pull first
			if (message.includes('rejected') || message.includes('non-fast-forward')) {
				this.updateRepo(repoId, { syncStatus: 'behind' });
			} else {
				this.updateRepo(repoId, { syncStatus: 'error' });
			}
		} finally {
			this.updateSyncState(repoId, { isSyncing: false });
		}
	}

	/**
	 * Commit all pending changes.
	 */
	async commitChanges(repoId: string, message: string): Promise<string | null> {
		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return null;

		try {
			const commitId = await gitService.commitChanges(repo.path, message);
			await this.refreshRepoStatus(repoId);
			return commitId;
		} catch (error) {
			const msg = error instanceof Error ? error.message : String(error);
			this.updateSyncState(repoId, { lastError: msg });
			return null;
		}
	}

	/**
	 * Refresh the Git status for a repository.
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
				conflictFiles: status.hasConflicts ? status.modifiedFiles : []
			});

			// Update sync status based on ahead/behind
			let syncStatus: RepoSyncStatus = 'synced';
			if (status.hasConflicts) {
				syncStatus = 'diverged';
			} else if (status.aheadBy > 0 && status.behindBy > 0) {
				syncStatus = 'diverged';
			} else if (status.aheadBy > 0) {
				syncStatus = 'ahead';
			} else if (status.behindBy > 0) {
				syncStatus = 'behind';
			}

			this.updateRepo(repoId, { syncStatus });
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			this.updateSyncState(repoId, { lastError: message });
		}
	}

	/**
	 * Set the remote URL for a repository.
	 */
	async setRemoteUrl(repoId: string, url: string): Promise<void> {
		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return;

		await gitService.setRemote(repo.path, url);
		this.updateRepo(repoId, { remoteUrl: url });
		this.schedulePersistence();
	}

	/**
	 * Load all queries from a repository's files.
	 */
	async loadQueriesFromRepo(repoId: string): Promise<void> {
		const repo = this.state.sharedRepos.find((r) => r.id === repoId);
		if (!repo) return;

		const queries: SharedQuery[] = [];

		try {
			await this.scanDirectory(repo.path, '', repoId, queries);

			this.state.sharedQueriesByRepo = {
				...this.state.sharedQueriesByRepo,
				[repoId]: queries
			};
		} catch (error) {
			console.error('Failed to load queries from repo:', error);
		}
	}

	/**
	 * Recursively scan a directory for .sql files.
	 */
	private async scanDirectory(
		basePath: string,
		relativePath: string,
		repoId: string,
		queries: SharedQuery[]
	): Promise<void> {
		const fullPath = relativePath ? await join(basePath, relativePath) : basePath;

		try {
			const entries = await readDir(fullPath);

			for (const entry of entries) {
				const entryRelativePath = relativePath
					? `${relativePath}/${entry.name}`
					: entry.name;

				// Skip hidden files and .seaquel config
				if (entry.name.startsWith('.') && entry.name !== '.seaquel') {
					continue;
				}

				if (entry.isDirectory) {
					await this.scanDirectory(basePath, entryRelativePath, repoId, queries);
				} else if (entry.name.toLowerCase().endsWith('.sql')) {
					const filePath = await join(basePath, entryRelativePath);
					const content = await readTextFile(filePath);
					const query = parseQueryFile(content, repoId, entryRelativePath);

					if (query) {
						queries.push(query);
					}
				}
			}
		} catch (error) {
			// Directory might not exist yet
			console.warn(`Failed to scan directory ${fullPath}:`, error);
		}
	}

	/**
	 * Update a repository's properties.
	 */
	private updateRepo(repoId: string, updates: Partial<SharedQueryRepo>): void {
		this.state.sharedRepos = this.state.sharedRepos.map((r) =>
			r.id === repoId ? { ...r, ...updates } : r
		);
		this.schedulePersistence();
	}

	/**
	 * Update sync state for a repository.
	 */
	private updateSyncState(repoId: string, updates: Partial<SyncState>): void {
		const current = this.state.syncStateByRepo[repoId] ?? {
			isSyncing: false,
			pendingChanges: 0,
			aheadBy: 0,
			behindBy: 0,
			conflictFiles: []
		};

		this.state.syncStateByRepo = {
			...this.state.syncStateByRepo,
			[repoId]: { ...current, ...updates }
		};
	}

	/**
	 * Check if a repo exists at the given path.
	 */
	async repoExistsAtPath(path: string): Promise<boolean> {
		try {
			const gitDir = await join(path, '.git');
			return await exists(gitDir);
		} catch {
			return false;
		}
	}

	/**
	 * Start background refresh of repo statuses.
	 * @param intervalMs - Refresh interval in milliseconds (default: 5 minutes)
	 */
	startBackgroundRefresh(intervalMs?: number): void {
		// Clear any existing interval
		this.stopBackgroundRefresh();

		const interval = intervalMs ?? SharedRepoManager.DEFAULT_REFRESH_INTERVAL;

		this.refreshIntervalId = setInterval(() => {
			this.refreshAllRepoStatuses();
		}, interval);

		// Do an initial refresh
		this.refreshAllRepoStatuses();
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
	 * Refresh the status of all repositories.
	 * Runs in parallel but handles errors gracefully.
	 */
	async refreshAllRepoStatuses(): Promise<void> {
		const repos = this.state.sharedRepos;
		if (repos.length === 0) return;

		// Refresh all repos in parallel, but don't wait for slow ones
		await Promise.allSettled(
			repos.map(async (repo) => {
				// Skip if already syncing
				const syncState = this.state.syncStateByRepo[repo.id];
				if (syncState?.isSyncing) return;

				try {
					await this.refreshRepoStatus(repo.id);
				} catch (error) {
					// Log but don't fail
					console.warn(`Failed to refresh status for ${repo.name}:`, error);
				}
			})
		);
	}
}
