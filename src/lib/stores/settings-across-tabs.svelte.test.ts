/**
 * The settings stores on the `settings` group (phase 5d-2, Decision 20),
 * against Core's `settings` group in the browser module (phase 8, the
 * demo's Core).
 * Each tab is a fresh set of store modules on one database; another tab's
 * write reaches a tab as Core's event does, through `applyStoredChange`
 * (what `LibrarySync` calls for the settings kinds).
 *
 * - Targeted writes: two tabs adding providers or themes keep both.
 * - Another tab's change applies at once, themes included (Q18).
 * - A setter called before the store's load waits for it, and the load
 *   doesn't overwrite what the setter set (re-survey bug 23).
 * - On the desktop an AI key goes with the Core call, never to the
 *   keychain from TypeScript.
 */
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { loadTestModule, testModuleMissing, type TestModule } from "$lib/core/browser/testing/node";
import { openModuleCore } from "$lib/core/browser/testing/meta";

const env = vi.hoisted(() => ({ tauri: false, web: false, applied: [] as string[] }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => env.web,
  isDemo: () => !env.tauri && !env.web,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
const toasts: string[] = [];
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/themes/apply", () => ({
  applyTheme: (theme: { id: string }) => env.applied.push(theme.id),
  cacheThemeColors: () => {},
}));
vi.mock("mode-watcher", () => ({ mode: { current: "light" } }));
const keychain = vi.hoisted(() => [] as string[]);
const vault = vi.hoisted(() => ({ fail: false }));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({
    isAvailable: () => true,
    setAIApiKeyForProvider: async (id: string) => {
      if (vault.fail) throw new Error("vault locked");
      keychain.push(`set:${id}`);
    },
    deleteAIApiKeyForProvider: async (id: string) => void keychain.push(`delete:${id}`),
    getAIApiKeyForProvider: async () => null,
  }),
}));
vi.mock("./license.svelte.js", () => ({ licenseStore: { status: "personal" } }));

const { CoreSettings } = await import("$lib/hooks/database/library/core-settings");
import type { SettingsService } from "$lib/hooks/database/library/types";

const missing = testModuleMissing();
let module: TestModule | null = null;
let settings: SettingsService;

beforeAll(async () => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  module = await loadTestModule();
});

afterAll(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

beforeEach(async () => {
  vault.fail = false;
  env.tauri = false;
  env.web = false;
  env.applied.length = 0;
  keychain.length = 0;
  toasts.length = 0;
  if (!module) return;
  const client = (await openModuleCore(module)).storage();
  settings = new CoreSettings(() => client);
});

/** One tab: fresh store modules over the shared settings service. */
async function openTab(service: SettingsService = settings) {
  vi.resetModules();
  const library = await import("$lib/hooks/database/library/index");
  library.setSettings(service);
  const [ai, theme, editor, pending, sync, tutorial, onboarding] = await Promise.all([
    import("./ai-settings.svelte.js"),
    import("./theme.svelte.js"),
    import("./editor-settings.svelte.js"),
    import("./pending-changes-settings.svelte.js"),
    import("./settings-sync.js"),
    import("./tutorial-progress.svelte.js"),
    import("./onboarding.svelte.js"),
  ]);
  const limits = await import("./version-limits.svelte.js");
  return {
    ai: ai.aiSettingsStore,
    aiModule: ai,
    theme: theme.themeStore,
    editor: editor.editorSettingsStore,
    pending: pending.pendingChangesSettingsStore,
    tutorial: tutorial.tutorialProgressStore,
    onboarding: onboarding.onboardingStore,
    limits: new limits.VersionLimitsStore(),
    /** Another tab's write reaching this one (Core's `storageChanged`). */
    changed: sync.applyStoredChange,
  };
}

describe.skipIf(missing)("settings across tabs", () => {
  it("a theme added in one tab appears in the other", async () => {
    const one = await openTab();
    const two = await openTab();
    await one.theme.initialize();
    await two.theme.initialize();

    const added = (await one.theme.addTheme({
      name: "Paper",
      isDark: false,
      colors: { background: "#ffffff" } as never,
    }))!;
    expect(one.theme.userThemes.map((t) => t.id)).toEqual([added.id]);

    await two.changed("theme", [added.id]);
    expect(two.theme.userThemes.map((t) => t.name)).toEqual(["Paper"]);
  });

  it("a theme changed in one tab applies in the other", async () => {
    const one = await openTab();
    const two = await openTab();
    await one.theme.initialize();
    await two.theme.initialize();
    const added = (await one.theme.addTheme({
      name: "Paper",
      isDark: false,
      colors: { background: "#ffffff" } as never,
    }))!;
    await one.theme.setLightTheme(added.id);

    env.applied.length = 0;
    await two.changed("theme", null);
    expect(two.theme.preferences.lightThemeId).toBe(added.id);
    expect(env.applied).toEqual([added.id]);

    // Removing it in the first tab puts the second back on the default.
    await one.theme.deleteTheme(added.id);
    await two.changed("theme", [added.id]);
    expect(two.theme.preferences.lightThemeId).toBe("default-light");
    expect(two.theme.userThemes).toEqual([]);
  });

  it("two tabs adding providers keep both", async () => {
    const one = await openTab();
    const two = await openTab();
    await one.ai.initialize();
    await two.ai.initialize();

    await one.ai.addProvider({ name: "First", type: "anthropic" });
    await two.ai.addProvider({ name: "Second", type: "anthropic" });
    await one.changed("aiSettings", null);

    const names = (s: typeof one) => s.ai.settings.providers.map((p) => p.name);
    expect(names(one)).toEqual(["First", "Second"]);
    expect(names(two)).toEqual(["First", "Second"]);
  });

  it("an AI key goes with the Core call on the desktop, not to the keychain", async () => {
    env.tauri = true;
    const calls: unknown[][] = [];
    const recording = new Proxy(settings, {
      get(target, method: string) {
        const fn = (target as unknown as Record<string, (...a: unknown[]) => unknown>)[method];
        if (method !== "createAiProvider" && method !== "updateAiProvider") return fn.bind(target);
        return async (...args: unknown[]) => {
          calls.push([method, ...args]);
          // The module's Core has no keychain and refuses keys (as web's
          // does); Core takes them on the desktop.
          const [first, second] = args;
          return method === "createAiProvider"
            ? fn.call(target, first)
            : fn.call(target, first, second);
        };
      },
    });
    const tab = await openTab(recording);
    await tab.ai.initialize();
    const id = await tab.ai.addProvider({ name: "Claude", type: "anthropic" }, "sk-1");
    await tab.ai.updateProvider({ id, name: "Claude", type: "anthropic" }, "");
    expect(calls).toEqual([
      ["createAiProvider", { name: "Claude", type: "anthropic" }, "sk-1"],
      ["updateAiProvider", id, {}, null],
    ]);
    expect(keychain).toEqual([]);
  });

  it("settings from another tab apply at once", async () => {
    const one = await openTab();
    const two = await openTab();
    await Promise.all([
      one.editor.load(),
      two.editor.load(),
      one.pending.load(),
      two.pending.load(),
    ]);

    await one.editor.setKeybindingMode("vim");
    await one.pending.setEnabled(false);
    await two.changed("setting", ["editorKeybindingMode", "pending_changes_enabled"]);

    expect(two.editor.keybindingMode).toBe("vim");
    expect(two.pending.enabled).toBe(false);

    // A setting cleared elsewhere reads as its default.
    await settings.setSetting("editorKeybindingMode", null);
    await two.changed("setting", ["editorKeybindingMode"]);
    expect(two.editor.keybindingMode).toBe("default");
  });

  it("a setting set while its load is on its way isn't undone by the load", async () => {
    await settings.setSetting("editorKeybindingMode", "emacs");
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const slow = new Proxy(settings, {
      get(target, method: string) {
        const fn = (target as unknown as Record<string, (...a: unknown[]) => unknown>)[method];
        if (method !== "getSetting") return fn.bind(target);
        return async (...args: unknown[]) => {
          await held;
          return fn.apply(target, args);
        };
      },
    });
    const tab = await openTab(slow);
    const loading = tab.editor.load();
    const setting = tab.editor.setKeybindingMode("vim");
    release();
    await Promise.all([loading, setting]);

    // The load didn't put back what it read, and the write came after it.
    expect(tab.editor.keybindingMode).toBe("vim");
    expect((await settings.getSetting("editorKeybindingMode")).value).toBe("vim");
  });

  it("a reload that starts while a set is on its way doesn't undo it", async () => {
    await settings.setSetting("editorKeybindingMode", "emacs");
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const slow = gated(settings, "setSetting", () => held);
    const tab = await openTab(slow);
    await tab.editor.load();

    const setting = tab.editor.setKeybindingMode("vim");
    // Another tab's change lands meanwhile, and this tab reads it.
    await settings.setSetting("editorKeybindingMode", "default");
    await tab.changed("setting", ["editorKeybindingMode"]);
    release();
    await setting;

    expect(tab.editor.keybindingMode).toBe("vim");
  });

  it("an older read is ignored", async () => {
    await settings.setSetting("editorKeybindingMode", "emacs");
    let release!: () => void;
    let armed = false;
    const late = gatedAfter(settings, "getSetting", () =>
      armed ? new Promise<void>((r) => (release = r)) : Promise.resolve(),
    );
    const tab = await openTab(late);
    await tab.editor.load();

    armed = true;
    const reloading = tab.changed("setting", ["editorKeybindingMode"]); // reads "emacs", held
    armed = false;
    await tab.editor.setKeybindingMode("vim");
    release();
    await reloading;

    expect(tab.editor.keybindingMode).toBe("vim");
  });

  it("another tab's newer value read while a set is on its way shows once it answers", async () => {
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    // Tab A's set commits, but its answer is held.
    const slow = gatedAfter(settings, "setSetting", () => held);
    const tab = await openTab(slow);
    await tab.editor.load();

    const setting = tab.editor.setKeybindingMode("vim");
    await vi.waitFor(async () =>
      expect((await settings.getSetting("editorKeybindingMode")).value).toBe("vim"),
    );
    // Tab B's value commits after A's, and A reads it while its set is on its way.
    await settings.setSetting("editorKeybindingMode", "emacs");
    await tab.changed("setting", ["editorKeybindingMode"]);
    release();
    await setting;

    await vi.waitFor(() => expect(tab.editor.keybindingMode).toBe("emacs"));
  });

  it("tutorial progress saved during a reload stays", async () => {
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const slow = gated(settings, "saveTutorial", () => held);
    const tab = await openTab(slow);
    await tab.tutorial.initialize();

    const completing = tab.tutorial.completeChallenge("select-basics", "c1");
    // Another tab's progress lands meanwhile, and this tab reads it.
    await settings.saveTutorial("joins", "c1", null);
    await tab.changed("tutorial", null);
    release();
    await completing;

    expect(tab.tutorial.isChallengeCompleted("select-basics", "c1")).toBe(true);
    // The other tab's progress, read while the save was on its way, shows too.
    await vi.waitFor(() => expect(tab.tutorial.isChallengeCompleted("joins", "c1")).toBe(true));
  });

  it("another tab's earlier progress shows after this tab's later save", async () => {
    const one = await openTab();
    const two = await openTab();
    await one.tutorial.initialize();
    await two.tutorial.initialize();
    // Tab B saves first; tab A hasn't read it when it saves its own.
    await two.tutorial.completeChallenge("joins", "c1");
    await one.tutorial.completeChallenge("select-basics", "c1");
    await one.changed("tutorial", null);
    expect(one.tutorial.isChallengeCompleted("joins", "c1")).toBe(true);
  });

  it("a refused setting shows the stored value again", async () => {
    await settings.setSetting("editorKeybindingMode", "emacs");
    const tab = await openTab();
    await tab.editor.load();
    vi.spyOn(settings, "setSetting").mockRejectedValueOnce(new Error("STORAGE_FULL: full"));
    await tab.editor.setKeybindingMode("vim");
    await vi.waitFor(() => expect(tab.editor.keybindingMode).toBe("emacs"));
  });

  it("a theme picked twice quickly doesn't flicker back", async () => {
    const gates: (() => void)[] = [];
    const slow = gated(
      settings,
      "setThemePreferences",
      () => new Promise<void>((r) => gates.push(r)),
    );
    const tab = await openTab(slow);
    await tab.theme.initialize();

    const first = tab.theme.setLightTheme("nord-light");
    const second = tab.theme.setLightTheme("default-light");
    gates[0]();
    await first;
    // The first answer landed; the second choice is still what shows.
    expect(tab.theme.preferences.lightThemeId).toBe("default-light");
    await vi.waitFor(() => expect(gates).toHaveLength(2));
    gates[1]();
    await second;
    expect(tab.theme.preferences.lightThemeId).toBe("default-light");
  });

  it("onboarding changed twice quickly doesn't flicker back", async () => {
    env.tauri = true;
    const gates: (() => void)[] = [];
    const slow = gated(settings, "patchOnboarding", () => new Promise<void>((r) => gates.push(r)));
    const tab = await openTab(slow);
    await tab.onboarding.initialize();

    tab.onboarding.setLearnEnabled(false);
    tab.onboarding.setShowWizardHints(false);
    tab.onboarding.setLearnEnabled(true);
    await vi.waitFor(() => expect(gates).toHaveLength(3));
    // The first patch's answer (learnEnabled false) lands after the third change.
    gates[0]();
    await new Promise((r) => setTimeout(r, 0));
    expect(tab.onboarding.learnEnabled).toBe(true);
    gates[1]();
    gates[2]();
    await vi.waitFor(async () =>
      expect(
        ((await settings.getOnboarding()).value as { learnEnabled: boolean }).learnEnabled,
      ).toBe(true),
    );
    expect(tab.onboarding.learnEnabled).toBe(true);
    expect(tab.onboarding.showWizardHints).toBe(false);
  });

  it("a refused onboarding change is taken back by reading the record again", async () => {
    env.tauri = true;
    const tab = await openTab();
    await tab.onboarding.initialize();
    vi.spyOn(settings, "patchOnboarding").mockRejectedValueOnce(new Error("STORAGE_FULL: full"));

    tab.onboarding.setLearnEnabled(false);
    await vi.waitFor(() => expect(tab.onboarding.learnEnabled).toBe(true));
  });

  it("web: a provider whose key the vault didn't take is kept, and says so", async () => {
    env.web = true;
    vault.fail = true;
    const tab = await openTab();
    await tab.ai.initialize();

    const error = await tab.ai
      .addProvider({ name: "Claude", type: "anthropic" }, "sk-1")
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(tab.aiModule.ProviderKeyNotSavedError);
    const id = (error as { providerId: string }).providerId;
    expect(tab.ai.settings.providers.map((p) => p.id)).toEqual([id]);

    // The form edits it from here: a retry stores the key, no second provider.
    vault.fail = false;
    await tab.ai.updateProvider({ id, name: "Claude", type: "anthropic" }, "sk-1");
    expect(tab.ai.settings.providers).toHaveLength(1);
    expect(keychain).toEqual([`set:${id}`]);
  });
});

describe.skipIf(missing)("the theme editor's save", () => {
  it("says whether a theme change was stored", async () => {
    const tab = await openTab();
    await tab.theme.initialize();
    const added = (await tab.theme.addTheme({
      name: "Paper",
      isDark: false,
      colors: {} as never,
    }))!;
    expect(await tab.theme.updateTheme(added.id, { name: "Paper 2" })).toBe(true);
    vi.spyOn(settings, "updateUserTheme").mockRejectedValueOnce(new Error("STORAGE_FULL: full"));
    expect(await tab.theme.updateTheme(added.id, { name: "Paper 3" })).toBe(false);
  });
});

describe.skipIf(missing)("the version limits", () => {
  it("another tab's limit applies, and a save before the read answers isn't undone", async () => {
    await settings.setSetting("query_version_limit", "50");
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const slow = gatedAfter(settings, "getSetting", () => held);
    const tab = await openTab(slow);

    const loading = tab.limits.load();
    await tab.limits.save("query_version_limit", 20);
    release();
    await loading;
    expect(tab.limits.query).toBe(20);

    await settings.setSetting("dashboard_version_limit", "30");
    await tab.changed("setting", ["dashboard_version_limit"]);
    expect(tab.limits.dashboard).toBe(30);
  });
});

/** `service` with `method` waiting for `gate()` before it runs. */
function gated(
  service: SettingsService,
  method: string,
  gate: () => Promise<void>,
): SettingsService {
  return new Proxy(service, {
    get(target, name: string) {
      const fn = (target as unknown as Record<string, (...a: unknown[]) => unknown>)[name];
      if (name !== method) return fn.bind(target);
      return async (...args: unknown[]) => {
        await gate();
        return fn.apply(target, args);
      };
    },
  });
}

/** `service` with `method` answering only after `gate()` (it has run already). */
function gatedAfter(
  service: SettingsService,
  method: string,
  gate: () => Promise<void>,
): SettingsService {
  return new Proxy(service, {
    get(target, name: string) {
      const fn = (target as unknown as Record<string, (...a: unknown[]) => unknown>)[name];
      if (name !== method) return fn.bind(target);
      return async (...args: unknown[]) => {
        const wait = gate();
        const answer = await fn.apply(target, args);
        await wait;
        return answer;
      };
    },
  });
}
