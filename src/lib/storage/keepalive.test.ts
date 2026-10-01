/**
 * The page-hide save on web (re-survey bug 2): one `keepalive` request,
 * outside the write queue, only when its body fits the browser's budget.
 * Since phase 5d-2 it is the `ui` group's `windowStateSave`, which needs
 * `X-Seaquel-Origin` to equal the window id (`window-state.svelte.ts`
 * sends it on `pagehide`).
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

let tauri = false;
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => tauri,
  isWeb: () => false,
  isDemo: () => false,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const { RustStorageClient, KEEPALIVE_MAX_BYTES } = await import("./rust-client");
const { windowId, windowIdReady } = await import("$lib/core/window-id");

beforeAll(async () => {
  await windowIdReady();
});

function save(text: string) {
  return {
    windowId: windowId()!,
    projectId: "p1",
    rev: 3,
    state: state(text),
  };
}

function state(text: string) {
  return {
    projectId: "p1",
    queryTabs: [{ id: "q", name: "q", query: text }],
    schemaTabs: [],
    explainTabs: [],
    erdTabs: [],
    tabOrder: ["q"],
    activeQueryTabId: "q",
    activeSchemaTabId: null,
    activeExplainTabId: null,
    activeErdTabId: null,
    activeView: "query",
    activeConnectionId: null,
  };
}

const fetchMock = vi.fn(async () => new Response("{}"));

beforeEach(() => {
  tauri = false;
  fetchMock.mockClear();
  vi.stubGlobal("fetch", fetchMock);
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("saveWindowStateKeepalive", () => {
  it("sends one keepalive request with the same body a queued save would", async () => {
    const bodies: Uint8Array[] = [];
    // A queued write is held, so the keepalive request can be seen to overtake it.
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const client = new RustStorageClient(async (body) => {
      bodies.push(body);
      await held;
      return { method: "storage", result: { method: "vaultStateReset", result: null } };
    });
    const queued = client.vaultState.reset();
    await new Promise((r) => setTimeout(r, 0));

    expect(client.saveWindowStateKeepalive(save("SELECT 1"))).toBe(true);

    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/rpc");
    expect(init.keepalive).toBe(true);
    expect(init.method).toBe("POST");
    const sent = JSON.parse(new TextDecoder().decode(init.body as Uint8Array));
    expect(sent.method).toBe("ui");
    expect(sent.params.method).toBe("windowStateSave");
    expect(sent.params.params.rev).toBe(3);
    expect(sent.params.params.state.queryTabs[0].query).toBe("SELECT 1");
    // The window id must equal the origin the request carries.
    expect((init.headers as Record<string, string>)["x-seaquel-origin"]).toBe(
      sent.params.params.windowId,
    );
    // The queued write hadn't answered when the keepalive left.
    expect(bodies).toHaveLength(1);
    release();
    await queued;
  });

  it("a body over the cap isn't sent", () => {
    const client = new RustStorageClient(async () => ({}));
    expect(client.saveWindowStateKeepalive(save("x".repeat(KEEPALIVE_MAX_BYTES)))).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("desktop doesn't use it", () => {
    tauri = true;
    const client = new RustStorageClient(async () => ({}));
    expect(client.saveWindowStateKeepalive(save("SELECT 1"))).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("the cap is under the browsers' 64 KiB keepalive budget", () => {
    expect(KEEPALIVE_MAX_BYTES).toBe(60 * 1024);
  });
});
