/**
 * Regression tests for `persistConnection`'s keychain handling.
 *
 * Metadata-only saves (label changes, AI model selection) call it without
 * `options`. That used to mean "no secrets should be saved", which deleted the
 * user's stored passwords and cleared the save flags — the user only found out
 * at the next launch when auto-reconnect failed.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { DatabaseConnection } from "$lib/types";

const saved: Array<Record<string, unknown>> = [];
const keyringCalls: string[] = [];

vi.mock("$lib/storage", () => ({
  getDatabase: vi.fn(async () => ({})),
  connectionsRepo: {
    save: vi.fn(async (_db: unknown, connection: Record<string, unknown>) => {
      saved.push(connection);
    }),
  },
  projectsRepo: {},
  appStateRepo: {},
  projectStateRepo: {},
  savedQueriesRepo: {},
  queryHistoryRepo: {},
  queryVersionsRepo: {},
  sharedReposRepo: {},
  dashboardsRepo: {},
  dashboardVersionsRepo: {},
  connectionOverridesRepo: {},
  aiChatsRepo: {},
}));

vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    isAvailable: () => true,
    setDbPassword: vi.fn(async () => keyringCalls.push("setDbPassword")),
    deleteDbPassword: vi.fn(async () => keyringCalls.push("deleteDbPassword")),
    setSshPassword: vi.fn(async () => keyringCalls.push("setSshPassword")),
    deleteSshPassword: vi.fn(async () => keyringCalls.push("deleteSshPassword")),
    setSshKeyPassphrase: vi.fn(async () => keyringCalls.push("setSshKeyPassphrase")),
    deleteSshKeyPassphrase: vi.fn(async () => keyringCalls.push("deleteSshKeyPassphrase")),
  }),
}));

const connection = (overrides: Partial<DatabaseConnection> = {}): DatabaseConnection =>
  ({
    id: "conn-1",
    name: "Local",
    type: "postgres",
    host: "localhost",
    port: 5432,
    databaseName: "app",
    username: "postgres",
    password: "hunter2",
    projectId: "proj-1",
    labelIds: [],
    savePassword: true,
    saveSshPassword: true,
    saveSshKeyPassphrase: true,
    ...overrides,
  }) as DatabaseConnection;

// Imported once up front: pulling it in inside the first test makes that test
// pay the whole transform cost and time out.
const { PersistenceManager } = await import("./persistence-manager.svelte.js");

function manager() {
  return new PersistenceManager({} as never);
}

describe("persistConnection", () => {
  beforeEach(() => {
    saved.length = 0;
    keyringCalls.length = 0;
  });

  it("keeps stored secrets and save flags when called without options", async () => {
    const pm = manager();
    await pm.persistConnection(connection());

    expect(keyringCalls).toEqual([]);
    expect(saved[0]).toMatchObject({
      savePassword: true,
      saveSshPassword: true,
      saveSshKeyPassphrase: true,
    });
  });

  it("deletes a secret only when its flag is explicitly false", async () => {
    const pm = manager();
    await pm.persistConnection(connection(), {
      savePassword: false,
      saveSshPassword: true,
      sshPassword: "ssh-secret",
    });

    expect(keyringCalls).toContain("deleteDbPassword");
    expect(keyringCalls).toContain("setSshPassword");
    // saveSshKeyPassphrase was not mentioned at all, so the stored one stays.
    expect(keyringCalls).not.toContain("deleteSshKeyPassphrase");
    expect(saved[0]).toMatchObject({ savePassword: false, saveSshPassword: true });
  });

  it("saves the db password when asked", async () => {
    const pm = manager();
    await pm.persistConnection(connection(), { savePassword: true });

    expect(keyringCalls).toContain("setDbPassword");
    expect(keyringCalls).not.toContain("deleteDbPassword");
  });
});
