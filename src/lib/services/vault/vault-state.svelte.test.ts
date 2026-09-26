import { describe, it, expect, vi } from "vitest";

const saved: unknown[] = [];
vi.mock("$lib/storage", () => ({
  getStorage: () => ({
    vaultState: {
      load: vi.fn(async () => {
        throw new Error("STORAGE_ERROR: upstream unavailable");
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
