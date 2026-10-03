/**
 * F4 re-review M-a: a stored password the vault can't decrypt (a reset
 * vault, a corrupt row) isn't "no password". `getDbPassword` keeps
 * answering null for it, as callers that re-prompt expect; the strict
 * getter auto-reconnect uses throws `VaultEntryUnreadableError`, so a row
 * that saves its password is never dialled without it.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const rows = new Map<string, { nonce: string; ciphertext: string }>();
vi.mock("$lib/storage", () => ({
  getStorage: () => ({
    userCredentials: {
      load: async (scope: string, key: string) => rows.get(`${scope}:${key}`) ?? null,
    },
  }),
}));
const decryptToString = vi.fn(async (): Promise<string> => "secret");
vi.mock("./crypto", () => ({
  decryptToString: () => decryptToString(),
  encrypt: vi.fn(),
  fromBase64: () => new Uint8Array(),
  toBase64: () => "",
}));

const { VaultKeyringService, VaultEntryUnreadableError } = await import("./vault-keyring");

const vault = { ensureUnlocked: async () => ({}) as CryptoKey } as never;

beforeEach(() => {
  rows.clear();
  decryptToString.mockReset();
  decryptToString.mockResolvedValue("secret");
});

describe("getDbPasswordStrict", () => {
  it("answers null when no password is stored (trust auth connects)", async () => {
    const keyring = new VaultKeyringService(vault);
    expect(await keyring.getDbPasswordStrict("conn-1")).toBeNull();
  });

  it("answers the password it can decrypt", async () => {
    rows.set("db:conn-1", { nonce: "n", ciphertext: "c" });
    const keyring = new VaultKeyringService(vault);
    expect(await keyring.getDbPasswordStrict("conn-1")).toBe("secret");
  });

  it("throws for a stored password it can't decrypt; the plain getter still answers null", async () => {
    rows.set("db:conn-1", { nonce: "n", ciphertext: "c" });
    decryptToString.mockRejectedValue(new Error("OperationError"));
    const keyring = new VaultKeyringService(vault);
    await expect(keyring.getDbPasswordStrict("conn-1")).rejects.toBeInstanceOf(
      VaultEntryUnreadableError,
    );
    expect(await keyring.getDbPassword("conn-1")).toBeNull();
  });
});
