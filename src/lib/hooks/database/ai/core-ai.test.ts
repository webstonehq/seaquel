/**
 * `CoreAi`: every assistant call is an `ai` request to
 * Core. On web the page sends the provider's key from the vault with each
 * call; on the desktop it sends none and never reads one (Core reads the keychain):
 * no `secret` call at all. The key is the fake
 * test key; nothing here reaches a provider.
 */
import { describe, expect, it, vi } from "vitest";
import type { CoreClient, StreamRequest } from "$lib/core/client";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import { CoreCallError } from "$lib/storage/rust-client";
import { CoreAi, type AiKeyVault } from "./core-ai";

const TEST_KEY = "test-key-not-real";

/** A client that records each request; `answer` gives each call's result. */
function fakeClient(answer: (req: CoreRequest) => unknown = () => null) {
  const calls: CoreRequest[] = [];
  const streams: StreamRequest[] = [];
  const end: AiEvent = { type: "done", messages: [], seq: { epoch: "e", n: 1 }, stop: "end" };
  const client = {
    call: vi.fn(async (req: CoreRequest) => {
      calls.push(req);
      const inner = (req as { params: { method: string } }).params.method;
      return { method: req.method, result: { method: inner, result: answer(req) } };
    }),
    stream: vi.fn((req: StreamRequest) => {
      streams.push(req);
      return (async function* () {
        yield end;
      })();
    }),
    events: () => () => {},
    onResubscribed: () => () => {},
    onEventsUnavailable: () => () => {},
  } as unknown as CoreClient;
  return { client, calls, streams };
}

/** The web's vault: `keys` holds a provider's key; reads are counted. */
function vault(keys: Record<string, string>) {
  const reads: string[] = [];
  const v: AiKeyVault = {
    hasAIApiKeyForProvider: async (id) => id in keys,
    getAIApiKeyForProvider: async (id) => {
      reads.push(id);
      return keys[id] ?? null;
    },
  };
  return { v, reads };
}

const chat = {
  streamId: "t-1",
  chatId: "chat-1",
  connectionId: "c-1",
  userMessage: { id: "u-1", content: "How many?" },
  assistantMessageId: "a-1",
  approval: "ask" as const,
  clientTools: true,
};

async function drain(it: AsyncIterable<AiEvent>) {
  const out: AiEvent[] = [];
  for await (const e of it) out.push(e);
  return out;
}

describe("CoreAi on the desktop (no vault)", () => {
  it("sends a turn as ai.chat with no key", async () => {
    const { client, streams, calls } = fakeClient();
    const ai = new CoreAi(
      () => client,
      () => null,
    );
    const events = await drain(ai.chat({ ...chat, providerId: "prov-1" }));
    expect(events.at(-1)?.type).toBe("done");
    expect(streams).toEqual([{ method: "ai", params: { method: "chat", params: chat } }]);
    expect(JSON.stringify(streams)).not.toContain("apiKey");
    // No call reads a secret.
    expect(calls.filter((c) => c.method === "secret")).toEqual([]);
  });

  it("generate, models and test send no key and answer Core's result", async () => {
    const { client, calls } = fakeClient((req) => {
      const inner = (req as { params: { method: string } }).params.method;
      if (inner === "generate") return { sql: "SELECT 1" };
      if (inner === "models") return ["m-1", "m-2"];
      return null;
    });
    const ai = new CoreAi(
      () => client,
      () => null,
    );
    expect(
      await ai.generate({
        connectionId: "conn-1",
        providerId: "prov-1",
        request: "users",
        existingQuery: "",
      }),
    ).toBe("SELECT 1");
    expect(await ai.models("prov-1")).toEqual(["m-1", "m-2"]);
    await ai.test("prov-1");
    expect(calls).toEqual([
      {
        method: "ai",
        params: {
          method: "generate",
          params: { connectionId: "conn-1", request: "users", existingQuery: "" },
        },
      },
      { method: "ai", params: { method: "models", params: { providerId: "prov-1" } } },
      { method: "ai", params: { method: "test", params: { providerId: "prov-1" } } },
    ]);
  });

  it("respond sends the decision for the turn's call", async () => {
    const { client, calls } = fakeClient();
    const ai = new CoreAi(
      () => client,
      () => null,
    );
    await ai.respond("t-1", "call_1", "allowAll");
    await ai.respond("t-1", "call_2", { result: '{"dashboard_id":"d"}' });
    expect(calls.map((c) => (c as { params: unknown }).params)).toEqual([
      {
        method: "respond",
        params: { streamId: "t-1", callId: "call_1", decision: "allowAll" },
      },
      {
        method: "respond",
        params: { streamId: "t-1", callId: "call_2", decision: { result: '{"dashboard_id":"d"}' } },
      },
    ]);
  });

  it("rejects with Core's code", async () => {
    const client = {
      call: async () => {
        throw new CoreCallError({ code: "NO_API_KEY", message: "No API key is set." });
      },
    } as unknown as CoreClient;
    const ai = new CoreAi(
      () => client,
      () => null,
    );
    await expect(ai.models("prov-1")).rejects.toMatchObject({ code: "NO_API_KEY" });
  });
});

describe("CoreAi on web (the vault)", () => {
  it("sends the vault's key with every call", async () => {
    const { client, streams, calls } = fakeClient((req) => {
      const inner = (req as { params: { method: string } }).params.method;
      return inner === "generate" ? { sql: "SELECT 1" } : inner === "models" ? [] : null;
    });
    const { v } = vault({ "prov-1": TEST_KEY });
    const ai = new CoreAi(
      () => client,
      () => v,
    );
    await drain(ai.chat({ ...chat, providerId: "prov-1" }));
    await ai.generate({
      connectionId: "conn-1",
      providerId: "prov-1",
      request: "r",
      existingQuery: "",
    });
    await ai.models("prov-1");
    await ai.test("prov-1");
    const sent = [streams[0], ...calls].map(
      (r) => (r as { params: { params: { apiKey?: string } } }).params.params.apiKey,
    );
    expect(sent).toEqual([TEST_KEY, TEST_KEY, TEST_KEY, TEST_KEY]);
    // Every key names its provider, so Core can refuse it for another.
    const named = [streams[0], ...calls].map(
      (r) => (r as { params: { params: { providerId?: string } } }).params.params.providerId,
    );
    expect(named).toEqual(["prov-1", "prov-1", "prov-1", "prov-1"]);
  });

  it("a send unlocks the vault quietly; the other calls don't (probe F1)", async () => {
    const { client } = fakeClient((req) => {
      const inner = (req as { params: { method: string } }).params.method;
      return inner === "generate" ? { sql: "SELECT 1" } : inner === "models" ? [] : null;
    });
    const quiet: (boolean | undefined)[] = [];
    const v: AiKeyVault = {
      hasAIApiKeyForProvider: async () => true,
      getAIApiKeyForProvider: async (_id, options) => {
        quiet.push(options?.quiet);
        return TEST_KEY;
      },
    };
    const ai = new CoreAi(
      () => client,
      () => v,
    );
    await drain(ai.chat({ ...chat, providerId: "prov-1" }));
    await ai.generate({
      connectionId: "conn-1",
      providerId: "prov-1",
      request: "r",
      existingQuery: "",
    });
    await ai.models("prov-1");
    expect(quiet).toEqual([true, undefined, undefined]);
  });

  it("sends none for a provider the vault holds no key for, without unlocking it", async () => {
    const { client, streams } = fakeClient();
    const { v, reads } = vault({});
    const ai = new CoreAi(
      () => client,
      () => v,
    );
    await drain(ai.chat({ ...chat, providerId: "prov-2" }));
    expect(JSON.stringify(streams)).not.toContain("apiKey");
    expect(reads).toEqual([]);
  });

  it("ends a turn whose key can't be read (a cancelled unlock) with an error event", async () => {
    const { client, streams } = fakeClient();
    const v: AiKeyVault = {
      hasAIApiKeyForProvider: async () => true,
      getAIApiKeyForProvider: async () => {
        throw new Error("cancelled");
      },
    };
    const ai = new CoreAi(
      () => client,
      () => v,
    );
    const events = await drain(ai.chat({ ...chat, providerId: "prov-1" }));
    expect(events).toEqual([expect.objectContaining({ type: "error", code: "VAULT_LOCKED" })]);
    expect(streams).toEqual([]);
  });
});
