/**
 * Never save what failed to load: the license, whose save writes a whole
 * record, refuses to save after its load failed. The settings stores moved
 * to targeted `settings` calls in phase 5d-2 (Decision 20): a change there
 * replaces nothing, so it is sent after a failed load too.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

const calls: string[] = [];
let failLoads = true;

/** The `settings` group (5d-2): every call recorded as `settings.method`. */
vi.mock("$lib/hooks/database/library/index", () => {
  let n = 0;
  const answer = (method: string, args: unknown[]): unknown => {
    switch (method) {
      case "getAiSettings":
      case "patchAiSettings":
      case "updateAiProvider":
      case "removeAiProvider":
        return {
          enabled: true,
          providers: [],
          shareSchemaGlobally: true,
          shareDataGlobally: false,
        };
      case "createAiProvider":
        return {
          id: "prov-1",
          settings: { providers: [{ id: "prov-1", ...(args[0] as object) }] },
        };
      case "getThemes":
      case "setThemePreferences":
      case "updateUserTheme":
      case "removeUserTheme":
        return {
          preferences: { lightThemeId: "default-light", darkThemeId: "default-dark" },
          userThemes: [],
        };
      case "createUserTheme":
        return {
          id: "theme-1",
          themes: {
            preferences: { lightThemeId: "default-light", darkThemeId: "default-dark" },
            userThemes: [{ id: "theme-1", ...(args[0] as object) }],
          },
        };
      case "getOnboarding":
      case "patchOnboarding":
        return {};
      default:
        return null;
    }
  };
  const settings = new Proxy(
    {},
    {
      get: (_t, method: string) =>
        vi.fn(async (...args: unknown[]) => {
          calls.push(`settings.${method}`);
          if (/^(get|list)/.test(method) && failLoads) {
            throw new Error("STORAGE_ERROR: upstream unavailable");
          }
          return { value: answer(method, args), seq: { epoch: "e", n: ++n } };
        }),
    },
  );
  return { getSettings: () => settings };
});

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
// Onboarding is saved on the desktop only.
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => true,
  isWeb: () => false,
  isDemo: () => false,
}));
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
  it("a change after a failed load is one targeted call, without touching keys", async () => {
    const { aiSettingsStore } = await import("./ai-settings.svelte");
    await aiSettingsStore.initialize(); // doesn't throw

    await aiSettingsStore.addProvider({ name: "Mine", type: "anthropic" });
    await aiSettingsStore.setEnabled(false);
    await aiSettingsStore.savePrivacySettings({
      shareSchemaGlobally: false,
      shareDataGlobally: false,
    });

    // Core rewrites the record from its stored copy: nothing is replaced.
    expect(writes()).toEqual([
      "settings.createAiProvider",
      "settings.patchAiSettings",
      "settings.patchAiSettings",
    ]);
    expect(keyring).toEqual([]);
  });
});

describe("themes", () => {
  it("a theme added after a failed load is its own row, nothing else is written", async () => {
    const { themeStore } = await import("./theme.svelte");
    await themeStore.initialize();
    expect(themeStore.isLoaded).toBe(true); // built-in themes still apply

    const added = await themeStore.addTheme({ name: "New", isDark: false, colors: {} as never });

    expect(writes()).toEqual(["settings.createUserTheme"]);
    expect(added?.id).toBe("theme-1");
    expect(toasts).toEqual([]);
  });
});

describe("onboarding", () => {
  it("a change after a failed load is a patch of its fields, and the load is retried", async () => {
    const { onboardingStore } = await import("./onboarding.svelte");
    await onboardingStore.initialize();

    onboardingStore.dismissHint("h1");
    await vi.waitFor(() => expect(writes()).toEqual(["settings.patchOnboarding"]));

    failLoads = false;
    calls.length = 0;
    await onboardingStore.initialize();
    expect(calls).toContain("settings.getOnboarding");
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
