/**
 * Settings → AI in the demo (phase 6 Task 8, Q2 B): the visitor's key is
 * kept in this page's memory for the session, never sent to Core with the
 * provider, never in storage, and sent with each `ai` call naming its
 * provider (`CoreAi`'s vault is the page's session keys). A reload (a new
 * module graph) forgets it. The key is the fake test key.
 */
import { afterAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { CoreClient, StreamRequest } from "$lib/core/client";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";

const TEST_KEY = "test-key-not-real";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => true,
  isBrowser: () => true,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const seq = { epoch: "e", n: 0 };
const next = () => ({ ...seq, n: ++seq.n });
let record: Record<string, unknown> = { enabled: true, providers: [] };
const sent: unknown[][] = [];
const settings = {
  getAiSettings: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  createAiProvider: vi.fn(async (...args: unknown[]) => {
    sent.push(args);
    const draft = args[0] as Record<string, unknown>;
    const id = `prov-${(record.providers as unknown[]).length + 1}`;
    record = { ...record, providers: [...(record.providers as unknown[]), { id, ...draft }] };
    return { value: { id, settings: structuredClone(record) }, seq: next() };
  }),
  updateAiProvider: vi.fn(async (...args: unknown[]) => {
    sent.push(args);
    return { value: structuredClone(record), seq: next() };
  }),
  removeAiProvider: vi.fn(async (id: string) => {
    record = {
      ...record,
      providers: (record.providers as { id: string }[]).filter((p) => p.id !== id),
    };
    return { value: structuredClone(record), seq: next() };
  }),
  patchAiSettings: vi.fn(async () => ({ value: structuredClone(record), seq: next() })),
  aiProviderHasKey: vi.fn(async () => {
    throw new Error("the demo has no secret store");
  }),
};
vi.mock("$lib/hooks/database/library/index", () => ({ getSettings: () => settings }));
vi.mock("$lib/stores/settings-sync", () => ({ onStoredChange: () => {} }));

/** Storage the test can search: whatever the page writes lands here. */
class SearchableStorage {
  readonly map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, String(value));
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  clear() {
    this.map.clear();
  }
  text() {
    return JSON.stringify([...this.map.entries()]);
  }
}
const local = new SearchableStorage();
const session = new SearchableStorage();

/** A client that records each request it is sent. */
function recordingClient() {
  const requests: unknown[] = [];
  const client = {
    call: vi.fn(async (req: CoreRequest) => {
      requests.push(req);
      const inner = (req as { params: { method: string } }).params.method;
      return {
        method: req.method,
        result: { method: inner, result: inner === "models" ? [] : null },
      };
    }),
    stream: vi.fn((req: StreamRequest) => {
      requests.push(req);
      return (async function* () {
        yield { type: "done", messages: [], seq: { epoch: "e", n: 1 }, stop: "end" };
      })();
    }),
    events: () => () => {},
    onResubscribed: () => () => {},
    onEventsUnavailable: () => () => {},
  } as unknown as CoreClient;
  return { client, requests };
}

/** One page load: a fresh module graph, as a reload gives. */
async function page() {
  vi.resetModules();
  const store = await import("./ai-settings.svelte");
  const keyring = await import("$lib/services/keyring");
  const { CoreAi } = await import("$lib/hooks/database/ai/core-ai");
  return { store: new store.AISettingsStore(), keyring, CoreAi };
}

beforeEach(() => {
  vi.stubEnv("VITE_BUILD_TARGET", "demo");
  vi.stubGlobal("localStorage", local);
  vi.stubGlobal("sessionStorage", session);
  record = { enabled: true, providers: [] };
  sent.length = 0;
  local.clear();
  session.clear();
});

afterAll(() => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
});

describe("the demo's session key", () => {
  it("offers the assistant in the demo", async () => {
    const { store } = await page();
    await store.initialize();
    expect(store.available).toBe(true);
  });

  it("keeps the key in page memory, never sends it with the provider, and sends it with each call", async () => {
    const { store, keyring, CoreAi } = await page();
    const id = await store.addProvider(
      { name: "Mock", type: "openai-compatible", baseUrl: "http://127.0.0.1:1/v1" },
      TEST_KEY,
    );
    // Core stores the provider without its key.
    expect(JSON.stringify(sent)).not.toContain(TEST_KEY);
    expect(sent[0][1]).toBeUndefined();
    expect(await store.hasKey(id)).toBe(true);
    // CoreAi's vault is the session's keys.
    const vault = keyring.aiKeyVault();
    expect(vault).not.toBeNull();
    expect(await vault!.getAIApiKeyForProvider(id)).toBe(TEST_KEY);

    const { client, requests } = recordingClient();
    const ai = new CoreAi(() => client);
    for await (const _ of ai.chat({
      streamId: "t-1",
      chatId: "chat-1",
      connectionId: "c-1",
      userMessage: { id: "u-1", content: "Hi" },
      assistantMessageId: "a-1",
      approval: "ask",
      clientTools: true,
      providerId: id,
    })) {
      // drain
    }
    await ai.models(id);
    await ai.test(id);
    expect(requests).toHaveLength(3);
    const chat = requests[0] as { params: { params: { apiKey?: string; providerId?: string } } };
    expect(chat.params.params).toMatchObject({ apiKey: TEST_KEY, providerId: id });
    for (const req of requests.slice(1)) {
      expect((req as { params: { params: unknown } }).params.params).toEqual({
        providerId: id,
        apiKey: TEST_KEY,
      });
    }
    // Nothing reached storage.
    expect(local.text()).not.toContain(TEST_KEY);
    expect(session.text()).not.toContain(TEST_KEY);
  });

  it("a changed key replaces it, a cleared key and a removed provider forget it", async () => {
    const { store, keyring } = await page();
    const id = await store.addProvider({ name: "Mock", type: "anthropic" }, TEST_KEY);
    await store.updateProvider(store.getProvider(id)!, `${TEST_KEY}-2`);
    expect(await keyring.aiKeyVault()!.getAIApiKeyForProvider(id)).toBe(`${TEST_KEY}-2`);
    expect(JSON.stringify(sent)).not.toContain(TEST_KEY);
    await store.updateProvider(store.getProvider(id)!, "");
    expect(await store.hasKey(id)).toBe(false);

    await store.updateProvider(store.getProvider(id)!, TEST_KEY);
    await store.deleteProvider(id);
    expect(await store.hasKey(id)).toBe(false);
  });

  it("a reload forgets the key; the provider stays", async () => {
    const first = await page();
    const id = await first.store.addProvider({ name: "Mock", type: "anthropic" }, TEST_KEY);
    expect(await first.store.hasKey(id)).toBe(true);

    const second = await page();
    await second.store.initialize();
    expect(second.store.getProvider(id)?.name).toBe("Mock");
    expect(await second.store.hasKey(id)).toBe(false);
    const { client, requests } = recordingClient();
    await new second.CoreAi(() => client).test(id);
    expect(JSON.stringify(requests)).not.toContain(TEST_KEY);
  });
});
