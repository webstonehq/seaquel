/**
 * Shared query library types for Git-synced team collaboration.
 * @module types/shared-queries
 */

import type { QueryParameter } from './query';

/**
 * Sync status for a shared query repository.
 */
export type RepoSyncStatus = 'synced' | 'ahead' | 'behind' | 'diverged' | 'error' | 'uninitialized';

/**
 * A Git repository containing shared queries.
 */
export interface SharedQueryRepo {
	/** Unique identifier for the repo */
	id: string;
	/** Display name for the repo */
	name: string;
	/** Local file system path where the repo is cloned */
	path: string;
	/** Git remote URL (HTTPS or SSH) */
	remoteUrl: string;
	/** Branch to sync with (default: main) */
	branch: string;
	/** When the repo was last synced */
	lastSyncAt: Date | null;
	/** Current sync status */
	syncStatus: RepoSyncStatus;
	/** Credentials identifier (for keyring lookup) */
	credentialsId?: string;
}

/**
 * A shared query loaded from a .sql file with YAML frontmatter.
 */
export interface SharedQuery {
	/** Unique identifier (derived from repo ID + file path) */
	id: string;
	/** ID of the containing repo */
	repoId: string;
	/** Relative path within the repo (e.g., "analytics/active-users.sql") */
	filePath: string;
	/** Display name from frontmatter */
	name: string;
	/** Optional description from frontmatter */
	description?: string;
	/** The SQL query text */
	query: string;
	/** Optional parameter definitions */
	parameters?: QueryParameter[];
	/** Target database type (postgresql, mysql, etc.) */
	databaseType?: string;
	/** Tags for categorization */
	tags: string[];
	/** Folder path for tree display (derived from filePath) */
	folder: string;
}

/**
 * State of the sync operation for a repository.
 */
export interface SyncState {
	/** Whether a sync operation is in progress */
	isSyncing: boolean;
	/** Last sync error message, if any */
	lastError?: string;
	/** Number of uncommitted local changes */
	pendingChanges: number;
	/** Commits ahead of remote */
	aheadBy: number;
	/** Commits behind remote */
	behindBy: number;
	/** List of files in conflict */
	conflictFiles: string[];
}

/**
 * Git credentials for repository authentication.
 */
export interface GitCredentials {
	/** Username for HTTPS auth */
	username?: string;
	/** Password/token for HTTPS auth */
	password?: string;
	/** Path to SSH private key */
	sshKeyPath?: string;
	/** Passphrase for SSH key */
	sshPassphrase?: string;
}

/**
 * Result of a Git sync operation.
 */
export interface SyncResult {
	/** Whether the operation succeeded */
	success: boolean;
	/** Human-readable result message */
	message: string;
	/** List of files with merge conflicts */
	conflicts: string[];
	/** List of files that were changed */
	filesChanged: string[];
}

/**
 * Repository status from Git.
 */
export interface RepoStatus {
	/** Whether the working directory is clean */
	isClean: boolean;
	/** Number of pending changes */
	pendingChanges: number;
	/** Commits ahead of upstream */
	aheadBy: number;
	/** Commits behind upstream */
	behindBy: number;
	/** Whether there are unresolved conflicts */
	hasConflicts: boolean;
	/** Current branch name */
	currentBranch: string;
	/** List of modified files */
	modifiedFiles: string[];
	/** List of untracked files */
	untrackedFiles: string[];
}

/**
 * Content of a conflicting file for merge resolution.
 */
export interface ConflictContent {
	/** Base (common ancestor) content */
	base: string;
	/** Our (local) content */
	ours: string;
	/** Their (remote) content */
	theirs: string;
}

/**
 * YAML frontmatter structure for .sql files.
 */
export interface QueryFrontmatter {
	/** Query name */
	name: string;
	/** Optional description */
	description?: string;
	/** Target database type */
	database?: string;
	/** Tags for categorization */
	tags?: string[];
	/** Parameter definitions */
	parameters?: QueryParameter[];
}

/**
 * A folder in the shared query tree.
 */
export interface SharedQueryFolder {
	/** Folder name */
	name: string;
	/** Full path relative to repo root */
	path: string;
	/** Nested folders */
	children: SharedQueryFolder[];
	/** Queries directly in this folder */
	queries: SharedQuery[];
}

// === Persisted Types (for JSON serialization) ===

/**
 * Persisted version of SharedQueryRepo with ISO date strings.
 */
export interface PersistedSharedQueryRepo {
	id: string;
	name: string;
	path: string;
	remoteUrl: string;
	branch: string;
	lastSyncAt: string | null;
	syncStatus: RepoSyncStatus;
	credentialsId?: string;
}

/**
 * Converts a SharedQueryRepo to its persisted form.
 */
export function serializeRepo(repo: SharedQueryRepo): PersistedSharedQueryRepo {
	return {
		...repo,
		lastSyncAt: repo.lastSyncAt?.toISOString() ?? null
	};
}

/**
 * Converts a persisted repo to the runtime form.
 */
export function deserializeRepo(persisted: PersistedSharedQueryRepo): SharedQueryRepo {
	return {
		...persisted,
		lastSyncAt: persisted.lastSyncAt ? new Date(persisted.lastSyncAt) : null
	};
}
