import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";
import type { ProviderRegistry } from "$lib/providers";
import { isTauri, isDemo } from "$lib/utils/environment";
import { isFeatureEnabled } from "$lib/features";
import { getKeyringService } from "$lib/services/keyring";
import { VaultCancelledError } from "$lib/services/vault/vault-state.svelte";
import { SvelteSet } from "svelte/reactivity";
import type { SharedRepoManager } from "./shared-repo-manager.svelte.js";
import { log } from "$lib/utils/logger";

type ConnectionInput = Omit<DatabaseConnection, "id" | "projectId" | "labelIds"> & {
  projectId?: string;
  labelIds?: string[];
  sshPassword?: string;
  sshKeyPath?: string;
  sshKeyPassphrase?: string;
  savePassword?: boolean;
  saveSshPassword?: boolean;
  saveSshKeyPassphrase?: boolean;
  createIfMissing?: boolean;
};

/**
 * Manages database connections: add, reconnect, remove, test.
 * Handles SSH tunnel lifecycle and schema loading.
 */
export class ConnectionManager {
  // Map connection IDs to their SSH tunnel IDs for cleanup
  private tunnelIds = new Map<string, string>();

  // Track which connections are currently being connected (for UI loading indicators)
  readonly connectingIds = new SvelteSet<string>();

  private sharedRepos: SharedRepoManager | null = null;

  setSharedRepoManager(manager: SharedRepoManager): void {
    this.sharedRepos = manager;
  }

  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager,
    private stateRestoration: StateRestorationManager,
    private tabOrdering: TabOrderingManager,
    private providers: ProviderRegistry,
    private onSchemaLoaded: (
      connectionId: string,
      schemas: SchemaTable[],
      adapter: DatabaseAdapter,
      providerConnectionId?: string,
    ) => Promise<void>,
    private onCreateInitialTab: () => void,
    private onActiveConnectionChanged: () => void = () => {},
  ) {}

  /**
   * Initialize persisted connections on app startup.
   */
  async initializePersistedConnections(): Promise<void> {
    try {
      const persistedConnections = await this.persistence.loadPersistedConnections();
      const keyring = getKeyringService();

      // Phase 1: Build connection objects + fetch keyring passwords in parallel
      const connectionEntries = await Promise.all(
        persistedConnections.map(async (persisted) => {
          let password = "";
          // Only pre-fetch when the keyring is *currently unlocked* — on
          // desktop that's always true; on web it's true only if the
          // user has already unlocked the vault in this tab. The web
          // connect flow prompts to unlock on demand when the user
          // actually hits Connect, so skipping here just avoids an
          // unsolicited passphrase dialog on every page load.
          if (persisted.savePassword && keyring.isUnlocked()) {
            try {
              const savedPassword = await keyring.getDbPassword(persisted.id);
              if (savedPassword) {
                password = savedPassword;
              }
            } catch (error) {
              void log.warn("Failed to load password from keyring:", error);
            }
          }

          // Extract username from connection string if not stored separately (backwards compat)
          let username = persisted.username ?? "";
          if (!username && persisted.connectionString) {
            try {
              const connStr = persisted.connectionString.replace("postgresql://", "postgres://");
              // SQLite and DuckDB use file-based connection strings, not URLs
              if (!connStr.startsWith("sqlite") && !connStr.startsWith("duckdb")) {
                const url = new URL(connStr);
                username = url.username ? decodeURIComponent(url.username) : "";
              }
            } catch {
              // Ignore parsing errors
            }
          }

          const connection: DatabaseConnection = {
            id: persisted.id,
            name: persisted.name,
            type: persisted.type,
            host: persisted.host,
            port: persisted.port,
            databaseName: persisted.databaseName,
            username,
            password,
            sslMode: persisted.sslMode,
            connectionString: persisted.connectionString,
            lastConnected: persisted.lastConnected ? new Date(persisted.lastConnected) : undefined,
            sshTunnel: persisted.sshTunnel,
            savePassword: persisted.savePassword,
            saveSshPassword: persisted.saveSshPassword,
            saveSshKeyPassphrase: persisted.saveSshKeyPassphrase,
            projectId: persisted.projectId || DEFAULT_PROJECT_ID,
            labelIds: persisted.labelIds || [],
            isLocalOnly: persisted.isLocalOnly,
            sharedConnectionId: persisted.sharedConnectionId,
            activeAIProviderId: persisted.activeAIProviderId,
            activeAIModel: persisted.activeAIModel,
          };
          return connection;
        }),
      );

      // Phase 2: Register all connections in state (must complete before loading data)
      for (const connection of connectionEntries) {
        this.state.connections.push(connection);
        this.stateRestoration.initializeConnectionMaps(connection.id);
        // Ensure legacy rows (pre-connection-order migration) are represented
        // in the in-memory order; the first persist writes them back to disk.
        this.appendToOrder(connection.projectId, connection.id);
      }

      // Phase 3: Load connection data (query history, AI chats) in parallel
      await Promise.all(
        connectionEntries.map((conn) => this.stateRestoration.loadConnectionData(conn.id)),
      );
    } catch (error) {
      void log.error("Failed to load persisted connections:", error);
      // Silently fail - app will continue with no persisted connections
    } finally {
      this.state.connectionsLoading = false;
    }
  }

  /**
   * Establish SSH tunnel if configured.
   */
  private async setupSshTunnel(
    connection: {
      sshTunnel?: DatabaseConnection["sshTunnel"];
      host: string;
      port: number;
      connectionString?: string;
    },
    credentials: {
      sshPassword?: string;
      sshKeyPath?: string;
      sshKeyPassphrase?: string;
    },
    connectionId: string,
  ): Promise<{ effectiveConnectionString: string | undefined; tunnelLocalPort?: number }> {
    if (!connection.sshTunnel?.enabled) {
      return { effectiveConnectionString: connection.connectionString };
    }

    // Belt-and-braces guard for environments that don't support tunnels
    // (web tenant container, demo). The wizard hides the form there so
    // new connections won't have `sshTunnel.enabled = true`, but a
    // connection imported from a desktop install could; surface a clean
    // error instead of letting the Tauri `invoke()` blow up.
    if (!isFeatureEnabled("sshTunnels")) {
      const msg = "SSH tunnels are not available in this build";
      void log.warn(`${msg} (connection ${connectionId})`);
      throw new Error(msg);
    }

    try {
      void log.info(`Establishing SSH tunnel for ${connectionId}`);
      const tunnelResult = await createSshTunnel({
        sshHost: connection.sshTunnel.host,
        sshPort: connection.sshTunnel.port,
        sshUsername: connection.sshTunnel.username,
        authMethod: connection.sshTunnel.authMethod,
        password: credentials.sshPassword,
        keyPath: credentials.sshKeyPath,
        keyPassphrase: credentials.sshKeyPassphrase,
        remoteHost: connection.host,
        remotePort: connection.port,
      });

      let effectiveConnectionString = connection.connectionString;
      if (effectiveConnectionString) {
        const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
        url.hostname = "127.0.0.1";
        url.port = String(tunnelResult.localPort);
        effectiveConnectionString = url.toString();
      }

      void log.info(`SSH tunnel established for ${connectionId}`);
      toast.success(`SSH tunnel established on port ${tunnelResult.localPort}`);
      this.tunnelIds.set(connectionId, tunnelResult.tunnelId);

      return {
        effectiveConnectionString,
        tunnelLocalPort: tunnelResult.localPort,
      };
    } catch (error) {
      void log.error(`SSH tunnel failed for ${connectionId}`);
      errorToast(`SSH tunnel failed: ${JSON.stringify(error)}`);
      throw error;
    }
  }

  /**
   * Add a new database connection.
   */
  async add(connection: ConnectionInput): Promise<string> {
    void log.info(`Adding connection: type=${connection.type}`);
    const connectionId = `conn-${crypto.randomUUID()}`;
    this.connectingIds.add(connectionId);

    try {
      const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
        connection,
        {
          sshPassword: connection.sshPassword,
          sshKeyPath: connection.sshKeyPath,
          sshKeyPassphrase: connection.sshKeyPassphrase,
        },
        connectionId,
      );

      // Connect to database via unified provider
      const provider = await this.providers.getForType(connection.type);
      const providerConnectionId = await provider.connect({
        type: connection.type,
        host: tunnelLocalPort ? "127.0.0.1" : connection.host,
        port: tunnelLocalPort || connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        password: connection.password,
        sslMode: connection.sslMode,
        connectionString: effectiveConnectionString,
        createIfMissing: connection.createIfMissing,
      });

      const projectId = connection.projectId || this.state.activeProjectId || DEFAULT_PROJECT_ID;
      // createIfMissing applies to this connect only — don't persist it, or a
      // deleted/moved database would be silently recreated on reconnect.
      const { createIfMissing: _createIfMissing, ...persisted } = connection;
      const newConnection: DatabaseConnection = {
        ...persisted,
        id: connectionId,
        projectId,
        isLocalOnly: connection.isLocalOnly ?? true,
        labelIds: connection.labelIds || [],
        lastConnected: new Date(),
        tunnelLocalPort,
        providerConnectionId,
      };

      if (!this.state.connections.find((c) => c.id === newConnection.id)) {
        this.state.connections.push(newConnection);
      }
      this.appendToOrder(projectId, newConnection.id);

      this.stateRestoration.initializeConnectionMaps(newConnection.id);

      // Load schema - wrap in try-catch to handle failures gracefully
      const adapter = getAdapter(newConnection.type);
      let schemasWithTables: SchemaTable[];
      try {
        const schemaProvider = await this.providers.getForType(newConnection.type);
        const schemasWithTablesDbResult = await schemaProvider.select(
          providerConnectionId,
          adapter.getSchemaQuery(),
        );
        schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
      } catch (error) {
        // Cleanup: remove the connection we just added
        this.state.connections = this.state.connections.filter((c) => c.id !== newConnection.id);
        this.stateRestoration.cleanupConnectionMaps(newConnection.id);
        const cleanupProvider = await this.providers.getForType(newConnection.type);
        await cleanupProvider.disconnect(providerConnectionId).catch(() => {});
        throw new Error(`Failed to load database schema: ${String(error)}`);
      }

      // Only set active connection after schema loading succeeds
      this.setActiveForProject(newConnection.id, projectId);

      // Store tables immediately (without column metadata) so UI is responsive
      this.state.schemas = {
        ...this.state.schemas,
        [newConnection.id]: schemasWithTables,
      };

      // Load column metadata asynchronously in the background
      void this.onSchemaLoaded(
        newConnection.id,
        schemasWithTables,
        adapter,
        newConnection.providerConnectionId,
      );

      void log.info(`Schema loaded for ${newConnection.id}: ${schemasWithTables.length} tables`);

      // Create initial query tab for new connection
      this.onCreateInitialTab();

      // Persist the connection to store (password saved to keyring if enabled).
      //
      // Separate the "vault unlock cancelled" path from any other persistence
      // failure: the connection is already in memory, fully usable this
      // session — we just can't encrypt its credentials to disk yet. Roll it
      // back from memory + disconnect the provider so we don't leave an
      // orphaned half-state, then surface a specific toast.
      try {
        await this.persistence.persistConnection(newConnection, {
          savePassword: connection.savePassword,
          saveSshPassword: connection.saveSshPassword,
          saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
          sshPassword: connection.sshPassword,
          sshKeyPassphrase: connection.sshKeyPassphrase,
        });
      } catch (err) {
        if (err instanceof VaultCancelledError) {
          this.state.connections = this.state.connections.filter((c) => c.id !== newConnection.id);
          this.stateRestoration.cleanupConnectionMaps(newConnection.id);
          const cleanupProvider = await this.providers.getForType(newConnection.type);
          await cleanupProvider.disconnect(providerConnectionId).catch(() => {});
          errorToast(
            "Vault unlock cancelled — connection not saved. Unlock the vault and try again.",
          );
          throw err;
        }
        throw err;
      }

      void log.info(`Connection established: ${newConnection.id}`);
      return newConnection.id;
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  /**
   * Reconnect to an existing connection.
   */
  async reconnect(connectionId: string, connection: ConnectionInput): Promise<string> {
    void log.info(`Reconnecting: ${connectionId}`);
    const existingConnection = this.state.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }

    this.connectingIds.add(connectionId);
    try {
      // Close existing tunnel if any
      const existingTunnelId = this.tunnelIds.get(connectionId);
      if (existingTunnelId) {
        try {
          await closeSshTunnel(existingTunnelId);
        } catch {
          // Ignore cleanup errors
        }
        this.tunnelIds.delete(connectionId);
      }

      const { effectiveConnectionString: rawConnectionString, tunnelLocalPort } =
        await this.setupSshTunnel(
          connection,
          {
            sshPassword: connection.sshPassword,
            sshKeyPath: connection.sshKeyPath,
            sshKeyPassphrase: connection.sshKeyPassphrase,
          },
          connectionId,
        );

      // Inject password into connection string if provided separately
      let effectiveConnectionString = rawConnectionString;
      if (effectiveConnectionString && connection.password) {
        try {
          const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
          url.password = connection.password;
          effectiveConnectionString = url.toString();
        } catch {
          // Not a URL-based connection string (e.g., file path), skip
        }
      }

      // Close existing connection
      if (existingConnection.providerConnectionId) {
        const oldProvider = await this.providers.getForType(existingConnection.type);
        await oldProvider.disconnect(existingConnection.providerConnectionId).catch(() => {});
      }

      // Connect to database via unified provider
      const provider = await this.providers.getForType(connection.type);
      const providerConnectionId = await provider.connect({
        type: connection.type,
        host: tunnelLocalPort ? "127.0.0.1" : connection.host,
        port: tunnelLocalPort || connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        password: connection.password,
        sslMode: connection.sslMode,
        connectionString: effectiveConnectionString,
        createIfMissing: connection.createIfMissing,
      });

      // Create updated connection object to ensure Svelte reactivity sees the change
      const updatedConnection: DatabaseConnection = {
        ...existingConnection,
        providerConnectionId,
        lastConnected: new Date(),
        username: connection.username,
        password: connection.password,
        sslMode: connection.sslMode,
        connectionString: connection.connectionString,
        tunnelLocalPort,
        sshTunnel: connection.sshTunnel,
        savePassword: connection.savePassword,
        saveSshPassword: connection.saveSshPassword,
        saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      };

      // Replace the old connection with the updated one in the connections array
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? updatedConnection : c,
      );

      this.stateRestoration.ensureConnectionMapsExist(connectionId);

      // Fetch schemas - wrap in try-catch to handle failures gracefully
      const adapter = getAdapter(existingConnection.type);
      let schemasWithTables: SchemaTable[];
      try {
        const schemaProvider = await this.providers.getForType(existingConnection.type);
        const schemasWithTablesDbResult = await schemaProvider.select(
          providerConnectionId,
          adapter.getSchemaQuery(),
        );
        schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
      } catch (error) {
        // Revert: set providerConnectionId back to undefined on the connection
        this.state.connections = this.state.connections.map((c) =>
          c.id === connectionId ? { ...c, providerConnectionId: undefined } : c,
        );
        const cleanupProvider = await this.providers.getForType(existingConnection.type);
        await cleanupProvider.disconnect(providerConnectionId).catch(() => {});
        throw new Error(`Failed to load database schema: ${String(error)}`);
      }

      // Store tables immediately (without column metadata) so UI is responsive
      this.state.schemas = {
        ...this.state.schemas,
        [connectionId]: schemasWithTables,
      };

      // Load column metadata asynchronously in the background
      void this.onSchemaLoaded(connectionId, schemasWithTables, adapter, providerConnectionId);

      // Set this as the active connection (only after schema loading succeeds)
      this.setActiveForProject(connectionId, existingConnection.projectId);

      // Create initial query tab if no tabs exist for the project
      const projectId = existingConnection.projectId;
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      if (tabs.length === 0) {
        this.onCreateInitialTab();
      }

      // Persist the connection to store (password saved to keyring if enabled)
      await this.persistence.persistConnection(updatedConnection, {
        savePassword: connection.savePassword,
        saveSshPassword: connection.saveSshPassword,
        saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
        sshPassword: connection.sshPassword,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      });

      return connectionId;
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  /**
   * Update connection settings without reconnecting.
   * Used for editing connection details while preserving the connection state.
   */
  async update(connectionId: string, connection: ConnectionInput): Promise<void> {
    const existingConnection = this.state.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }

    const oldName = existingConnection.name;

    // Update connection properties (but preserve connection state like providerConnectionId)
    const updatedConnection: DatabaseConnection = {
      ...existingConnection,
      name: connection.name,
      type: connection.type,
      host: connection.host,
      port: connection.port,
      databaseName: connection.databaseName,
      username: connection.username,
      password: connection.password,
      sslMode: connection.sslMode,
      connectionString: connection.connectionString,
      sshTunnel: connection.sshTunnel,
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
    };

    // Replace the connection in the array
    this.state.connections = this.state.connections.map((c) =>
      c.id === connectionId ? updatedConnection : c,
    );

    // Persist the updated connection
    await this.persistence.persistConnection(updatedConnection, {
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      sshPassword: connection.sshPassword,
      sshKeyPassphrase: connection.sshKeyPassphrase,
    });

    // Update the shared YAML file if the connection is shared
    if (!updatedConnection.isLocalOnly && this.sharedRepos) {
      await this.sharedRepos.updateSharedConnection(oldName, updatedConnection);
    }
  }

  /**
   * Test a connection without persisting it.
   * Throws on failure so callers can display the error inline.
   */
  async test(connection: ConnectionInput): Promise<void> {
    let effectiveConnectionString = connection.connectionString;
    let tunnelId: string | undefined;
    let tunnelLocalPort: number | undefined;

    // Establish SSH tunnel if enabled (only in Tauri)
    if (connection.sshTunnel?.enabled) {
      if (!isTauri()) {
        throw new Error("SSH tunnels are only available in the desktop app");
      }
      const tunnelResult = await createSshTunnel({
        sshHost: connection.sshTunnel.host,
        sshPort: connection.sshTunnel.port,
        sshUsername: connection.sshTunnel.username,
        authMethod: connection.sshTunnel.authMethod,
        password: connection.sshPassword,
        keyPath: connection.sshKeyPath,
        keyPassphrase: connection.sshKeyPassphrase,
        remoteHost: connection.host,
        remotePort: connection.port,
      });

      tunnelId = tunnelResult.tunnelId;
      tunnelLocalPort = tunnelResult.localPort;

      // Build new connection string using tunnel (for non-MSSQL databases)
      if (effectiveConnectionString) {
        const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
        url.hostname = "127.0.0.1";
        url.port = String(tunnelResult.localPort);
        effectiveConnectionString = url.toString();
      }
    }

    try {
      const provider = await this.providers.getForType(connection.type);
      await provider.test({
        type: connection.type,
        host: tunnelLocalPort ? "127.0.0.1" : connection.host,
        port: tunnelLocalPort || connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        password: connection.password,
        sslMode: connection.sslMode,
        connectionString: effectiveConnectionString,
        createIfMissing: connection.createIfMissing,
      });
    } finally {
      // Clean up SSH tunnel if we created one
      if (tunnelId) {
        try {
          await closeSshTunnel(tunnelId);
        } catch {
          // Ignore cleanup errors
        }
      }
    }
  }

  /**
   * Remove a connection and all its state.
   */
  async remove(id: string, { skipUnshare = false } = {}): Promise<void> {
    void log.info(`Removing connection: ${id}`);
    // Prevent deletion of demo connection in demo mode
    if (isDemo() && id === "demo-connection") {
      return;
    }

    const connection = this.state.connections.find((c) => c.id === id);

    // Close provider connection if exists
    if (connection?.providerConnectionId) {
      await this.providers.getForType(connection.type).then((provider) => {
        provider.disconnect(connection.providerConnectionId!).catch((e) => void log.error(e));
      });
    }

    // Close SSH tunnel if exists
    const tunnelId = this.tunnelIds.get(id);
    if (tunnelId) {
      closeSshTunnel(tunnelId).catch((e) => void log.error(e));
      this.tunnelIds.delete(id);
    }

    // Remove the YAML file from the git directory if the connection is shared
    // Skip unsharing when removing as part of project deletion — the user is only
    // removing the project from their local Seaquel instance, not from the git repo.
    if (!skipUnshare && connection && !connection.isLocalOnly && this.sharedRepos) {
      await this.sharedRepos.unshareConnection(connection);
    }

    // Remove from persistence (both connection and its data)
    await this.persistence.removePersistedConnection(id);
    this.state.connections = this.state.connections.filter((c) => c.id !== id);
    this.stateRestoration.cleanupConnectionMaps(id);
    if (connection) {
      this.removeFromOrder(connection.projectId, id);
    }

    // If this was the active connection for its project, switch to another
    if (connection && this.state.activeConnectionIdByProject[connection.projectId] === id) {
      const nextConnection = this.state.connections.find(
        (c) => c.projectId === connection.projectId && !!c.providerConnectionId,
      );
      this.setActiveForProject(nextConnection?.id ?? null, connection.projectId);
    }
  }

  /**
   * Set the active connection for the current project.
   */
  setActive(id: string): void {
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection) {
      this.setActiveForProject(id, connection.projectId);
      this.onActiveConnectionChanged();
    }
  }

  /**
   * Set the active connection for a specific project.
   */
  setActiveForProject(connectionId: string | null, projectId: string): void {
    this.state.activeConnectionIdByProject = {
      ...this.state.activeConnectionIdByProject,
      [projectId]: connectionId,
    };
    this.persistence.scheduleProject(projectId);
  }

  /**
   * Add a demo connection that's already established.
   * Used in browser demo mode where the provider connection is pre-established.
   */
  async addDemoConnection(providerConnectionId: string): Promise<string> {
    const connectionId = "demo-connection";
    const projectId = this.state.activeProjectId || DEFAULT_PROJECT_ID;

    const newConnection: DatabaseConnection = {
      id: connectionId,
      name: "Demo Database",
      type: "duckdb",
      host: "browser",
      port: 0,
      databaseName: "demo",
      username: "",
      password: "",
      lastConnected: new Date(),
      providerConnectionId,
      projectId,
      labelIds: ["prod"],
    };

    // Check if connection already exists (from persisted storage) and update it,
    // otherwise add new connection
    const existingIndex = this.state.connections.findIndex((c) => c.id === connectionId);
    if (existingIndex >= 0) {
      // Update existing connection with providerConnectionId
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? newConnection : c,
      );
    } else {
      // Add new connection
      this.state.connections = [...this.state.connections, newConnection];
    }

    this.stateRestoration.initializeConnectionMaps(connectionId);
    this.appendToOrder(projectId, connectionId);

    // Load schema
    const adapter = getAdapter("duckdb");
    const provider = await this.providers.getOrCreateDuckDB();
    const schemasWithTablesDbResult = await provider.select(
      providerConnectionId,
      adapter.getSchemaQuery(),
    );
    const schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);

    // Set active connection
    this.setActiveForProject(connectionId, projectId);

    // Store tables
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously
    void this.onSchemaLoaded(connectionId, schemasWithTables, adapter, providerConnectionId);

    // Create initial query tab
    this.onCreateInitialTab();

    return connectionId;
  }

  /**
   * Attempt to auto-reconnect using saved keychain credentials.
   * Returns true if successful, false if credentials are missing or connection fails.
   * Use this to reconnect without showing a dialog when password is saved.
   */
  async autoReconnect(connectionId: string): Promise<boolean> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) {
      return false;
    }

    void log.info(`Auto-reconnect attempt: ${connectionId}`);
    this.connectingIds.add(connectionId);
    try {
      return await this._autoReconnect(connectionId, connection);
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  private async _autoReconnect(
    connectionId: string,
    connection: DatabaseConnection,
  ): Promise<boolean> {
    // SQLite and DuckDB don't require passwords, always auto-reconnect
    if (connection.type === "sqlite" || connection.type === "duckdb") {
      try {
        await this.reconnect(connectionId, {
          name: connection.name,
          type: connection.type,
          host: connection.host,
          port: connection.port,
          databaseName: connection.databaseName,
          username: connection.username,
          password: "",
          sslMode: connection.sslMode,
          connectionString: connection.connectionString,
        });
        return true;
      } catch {
        void log.warn(`Auto-reconnect failed: ${connectionId}`);
        return false;
      }
    }

    // Resolve the password: try keyring first, then fall back to in-memory password
    let password: string | undefined;

    if (connection.savePassword) {
      const keyring = getKeyringService();
      if (keyring.isAvailable()) {
        try {
          password = (await keyring.getDbPassword(connectionId)) || undefined;
        } catch {
          // Keyring access failed, continue with fallback
        }
      }
    }

    // Fall back to in-memory password (e.g., user connected earlier this session)
    if (!password && connection.password) {
      password = connection.password;
    }

    // If `savePassword` is off, the user opted to re-enter credentials each time —
    // fall back to the reconnect tab. Otherwise an empty password is treated as
    // an intentionally passwordless connection; the reconnect attempt below will
    // surface a failure if the server actually requires one.
    if (!password && !connection.savePassword) {
      void log.debug(`Auto-reconnect skipped (no password available): ${connectionId}`);
      return false;
    }

    // Load SSH credentials if needed
    let sshPassword: string | undefined;
    let sshKeyPassphrase: string | undefined;

    if (connection.sshTunnel?.enabled) {
      const keyring = getKeyringService();

      if (connection.saveSshPassword && keyring.isAvailable()) {
        sshPassword = (await keyring.getSshPassword(connectionId)) || undefined;
      }
      if (connection.saveSshKeyPassphrase && keyring.isAvailable()) {
        sshKeyPassphrase = (await keyring.getSshKeyPassphrase(connectionId)) || undefined;
      }

      // If SSH is enabled but credentials not available, we can't auto-reconnect
      if (connection.sshTunnel.authMethod === "password" && !sshPassword) {
        void log.debug(`Auto-reconnect skipped (no SSH password): ${connectionId}`);
        return false;
      }
      if (connection.sshTunnel.authMethod === "key" && !connection.sshTunnel.keyPath) {
        void log.debug(`Auto-reconnect skipped (no SSH key path): ${connectionId}`);
        return false;
      }
    }

    try {
      // Attempt reconnection
      await this.reconnect(connectionId, {
        name: connection.name,
        type: connection.type,
        host: connection.host,
        port: connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        password: password ?? "",
        sslMode: connection.sslMode,
        connectionString: connection.connectionString,
        sshTunnel: connection.sshTunnel,
        sshPassword,
        sshKeyPath: connection.sshTunnel?.keyPath,
        sshKeyPassphrase,
        savePassword: connection.savePassword,
        saveSshPassword: connection.saveSshPassword,
        saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      });

      void log.info(`Auto-reconnect successful: ${connectionId}`);
      return true;
    } catch {
      void log.warn(`Auto-reconnect failed: ${connectionId}`);
      return false;
    }
  }

  /**
   * Refresh the schema for a connected database (re-fetches tables/columns/indexes).
   */
  async refreshSchema(connectionId: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) {
      throw new Error("Connection not found");
    }

    if (!connection.providerConnectionId) {
      throw new Error("Connection is not active");
    }

    const adapter = getAdapter(connection.type);
    const provider = await this.providers.getForType(connection.type);
    const schemasWithTablesDbResult = await provider.select(
      connection.providerConnectionId,
      adapter.getSchemaQuery(),
    );

    const schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);

    // Preserve existing column/index metadata for tables that already exist
    // so that derived values like hasPrimaryKey don't briefly become false
    // while new metadata loads.
    const existingSchemas = this.state.schemas[connectionId] ?? [];
    const mergedSchemas = schemasWithTables.map((newTable) => {
      const existing = existingSchemas.find(
        (t) => t.name === newTable.name && t.schema === newTable.schema,
      );
      return existing
        ? { ...newTable, columns: existing.columns, indexes: existing.indexes }
        : newTable;
    });

    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: mergedSchemas,
    };

    // Reload column metadata and wait for it to complete
    await this.onSchemaLoaded(
      connectionId,
      mergedSchemas,
      adapter,
      connection.providerConnectionId,
    );
  }

  /**
   * Toggle a connection's local-only flag.
   * When switching to shared: exports connection to git.
   * When switching to local-only: removes connection from git.
   */
  async toggleLocalOnly(connectionId: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;

    const newIsLocalOnly = !connection.isLocalOnly;

    // Update in-memory state
    this.state.connections = this.state.connections.map((c) =>
      c.id === connectionId ? { ...c, isLocalOnly: newIsLocalOnly } : c,
    );

    // Persist the change
    const updated = this.state.connections.find((c) => c.id === connectionId)!;
    await this.persistence.persistConnection(updated, {
      savePassword: updated.savePassword,
      saveSshPassword: updated.saveSshPassword,
      saveSshKeyPassphrase: updated.saveSshKeyPassphrase,
    });

    // Write or remove the connection YAML in the shared repo
    if (this.sharedRepos) {
      if (newIsLocalOnly) {
        await this.sharedRepos.unshareConnection(updated);
      } else {
        await this.sharedRepos.shareConnection(updated);
      }
    }
  }

  /**
   * Toggle connection state (disconnect if connected).
   */
  async toggle(id: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection) {
      const wasConnected = !!connection.providerConnectionId;

      // Disconnect provider connection if connected
      if (connection.providerConnectionId) {
        await this.providers.getForType(connection.type).then((provider) => {
          provider.disconnect(connection.providerConnectionId!).catch((e) => void log.error(e));
        });
        this.state.connections = this.state.connections.map((c) =>
          c.id === id ? { ...c, providerConnectionId: undefined } : c,
        );
      }

      if (wasConnected) {
        void log.info(`Connection disconnected: ${id}`);
        // Remove schema tabs belonging to the disconnected connection
        const projectId = connection.projectId;
        const schemaTabs = this.state.schemaTabsByProject[projectId] ?? [];
        const removedTabIds = new Set(
          schemaTabs.filter((t) => t.connectionId === id).map((t) => t.id),
        );
        const remainingTabs = schemaTabs.filter((t) => t.connectionId !== id);
        // Remove from tab order
        const tabOrder = this.state.tabOrderByProject[projectId] ?? [];
        this.state.tabOrderByProject = {
          ...this.state.tabOrderByProject,
          [projectId]: tabOrder.filter((tabId) => !removedTabIds.has(tabId)),
        };
        this.state.schemaTabsByProject = {
          ...this.state.schemaTabsByProject,
          [projectId]: remainingTabs,
        };
        // Reset active schema tab if it was removed
        const activeSchemaTabId = this.state.activeSchemaTabIdByProject[projectId];
        if (activeSchemaTabId && removedTabIds.has(activeSchemaTabId)) {
          this.state.activeSchemaTabIdByProject = {
            ...this.state.activeSchemaTabIdByProject,
            [projectId]: remainingTabs[0]?.id ?? null,
          };
        }
        this.persistence.scheduleProject(projectId);

        // If disconnecting the active connection for its project, switch to another connected one
        if (this.state.activeConnectionIdByProject[connection.projectId] === id) {
          const nextConnection = this.state.connections.find(
            (c) => c.projectId === connection.projectId && !!c.providerConnectionId && c.id !== id,
          );
          this.setActiveForProject(nextConnection?.id ?? null, connection.projectId);
        }
      }
    }
  }

  /**
   * Replace the entire connection order for a project (used by drag-and-drop).
   */
  reorder(projectId: string, orderedIds: string[]): void {
    this.state.connectionOrderByProject = {
      ...this.state.connectionOrderByProject,
      [projectId]: [...orderedIds],
    };
    this.persistence.scheduleProject(projectId);
  }

  /**
   * Append a connection ID to its project's order (if not already present).
   */
  private appendToOrder(projectId: string, connectionId: string): void {
    const current = this.state.connectionOrderByProject[projectId] ?? [];
    if (current.includes(connectionId)) return;
    this.state.connectionOrderByProject = {
      ...this.state.connectionOrderByProject,
      [projectId]: [...current, connectionId],
    };
  }

  /**
   * Remove a connection ID from its project's order.
   */
  private removeFromOrder(projectId: string, connectionId: string): void {
    const current = this.state.connectionOrderByProject[projectId] ?? [];
    if (!current.includes(connectionId)) return;
    this.state.connectionOrderByProject = {
      ...this.state.connectionOrderByProject,
      [projectId]: current.filter((id) => id !== connectionId),
    };
  }
}
