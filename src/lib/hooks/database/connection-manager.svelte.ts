import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getEngineClient, TsEngineClient, type EngineClient } from "$lib/engine";
import {
  getCoreClient,
  withHostKeyPrompt,
  type ConnectionClosedEvent,
  type SshServer,
} from "$lib/core";
import type { ConnectRequest, ProviderRegistry } from "$lib/providers";
import type { ConnectionForm } from "$lib/types/generated/ConnectionForm";
import type { SuppliedSecrets } from "$lib/types/generated/SuppliedSecrets";
import { isDemo, isWeb } from "$lib/utils/environment";
import { storedConnectionString } from "$lib/utils/connection-string-rules";
import {
  assertDatabaseTypeAvailable,
  databaseTypeUnavailableMessage,
  isDatabaseTypeAvailable,
  isFeatureEnabled,
} from "$lib/features";
import { getKeyringService } from "$lib/services/keyring";
import { VaultCancelledError } from "$lib/services/vault/vault-state.svelte";
import { SvelteSet } from "svelte/reactivity";
import type { SharedRepoManager } from "./shared-repo-manager.svelte.js";
import { log } from "$lib/utils/logger";

export type ConnectionInput = Omit<DatabaseConnection, "id" | "projectId" | "labelIds"> & {
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
 * The connection form Core connects (`{type:"form",form}`): the wizard's or
 * reconnect tab's fields as typed, the string included (`""` means "build it
 * from the fields"), without its three secrets. An empty SSL mode is the
 * wizard's "Default" and is left out.
 */
export function toConnectionForm(connection: ConnectionInput): ConnectionForm {
  const tunnel = connection.sshTunnel;
  return {
    name: connection.name,
    type: connection.type,
    host: connection.host ?? "",
    port: connection.port ?? 0,
    databaseName: connection.databaseName ?? "",
    username: connection.username ?? "",
    ...(connection.sslMode ? { sslMode: connection.sslMode } : {}),
    connectionString: connection.connectionString ?? "",
    sshEnabled: tunnel?.enabled ?? false,
    sshHost: tunnel?.host ?? "",
    sshPort: tunnel?.port ?? 22,
    sshUsername: tunnel?.username ?? "",
    sshAuthMethod: tunnel?.authMethod ?? "password",
    sshKeyPath: connection.sshKeyPath || tunnel?.keyPath || "",
    savePassword: connection.savePassword ?? false,
    saveSshPassword: connection.saveSshPassword ?? false,
    saveSshKeyPassphrase: connection.saveSshKeyPassphrase ?? false,
  };
}

/** Only the secrets that are there: an empty one isn't supplied. */
function secretsOf(secrets: SuppliedSecrets): SuppliedSecrets | undefined {
  const present: SuppliedSecrets = {};
  if (secrets.db) present.db = secrets.db;
  if (secrets.ssh) present.ssh = secrets.ssh;
  if (secrets.sshKey) present.sshKey = secrets.sshKey;
  return Object.keys(present).length > 0 ? present : undefined;
}

/** `connect`/`test` for a form: its fields, the secrets typed into it, `createIfMissing`. */
export function formConnectRequest(connection: ConnectionInput): ConnectRequest {
  const secrets = secretsOf({
    db: connection.password,
    ssh: connection.sshPassword,
    sshKey: connection.sshKeyPassphrase,
  });
  return {
    target: { type: "form", form: toConnectionForm(connection) },
    ...(secrets ? { secrets } : {}),
    ...(connection.createIfMissing ? { createIfMissing: true } : {}),
  };
}

/** The SSH server the host-key prompt names. */
function sshServerOf(connection: Pick<DatabaseConnection, "sshTunnel">): SshServer {
  return { host: connection.sshTunnel?.host ?? "", port: connection.sshTunnel?.port ?? 22 };
}

/** What a `connectionClosed` event's toast says. */
function connectionClosedMessage(name: string, event: ConnectionClosedEvent): string {
  switch (event.code) {
    case "WORKSPACE_EVICTED":
      return m.connection_closed_evicted({ name });
    case "CONNECTION_CLOSED":
      return m.connection_closed_lost({ name });
    case "TUNNEL_CLOSED":
      return m.connection_closed_tunnel({ name });
    default:
      return m.connection_closed_other({ name, message: event.message });
  }
}

/**
 * Manages database connections: add, reconnect, remove, test. Core opens and
 * closes them (SSH tunnels included); this keeps the view of them and loads
 * their schemas.
 */
export class ConnectionManager {
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
      client: EngineClient,
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

      // Register saved connections without waiting for the OS keychain. A
      // keychain read can take seconds on macOS and Core reads saved desktop
      // secrets itself when connecting. Web reads the vault on demand in
      // heldSecrets, so neither target needs a startup secret fetch.
      const connectionEntries = persistedConnections.map((persisted) => {
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
          password: "",
          sslMode: persisted.sslMode,
          connectionString: persisted.connectionString,
          lastConnected: persisted.lastConnected ? new Date(persisted.lastConnected) : undefined,
          // A stored JSON `null` loads as `null`.
          sshTunnel: persisted.sshTunnel ?? undefined,
          savePassword: persisted.savePassword,
          saveSshPassword: persisted.saveSshPassword,
          saveSshKeyPassphrase: persisted.saveSshKeyPassphrase,
          projectId: persisted.projectId || DEFAULT_PROJECT_ID,
          labelIds: persisted.labelIds || [],
          isLocalOnly: persisted.isLocalOnly,
          sharedConnectionId: persisted.sharedConnectionId,
          // Every field a save writes back must be carried over here, or
          // the first save of this object (the migration below, a label
          // change) would clear it.
          aiShareSchema: persisted.aiShareSchema,
          aiShareData: persisted.aiShareData,
          activeAIProviderId: persisted.activeAIProviderId,
          activeAIModel: persisted.activeAIModel,
        };
        return connection;
      });

      // Rows written before phase 5a hold the string the old builder rebuilt
      // from their fields. Core would connect with it and ignore the fields,
      // so drop it, and save the row once so a saved connect (which Core
      // reads from storage) sees the same.
      const legacy = connectionEntries.filter(
        (c) => c.connectionString && !storedConnectionString(c),
      );
      for (const c of legacy) c.connectionString = undefined;
      await Promise.all(
        legacy.map((c) =>
          this.persistence
            .persistConnection(c)
            .catch((e) => void log.warn(`Couldn't drop the old string of ${c.id}:`, e)),
        ),
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
   * Refuse an SSH connection where this build has no SSH (web, demo). The
   * wizard hides the SSH form there, but a connection imported from a
   * desktop install can still have a tunnel; Core refuses it too.
   */
  private assertSshAvailable(connection: Pick<DatabaseConnection, "sshTunnel">): void {
    if (connection.sshTunnel?.enabled && !isFeatureEnabled("sshTunnels")) {
      throw new Error("SSH tunnels are not available in this build");
    }
  }

  /** `provider.connect`, with the SSH host-key prompt and its retry. */
  private async connectCore(
    type: DatabaseConnection["type"],
    request: ConnectRequest,
    ssh: SshServer,
  ): Promise<string> {
    const provider = await this.providers.getForType(type);
    return withHostKeyPrompt(
      (trustHostKey) => provider.connect(trustHostKey ? { ...request, trustHostKey } : request),
      ssh,
    );
  }

  /**
   * Listen for connections Core closed without being asked (an evicted web
   * session, a lost connection): each is shown as disconnected. Call once
   * per page; returns the unsubscribe.
   */
  listenForCoreEvents(): () => void {
    return getCoreClient().events((event) => this.handleConnectionClosed(event));
  }

  /** Mark the connection `event` names as disconnected, and say why. */
  handleConnectionClosed(event: ConnectionClosedEvent): void {
    const connection = this.state.connections.find(
      (c) => c.providerConnectionId === event.connectionId,
    );
    if (!connection) return;
    void log.warn(`Connection closed by Core (${event.code}): ${connection.id}`);
    this.markDisconnected(connection);
    errorToast(connectionClosedMessage(connection.name, event));
  }

  /**
   * Add a new database connection.
   */
  async add(connection: ConnectionInput): Promise<string> {
    void log.info(`Adding connection: type=${connection.type}`);
    // SQLite and DuckDB on web (Decision 11b): refuse before anything runs.
    assertDatabaseTypeAvailable(connection.type);
    this.assertSshAvailable(connection);
    const connectionId = `conn-${crypto.randomUUID()}`;
    this.connectingIds.add(connectionId);

    try {
      const providerConnectionId = await this.connectCore(
        connection.type,
        formConnectRequest(connection),
        sshServerOf(connection),
      );

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
        providerConnectionId,
      };

      if (!this.state.connections.find((c) => c.id === newConnection.id)) {
        this.state.connections.push(newConnection);
      }
      this.appendToOrder(projectId, newConnection.id);

      this.stateRestoration.initializeConnectionMaps(newConnection.id);

      // Load schema - wrap in try-catch to handle failures gracefully
      let client: EngineClient;
      let schemasWithTables: SchemaTable[];
      try {
        client = getEngineClient(newConnection, this.state);
        schemasWithTables = await client.schemaTables();
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
      void this.onSchemaLoaded(newConnection.id, schemasWithTables, client);

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
   * Reconnect to an existing connection from the reconnect tab's form.
   */
  async reconnect(connectionId: string, connection: ConnectionInput): Promise<string> {
    void log.info(`Reconnecting: ${connectionId}`);
    const existingConnection = this.state.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }
    // A SQLite or DuckDB connection saved on desktop can reach a web
    // workspace; it fails here with the reason instead of at the server.
    assertDatabaseTypeAvailable(connection.type);
    this.assertSshAvailable(connection);

    this.connectingIds.add(connectionId);
    try {
      await this.connectExisting(
        existingConnection,
        formConnectRequest(connection),
        sshServerOf(connection),
        {
          // What was connected is what the row shows and stores.
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
        },
        {
          savePassword: connection.savePassword,
          saveSshPassword: connection.saveSshPassword,
          saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
          sshPassword: connection.sshPassword,
          sshKeyPassphrase: connection.sshKeyPassphrase,
        },
      );
      return connectionId;
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  /**
   * Connect a listed connection again: drop its old Core connection, connect
   * `request`, load its schema, then save the row with `changes` (and
   * `secrets`, when the form had them).
   */
  private async connectExisting(
    existingConnection: DatabaseConnection,
    request: ConnectRequest,
    ssh: SshServer,
    changes: Partial<DatabaseConnection>,
    secrets?: Parameters<PersistenceManager["persistConnection"]>[1],
  ): Promise<void> {
    const connectionId = existingConnection.id;
    // Disconnect the existing connection first and mark it disconnected (Core
    // closes its tunnel with it). If the new connect fails, the connection
    // then reads as disconnected instead of pointing at a dead connection.
    if (existingConnection.providerConnectionId) {
      const oldProvider = await this.providers.getForType(existingConnection.type);
      await oldProvider.disconnect(existingConnection.providerConnectionId).catch(() => {});
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? { ...c, providerConnectionId: undefined } : c,
      );
    }

    const providerConnectionId = await this.connectCore(existingConnection.type, request, ssh);

    // Create updated connection object to ensure Svelte reactivity sees the change
    const current = this.state.connections.find((c) => c.id === connectionId) ?? existingConnection;
    const updatedConnection: DatabaseConnection = {
      ...current,
      ...changes,
      providerConnectionId,
      lastConnected: new Date(),
    };

    // Replace the old connection with the updated one in the connections array
    this.state.connections = this.state.connections.map((c) =>
      c.id === connectionId ? updatedConnection : c,
    );

    this.stateRestoration.ensureConnectionMapsExist(connectionId);

    // Fetch schemas - wrap in try-catch to handle failures gracefully
    let client: EngineClient;
    let schemasWithTables: SchemaTable[];
    try {
      client = getEngineClient(updatedConnection, this.state);
      schemasWithTables = await client.schemaTables();
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
    void this.onSchemaLoaded(connectionId, schemasWithTables, client);

    // Set this as the active connection (only after schema loading succeeds)
    this.setActiveForProject(connectionId, existingConnection.projectId);

    // Create initial query tab if no tabs exist for the project
    const projectId = existingConnection.projectId;
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    if (tabs.length === 0) {
      this.onCreateInitialTab();
    }

    // Persist the connection (and, from a form, its secrets under its flags)
    await this.persistence.persistConnection(updatedConnection, secrets);
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
      // undefined means "follow the global AI setting", so copy it as is.
      aiShareSchema: connection.aiShareSchema,
      aiShareData: connection.aiShareData,
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
   * Test a connection without persisting it: Core connects the form and
   * closes it again (with its tunnel). Throws on failure so callers can
   * display the error inline.
   */
  async test(connection: ConnectionInput): Promise<void> {
    assertDatabaseTypeAvailable(connection.type);
    this.assertSshAvailable(connection);
    const provider = await this.providers.getForType(connection.type);
    const request = formConnectRequest(connection);
    await withHostKeyPrompt(
      (trustHostKey) => provider.test(trustHostKey ? { ...request, trustHostKey } : request),
      sshServerOf(connection),
    );
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

    // Close the Core connection (and its SSH tunnel) if there is one
    if (connection?.providerConnectionId) {
      await this.providers.getForType(connection.type).then((provider) => {
        provider.disconnect(connection.providerConnectionId!).catch((e) => void log.error(e));
      });
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
    const persisted = this.state.connections.find((c) => c.id === connectionId);
    const projectId = persisted?.projectId ?? (this.state.activeProjectId || DEFAULT_PROJECT_ID);

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
    // otherwise add new connection. A persisted row keeps the user's edits
    // (labels, AI model) and its project.
    const existing = persisted;
    const connection: DatabaseConnection = existing
      ? {
          ...newConnection,
          labelIds: existing.labelIds,
          activeAIProviderId: existing.activeAIProviderId,
          activeAIModel: existing.activeAIModel,
          aiShareSchema: existing.aiShareSchema,
          aiShareData: existing.aiShareData,
        }
      : newConnection;
    this.state.connections = existing
      ? this.state.connections.map((c) => (c.id === connectionId ? connection : c))
      : [...this.state.connections, connection];

    // Save the row (an upsert, so every load is safe): history, AI chats and
    // the other rows that reference this connection need it, now that the
    // demo's foreign keys hold.
    await this.persistence.persistConnection(connection);

    this.stateRestoration.initializeConnectionMaps(connectionId);
    this.appendToOrder(projectId, connectionId);

    // Load schema
    const client = new TsEngineClient({
      type: "duckdb",
      getConnectionId: () =>
        this.state.connections.find((c) => c.id === connectionId)?.providerConnectionId,
      getProvider: () => this.providers.getOrCreateDuckDB(),
    });
    const schemasWithTables = await client.schemaTables();

    // Set active connection
    this.setActiveForProject(connectionId, projectId);

    // Store tables
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously
    void this.onSchemaLoaded(connectionId, schemasWithTables, client);

    // Create initial query tab
    this.onCreateInitialTab();

    return connectionId;
  }

  /**
   * Connect a saved connection again without showing a form: Core reads the
   * saved row (`{type:"saved",id}`) with the secrets this page holds, and
   * the secret store on desktop. Returns false, having said nothing, when
   * that isn't enough (Core answers `CREDENTIALS_REQUIRED` where the old
   * give-up rules did) or the connect fails; callers then open the
   * connection's tab.
   */
  async autoReconnect(connectionId: string): Promise<boolean> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) {
      return false;
    }

    // Nothing to retry: say why, and let the caller open the connection's
    // tab (where Connect gives the same message).
    if (!isDatabaseTypeAvailable(connection.type)) {
      errorToast(databaseTypeUnavailableMessage(connection.type));
      return false;
    }

    void log.info(`Auto-reconnect attempt: ${connectionId}`);
    this.connectingIds.add(connectionId);
    try {
      this.assertSshAvailable(connection);
      const secrets = await this.heldSecrets(connection);
      await this.connectExisting(
        connection,
        { target: { type: "saved", id: connectionId }, ...(secrets ? { secrets } : {}) },
        sshServerOf(connection),
        {},
      );
      void log.info(`Auto-reconnect successful: ${connectionId}`);
      return true;
    } catch (error) {
      void log.warn(`Auto-reconnect failed: ${connectionId}`, error);
      return false;
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  /**
   * The secrets this page holds for a saved connection, for `autoReconnect`.
   * A password it already has (typed this session, or read from the
   * keychain or vault at startup) is always sent: a supplied secret wins
   * over the store. Otherwise:
   * - Web has no secret store in Core, so the vault's password when the row
   *   saves it (unlocking the vault if needed). Web has no SSH.
   * - Desktop sends nothing more: Core reads the keychain under the row's
   *   flags.
   */
  private async heldSecrets(connection: DatabaseConnection): Promise<SuppliedSecrets | undefined> {
    let db = connection.password || undefined;
    if (!db && isWeb() && connection.savePassword) {
      const keyring = getKeyringService();
      if (keyring.isAvailable()) {
        try {
          db = (await keyring.getDbPassword(connection.id)) || undefined;
        } catch (error) {
          // A cancelled unlock, say: Core then tries without it.
          void log.warn("Reading the saved password from the vault failed:", error);
        }
      }
    }
    return secretsOf({ db });
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

    const client = getEngineClient(connection, this.state);
    const schemasWithTables = await client.schemaTables();

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
    await this.onSchemaLoaded(connectionId, mergedSchemas, client);
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
    if (!connection?.providerConnectionId) return;

    // Disconnect the Core connection (Core closes its SSH tunnel with it).
    await this.providers.getForType(connection.type).then((provider) => {
      provider.disconnect(connection.providerConnectionId!).catch((e) => void log.error(e));
    });
    this.markDisconnected(connection);
  }

  /**
   * Show a connection as disconnected: clear its Core id, close its schema
   * tabs, and make another connected one active for its project if it was
   * the active one. For a toggle and for a connection Core closed itself.
   */
  private markDisconnected(connection: DatabaseConnection): void {
    const id = connection.id;
    this.state.connections = this.state.connections.map((c) =>
      c.id === id ? { ...c, providerConnectionId: undefined } : c,
    );
    void log.info(`Connection disconnected: ${id}`);

    // Remove schema tabs belonging to the disconnected connection
    const projectId = connection.projectId;
    const schemaTabs = this.state.schemaTabsByProject[projectId] ?? [];
    const removedTabIds = new Set(schemaTabs.filter((t) => t.connectionId === id).map((t) => t.id));
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

    // If it was the project's active connection, switch to another connected one
    if (this.state.activeConnectionIdByProject[projectId] === id) {
      const nextConnection = this.state.connections.find(
        (c) => c.projectId === projectId && !!c.providerConnectionId && c.id !== id,
      );
      this.setActiveForProject(nextConnection?.id ?? null, projectId);
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
