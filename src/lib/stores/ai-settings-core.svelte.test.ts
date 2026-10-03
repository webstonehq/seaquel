/**
 * Settings → AI on Core (phase 6 Task 7): the model list and the provider
 * test are `ai.models` and `ai.test` (no `fetch` from the page), and "a key
 * is saved" comes from `aiSettingsGet`'s `hasKey` on the desktop (the page
 * can't read the key, Decision 7) and from the vault's rows on web.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const env = vi.hoisted(() => ({ tauri: true, web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => env.web,
  isDemo: () => false,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
const vaultRows = vi.hoisted(() => new Set<string>());
const keyring = vi.hoisted(() => ({
  isAvailable: () => true,
  setAIApiKeyForProvider: async (id: string) => void vaultRows.add(id),
  deleteAIApiKeyForProvider: async (id: string) => void vaultRows.delete(id),
  hasAIApiKeyForProvider: async (id: string) => vaultRows.has(id),
  getAIApiKeyForProvider: vi.fn(async () => "test-key-not-real"),
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => keyring,
  aiKeyVault: () => (env.web ? keyring : null),
}));

const seq = { epoch: "e", n: 0 };
const next = () => ({ ...seq, n: ++seq.n });
let record: Record<string, unknown> = { enabled: true, providers: [] };
const settings = {
  getAiSettings: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  createAiProvider: vi.fn(async (draft: Record<string, unknown>) => {
    const id = `prov-${(record.providers as unknown[]).length + 1}`;
    record = { ...record, providers: [...(record.providers as unknown[]), { id, ...draft }] };
    return { value: { id, settings: structuredClone(record) }, seq: next() };
  }),
  updateAiProvider: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  removeAiProvider: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  patchAiSettings: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  aiProviderHasKey: vi.fn(async (id: string) => ({ value: keychain.has(id), seq: next() })),
};
/** The desktop keychain's providers with a key, as Core answers `aiProviderHasKey`. */
const keychain = new Set<string>();
vi.mock("$lib/hooks/database/library/index", () => ({ getSettings: () => settings }));
vi.mock("./settings-sync", () => ({ onStoredChange: () => {} }));

const { AISettingsStore } = await import("./ai-settings.svelte");
const { setAi } = await import("$lib/hooks/database/ai/index");
const { FakeAi } = await import("$lib/hooks/database/ai/testing");
const { CoreCallError } = await import("$lib/storage/rust-client");

let fetchSpy: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  env.tauri = true;
  env.web = false;
  vaultRows.clear();
  record = { enabled: true, providers: [] };
  fetchSpy = vi.fn();
  vi.stubGlobal("fetch", fetchSpy);
});

describe("the model list and the provider test go through Core", () => {
  it("models and test call ai.models and ai.test, never fetch", async () => {
    const ai = new FakeAi();
    const asked: string[] = [];
    ai.modelsAnswer = async (id) => (asked.push(`models:${id}`), ["m-1", "m-2"]);
    ai.testAnswer = async (id) => void asked.push(`test:${id}`);
    setAi(ai);
    const store = new AISettingsStore();
    expect(await store.fetchModels("prov-1")).toEqual(["m-1", "m-2"]);
    expect(await store.testConnection("prov-1")).toBe(true);
    expect(asked).toEqual(["models:prov-1", "test:prov-1"]);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("a refused test is false and a refused list is empty", async () => {
    const ai = new FakeAi();
    ai.modelsAnswer = async () => {
      throw new CoreCallError({
        code: "NO_API_KEY",
        message: "No API key is set for this provider.",
      });
    };
    ai.testAnswer = async () => {
      throw new CoreCallError({ code: "PROVIDER_ERROR", message: "Unauthorized" });
    };
    setAi(ai);
    const store = new AISettingsStore();
    expect(await store.fetchModels("prov-1")).toEqual([]);
    expect(await store.testConnection("prov-1")).toBe(false);
  });
});

describe("hasKey", () => {
  it("desktop: one aiProviderHasKey for the provider asked about, never a key read", async () => {
    record = {
      enabled: true,
      providers: [
        { id: "prov-1", name: "A", type: "anthropic" },
        { id: "prov-2", name: "B", type: "openai-compatible" },
      ],
    };
    keychain.clear();
    keychain.add("prov-1");
    const store = new AISettingsStore();
    await store.initialize();
    // Loading the settings asks about no key (review I2).
    expect(settings.aiProviderHasKey).not.toHaveBeenCalled();
    expect(await store.hasKey("prov-1")).toBe(true);
    expect(settings.aiProviderHasKey).toHaveBeenCalledTimes(1);
    expect(settings.aiProviderHasKey).toHaveBeenLastCalledWith("prov-1");
    expect(await store.hasKey("prov-2")).toBe(false);
    expect(settings.aiProviderHasKey).toHaveBeenCalledTimes(2);
    expect(keyring.getAIApiKeyForProvider).not.toHaveBeenCalled();
  });

  it("desktop: a refused or failed read says no key is known", async () => {
    settings.aiProviderHasKey.mockRejectedValueOnce(
      new CoreCallError({ code: "SECRET_UNREADABLE", message: "locked" }),
    );
    const store = new AISettingsStore();
    expect(await store.hasKey("prov-1")).toBe(false);
  });

  it("web: whether the vault holds a row, without decrypting it or asking Core", async () => {
    env.tauri = false;
    env.web = true;
    record = { enabled: true, providers: [{ id: "prov-1", name: "A", type: "anthropic" }] };
    const store = new AISettingsStore();
    await store.initialize();
    expect(await store.hasKey("prov-1")).toBe(false);
    vaultRows.add("prov-1");
    expect(await store.hasKey("prov-1")).toBe(true);
    expect(settings.aiProviderHasKey).not.toHaveBeenCalled();
    expect(keyring.getAIApiKeyForProvider).not.toHaveBeenCalled();
  });
});
