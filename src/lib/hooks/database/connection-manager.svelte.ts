import { toast } from "svelte-sonner";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";
import { mssqlConnect, mssqlDisconnect, mssqlQuery } from "$lib/services/mssql";
import { getProvider, type DatabaseProvider } from "$lib/providers";
import { isTauri, isDemo } from "$lib/utils/environment";
import { getKeyringService } from "$lib/services/keyring";

type ConnectionInput = Omit<DatabaseConnection, "id" | "projectId" | "labelIds"> & {
  projectId?: string;
  labelIds?: string[];
  sshPassword?: string;
  sshKeyPath?: string;
  sshKeyPassphrase?: string;
  savePassword?: boolean;
  saveSshPassword?: boolean;
  saveSshKeyPassphrase?: boolean;
};

/**
 * Manages database connections: add, reconnect, remove, test.
 * Handles SSH tunnel lifecycle and schema loading.
 */
export class ConnectionManager {
  // Map connection IDs to their SSH tunnel IDs for cleanup
  private tunnelIds = new Map<string, string>();
  private provider: DatabaseProvider | null = null;

  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager,
    private stateRestoration: StateRestorationManager,
    private tabOrdering: TabOrderingManager,
    private onSchemaLoaded: (connectionId: string, schemas: SchemaTable[], adapter: DatabaseAdapter, providerConnectionId?: string, mssqlConnectionId?: string) => void,
    private onCreateInitialTab: () => void
  ) {}

  /**
   * Get or create the database provider.
   */
  private async getOrCreateProvider(): Promise<DatabaseProvider> {
    if (!this.provider) {
      this.provider = await getProvider();
    }
    return this.provider;
  }

  /**
   * Initialize persisted connections on app startup.
   */
  async initializePersistedConnections(): Promise<void> {
    try {
      const persistedConnections = await this.persistence.loadPersistedConnections();
      const keyring = getKeyringService();

      for (const persisted of persistedConnections) {
        // Try to load password from keyring if it was saved
        let password = "";
        if (persisted.savePassword && keyring.isAvailable()) {
          try {
            const savedPassword = await keyring.getDbPassword(persisted.id);
            if (savedPassword) {
              password = savedPassword;
            }
          } catch (error) {
            console.warn("Failed to load password from keyring:", error);
          }
        }

        // Extract username from connection string if not stored separately (backwards compat)
        let username = persisted.username ?? "";
        if (!username && persisted.connectionString) {
          try {
            const connStr = persisted.connectionString.replace("postgresql://", "postgres://");
            if (!connStr.startsWith("sqlite")) {
              const url = new URL(connStr);
              username = url.username ? decodeURIComponent(url.username) : "";
            }
          } catch {
            // Ignore parsing errors
          }
        }

        // Load the connection without connecting
        const connection: DatabaseConnection = {
          id: persisted.id,
          name: persisted.name,
          type: persisted.type,
          host: persisted.host,
          port: persisted.port,
          databaseName: persisted.databaseName,
          username,
          password, // Load from keyring if available
          sslMode: persisted.sslMode,
          connectionString: persisted.connectionString,
          lastConnected: persisted.lastConnected ? new Date(persisted.lastConnected) : undefined,
          sshTunnel: persisted.sshTunnel,
          savePassword: persisted.savePassword,
          saveSshPassword: persisted.saveSshPassword,
          saveSshKeyPassphrase: persisted.saveSshKeyPassphrase,
          projectId: persisted.projectId || DEFAULT_PROJECT_ID,
          labelIds: persisted.labelIds || [],
          // database is undefined - user needs to provide password to connect
        };
        this.state.connections.push(connection);
        this.stateRestoration.initializeConnectionMaps(connection.id);

        // Pre-load saved queries and history (doesn't require active DB connection)
        await this.stateRestoration.loadConnectionData(connection.id);
      }
    } catch (error) {
      console.error("Failed to load persisted connections:", error);
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
    connectionId: string
  ): Promise<{ effectiveConnectionString: string | undefined; tunnelLocalPort?: number }> {
    if (!connection.sshTunnel?.enabled) {
      return { effectiveConnectionString: connection.connectionString };
    }

    try {
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

      toast.success(`SSH tunnel established on port ${tunnelResult.localPort}`);
      this.tunnelIds.set(connectionId, tunnelResult.tunnelId);

      return {
        effectiveConnectionString,
        tunnelLocalPort: tunnelResult.localPort,
      };
    } catch (error) {
      toast.error(`SSH tunnel failed: ${error}`);
      throw error;
    }
  }

  /**
   * Add a new database connection.
   */
  async add(connection: ConnectionInput): Promise<string> {
    const connectionId = crypto.randomUUID();

    const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
      connection,
      {
        sshPassword: connection.sshPassword,
        sshKeyPath: connection.sshKeyPath,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      },
      connectionId
    );

    // Connect to database - MSSQL uses custom backend, others use provider
    let providerConnectionId: string | undefined;
    let mssqlConnectionId: string | undefined;

    if (connection.type === "mssql") {
      // MSSQL uses custom Tauri backend (only available in desktop)
      if (!isTauri()) {
        throw new Error("MSSQL connections are only available in the desktop app");
      }
      const host = tunnelLocalPort ? "127.0.0.1" : connection.host;
      const port = tunnelLocalPort || connection.port;
      const mssqlConn = await mssqlConnect({
        host,
        port,
        database: connection.databaseName,
        username: connection.username,
        password: connection.password,
        encrypt: connection.sslMode === "require",
        trustCert: connection.sslMode !== "require",
      });
      mssqlConnectionId = mssqlConn.connectionId;
    } else if (effectiveConnectionString) {
      // Use provider for PostgreSQL, SQLite, DuckDB
      const provider = await this.getOrCreateProvider();
      providerConnectionId = await provider.connect({
        type: connection.type,
        connectionString: effectiveConnectionString,
        databaseName: connection.databaseName,
      });
    }

    const projectId = connection.projectId || this.state.activeProjectId || DEFAULT_PROJECT_ID;
    const newConnection: DatabaseConnection = {
      ...connection,
      id: connectionId,
      projectId,
      labelIds: connection.labelIds || [],
      lastConnected: new Date(),
      tunnelLocalPort,
      providerConnectionId,
      mssqlConnectionId,
    };

    if (!this.state.connections.find((c) => c.id === newConnection.id)) {
      this.state.connections.push(newConnection);
    }

    this.stateRestoration.initializeConnectionMaps(newConnection.id);

    // Load schema - wrap in try-catch to handle failures gracefully
    const adapter = getAdapter(newConnection.type);
    let schemasWithTables: SchemaTable[];
    try {
      let schemasWithTablesDbResult: unknown[];
      if (connection.type === "mssql" && mssqlConnectionId) {
        const result = await mssqlQuery(mssqlConnectionId, adapter.getSchemaQuery());
        schemasWithTablesDbResult = result.rows;
      } else if (providerConnectionId) {
        const provider = await this.getOrCreateProvider();
        schemasWithTablesDbResult = await provider.select(providerConnectionId, adapter.getSchemaQuery());
      } else {
        throw new Error("No connection established");
      }
      schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
    } catch (error) {
      // Cleanup: remove the connection we just added
      this.state.connections = this.state.connections.filter((c) => c.id !== newConnection.id);
      this.stateRestoration.cleanupConnectionMaps(newConnection.id);
      if (mssqlConnectionId) {
        await mssqlDisconnect(mssqlConnectionId).catch(() => {});
      }
      if (providerConnectionId) {
        const provider = await this.getOrCreateProvider();
        await provider.disconnect(providerConnectionId).catch(() => {});
      }
      throw new Error(`Failed to load database schema: ${error}`);
    }

    // Only set active connection after schema loading succeeds
    this.setActiveForProject(newConnection.id, projectId);

    // Store tables immediately (without column metadata) so UI is responsive
    this.state.schemas = {
      ...this.state.schemas,
      [newConnection.id]: schemasWithTables,
    };

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(newConnection.id, schemasWithTables, adapter, newConnection.providerConnectionId, newConnection.mssqlConnectionId);

    // Create initial query tab for new connection
    this.onCreateInitialTab();

    // Persist the connection to store (password saved to keyring if enabled)
    this.persistence.persistConnection(newConnection, {
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      sshPassword: connection.sshPassword,
      sshKeyPassphrase: connection.sshKeyPassphrase,
    });

    return newConnection.id;
  }

  /**
   * Reconnect to an existing connection.
   */
  async reconnect(connectionId: string, connection: ConnectionInput): Promise<string> {
    const existingConnection = this.state.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }

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

    const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
      connection,
      {
        sshPassword: connection.sshPassword,
        sshKeyPath: connection.sshKeyPath,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      },
      connectionId
    );

    // Close existing connections
    if (existingConnection.mssqlConnectionId) {
      await mssqlDisconnect(existingConnection.mssqlConnectionId).catch(() => {});
    }
    if (existingConnection.providerConnectionId) {
      const provider = await this.getOrCreateProvider();
      await provider.disconnect(existingConnection.providerConnectionId).catch(() => {});
    }

    // Connect to database - MSSQL uses custom backend, others use provider
    let providerConnectionId: string | undefined;
    let mssqlConnectionId: string | undefined;

    if (connection.type === "mssql") {
      // MSSQL uses custom Tauri backend (only available in desktop)
      if (!isTauri()) {
        throw new Error("MSSQL connections are only available in the desktop app");
      }
      const host = tunnelLocalPort ? "127.0.0.1" : connection.host;
      const port = tunnelLocalPort || connection.port;
      const mssqlConn = await mssqlConnect({
        host,
        port,
        database: connection.databaseName,
        username: connection.username,
        password: connection.password,
        encrypt: connection.sslMode === "require",
        trustCert: connection.sslMode !== "require",
      });
      mssqlConnectionId = mssqlConn.connectionId;
    } else if (effectiveConnectionString) {
      // Use provider for PostgreSQL, SQLite, DuckDB
      const provider = await this.getOrCreateProvider();
      providerConnectionId = await provider.connect({
        type: connection.type,
        connectionString: effectiveConnectionString,
        databaseName: connection.databaseName,
      });
    }

    // Create updated connection object to ensure Svelte reactivity sees the change
    const updatedConnection: DatabaseConnection = {
      ...existingConnection,
      providerConnectionId,
      mssqlConnectionId,
      lastConnected: new Date(),
      password: connection.password,
      tunnelLocalPort,
      sshTunnel: connection.sshTunnel,
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
    };

    // Replace the old connection with the updated one in the connections array
    this.state.connections = this.state.connections.map((c) => (c.id === connectionId ? updatedConnection : c));

    this.stateRestoration.ensureConnectionMapsExist(connectionId);

    // Fetch schemas - wrap in try-catch to handle failures gracefully
    const adapter = getAdapter(existingConnection.type);
    let schemasWithTables: SchemaTable[];
    try {
      let schemasWithTablesDbResult: unknown[];
      if (connection.type === "mssql" && mssqlConnectionId) {
        const result = await mssqlQuery(mssqlConnectionId, adapter.getSchemaQuery());
        schemasWithTablesDbResult = result.rows;
      } else if (providerConnectionId) {
        const provider = await this.getOrCreateProvider();
        schemasWithTablesDbResult = await provider.select(providerConnectionId, adapter.getSchemaQuery());
      } else {
        throw new Error("No connection established");
      }
      schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
    } catch (error) {
      // Revert: set providerConnectionId/mssqlConnectionId back to undefined on the connection
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? { ...c, providerConnectionId: undefined, mssqlConnectionId: undefined } : c
      );
      if (mssqlConnectionId) {
        await mssqlDisconnect(mssqlConnectionId).catch(() => {});
      }
      if (providerConnectionId) {
        const provider = await this.getOrCreateProvider();
        await provider.disconnect(providerConnectionId).catch(() => {});
      }
      throw new Error(`Failed to load database schema: ${error}`);
    }

    // Store tables immediately (without column metadata) so UI is responsive
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(connectionId, schemasWithTables, adapter, providerConnectionId, mssqlConnectionId);

    // Set this as the active connection (only after schema loading succeeds)
    this.setActiveForProject(connectionId, existingConnection.projectId);

    // Create initial query tab if no tabs exist for the project
    const projectId = existingConnection.projectId;
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    if (tabs.length === 0) {
      this.onCreateInitialTab();
    }

    // Persist the connection to store (password saved to keyring if enabled)
    this.persistence.persistConnection(updatedConnection, {
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      sshPassword: connection.sshPassword,
      sshKeyPassphrase: connection.sshKeyPassphrase,
    });

    return connectionId;
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
      c.id === connectionId ? updatedConnection : c
    );

    // Persist the updated connection
    this.persistence.persistConnection(updatedConnection, {
      savePassword: connection.savePassword,
      saveSshPassword: connection.saveSshPassword,
      saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
      sshPassword: connection.sshPassword,
      sshKeyPassphrase: connection.sshKeyPassphrase,
    });
  }

  /**
   * Test a connection without persisting it.
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

    let mssqlTestConnectionId: string | undefined;
    let providerTestConnectionId: string | undefined;

    try {
      // MSSQL uses custom backend, others use provider
      if (connection.type === "mssql") {
        if (!isTauri()) {
          throw new Error("MSSQL connections are only available in the desktop app");
        }
        const host = tunnelLocalPort ? "127.0.0.1" : connection.host;
        const port = tunnelLocalPort || connection.port;
        const mssqlConn = await mssqlConnect({
          host,
          port,
          database: connection.databaseName,
          username: connection.username,
          password: connection.password,
          encrypt: connection.sslMode === "require",
          trustCert: connection.sslMode !== "require",
        });
        mssqlTestConnectionId = mssqlConn.connectionId;
        // Close the test connection immediately
        await mssqlDisconnect(mssqlTestConnectionId);
      } else if (effectiveConnectionString) {
        // Use provider for PostgreSQL, SQLite, DuckDB
        const provider = await this.getOrCreateProvider();
        providerTestConnectionId = await provider.connect({
          type: connection.type,
          connectionString: effectiveConnectionString,
          databaseName: connection.databaseName,
        });
        // Close the test connection immediately
        await provider.disconnect(providerTestConnectionId);
      }
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
  remove(id: string): void {
    // Prevent deletion of demo connection in demo mode
    if (isDemo() && id === "demo-connection") {
      return;
    }

    const connection = this.state.connections.find((c) => c.id === id);

    // Close provider connection if exists
    if (connection?.providerConnectionId) {
      this.getOrCreateProvider().then(provider => {
        provider.disconnect(connection.providerConnectionId!).catch(console.error);
      });
    }

    // Close MSSQL connection if exists
    if (connection?.mssqlConnectionId) {
      mssqlDisconnect(connection.mssqlConnectionId).catch(console.error);
    }

    // Close SSH tunnel if exists
    const tunnelId = this.tunnelIds.get(id);
    if (tunnelId) {
      closeSshTunnel(tunnelId).catch(console.error);
      this.tunnelIds.delete(id);
    }

    // Remove from persistence (both connection and its data)
    this.persistence.removePersistedConnection(id);
    this.state.connections = this.state.connections.filter((c) => c.id !== id);
    this.stateRestoration.cleanupConnectionMaps(id);

    // If this was the active connection for its project, switch to another
    if (connection && this.state.activeConnectionIdByProject[connection.projectId] === id) {
      const nextConnection = this.state.connections.find(
        (c) => c.projectId === connection.projectId && (c.providerConnectionId || c.mssqlConnectionId)
      );
      this.setActiveForProject(nextConnection?.id || "", connection.projectId);
    }
  }

  /**
   * Set the active connection for the current project.
   */
  setActive(id: string): void {
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection) {
      this.setActiveForProject(id, connection.projectId);
    }
  }

  /**
   * Set the active connection for a specific project.
   */
  setActiveForProject(connectionId: string, projectId: string): void {
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
    const existingIndex = this.state.connections.findIndex(c => c.id === connectionId);
    if (existingIndex >= 0) {
      // Update existing connection with providerConnectionId
      this.state.connections = this.state.connections.map(c =>
        c.id === connectionId ? newConnection : c
      );
    } else {
      // Add new connection
      this.state.connections = [...this.state.connections, newConnection];
    }

    this.stateRestoration.initializeConnectionMaps(connectionId);

    // Load schema
    const adapter = getAdapter("duckdb");
    const provider = await this.getOrCreateProvider();
    const schemasWithTablesDbResult = await provider.select(providerConnectionId, adapter.getSchemaQuery());
    const schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);

    // Set active connection
    this.setActiveForProject(connectionId, projectId);

    // Store tables
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously
    this.onSchemaLoaded(connectionId, schemasWithTables, adapter, providerConnectionId);

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

    // SQLite doesn't require passwords, always auto-reconnect
    if (connection.type === "sqlite") {
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
      } catch (error) {
        console.warn("Auto-reconnect failed for SQLite:", error);
        return false;
      }
    }

    // For other databases, check if password is saved
    if (!connection.savePassword) {
      return false;
    }

    const keyring = getKeyringService();
    if (!keyring.isAvailable()) {
      return false;
    }

    try {
      // Load credentials from keyring
      const password = await keyring.getDbPassword(connectionId);
      if (!password) {
        return false;
      }

      // Load SSH credentials if needed
      let sshPassword: string | undefined;
      let sshKeyPassphrase: string | undefined;

      if (connection.sshTunnel?.enabled) {
        if (connection.saveSshPassword) {
          sshPassword = (await keyring.getSshPassword(connectionId)) || undefined;
        }
        if (connection.saveSshKeyPassphrase) {
          sshKeyPassphrase = (await keyring.getSshKeyPassphrase(connectionId)) || undefined;
        }

        // If SSH is enabled but credentials not saved, we can't auto-reconnect
        if (connection.sshTunnel.authMethod === "password" && !sshPassword) {
          return false;
        }
        if (connection.sshTunnel.authMethod === "key" && !connection.sshTunnel.keyPath) {
          // SSH key auth requires keyPath to be stored
          return false;
        }
      }

      // Attempt reconnection
      await this.reconnect(connectionId, {
        name: connection.name,
        type: connection.type,
        host: connection.host,
        port: connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        password,
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

      return true;
    } catch (error) {
      console.warn("Auto-reconnect failed:", error);
      return false;
    }
  }

  /**
   * Toggle connection state (disconnect if connected).
   */
  toggle(id: string): void {
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection) {
      const wasConnected = !!connection.providerConnectionId || !!connection.mssqlConnectionId;

      // Disconnect provider connection if connected
      if (connection.providerConnectionId) {
        this.getOrCreateProvider().then(provider => {
          provider.disconnect(connection.providerConnectionId!).catch(console.error);
        });
        connection.providerConnectionId = undefined;
      }

      // Disconnect MSSQL if connected
      if (connection.mssqlConnectionId) {
        mssqlDisconnect(connection.mssqlConnectionId).catch(console.error);
        connection.mssqlConnectionId = undefined;
      }

      if (wasConnected) {
        // If disconnecting the active connection for its project, switch to another connected one
        if (this.state.activeConnectionIdByProject[connection.projectId] === id) {
          const nextConnection = this.state.connections.find(
            (c) => c.projectId === connection.projectId && (c.providerConnectionId || c.mssqlConnectionId) && c.id !== id
          );
          this.setActiveForProject(nextConnection?.id || "", connection.projectId);
        }
      }
    }
  }
}
