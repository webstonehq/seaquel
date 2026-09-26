/**
 * The SSH tunnel lifecycle in `ConnectionManager`: a tunnel opened for a
 * connect that then fails is closed, `reconnect` drops the old connection
 * before the new one is tried, and toggling a connection off closes its
 * tunnel.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseConnection } from "$lib/types";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

const calls: string[] = [];
let nextTunnel = 0;
const createTunnel = vi.fn(async () => {
  nextTunnel += 1;
  calls.push(`open tunnel-${nextTunnel}`);
  return { tunnelId: `tunnel-${nextTunnel}`, localPort: 40000 + nextTunnel };
});
const closeTunnel = vi.fn(async (id: string) => {
  calls.push(`close ${id}`);
});
const schemaTables = vi.fn(async () => [] as unknown[]);

vi.mock("$lib/services/ssh-tunnel", () => ({
  createSshTunnelWithHostKeyCheck: () => createTunnel(),
  closeSshTunnel: (id: string) => closeTunnel(id),
}));
vi.mock("$lib/features", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/features")>()),
  isFeatureEnabled: () => true,
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: () => schemaTables() }),
  TsEngineClient: class {},
}));
vi.mock("$lib/services/keyring", () => ({ getKeyringService: () => ({}) }));
vi.mock("$lib/services/vault/vault-state.svelte", () => ({
  VaultCancelledError: class extends Error {},
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { success: vi.fn(), info: vi.fn() } }));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");

const connect = vi.fn(async (): Promise<string> => "pc-new");
const disconnect = vi.fn(async (id: string) => {
  calls.push(`disconnect ${id}`);
});

function setup() {
  const state = new DatabaseState();
  const providers = {
    getForType: async () => ({ connect, disconnect }),
  } as unknown as ProviderRegistry;
  const persistence = {
    persistConnection: vi.fn(async () => {}),
    scheduleProject: vi.fn(),
  } as unknown as PersistenceManager;
  const restoration = {
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
  return { state, manager };
}

const input = {
  name: "Via bastion",
  type: "postgres",
  host: "db.internal",
  port: 5432,
  databaseName: "app",
  username: "me",
  password: "pw",
  sshTunnel: {
    enabled: true,
    host: "bastion",
    port: 22,
    username: "me",
    authMethod: "password",
  },
  sshPassword: "ssh-pw",
} as unknown as Parameters<InstanceType<typeof ConnectionManager>["add"]>[0];

beforeEach(() => {
  calls.length = 0;
  nextTunnel = 0;
  vi.clearAllMocks();
  connect.mockResolvedValue("pc-new");
  schemaTables.mockResolvedValue([]);
});

describe("add", () => {
  it("closes the tunnel when the database connect fails", async () => {
    const { manager } = setup();
    connect.mockRejectedValueOnce(new Error("refused"));
    await expect(manager.add(input)).rejects.toThrow("refused");
    expect(calls).toEqual(["open tunnel-1", "close tunnel-1"]);
  });

  it("closes the tunnel when the schema load fails", async () => {
    const { manager, state } = setup();
    schemaTables.mockRejectedValueOnce(new Error("no schema"));
    await expect(manager.add(input)).rejects.toThrow("Failed to load database schema");
    expect(calls).toEqual(["open tunnel-1", "disconnect pc-new", "close tunnel-1"]);
    expect(state.connections).toEqual([]);
  });

  it("keeps the tunnel of a connection that connected", async () => {
    const { manager } = setup();
    await manager.add(input);
    expect(closeTunnel).not.toHaveBeenCalled();
  });
});

describe("reconnect", () => {
  async function connected() {
    const ctx = setup();
    const id = await ctx.manager.add(input);
    calls.length = 0;
    return { ...ctx, id };
  }

  it("drops the old connection and tunnel before opening the new ones", async () => {
    const { manager, id } = await connected();
    connect.mockResolvedValueOnce("pc-2");
    await manager.reconnect(id, input);
    expect(calls).toEqual(["disconnect pc-new", "close tunnel-1", "open tunnel-2"]);
  });

  it("leaves the connection disconnected, and no tunnel open, when it fails", async () => {
    const { manager, state, id } = await connected();
    connect.mockRejectedValueOnce(new Error("refused"));
    await expect(manager.reconnect(id, input)).rejects.toThrow("refused");
    expect(calls).toEqual([
      "disconnect pc-new",
      "close tunnel-1",
      "open tunnel-2",
      "close tunnel-2",
    ]);
    const conn = state.connections.find((c: DatabaseConnection) => c.id === id);
    expect(conn?.providerConnectionId).toBeUndefined();
  });
});

describe("toggle", () => {
  it("closes the tunnel after disconnecting", async () => {
    const { manager } = setup();
    const id = await manager.add(input);
    calls.length = 0;
    await manager.toggle(id);
    await vi.waitFor(() => expect(calls).toEqual(["disconnect pc-new", "close tunnel-1"]));
  });
});
