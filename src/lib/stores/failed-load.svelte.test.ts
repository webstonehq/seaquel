/**
 * Never save what failed to load: stores whose save writes a whole record
 * or replaces a whole table refuse to save after their load failed.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

const calls: string[] = [];
let failLoads = true;

vi.mock("$lib/storage", () => {
  const repo = (name: string) =>
    new Proxy(
      {},
      {
        get: (_t, method: string) =>
          vi.fn(async () => {
            calls.push(`${name}.${method}`);
            if (/^(load|get)/.test(method)) {
              if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
              return method.startsWith("loadUser") || method === "loadAll" ? [] : null;
            }
            return undefined;
          }),
      },
    );
  const storage = new Proxy({}, { get: (_t, name: string) => repo(name) });
  return { getStorage: () => storage };
});
const toasts: string[] = [];
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
const keyring: string[] = [];
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    setAIApiKeyForProvider: vi.fn(async () => keyring.push("set")),
    deleteAIApiKeyForProvider: vi.fn(async () => keyring.push("delete")),
    deleteLicenseKey: vi.fn(async () => keyring.push("deleteLicense")),
    setLicenseKey: vi.fn(async () => keyring.push("setLicense")),
    getLicenseKey: vi.fn(async () => null),
  }),
}));
vi.mock("@tauri-apps/plugin-os", () => ({ hostname: async () => "host" }));
vi.mock("$lib/api/tauri", () => ({
  activateLicense: vi.fn(async () => ({ status: "active", tier: "individual" })),
  validateLicense: vi.fn(),
  deactivateLicense: vi.fn(),
  getUsername: async () => "me",
}));
vi.mock("$lib/themes/apply", () => ({ applyTheme: vi.fn(), cacheThemeColors: vi.fn() }));
vi.mock("mode-watcher", () => ({ mode: { current: "light" } }));

beforeEach(() => {
  calls.length = 0;
  toasts.length = 0;
  keyring.length = 0;
  failLoads = true;
  vi.resetModules();
});

const writes = () => calls.filter((c) => !/\.(load|get)/.test(c));

describe("AI settings", () => {
  it("refuses a change after a failed load, without touching settings or keys", async () => {
    const { aiSettingsStore } = await import("./ai-settings.svelte");
    await aiSettingsStore.initialize(); // doesn't throw
    const provider = { id: "p1", name: "Mine", type: "anthropic" as const, baseUrl: "" };

    await expect(aiSettingsStore.addProvider(provider, "sk-key")).rejects.toThrow();
    await expect(aiSettingsStore.setEnabled(false)).rejects.toThrow();
    await expect(
      aiSettingsStore.savePrivacySettings({ shareSchemaGlobally: false, shareDataGlobally: false }),
    ).rejects.toThrow();

    expect(writes()).toEqual([]);
    expect(keyring).toEqual([]);
    expect(aiSettingsStore.settings.providers).toEqual([]);
  });

  it("loads again before a change, and saves once that works", async () => {
    const { aiSettingsStore } = await import("./ai-settings.svelte");
    await aiSettingsStore.initialize();
    failLoads = false;

    await aiSettingsStore.setEnabled(false);

    expect(writes()).toEqual(["appState.set"]);
  });
});

describe("themes", () => {
  it("doesn't replace the user themes after a failed load", async () => {
    const { themeStore } = await import("./theme.svelte");
    await themeStore.initialize();
    expect(themeStore.isLoaded).toBe(true); // built-in themes still apply

    themeStore.addTheme({ name: "New", isDark: false, colors: {} as never });
    themeStore.flush();
    await Promise.resolve();

    expect(writes()).toEqual([]);
    expect(toasts).toHaveLength(1);
  });

  it("saves after a successful load", async () => {
    failLoads = false;
    const { themeStore } = await import("./theme.svelte");
    await themeStore.initialize();

    themeStore.addTheme({ name: "New", isDark: false, colors: {} as never });
    themeStore.flush();
    await vi.waitFor(() => expect(writes()).toContain("themes.saveUserThemes"));
  });
});

describe("onboarding", () => {
  it("doesn't overwrite the stored state after a failed load, and retries the load", async () => {
    const { onboardingStore } = await import("./onboarding.svelte");
    await onboardingStore.initialize();

    onboardingStore.dismissHint("h1");
    await Promise.resolve();
    expect(writes()).toEqual([]);

    failLoads = false;
    calls.length = 0;
    await onboardingStore.initialize();
    expect(calls).toContain("onboarding.load");
  });
});

describe("license", () => {
  it("refuses to activate or deactivate before the server or the keychain is touched", async () => {
    const { licenseStore } = await import("./license.svelte");
    const { NotLoadedError } = await import("$lib/storage/load-guard");
    const licenseApi = await import("$lib/api/tauri");
    await licenseStore.initialize();

    await expect(licenseStore.activate("KEY-123")).rejects.toBeInstanceOf(NotLoadedError);
    await expect(licenseStore.deactivate()).rejects.toBeInstanceOf(NotLoadedError);

    expect(licenseApi.activateLicense).not.toHaveBeenCalled();
    expect(licenseApi.deactivateLicense).not.toHaveBeenCalled();
    expect(keyring).toEqual([]);
    expect(writes()).toEqual([]);
  });

  it("never writes the license record while it isn't loaded", async () => {
    const { licenseStore } = await import("./license.svelte");
    await licenseStore.initialize();

    // Any path that reaches `persist` (revalidation, marking invalid, …).
    await (licenseStore as unknown as { persist(): Promise<void> }).persist();

    expect(writes()).toEqual([]);
  });

  it("retries the load instead of treating a failure as loaded", async () => {
    const { licenseStore } = await import("./license.svelte");
    await licenseStore.initialize();
    calls.length = 0;
    await licenseStore.initialize();
    expect(calls).toContain("license.load");
    expect(writes()).toEqual([]);
  });
});
