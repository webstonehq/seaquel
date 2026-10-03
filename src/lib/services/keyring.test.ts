/**
 * The keyrings and AI keys (phase 6, Decision 7): on the desktop Core reads
 * a provider's key from the keychain itself and the `secret` group refuses
 * `ai-api-key:*`, so the page has no way to read one (`aiKeyVault()` is
 * `null`, and the keyring has no AI key read). On web the vault holds it,
 * and the page sends it with each `ai` call (Task 7).
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const callSecret = vi.fn();
const env = vi.hoisted(() => ({ web: false }));

vi.mock("$lib/storage/rust-client", () => ({ callSecret }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => !env.web,
  isWeb: () => env.web,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

describe("the desktop keyring", () => {
  beforeEach(() => {
    callSecret.mockReset();
    env.web = false;
    vi.resetModules();
  });

  it("has no AI key read, and no vault of AI keys", async () => {
    const { getKeyringService, aiKeyVault } = await import("./keyring");
    const keyring = getKeyringService() as unknown as Record<string, unknown>;
    expect(keyring.getAIApiKeyForProvider).toBeUndefined();
    expect(aiKeyVault()).toBeNull();
    expect(callSecret).not.toHaveBeenCalled();
  });

  it("still reads a database password", async () => {
    const { getKeyringService } = await import("./keyring");
    callSecret.mockResolvedValue("pw");
    expect(await getKeyringService().getDbPassword("c1")).toBe("pw");
    expect(callSecret).toHaveBeenCalledWith({ method: "get", params: { key: "db:c1" } });
  });
});

describe("the web keyring", () => {
  beforeEach(() => {
    env.web = true;
    vi.resetModules();
  });

  it("is the vault of AI keys", async () => {
    const { getKeyringService, aiKeyVault } = await import("./keyring");
    expect(aiKeyVault()).toBe(getKeyringService());
  });
});
