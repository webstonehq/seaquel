/**
 * A model provider on 127.0.0.1 for the assistant's tests in Node (vitest
 * only; nothing in the app imports it), and a `fetch` that can reach only
 * it. No real provider is ever called: `localFetch` refuses every host but
 * the mock's, and sends Anthropic's fixed URL to the mock.
 */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

/** One scripted answer: SSE events (then end, or hold the stream open). */
export interface Script {
  events: string[];
  stall?: boolean;
}

export interface Seen {
  method: string;
  path: string;
  headers: IncomingMessage["headers"];
  body: string;
}

/** A provider on 127.0.0.1 that answers each request with the next script. */
export class MockProvider {
  readonly scripts: Script[] = [];
  readonly seen: Seen[] = [];
  /** Responses the client went away from before they ended. */
  gone = 0;
  private server: Server | null = null;
  url = "";

  async start() {
    this.server = createServer((req, res) => this.answer(req, res));
    await new Promise<void>((resolve) => this.server!.listen(0, "127.0.0.1", resolve));
    this.url = `http://127.0.0.1:${(this.server.address() as AddressInfo).port}`;
  }

  private answer(req: IncomingMessage, res: ServerResponse) {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      this.seen.push({
        method: req.method ?? "",
        path: req.url ?? "",
        headers: req.headers,
        body: Buffer.concat(chunks).toString("utf8"),
      });
      const script = this.scripts.shift();
      if (!script) {
        res.writeHead(500).end("no script");
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream" });
      for (const event of script.events) res.write(event);
      if (script.stall) {
        res.on("close", () => {
          if (!res.writableEnded) this.gone += 1;
        });
      } else {
        res.end();
      }
    });
  }

  async stop() {
    this.server?.closeAllConnections();
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }
}

/**
 * Node's `fetch`, limited to the mock: Anthropic's fixed URL goes to the
 * mock, and any other host but 127.0.0.1 is refused before anything is sent.
 */
export function localFetch(mock: MockProvider): typeof fetch {
  return (input, init) => {
    const raw = input instanceof Request ? input.url : input instanceof URL ? input.href : input;
    const url = raw.replace("https://api.anthropic.com", mock.url);
    const host = new URL(url).hostname;
    if (host !== "127.0.0.1" && host !== "localhost") {
      return Promise.reject(new Error("tests reach only the local mock"));
    }
    return fetch(url, init);
  };
}

export const chunk = (data: unknown) => `data: ${JSON.stringify(data)}\n\n`;

export const openaiText = (text: string) => [
  chunk({ choices: [{ index: 0, delta: { role: "assistant", content: text } }] }),
  chunk({ choices: [{ index: 0, delta: {}, finish_reason: "stop" }] }),
  "data: [DONE]\n\n",
];

const anthropicEvent = (name: string, data: unknown) =>
  `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`;

export const anthropicText = (text: string) => [
  anthropicEvent("message_start", {
    type: "message_start",
    message: { id: "m", role: "assistant", content: [] },
  }),
  anthropicEvent("content_block_start", {
    type: "content_block_start",
    index: 0,
    content_block: { type: "text", text: "" },
  }),
  anthropicEvent("content_block_delta", {
    type: "content_block_delta",
    index: 0,
    delta: { type: "text_delta", text },
  }),
  anthropicEvent("content_block_stop", { type: "content_block_stop", index: 0 }),
  anthropicEvent("message_delta", { type: "message_delta", delta: { stop_reason: "end_turn" } }),
  anthropicEvent("message_stop", { type: "message_stop" }),
];
