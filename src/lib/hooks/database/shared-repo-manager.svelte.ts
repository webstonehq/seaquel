import type {
  SharedQueryRepo,
  SharedQuery,
  SharedDashboard,
  SyncState,
  GitCredentials,
  RepoSyncStatus,
  SharedProject,
  SharedConnection,
  DatabaseConnection,
} from "$lib/types";
import type { ConnectionLabel } from "$lib/types/project";
import type { DatabaseState } from "./state.svelte.js";
import * as gitService from "$lib/services/git";
import { parseQueryFile } from "$lib/services/query-file-parser";
import { parseDashboardFile } from "$lib/services/dashboard-file-parser";
import {
  parseLabelsFile,
  parseProjectFile,
  parseConnectionFile,
  serializeLabelsFile,
  serializeProjectFile,
  serializeConnectionFile,
  nameToFilename,
} from "$lib/services/config-file-parser";
import {
  readDir,
  readTextFile,
  exists,
  writeTextFile,
  mkdir,
  remove,
  rename,
  stat,
} from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { log } from "$lib/utils/logger";
import { extractErrorMessage } from "$lib/errors";

export const SEAQUEL_DIR = ".seaquel";

const DEFAULT_SYNC_STATE: SyncState = {
  isSyncing: false,
  pendingChanges: 0,
  aheadBy: 0,
  behindBy: 0,
  conflictFiles: [],
};

/**
 * Parse a composite ID ("repoId:filePath") into its parts.
 */
export function parseCompositeId(id: string): { repoId: string; filePath: string } {
  const [repoId, ...pathParts] = id.split(":");
  return { repoId, filePath: pathParts.join(":") };
}

/**
 * Manages shared query repositories: clone, sync, status updates.
 */
export class SharedRepoManager {
  /** Interval ID for background refresh */
  private refreshIntervalId: ReturnType<typeof setInterval> | null = null;

  /** Per-repo lock to serialize concurrent operations */
  private repoLocks = new Map<string, Promise<void>>();

  /** Default refresh interval in milliseconds (5 minutes) */
  private static readonly DEFAULT_REFRESH_INTERVAL = 5 * 60 * 1000;

  constructor(
    private state: DatabaseState,
    private schedulePersistence: () => void,
  ) {}

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
   * Build a SharedConnection from a local DatabaseConnection (credentials stripped).
   */
  private buildSharedConnectionFromLocal(
    repoId: string,
    projectId: string,
    filePath: string,
    conn: DatabaseConnection,
  ): SharedConnection {
    return {
      id: `${repoId}:${filePath}`,
      repoId,
      projectId,
      filePath,
      name: conn.name,
      type: conn.type,
      host: conn.host,
      port: conn.port,
      databaseName: conn.databaseName,
      sslMode: conn.sslMode,
      sshTunnel: conn.sshTunnel?.enabled
        ? {
            enabled: true,
            host: conn.sshTunnel.host,
            port: conn.sshTunnel.port,
          }
        : undefined,
      labels: [],
    };
  }

  /**
   * Add a new repository by cloning from a remote URL.
   */
  async cloneRepo(
    name: string,
    remoteUrl: string,
    localPath: string,
    credentials?: GitCredentials,
  ): Promise<string> {
    // Clone the repository
    await gitService.cloneRepo(remoteUrl, localPath, credentials);

    // Create repo entry
    const repo: SharedQueryRepo = {
      id: `repo-${crypto.randomUUID()}`,
      name,
      path: localPath,
      remoteUrl,
      branch: "main",
      lastSyncAt: new Date(),
      syncStatus: "synced",
    };

    // Add to state
    this.state.sharedRepos = [...this.state.sharedRepos, repo];

    // Initialize sync state
    this.state.syncStateByRepo = {
      ...this.state.syncStateByRepo,
      [repo.id]: { ...DEFAULT_SYNC_STATE },
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
      id: `repo-${crypto.randomUUID()}`,
      name,
      path: localPath,
      remoteUrl: "",
      branch: "main",
      lastSyncAt: null,
      syncStatus: "uninitialized",
    };

    // Add to state
    this.state.sharedRepos = [...this.state.sharedRepos, repo];

    // Initialize sync state
    this.state.syncStateByRepo = {
      ...this.state.syncStateByRepo,
      [repo.id]: { ...DEFAULT_SYNC_STATE },
    };

    // Initialize empty queries list
    this.state.sharedQueriesByRepo = {
      ...this.state.sharedQueriesByRepo,
      [repo.id]: [],
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

    const { [repoId]: _dashboards2, ...remainingDashboards } = this.state.sharedDashboardsByRepo;
    this.state.sharedDashboardsByRepo = remainingDashboards;

    const { [repoId]: _syncState, ...remainingSyncState } = this.state.syncStateByRepo;
    this.state.syncStateByRepo = remainingSyncState;

    // Clean up shared config state
    const { [repoId]: _labels, ...remainingLabels } = this.state.sharedLabelsByRepo;
    this.state.sharedLabelsByRepo = remainingLabels;

    // Remove shared projects and their connections
    const projects = this.state.sharedProjectsByRepo[repoId] ?? [];
    const { [repoId]: _projects, ...remainingProjects } = this.state.sharedProjectsByRepo;
    this.state.sharedProjectsByRepo = remainingProjects;

    const remainingConnections = { ...this.state.sharedConnectionsByProject };
    for (const project of projects) {
      delete remainingConnections[project.id];
    }
    this.state.sharedConnectionsByProject = remainingConnections;

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
    const repo = this.getRepoOrThrow(repoId);

    return this.withRepoLock(repoId, async () => {
      this.updateSyncState(repoId, { isSyncing: true, lastError: undefined });

      try {
        const result = await gitService.pullRepo(repo.path, credentials);

        if (result.success) {
          await this.loadQueriesFromRepo(repoId);
          this.updateRepo(repoId, {
            lastSyncAt: new Date(),
            syncStatus: "synced",
          });
          await this.refreshRepoStatus(repoId);
        } else if (result.conflicts.length > 0) {
          this.updateSyncState(repoId, { conflictFiles: result.conflicts });
          this.updateRepo(repoId, { syncStatus: "diverged" });
        }
      } catch (error) {
        const message = extractErrorMessage(error);
        this.updateSyncState(repoId, { lastError: message });
        this.updateRepo(repoId, { syncStatus: "error" });
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
          this.updateRepo(repoId, {
            lastSyncAt: new Date(),
            syncStatus: "synced",
          });
          await this.refreshRepoStatus(repoId);
        }
      } catch (error) {
        const message = extractErrorMessage(error);
        this.updateSyncState(repoId, { lastError: message });
        // seaquel-git's PUSH_REJECTED_NON_FAST_FORWARD: pull first. Any other
        // refusal (a protected branch, a hook) is an error with its reason.
        if (message.includes("rejected (non-fast-forward)")) {
          this.updateRepo(repoId, { syncStatus: "behind" });
        } else {
          this.updateRepo(repoId, { syncStatus: "error" });
        }
        throw error;
      } finally {
        this.updateSyncState(repoId, { isSyncing: false });
      }
    });
  }

  /**
   * Commit all pending changes.
   */
  async commitChanges(repoId: string, message: string): Promise<string | null> {
    const repo = this.getRepoOrThrow(repoId);

    return this.withRepoLock(repoId, async () => {
      try {
        const commitId = await gitService.commitChanges(repo.path, message);
        await this.refreshRepoStatus(repoId);
        return commitId;
      } catch (error) {
        const msg = extractErrorMessage(error);
        this.updateSyncState(repoId, { lastError: msg });
        return null;
      }
    });
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
        conflictFiles: status.hasConflicts ? status.conflictFiles : [],
      });

      // Update sync status based on ahead/behind
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

      this.updateRepo(repoId, { syncStatus });
    } catch (error) {
      const message = extractErrorMessage(error);
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
   * Update repository settings (name, branch).
   */
  updateRepoSettings(repoId: string, updates: Pick<SharedQueryRepo, "name" | "branch">): void {
    this.state.sharedRepos = this.state.sharedRepos.map((r) =>
      r.id === repoId ? { ...r, ...updates } : r,
    );
    this.schedulePersistence();
  }

  /**
   * Load all queries from a repository's .seaquel/projects/<name>/queries/ directories.
   */
  async loadQueriesFromRepo(repoId: string): Promise<void> {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const queries: SharedQuery[] = [];
    const dashboards: SharedDashboard[] = [];

    try {
      const projectsDir = await join(repo.path, SEAQUEL_DIR, "projects");
      if (await exists(projectsDir)) {
        const entries = await readDir(projectsDir);

        for (const entry of entries) {
          if (!entry.isDirectory || entry.name.startsWith(".")) continue;

          const queriesDir = await join(projectsDir, entry.name, "queries");
          if (await exists(queriesDir)) {
            const queriesRelBase = `${SEAQUEL_DIR}/projects/${entry.name}/queries`;
            await this.scanDirectory(repo.path, queriesRelBase, repoId, queries, queriesRelBase);
          }

          const dashboardsDir = await join(projectsDir, entry.name, "dashboards");
          if (await exists(dashboardsDir)) {
            const dashboardsRelBase = `${SEAQUEL_DIR}/projects/${entry.name}/dashboards`;
            await this.scanDashboardDirectory(repo.path, dashboardsRelBase, repoId, dashboards);
          }
        }
      }

      this.state.sharedQueriesByRepo = {
        ...this.state.sharedQueriesByRepo,
        [repoId]: queries,
      };

      this.state.sharedDashboardsByRepo = {
        ...this.state.sharedDashboardsByRepo,
        [repoId]: dashboards,
      };

      // Also load shared configs from .seaquel/ directory
      await this.loadSharedConfigs(repoId);
    } catch (error) {
      void log.error("Failed to load queries from repo:", error);
    }
  }

  /**
   * Load shared configs from .seaquel/ directory in a repository.
   * Parses labels.yaml, project.yaml files, and connection templates.
   */
  async loadSharedConfigs(repoId: string): Promise<void> {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    try {
      const seaquelDir = await join(repo.path, SEAQUEL_DIR);
      if (!(await exists(seaquelDir))) return;

      // Parse labels.yaml
      await this.loadSharedLabels(repoId, seaquelDir);

      // Parse projects
      await this.loadSharedProjects(repoId, seaquelDir);
    } catch (error) {
      void log.warn("Failed to load shared configs:", error);
    }
  }

  /**
   * Scan a folder for shared projects without modifying state.
   * Used by the import flow to preview available projects before committing.
   */
  async scanForSharedProjects(folderPath: string): Promise<SharedProject[]> {
    const projects: SharedProject[] = [];

    try {
      const seaquelDir = await join(folderPath, SEAQUEL_DIR);
      if (!(await exists(seaquelDir))) return projects;

      const projectsDir = await join(seaquelDir, "projects");
      if (!(await exists(projectsDir))) return projects;

      const entries = await readDir(projectsDir);
      const tempRepoId = "scan-preview";

      for (const entry of entries) {
        if (!entry.isDirectory || entry.name.startsWith(".")) continue;

        const projectDir = await join(projectsDir, entry.name);

        try {
          let content = await this.readYamlFile(projectDir, "project");
          if (!content) content = `name: ${entry.name}`;

          const project = parseProjectFile(content, tempRepoId, entry.name);
          const connections = await this.loadProjectConnections(
            tempRepoId,
            project.id,
            projectDir,
            entry.name,
          );
          project.connections = connections;
          projects.push(project);
        } catch (error) {
          void log.warn(`Failed to scan project ${entry.name}:`, error);
        }
      }
    } catch (error) {
      void log.warn("Failed to scan for shared projects:", error);
    }

    return projects;
  }

  /**
   * Try to read a YAML file, falling back from .yaml to .yml extension.
   */
  private async readYamlFile(dir: string, basename: string): Promise<string | null> {
    const yamlPath = await join(dir, `${basename}.yaml`);
    if (await exists(yamlPath)) {
      return readTextFile(yamlPath);
    }
    const ymlPath = await join(dir, `${basename}.yml`);
    if (await exists(ymlPath)) {
      return readTextFile(ymlPath);
    }
    return null;
  }

  /**
   * Load shared labels from .seaquel/labels.yaml.
   */
  private async loadSharedLabels(repoId: string, seaquelDir: string): Promise<void> {
    try {
      const content = await this.readYamlFile(seaquelDir, "labels");
      if (!content) return;

      const parsed = parseLabelsFile(content);
      this.state.sharedLabelsByRepo = {
        ...this.state.sharedLabelsByRepo,
        [repoId]: parsed.labels,
      };
    } catch (error) {
      void log.warn("Failed to load shared labels:", error);
    }
  }

  /**
   * Load shared projects from .seaquel/projects/<name>/project.yaml.
   */
  private async loadSharedProjects(repoId: string, seaquelDir: string): Promise<void> {
    try {
      const projectsDir = await join(seaquelDir, "projects");
      if (!(await exists(projectsDir))) return;

      const entries = await readDir(projectsDir);
      const projects: SharedProject[] = [];

      for (const entry of entries) {
        if (!entry.isDirectory || entry.name.startsWith(".")) continue;

        const projectDir = await join(projectsDir, entry.name);
        const project = await this.loadSingleProject(repoId, projectDir, entry.name);
        if (project) {
          projects.push(project);
        }
      }

      this.state.sharedProjectsByRepo = {
        ...this.state.sharedProjectsByRepo,
        [repoId]: projects,
      };
    } catch (error) {
      void log.warn("Failed to load shared projects:", error);
    }
  }

  /**
   * Load a single project and its connections.
   */
  private async loadSingleProject(
    repoId: string,
    projectDir: string,
    dirName: string,
  ): Promise<SharedProject | null> {
    try {
      let content = await this.readYamlFile(projectDir, "project");

      if (!content) {
        // No project.yaml — create a project from directory name
        content = `name: ${dirName}`;
      }

      const project = parseProjectFile(content, repoId, dirName);

      // Load connections for this project
      const connections = await this.loadProjectConnections(
        repoId,
        project.id,
        projectDir,
        dirName,
      );
      project.connections = connections;

      // Store connections in state
      this.state.sharedConnectionsByProject = {
        ...this.state.sharedConnectionsByProject,
        [project.id]: connections,
      };

      return project;
    } catch (error) {
      void log.warn(`Failed to load project ${dirName}:`, error);
      return null;
    }
  }

  /**
   * Load connection templates from a project's connections/ directory.
   */
  private async loadProjectConnections(
    repoId: string,
    projectId: string,
    projectDir: string,
    dirName: string,
  ): Promise<SharedConnection[]> {
    const connections: SharedConnection[] = [];

    try {
      const connectionsDir = await join(projectDir, "connections");
      if (!(await exists(connectionsDir))) return connections;

      const entries = await readDir(connectionsDir);

      for (const entry of entries) {
        if (entry.isDirectory) continue;
        if (!entry.name.endsWith(".yaml") && !entry.name.endsWith(".yml")) continue;
        if (entry.name.startsWith(".")) continue;

        const filePath = await join(connectionsDir, entry.name);
        const content = await readTextFile(filePath);

        // Build relative path within repo using dirName directly
        const relPath = `${SEAQUEL_DIR}/projects/${dirName}/connections/${entry.name}`;

        const connection = parseConnectionFile(content, repoId, projectId, relPath);
        if (connection) {
          connections.push(connection);
        }
      }
    } catch (error) {
      void log.warn("Failed to load project connections:", error);
    }

    return connections;
  }

  /**
   * Recursively scan a directory for .sql files.
   * @param folderPrefix - Prefix to strip from folder paths in parsed queries
   */
  private async scanDirectory(
    basePath: string,
    relativePath: string,
    repoId: string,
    queries: SharedQuery[],
    folderPrefix?: string,
  ): Promise<void> {
    const fullPath = relativePath ? await join(basePath, relativePath) : basePath;

    try {
      const entries = await readDir(fullPath);

      for (const entry of entries) {
        const entryRelativePath = relativePath ? `${relativePath}/${entry.name}` : entry.name;

        // Skip hidden files/directories
        if (entry.name.startsWith(".")) {
          continue;
        }

        if (entry.isDirectory) {
          await this.scanDirectory(basePath, entryRelativePath, repoId, queries, folderPrefix);
        } else if (entry.name.toLowerCase().endsWith(".sql")) {
          const filePath = await join(basePath, entryRelativePath);
          const content = await readTextFile(filePath);
          const query = parseQueryFile(content, repoId, entryRelativePath, folderPrefix);

          if (query) {
            try {
              const meta = await stat(filePath);
              if (meta.mtime) {
                query.updatedAt = new Date(meta.mtime);
              }
            } catch {
              // Ignore stat errors
            }
            queries.push(query);
          }
        }
      }
    } catch (error) {
      // Directory might not exist yet
      void log.warn(`Failed to scan directory ${fullPath}:`, error);
    }
  }

  /**
   * Recursively scan a directory for .json dashboard files.
   */
  private async scanDashboardDirectory(
    basePath: string,
    relativePath: string,
    repoId: string,
    dashboards: SharedDashboard[],
  ): Promise<void> {
    const fullPath = relativePath ? await join(basePath, relativePath) : basePath;

    try {
      const entries = await readDir(fullPath);

      for (const entry of entries) {
        if (entry.name.startsWith(".")) continue;

        const entryRelativePath = relativePath ? `${relativePath}/${entry.name}` : entry.name;

        if (entry.isDirectory) {
          await this.scanDashboardDirectory(basePath, entryRelativePath, repoId, dashboards);
        } else if (entry.name.toLowerCase().endsWith(".json")) {
          const filePath = await join(basePath, entryRelativePath);
          const content = await readTextFile(filePath);
          const dashboard = parseDashboardFile(content, repoId, entryRelativePath);

          if (dashboard) {
            try {
              const meta = await stat(filePath);
              if (meta.mtime) {
                dashboard.updatedAt = new Date(meta.mtime);
              }
            } catch {
              // Ignore stat errors
            }
            dashboards.push(dashboard);
          }
        }
      }
    } catch (error) {
      void log.warn(`Failed to scan dashboard directory ${fullPath}:`, error);
    }
  }

  /**
   * Export a local project as a shared project to a Git repo.
   * Creates .seaquel/projects/<name>/ with project.yaml and connection YAML files.
   * Credentials are NEVER exported.
   *
   * @param repoId - Target repo ID
   * @param projectName - Name for the shared project
   * @param connections - Local connections to export (credentials stripped)
   * @param labels - Optional shared labels to write to labels.yaml
   * @param description - Optional project description
   */
  async exportProject(
    repoId: string,
    projectName: string,
    connections: DatabaseConnection[],
    options?: {
      description?: string;
      labels?: ConnectionLabel[];
    },
  ): Promise<SharedProject | null> {
    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return null;

    const dirName = nameToFilename(projectName);
    const seaquelDir = await join(repo.path, SEAQUEL_DIR);
    const projectDir = await join(seaquelDir, "projects", dirName);
    const connectionsDir = await join(projectDir, "connections");
    const queriesDir = await join(projectDir, "queries");
    const dashboardsDir = await join(projectDir, "dashboards");

    try {
      // Create directories
      await mkdir(seaquelDir, { recursive: true });
      await mkdir(projectDir, { recursive: true });
      await mkdir(connectionsDir, { recursive: true });
      await mkdir(queriesDir, { recursive: true });
      await mkdir(dashboardsDir, { recursive: true });

      // Write project.yaml
      const sharedProject: SharedProject = {
        id: `${repoId}:${SEAQUEL_DIR}/projects/${dirName}`,
        repoId,
        name: projectName,
        description: options?.description,
        dirName,
        connections: [],
      };

      const projectYaml = serializeProjectFile(sharedProject);
      const projectYamlPath = await join(projectDir, "project.yaml");
      await writeTextFile(projectYamlPath, projectYaml);

      // Write connection YAML files (credentials stripped)
      const sharedConnections: SharedConnection[] = [];
      for (const conn of connections) {
        const connFilename = `${nameToFilename(conn.name)}.yaml`;
        const connRelPath = `${SEAQUEL_DIR}/projects/${dirName}/connections/${connFilename}`;

        const sharedConn = this.buildSharedConnectionFromLocal(
          repoId,
          sharedProject.id,
          connRelPath,
          conn,
        );

        const connYaml = serializeConnectionFile(sharedConn);
        const connYamlPath = await join(connectionsDir, connFilename);
        await writeTextFile(connYamlPath, connYaml);

        sharedConnections.push(sharedConn);
      }

      sharedProject.connections = sharedConnections;

      // Write labels.yaml if provided
      if (options?.labels && options.labels.length > 0) {
        const labelsYaml = serializeLabelsFile({ labels: options.labels });
        const labelsPath = await join(seaquelDir, "labels.yaml");
        await writeTextFile(labelsPath, labelsYaml);
      }

      // Reload configs
      await this.loadSharedConfigs(repoId);

      return sharedProject;
    } catch (error) {
      void log.error("Failed to export project:", error);
      throw error;
    }
  }

  /**
   * Share a single connection by writing its YAML file to the active repo's .seaquel/ directory.
   * Credentials are NEVER exported.
   */
  async shareConnection(connection: DatabaseConnection): Promise<void> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const project = this.state.activeProject;
    if (!project) return;

    const dirName = nameToFilename(project.name);
    const seaquelDir = await join(repo.path, SEAQUEL_DIR);
    const projectDir = await join(seaquelDir, "projects", dirName);
    const connectionsDir = await join(projectDir, "connections");

    try {
      // Write project.yaml if it doesn't exist
      const projectYamlPath = await join(projectDir, "project.yaml");
      if (!(await exists(projectYamlPath))) {
        const sharedProject: SharedProject = {
          id: `${repoId}:${SEAQUEL_DIR}/projects/${dirName}`,
          repoId,
          name: project.name,
          dirName,
          connections: [],
        };
        await writeTextFile(projectYamlPath, serializeProjectFile(sharedProject));
      }

      const connFilename = `${nameToFilename(connection.name)}.yaml`;
      const connRelPath = `${SEAQUEL_DIR}/projects/${dirName}/connections/${connFilename}`;
      const projectId = `${repoId}:${SEAQUEL_DIR}/projects/${dirName}`;

      const sharedConn = this.buildSharedConnectionFromLocal(
        repoId,
        projectId,
        connRelPath,
        connection,
      );

      const connYaml = serializeConnectionFile(sharedConn);
      const connYamlPath = await join(connectionsDir, connFilename);
      await writeTextFile(connYamlPath, connYaml);

      await this.loadSharedConfigs(repoId);
      await this.refreshRepoStatus(repoId);
    } catch (error) {
      void log.error("Failed to share connection:", error);
      throw error;
    }
  }

  /**
   * Update a shared connection's YAML file in the repo.
   * If the name changed, renames the file and updates content.
   */
  async updateSharedConnection(oldName: string, connection: DatabaseConnection): Promise<void> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const project = this.state.activeProject;
    if (!project) return;

    const dirName = nameToFilename(project.name);
    const connectionsDir = await join(repo.path, SEAQUEL_DIR, "projects", dirName, "connections");

    try {
      const oldFilename = `${nameToFilename(oldName)}.yaml`;
      const newFilename = `${nameToFilename(connection.name)}.yaml`;
      const oldPath = await join(connectionsDir, oldFilename);
      const newPath = await join(connectionsDir, newFilename);

      // Rename file if name changed
      if (oldFilename !== newFilename && (await exists(oldPath))) {
        await rename(oldPath, newPath);
      }

      // Update file content with current connection details
      const connRelPath = `${SEAQUEL_DIR}/projects/${dirName}/connections/${newFilename}`;
      const projectId = `${repoId}:${SEAQUEL_DIR}/projects/${dirName}`;

      const sharedConn = this.buildSharedConnectionFromLocal(
        repoId,
        projectId,
        connRelPath,
        connection,
      );

      await writeTextFile(newPath, serializeConnectionFile(sharedConn));

      await this.loadSharedConfigs(repoId);
      await this.refreshRepoStatus(repoId);
    } catch (error) {
      void log.error("Failed to update shared connection:", error);
      throw error;
    }
  }

  /**
   * Unshare a single connection by removing its YAML file from the repo.
   */
  async unshareConnection(connection: DatabaseConnection): Promise<void> {
    const repoId = this.state.activeRepoId;
    if (!repoId) return;

    const repo = this.state.sharedRepos.find((r) => r.id === repoId);
    if (!repo) return;

    const project = this.state.activeProject;
    if (!project) return;

    const dirName = nameToFilename(project.name);
    const connFilename = `${nameToFilename(connection.name)}.yaml`;
    const connPath = await join(
      repo.path,
      SEAQUEL_DIR,
      "projects",
      dirName,
      "connections",
      connFilename,
    );

    try {
      if (await exists(connPath)) {
        await remove(connPath);
      }

      await this.loadSharedConfigs(repoId);
      await this.refreshRepoStatus(repoId);
    } catch (error) {
      void log.error("Failed to unshare connection:", error);
      throw error;
    }
  }

  /**
   * Update a repository's properties.
   */
  private updateRepo(repoId: string, updates: Partial<SharedQueryRepo>): void {
    this.state.sharedRepos = this.state.sharedRepos.map((r) =>
      r.id === repoId ? { ...r, ...updates } : r,
    );
    this.schedulePersistence();
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
   * Check if a repo exists at the given path.
   */
  async repoExistsAtPath(path: string): Promise<boolean> {
    try {
      const gitDir = await join(path, ".git");
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
      void this.refreshAllRepoStatuses();
    }, interval);

    // Do an initial refresh
    void this.refreshAllRepoStatuses();
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

          // Only reload queries from disk if the repo has changes
          const updatedSyncState = this.state.syncStateByRepo[repo.id];
          const hasChanges =
            updatedSyncState &&
            (updatedSyncState.pendingChanges > 0 ||
              updatedSyncState.behindBy > 0 ||
              updatedSyncState.aheadBy > 0);

          if (hasChanges) {
            await this.loadQueriesFromRepo(repo.id);
          }
        } catch (error) {
          void log.warn(`Failed to refresh status for ${repo.name}:`, error);
        }
      }),
    );
  }
}
