import { describe, it, expect, vi, beforeEach } from "vitest";

const storage = new Map<string, string | null>();

vi.mock("$lib/storage", () => ({
  getDatabase: vi.fn(async () => ({})),
  appStateRepo: {
    get: vi.fn(async (_db: unknown, key: string) => storage.get(key) ?? null),
    set: vi.fn(async (_db: unknown, key: string, value: string | null) => {
      storage.set(key, value);
    }),
  },
}));

const license = { status: "personal" };
vi.mock("./license.svelte.js", () => ({ licenseStore: license }));

async function freshStore(persisted?: object) {
  storage.clear();
  if (persisted) storage.set("license_nudge", JSON.stringify(persisted));
  vi.resetModules();
  const { licenseNudgeStore } = await import("./license-nudge.svelte.js");
  await licenseNudgeStore.initialize();
  return licenseNudgeStore;
}

const base = {
  queryCount: 0,
  activeDays: 0,
  lastActiveDay: null,
  answer: null,
  snoozedUntil: null,
};

describe("licenseNudgeStore", () => {
  beforeEach(() => {
    license.status = "personal";
  });

  it("stays hidden for new users", async () => {
    const store = await freshStore();
    store.recordQuery();
    expect(store.shouldShow).toBe(false);
  });

  it("shows once the query threshold is reached", async () => {
    const store = await freshStore({ ...base, queryCount: 99 });
    expect(store.shouldShow).toBe(false);
    store.recordQuery();
    expect(store.shouldShow).toBe(true);
    expect(store.milestone).toEqual({ kind: "queries", count: 100 });
  });

  it("shows once the active-days threshold is reached", async () => {
    const store = await freshStore({
      ...base,
      queryCount: 20,
      activeDays: 13,
      lastActiveDay: "2000-01-01",
    });
    store.recordQuery();
    expect(store.activeDays).toBe(14);
    expect(store.shouldShow).toBe(true);
    expect(store.milestone).toEqual({ kind: "days", count: 14 });
  });

  it("counts each calendar day only once", async () => {
    const store = await freshStore();
    store.recordQuery();
    store.recordQuery();
    expect(store.queryCount).toBe(2);
    expect(store.activeDays).toBe(1);
  });

  it("never shows to licensed users", async () => {
    license.status = "active";
    const store = await freshStore({ ...base, queryCount: 1000 });
    expect(store.shouldShow).toBe(false);
  });

  it("never shows again once answered personal, and persists the answer", async () => {
    const store = await freshStore({ ...base, queryCount: 1000 });
    store.respond("personal");
    expect(store.shouldShow).toBe(false);
    await vi.waitFor(() =>
      expect(JSON.parse(storage.get("license_nudge")!).answer).toBe("personal"),
    );

    const reloaded = await freshStore(JSON.parse(storage.get("license_nudge")!));
    expect(reloaded.shouldShow).toBe(false);
  });

  it("schedules a reminder 30 days after answering work", async () => {
    const store = await freshStore({ ...base, queryCount: 1000 });
    store.respond("work");
    expect(store.shouldShow).toBe(false);
    expect(store.isWorkReminder).toBe(true);
    const days = (new Date(store.snoozedUntil!).getTime() - Date.now()) / 86_400_000;
    expect(Math.round(days)).toBe(30);
  });

  it("shows the work reminder when due, and snoozing defers it another 30 days", async () => {
    const due = await freshStore({
      ...base,
      queryCount: 1000,
      answer: "work",
      snoozedUntil: new Date(Date.now() - 1000).toISOString(),
    });
    expect(due.shouldShow).toBe(true);
    due.snooze();
    expect(Math.round((new Date(due.snoozedUntil!).getTime() - Date.now()) / 86_400_000)).toBe(30);
  });

  it("stops reminding work users once licensed", async () => {
    license.status = "active";
    const store = await freshStore({
      ...base,
      queryCount: 1000,
      answer: "work",
      snoozedUntil: null,
    });
    expect(store.shouldShow).toBe(false);
  });

  it("hides while snoozed", async () => {
    const store = await freshStore({ ...base, queryCount: 1000 });
    store.snooze();
    expect(store.shouldShow).toBe(false);
  });

  it("returns after the snooze expires", async () => {
    const expired = await freshStore({
      ...base,
      queryCount: 1000,
      snoozedUntil: new Date(Date.now() - 1000).toISOString(),
    });
    expect(expired.shouldShow).toBe(true);
  });
});
