/**
 * `ConnectionManager` on Core: the request each connect path sends (`add`,
 * `reconnect` and `test` send the form; `autoReconnect` sends the saved row
 * with the secrets this page holds, which differ on web and desktop), the
 * SSH host-key prompt and retry, cleanup when a connect or schema load
 * fails, and `connectionClosed` events marking a connection disconnected.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectRequest, ProviderRegistry } from "$lib/providers";
import type { DatabaseConnection } from "$lib/types";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

const env = vi.hoisted(() => ({ web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => !env.web,
  isWeb: () => env.web,
  isDemo: () => false,
}));

const calls: string[] = [];
const schemaTables = vi.fn(async () => [] as unknown[]);
const prompt = vi.fn(async (..._args: unknown[]) => true);
const vaultPassword = vi.fn(async (_id: string): Promise<string | null> => "vault-pw");
const errorToast = vi.fn();

vi.mock("$lib/features", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/features")>()),
  isFeatureEnabled: () => !env.web,
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: () => schemaTables() }),
  TsEngineClient: class {},
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    isAvailable: () => true,
    isUnlocked: () => true,
    getDbPassword: (id: string) => vaultPassword(id),
  }),
}));
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: { prompt: (...args: unknown[]) => prompt(...args) },
}));
vi.mock("$lib/services/vault/vault-state.svelte", () => ({
  VaultCancelledError: class extends Error {},
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: (msg: string) => errorToast(msg) }));
vi.mock("svelte-sonner", () => ({ toast: { success: vi.fn(), info: vi.fn() } }));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { getConnectionData, applyPastedConnectionString } =
  await import("$lib/utils/connection-string");
const { forgetConnectionString } = await import("$lib/utils/connection-string-rules");
const { defaultFormData } = await import("./connection-tabs.svelte.js");

const connect = vi.fn(async (_request: ConnectRequest): Promise<string> => "pc-new");
const test = vi.fn(async (_request: ConnectRequest): Promise<void> => {});
const disconnect = vi.fn(async (id: string) => {
  calls.push(`disconnect ${id}`);
});

let persistedRows: unknown[] = [];

function setup() {
  const state = new DatabaseState();
  const providers = {
    getForType: async () => ({ connect, test, disconnect }),
  } as unknown as ProviderRegistry;
  const persistConnection = vi.fn(async (..._args: unknown[]) => {});
  const persistence = {
    persistConnection,
    scheduleProject: vi.fn(),
    loadPersistedConnections: vi.fn(async () => persistedRows),
  } as unknown as PersistenceManager;
  const restoration = {
    loadConnectionData: vi.fn(async () => {}),
    initializeConnectionMaps: vi.fn(),
    cleanupConnectionMaps: vi.fn(),
    ensureConnectionMapsExist: vi.fn(),
  } as unknown as StateRestorationManager;
  const manager = new ConnectionManager(
    state,
    persistence,
    restoration,
    {} as TabOrderingManager,
    providers,
    async () => {},
    () => {},
  );
  return { state, manager, persistConnection };
}

type Input = Parameters<InstanceType<typeof ConnectionManager>["add"]>[0];

const input = {
  name: "Via bastion",
  type: "postgres",
  host: "db.internal",
  port: 5432,
  databaseName: "app",
  username: "me",
  password: "pw",
  sslMode: "require",
  connectionString: "",
  sshTunnel: {
    enabled: true,
    host: "bastion",
    port: 2222,
    username: "tunnel",
    authMethod: "password",
  },
  sshPassword: "ssh-pw",
  savePassword: true,
  saveSshPassword: false,
  saveSshKeyPassphrase: false,
} as unknown as Input;

/** The form `input` becomes. */
const form = {
  name: "Via bastion",
  type: "postgres",
  host: "db.internal",
  port: 5432,
  databaseName: "app",
  username: "me",
  sslMode: "require",
  connectionString: "",
  sshEnabled: true,
  sshHost: "bastion",
  sshPort: 2222,
  sshUsername: "tunnel",
  sshAuthMethod: "password",
  sshKeyPath: "",
  savePassword: true,
  saveSshPassword: false,
  saveSshKeyPassphrase: false,
};

const saved = {
  id: "conn-1",
  name: "Local",
  type: "postgres",
  host: "localhost",
  port: 5432,
  databaseName: "app",
  username: "me",
  password: "",
  savePassword: true,
  projectId: "p1",
  labelIds: [],
  isLocalOnly: true,
} as unknown as DatabaseConnection;

const lastRequest = (fn: typeof connect | typeof test) => fn.mock.calls.at(-1)?.[0];

beforeEach(() => {
  persistedRows = [];
  calls.length = 0;
  env.web = false;
  vi.clearAllMocks();
  connect.mockResolvedValue("pc-new");
  test.mockResolvedValue(undefined);
  schemaTables.mockResolvedValue([]);
  prompt.mockResolvedValue(true);
  vaultPassword.mockResolvedValue("vault-pw");
});

describe("add", () => {
  it("connects the form with the secrets typed into it, then saves the row", async () => {
    const { manager, state, persistConnection } = setup();
    const id = await manager.add(input);
    expect(lastRequest(connect)).toEqual({
      target: { type: "form", form },
      secrets: { db: "pw", ssh: "ssh-pw" },
    });
    expect(state.connections.find((c) => c.id === id)?.providerConnectionId).toBe("pc-new");
    expect(persistConnection).toHaveBeenCalledWith(
      expect.objectContaining({ id }),
      expect.objectContaining({ savePassword: true, sshPassword: "ssh-pw" }),
    );
  });

  it("leaves out an unset SSL mode, empty secrets and a false createIfMissing", async () => {
    const { manager } = setup();
    await manager.add({
      ...input,
      sslMode: "",
      password: "",
      sshTunnel: undefined,
      sshPassword: "",
    } as Input);
    const request = lastRequest(connect)!;
    expect(request).toEqual({ target: { type: "form", form: expect.any(Object) } });
    if (request.target.type !== "form") throw new Error("not a form");
    expect("sslMode" in request.target.form).toBe(false);
    expect(request.target.form.sshEnabled).toBe(false);
  });

  it("sends createIfMissing for a new SQLite file", async () => {
    const { manager } = setup();
    await manager.add({
      name: "f",
      type: "sqlite",
      host: "",
      port: 0,
      databaseName: "/tmp/new.db",
      username: "",
      password: "",
      createIfMissing: true,
    } as Input);
    expect(lastRequest(connect)).toMatchObject({ createIfMissing: true });
  });

  it("disconnects and forgets the connection when the schema load fails", async () => {
    const { manager, state } = setup();
    schemaTables.mockRejectedValueOnce(new Error("no schema"));
    await expect(manager.add(input)).rejects.toThrow("Failed to load database schema");
    expect(calls).toEqual(["disconnect pc-new"]);
    expect(state.connections).toEqual([]);
  });

  it("adds nothing when the connect fails", async () => {
    const { manager, state } = setup();
    connect.mockRejectedValueOnce(new Error("refused"));
    await expect(manager.add(input)).rejects.toThrow("refused");
    expect(state.connections).toEqual([]);
    expect(manager.connectingIds.size).toBe(0);
  });
});

describe("the SSH host-key prompt", () => {
  const unknownKey = new CoreCallError({
    code: "UNKNOWN_HOST_KEY",
    message: "bastion's key SHA256:abc/DEF+123= isn't known",
  });

  it("prompts with the form's SSH server and retries with trustHostKey", async () => {
    const { manager } = setup();
    connect.mockRejectedValueOnce(unknownKey);
    await manager.add(input);
    expect(prompt).toHaveBeenCalledWith("bastion", 2222, "SHA256:abc/DEF+123=");
    expect(connect).toHaveBeenCalledTimes(2);
    expect(connect.mock.calls[0][0]).not.toHaveProperty("trustHostKey");
    expect(connect.mock.calls[1][0]).toMatchObject({ trustHostKey: "SHA256:abc/DEF+123=" });
  });

  it("test prompts and retries too", async () => {
    const { manager } = setup();
    test.mockRejectedValueOnce(unknownKey);
    await manager.test(input);
    expect(test.mock.calls[1][0]).toMatchObject({
      target: { type: "form", form },
      trustHostKey: "SHA256:abc/DEF+123=",
    });
  });

  it("a refused prompt fails the connect with the original error", async () => {
    const { manager } = setup();
    prompt.mockResolvedValueOnce(false);
    connect.mockRejectedValueOnce(unknownKey);
    await expect(manager.add(input)).rejects.toThrow("UNKNOWN_HOST_KEY");
    expect(connect).toHaveBeenCalledOnce();
  });
});

describe("test", () => {
  it("sends the form and keeps nothing", async () => {
    const { manager, state } = setup();
    await manager.test(input);
    expect(lastRequest(test)).toEqual({
      target: { type: "form", form },
      secrets: { db: "pw", ssh: "ssh-pw" },
    });
    expect(connect).not.toHaveBeenCalled();
    expect(state.connections).toEqual([]);
  });
});

describe("reconnect", () => {
  it("drops the old connection first, then connects the form", async () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved, providerConnectionId: "pc-old" }];
    connect.mockResolvedValueOnce("pc-2");
    await manager.reconnect("conn-1", { ...input, password: "typed" } as Input);
    expect(calls).toEqual(["disconnect pc-old"]);
    expect(lastRequest(connect)).toMatchObject({
      target: { type: "form", form },
      secrets: { db: "typed", ssh: "ssh-pw" },
    });
    expect(state.connections[0].providerConnectionId).toBe("pc-2");
  });

  it("leaves the connection disconnected when the connect fails", async () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved, providerConnectionId: "pc-old" }];
    connect.mockRejectedValueOnce(new Error("refused"));
    await expect(manager.reconnect("conn-1", input)).rejects.toThrow("refused");
    expect(state.connections[0].providerConnectionId).toBeUndefined();
    expect(manager.connectingIds.has("conn-1")).toBe(false);
  });
});

describe("autoReconnect", () => {
  it("desktop: sends the saved row with no secrets (Core reads the keychain)", async () => {
    const { manager, state, persistConnection } = setup();
    state.connections = [{ ...saved }];
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(true);
    expect(lastRequest(connect)).toEqual({ target: { type: "saved", id: "conn-1" } });
    expect(vaultPassword).not.toHaveBeenCalled();
    // The row is saved again (lastConnected), its secrets left alone.
    expect(persistConnection).toHaveBeenCalledWith(
      expect.objectContaining({ id: "conn-1", providerConnectionId: "pc-new" }),
      undefined,
    );
  });

  it.each([true, false])(
    "desktop: a password this page holds is sent (savePassword %s)",
    async (savePassword) => {
      const { manager, state } = setup();
      state.connections = [{ ...saved, savePassword, password: "typed-earlier" }];
      await manager.autoReconnect("conn-1");
      expect(lastRequest(connect)).toEqual({
        target: { type: "saved", id: "conn-1" },
        secrets: { db: "typed-earlier" },
      });
    },
  );

  it("web: sends the vault's password for a row that saves it", async () => {
    env.web = true;
    const { manager, state } = setup();
    state.connections = [{ ...saved }];
    await manager.autoReconnect("conn-1");
    expect(vaultPassword).toHaveBeenCalledWith("conn-1");
    expect(lastRequest(connect)).toEqual({
      target: { type: "saved", id: "conn-1" },
      secrets: { db: "vault-pw" },
    });
  });

  it("web: a password this page holds wins over the vault", async () => {
    env.web = true;
    const { manager, state } = setup();
    state.connections = [{ ...saved, password: "typed-earlier" }];
    await manager.autoReconnect("conn-1");
    expect(vaultPassword).not.toHaveBeenCalled();
    expect(lastRequest(connect)).toMatchObject({ secrets: { db: "typed-earlier" } });
  });

  it("web: a row that doesn't save its password never opens the vault", async () => {
    env.web = true;
    const { manager, state } = setup();
    state.connections = [{ ...saved, savePassword: false }];
    await manager.autoReconnect("conn-1");
    expect(vaultPassword).not.toHaveBeenCalled();
    expect(lastRequest(connect)).toEqual({ target: { type: "saved", id: "conn-1" } });
  });

  it("returns false, without a toast, when Core wants credentials", async () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved }];
    connect.mockRejectedValueOnce(
      new CoreCallError({ code: "CREDENTIALS_REQUIRED", message: "no password" }),
    );
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(false);
    expect(errorToast).not.toHaveBeenCalled();
    expect(state.connections[0].providerConnectionId).toBeUndefined();
  });

  it("prompts for an unknown SSH host key of a saved row", async () => {
    const { manager, state } = setup();
    state.connections = [
      {
        ...saved,
        sshTunnel: { enabled: true, host: "jump", port: 22, username: "u", authMethod: "key" },
      } as DatabaseConnection,
    ];
    connect.mockRejectedValueOnce(
      new CoreCallError({ code: "UNKNOWN_HOST_KEY", message: "SHA256:zzz" }),
    );
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(true);
    expect(prompt).toHaveBeenCalledWith("jump", 22, "SHA256:zzz");
    expect(lastRequest(connect)).toEqual({
      target: { type: "saved", id: "conn-1" },
      trustHostKey: "SHA256:zzz",
    });
  });
});

describe("toggle and remove", () => {
  it("toggle disconnects through Core and clears the provider id", async () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved, providerConnectionId: "pc-1" }];
    await manager.toggle("conn-1");
    expect(calls).toEqual(["disconnect pc-1"]);
    expect(state.connections[0].providerConnectionId).toBeUndefined();
  });
});

describe("connectionClosed events", () => {
  it("marks the connection disconnected and says why", () => {
    const { manager, state } = setup();
    state.connections = [
      { ...saved, providerConnectionId: "pc-1" },
      { ...saved, id: "conn-2", name: "Other", providerConnectionId: "pc-2" },
    ];
    manager.handleConnectionClosed({
      type: "connectionClosed",
      connectionId: "pc-1",
      code: "WORKSPACE_EVICTED",
      message: "evicted",
    });
    expect(state.connections.map((c) => c.providerConnectionId)).toEqual([undefined, "pc-2"]);
    expect(errorToast).toHaveBeenCalledOnce();
    expect(errorToast.mock.calls[0][0]).toContain('"Local" was disconnected');
  });

  it("names a lost connection", () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved, providerConnectionId: "pc-1" }];
    manager.handleConnectionClosed({
      type: "connectionClosed",
      connectionId: "pc-1",
      code: "CONNECTION_CLOSED",
      message: "lost",
    });
    expect(errorToast.mock.calls[0][0]).toContain("lost its connection");
  });

  it("ignores a connection this page doesn't show", () => {
    const { manager, state } = setup();
    state.connections = [{ ...saved, providerConnectionId: "pc-1" }];
    manager.handleConnectionClosed({
      type: "connectionClosed",
      connectionId: "pc-other",
      code: "CONNECTION_CLOSED",
      message: "lost",
    });
    expect(state.connections[0].providerConnectionId).toBe("pc-1");
    expect(errorToast).not.toHaveBeenCalled();
  });
});

describe("update", () => {
  it("saves the AI sharing overrides from the edit form", async () => {
    const { state, manager, persistConnection } = setup();
    state.connections = [saved];

    await manager.update("conn-1", {
      ...saved,
      aiShareSchema: false,
      aiShareData: true,
    } as unknown as Parameters<InstanceType<typeof ConnectionManager>["update"]>[1]);

    expect(state.connections[0]).toMatchObject({ aiShareSchema: false, aiShareData: true });
    expect(persistConnection).toHaveBeenCalledWith(
      expect.objectContaining({ id: "conn-1", aiShareSchema: false, aiShareData: true }),
      expect.anything(),
    );
  });

  it("clears an override back to the global setting", async () => {
    const { state, manager } = setup();
    state.connections = [{ ...saved, aiShareData: true } as DatabaseConnection];

    await manager.update("conn-1", {
      ...saved,
      aiShareData: undefined,
    } as unknown as Parameters<InstanceType<typeof ConnectionManager>["update"]>[1]);

    expect(state.connections[0].aiShareData).toBeUndefined();
  });
});

describe("the connection string and the fields", () => {
  const formRequest = () => {
    const request = lastRequest(connect)!;
    if (request.target.type !== "form") throw new Error("not a form");
    return request.target.form;
  };

  it("paste, then edit a field: the edited field connects", async () => {
    const { manager } = setup();
    const form = { ...defaultFormData, name: "App" };
    expect(applyPastedConnectionString(form, "postgres://u:p@db.example.com/app").success).toBe(
      true,
    );
    // The fields say everything the string did, so it's gone.
    expect(form.connectionString).toBe("");
    form.databaseName = "app2";
    await manager.add(getConnectionData(form) as Input);
    expect(formRequest()).toMatchObject({
      host: "db.example.com",
      databaseName: "app2",
      username: "u",
      connectionString: "",
    });
    expect(lastRequest(connect)?.secrets).toEqual({ db: "p" });
  });

  it("a pasted string with parameters is kept, and a field edit clears it", async () => {
    const { manager } = setup();
    const form = { ...defaultFormData, name: "App" };
    const typed = "postgres://u@db.example.com/app?application_name=seaquel";
    applyPastedConnectionString(form, typed);
    expect(form.connectionString).toBe(typed);
    await manager.add(getConnectionData(form) as Input);
    expect(formRequest().connectionString).toBe(typed);

    // What the details step does when Host is edited.
    form.host = "replica.example.com";
    forgetConnectionString(form);
    await manager.add(getConnectionData(form) as Input);
    expect(formRequest()).toMatchObject({ host: "replica.example.com", connectionString: "" });
  });

  it("edit a saved row's field, then reconnect: the new field connects", async () => {
    const { manager, state } = setup();
    state.connections = [
      { ...saved, connectionString: "postgresql://me@localhost/app?application_name=x" },
    ];
    const form = {
      ...defaultFormData,
      name: saved.name,
      host: saved.host,
      databaseName: saved.databaseName,
      username: saved.username,
      connectionString: state.connections[0].connectionString!,
    };
    form.databaseName = "other";
    forgetConnectionString(form);
    await manager.reconnect("conn-1", getConnectionData(form) as Input);
    expect(formRequest()).toMatchObject({ databaseName: "other", connectionString: "" });
    // The row now shows, and stores, what was connected.
    expect(state.connections[0]).toMatchObject({ databaseName: "other", connectionString: "" });
  });

  it("loads saved connections without waiting for a keychain read", async () => {
    persistedRows = [
      {
        id: "saved-secret",
        name: "Saved secret",
        type: "postgres",
        host: "localhost",
        port: 5432,
        databaseName: "app",
        username: "me",
        savePassword: true,
        projectId: "p1",
      },
    ];
    vaultPassword.mockImplementation(() => new Promise(() => {}));
    const { manager, state } = setup();

    await manager.initializePersistedConnections();

    expect(state.connectionsLoading).toBe(false);
    expect(state.connections[0]).toMatchObject({ id: "saved-secret", password: "" });
    expect(vaultPassword).not.toHaveBeenCalled();
  });

  it("drops an old rebuilt string at load and saves the row once", async () => {
    persistedRows = [
      {
        id: "old",
        name: "Old",
        type: "postgres",
        host: "prod",
        port: 5432,
        databaseName: "app",
        username: "alice",
        sslMode: "disable",
        connectionString: "postgresql://alice@prod/app?sslmode=disable",
        savePassword: false,
        projectId: "p1",
        aiShareSchema: false,
        aiShareData: true,
        activeAIProviderId: "anthropic",
        activeAIModel: "m",
        labelIds: ["prod"],
        isLocalOnly: true,
      },
      {
        id: "typed",
        name: "Typed",
        type: "postgres",
        host: "prod",
        port: 5432,
        databaseName: "app",
        username: "alice",
        sslMode: "disable",
        connectionString: "postgresql://alice@prod/app?application_name=x",
        savePassword: false,
        projectId: "p1",
      },
    ];
    const { manager, state, persistConnection } = setup();
    await manager.initializePersistedConnections();
    expect(state.connections.map((c) => [c.id, c.connectionString])).toEqual([
      ["old", undefined],
      ["typed", "postgresql://alice@prod/app?application_name=x"],
    ]);
    expect(persistConnection).toHaveBeenCalledOnce();
    expect(persistConnection.mock.calls[0][0]).toMatchObject({
      id: "old",
      connectionString: undefined,
    });
    // The save keeps every field of the row, the AI sharing overrides included.
    const kept = {
      aiShareSchema: false,
      aiShareData: true,
      activeAIProviderId: "anthropic",
      activeAIModel: "m",
      labelIds: ["prod"],
      isLocalOnly: true,
      username: "alice",
      sslMode: "disable",
    };
    expect(persistConnection.mock.calls[0][0]).toMatchObject(kept);
    expect(state.connections[0]).toMatchObject(kept);
  });
});

describe("markDisconnected", () => {
  it("a closed connection loses its schema tabs and its place as active", () => {
    const { manager, state } = setup();
    state.connections = [
      { ...saved, providerConnectionId: "pc-1" },
      { ...saved, id: "conn-2", name: "Other", providerConnectionId: "pc-2" },
    ];
    state.schemaTabsByProject = {
      p1: [
        { id: "t1", connectionId: "conn-1" },
        { id: "t2", connectionId: "conn-2" },
      ],
    } as unknown as typeof state.schemaTabsByProject;
    state.tabOrderByProject = { p1: ["t1", "t2"] };
    state.activeSchemaTabIdByProject = { p1: "t1" };
    state.activeConnectionIdByProject = { p1: "conn-1" };
    manager.handleConnectionClosed({
      type: "connectionClosed",
      connectionId: "pc-1",
      code: "CONNECTION_CLOSED",
      message: "lost",
    });
    expect(state.schemaTabsByProject.p1.map((t) => t.id)).toEqual(["t2"]);
    expect(state.tabOrderByProject.p1).toEqual(["t2"]);
    expect(state.activeSchemaTabIdByProject.p1).toBe("t2");
    expect(state.activeConnectionIdByProject.p1).toBe("conn-2");
  });
});
