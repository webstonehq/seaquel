/**
 * 5d-2 Task 1: stores that saved what they never loaded, or dropped what
 * they did load (re-survey bugs 15–17).
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

const calls: { call: string; args: unknown[] }[] = [];
let failLoads = false;
let tutorialRows: { lessonId: string; challengeId: string; state: string | null }[] = [];
let tauri = true;

// The `settings` group (5d-2): every call recorded as `settings.method`.
vi.mock("$lib/hooks/database/library/index", () => {
  let n = 0;
  const settings = new Proxy(
    {},
    {
      get: (_t, method: string) =>
        vi.fn(async (...args: unknown[]) => {
          calls.push({ call: `settings.${method}`, args });
          if (/^(get|list)/.test(method) && failLoads) {
            throw new Error("STORAGE_ERROR: upstream unavailable");
          }
          const value =
            method === "listTutorial" ? tutorialRows : method.startsWith("get") ? null : {};
          return { value, seq: { epoch: "e", n: ++n } };
        }),
    },
  );
  return { getSettings: () => settings };
});
const toasts: string[] = [];
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
vi.mock("$lib/utils/environment", () => ({ isTauri: () => tauri, isDemo: () => false }));
vi.mock("./license.svelte.js", () => ({ licenseStore: { status: "personal" } }));

const writes = () => calls.filter((c) => !/\.(get|list)/.test(c.call)).map((c) => c.call);

beforeEach(() => {
  calls.length = 0;
  toasts.length = 0;
  failLoads = false;
  tutorialRows = [];
  tauri = true;
  vi.resetModules();
});

describe("onboarding", () => {
  it("onboarding on web writes nothing and shows no toast", async () => {
    tauri = false;
    const { resetLoadGuardToast } = await import("$lib/storage/load-guard");
    resetLoadGuardToast();
    const { onboardingStore } = await import("./onboarding.svelte.js");
    // The web layout never initialises the store (desktop only).
    onboardingStore.completeWizard();
    onboardingStore.setLearnEnabled(false);
    await Promise.resolve();

    expect(writes()).toEqual([]);
    expect(toasts).toEqual([]);
    // The in-memory choice still applies for the session.
    expect(onboardingStore.learnEnabled).toBe(false);
  });

  it("on desktop a loaded store still saves", async () => {
    const { onboardingStore } = await import("./onboarding.svelte.js");
    await onboardingStore.initialize();
    onboardingStore.completeWizard();
    await Promise.resolve();
    await vi.waitFor(() => expect(writes()).toEqual(["settings.patchOnboarding"]));
  });
});

describe("the license nudge", () => {
  it("the license nudge's answer isn't saved after a failed load", async () => {
    failLoads = true;
    const { licenseNudgeStore } = await import("./license-nudge.svelte.js");
    await licenseNudgeStore.initialize();

    licenseNudgeStore.respond("work");
    licenseNudgeStore.respond("personal");
    licenseNudgeStore.snooze();
    await Promise.resolve();

    expect(writes()).toEqual([]);
  });
});

describe("tutorial progress", () => {
  it("one bad tutorial row keeps the others", async () => {
    tutorialRows = [
      { lessonId: "l1", challengeId: "c1", state: '{"tables":[]}' },
      { lessonId: "l1", challengeId: "c2", state: "{not json" },
      { lessonId: "l2", challengeId: "c1", state: null },
    ];
    const { tutorialProgressStore } = await import("./tutorial-progress.svelte.js");
    await tutorialProgressStore.initialize();

    expect(tutorialProgressStore.isChallengeCompleted("l1", "c1")).toBe(true);
    expect(tutorialProgressStore.isChallengeCompleted("l2", "c1")).toBe(true);
    expect(tutorialProgressStore.getChallengeState("l1", "c1")).toEqual({ tables: [] });
    // The row says the challenge was completed; only its unreadable state is skipped.
    expect(tutorialProgressStore.isChallengeCompleted("l1", "c2")).toBe(true);
    expect(tutorialProgressStore.getChallengeState("l1", "c2")).toBeUndefined();

    // Resetting a lesson still reaches storage.
    await tutorialProgressStore.resetLesson("l1");
    expect(writes()).toEqual(["settings.removeTutorialLesson"]);
  });
});
