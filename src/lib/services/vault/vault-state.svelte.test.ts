import { describe, it, expect, vi } from "vitest";

const saved: unknown[] = [];
/** What `vaultState.load` answers: `null` throws, as an unreachable server. */
const stored = vi.hoisted(() => ({ row: null as unknown }));
vi.mock("$lib/storage", () => ({
  getStorage: () => ({
    vaultState: {
      load: vi.fn(async () => {
        if (stored.row === null) throw new Error("STORAGE_ERROR: upstream unavailable");
        return stored.row;
      }),
      save: vi.fn(async (row: unknown) => saved.push(row)),
    },
  }),
}));

const { Vault } = await import("./vault-state.svelte");

describe("Vault after a failed vault_state read", () => {
  it("refresh() resolves and leaves the status unknown", async () => {
    const vault = new Vault();
    await expect(vault.refresh()).resolves.toBeUndefined();
    expect(vault.status).toBe("unknown");
  });

  it("setup() refuses rather than replacing a vault it couldn't read", async () => {
    const vault = new Vault();
    await expect(vault.setup("passphrase")).rejects.toThrow();
    expect(saved).toEqual([]);
  });
});

describe("Whether an unlock is announced (probe F1)", () => {
  /** A vault set up with `passphrase`, its row served by the mock. */
  async function lockedVault(passphrase: string) {
    const { DEFAULT_KDF_PARAMS } = await import("./crypto");
    // A cheap KDF: the test checks who waited, not argon2's cost.
    const params = { ...DEFAULT_KDF_PARAMS, t: 1, m: 64, p: 1 };
    const setup = new Vault();
    // No row yet: `setup` creates one.
    stored.row = undefined;
    await setup.setup(passphrase, params);
    stored.row = saved.at(-1);
    const vault = new Vault();
    await vault.refresh();
    expect(vault.status).toBe("locked");
    return vault;
  }

  it("a send's quiet wait isn't announced", async () => {
    const vault = await lockedVault("correct horse");
    const waiting = vault.ensureUnlocked({ quiet: true });
    expect(vault.waitersPending).toBe(true);
    await expect(vault.unlock("correct horse")).resolves.toEqual({ announce: false });
    await expect(waiting).resolves.toBeDefined();
  });

  it("any other waiter is announced", async () => {
    const vault = await lockedVault("correct horse");
    const quiet = vault.ensureUnlocked({ quiet: true });
    const loud = vault.ensureUnlocked();
    await expect(vault.unlock("correct horse")).resolves.toEqual({ announce: true });
    await Promise.all([quiet, loud]);
  });
});
