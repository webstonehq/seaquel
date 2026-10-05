import { errorToast } from "$lib/utils/toast";
import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";
import { extractErrorMessage } from "$lib/errors";
import type { DatabaseConnection, SchemaTable } from "$lib/types";
import { DEFAULT_PROJECT_ID } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { WindowStateManager } from "./window-state.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getEngineClient, type EngineClient } from "$lib/engine";
import { errorCode } from "$lib/core/client";
import {
  callDb,
  getCoreClient,
  onConnectionNotFound,
  withHostKeyPrompt,
  type ConnectionClosedEvent,
  type SshServer,
} from "$lib/core";
import type { ConnectRequest, ProviderRegistry } from "$lib/providers";
import type { ConnectionForm } from "$lib/types/generated/ConnectionForm";
import type { SuppliedSecrets } from "$lib/types/generated/SuppliedSecrets";
import { isDemo, isTauri, isWeb } from "$lib/utils/environment";
import {
  assertDatabaseTypeAvailable,
  databaseTypeUnavailableMessage,
  isDatabaseTypeAvailable,
  isFeatureEnabled,
} from "$lib/features";
import { getKeyringService } from "$lib/services/keyring";
import { withDuckdbHelper } from "$lib/core/duckdb-helper";
import { VaultCancelledError, getVault } from "$lib/services/vault/vault-state.svelte";
import { SvelteSet } from "svelte/reactivity";
import { log } from "$lib/utils/logger";
import { getImports, type ImportSource } from "./shared/index.js";
import { reportProjection } from "./shared/projection.js";
import {
  NEW,
  getLibrary,
  rowKey,
  type ChangeSeq,
  type ConnectionDraft,
  type ConnectionPatch,
  type SecretChanges,
  type WireConnection,
} from "./library/index.js";
import {
  connectionFromWire,
  connectionPatch,
  isEmptyPatch,
  type ConnectionFields,
} from "./library/convert.js";
import { LibraryError, libraryError } from "./library/messages.js";
import { keepSame } from "./library/same-data.js";
import { closeConnectionTabs } from "./connection-tabs-cleanup.js";
import {
  applyConnectionRow,
  bumpRevisions,
  libraryNameOf,
  patchConnection,
  refreshConnectionOrder,
  storeConnectionOrder,
  storedFieldsDiffer,
} from "./library/view.js";

/**
 * Core restarted in the demo's page (`$lib/core/browser`'s `CORE_RESTARTED`),
 * named here rather than imported so desktop and web don't bundle that module.
 */
const CORE_RESTARTED = "CORE_RESTARTED";

/** `connectionClosed` codes shown without a toast (phase 6 probe F4). */
const QUIET_CLOSE_CODES = new Set(["WINDOW_CLOSED", "CONNECTION_REPLACED"]);

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

/** A candidate the import dialog ticked: Core's key, and its name to say a failure. */
export interface ImportChoice {
  key: string;
  name: string;
}

/** What an import did: how many were saved and skipped, and each failure's reason. */
export interface ImportResult {
  imported: number;
  skipped: number;
  failures: { name: string; reason: string }[];
}

/** Whether `add` would store a secret for `connection`: the vault must be unlocked first on web. */
function hasSecretsToSave(connection: ConnectionInput): boolean {
  return (
    (!!connection.savePassword && !!connection.password) ||
    (!!connection.saveSshPassword && !!connection.sshPassword) ||
    (!!connection.saveSshKeyPassphrase && !!connection.sshKeyPassphrase)
  );
}

/** The prefix of `connectingIds`' entry for a connection `add` hasn't saved yet. */
const NEW_CONNECTION = "new-connection-";

/** A patch's fields as the page's connection holds them (`null` clears). */
function fieldsOfPatch(patch: ConnectionPatch): Partial<DatabaseConnection> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(patch)) {
    if (key !== "connected") out[key] = value === null ? undefined : value;
  }
  return out as Partial<DatabaseConnection>;
}

/** A shown connection's stored fields, as a form would send them. */
export function fieldsOf(c: DatabaseConnection): ConnectionFields {
  return {
    name: c.name,
    type: c.type,
    host: c.host,
    port: c.port,
    databaseName: c.databaseName,
    username: c.username,
    sslMode: c.sslMode,
    connectionString: c.connectionString,
    sshTunnel: c.sshTunnel,
    savePassword: c.savePassword,
    saveSshPassword: c.saveSshPassword,
    saveSshKeyPassphrase: c.saveSshKeyPassphrase,
    aiShareSchema: c.aiShareSchema,
    aiShareData: c.aiShareData,
  };
}

/**
 * The `connectionCreate` draft for a form's connection in `projectId`. An
 * empty SSL mode ("Default") or string is left out; the string is stored
 * without its secrets (Core strips them).
 */
export function connectionDraft(
  input: Omit<ConnectionInput, "password"> & {
    password?: string;
    labelIds?: string[];
    isLocalOnly?: boolean;
    sharedConnectionId?: string;
    activeAIProviderId?: string;
    activeAIModel?: string;
  },
  projectId: string,
): ConnectionDraft {
  const draft: ConnectionDraft = {
    projectId,
    name: input.name,
    type: input.type,
    host: input.host ?? "",
    port: input.port ?? 0,
    databaseName: input.databaseName ?? "",
    username: input.username ?? "",
    savePassword: !!input.savePassword,
    saveSshPassword: !!input.saveSshPassword,
    saveSshKeyPassphrase: !!input.saveSshKeyPassphrase,
    labelIds: input.labelIds ?? [],
    isLocalOnly: input.isLocalOnly ?? true,
    connected: true,
  };
  if (input.sslMode) draft.sslMode = input.sslMode;
  if (input.connectionString) draft.connectionString = input.connectionString;
  if (input.sshTunnel) draft.sshTunnel = input.sshTunnel;
  if (input.sharedConnectionId) draft.sharedConnectionId = input.sharedConnectionId;
  if (input.aiShareSchema !== undefined) draft.aiShareSchema = input.aiShareSchema;
  if (input.aiShareData !== undefined) draft.aiShareData = input.aiShareData;
  if (input.activeAIProviderId) draft.activeAIProviderId = input.activeAIProviderId;
  if (input.activeAIModel) draft.activeAIModel = input.activeAIModel;
  return draft;
}

/**
 * Desktop: the secrets a form saves with the connection, set in Core's
 * call (Decision 8). Only a typed secret whose flag is on is sent; Core
 * deletes the entries whose flag the save turns off. `undefined` when
 * there's none. Never used on web, whose vault stays in the browser.
 */
export function newSecrets(input: ConnectionInput): SecretChanges | undefined {
  const secrets: SecretChanges = {};
  if (input.savePassword && input.password) secrets.db = input.password;
  if (input.saveSshPassword && input.sshPassword) secrets.ssh = input.sshPassword;
  if (input.saveSshKeyPassphrase && input.sshKeyPassphrase) secrets.sshKey = input.sshKeyPassphrase;
  return Object.keys(secrets).length > 0 ? secrets : undefined;
}

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
  return {
    host: connection.sshTunnel?.host ?? "",
    port: connection.sshTunnel?.port ?? 22,
  };
}

/**
 * On desktop a DuckDB connect or test goes through `withDuckdbHelper` (the
 * install dialog); every other engine, and the demo's in-page DuckDB, as is.
 */
function viaDuckdbHelper<T>(
  type: DatabaseConnection["type"],
  attempt: () => Promise<T>,
  options: { interactive: boolean },
): Promise<T> {
  return type === "duckdb" && isTauri() ? withDuckdbHelper(attempt, options) : attempt();
}

/** A DuckDB database that is a file (not `:memory:`), as the form names it. */
function duckdbFile(connection: Pick<DatabaseConnection, "type" | "databaseName">): string | null {
  if (connection.type !== "duckdb") return null;
  const path = connection.databaseName?.trim() ?? "";
  return path && path !== ":memory:" ? path : null;
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
  /** The auto-reconnect and reconnect running for each connection id. */
  private readonly autoReconnects = new Map<string, Promise<boolean>>();
  private readonly reconnects = new Map<string, Promise<string>>();
  /** Connections the user disconnected while a background reconnect ran. */
  private readonly userDisconnected = new Set<string>();
  /** Connections whose running auto-reconnect is a background one. */
  private readonly backgroundIds = new Set<string>();
  /** Background attempts a caller that isn't background joined (N2). */
  private readonly upgraded = new Set<string>();
  /** Attempts that ended because the user disconnected meanwhile. */
  private readonly cancelledAttempts = new WeakSet<Promise<boolean>>();
  private pendingCount = 0;
  /** Whether the saved connections were read at startup. */
  loaded = false;

  constructor(
    private state: DatabaseState,
    private windowState: Pick<WindowStateManager, "scheduleProject">,
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
    /** The page's connections were applied or one was removed (fields may have changed). */
    private onConnectionsChanged: () => void = () => {},
  ) {}

  /**
   * Load the saved connections on app startup: one `connectionsList`, whose
   * `seq` each row is applied at. Core dropped the strings the old builder
   * rebuilt from the fields when it opened the file (a data step), so
   * nothing is saved here.
   */
  async initializePersistedConnections(): Promise<void> {
    this.loaded = false;
    try {
      const { value, seq } = await getLibrary().listConnections();
      this.loaded = true;
      this.applyConnections(value, seq, null, false);
      // Load connection data (query history, AI chats) in parallel
      await Promise.all(value.map((c) => this.stateRestoration.loadConnectionData(c.id)));
    } catch (error) {
      void log.error("Failed to load saved connections:", error);
      // The app continues with no saved connections; nothing replaces the
      // stored list, so a failed load can't lose any.
    } finally {
      this.state.connectionsLoading = false;
    }
  }

  // -------- The library's rows (phase 5d-1) --------

  /** The names the page holds, for a `NAME_TAKEN` message. */
  private nameOf = (id: string) => libraryNameOf(this.state, id);

  /** A library call's failure, worded for the user. */
  private failed(error: unknown): LibraryError {
    return libraryError(error, this.nameOf);
  }

  /** Apply one of this page's write answers for connection `row`. */
  private applyOwn(row: WireConnection, seq: ChangeSeq, keep?: Partial<DatabaseConnection>) {
    return applyConnectionRow(this.state, row, seq, keep);
  }

  /**
   * Apply a `connectionsList` taken at `seq` to the rows `ids` names (every
   * row when `null`), each only if `seq` is newer than what it shows
   * (Decision 17). A row the list lacks was deleted. `remote`: the list is
   * a refetch for another window's change, so a changed row is counted for
   * the forms editing it, and a deleted one is disconnected with a toast.
   */
  applyConnections(
    rows: readonly WireConnection[],
    seq: ChangeSeq,
    ids: readonly string[] | null,
    remote: boolean,
  ): void {
    const seqs = this.state.librarySeqs;
    const byId = new Map(rows.map((r) => [r.id, r]));
    const scope = new Set(ids ?? [...this.state.connections.map((c) => c.id), ...byId.keys()]);
    let next = [...this.state.connections];
    const added: DatabaseConnection[] = [];
    const removed: DatabaseConnection[] = [];
    const revisions: string[] = [];
    for (const id of scope) {
      const key = rowKey("connection", id);
      if (!seqs.take(key, seq)) continue;
      const row = byId.get(id);
      const current = next.find((c) => c.id === id);
      if (row) {
        // An unchanged row keeps its object (an `external` reload reads them all).
        const updated = keepSame(current, connectionFromWire(row, current));
        if (current) {
          if (remote && storedFieldsDiffer(current, updated)) revisions.push(key);
          next = next.map((c) => (c.id === id ? updated : c));
        } else {
          next.push(updated);
          added.push(updated);
        }
      } else if (current) {
        removed.push(current);
      }
    }
    this.state.connections = next;
    for (const connection of added) {
      this.stateRestoration.initializeConnectionMaps(connection.id);
      this.appendToOrder(connection.projectId, connection.id);
      if (remote) void this.stateRestoration.loadConnectionData(connection.id);
    }
    bumpRevisions(this.state, revisions);
    for (const connection of removed) this.forgetRemoved(connection, remote);
    this.onConnectionsChanged();
  }

  /**
   * Refetch the connections another window changed (`ids`, or all) and
   * apply them by the `seq` rule, after this page's own writes to them.
   */
  async refreshFromLibrary(
    ids: readonly string[] | null,
    { remote = true }: { remote?: boolean } = {},
  ): Promise<void> {
    await this.state.librarySeqs.settled("connection:");
    const { value, seq } = await getLibrary().listConnections();
    this.applyConnections(value, seq, ids, remote);
  }

  /**
   * A connection that is gone from storage (deleted in another window, or
   * with its project): close its Core connection and schema tabs, and take
   * it out of the page. `notify` says so with a toast.
   */
  forgetRemoved(connection: DatabaseConnection, notify: boolean): void {
    if (connection.providerConnectionId) {
      const coreId = connection.providerConnectionId;
      void this.providers
        .getForType(connection.type)
        .then((provider) => provider.disconnect(coreId))
        .catch((e) => void log.warn("Disconnecting a removed connection failed:", e));
      this.markDisconnected(connection);
    }
    this.forget(connection);
    if (notify) toast.info(m.library_connection_removed_elsewhere({ name: connection.name }));
  }

  /**
   * Take a connection that is gone from storage out of the page: its maps,
   * its place in the order and as the active one, and its tabs in every
   * project (local removal and another window's share this).
   */
  private forget(connection: DatabaseConnection): void {
    this.state.connections = this.state.connections.filter((c) => c.id !== connection.id);
    this.onConnectionsChanged();
    this.stateRestoration.cleanupConnectionMaps(connection.id);
    this.removeFromOrder(connection.projectId, connection.id);
    const panes = this.tabOrdering?.paneManager;
    const syncActive = panes ? (tabId: string) => panes.syncGlobalActiveState(tabId) : undefined;
    for (const projectId of closeConnectionTabs(this.state, connection.id, syncActive)) {
      this.windowState.scheduleProject(projectId);
    }
    if (this.state.activeConnectionIdByProject[connection.projectId] === connection.id) {
      const nextConnection = this.state.connections.find(
        (c) => c.projectId === connection.projectId && !!c.providerConnectionId,
      );
      this.setActiveForProject(nextConnection?.id ?? null, connection.projectId);
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

  /**
   * `provider.connect`, with the SSH host-key prompt and its retry, and on
   * desktop, for DuckDB, the DuckDB support install (`withDuckdbHelper`):
   * an `interactive` connect that finds it missing asks to install it and
   * connects again with the same request. Background reconnects never ask.
   */
  private async connectCore(
    type: DatabaseConnection["type"],
    request: ConnectRequest,
    ssh: SshServer,
    options: { interactive: boolean },
  ): Promise<string> {
    const provider = await this.providers.getForType(type);
    return viaDuckdbHelper(
      type,
      () =>
        withHostKeyPrompt(
          (trustHostKey) => provider.connect(trustHostKey ? { ...request, trustHostKey } : request),
          ssh,
        ),
      options,
    );
  }

  /**
   * Listen for connections Core closed without being asked (an evicted web
   * session, a lost connection): each is shown as disconnected. Call once
   * per page; returns the unsubscribe.
   */
  listenForCoreEvents(): () => void {
    const client = getCoreClient();
    // Before `events`, so the first start isn't missed. Events sent while
    // the channel was down are lost (on web a sleep past the server's
    // window grace closes this tab's connections, probe F4 review I1), so
    // after every restart ask Core which connections it still holds.
    const stopResubscribed = client.onResubscribed(({ initial }) => {
      if (!initial) void this.checkAlive();
    });
    // Any call that finds its connection gone says so (`watchClient`).
    const stopNotFound = onConnectionNotFound((id) => this.handleConnectionLost(id));
    const stopEvents = client.events((event) => {
      // `storageChanged` is for the change feed (phase 5d-1, Task 6).
      if (event.type === "connectionClosed") this.handleConnectionClosed(event);
    });
    return () => {
      stopEvents();
      stopNotFound();
      stopResubscribed();
    };
  }

  /**
   * Ask Core (`db.alive`) which of the connections this page shows it still
   * holds; reconnect, quietly, each one it doesn't. Nothing is asked when
   * the page holds none.
   */
  async checkAlive(): Promise<void> {
    const held = this.state.connections.filter((c) => !!c.providerConnectionId);
    if (held.length === 0) return;
    let alive: string[];
    try {
      alive = await callDb(getCoreClient(), "alive", {
        connectionIds: held.map((c) => c.providerConnectionId!),
      });
    } catch (error) {
      void log.warn("Checking which connections Core holds failed:", error);
      return;
    }
    const open = new Set(alive);
    for (const connection of held) {
      if (!open.has(connection.providerConnectionId!)) {
        this.handleConnectionLost(connection.providerConnectionId!);
      }
    }
  }

  /**
   * Core no longer holds `providerConnectionId` (a call answered
   * `CONNECTION_NOT_FOUND`, or `db.alive` left it out): if this page still
   * shows it, mark it disconnected and reconnect it once, quietly; only a
   * failed reconnect is shown. The call that failed isn't retried.
   */
  handleConnectionLost(providerConnectionId: string): void {
    const connection = this.state.connections.find(
      (c) => c.providerConnectionId === providerConnectionId,
    );
    if (!connection) return;
    // On desktop Core drops a DuckDB connection unasked only when its
    // helper died (Decision 7, Task 2 review M3): shown lost, never
    // reconnected quietly, since the query that killed the helper may do
    // it again. The `CONNECTION_CLOSED` event says the same if it comes
    // first; then this finds nothing.
    if (connection.type === "duckdb" && isTauri()) {
      void log.warn(`Core no longer holds DuckDB connection ${connection.id}`);
      this.markDisconnected(connection);
      errorToast(m.connection_closed_lost({ name: connection.name }));
      return;
    }
    void log.warn(`Core no longer holds connection ${connection.id}; reconnecting`);
    this.reconnectQuietly(connection, () => m.connection_closed_lost({ name: connection.name }));
  }

  /**
   * The background path (F4 re-review R1, M-b, M-c): show `connection` as
   * disconnected without moving the project's active connection or closing
   * its schema tabs, and reconnect it once. The active connection stays the
   * active one, shown disconnected until it's back; landing doesn't make it
   * active either. Only a failed reconnect is shown (`failure()`), and then
   * its schema tabs close. A disconnect by the user meanwhile wins: the
   * connection that lands is closed again, and nothing is said.
   */
  private reconnectQuietly(connection: DatabaseConnection, failure: () => string): void {
    this.markDisconnected(connection, { background: true });
    const attempt = this.autoReconnect(connection.id, { background: true });
    void attempt.then((ok) => {
      const userDisconnected = this.cancelledAttempts.has(attempt);
      if (ok) return;
      // Disconnected for good now: its schema tabs close, as a disconnect's
      // do. Only a failure the user didn't ask for is said.
      this.closeSchemaTabs(connection);
      if (!userDisconnected) errorToast(failure());
    });
  }

  /**
   * Mark the connection `event` names as disconnected, and say why. After
   * Core restarted in the page (`CORE_RESTARTED`, the demo's trap recovery,
   * phase 8 Decision 16) nothing is wrong with the connection itself, so
   * it's reconnected at once as a saved target, as `autoReconnect` does;
   * only a failed reconnect is shown.
   */
  handleConnectionClosed(event: ConnectionClosedEvent): void {
    const connection = this.state.connections.find(
      (c) => c.providerConnectionId === event.connectionId,
    );
    if (!connection) return;
    void log.warn(`Connection closed by Core (${event.code}): ${connection.id}`);
    if (event.code === CORE_RESTARTED) {
      this.reconnectQuietly(connection, () => connectionClosedMessage(connection.name, event));
      return;
    }
    this.markDisconnected(connection);
    // The web server closed a closed tab's connections, or this tab's older
    // connection after it connected again (phase 6 probe F4). Only a page
    // that no longer holds the connection should hear these; one that does
    // just shows it disconnected.
    if (QUIET_CLOSE_CODES.has(event.code)) return;
    errorToast(connectionClosedMessage(connection.name, event));
  }

  /**
   * Add a new database connection: connect the form, load its schema, then
   * save it through the library, whose answer carries Core's id (Decision
   * 1). Only then does it join the page, so a failed save leaves nothing
   * and disconnects. On desktop its secrets go in the same call; on web the
   * vault is unlocked first (a cancelled unlock saves nothing) and its
   * ciphertext is written after the row, under Core's id.
   */
  async add(connection: ConnectionInput): Promise<string> {
    void log.info(`Adding connection: type=${connection.type}`);
    // SQLite and DuckDB on web (Decision 11b): refuse before anything runs.
    assertDatabaseTypeAvailable(connection.type);
    this.assertSshAvailable(connection);
    // The row's id is Core's, known once it's saved.
    const pendingId = `${NEW_CONNECTION}${++this.pendingCount}`;
    this.connectingIds.add(pendingId);

    try {
      const providerConnectionId = await this.connectCore(
        connection.type,
        formConnectRequest(connection),
        sshServerOf(connection),
        { interactive: true },
      );
      const projectId = connection.projectId || this.state.activeProjectId || DEFAULT_PROJECT_ID;

      // Load the schema before saving, so a database that can't be read
      // isn't saved. The client reads the Core id from the connection.
      let schemasWithTables: SchemaTable[];
      try {
        schemasWithTables = await getEngineClient({
          id: pendingId,
          type: connection.type,
          providerConnectionId,
        }).schemaTables();
      } catch (error) {
        await this.disconnectCore(connection.type, providerConnectionId);
        throw new Error(`Failed to load database schema: ${String(error)}`);
      }

      let saved: DatabaseConnection;
      try {
        if (isWeb() && hasSecretsToSave(connection)) {
          await getVault().ensureUnlocked();
        }
        const { value, seq } = await this.state.librarySeqs.write([rowKey("connection", NEW)], () =>
          getLibrary().createConnection(
            connectionDraft(connection, projectId),
            isTauri() ? newSecrets(connection) : undefined,
          ),
        );
        saved = this.applyOwn(value, seq, {
          password: connection.password,
          providerConnectionId,
        });
      } catch (err) {
        await this.disconnectCore(connection.type, providerConnectionId);
        if (err instanceof VaultCancelledError) {
          errorToast(m.connection_vault_unlock_cancelled());
          throw err;
        }
        throw this.failed(err);
      }
      if (isWeb()) await this.saveVaultSecrets(saved.id, connection, {});
      await this.bindSaved(connection.type, providerConnectionId, saved.id);

      this.appendToOrder(projectId, saved.id);
      this.stateRestoration.initializeConnectionMaps(saved.id);
      this.state.schemas = {
        ...this.state.schemas,
        [saved.id]: schemasWithTables,
      };
      void log.info(`Schema loaded for ${saved.id}: ${schemasWithTables.length} tables`);

      // Only set active connection once the schema loaded and the row is saved
      this.setActiveForProject(saved.id, projectId);

      // Load column metadata asynchronously in the background, with the
      // saved connection's engine client.
      void this.onSchemaLoaded(saved.id, schemasWithTables, getEngineClient(saved, this.state));

      // Create initial query tab for new connection
      this.onCreateInitialTab();

      void log.info(`Connection established: ${saved.id}`);
      return saved.id;
    } finally {
      this.connectingIds.delete(pendingId);
    }
  }

  /**
   * Tell Core the connection `add` opened is the row it just saved, so
   * the assistant runs on it (Decision 6: a turn needs a connection opened
   * for its chat's row). A failure is logged: only the assistant needs it,
   * and a reconnect records it again.
   */
  private async bindSaved(
    type: DatabaseConnection["type"],
    providerConnectionId: string,
    savedId: string,
  ): Promise<void> {
    try {
      const provider = await this.providers.getForType(type);
      await provider.bindSaved?.(providerConnectionId, savedId);
    } catch (error) {
      void log.warn(`Recording the saved connection failed (${errorCode(error) ?? "unknown"})`);
    }
  }

  /** Close a Core connection this page opened, ignoring a failure. */
  private async disconnectCore(type: DatabaseConnection["type"], providerConnectionId: string) {
    const provider = await this.providers.getForType(type);
    await provider.disconnect(providerConnectionId).catch(() => {});
  }

  /**
   * Web: write the secrets the form saves to the vault under the saved
   * connection's id, and delete those whose flag the save turned off.
   * Core has no store on web (Decision 8). A failure is shown, not thrown:
   * the row is saved.
   */
  private async saveVaultSecrets(
    id: string,
    input: ConnectionInput,
    before: Pick<DatabaseConnection, "savePassword" | "saveSshPassword" | "saveSshKeyPassphrase">,
  ): Promise<void> {
    const keyring = getKeyringService();
    if (!keyring.isAvailable()) return;
    try {
      if (input.savePassword && input.password) await keyring.setDbPassword(id, input.password);
      else if (input.savePassword === false && before.savePassword)
        await keyring.deleteDbPassword(id);
      if (input.saveSshPassword && input.sshPassword)
        await keyring.setSshPassword(id, input.sshPassword);
      else if (input.saveSshPassword === false && before.saveSshPassword)
        await keyring.deleteSshPassword(id);
      if (input.saveSshKeyPassphrase && input.sshKeyPassphrase)
        await keyring.setSshKeyPassphrase(id, input.sshKeyPassphrase);
      else if (input.saveSshKeyPassphrase === false && before.saveSshKeyPassphrase)
        await keyring.deleteSshKeyPassphrase(id);
    } catch (error) {
      void log.warn("Saving a password to the vault failed:", error);
      errorToast(m.connection_vault_save_failed({ message: extractErrorMessage(error) }));
    }
  }

  /**
   * Import connections from another tool (Decision 47): Core reads the file
   * again, imports the candidates `chosen` names into `projectId` in one
   * transaction (local-only, a taken name as the first free "<name> (2)",
   * the duplicate check inside it) and appends them to the project's order.
   * The page then reads the new connections and the order. A candidate
   * that matches a saved connection is skipped; one no longer in the file,
   * or a refused call, is a failure with its reason.
   */
  async importConnections(
    source: ImportSource,
    projectId: string,
    chosen: readonly ImportChoice[],
  ): Promise<ImportResult> {
    const result: ImportResult = { imported: 0, skipped: 0, failures: [] };
    if (chosen.length === 0) return result;
    const names = new Map(chosen.map((c) => [c.key, c.name]));
    let outcomes;
    try {
      ({
        value: { results: outcomes },
      } = await getImports().create(
        source,
        projectId,
        chosen.map((c) => c.key),
      ));
    } catch (error) {
      void log.warn(`Importing ${source} connections failed (${errorCode(error) ?? "unknown"})`);
      const reason = extractErrorMessage(this.failed(error));
      result.failures = chosen.map((c) => ({ name: c.name, reason }));
      return result;
    }
    const imported: string[] = [];
    for (const outcome of outcomes) {
      const name = names.get(outcome.key) ?? outcome.key;
      if (outcome.status === "imported" && outcome.id) {
        imported.push(outcome.id);
        result.imported++;
      } else if (outcome.status === "duplicate") {
        result.skipped++;
      } else {
        result.failures.push({ name, reason: m.import_failure_not_found() });
      }
    }
    if (imported.length > 0) {
      await this.refreshFromLibrary(imported, { remote: false });
      await refreshConnectionOrder(this.state, projectId);
    }
    return result;
  }

  /**
   * Reconnect to an existing connection from the reconnect tab's form.
   * `askToInstall: false` (the tab's own auto-connect) never opens the
   * DuckDB support dialog; the tab's Connect does.
   */
  reconnect(
    connectionId: string,
    connection: ConnectionInput,
    baseline?: ConnectionFields,
    options: { askToInstall?: boolean } = {},
  ): Promise<string> {
    // One at a time per connection (review M1), as `autoReconnect`.
    const running = this.reconnects.get(connectionId);
    if (running) return running;
    const interactive = options.askToInstall ?? true;
    const attempt = this.reconnectOnce(connectionId, connection, baseline, interactive).finally(
      () => {
        this.reconnects.delete(connectionId);
      },
    );
    this.reconnects.set(connectionId, attempt);
    return attempt;
  }

  private async reconnectOnce(
    connectionId: string,
    connection: ConnectionInput,
    baseline: ConnectionFields | undefined,
    interactive: boolean,
  ): Promise<string> {
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
        // The row it edits: Core records it, so the assistant runs here.
        { ...formConnectRequest(connection), savedConnectionId: connectionId },
        sshServerOf(connection),
        // What was connected is what the row stores: the fields the form
        // changed since `baseline` (what it opened with), so another
        // window's edit to another field survives, and the secrets it saves.
        { input: connection, baseline },
        { interactive },
      );
      return connectionId;
    } finally {
      this.connectingIds.delete(connectionId);
    }
  }

  /**
   * Connect a listed connection again: drop its old Core connection, connect
   * `request`, load its schema, then store that it connected (`connected`,
   * so `lastConnected` is now) with the fields a form changed since its
   * `baseline` and the secrets it saves. If that save is refused, the page
   * shows the stored fields again (the connection stays open).
   */
  private async connectExisting(
    existingConnection: DatabaseConnection,
    request: ConnectRequest,
    ssh: SshServer,
    form: { input: ConnectionInput; baseline?: ConnectionFields } | undefined,
    options: { background?: boolean; interactive: boolean },
  ): Promise<void> {
    const background = options.background ?? false;
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

    const providerConnectionId = await this.connectCore(existingConnection.type, request, ssh, {
      interactive: options.interactive,
    });
    // The user disconnected it while a background reconnect ran: theirs wins.
    if (background && this.userDisconnected.has(connectionId)) {
      await this.disconnectCore(existingConnection.type, providerConnectionId);
      throw new Error("Disconnected while reconnecting");
    }

    // The fields the form changed are shown at once (and stored below).
    const current = this.state.connections.find((c) => c.id === connectionId) ?? existingConnection;
    const input = form?.input;
    const changed = input
      ? connectionPatch(form?.baseline ?? fieldsOf(existingConnection), input)
      : {};
    const updatedConnection: DatabaseConnection = {
      ...current,
      ...fieldsOfPatch(changed),
      ...(input ? { password: input.password } : {}),
      providerConnectionId,
      lastConnected: new Date(),
    };
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
      await this.disconnectCore(existingConnection.type, providerConnectionId);
      throw new Error(`Failed to load database schema: ${String(error)}`);
    }

    if (background && this.userDisconnected.has(connectionId)) {
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? { ...c, providerConnectionId: undefined } : c,
      );
      await this.disconnectCore(existingConnection.type, providerConnectionId);
      throw new Error("Disconnected while reconnecting");
    }

    // Store tables immediately (without column metadata) so UI is responsive
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously in the background
    void this.onSchemaLoaded(connectionId, schemasWithTables, client);

    // Set this as the active connection (only after schema loading
    // succeeds), unless it reconnected in the background (F4 re-review R1):
    // then the user's choice of active connection stands.
    if (!background || this.upgraded.has(connectionId)) {
      this.setActiveForProject(connectionId, existingConnection.projectId);

      // Create initial query tab if no tabs exist for the project
      const projectId = existingConnection.projectId;
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      if (tabs.length === 0) {
        this.onCreateInitialTab();
      }
    }

    // Store it. The connection is open and usable either way: a failed save
    // is shown, and failing the connect for it would be wrong.
    const patch: ConnectionPatch = { ...changed, connected: true };
    try {
      const answer = await this.state.librarySeqs.write([rowKey("connection", connectionId)], () =>
        getLibrary().updateConnection(
          connectionId,
          patch,
          input && isTauri() ? newSecrets(input) : undefined,
        ),
      );
      this.applyOwn(answer.value, answer.seq);
      reportProjection(answer, existingConnection.projectId);
      if (input && isWeb()) await this.saveVaultSecrets(connectionId, input, existingConnection);
    } catch (error) {
      void log.warn(`Saving reconnected ${connectionId} failed:`, error);
      // Show what's stored again for the fields the refused save changed
      // (a taken name, say); the connection stays open.
      const restore = Object.fromEntries(
        Object.keys(fieldsOfPatch(changed)).map((k) => [k, current[k as keyof DatabaseConnection]]),
      ) as Partial<DatabaseConnection>;
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? { ...c, ...restore } : c,
      );
      errorToast(extractErrorMessage(this.failed(error)));
      // Then the newest stored row: another window may have changed a field
      // while this save was refused.
      await this.refreshFromLibrary([connectionId], { remote: false }).catch(
        (e) => void log.warn(`Reading ${connectionId} again failed:`, e),
      );
    }
  }

  /**
   * Save an edited connection without reconnecting. Only the fields the
   * form changed are sent (Decision 2): `baseline` is what the form opened
   * with, so another window's change to a field this form didn't touch
   * survives (Decision 18). Without one, the page's copy is the baseline.
   * A refusal (`NAME_TAKEN`, a removed row) throws, worded for the user,
   * and changes nothing.
   */
  async update(
    connectionId: string,
    connection: ConnectionInput,
    baseline?: ConnectionFields,
  ): Promise<void> {
    const existingConnection = this.state.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }
    const patch = connectionPatch(baseline ?? fieldsOf(existingConnection), connection);
    const secrets = isTauri() ? newSecrets(connection) : undefined;

    if (isEmptyPatch(patch) && !secrets) {
      const updatedConnection = {
        ...existingConnection,
        password: connection.password,
      };
      this.state.connections = this.state.connections.map((c) =>
        c.id === connectionId ? updatedConnection : c,
      );
    } else {
      try {
        const answer = await this.state.librarySeqs.write(
          [rowKey("connection", connectionId)],
          () => getLibrary().updateConnection(connectionId, patch, secrets),
        );
        const updatedConnection = this.applyOwn(answer.value, answer.seq, {
          password: connection.password,
        });
        // A linked connection's template follows (Core publishes it).
        reportProjection(answer, updatedConnection.projectId);
      } catch (error) {
        throw this.failed(error);
      }
    }
    if (isWeb()) await this.saveVaultSecrets(connectionId, connection, existingConnection);
  }

  /**
   * Change a saved connection's stored fields with `patch`: one targeted
   * call, then the page shows Core's row. A refusal throws, worded for the
   * user, and changes nothing.
   */
  patch(connectionId: string, patch: ConnectionPatch): Promise<DatabaseConnection> {
    return patchConnection(this.state, connectionId, patch);
  }

  /**
   * Test a connection without persisting it: Core connects the form and
   * closes it again (with its tunnel). Throws on failure so callers can
   * display the error inline.
   */
  async test(connection: ConnectionInput): Promise<void> {
    assertDatabaseTypeAvailable(connection.type);
    this.assertSshAvailable(connection);
    // Decision 6: a DuckDB file a connected connection holds can't be
    // opened twice (Core refuses at once). It is open and working, so the
    // test passes without asking, when the form names it the same way: the
    // same path, and no connection string or the row's own (one with other
    // options is Core's to test). Core compares files, this paths: another
    // spelling of the same file gets Core's answer as it is.
    const file = isTauri() ? duckdbFile(connection) : null;
    const formString = connection.connectionString?.trim() ?? "";
    if (
      file &&
      this.state.connections.some(
        (c) =>
          !!c.providerConnectionId &&
          duckdbFile(c) === file &&
          (formString === "" || formString === (c.connectionString?.trim() ?? "")),
      )
    ) {
      return;
    }
    const provider = await this.providers.getForType(connection.type);
    const request = formConnectRequest(connection);
    await viaDuckdbHelper(
      connection.type,
      () =>
        withHostKeyPrompt(
          (trustHostKey) => provider.test(trustHostKey ? { ...request, trustHostKey } : request),
          sshServerOf(connection),
        ),
      { interactive: true },
    );
  }

  /**
   * Remove a connection and all its state. Core deletes the row (its
   * history, chats and labels cascade) and its secrets (Decision 9); a
   * failed delete throws, worded for the user, and the connection stays.
   */
  async remove(id: string): Promise<void> {
    void log.info(`Removing connection: ${id}`);
    // Prevent deletion of demo connection in demo mode
    if (isDemo() && id === "demo-connection") {
      return;
    }

    const connection = this.state.connections.find((c) => c.id === id);

    let answer;
    try {
      answer = await this.state.librarySeqs.write([rowKey("connection", id)], () =>
        getLibrary().removeConnection(id),
      );
      // A tombstone: an older list can't bring it back.
      this.state.librarySeqs.note(rowKey("connection", id), answer.seq);
    } catch (error) {
      throw this.failed(error);
    }

    // Close the Core connection (and its SSH tunnel) if there is one
    if (connection?.providerConnectionId) {
      await this.providers.getForType(connection.type).then((provider) => {
        provider.disconnect(connection.providerConnectionId!).catch((e) => void log.error(e));
      });
    }

    if (connection) {
      // Out of the page, with its tabs in every project, and another
      // connection made active if it was.
      this.forget(connection);
    }
    // A linked connection's template went first (Core, Decision 37).
    reportProjection(answer, connection?.projectId, { removal: true });
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
    this.windowState.scheduleProject(projectId);
  }

  /**
   * The demo's connection, once the demo's start (`$lib/demo/init`) has
   * stored it (Core's `ensureDemoConnection`, Decision 19: the fixed
   * `demo-connection` row, created once and afterwards only marked
   * connected, so the visitor's labels, AI flags and project stay),
   * connected it as a saved target (`providerConnectionId`) and seeded its
   * sample tables. Shows the row Core stored, loads the schema and makes
   * it the active connection.
   *
   * A row the page already listed keeps its query history and chats (the
   * page loaded them with the list); one stored just now has none.
   */
  async addDemoConnection(
    stored: { value: WireConnection; seq: ChangeSeq },
    providerConnectionId: string,
  ): Promise<string> {
    const listed = this.state.connections.some((c) => c.id === stored.value.id);
    const connection = this.applyOwn(stored.value, stored.seq, {
      providerConnectionId,
      password: "",
    });
    const { id: connectionId, projectId } = connection;
    if (listed) {
      this.stateRestoration.ensureConnectionMapsExist(connectionId);
    } else {
      this.stateRestoration.initializeConnectionMaps(connectionId);
      this.appendToOrder(projectId, connectionId);
    }

    const client = getEngineClient(connection, this.state);
    const schemasWithTables = await client.schemaTables();

    this.setActiveForProject(connectionId, projectId);
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: schemasWithTables,
    };

    // Load column metadata asynchronously
    void this.onSchemaLoaded(connectionId, schemasWithTables, client);

    // A query tab only when the project has none, as `reconnect` does. The
    // page restored the project's tabs before the demo connects, so a
    // reload opening one here added another ("Query 2", …) on every load
    // and made it the active tab.
    if ((this.state.queryTabsByProject[projectId] ?? []).length === 0) {
      this.onCreateInitialTab();
    }

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
  autoReconnect(connectionId: string, options: { background?: boolean } = {}): Promise<boolean> {
    const background = options.background ?? false;
    // One at a time per connection (review M1): a second call while one
    // runs (an event and a failed call at once) gets the same promise. A
    // caller that isn't background joining a background attempt (the user
    // picks the connection) upgrades it: it activates when it lands (N2).
    const running = this.autoReconnects.get(connectionId);
    if (running) {
      if (!background && this.backgroundIds.has(connectionId)) this.upgraded.add(connectionId);
      return running;
    }
    if (background) this.backgroundIds.add(connectionId);
    const attempt: Promise<boolean> = this.autoReconnectOnce(connectionId, background).finally(
      () => {
        this.autoReconnects.delete(connectionId);
        this.backgroundIds.delete(connectionId);
        this.upgraded.delete(connectionId);
        // The flag lives only as long as its attempt (N1); whether it was
        // set stays on the attempt, for `reconnectQuietly`.
        if (this.userDisconnected.delete(connectionId)) this.cancelledAttempts.add(attempt);
      },
    );
    this.autoReconnects.set(connectionId, attempt);
    return attempt;
  }

  private async autoReconnectOnce(connectionId: string, background: boolean): Promise<boolean> {
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
        {
          target: { type: "saved", id: connectionId },
          ...(secrets ? { secrets } : {}),
        },
        sshServerOf(connection),
        undefined,
        // A background reconnect never opens the DuckDB support dialog.
        { background, interactive: !background },
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
   *   saves it (unlocking the vault if needed). Web has no SSH. When that
   *   read fails or the unlock is cancelled this throws, and
   *   `autoReconnect` answers false without connecting.
   * - Desktop sends nothing more: Core reads the keychain under the row's
   *   flags.
   */
  private async heldSecrets(connection: DatabaseConnection): Promise<SuppliedSecrets | undefined> {
    let db = connection.password || undefined;
    if (!db && isWeb() && connection.savePassword) {
      const keyring = getKeyringService();
      if (keyring.isAvailable()) {
        try {
          // Strict (F4 re-review M-a): null only when no password is
          // stored; a stored one that can't be decrypted throws.
          const stored = keyring.getDbPasswordStrict
            ? await keyring.getDbPasswordStrict(connection.id)
            : await keyring.getDbPassword(connection.id);
          db = stored || undefined;
        } catch (error) {
          // A failed or cancelled unlock, or a stored password that can't be
          // decrypted: the row saves a password this page couldn't read, so
          // don't dial without it (F1/F2/F5 review P2: a
          // failed login can count toward a lockout, as on SQL Server). The
          // user connects from the connection's tab. A vault that holds no
          // password for the row (trust auth, probe F5) still connects.
          void log.warn("Reading the saved password from the vault failed:", error);
          throw new Error("The saved password couldn't be read from the vault");
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
   * The shared switch: a connection is shared when it has a template link
   * (Decision 53), whatever its stored local-only flag says. Sharing sends
   * `isLocalOnly: false`, on which Core writes its template and links it;
   * unsharing sends `true`, which takes the template out. One click either
   * way; the outcome is said.
   */
  async toggleLocalOnly(connectionId: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection) return;
    // Store the change, then show it (a refusal throws, worded for the user)
    await this.patch(connectionId, {
      isLocalOnly: !!connection.sharedConnectionId,
    });
  }

  /**
   * Toggle connection state (disconnect if connected).
   */
  async toggle(id: string): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === id);
    // A background reconnect running for it (F4 re-review M-c): the user's
    // disconnect wins when it lands.
    if (connection && this.backgroundIds.has(id)) this.userDisconnected.add(id);
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
  private markDisconnected(
    connection: DatabaseConnection,
    options: { background?: boolean } = {},
  ): void {
    const id = connection.id;
    this.state.connections = this.state.connections.map((c) =>
      c.id === id ? { ...c, providerConnectionId: undefined } : c,
    );
    void log.info(`Connection disconnected: ${id}`);
    // A background reconnect (F4 re-review R1, M-b) keeps the schema tabs
    // and the project's active connection while it runs.
    if (options.background) return;

    this.closeSchemaTabs(connection);
    const projectId = connection.projectId;

    // If it was the project's active connection, switch to another connected one
    if (this.state.activeConnectionIdByProject[projectId] === id) {
      const nextConnection = this.state.connections.find(
        (c) => c.projectId === projectId && !!c.providerConnectionId && c.id !== id,
      );
      this.setActiveForProject(nextConnection?.id ?? null, projectId);
    }
  }

  /** Close `connection`'s schema tabs (and drop them from the tab order). */
  private closeSchemaTabs(connection: DatabaseConnection): void {
    const id = connection.id;
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
    this.windowState.scheduleProject(projectId);
  }

  /**
   * Replace the entire connection order for a project (used by drag-and-drop).
   */
  reorder(projectId: string, orderedIds: string[]): void {
    this.state.connectionOrderByProject = {
      ...this.state.connectionOrderByProject,
      [projectId]: [...orderedIds],
    };
    // Shared by the project's windows (not view state): stored at once.
    void storeConnectionOrder(this.state, projectId);
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
