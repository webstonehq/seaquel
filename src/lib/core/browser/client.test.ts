/**
 * The demo's `CoreClient` over a stand-in `BrowserCore`: an assistant
 * turn's `ai` events reach its stream, routed by stream id, and a turn the
 * module ends with no `done` or `error` (cancelled) ends as `CANCELLED`
 * (phase 6 Task 7; the module's own turn is pinned in `ai-turn.test.ts`).
 */
import { describe, expect, it, vi } from "vitest";
import type { CoreEvent } from "$lib/types/generated/CoreEvent";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { AiChatRequest } from "../client";
import type { BrowserCore } from "./transport";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { browserCoreClient } = await import("./client");

/** A `BrowserCore` whose `stream` plays `events` for the request it gets. */
function fakeCore(events: CoreEvent[]) {
  const bodies: unknown[] = [];
  const core = {
    onRestarted: () => () => {},
    subscribe: () => () => {},
    call: async () => JSON.stringify({ method: "db", result: { method: "cancel", result: null } }),
    stream: async (body: Uint8Array, onEvent: (e: CoreEvent) => void) => {
      bodies.push(JSON.parse(new TextDecoder().decode(body)));
      for (const e of events) onEvent(e);
      return events.length;
    },
  } as unknown as BrowserCore;
  return { core, bodies };
}

const turn: AiChatRequest = {
  method: "ai",
  params: {
    method: "chat",
    params: {
      streamId: "t-1",
      chatId: "chat-1",
      connectionId: "c-1",
      userMessage: { id: "u-1", content: "Hi" },
      assistantMessageId: "a-1",
    },
  },
};

async function collect(iterable: AsyncIterable<AiEvent>): Promise<AiEvent[]> {
  const out: AiEvent[] = [];
  for await (const event of iterable) out.push(event);
  return out;
}

describe("the demo's CoreClient and an assistant turn", () => {
  it("yields the turn's ai events and ends at its done", async () => {
    const end: AiEvent = { type: "done", messages: [], seq: { epoch: "e", n: 2 }, stop: "end" };
    const { core, bodies } = fakeCore([
      { type: "ai", streamId: "t-1", event: { type: "text", delta: "Hello" } },
      { type: "ai", streamId: "t-2", event: { type: "text", delta: "Not mine" } },
      { type: "ai", streamId: "t-1", event: end },
    ]);
    const events = await collect(browserCoreClient(core).stream(turn));
    expect(events).toEqual([{ type: "text", delta: "Hello" }, end]);
    expect(bodies).toEqual([turn]);
  });

  it("ends a turn the module stopped without an ending as CANCELLED", async () => {
    const { core } = fakeCore([
      { type: "ai", streamId: "t-1", event: { type: "text", delta: "Part" } },
    ]);
    const events = await collect(browserCoreClient(core).stream(turn));
    expect(events.at(-1)).toEqual(expect.objectContaining({ type: "error", code: "CANCELLED" }));
  });
});
