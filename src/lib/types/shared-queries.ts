/**
 * Shared query library types for Git-synced team collaboration.
 * @module types/shared-queries
 */

import type { QueryParameter } from "./query";
import type { ConnectionLabel } from "./project";
import type { DatabaseType } from "./database";

/**
 * Sync status for a shared query repository.
 */
export type RepoSyncStatus = "synced" | "ahead" | "behind" | "diverged" | "error" | "uninitialized";

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
  /** File modification time (from filesystem metadata) */
  updatedAt?: Date;
}

/**
 * A shared dashboard loaded from a .json file in the repo.
 */
export interface SharedDashboard {
  /** Unique identifier (derived from repo ID + file path) */
  id: string;
  /** ID of the containing repo */
  repoId: string;
  /** Relative path within the repo */
  filePath: string;
  /** Display name */
  name: string;
  /** Optional description */
  description?: string;
  /** Dashboard widgets (serialized) */
  widgets: import("./dashboard").DashboardWidget[];
  /** Dashboard viewport */
  viewport: { x: number; y: number; zoom: number };
  /** Date filter */
  dateFilter?: { start: string; end: string } | null;
  /** File modification time */
  updatedAt?: Date;
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
  /** Files with unresolved merge conflicts (not in `modifiedFiles`) */
  conflictFiles: string[];
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

// === Shared Config Types (for .seaquel/ directory) ===

/**
 * Shared labels defined in a repo's .seaquel/labels.yaml.
 */
export interface SharedLabels {
  /** Labels available across all projects in this repo */
  labels: ConnectionLabel[];
}

/**
 * A shared project loaded from .seaquel/projects/<name>/project.yaml.
 */
export interface SharedProject {
  /** Unique identifier (repoId + directory path) */
  id: string;
  /** ID of the containing repo */
  repoId: string;
  /** Display name from project.yaml */
  name: string;
  /** Optional description from project.yaml */
  description?: string;
  /** Directory name within projects/ */
  dirName: string;
  /** Shared connections belonging to this project */
  connections: SharedConnection[];
}

/**
 * A shared connection template loaded from .seaquel/projects/<name>/connections/<name>.yaml.
 * Never contains credentials — those are stored locally per user.
 */
export interface SharedConnection {
  /** Unique identifier (repoId + file path) */
  id: string;
  /** ID of the containing repo */
  repoId: string;
  /** ID of the shared project this belongs to */
  projectId: string;
  /** Relative file path within the repo */
  filePath: string;
  /** Display name */
  name: string;
  /** Database engine type */
  type: DatabaseType;
  /** Database server hostname */
  host: string;
  /** Database server port */
  port: number;
  /** Name of the database */
  databaseName: string;
  /** SSL/TLS mode */
  sslMode?: string;
  /** SSH tunnel configuration (no credentials) */
  sshTunnel?: SharedSSHTunnelConfig;
  /** Label names referenced from labels.yaml */
  labels: string[];
}

/**
 * SSH tunnel config for shared connections (no passwords/passphrases).
 */
export interface SharedSSHTunnelConfig {
  enabled: boolean;
  host: string;
  port: number;
}

/**
 * Personal overrides for a shared connection template.
 * Stored locally in SQLite — never shared via Git.
 */
export interface ConnectionOverride {
  /** ID of the shared connection this overrides */
  sharedConnectionId: string;
  /** User's username for this connection */
  username?: string;
  /** Host override (e.g., for local tunnel) */
  hostOverride?: string;
  /** Port override */
  portOverride?: number;
  /** Whether the DB password is saved in keychain */
  savePassword: boolean;
  /** Whether the SSH password is saved in keychain */
  saveSshPassword: boolean;
  /** Whether the SSH key passphrase is saved in keychain */
  saveSshKeyPassphrase: boolean;
}

/**
 * Fields that must NEVER appear in shared connection YAML files.
 * The parser must actively strip these.
 */
export const CREDENTIAL_FIELDS = [
  "username",
  "password",
  "connectionString",
  "sshPassword",
  "sshKeyPassphrase",
  "sshUsername",
] as const;

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
    lastSyncAt: repo.lastSyncAt?.toISOString() ?? null,
  };
}

/**
 * Converts a persisted repo to the runtime form.
 */
export function deserializeRepo(persisted: PersistedSharedQueryRepo): SharedQueryRepo {
  return {
    ...persisted,
    lastSyncAt: persisted.lastSyncAt ? new Date(persisted.lastSyncAt) : null,
  };
}
