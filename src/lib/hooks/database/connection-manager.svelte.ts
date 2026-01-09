import Database from "@tauri-apps/plugin-sql";
import { toast } from "svelte-sonner";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";
import { mssqlConnect, mssqlDisconnect, mssqlQuery } from "$lib/services/mssql";

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
    private onSchemaLoaded: (connectionId: string, schemas: SchemaTable[], adapter: DatabaseAdapter, database: Database | undefined, mssqlConnectionId?: string) => void,
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

    // Connect to database - MSSQL uses custom backend, others use tauri-plugin-sql
    let database: Database | undefined;
    let mssqlConnectionId: string | undefined;

    if (connection.type === "mssql") {
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
    } else {
      database = effectiveConnectionString ? await Database.load(effectiveConnectionString) : undefined;
    }

    const newConnection: DatabaseConnection = {
      ...connection,
      id: connectionId,
      lastConnected: new Date(),
      tunnelLocalPort,
      database,
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
      } else {
        schemasWithTablesDbResult = await newConnection.database!.select(adapter.getSchemaQuery());
      }
      schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
    } catch (error) {
      // Cleanup: remove the connection we just added
      this.state.connections = this.state.connections.filter((c) => c.id !== newConnection.id);
      this.stateRestoration.cleanupConnectionMaps(newConnection.id);
      if (mssqlConnectionId) {
        await mssqlDisconnect(mssqlConnectionId).catch(() => {});
      }
      throw new Error(`Failed to load database schema: ${error}`);
    }

    // Only set active connection after schema loading succeeds
    this.state.activeConnectionId = newConnection.id;

    // Store tables immediately (without column metadata) so UI is responsive
    this.state.schemas = {
      ...this.state.schemas,
      [newConnection.id]: schemasWithTables,
    };

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(newConnection.id, schemasWithTables, adapter, newConnection.database, newConnection.mssqlConnectionId);

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

    // Close existing MSSQL connection if any
    if (existingConnection.mssqlConnectionId) {
      await mssqlDisconnect(existingConnection.mssqlConnectionId).catch(() => {});
    }

    // Connect to database - MSSQL uses custom backend, others use tauri-plugin-sql
    let database: Database | undefined;
    let mssqlConnectionId: string | undefined;

    if (connection.type === "mssql") {
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
    } else {
      database = effectiveConnectionString ? await Database.load(effectiveConnectionString) : undefined;
    }

    // Create updated connection object to ensure Svelte reactivity sees the change
    const updatedConnection: DatabaseConnection = {
      ...existingConnection,
      database,
      mssqlConnectionId,
      lastConnected: new Date(),
      password: connection.password,
      tunnelLocalPort,
      sshTunnel: connection.sshTunnel,
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
      } else {
        schemasWithTablesDbResult = await database!.select(adapter.getSchemaQuery());
      }
      schemasWithTables = adapter.parseSchemaResult(schemasWithTablesDbResult as unknown[]);
    } catch (error) {
      // Revert: set database/mssqlConnectionId back to undefined on the connection
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? { ...c, database: undefined, mssqlConnectionId: undefined } : c
      );
      if (mssqlConnectionId) {
        await mssqlDisconnect(mssqlConnectionId).catch(() => {});
      }
      throw new Error(`Failed to load database schema: ${error}`);
    }

    // Store tables immediately (without column metadata) so UI is responsive
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously in the background
    this.onSchemaLoaded(connectionId, schemasWithTables, adapter, database, mssqlConnectionId);

    // Set this as the active connection (only after schema loading succeeds)
    this.state.activeConnectionId = connectionId;

    // Try to restore tabs from persisted state
    const hasRestoredTabs = await this.stateRestoration.restoreConnectionTabs(connectionId);

    // Create initial query tab if no tabs were restored
    if (!hasRestoredTabs) {
      const tabs = this.state.queryTabsByConnection[connectionId] ?? [];
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
    let tunnelLocalPort: number | undefined;

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

    try {
      // MSSQL uses custom backend, others use tauri-plugin-sql
      if (connection.type === "mssql") {
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
      } else {
        // Try to connect to the database
        const database = await Database.load(effectiveConnectionString!);
        // Close the test connection immediately
        await database.close();
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
    // Close MSSQL connection if exists
    const connection = this.state.connections.find((c) => c.id === id);
    if (connection?.mssqlConnectionId) {
      mssqlDisconnect(connection.mssqlConnectionId).catch(console.error);
    }

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
      const nextConnection = this.state.connections.find((c) => c.database || c.mssqlConnectionId);
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
      const wasConnected = !!connection.database || !!connection.mssqlConnectionId;

      // Disconnect MSSQL if connected
      if (connection.mssqlConnectionId) {
        mssqlDisconnect(connection.mssqlConnectionId).catch(console.error);
        connection.mssqlConnectionId = undefined;
      }
      connection.database = undefined;

      if (wasConnected) {
        // If disconnecting the active connection, switch to another connected one
        if (this.state.activeConnectionId === id) {
          const nextConnection = this.state.connections.find((c) => (c.database || c.mssqlConnectionId) && c.id !== id);
          this.state.activeConnectionId = nextConnection?.id || null;
        }
      }
    }
  }
}
