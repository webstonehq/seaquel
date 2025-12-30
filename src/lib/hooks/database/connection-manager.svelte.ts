import Database from "@tauri-apps/plugin-sql";
import { toast } from "svelte-sonner";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";
import { setMapValue } from "./map-utils.js";

type ConnectionInput = Omit<DatabaseConnection, "id"> & {
  sshPassword?: string;
  sshKeyPath?: string;
  sshKeyPassphrase?: string;
};

/**
 * Manages database connections: add, reconnect, remove, test.
 * Handles SSH tunnel lifecycle and schema loading.
 */
export class ConnectionManager {
  // Map connection IDs to their SSH tunnel IDs for cleanup
  private tunnelIds = new Map<string, string>();

  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager,
    private stateRestoration: StateRestorationManager,
    private tabOrdering: TabOrderingManager,
    private onSchemaLoaded: (connectionId: string, schemas: SchemaTable[], adapter: DatabaseAdapter, database: Database) => void,
    private onCreateInitialTab: () => void
  ) {}

  /**
   * Initialize persisted connections on app startup.
   */
  async initializePersistedConnections(): Promise<void> {
    try {
      const persistedConnections = await this.persistence.loadPersistedConnections();

      for (const persisted of persistedConnections) {
        // Load the connection without connecting (password is not persisted)
        const connection: DatabaseConnection = {
          id: persisted.id,
          name: persisted.name,
          type: persisted.type,
          host: persisted.host,
          port: persisted.port,
          databaseName: persisted.databaseName,
          username: persisted.username,
          password: "", // Password not persisted - user must enter it
          sslMode: persisted.sslMode,
          connectionString: persisted.connectionString,
          lastConnected: persisted.lastConnected ? new Date(persisted.lastConnected) : undefined,
          sshTunnel: persisted.sshTunnel,
          // database is undefined - user needs to provide password to connect
        };
        this.state.connections.push(connection);
        this.stateRestoration.initializeConnectionMaps(connection.id);

        // Pre-load saved queries and history (doesn't require active DB connection)
        const persistedState = await this.persistence.loadPersistedConnectionState(connection.id);
        if (persistedState) {
          this.stateRestoration.restoreSavedQueries(connection.id, persistedState.savedQueries);
          this.stateRestoration.restoreQueryHistory(connection.id, persistedState.queryHistory);
        }
      }
    } catch (error) {
      console.error("Failed to load persisted connections:", error);
      // Silently fail - app will continue with no persisted connections
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
    const connectionId =
      connection.type === "sqlite"
        ? `conn-sqlite-${connection.databaseName}`
        : `conn-${connection.host}-${connection.port}`;

    const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
      connection,
      {
        sshPassword: connection.sshPassword,
        sshKeyPath: connection.sshKeyPath,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      },
      connectionId
    );

    const newConnection: DatabaseConnection = {
      ...connection,
      id: connectionId,
      lastConnected: new Date(),
      tunnelLocalPort,
      database: effectiveConnectionString ? await Database.load(effectiveConnectionString) : undefined,
    };

    if (!this.state.connections.find((c) => c.id === newConnection.id)) {
      this.state.connections.push(newConnection);
    }

    this.state.activeConnectionId = newConnection.id;
    this.stateRestoration.initializeConnectionMaps(newConnection.id);

    const adapter = getAdapter(newConnection.type);
    const schemasWithTablesDbResult = await newConnection.database!.select(adapter.getSchemaQuery());

    const schemasWithTables: SchemaTable[] = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);

    // Store tables immediately (without column metadata) so UI is responsive
    const newSchemas = new Map(this.state.schemas);
    newSchemas.set(newConnection.id, schemasWithTables);
    this.state.schemas = newSchemas;

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(newConnection.id, schemasWithTables, adapter, newConnection.database!);

    // Create initial query tab for new connection
    this.onCreateInitialTab();

    // Persist the connection to store (without password)
    this.persistence.persistConnection(newConnection);

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

    // Update the existing connection with the new database connection
    const database = effectiveConnectionString ? await Database.load(effectiveConnectionString) : undefined;

    // Create updated connection object to ensure Svelte reactivity sees the change
    const updatedConnection: DatabaseConnection = {
      ...existingConnection,
      database,
      lastConnected: new Date(),
      password: connection.password,
      tunnelLocalPort,
      sshTunnel: connection.sshTunnel,
    };

    // Replace the old connection with the updated one in the connections array
    this.state.connections = this.state.connections.map((c) => (c.id === connectionId ? updatedConnection : c));

    this.stateRestoration.ensureConnectionMapsExist(connectionId);

    // Fetch schemas
    const adapter = getAdapter(existingConnection.type);
    const schemasWithTablesDbResult = await database!.select(adapter.getSchemaQuery());

    const schemasWithTables: SchemaTable[] = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);

    // Store tables immediately (without column metadata) so UI is responsive
    const newSchemas = new Map(this.state.schemas);
    newSchemas.set(connectionId, schemasWithTables);
    this.state.schemas = newSchemas;

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(connectionId, schemasWithTables, adapter, database!);

    // Set this as the active connection
    this.state.activeConnectionId = connectionId;

    // Try to restore tabs from persisted state
    const hasRestoredTabs = await this.stateRestoration.restoreConnectionTabs(connectionId);

    // Create initial query tab if no tabs were restored
    if (!hasRestoredTabs) {
      const tabs = this.state.queryTabsByConnection.get(connectionId) || [];
      if (tabs.length === 0) {
        this.onCreateInitialTab();
      }
    }

    // Persist the connection to store (without password for security)
    this.persistence.persistConnection(updatedConnection);

    return connectionId;
  }

  /**
   * Test a connection without persisting it.
   */
  async test(connection: ConnectionInput): Promise<void> {
    let effectiveConnectionString = connection.connectionString;
    let tunnelId: string | undefined;

    // Establish SSH tunnel if enabled
    if (connection.sshTunnel?.enabled) {
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

      // Build new connection string using tunnel
      if (effectiveConnectionString) {
        const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
        url.hostname = "127.0.0.1";
        url.port = String(tunnelResult.localPort);
        effectiveConnectionString = url.toString();
      }
    }

    try {
      // Try to connect to the database
      const database = await Database.load(effectiveConnectionString!);
      // Close the test connection immediately
      await database.close();
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
    // Close SSH tunnel if exists
    const tunnelId = this.tunnelIds.get(id);
    if (tunnelId) {
      closeSshTunnel(tunnelId).catch(console.error);
      this.tunnelIds.delete(id);
    }

    // Remove from persistence (both connection and its state)
    this.persistence.removePersistedConnection(id);
    this.persistence.removePersistedConnectionState(id);
    this.state.connections = this.state.connections.filter((c) => c.id !== id);
    this.stateRestoration.cleanupConnectionMaps(id);

    if (this.state.activeConnectionId === id) {
      const nextConnection = this.state.connections.find((c) => c.database);
      this.state.activeConnectionId = nextConnection?.id || null;
    }
  }

  /**
   * Set the active connection by ID.
   */
  setActive(id: string): void {
    this.state.activeConnectionId = id;
  }

  /**
   * Toggle connection state (disconnect if connected).
   */
  toggle(id: string): void {
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection) {
      const wasConnected = !!connection.database;
      connection.database = undefined;
      if (wasConnected) {
        // If disconnecting the active connection, switch to another connected one
        if (this.state.activeConnectionId === id) {
          const nextConnection = this.state.connections.find((c) => c.database && c.id !== id);
          this.state.activeConnectionId = nextConnection?.id || null;
        }
      }
    }
  }
}
