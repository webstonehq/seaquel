/**
 * The window id (phase 5d-2): the web tab's `win-<uuid>` in
 * `sessionStorage`, a new one for a duplicated tab, settled before the
 * page's first Core call, and the origin of every call and of the socket.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

let web = true;
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => web,
  isDemo: () => !web,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const {
  DEMO_WINDOW_ID,
  WINDOW_ID_CHANNEL,
  WINDOW_ID_KEY,
  resetWindowIdForTests,
  resolveWebWindowId,
  windowId,
  windowIdReady,
} = await import("./window-id");
const { pageOrigin, webPageOrigin } = await import("./origin");
const { httpCoreTransport, sendKeepaliveRequest, encodeCoreRequest } =
  await import("$lib/storage/rust-client");
const { HttpCoreClient } = await import("./http");

/** A `sessionStorage` stand-in. */
class MemoryStorage {
  readonly items = new Map<string, string>();
  constructor(entries: Record<string, string> = {}) {
    for (const [k, v] of Object.entries(entries)) this.items.set(k, v);
  }
  getItem(key: string) {
    return this.items.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.items.set(key, value);
  }
}

const channel = () => new BroadcastChannel(WINDOW_ID_CHANNEL) as never;
const opened: { close(): void }[] = [];

/** Another live tab holding `id` (it has settled and answers claims). */
async function liveTab(id: string) {
  const tab = await resolveWebWindowId({
    storage: new MemoryStorage({ [WINDOW_ID_KEY]: id }),
    openChannel: channel,
    waitMs: 20,
  });
  if (tab.channel) opened.push(tab.channel);
  return tab;
}

function okFetch() {
  return vi.fn(
    async (_url: string, _init: RequestInit) =>
      new Response(JSON.stringify({ method: "storage", result: { method: "x", result: null } })),
  );
}

function originOf(init: RequestInit | undefined): string | null {
  return new Headers(init?.headers).get("x-seaquel-origin");
}

const request = { method: "storage", params: { method: "vaultStateLoad" } } as const;

beforeEach(() => {
  web = true;
  resetWindowIdForTests();
});

afterEach(() => {
  for (const c of opened.splice(0)) c.close();
  resetWindowIdForTests();
  vi.unstubAllGlobals();
});

describe("the web window id", () => {
  it("a new tab makes a win- id and keeps it in sessionStorage", async () => {
    const storage = new MemoryStorage();
    const { id, channel: ch } = await resolveWebWindowId({
      storage,
      openChannel: channel,
      newId: () => "11111111-2222-4333-8444-555555555555",
    });
    ch?.close();
    expect(id).toBe("win-11111111-2222-4333-8444-555555555555");
    expect(storage.getItem(WINDOW_ID_KEY)).toBe(id);
  });

  it("a reload keeps the tab's id", async () => {
    const storage = new MemoryStorage({ [WINDOW_ID_KEY]: "win-kept" });
    const { id, channel: ch } = await resolveWebWindowId({
      storage,
      openChannel: channel,
      waitMs: 20,
    });
    ch?.close();
    expect(id).toBe("win-kept");
  });

  it("a stored value that isn't a window id is replaced", async () => {
    const storage = new MemoryStorage({ [WINDOW_ID_KEY]: "not a window id" });
    const { id } = await resolveWebWindowId({
      storage,
      openChannel: () => null,
      newId: () => "fresh",
    });
    expect(id).toBe("win-fresh");
  });

  it("works without sessionStorage or BroadcastChannel", async () => {
    const { id, channel: ch } = await resolveWebWindowId({
      storage: null,
      openChannel: () => null,
      newId: () => "only-this-page",
    });
    expect(id).toBe("win-only-this-page");
    expect(ch).toBeNull();
  });

  it("a duplicated tab gets a new window id before its first call", async () => {
    // Tab A is live with win-A; tab B is its duplicate, sessionStorage copied.
    await liveTab("win-A");
    const storage = new MemoryStorage({ [WINDOW_ID_KEY]: "win-A" });
    vi.stubGlobal("sessionStorage", storage);
    const fetchMock = okFetch();
    vi.stubGlobal("fetch", fetchMock);

    // B's very first Core call, with nothing awaited before it.
    await httpCoreTransport(encodeCoreRequest(request));

    const sent = originOf(fetchMock.mock.calls[0][1]);
    expect(sent).toMatch(/^win-/);
    expect(sent).not.toBe("win-A");
    expect(storage.getItem(WINDOW_ID_KEY)).toBe(sent);
    expect(windowId()).toBe(sent);
    // And A keeps its own: a later duplicate check of win-A is still answered.
    const again = await resolveWebWindowId({
      storage: new MemoryStorage({ [WINDOW_ID_KEY]: "win-A" }),
      openChannel: channel,
      waitMs: 50,
      newId: () => "third",
    });
    again.channel?.close();
    expect(again.id).toBe("win-third");
  });

  it("the web origin is the window id on every call and on the socket", async () => {
    vi.stubGlobal("sessionStorage", new MemoryStorage({ [WINDOW_ID_KEY]: "win-tab-1" }));
    const fetchMock = okFetch();
    vi.stubGlobal("fetch", fetchMock);

    // Nothing is settled yet: the page has no origin to put on a keepalive.
    expect(webPageOrigin()).toBeNull();
    expect(sendKeepaliveRequest(request, "test")).toBe(false);

    // The socket waits for the id before it opens.
    const urls: string[] = [];
    const client = new HttpCoreClient({
      createSocket: (url) => {
        urls.push(url);
        return {
          readyState: 0,
          send: () => {},
          close: () => {},
          onopen: null,
          onmessage: null,
          onclose: null,
          onerror: null,
        } as never;
      },
    });
    const stop = client.events(() => {});
    expect(urls).toEqual([]);

    await httpCoreTransport(encodeCoreRequest(request));
    await httpCoreTransport(encodeCoreRequest(request));
    expect(sendKeepaliveRequest(request, "test")).toBe(true);
    await vi.waitFor(() => expect(urls).toHaveLength(1));
    stop();

    const origins = fetchMock.mock.calls.map(([, init]) => originOf(init));
    expect(origins).toEqual(["win-tab-1", "win-tab-1", "win-tab-1"]);
    expect(urls[0]).toBe("ws://localhost/api/rpc/stream?origin=win-tab-1");
    expect(webPageOrigin()).toBe("win-tab-1");
    expect(pageOrigin()).toBe("win-tab-1");
  });

  it("two pages checking the same id at once don't both keep it", async () => {
    // A tab reloads while its duplicate opens: both start from win-same.
    const nonces = ["b-nonce", "a-nonce"];
    const [first, second] = await Promise.all(
      ["one", "two"].map((name, i) =>
        resolveWebWindowId({
          storage: new MemoryStorage({ [WINDOW_ID_KEY]: "win-same" }),
          openChannel: channel,
          waitMs: 50,
          newId: () => `fresh-${name}`,
          nonce: () => nonces[i],
        }),
      ),
    );
    for (const t of [first, second]) if (t.channel) opened.push(t.channel);
    const ids = [first.id, second.id];
    expect(ids).toContain("win-same");
    expect(new Set(ids).size).toBe(2);
    // The lower nonce yields.
    expect(second.id).toBe("win-fresh-two");
  });

  it("a socket no longer needed when the id settles isn't opened", async () => {
    vi.stubGlobal("sessionStorage", new MemoryStorage({ [WINDOW_ID_KEY]: "win-tab-2" }));
    vi.stubGlobal("fetch", okFetch());
    const urls: string[] = [];
    const client = new HttpCoreClient({
      createSocket: (url) => {
        urls.push(url);
        return { send: () => {}, close: () => {} } as never;
      },
    });
    const stop = client.events(() => {});
    stop();
    await windowIdReady();
    await new Promise((r) => setTimeout(r, 0));
    expect(urls).toEqual([]);
  });

  it("the demo's window is `demo`", async () => {
    web = false;
    expect(await windowIdReady()).toBe(DEMO_WINDOW_ID);
    expect(windowId()).toBe(DEMO_WINDOW_ID);
  });
});
