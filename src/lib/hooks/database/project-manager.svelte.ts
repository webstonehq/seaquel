import type { Project, ConnectionLabel, PersistedProject } from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import { MigrationManager, CURRENT_STORAGE_VERSION } from "./migration.svelte.js";

/**
 * Manages projects and their lifecycle.
 * Projects group connections and provide organization.
 */
export class ProjectManager {
  private migration: MigrationManager;
  private removeConnection: ((connectionId: string) => void) | null = null;
  private initializeStarterTabs: ((projectId: string) => void) | null = null;

  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager
  ) {
    this.migration = new MigrationManager(persistence);
  }

  /**
   * Set the callback for removing connections.
   * This is called by the main database class after ConnectionManager is created.
   */
  setRemoveConnectionCallback(callback: (connectionId: string) => void): void {
    this.removeConnection = callback;
  }

  /**
   * Set the callback for initializing starter tabs.
   * This is called by the main database class after StarterTabManager is created.
   */
  setInitializeStarterTabsCallback(callback: (projectId: string) => void): void {
    this.initializeStarterTabs = callback;
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
      id: `project-${Date.now()}`,
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
  update(id: string, updates: Partial<Pick<Project, "name" | "description">>): void {
    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== id) return p;
      return {
        ...p,
        ...updates,
        updatedAt: new Date(),
      };
    });
    this.persistence.persistProjects();
  }

  /**
   * Delete a project and all its connections.
   * Cannot delete the last project.
   */
  async remove(id: string): Promise<boolean> {
    // Cannot delete the last project
    if (this.state.projects.length <= 1) {
      console.warn("Cannot delete the last project");
      return false;
    }

    // Delete all connections in the project
    const projectConnections = this.state.connections.filter((c) => c.projectId === id);
    if (this.removeConnection) {
      for (const connection of projectConnections) {
        this.removeConnection(connection.id);
      }
    }

    // Remove project state
    await this.persistence.removeProjectState(id);

    // Remove from state
    this.state.projects = this.state.projects.filter((p) => p.id !== id);
    await this.persistence.persistProjects();

    // Switch active project if needed
    if (this.state.activeProjectId === id) {
      await this.setActive(this.state.projects[0]?.id || null);
    }

    return true;
  }

  /**
   * Set the active project.
   */
  async setActive(id: string | null): Promise<void> {
    if (id === this.state.activeProjectId) return;

    // Save current project state before switching
    if (this.state.activeProjectId) {
      await this.persistence.persistProjectState(this.state.activeProjectId);
    }

    this.state.activeProjectId = id;
    await this.persistence.persistAppState();

    // Load new project state
    if (id) {
      await this.loadProjectState(id);
    }
  }

  /**
   * Add a custom label to a project.
   */
  addCustomLabel(
    projectId: string,
    label: Omit<ConnectionLabel, "id" | "isPredefined">
  ): ConnectionLabel {
    const newLabel: ConnectionLabel = {
      id: `label-${Date.now()}`,
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
    this.persistence.persistProjects();

    return newLabel;
  }

  /**
   * Remove a custom label from a project.
   * Also removes the label from all connections.
   */
  removeCustomLabel(projectId: string, labelId: string): void {
    // Remove from project
    this.state.projects = this.state.projects.map((p) => {
      if (p.id !== projectId) return p;
      return {
        ...p,
        customLabels: p.customLabels.filter((l) => l.id !== labelId),
        updatedAt: new Date(),
      };
    });
    this.persistence.persistProjects();

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
  updateCustomLabel(
    projectId: string,
    labelId: string,
    updates: Partial<Pick<ConnectionLabel, "name" | "color">>
  ): void {
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
    this.persistence.persistProjects();
  }

  // === PRIVATE METHODS ===

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
    };
  }

  private async loadProjectState(projectId: string): Promise<void> {
    const persistedState = await this.persistence.loadProjectState(projectId);
    if (!persistedState) {
      // Initialize empty state for this project
      this.state.queryTabsByProject[projectId] = [];
      this.state.schemaTabsByProject[projectId] = [];
      this.state.explainTabsByProject[projectId] = [];
      this.state.erdTabsByProject[projectId] = [];
      this.state.statisticsTabsByProject[projectId] = [];
      this.state.canvasTabsByProject[projectId] = [];
      this.state.savedCanvasesByProject[projectId] = [];
      this.state.tabOrderByProject[projectId] = [];
      this.state.activeQueryTabIdByProject[projectId] = null;
      this.state.activeSchemaTabIdByProject[projectId] = null;
      this.state.activeExplainTabIdByProject[projectId] = null;
      this.state.activeErdTabIdByProject[projectId] = null;
      this.state.activeStatisticsTabIdByProject[projectId] = null;
      this.state.activeCanvasTabIdByProject[projectId] = null;
      this.state.activeConnectionIdByProject[projectId] = null;
      // Initialize starter tabs for new projects
      this.initializeStarterTabs?.(projectId);
      return;
    }

    // Restore tabs - query tabs
    this.state.queryTabsByProject[projectId] = persistedState.queryTabs.map((t) => ({
      id: t.id,
      name: t.name,
      query: t.query,
      savedQueryId: t.savedQueryId,
      isExecuting: false,
    }));

    // Restore schema tabs (we'll need to look up the table info later)
    // For now, create placeholder tabs that will be populated when the connection loads
    this.state.schemaTabsByProject[projectId] = persistedState.schemaTabs.map((t) => ({
      id: t.id,
      table: {
        schema: t.schemaName,
        name: t.tableName,
        type: 'table' as const, // Default to table, will be updated when metadata loads
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

    // Restore canvas tabs
    this.state.canvasTabsByProject[projectId] = (persistedState.canvasTabs ?? [])
      .filter((t) => t.connectionId)
      .map((t) => ({
        id: t.id,
        name: t.name,
        connectionId: t.connectionId,
      }));

    // Restore saved canvases
    this.state.savedCanvasesByProject[projectId] = persistedState.savedCanvases ?? [];

    // Restore tab order and active IDs
    this.state.tabOrderByProject[projectId] = persistedState.tabOrder;
    this.state.activeQueryTabIdByProject[projectId] = persistedState.activeQueryTabId;
    this.state.activeSchemaTabIdByProject[projectId] = persistedState.activeSchemaTabId;
    this.state.activeExplainTabIdByProject[projectId] = persistedState.activeExplainTabId;
    this.state.activeErdTabIdByProject[projectId] = persistedState.activeErdTabId;
    this.state.activeStatisticsTabIdByProject[projectId] = persistedState.activeStatisticsTabId ?? null;
    this.state.activeCanvasTabIdByProject[projectId] = persistedState.activeCanvasTabId ?? null;
    this.state.activeConnectionIdByProject[projectId] = persistedState.activeConnectionId;
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
    } else {
      // Initialize default starter tabs if none persisted
      this.initializeStarterTabs?.(projectId);
    }
  }
}
