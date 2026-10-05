/**
 * The assistant's turn in the demo's module: `ai.chat`
 * through the module's `stream`, its model calls going out through a fetch
 * bridge the page passes to `open` (here over Node's `fetch`) to a local
 * Node mock provider. Stopping a turn is `db.cancel`, and the module keeps
 * polling it, so the reply is stored with what streamed; a dropped turn
 * aborts its request too. The bridge is the page's own (`./fetch-bridge`).
 * No real provider is called; the key is the fake test key.
 */
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import { makeDuckDbBridge } from "./duckdb-bridge";
import { makeFetchBridge } from "./fetch-bridge";
import {
  anthropicText,
  chunk,
  localFetch,
  MockProvider,
  openaiText,
} from "./testing/mock-provider";
import {
  bootDuckDb,
  body,
  loadTestModule,
  testModuleMissing,
  type TestModule,
} from "./testing/node";

const missing = testModuleMissing();
if (missing && process.env.CI) throw new Error(missing);

const TEST_KEY = "test-key-not-real";

/**
 * The page's fetch bridge (`./fetch-bridge`) over Node's `fetch`,
 * limited to the mock (`localFetch`), with each abort recorded.
 */
function fetchBridge(mock: MockProvider) {
  const bridge = makeFetchBridge(localFetch(mock));
  const aborted: number[] = [];
  const abort = bridge.abort.bind(bridge);
  bridge.abort = (id: number) => {
    aborted.push(id);
    abort(id);
  };
  return { aborted, bridge };
}

describe.skipIf(missing !== null)("the assistant in the browser module", () => {
  let module: TestModule;
  let db: AsyncDuckDB;
  const mock = new MockProvider();
  let fetch: ReturnType<typeof fetchBridge>;

  beforeAll(async () => {
    module = (await loadTestModule())!;
    db = await bootDuckDb();
    await mock.start();
    fetch = fetchBridge(mock);
    await module.open(makeDuckDbBridge(db), undefined, () => {}, fetch.bridge);
  }, 60_000);

  afterAll(async () => {
    await mock.stop();
    await db?.terminate();
  });

  async function call<T = unknown>(group: string, method: string, params?: unknown): Promise<T> {
    const response = JSON.parse(await module.call(body(group, method, params))) as {
      result: { result: T };
    };
    return response.result.result;
  }

  /** The provider `setUp` made: a supplied key names it. */
  let providerId = "";

  /** A provider at the mock on the demo connection, a chat, and the connection open. */
  async function setUp(type: "openai-compatible" | "anthropic") {
    await module.ensureDemoConnection();
    const provider = await call<{ value: { id: string } }>("settings", "aiProviderCreate", {
      provider: { name: "Mock", type, baseUrl: `${mock.url}/v1` },
    });
    providerId = provider.value.id;
    await call("library", "connectionUpdate", {
      id: "demo-connection",
      patch: { activeAIProviderId: provider.value.id, activeAIModel: "model-1" },
    });
    const chat = await call<{ value: { id: string } }>("library", "chatCreate", {
      chat: { connectionId: "demo-connection", title: "T" },
    });
    const { connectionId } = await call<{ connectionId: string }>("db", "connect", {
      target: { type: "saved", id: "demo-connection" },
    });
    return { chat: chat.value.id, connectionId };
  }

  function turn(stream: string, chat: string, connectionId: string, events: unknown[]) {
    return module.stream(
      body("ai", "chat", {
        streamId: stream,
        chatId: chat,
        connectionId,
        userMessage: { id: `${stream}-u`, content: "How many rows?" },
        assistantMessageId: `${stream}-a`,
        apiKey: TEST_KEY,
        providerId,
      }),
      (json) => events.push(JSON.parse(json)),
    );
  }

  async function stored(chat: string) {
    return (
      await call<{ value: { messages: { content: string }[] } }>("library", "chatMessagesList", {
        chatId: chat,
      })
    ).value.messages;
  }

  it("runs a turn through the fetch bridge", async () => {
    const { chat, connectionId } = await setUp("openai-compatible");
    mock.scripts.push({ events: openaiText("Three rows.") });
    const events: { type: string; streamId: string; event: Record<string, unknown> }[] = [];
    const sent = await turn("t1", chat, connectionId, events);
    expect(sent).toBe(events.length);
    expect(events.every((e) => e.type === "ai" && e.streamId === "t1")).toBe(true);
    expect(events[0].event.type).toBe("started");
    const done = events.at(-1)!.event;
    expect(done.type).toBe("done");
    expect((done.messages as { content: string }[])[1].content).toBe("Three rows.");
    const request = mock.seen.at(-1)!;
    expect(request.path).toBe("/v1/chat/completions");
    expect(request.headers.authorization).toBe(`Bearer ${TEST_KEY}`);
    expect(JSON.stringify(events)).not.toContain(TEST_KEY);
    expect(await stored(chat)).toHaveLength(2);
  });

  it("sends Anthropic's direct-access header from the page", async () => {
    const { chat, connectionId } = await setUp("anthropic");
    mock.scripts.push({ events: anthropicText("Hi.") });
    const events: { event: { type: string } }[] = [];
    await turn("t2", chat, connectionId, events);
    expect(events.at(-1)!.event.type).toBe("done");
    const request = mock.seen.at(-1)!;
    expect(request.path).toBe("/v1/messages");
    expect(request.headers["x-api-key"]).toBe(TEST_KEY);
    expect(request.headers["anthropic-dangerous-direct-browser-access"]).toBe("true");
  });

  it("stops a turn with db.cancel, stores the reply and aborts the request", async () => {
    const { chat, connectionId } = await setUp("openai-compatible");
    mock.scripts.push({
      events: [chunk({ choices: [{ index: 0, delta: { content: "Partial answer" } }] })],
      stall: true,
    });
    const events: { event: { type: string } }[] = [];
    let cancelled = false;
    const goneBefore = mock.gone;
    const abortedBefore = fetch.aborted.length;
    const sent = await module.stream(
      body("ai", "chat", {
        streamId: "t3",
        chatId: chat,
        connectionId,
        userMessage: { id: "t3-u", content: "Go on" },
        assistantMessageId: "t3-a",
      }),
      (json) => {
        const event = JSON.parse(json) as { event: { type: string } };
        events.push(event);
        if (event.event.type === "text" && !cancelled) {
          cancelled = true;
          // Never from inside the callback: a microtask later.
          queueMicrotask(() => void module.call(body("db", "cancel", { streamId: "t3" })));
        }
      },
    );
    expect(sent).toBe(events.length);
    expect(events.map((e) => e.event.type)).not.toContain("done");
    expect(events.map((e) => e.event.type)).not.toContain("error");
    const rows = await stored(chat);
    expect(rows).toHaveLength(2);
    expect(rows[1].content).toBe("Partial answer");
    expect(fetch.aborted.length).toBeGreaterThan(abortedBefore);
    await expect.poll(() => mock.gone).toBeGreaterThan(goneBefore);
  });
  it("a dropped turn aborts its request, and the provider sees the client go", async () => {
    const { chat, connectionId } = await setUp("openai-compatible");
    mock.scripts.push({
      events: [chunk({ choices: [{ index: 0, delta: { content: "Half" } }] })],
      stall: true,
    });
    const goneBefore = mock.gone;
    const abortedBefore = fetch.aborted.length;
    const answer = JSON.parse(
      await module.__test_stream_dropped_after(
        body("ai", "chat", {
          streamId: "t4",
          chatId: chat,
          connectionId,
          userMessage: { id: "t4-u", content: "Go on" },
          assistantMessageId: "t4-a",
          apiKey: TEST_KEY,
          providerId,
        }),
        500,
        false,
      ),
    ) as { dropped: boolean };
    expect(answer.dropped).toBe(true);
    expect(fetch.aborted.length).toBeGreaterThan(abortedBefore);
    await expect.poll(() => mock.gone).toBeGreaterThan(goneBefore);
  });
});
