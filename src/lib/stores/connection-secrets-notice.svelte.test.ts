import { beforeEach, describe, expect, it, vi } from "vitest";

const stored = vi.hoisted(() => ({ value: null as string | null, sets: [] as unknown[] }));
vi.mock("$lib/storage", () => ({
  getStorage: () => ({
    appState: {
      get: async () => stored.value,
      set: async (key: string, value: string | null) => {
        stored.sets.push([key, value]);
        stored.value = value;
      },
    },
  }),
}));
vi.mock("$lib/utils/logger", () => ({ log: { warn: vi.fn() } }));

const { connectionSecretsNotice, parseNoticeIds } =
  await import("./connection-secrets-notice.svelte");

beforeEach(() => {
  stored.value = null;
  stored.sets = [];
  connectionSecretsNotice.open = false;
  connectionSecretsNotice.names = [];
});

describe("the connection secrets notice (Decision 12a)", () => {
  it("names the listed connections once, and clears the list when dismissed", async () => {
    stored.value = JSON.stringify(["c1", "c2"]);
    await connectionSecretsNotice.check((id) => ({ c1: "Prod" })[id]);
    expect(connectionSecretsNotice.open).toBe(true);
    expect(connectionSecretsNotice.names).toEqual(["Prod"]);
    expect(stored.sets).toEqual([]);
    await connectionSecretsNotice.dismiss();
    expect(stored.sets).toEqual([["connectionStringSecretsNotice", null]]);
    await connectionSecretsNotice.check(() => "Prod");
    expect(connectionSecretsNotice.open).toBe(false);
  });

  it("clears a list whose connections are all gone", async () => {
    stored.value = JSON.stringify(["gone"]);
    await connectionSecretsNotice.check(() => undefined);
    expect(connectionSecretsNotice.open).toBe(false);
    expect(stored.value).toBeNull();
  });

  it("reads only a list of strings", () => {
    expect(parseNoticeIds('["a", 1, "b"]')).toEqual(["a", "b"]);
    expect(parseNoticeIds("{")).toEqual([]);
    expect(parseNoticeIds(null)).toEqual([]);
  });
});
