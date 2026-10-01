/**
 * `ConnectionManager` on Core: the request each connect path sends (`add`,
 * `reconnect` and `test` send the form; `autoReconnect` sends the saved row
 * with the secrets this page holds, which differ on web and desktop), the
 * SSH host-key prompt and retry, cleanup when a connect or schema load
 * fails, and `connectionClosed` events marking a connection disconnected.
 * And what each path stores through the library (phase 5d-1): Core's id on
 * a create, only the changed fields on an edit, the secrets in the Core
 * call on desktop and never on web.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectRequest, ProviderRegistry } from "$lib/providers";
import type { DatabaseConnection } from "$lib/types";
import type { WindowStateManager } from "./window-state.svelte.js";
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
const ensureUnlocked = vi.hoisted(() => vi.fn(async (): Promise<unknown> => ({})));

vi.mock("$lib/features", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/features")>()),
  isFeatureEnabled: () => !env.web,
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: () => schemaTables() }),
  TsEngineClient: class {},
}));
const keyringCalls: string[] = [];
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    isAvailable: () => true,
    isUnlocked: () => true,
    getDbPassword: (id: string) => vaultPassword(id),
    setDbPassword: async (id: string) => void keyringCalls.push(`setDbPassword ${id}`),
    deleteDbPassword: async (id: string) => void keyringCalls.push(`deleteDbPassword ${id}`),
    setSshPassword: async (id: string) => void keyringCalls.push(`setSshPassword ${id}`),
    deleteSshPassword: async (id: string) => void keyringCalls.push(`deleteSshPassword ${id}`),
    setSshKeyPassphrase: async (id: string) => void keyringCalls.push(`setSshKeyPassphrase ${id}`),
    deleteSshKeyPassphrase: async (id: string) =>
      void keyringCalls.push(`deleteSshKeyPassphrase ${id}`),
  }),
}));
vi.mock("$lib/stores/ssh-host-key-prompt.svelte", () => ({
  sshHostKeyPromptStore: { prompt: (...args: unknown[]) => prompt(...args) },
}));
vi.mock("$lib/services/vault/vault-state.svelte", () => ({
  VaultCancelledError: class extends Error {},
  getVault: () => ({ ensureUnlocked: () => ensureUnlocked() }),
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: (msg: string) => errorToast(msg) }));
const infoToast = vi.fn();
vi.mock("svelte-sonner", () => ({
  toast: { success: vi.fn(), info: (m: string) => infoToast(m) },
}));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { getConnectionData, applyPastedConnectionString } =
  await import("$lib/utils/connection-string");
const { forgetConnectionString } = await import("$lib/utils/connection-string-rules");
const { defaultFormData } = await import("./connection-tabs.svelte.js");
const { VaultCancelledError } = await import("$lib/services/vault/vault-state.svelte");
const { setLibrary } = await import("./library/index");
const { RecordingLibrary } = await import("./library/recording-library");
const { LibraryCallError } = await import("./library/types");

const connect = vi.fn(async (_request: ConnectRequest): Promise<string> => "pc-new");
const test = vi.fn(async (_request: ConnectRequest): Promise<void> => {});
const disconnect = vi.fn(async (id: string) => {
  calls.push(`disconnect ${id}`);
});

let library: InstanceType<typeof RecordingLibrary>;

function setup() {
  const state = new DatabaseState();
  const providers = {
    getForType: async () => ({ connect, test, disconnect }),
  } as unknown as ProviderRegistry;
  const persistence = {
    scheduleProject: vi.fn(),
  } as unknown as WindowStateManager;
  const restoration = {
    loadConnectionData: vi.fn(async () => {}),
    initializeConnectionMaps: vi.fn(),
    cleanupConnectionMaps: vi.fn(),
    ensureConnectionMapsExist: vi.fn(),
  } as unknown as StateRestorationManager;
  const onSchemaLoaded = vi.fn(async () => {});
  const onCreateInitialTab = vi.fn();
  const manager = new ConnectionManager(
    state,
    persistence,
    restoration,
    {} as TabOrderingManager,
    providers,
    onSchemaLoaded,
    onCreateInitialTab,
  );
  return { state, manager, onSchemaLoaded, onCreateInitialTab };
}

/** A shown connection that is stored too (the library has its row). */
function showSaved(
  state: InstanceType<typeof DatabaseState>,
  connection: DatabaseConnection = saved,
): void {
  const {
    password: _password,
    providerConnectionId: _core,
    lastConnected: _at,
    ...row
  } = connection;
  library.seedConnection(connection.id, row as never);
  state.connections = [...state.connections, connection];
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
  library = new RecordingLibrary();
  library.seedProject("p1");
  setLibrary(library);
  keyringCalls.length = 0;
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
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    const id = await manager.add(input);
    expect(lastRequest(connect)).toEqual({
      target: { type: "form", form },
      secrets: { db: "pw", ssh: "ssh-pw" },
    });
    expect(state.connections.find((c) => c.id === id)?.providerConnectionId).toBe("pc-new");
  });

  it("adding a connection creates it in Core and shows Core's id", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    const id = await manager.add(input);
    expect(id).toBe("conn-1");
    const [draft, secrets] = library.callsOf("createConnection")[0];
    expect(draft).toEqual({
      projectId: "p1",
      name: "Via bastion",
      type: "postgres",
      host: "db.internal",
      port: 5432,
      databaseName: "app",
      username: "me",
      sslMode: "require",
      sshTunnel: input.sshTunnel,
      savePassword: true,
      saveSshPassword: false,
      saveSshKeyPassphrase: false,
      labelIds: [],
      isLocalOnly: true,
      connected: true,
    });
    // Desktop: the password it saves goes in the same call; the SSH one's
    // flag is off, so it isn't sent. Nothing is written from the page.
    expect(secrets).toEqual({ db: "pw" });
    expect(keyringCalls).toEqual([]);
    expect(state.connections.map((c) => c.id)).toEqual(["conn-1"]);
    expect(state.connectionOrderByProject.p1).toEqual(["conn-1"]);
    expect(state.activeConnectionIdByProject.p1).toBe("conn-1");
  });

  it("web: sends no secrets to Core, and writes the vault under Core's id", async () => {
    env.web = true;
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    await manager.add({ ...input, sshTunnel: undefined, sshPassword: "" } as Input);
    expect(library.callsOf("createConnection")[0]).toHaveLength(1);
    expect(ensureUnlocked).toHaveBeenCalled();
    expect(keyringCalls).toEqual(["setDbPassword conn-1"]);
  });

  it("a taken name is refused with the other connection's name, and nothing is kept", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    showSaved(state, { ...saved, name: "Prod" } as DatabaseConnection);
    library.failures.set("createConnection", {
      error: new LibraryCallError("NAME_TAKEN", "taken", "conn-1"),
    });
    await expect(manager.add(input)).rejects.toThrow(
      'Another connection in this project is already called "Prod".',
    );
    expect(state.connections.map((c) => c.id)).toEqual(["conn-1"]);
    expect(calls).toEqual(["disconnect pc-new"]);
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

  it("a failed create disconnects and leaves nothing", async () => {
    const { manager, state } = setup();
    library.failures.set("createConnection", { error: new Error("STORAGE_ERROR: disk full") });
    await expect(manager.add(input)).rejects.toThrow("disk full");
    expect(state.connections).toEqual([]);
    expect(Object.values(state.connectionOrderByProject).flat()).toEqual([]);
    expect(Object.values(state.activeConnectionIdByProject).filter(Boolean)).toEqual([]);
    expect(calls).toEqual(["disconnect pc-new"]);
  });

  it("a failed save opens no tab, loads no metadata and keeps the active connection", async () => {
    const { manager, state, onSchemaLoaded, onCreateInitialTab } = setup();
    state.activeProjectId = "p1";
    state.activeConnectionIdByProject = { p1: "conn-before" };
    library.failures.set("createConnection", { error: new Error("STORAGE_ERROR: disk full") });
    await expect(manager.add(input)).rejects.toThrow("disk full");
    expect(state.activeConnectionIdByProject.p1).toBe("conn-before");
    expect(onCreateInitialTab).not.toHaveBeenCalled();
    expect(onSchemaLoaded).not.toHaveBeenCalled();
  });

  it("a cancelled vault unlock saves no connection", async () => {
    env.web = true;
    const { manager, state } = setup();
    ensureUnlocked.mockRejectedValueOnce(new VaultCancelledError("cancelled"));
    await expect(
      manager.add({ ...input, sshTunnel: undefined, sshPassword: "" } as Input),
    ).rejects.toBeInstanceOf(VaultCancelledError);
    expect(library.callsOf("createConnection")).toEqual([]);
    expect(state.connections).toEqual([]);
    expect(calls).toEqual(["disconnect pc-new"]);
    expect(errorToast).toHaveBeenCalledTimes(1);
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
    showSaved(state, { ...saved, providerConnectionId: "pc-old" } as DatabaseConnection);
    connect.mockResolvedValueOnce("pc-2");
    await manager.reconnect("conn-1", { ...input, password: "typed" } as Input);
    expect(calls).toEqual(["disconnect pc-old"]);
    expect(lastRequest(connect)).toMatchObject({
      target: { type: "form", form },
      secrets: { db: "typed", ssh: "ssh-pw" },
    });
    expect(state.connections[0].providerConnectionId).toBe("pc-2");
    // Stored: what the form changed, that it connected, and on desktop the
    // password it saves, in the one call.
    const [id, patch, secrets] = library.callsOf("updateConnection")[0];
    expect(id).toBe("conn-1");
    expect(patch).toMatchObject({ host: "db.internal", connected: true });
    expect(secrets).toEqual({ db: "typed" });
  });

  it("leaves the connection disconnected when the connect fails", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...saved, providerConnectionId: "pc-old" } as DatabaseConnection);
    connect.mockRejectedValueOnce(new Error("refused"));
    await expect(manager.reconnect("conn-1", input)).rejects.toThrow("refused");
    expect(state.connections[0].providerConnectionId).toBeUndefined();
    expect(manager.connectingIds.has("conn-1")).toBe(false);
  });
});

describe("autoReconnect", () => {
  it("desktop: sends the saved row with no secrets (Core reads the keychain)", async () => {
    const { manager, state } = setup();
    showSaved(state);
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(true);
    expect(lastRequest(connect)).toEqual({ target: { type: "saved", id: "conn-1" } });
    expect(vaultPassword).not.toHaveBeenCalled();
    // Only that it connected is stored (lastConnected); secrets left alone.
    expect(library.callsOf("updateConnection")).toEqual([["conn-1", { connected: true }]]);
    expect(state.connections[0].providerConnectionId).toBe("pc-new");
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

  it("a failed removal keeps the connection", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...saved, providerConnectionId: "pc-1" } as DatabaseConnection);
    state.connectionOrderByProject = { p1: ["conn-1"] };
    library.failures.set("removeConnection", { error: new Error("STORAGE_ERROR: locked") });
    await expect(manager.remove("conn-1")).rejects.toThrow("locked");
    expect(state.connections.map((c) => c.id)).toEqual(["conn-1"]);
    expect(state.connections[0].providerConnectionId).toBe("pc-1");
    expect(state.connectionOrderByProject.p1).toEqual(["conn-1"]);
    expect(calls).toEqual([]);
  });

  it("remove deletes the row, then disconnects and forgets the connection", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...saved, providerConnectionId: "pc-1" } as DatabaseConnection);
    await manager.remove("conn-1");
    expect(library.callsOf("removeConnection")).toEqual([["conn-1"]]);
    // Core deletes its secrets (desktop keychain, web vault rows).
    expect(keyringCalls).toEqual([]);
    expect(state.connections).toEqual([]);
    expect(calls).toEqual(["disconnect pc-1"]);
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
    const { state, manager } = setup();
    showSaved(state);

    await manager.update("conn-1", {
      ...saved,
      aiShareSchema: false,
      aiShareData: true,
    } as unknown as Parameters<InstanceType<typeof ConnectionManager>["update"]>[1]);

    expect(state.connections[0]).toMatchObject({ aiShareSchema: false, aiShareData: true });
    expect(library.callsOf("updateConnection")).toEqual([
      ["conn-1", { aiShareSchema: false, aiShareData: true }],
    ]);
  });

  it("an edit sends only the changed fields", async () => {
    const { state, manager } = setup();
    showSaved(state);
    await manager.update("conn-1", {
      ...saved,
      name: "Renamed",
      port: 6543,
    } as unknown as Parameters<InstanceType<typeof ConnectionManager>["update"]>[1]);
    expect(library.callsOf("updateConnection")).toEqual([
      ["conn-1", { name: "Renamed", port: 6543 }],
    ]);
  });

  it("an edit against the form's baseline keeps another window's change", async () => {
    const { state, manager } = setup();
    showSaved(state);
    const baseline = { ...saved } as never;
    // Another window renamed it meanwhile; this page shows the new name.
    library.connections.get("conn-1")!.name = "Theirs";
    state.connections = [{ ...saved, name: "Theirs" } as DatabaseConnection];
    // This form (opened before) changes only the port.
    await manager.update(
      "conn-1",
      { ...saved, port: 6543 } as unknown as Parameters<
        InstanceType<typeof ConnectionManager>["update"]
      >[1],
      baseline,
    );
    expect(library.callsOf("updateConnection")).toEqual([["conn-1", { port: 6543 }]]);
    expect(library.connections.get("conn-1")).toMatchObject({ name: "Theirs", port: 6543 });
  });

  it("desktop: a password the form saves goes in the call; web: to the vault after it", async () => {
    const { state, manager } = setup();
    showSaved(state, { ...saved, savePassword: false } as DatabaseConnection);
    const edit = {
      ...saved,
      savePassword: true,
      password: "new-pw",
    } as unknown as Parameters<InstanceType<typeof ConnectionManager>["update"]>[1];
    await manager.update("conn-1", edit);
    expect(library.callsOf("updateConnection").at(-1)).toEqual([
      "conn-1",
      { savePassword: true },
      { db: "new-pw" },
    ]);
    expect(keyringCalls).toEqual([]);

    env.web = true;
    library.connections.get("conn-1")!.savePassword = false;
    state.connections = [{ ...saved, savePassword: false } as DatabaseConnection];
    await manager.update("conn-1", edit);
    expect(library.callsOf("updateConnection").at(-1)).toEqual(["conn-1", { savePassword: true }]);
    expect(keyringCalls).toEqual(["setDbPassword conn-1"]);
  });

  it("a rename to a taken name says whose, and changes nothing", async () => {
    const { state, manager } = setup();
    showSaved(state);
    showSaved(state, { ...saved, id: "conn-2", name: "Prod" } as DatabaseConnection);
    library.failures.set("updateConnection", {
      error: new LibraryCallError("NAME_TAKEN", "taken", "conn-2"),
    });
    await expect(
      manager.update("conn-1", { ...saved, name: "prod" } as unknown as Parameters<
        InstanceType<typeof ConnectionManager>["update"]
      >[1]),
    ).rejects.toThrow('Another connection in this project is already called "Prod".');
    expect(state.connections[0].name).toBe("Local");
  });

  it("clears an override back to the global setting", async () => {
    const { state, manager } = setup();
    showSaved(state, { ...saved, aiShareData: true } as DatabaseConnection);

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
    showSaved(state, {
      ...saved,
      connectionString: "postgresql://me@localhost/app?application_name=x",
    } as DatabaseConnection);
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
    expect(state.connections[0]).toMatchObject({ databaseName: "other" });
    expect(state.connections[0].connectionString).toBeUndefined();
    expect(library.callsOf("updateConnection")[0][1]).toMatchObject({
      databaseName: "other",
      connectionString: null,
      connected: true,
    });
  });

  it("loads saved connections without waiting for a keychain read", async () => {
    library.seedConnection("saved-secret", { name: "Saved secret", savePassword: true });
    vaultPassword.mockImplementation(() => new Promise(() => {}));
    const { manager, state } = setup();

    await manager.initializePersistedConnections();

    expect(state.connectionsLoading).toBe(false);
    expect(state.connections[0]).toMatchObject({ id: "saved-secret", password: "" });
    expect(vaultPassword).not.toHaveBeenCalled();
  });

  it("a failed load says so (the one-time secrets notice then waits)", async () => {
    library.failures.set("listConnections", { error: new Error("STORAGE_ERROR: busy") });
    const { manager, state } = setup();
    await manager.initializePersistedConnections();
    expect(manager.loaded).toBe(false);
    expect(state.connectionsLoading).toBe(false);
    library.failures.clear();
    await manager.initializePersistedConnections();
    expect(manager.loaded).toBe(true);
  });

  it("loads the rows as Core lists them and saves nothing (Core dropped old strings)", async () => {
    library.seedConnection("typed", {
      connectionString: "postgresql://alice@prod/app?application_name=x",
      aiShareSchema: false,
      labelIds: ["prod"],
      isLocalOnly: true,
    });
    const { manager, state } = setup();
    await manager.initializePersistedConnections();
    expect(state.connections.map((c) => [c.id, c.connectionString])).toEqual([
      ["typed", "postgresql://alice@prod/app?application_name=x"],
    ]);
    expect(state.connections[0]).toMatchObject({
      aiShareSchema: false,
      labelIds: ["prod"],
      isLocalOnly: true,
    });
    expect(library.calls.map((c) => c.method)).toEqual(["listConnections"]);
    expect(state.connectionOrderByProject.p1).toEqual(["typed"]);
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

describe("importConnections", () => {
  const draft = (overrides: Record<string, unknown> = {}) =>
    ({
      name: "Imported",
      type: "postgres",
      host: "db",
      port: 5432,
      databaseName: "app",
      username: "me",
      ...overrides,
    }) as Parameters<InstanceType<typeof ConnectionManager>["importConnections"]>[0][number];

  it("two TablePlus connections on one host and port both import", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    const result = await manager.importConnections([
      draft({ databaseName: "app", sslMode: "require" }),
      draft({ databaseName: "reports" }),
    ]);
    expect(result).toEqual({ imported: 2, skipped: 0, failed: 0, failures: [] });
    const ids = state.connections.map((c) => c.id);
    expect(ids).toEqual(["conn-1", "conn-2"]);
    expect(state.connectionOrderByProject.p1).toEqual(ids);
    expect(state.connections[0]).toMatchObject({ projectId: "p1", sslMode: "require" });
    // Core picks a free name for a taken one; the page does no folding.
    for (const [draft] of library.callsOf("createConnection")) {
      expect(draft).toMatchObject({ renameIfTaken: true, isLocalOnly: true, connected: false });
    }
  });

  it("two DBeaver connections on one host and port with different users both import", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    const result = await manager.importConnections([
      draft({ username: "me" }),
      draft({ username: "reader" }),
    ]);
    expect(result.imported).toBe(2);
    expect(state.connections).toHaveLength(2);
  });

  it("skips a connection the project already has", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    state.connections = [{ ...saved, host: "db", username: "me", databaseName: "app" }];
    const result = await manager.importConnections([draft(), draft({ databaseName: "new" })]);
    expect(result).toEqual({ imported: 1, skipped: 1, failed: 0, failures: [] });
    expect(library.callsOf("createConnection")).toHaveLength(1);
  });

  it("an imported connection is local-only, like one made in the wizard", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    await manager.importConnections([draft()]);
    expect(state.connections[0].isLocalOnly).toBe(true);
    expect(library.callsOf("createConnection")[0][0]).toMatchObject({ isLocalOnly: true });
  });

  it("imports create through Core and report failures", async () => {
    const { manager, state } = setup();
    state.activeProjectId = "p1";
    library.failures.set("createConnection", { error: new Error("STORAGE_ERROR: full") });
    const result = await manager.importConnections([draft(), draft({ databaseName: "b" })]);
    expect(result).toEqual({ imported: 1, skipped: 0, failed: 1, failures: ["Imported"] });
    expect(state.connections.map((c) => c.databaseName)).toEqual(["b"]);
    expect(state.connectionOrderByProject.p1).toHaveLength(1);
  });
});
