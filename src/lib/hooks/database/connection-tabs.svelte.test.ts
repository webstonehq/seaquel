/**
 * Opening a saved connection for editing fills the form from the connection,
 * including its AI sharing overrides, so saving the form writes back what
 * the user saw instead of the global default.
 */
import { describe, expect, it, vi } from "vitest";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    isAvailable: () => false,
    getDbPassword: async () => null,
    getSshPassword: async () => null,
    getSshKeyPassphrase: async () => null,
  }),
}));

const { ConnectionTabManager } = await import("./connection-tabs.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p1";
  const tabOrdering = { add: vi.fn(), remove: vi.fn() } as unknown as TabOrderingManager;
  const manager = new ConnectionTabManager(
    state,
    tabOrdering,
    () => {},
    () => {},
  );
  return { state, manager };
}

const saved = {
  id: "conn-1",
  name: "Local",
  type: "postgres" as const,
  host: "localhost",
  port: 5432,
  databaseName: "app",
  username: "me",
  savePassword: false,
};

function formOf(state: InstanceType<typeof DatabaseState>) {
  return state.connectionTabsByProject["p1"]?.[0]?.formData;
}

describe("open in edit mode", () => {
  it("loads the connection's AI sharing overrides", async () => {
    const { state, manager } = setup();
    await manager.open({ ...saved, aiShareSchema: false, aiShareData: true }, "edit");
    expect(formOf(state)).toMatchObject({ aiShareSchema: false, aiShareData: true });
  });

  it("leaves them undefined when the connection follows the global setting", async () => {
    const { state, manager } = setup();
    await manager.open(saved, "edit");
    const form = formOf(state);
    expect(form?.aiShareSchema).toBeUndefined();
    expect(form?.aiShareData).toBeUndefined();
  });
});

describe("the wizard's SSL mode", () => {
  it('starts on "Default" (no mode) for a new connection', async () => {
    const { state, manager } = setup();
    await manager.open();
    expect(formOf(state)?.sslMode).toBe("");
  });

  it("keeps a saved row's mode", async () => {
    const { state, manager } = setup();
    await manager.open({ ...saved, sslMode: "disable" }, "edit");
    expect(formOf(state)?.sslMode).toBe("disable");
  });

  it('leaves a saved row without a mode on "Default"', async () => {
    const { state, manager } = setup();
    await manager.open(saved, "edit");
    expect(formOf(state)?.sslMode).toBe("");
  });
});

describe("the connection string on prefill", () => {
  it("drops the string the old builder made from the row's fields", async () => {
    const { state, manager } = setup();
    await manager.open(
      {
        ...saved,
        sslMode: "disable",
        connectionString: "postgresql://me@localhost/app?sslmode=disable",
      },
      "edit",
    );
    expect(formOf(state)?.connectionString).toBe("");
  });

  it("keeps, and so shows, a hand-typed string with parameters", async () => {
    const { state, manager } = setup();
    const typed = "postgresql://me@localhost/app?application_name=seaquel";
    await manager.open({ ...saved, connectionString: typed }, "edit");
    expect(formOf(state)?.connectionString).toBe(typed);
  });
});
