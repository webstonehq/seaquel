/**
 * `ConnectionManager` and DuckDB support's install on desktop (desktop
 * DuckDB helper plan, Task 5): every interactive connect path (`add`,
 * `reconnect`, `autoReconnect` that isn't background, `test`, the file
 * drop) that meets `ENGINE_NOT_INSTALLED` opens the dialog once and sends
 * the same request again after the install; background reconnects never
 * ask; a decline adds no toast. Also Decision 6's GUI half (Test of a
 * file a connected DuckDB connection holds answers at once) and Task 2's
 * M3 (a DuckDB connection whose helper died isn't reconnected quietly).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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

const request = vi.hoisted(() => vi.fn(async (): Promise<boolean> => true));
const showUnusable = vi.hoisted(() => vi.fn());
vi.mock("$lib/stores/duckdb-install.svelte", () => ({
  duckdbInstallStore: { request: () => request(), showUnusable },
}));

const schemaTables = vi.fn(async () => [] as unknown[]);
const errorToast = vi.fn();
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: () => schemaTables() }),
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => true }),
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({
  errorToast: (msg: string) => errorToast(msg),
}));
vi.mock("svelte-sonner", () => ({
  toast: { success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { CoreCallError } = await import("$lib/storage/rust-client");
const { setLibrary } = await import("./library/index");
const { RecordingLibrary } = await import("./library/recording-library");
const { DuckdbHelperDeclined } = await import("$lib/core/duckdb-helper");
const { m } = await import("$lib/paraglide/messages.js");
const core = await import("$lib/core");
const { reportConnectionNotFound } = await import("$lib/core/connection-watch");
const { handleFileDrop } = await import("$lib/services/file-drop.svelte");

const notInstalled = () =>
  new CoreCallError({
    code: "ENGINE_NOT_INSTALLED",
    message: "DuckDB support for Seaquel 2026.10.1 isn't installed: missing",
  });

const connect = vi.fn(async (_request: ConnectRequest): Promise<string> => "pc-new");
const test = vi.fn(async (_request: ConnectRequest): Promise<void> => {});
const disconnect = vi.fn(async (_id: string) => {});
const bindSaved = vi.fn(async (_id: string, _savedId: string) => {});

let library: InstanceType<typeof RecordingLibrary>;

function setup() {
  const state = new DatabaseState();
  const providers = {
    getForType: async () => ({ connect, test, disconnect, bindSaved }),
  } as unknown as ProviderRegistry;
  const manager = new ConnectionManager(
    state,
    { scheduleProject: vi.fn() } as unknown as WindowStateManager,
    {
      loadConnectionData: vi.fn(async () => {}),
      initializeConnectionMaps: vi.fn(),
      cleanupConnectionMaps: vi.fn(),
      ensureConnectionMapsExist: vi.fn(),
    } as unknown as StateRestorationManager,
    {} as TabOrderingManager,
    providers,
    vi.fn(async () => {}),
    vi.fn(),
  );
  state.activeProjectId = "p1";
  return { state, manager };
}

const duckFile = {
  id: "conn-1",
  name: "Warehouse",
  type: "duckdb",
  host: "",
  port: 0,
  databaseName: "/data/warehouse.duckdb",
  username: "",
  password: "",
  projectId: "p1",
  labelIds: [],
  isLocalOnly: true,
} as unknown as DatabaseConnection;

/** A shown connection that is stored too (the library has its row). */
function showSaved(
  state: InstanceType<typeof DatabaseState>,
  connection: DatabaseConnection = duckFile,
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

const duckInput = {
  name: "Warehouse",
  type: "duckdb",
  host: "",
  port: 0,
  databaseName: "/data/warehouse.duckdb",
  username: "",
  password: "typed-pw",
  createIfMissing: true,
} as unknown as Input;

/** Connect refuses once with ENGINE_NOT_INSTALLED, then connects. */
function refuseOnce() {
  connect.mockRejectedValueOnce(notInstalled()).mockResolvedValueOnce("pc-new");
}

beforeEach(() => {
  library = new RecordingLibrary();
  library.seedProject("p1");
  setLibrary(library);
  env.web = false;
  vi.clearAllMocks();
  connect.mockReset().mockResolvedValue("pc-new");
  test.mockReset().mockResolvedValue(undefined);
  schemaTables.mockResolvedValue([]);
  request.mockResolvedValue(true);
});

describe("the connect paths ask once and send the same request again", () => {
  it("add", async () => {
    const { manager } = setup();
    refuseOnce();
    await manager.add(duckInput);
    expect(request).toHaveBeenCalledOnce();
    expect(connect).toHaveBeenCalledTimes(2);
    const [first, second] = connect.mock.calls.map(([r]) => r);
    expect(second).toEqual(first);
    expect(second).toMatchObject({ secrets: { db: "typed-pw" }, createIfMissing: true });
  });

  it("reconnect, with the saved connection's id", async () => {
    const { manager, state } = setup();
    showSaved(state);
    refuseOnce();
    await manager.reconnect("conn-1", duckInput);
    expect(request).toHaveBeenCalledOnce();
    const [first, second] = connect.mock.calls.map(([r]) => r);
    expect(second).toEqual(first);
    expect(second).toMatchObject({ savedConnectionId: "conn-1", secrets: { db: "typed-pw" } });
    expect(state.connections[0].providerConnectionId).toBe("pc-new");
  });

  it("autoReconnect the user asked for", async () => {
    const { manager, state } = setup();
    showSaved(state);
    refuseOnce();
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(true);
    expect(request).toHaveBeenCalledOnce();
    const [first, second] = connect.mock.calls.map(([r]) => r);
    expect(second).toEqual(first);
    expect(second).toMatchObject({ target: { type: "saved", id: "conn-1" } });
  });

  it("test", async () => {
    const { manager } = setup();
    test.mockRejectedValueOnce(notInstalled()).mockResolvedValueOnce(undefined);
    await manager.test(duckInput);
    expect(request).toHaveBeenCalledOnce();
    expect(test).toHaveBeenCalledTimes(2);
    expect(test.mock.calls[1][0]).toEqual(test.mock.calls[0][0]);
  });

  it("the file drop of a CSV asks, then runs the query", async () => {
    const { manager, state } = setup();
    refuseOnce();
    const execute = vi.fn(async () => {});
    const addTab = vi.fn(() => "tab-1");
    const db = {
      state,
      connections: manager,
      queryTabs: { add: addTab },
      queries: { execute },
    };
    await handleFileDrop(["/tmp/sales.csv"], db as never);
    expect(request).toHaveBeenCalledOnce();
    expect(connect).toHaveBeenCalledTimes(2);
    expect(addTab).toHaveBeenCalledWith("sales.csv", "SELECT * FROM read_csv('/tmp/sales.csv')");
    expect(execute).toHaveBeenCalledWith("tab-1");
    expect(errorToast).not.toHaveBeenCalled();
  });
});

describe("what never asks", () => {
  it("a background reconnect gets the error without a dialog", async () => {
    const { manager, state } = setup();
    showSaved(state);
    connect.mockRejectedValue(notInstalled());
    await expect(manager.autoReconnect("conn-1", { background: true })).resolves.toBe(false);
    expect(request).not.toHaveBeenCalled();
    expect(connect).toHaveBeenCalledOnce();
  });

  it("the connection tab's own auto-connect", async () => {
    const { manager, state } = setup();
    showSaved(state);
    connect.mockRejectedValue(notInstalled());
    await expect(
      manager.reconnect("conn-1", duckInput, undefined, { askToInstall: false }),
    ).rejects.toThrow("DuckDB support for Seaquel 2026.10.1 isn't installed: missing");
    expect(request).not.toHaveBeenCalled();
  });

  it("another engine", async () => {
    const { manager } = setup();
    connect.mockRejectedValue(notInstalled());
    await expect(
      manager.add({ ...duckInput, type: "postgres", host: "h" } as Input),
    ).rejects.toThrow("ENGINE_NOT_INSTALLED: ");
    expect(request).not.toHaveBeenCalled();
  });
});

describe("declining", () => {
  it("rejects add with DuckdbHelperDeclined, worded for the wizard, and toasts nothing", async () => {
    const { manager, state } = setup();
    connect.mockRejectedValue(notInstalled());
    request.mockResolvedValue(false);
    const error = await manager.add(duckInput).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DuckdbHelperDeclined);
    expect((error as Error).message).toBe(m.duckdb_helper_declined());
    expect(connect).toHaveBeenCalledOnce();
    expect(state.connections).toEqual([]);
    expect(errorToast).not.toHaveBeenCalled();
  });

  it("autoReconnect answers false, as for any failure", async () => {
    const { manager, state } = setup();
    showSaved(state);
    connect.mockRejectedValue(notInstalled());
    request.mockResolvedValue(false);
    await expect(manager.autoReconnect("conn-1")).resolves.toBe(false);
    expect(errorToast).not.toHaveBeenCalled();
  });

  it("a dropped data file toasts nothing more", async () => {
    const { manager, state } = setup();
    connect.mockRejectedValue(notInstalled());
    request.mockResolvedValue(false);
    const db = {
      state,
      connections: manager,
      queryTabs: { add: vi.fn() },
      queries: { execute: vi.fn() },
    };
    await handleFileDrop(["/tmp/sales.csv"], db as never);
    expect(errorToast).not.toHaveBeenCalled();
  });

  it("one decline ends the drop's DuckDB work: two .duckdb files and a CSV ask once (review I1)", async () => {
    const { manager, state } = setup();
    connect.mockRejectedValue(notInstalled());
    request.mockResolvedValue(false);
    const execute = vi.fn();
    const db = {
      state,
      connections: manager,
      queryTabs: { add: vi.fn(() => "tab-1") },
      queries: { execute },
    };
    await handleFileDrop(["/tmp/a.duckdb", "/tmp/b.duckdb", "/tmp/sales.csv"], db as never);
    expect(request).toHaveBeenCalledOnce();
    expect(connect).toHaveBeenCalledOnce();
    expect(execute).not.toHaveBeenCalled();
    expect(errorToast).not.toHaveBeenCalled();
  });
});

// Decision 6's GUI half: Test on a DuckDB file that a connected
// connection holds would be refused by Core ("already open"); it is open
// and working, so Test answers at once. The page compares paths, Core
// files: a path the page doesn't match falls back to Core's answer.
describe("Test of a file a connected DuckDB connection holds", () => {
  it("answers success without calling Core", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    await expect(manager.test(duckInput)).resolves.toBeUndefined();
    expect(test).not.toHaveBeenCalled();
  });

  it("asks Core when the form's connection string differs from the row's (review M1)", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    await manager.test({
      ...duckInput,
      connectionString: "duckdb:///data/warehouse.duckdb?access_mode=read_only",
    } as Input);
    expect(test).toHaveBeenCalledOnce();
  });

  it("takes the shortcut when the form's connection string is the row's", async () => {
    const { manager, state } = setup();
    const connectionString = "duckdb:///data/warehouse.duckdb?threads=2";
    showSaved(state, {
      ...duckFile,
      connectionString,
      providerConnectionId: "pc-1",
    } as DatabaseConnection);
    await manager.test({ ...duckInput, connectionString } as Input);
    expect(test).not.toHaveBeenCalled();
  });

  it("asks Core when that connection isn't connected", async () => {
    const { manager, state } = setup();
    showSaved(state);
    await manager.test(duckInput);
    expect(test).toHaveBeenCalledOnce();
  });

  it("asks Core for an in-memory database", async () => {
    const { manager, state } = setup();
    showSaved(state, {
      ...duckFile,
      databaseName: ":memory:",
      providerConnectionId: "pc-1",
    } as DatabaseConnection);
    await manager.test({ ...duckInput, databaseName: ":memory:" } as Input);
    expect(test).toHaveBeenCalledOnce();
  });

  it("reports Core's answer as it is for another spelling of the same file", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    test.mockRejectedValue(
      new CoreCallError({
        code: "CONNECTION_ERROR",
        message: "This DuckDB file is already open in another connection. Disconnect it first.",
      }),
    );
    await expect(
      manager.test({ ...duckInput, databaseName: "/data/../data/warehouse.duckdb" } as Input),
    ).rejects.toThrow("This DuckDB file is already open in another connection.");
    expect(test).toHaveBeenCalledOnce();
  });
});

// Task 2's review M3 (Decision 7): a helper that dies is announced as
// CONNECTION_CLOSED and its connection taken out of Core. No quiet
// reconnect follows, whatever reports the loss first: the query that
// killed the helper may do it again.
describe("a DuckDB connection whose helper died", () => {
  let resubscribe: ((info: { initial: boolean }) => void) | null = null;
  let events: ((event: unknown) => void) | null = null;
  beforeEach(() => {
    resubscribe = null;
    events = null;
    core.setCoreClient({
      call: async () => ({ method: "db", result: { method: "alive", result: [] } }),
      stream: () => (async function* () {})(),
      events: (h: (event: unknown) => void) => {
        events = h;
        return () => {};
      },
      onResubscribed: (h: (info: { initial: boolean }) => void) => {
        resubscribe = h;
        return () => {};
      },
      onEventsUnavailable: () => () => {},
    } as never);
  });
  afterEach(() => core.setCoreClient(null));

  it("after the event, a CONNECTION_NOT_FOUND or a db.alive miss starts no connect", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    const reconnect = vi.spyOn(manager, "autoReconnect");
    const stop = manager.listenForCoreEvents();
    events!({
      type: "connectionClosed",
      connectionId: "pc-1",
      code: "CONNECTION_CLOSED",
      message: "The DuckDB helper stopped (signal 9). Reconnect to continue.",
    });
    reportConnectionNotFound("pc-1");
    resubscribe!({ initial: false });
    await new Promise((r) => setTimeout(r, 10));
    expect(reconnect).not.toHaveBeenCalled();
    expect(connect).not.toHaveBeenCalled();
    expect(state.connections[0].providerConnectionId).toBeUndefined();
    expect(errorToast).toHaveBeenCalledTimes(1);
    stop();
  });

  it("a CONNECTION_NOT_FOUND before the event shows it lost, once, and starts no connect", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    const reconnect = vi.spyOn(manager, "autoReconnect");
    const stop = manager.listenForCoreEvents();
    reportConnectionNotFound("pc-1");
    events!({
      type: "connectionClosed",
      connectionId: "pc-1",
      code: "CONNECTION_CLOSED",
      message: "The DuckDB helper stopped (signal 9). Reconnect to continue.",
    });
    await new Promise((r) => setTimeout(r, 10));
    expect(reconnect).not.toHaveBeenCalled();
    expect(connect).not.toHaveBeenCalled();
    expect(state.connections[0].providerConnectionId).toBeUndefined();
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(errorToast).toHaveBeenCalledWith(m.connection_closed_lost({ name: "Warehouse" }));
    stop();
  });

  it("a db.alive miss alone starts no connect either", async () => {
    const { manager, state } = setup();
    showSaved(state, { ...duckFile, providerConnectionId: "pc-1" } as DatabaseConnection);
    const reconnect = vi.spyOn(manager, "autoReconnect");
    const stop = manager.listenForCoreEvents();
    resubscribe!({ initial: false });
    await vi.waitFor(() => expect(state.connections[0].providerConnectionId).toBeUndefined());
    await new Promise((r) => setTimeout(r, 10));
    expect(reconnect).not.toHaveBeenCalled();
    stop();
  });
});
