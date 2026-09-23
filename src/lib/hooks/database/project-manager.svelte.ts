import type {
  Project,
  ConnectionLabel,
  PersistedProject,
  DatabaseConnection,
  SharedProject,
  SharedConnection,
} from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import { SEAQUEL_DIR, type SharedRepoManager } from "./shared-repo-manager.svelte.js";
import type { SharedQueryManager } from "./shared-query-manager.svelte.js";
import type { SharedDashboardManager } from "./shared-dashboard-manager.svelte.js";
import type { StarterTabManager } from "./starter-tabs.svelte.js";
import { MigrationManager } from "./migration.svelte.js";
import { isTauri } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import { mkdir, rename as renameFs, exists, writeTextFile } from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { nameToFilename, serializeProjectFile } from "$lib/services/config-file-parser";
import type { PersistedWorkflowTab } from "$lib/types/persisted";
import type { SavedWorkflow } from "$lib/types/workflow";
import type { PersistedProjectState } from "$lib/types/project";

/** Legacy persisted state from before canvas→workflow rename */
interface LegacyPersistedProjectState extends PersistedProjectState {
  canvasTabs?: PersistedWorkflowTab[];
  savedCanvases?: SavedWorkflow[];
  activeCanvasTabId?: string | null;
}

/**
 * Manages projects and their lifecycle.
 * Projects group connections and provide organization.
 */
export class ProjectManager {
  private migration: MigrationManager;
  private removeConnection:
    | ((connectionId: string, options?: { skipUnshare?: boolean }) => Promise<void>)
    | null = null;
  private starterTabManager: StarterTabManager | null = null;
  private sharedRepos: SharedRepoManager | null = null;
  private sharedQueryManager: SharedQueryManager | null = null;
  private sharedDashboardManager: SharedDashboardManager | null = null;

  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager,
    private stateRestoration: StateRestorationManager,
  ) {
    this.migration = new MigrationManager(persistence);
  }

  /**
   * Set the shared repo manager reference.
   * Called by the main database class after SharedRepoManager is created.
   */
  setSharedRepoManager(manager: SharedRepoManager): void {
    this.sharedRepos = manager;
  }

  /**
   * Set the shared query manager reference.
   * Called by the main database class after SharedQueryManager is created.
   */
  setSharedQueryManager(manager: SharedQueryManager): void {
    this.sharedQueryManager = manager;
  }

  /**
   * Set the shared dashboard manager reference.
   * Called by the main database class after SharedDashboardManager is created.
   */
  setSharedDashboardManager(manager: SharedDashboardManager): void {
    this.sharedDashboardManager = manager;
  }

  /**
   * Set the callback for removing connections.
   * This is called by the main database class after ConnectionManager is created.
   */
  setRemoveConnectionCallback(
    callback: (connectionId: string, options?: { skipUnshare?: boolean }) => Promise<void>,
  ): void {
    this.removeConnection = callback;
  }

  /**
   * Set the starter tab manager reference.
   * Called by the main database class after StarterTabManager is created.
   */
  setStarterTabManager(manager: StarterTabManager): void {
    this.starterTabManager = manager;
  }

  /**
   * Initialize projects on app startup.
   * Runs migrations if needed and loads projects.
   */
  async initialize(): Promise<void> {
    // Run migrations first
    await this.migration.migrateIfNeeded();

    // Load projects
    const persistedProjects = await this.persistence.loadProjects();

    if (persistedProjects.length === 0) {
      // Create default project
      const defaultProject = this.createDefaultProject();
      this.state.projects = [defaultProject];
      await this.persistence.persistProjects();
    } else {
      // Deserialize projects
      this.state.projects = persistedProjects.map((p) => this.deserializeProject(p));
    }

    // Set active project
    const lastActiveProjectId = await this.persistence.getLastActiveProjectId();
    const validProjectId = this.state.projects.find((p) => p.id === lastActiveProjectId)?.id;
    this.state.activeProjectId = validProjectId || this.state.projects[0]?.id || null;

    // Load project state if there's an active project
    if (this.state.activeProjectId) {
      await this.loadProjectState(this.state.activeProjectId);
    }

    this.state.projectsLoading = false;
  }

  /**
   * Create a new project.
   * The newly created project is automatically set as active.
   */
  async add(name: string, description?: string): Promise<Project> {
    const now = new Date();
    const project: Project = {
      id: `project-${crypto.randomUUID()}`,
      name,
      description,
      createdAt: now,
      updatedAt: now,
      customLabels: [],
    };

    this.state.projects = [...this.state.projects, project];
    await this.persistence.persistProjects();

    // Automatically make the new project active
    await this.setActive(project.id);

    return project;
  }

  /**
   * Update an existing project.
   */
  async update(
    id: string,
    updates: Partial<Pick<Project, "name" | "description" | "gitRepoPath">>,
  ): Promise<void> {
    const project = this.state.projects.find((p) => p.id === id);

    // Rename git repo project directory and update project.yaml if the name changed
    if (updates.name && project && project.name !== updates.name && project.gitRepoPath) {
      const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
      if (repo) {
        try {
          const oldDirName = nameToFilename(project.name);
          const newDirName = nameToFilename(updates.name);
          const projectsDir = await join(repo.path, SEAQUEL_DIR, "projects");
          const oldDir = await join(projectsDir, oldDirName);

          if (await exists(oldDir)) {
            // Rename directory if the filename changed
            const targetDir =
              oldDirName !== newDirName ? await join(projectsDir, newDirName) : oldDir;
            if (oldDirName !== newDirName) {
              await renameFs(oldDir, targetDir);
            }

            // Update name in project.yaml
            const projectYamlPath = await join(targetDir, "project.yaml");
            if (await exists(projectYamlPath)) {
              const yaml = serializeProjectFile({
                id: `${repo.id}:${SEAQUEL_DIR}/projects/${newDirName}`,
                repoId: repo.id,
                name: updates.name,
                description: updates.description ?? project.description,
                dirName: newDirName,
                connections: [],
              });
              await writeTextFile(projectYamlPath, yaml);
            }

            // Reload shared state so in-memory paths reflect the renamed directory
            if (this.sharedRepos) {
              await this.sharedRepos.loadQueriesFromRepo(repo.id);
            }

            // Tab queryIds are stable SQLite IDs — no path updates needed on rename
          }
        } catch {
          // Directory may not exist yet
        }
      }
    }

    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== id) return p;
      return {
        ...p,
        ...updates,
        updatedAt: new Date(),
      };
    });
    await this.persistence.persistProjects();
  }

  /**
   * Set the git repo path for a project and trigger scanning.
   * When set for the first time, auto-links the project to the repo.
   */
  async setGitRepoPath(projectId: string, path: string | undefined): Promise<void> {
    const project = this.state.projects.find((p) => p.id === projectId);
    if (!project) return;

    const hadGitPath = !!project.gitRepoPath;
    const oldGitRepoPath = project.gitRepoPath;

    // Update the project
    await this.update(projectId, { gitRepoPath: path });

    if (path && this.sharedRepos) {
      // Create .seaquel directory structure while dialog scope is active
      try {
        const dirName = nameToFilename(project.name);
        const projectDir = await join(path, SEAQUEL_DIR, "projects", dirName);
        await mkdir(await join(projectDir, "connections"), { recursive: true });
        await mkdir(await join(projectDir, "queries"), { recursive: true });
        await mkdir(await join(projectDir, "dashboards"), { recursive: true });
      } catch {
        // Directory may already exist
      }

      // Check if a SharedQueryRepo entry already exists for this path
      const existingRepo = this.state.sharedRepos.find((r) => r.path === path);

      let activeRepoId: string;

      if (existingRepo) {
        activeRepoId = existingRepo.id;
        // Reuse existing repo - set as active when this project is active
        if (this.state.activeProjectId === projectId) {
          this.state.activeRepoId = existingRepo.id;
        }
        // Reload queries/configs
        await this.sharedRepos.loadQueriesFromRepo(existingRepo.id);
      } else {
        // Register the path as a new repo (without cloning)
        activeRepoId = await this.sharedRepos.initRepo(project.name, path);

        // Set as active when this project is active
        if (this.state.activeProjectId === projectId) {
          this.state.activeRepoId = activeRepoId;
        }

        // Load existing queries and shared configs from the repo
        await this.sharedRepos.loadQueriesFromRepo(activeRepoId);
      }

      // Auto-detect remote URL from the git repo
      const linkedRepo = this.state.sharedRepos.find((r) => r.path === path);
      if (linkedRepo && !linkedRepo.remoteUrl && isTauri()) {
        try {
          const { getRemoteUrl } = await import("$lib/services/git");
          const remoteUrl = await getRemoteUrl(path);
          if (remoteUrl) {
            await this.sharedRepos.setRemoteUrl(linkedRepo.id, remoteUrl);
          }
        } catch {
          // No remote configured — that's fine
        }
      }

      // If first-time setup, export existing non-local-only connections to git
      if (!hadGitPath) {
        const projectConnections = this.state.connections.filter(
          (c) => c.projectId === projectId && !c.isLocalOnly,
        );
        if (projectConnections.length > 0) {
          const activeRepo = this.state.sharedRepos.find((r) => r.path === path);
          if (activeRepo) {
            await this.sharedRepos.exportProject(
              activeRepo.id,
              project.name,
              projectConnections,
              {},
            );
          }
        }
      }
    } else if (!path) {
      // Clearing git path - clean up shared state for the old repo
      const oldRepo = oldGitRepoPath
        ? this.state.sharedRepos.find((r) => r.path === oldGitRepoPath)
        : null;

      if (oldRepo) {
        // Remove local connections that were imported from this repo's shared connections
        const repoId = oldRepo.id;
        const sharedProjects = this.state.sharedProjectsByRepo[repoId] ?? [];
        const sharedConnectionIds = new Set(
          sharedProjects.flatMap((sp) =>
            (this.state.sharedConnectionsByProject[sp.id] ?? []).map((sc) => sc.id),
          ),
        );
        if (sharedConnectionIds.size > 0) {
          const importedConnections = this.state.connections.filter(
            (c) =>
              c.projectId === projectId &&
              c.sharedConnectionId &&
              sharedConnectionIds.has(c.sharedConnectionId),
          );
          for (const conn of importedConnections) {
            await this.persistence.removePersistedConnection(conn.id);
          }
          this.state.connections = this.state.connections.filter(
            (c) =>
              !(
                c.projectId === projectId &&
                c.sharedConnectionId &&
                sharedConnectionIds.has(c.sharedConnectionId)
              ),
          );
        }

        // Remove the repo and all its shared state (queries, configs, etc.)
        if (this.sharedRepos) {
          this.sharedRepos.removeRepo(repoId);
        }
      }

      if (this.state.activeProjectId === projectId) {
        this.state.activeRepoId = null;
      }
    }
  }

  /**
   * Delete a project and all its connections.
   * Cannot delete the last project.
   */
  async remove(id: string): Promise<boolean> {
    // Cannot delete the last project
    if (this.state.projects.length <= 1) {
      void log.warn("Cannot delete the last project");
      return false;
    }

    // Delete all connections in the project (must await to avoid race conditions
    // where setActiveForProject schedules persistence for the soon-to-be-deleted project)
    const projectConnections = this.state.connections.filter((c) => c.projectId === id);
    if (this.removeConnection) {
      for (const connection of projectConnections) {
        await this.removeConnection(connection.id, { skipUnshare: true });
        // Cancel any debounced persistence scheduled by removeConnection
        // (e.g. via setActiveForProject) before the next iteration's await
        // gives the timer a chance to fire against the soon-to-be-deleted project
        this.persistence.cancelPendingPersistenceFor(id, [connection.id]);
      }
    }

    // Remove project and its state from database
    await this.persistence.removeProjectState(id);
    await this.persistence.removeProject(id);

    // Remove from in-memory state
    this.state.projects = this.state.projects.filter((p) => p.id !== id);

    // Switch active project if needed
    if (this.state.activeProjectId === id) {
      // Clear active project ID first to prevent setActive from trying
      // to persist state for the just-deleted project (FK constraint)
      this.state.activeProjectId = null;
      await this.setActive(this.state.projects[0]?.id || null);
    }

    return true;
  }

  /**
   * Set the active project.
   */
  async setActive(id: string | null): Promise<void> {
    if (id === this.state.activeProjectId) return;

    void log.info(`Project changed: from=${this.state.activeProjectId} to=${id}`);

    // Save current project state before switching
    if (this.state.activeProjectId) {
      await this.persistence.persistProjectState(this.state.activeProjectId);
    }

    // Preload saved queries/dashboards before changing activeProjectId so that
    // $derived values (e.g. projectQueries) see the data immediately.
    if (id && !(id in this.state.queriesByProject)) {
      await this.stateRestoration.loadProjectData(id);
    }

    this.state.activeProjectId = id;
    await this.persistence.persistAppState();

    // Load new project state
    if (id) {
      await this.loadProjectState(id);

      // Auto-link repo when project has gitRepoPath
      const project = this.state.projects.find((p) => p.id === id);
      if (project?.gitRepoPath && this.sharedRepos) {
        const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
        if (repo) {
          this.state.activeRepoId = repo.id;
          await this.reconcileGitState(id);
        }
      }
    }
  }

  /**
   * Add a custom label to a project.
   */
  async addCustomLabel(
    projectId: string,
    label: Omit<ConnectionLabel, "id" | "isPredefined">,
  ): Promise<ConnectionLabel> {
    const newLabel: ConnectionLabel = {
      id: `label-${crypto.randomUUID()}`,
      name: label.name,
      color: label.color,
      isPredefined: false,
    };

    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== projectId) return p;
      return {
        ...p,
        customLabels: [...p.customLabels, newLabel],
        updatedAt: new Date(),
      };
    });
    await this.persistence.persistProjects();

    return newLabel;
  }

  /**
   * Remove a custom label from a project.
   * Also removes the label from all connections.
   */
  async removeCustomLabel(projectId: string, labelId: string): Promise<void> {
    // Remove from project
    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== projectId) return p;
      return {
        ...p,
        customLabels: p.customLabels.filter((l) => l.id !== labelId),
        updatedAt: new Date(),
      };
    });
    await this.persistence.persistProjects();

    // Remove from connections
    this.state.connections = this.state.connections.map((c) => {
      if (c.projectId !== projectId) return c;
      if (!c.labelIds.includes(labelId)) return c;
      return {
        ...c,
        labelIds: c.labelIds.filter((id) => id !== labelId),
      };
    });
  }

  /**
   * Update a custom label.
   */
  async updateCustomLabel(
    projectId: string,
    labelId: string,
    updates: Partial<Pick<ConnectionLabel, "name" | "color">>,
  ): Promise<void> {
    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== projectId) return p;
      return {
        ...p,
        customLabels: p.customLabels.map((l) => {
          if (l.id !== labelId) return l;
          return { ...l, ...updates };
        }),
        updatedAt: new Date(),
      };
    });
    await this.persistence.persistProjects();
  }

  /**
   * Import shared projects from a git repo folder.
   * Creates local projects, links them to the repo, and imports their connections.
   */
  async importFromGitRepo(repoPath: string, selectedProjects: SharedProject[]): Promise<string[]> {
    const createdIds: string[] = [];

    for (const sharedProject of selectedProjects) {
      // Deduplicate name
      let name = sharedProject.name;
      const existingNames = new Set(this.state.projects.map((p) => p.name));
      if (existingNames.has(name)) {
        let suffix = 2;
        while (existingNames.has(`${name} (${suffix})`)) suffix++;
        name = `${name} (${suffix})`;
      }

      // Create the local project
      const project = await this.add(name);
      createdIds.push(project.id);

      // Link to git repo (registers repo, loads configs, exports existing connections)
      await this.setGitRepoPath(project.id, repoPath);

      // Import shared connections as local entries
      await this.importSharedConnections(project.id);

      // Reconcile git queries and dashboards into local state
      await this.reconcileGitState(project.id);
    }

    // Switch to first created project
    if (createdIds.length > 0) {
      await this.setActive(createdIds[0]);
    }

    return createdIds;
  }

  // === PRIVATE METHODS ===

  /**
   * Reconcile git .sql and .json files with local saved queries and dashboards.
   * Called after git repo is linked and queries/dashboards are scanned.
   */
  async reconcileGitState(projectId: string): Promise<void> {
    // Reconcile queries
    if (this.sharedQueryManager) {
      const queries = this.state.queriesByProject[projectId] ?? [];
      const reconciled = this.sharedQueryManager.reconcileWithGitFiles(projectId, queries);
      if (reconciled !== queries) {
        this.state.queriesByProject = {
          ...this.state.queriesByProject,
          [projectId]: reconciled,
        };
        this.persistence.scheduleProject(projectId);
      }
    }

    // Reconcile dashboards
    if (this.sharedDashboardManager) {
      const dashboards = this.state.dashboardsByProject[projectId] ?? [];
      const reconciled = this.sharedDashboardManager.reconcileWithGitFiles(projectId, dashboards);
      if (reconciled !== dashboards) {
        this.state.dashboardsByProject = {
          ...this.state.dashboardsByProject,
          [projectId]: reconciled,
        };
        await this.persistence.persistProjectDashboards(projectId);
      }
    }
  }

  /**
   * Import shared connections from the linked repo as local DatabaseConnection entries.
   * Skips connections that are already imported (matched by sharedConnectionId).
   * Called explicitly (e.g. on project settings save), not automatically on folder selection.
   */
  async importSharedConnections(projectId: string): Promise<void> {
    const project = this.state.projects.find((p) => p.id === projectId);
    if (!project?.gitRepoPath) return;

    const repo = this.state.sharedRepos.find((r) => r.path === project.gitRepoPath);
    if (!repo) return;

    const repoId = repo.id;
    const sharedProjects = this.state.sharedProjectsByRepo[repoId] ?? [];

    for (const sharedProject of sharedProjects) {
      const sharedConnections = this.state.sharedConnectionsByProject[sharedProject.id] ?? [];

      for (const sharedConn of sharedConnections) {
        // Check if already imported
        const alreadyImported = this.state.connections.some(
          (c) => c.sharedConnectionId === sharedConn.id,
        );
        if (alreadyImported) continue;

        // Create a local DatabaseConnection from the shared template
        const connection: DatabaseConnection = {
          id: `conn-${crypto.randomUUID()}`,
          name: sharedConn.name,
          type: sharedConn.type,
          host: sharedConn.host,
          port: sharedConn.port,
          databaseName: sharedConn.databaseName,
          username: "",
          password: "",
          sslMode: sharedConn.sslMode,
          projectId,
          labelIds: [],
          sharedConnectionId: sharedConn.id,
          sshTunnel: sharedConn.sshTunnel
            ? {
                enabled: true,
                host: sharedConn.sshTunnel.host,
                port: sharedConn.sshTunnel.port,
                username: "",
                authMethod: "key",
              }
            : undefined,
        };

        this.state.connections = [...this.state.connections, connection];
        await this.persistence.persistConnection(connection);
      }
    }
  }

  /**
   * Import a single shared connection template into a specific project.
   * Used by deep links to import individual connections.
   */
  async importSingleSharedConnection(
    sharedConn: SharedConnection,
    projectId: string,
  ): Promise<void> {
    const alreadyImported = this.state.connections.some(
      (c) => c.sharedConnectionId === sharedConn.id,
    );
    if (alreadyImported) return;

    const connection: DatabaseConnection = {
      id: `conn-${crypto.randomUUID()}`,
      name: sharedConn.name,
      type: sharedConn.type,
      host: sharedConn.host,
      port: sharedConn.port,
      databaseName: sharedConn.databaseName,
      username: "",
      password: "",
      sslMode: sharedConn.sslMode,
      projectId,
      labelIds: [],
      sharedConnectionId: sharedConn.id,
      sshTunnel: sharedConn.sshTunnel
        ? {
            enabled: true,
            host: sharedConn.sshTunnel.host,
            port: sharedConn.sshTunnel.port,
            username: "",
            authMethod: "key",
          }
        : undefined,
    };

    this.state.connections = [...this.state.connections, connection];
    await this.persistence.persistConnection(connection);
  }

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

  private deserializeProject(persisted: PersistedProject): Project {
    return {
      id: persisted.id,
      name: persisted.name,
      description: persisted.description,
      createdAt: new Date(persisted.createdAt),
      updatedAt: new Date(persisted.updatedAt),
      customLabels: persisted.customLabels,
      gitRepoPath: persisted.gitRepoPath,
    };
  }

  private async loadProjectState(projectId: string): Promise<void> {
    // Load saved queries and dashboards FIRST, before any state assignments
    // that trigger UI re-renders via $derived. This ensures queriesByProject
    // is populated when projectQueries recomputes after activeProjectId changes.
    await this.stateRestoration.loadProjectData(projectId);

    const persistedState = (await this.persistence.loadProjectState(
      projectId,
    )) as LegacyPersistedProjectState | null;
    if (!persistedState) {
      // Initialize empty state for this project
      this.state.queryTabsByProject[projectId] = [];
      this.state.schemaTabsByProject[projectId] = [];
      this.state.explainTabsByProject[projectId] = [];
      this.state.erdTabsByProject[projectId] = [];
      this.state.statisticsTabsByProject[projectId] = [];
      this.state.workflowTabsByProject[projectId] = [];
      this.state.savedWorkflowsByProject[projectId] = [];
      this.state.dashboardTabsByProject[projectId] = [];
      this.state.tabOrderByProject[projectId] = [];
      this.state.connectionOrderByProject[projectId] = [];
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
    this.state.schemaTabsByProject[projectId] = persistedState.schemaTabs
      .filter((t) => t.connectionId)
      .map((t) => ({
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
    this.state.erdTabsByProject[projectId] = persistedState.erdTabs
      .filter((t) => t.connectionId)
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connectionId!,
      }));

    // Restore statistics tabs
    this.state.statisticsTabsByProject[projectId] = (persistedState.statisticsTabs ?? [])
      .filter((t) => t.connectionId)
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connectionId,
        isLoading: false,
      }));

    // Restore workflow tabs
    this.state.workflowTabsByProject[projectId] = (
      persistedState.workflowTabs ??
      persistedState.canvasTabs ??
      []
    )
      .filter((t) => t.connectionId)
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connectionId,
      }));

    // Restore saved workflows
    this.state.savedWorkflowsByProject[projectId] =
      persistedState.savedWorkflows ?? persistedState.savedCanvases ?? [];

    // Restore tab order and active IDs
    this.state.tabOrderByProject[projectId] = persistedState.tabOrder;
    this.state.connectionOrderByProject[projectId] = persistedState.connectionOrder ?? [];
    this.state.activeQueryTabIdByProject[projectId] = persistedState.activeQueryTabId;
    this.state.activeSchemaTabIdByProject[projectId] = persistedState.activeSchemaTabId;
    this.state.activeExplainTabIdByProject[projectId] = persistedState.activeExplainTabId;
    this.state.activeErdTabIdByProject[projectId] = persistedState.activeErdTabId;
    this.state.activeStatisticsTabIdByProject[projectId] =
      persistedState.activeStatisticsTabId ?? null;
    this.state.activeWorkflowTabIdByProject[projectId] =
      persistedState.activeWorkflowTabId ?? persistedState.activeCanvasTabId ?? null;
    // Restore the active connection ID if the connection exists (even if not yet reconnected).
    // Auto-reconnect runs after restore and will establish providerConnectionId.
    const restoredConnectionExists = persistedState.activeConnectionId
      ? this.state.connections.some((c) => c.id === persistedState.activeConnectionId)
      : false;
    this.state.activeConnectionIdByProject[projectId] = restoredConnectionExists
      ? persistedState.activeConnectionId
      : null;
    this.state.activeView = persistedState.activeView;

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
      // Only show starter tabs if the project has no other open tabs
      const hasOtherTabs =
        (this.state.queryTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.schemaTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.connectionTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.explainTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.erdTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.dashboardTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.settingsTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.createTableTabsByProject[projectId]?.length ?? 0) > 0 ||
        (this.state.dataTabsByProject[projectId]?.length ?? 0) > 0;
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
    this.state.createTableTabsByProject[projectId] = (persistedState.createTableTabs ?? [])
      .filter((t) => t.connectionId)
      .map((t) => ({
        id: t.id,
        connectionId: t.connectionId,
        name: t.name,
        tableDefinition:
          typeof t.tableDefinition === "string"
            ? JSON.parse(t.tableDefinition)
            : { tableName: "", schemaName: "public", columns: [], indexes: [], foreignKeys: [] },
      }));
    this.state.activeCreateTableTabIdByProject[projectId] =
      persistedState.activeCreateTableTabId ?? null;

    // Restore data tabs (results will be fetched fresh on activation)
    this.state.dataTabsByProject[projectId] = (persistedState.dataTabs ?? [])
      .filter((t) => t.connectionId)
      .map((t) => ({
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

    // Restore DuckDB extensions tabs
    this.state.extensionsDuckdbTabsByProject[projectId] = (
      persistedState.extensionsDuckdbTabs ?? []
    )
      .filter((t) => t.connectionId)
      .map((t) => ({
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
