/**
 * `StorageClient`: the app's only way to read and write its metadata
 * (projects, connections, tabs, saved queries, history, settings, …).
 *
 * One property per repository, with the repositories' method names minus
 * the `db` argument. Rows are the app's own `Persisted*` types; the Rust
 * client maps the generated wire types onto them (`rust-client.ts`).
 *
 * - Desktop and web: `RustStorageClient`, which sends one typed call per
 *   method to `seaquel-storage` (`core_call` / `POST /api/rpc`).
 * - Demo: `SqljsStorageClient`, the TypeScript repositories over sql.js,
 *   until phase 8.
 */

import type {
  PersistedAIChat,
  PersistedAIMessage,
  PersistedDashboardVersion,
  PersistedProject,
  PersistedProjectState,
  PersistedQueryHistoryItem,
  PersistedQueryVersion,
  PersistedSavedQuery,
  PersistedSharedQueryRepo,
} from "$lib/types";
import type { PersistedConnection } from "$lib/hooks/database/types";

/** This machine's settings for a shared connection. */
export interface PersistedConnectionOverride {
  sharedConnectionId: string;
  username?: string;
  hostOverride?: string;
  portOverride?: number;
  savePassword: boolean;
  saveSshPassword: boolean;
  saveSshKeyPassphrase: boolean;
}

export interface PersistedDashboard {
  id: string;
  projectId: string;
  name: string;
  viewport: string; // JSON: { x, y, zoom }
  widgets: string; // JSON blob
  dateFilter?: string | null;
  starred?: boolean;
  shared?: boolean;
  description?: string;
  createdAt: string;
  updatedAt: string;
}

/**
 * Singleton row that tells the browser everything it needs to re-derive the
 * Vault Key from a passphrase. `salt` and `verifier*` are base64.
 */
export interface PersistedVaultState {
  salt: string;
  kdfParams: { version: number; t: number; m: number; p: number };
  verifier: string;
  verifierNonce: string;
  createdAt: string;
}

/**
 * One vault-encrypted credential. `scope` is the `KeyringService` category
 * (`db`, `ssh`, `ssh-key`, `license`, `ai-api-key-provider`, …) and `key`
 * the connection or provider id, empty for singletons. `nonce` and
 * `ciphertext` are base64.
 */
export interface PersistedCredential {
  scope: string;
  key: string;
  nonce: string;
  ciphertext: string;
  updatedAt: string;
}

export interface ImportStateRow {
  hasOfferedImport: boolean;
  lastCheckTimestamp: string | null;
}

export interface TutorialProgressRow {
  lessonId: string;
  challengeId: string;
  state: string | null;
}

export interface StorageClient {
  projects: {
    loadAll(): Promise<PersistedProject[]>;
    save(project: PersistedProject): Promise<void>;
    saveAll(projects: PersistedProject[]): Promise<void>;
    remove(projectId: string): Promise<void>;
  };
  appState: {
    get(key: string): Promise<string | null>;
    set(key: string, value: string | null): Promise<void>;
  };
  connections: {
    loadAll(): Promise<PersistedConnection[]>;
    save(connection: PersistedConnection): Promise<void>;
    remove(connectionId: string): Promise<void>;
  };
  connectionOverrides: {
    load(sharedConnectionId: string): Promise<PersistedConnectionOverride | null>;
    loadAll(): Promise<PersistedConnectionOverride[]>;
    save(override: PersistedConnectionOverride): Promise<void>;
    remove(sharedConnectionId: string): Promise<void>;
  };
  projectState: {
    load(projectId: string): Promise<PersistedProjectState | null>;
    save(state: PersistedProjectState): Promise<void>;
    remove(projectId: string): Promise<void>;
  };
  savedQueries: {
    loadByProject(projectId: string): Promise<PersistedSavedQuery[]>;
    saveAll(projectId: string, queries: PersistedSavedQuery[]): Promise<void>;
    removeByProject(projectId: string): Promise<void>;
  };
  queryVersions: {
    loadByQuery(queryId: string): Promise<PersistedQueryVersion[]>;
    loadByProject(projectId: string): Promise<PersistedQueryVersion[]>;
    insert(version: PersistedQueryVersion): Promise<void>;
    /**
     * Keep the newest `keepCount` versions and turn the oldest survivor into
     * a keyframe. `keepCount` 0 keeps everything.
     */
    pruneOldVersions(queryId: string, keepCount: number): Promise<void>;
  };
  queryHistory: {
    loadByConnection(connectionId: string): Promise<PersistedQueryHistoryItem[]>;
    replaceAll(connectionId: string, items: PersistedQueryHistoryItem[]): Promise<void>;
    removeByConnection(connectionId: string): Promise<void>;
  };
  sharedRepos: {
    loadAll(): Promise<{ repos: PersistedSharedQueryRepo[]; activeRepoId: string | null }>;
    saveAll(repos: PersistedSharedQueryRepo[], activeRepoId: string | null): Promise<void>;
  };
  themes: {
    loadPreferences(): Promise<{ lightThemeId: string; darkThemeId: string } | null>;
    savePreferences(lightThemeId: string, darkThemeId: string): Promise<void>;
    loadUserThemes(): Promise<unknown[]>;
    saveUserThemes(themes: unknown[]): Promise<void>;
  };
  license: {
    load(): Promise<unknown>;
    save(data: unknown): Promise<void>;
  };
  onboarding: {
    load(): Promise<unknown>;
    save(data: unknown): Promise<void>;
  };
  tutorial: {
    loadAll(): Promise<TutorialProgressRow[]>;
    save(lessonId: string, challengeId: string, state: string | null): Promise<void>;
    removeLesson(lessonId: string): Promise<void>;
    removeAll(): Promise<void>;
  };
  importState: {
    load(source: string): Promise<ImportStateRow | null>;
    save(
      source: string,
      hasOfferedImport: boolean,
      lastCheckTimestamp: string | null,
    ): Promise<void>;
  };
  dashboards: {
    loadByProject(projectId: string): Promise<PersistedDashboard[]>;
    save(dashboard: PersistedDashboard): Promise<void>;
    remove(id: string): Promise<void>;
    removeByProject(projectId: string): Promise<void>;
  };
  dashboardVersions: {
    loadByDashboard(dashboardId: string): Promise<PersistedDashboardVersion[]>;
    loadByProject(projectId: string): Promise<PersistedDashboardVersion[]>;
    insert(version: PersistedDashboardVersion): Promise<void>;
    /** Keep the newest `keepCount` versions. `keepCount` 0 deletes them all. */
    pruneOldVersions(dashboardId: string, keepCount: number): Promise<void>;
  };
  aiChats: {
    loadByConnection(connectionId: string): Promise<PersistedAIChat[]>;
    saveChat(chat: PersistedAIChat): Promise<void>;
    removeChat(chatId: string): Promise<void>;
    removeByConnection(connectionId: string): Promise<void>;
    loadMessages(chatId: string): Promise<PersistedAIMessage[]>;
    replaceAllMessages(chatId: string, messages: PersistedAIMessage[]): Promise<void>;
  };
  vaultState: {
    load(): Promise<PersistedVaultState | null>;
    save(state: PersistedVaultState): Promise<void>;
    /** Deletes the vault and every credential, which only make sense together. */
    reset(): Promise<void>;
  };
  userCredentials: {
    load(scope: string, key: string): Promise<PersistedCredential | null>;
    save(credential: PersistedCredential): Promise<void>;
    remove(scope: string, key: string): Promise<void>;
    /** Removes every credential tied to `key` (e.g. a connection id). */
    removeAllForKey(key: string): Promise<void>;
  };
}
