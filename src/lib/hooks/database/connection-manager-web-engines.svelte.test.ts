/**
 * Decision 11b: on web, a SQLite or DuckDB connection (saved on desktop, or
 * typed in) fails in `ConnectionManager` with the reason, before anything
 * reaches the server.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseConnection } from "$lib/types";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => true,
  isDemo: () => false,
}));
vi.mock("$lib/services/ssh-tunnel", () => ({
  createSshTunnelWithHostKeyCheck: vi.fn(),
  closeSshTunnel: vi.fn(),
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: async () => [] }),
  TsEngineClient: class {},
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false, isUnlocked: () => false }),
}));
vi.mock("$lib/services/vault/vault-state.svelte", () => ({
  VaultCancelledError: class extends Error {},
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
const errorToast = vi.fn();
vi.mock("$lib/utils/toast", () => ({ errorToast: (msg: string) => errorToast(msg) }));
vi.mock("svelte-sonner", () => ({ toast: { success: vi.fn(), info: vi.fn() } }));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");

const connect = vi.fn(async (): Promise<string> => "pc-new");
const test = vi.fn(async () => {});
const disconnect = vi.fn(async () => {});

function setup() {
  const state = new DatabaseState();
  const providers = {
    getForType: async () => ({ connect, test, disconnect }),
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

type Input = Parameters<InstanceType<typeof ConnectionManager>["add"]>[0];

function fileInput(type: "sqlite" | "duckdb"): Input {
  return {
    name: `${type} file`,
    type,
    host: "",
    port: 0,
    databaseName: type === "sqlite" ? "/data/auth.db" : ":memory:",
    username: "",
    password: "",
    connectionString: type === "sqlite" ? "sqlite:/data/auth.db" : undefined,
  } as Input;
}

const message = (label: string) =>
  `${label} connections aren't available in the web app, because they would open files on the server. Use the desktop app for ${label}.`;

beforeEach(() => {
  vi.clearAllMocks();
});

describe.each([
  ["sqlite", "SQLite"],
  ["duckdb", "DuckDB"],
] as const)("%s on web", (type, label) => {
  it("add refuses without calling the provider", async () => {
    const { manager, state } = setup();
    await expect(manager.add(fileInput(type))).rejects.toThrow(message(label));
    expect(connect).not.toHaveBeenCalled();
    expect(state.connections).toEqual([]);
  });

  it("test refuses without calling the provider", async () => {
    const { manager } = setup();
    await expect(manager.test(fileInput(type))).rejects.toThrow(message(label));
    expect(test).not.toHaveBeenCalled();
  });

  it("a saved connection's reconnect refuses and leaves it listed", async () => {
    const { manager, state } = setup();
    const saved = { ...fileInput(type), id: "saved-1", projectId: "p", labelIds: [] };
    state.connections.push(saved as unknown as DatabaseConnection);
    await expect(manager.reconnect("saved-1", fileInput(type))).rejects.toThrow(message(label));
    expect(connect).not.toHaveBeenCalled();
    expect(state.connections.map((c) => c.id)).toEqual(["saved-1"]);
    expect(manager.connectingIds.has("saved-1")).toBe(false);
  });

  it("autoReconnect returns false and says why", async () => {
    const { manager, state } = setup();
    const saved = { ...fileInput(type), id: "saved-1", projectId: "p", labelIds: [] };
    state.connections.push(saved as unknown as DatabaseConnection);
    await expect(manager.autoReconnect("saved-1")).resolves.toBe(false);
    expect(errorToast).toHaveBeenCalledWith(message(label));
    expect(connect).not.toHaveBeenCalled();
  });
});

it("postgres still connects on web", async () => {
  const { manager } = setup();
  await manager.add({
    name: "pg",
    type: "postgres",
    host: "db",
    port: 5432,
    databaseName: "app",
    username: "u",
    password: "p",
  } as Input);
  expect(connect).toHaveBeenCalledOnce();
});
