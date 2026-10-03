/**
 * The demo's fetch bridge (phase 6 Task 8): the page's half of the module's
 * model calls. It sends what Rust asks for, gives the head and the body
 * back chunk by chunk, and `abort` stops the request (before the head and
 * after). Against a local mock only; the key is the fake test key.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { makeFetchBridge } from "./fetch-bridge";
import { chunk, localFetch, MockProvider } from "./testing/mock-provider";

const TEST_KEY = "test-key-not-real";
const mock = new MockProvider();

beforeAll(() => mock.start());
afterAll(() => mock.stop());

const decode = (bytes: Uint8Array[]) =>
  new TextDecoder().decode(Uint8Array.from(bytes.flatMap((b) => [...b])));

describe("makeFetchBridge", () => {
  it("sends the request and streams the body back, then null", async () => {
    mock.scripts.push({ events: [chunk({ a: 1 }), chunk({ b: 2 })] });
    const bridge = makeFetchBridge(localFetch(mock));
    const status = await bridge.start(
      1,
      "POST",
      `${mock.url}/v1/chat/completions`,
      [
        ["content-type", "application/json"],
        ["authorization", `Bearer ${TEST_KEY}`],
      ],
      new TextEncoder().encode('{"x":1}'),
    );
    expect(status).toBe(200);
    const chunks: Uint8Array[] = [];
    for (let next = await bridge.read(1); next; next = await bridge.read(1)) chunks.push(next);
    expect(decode(chunks)).toBe(`${chunk({ a: 1 })}${chunk({ b: 2 })}`);
    // Finished: reading again and aborting are no-ops.
    expect(await bridge.read(1)).toBeNull();
    bridge.abort(1);
    const seen = mock.seen.at(-1)!;
    expect(seen).toMatchObject({ method: "POST", path: "/v1/chat/completions", body: '{"x":1}' });
    expect(seen.headers.authorization).toBe(`Bearer ${TEST_KEY}`);
    // No cookie, no referrer: the request carries only what Rust gave it.
    expect(seen.headers.cookie).toBeUndefined();
    expect(seen.headers.referer).toBeUndefined();
  });

  it("a GET sends no body", async () => {
    mock.scripts.push({ events: ["{}"] });
    const bridge = makeFetchBridge(localFetch(mock));
    expect(await bridge.start(2, "GET", `${mock.url}/v1/models`, [], new Uint8Array())).toBe(200);
    expect(mock.seen.at(-1)).toMatchObject({ method: "GET", body: "" });
    expect(decode([(await bridge.read(2))!])).toBe("{}");
  });

  it("passes the page's fetch options: no credentials, no referrer, no cache, no redirects", async () => {
    const seen: RequestInit[] = [];
    const fake = vi.fn(async (_url: unknown, init?: RequestInit) => {
      seen.push(init!);
      return new Response("ok", { status: 201 });
    });
    const bridge = makeFetchBridge(fake as unknown as typeof fetch);
    expect(
      await bridge.start(3, "POST", "https://example.invalid/x", [["a", "b"]], new Uint8Array([1])),
    ).toBe(201);
    expect(seen[0]).toMatchObject({
      method: "POST",
      headers: [["a", "b"]],
      credentials: "omit",
      referrerPolicy: "no-referrer",
      cache: "no-store",
      redirect: "error",
    });
    expect(seen[0].signal).toBeInstanceOf(AbortSignal);
  });

  it("abort after the head stops the body, and the server sees the client go", async () => {
    mock.scripts.push({ events: [chunk({ part: 1 })], stall: true });
    const bridge = makeFetchBridge(localFetch(mock));
    const goneBefore = mock.gone;
    expect(
      await bridge.start(4, "POST", `${mock.url}/v1/chat/completions`, [], new Uint8Array([1])),
    ).toBe(200);
    expect(await bridge.read(4)).toBeInstanceOf(Uint8Array);
    const pending = bridge.read(4);
    bridge.abort(4);
    await expect(pending.catch(() => null)).resolves.toBeNull();
    await expect.poll(() => mock.gone).toBeGreaterThan(goneBefore);
    expect(await bridge.read(4)).toBeNull();
  });

  it("abort before the head aborts the fetch", async () => {
    let signal: AbortSignal | undefined;
    const fake = (_url: unknown, init?: RequestInit) =>
      new Promise<Response>((_, reject) => {
        signal = init!.signal!;
        signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      });
    const bridge = makeFetchBridge(fake as unknown as typeof fetch);
    const head = bridge.start(5, "POST", "https://example.invalid/x", [], new Uint8Array());
    bridge.abort(5);
    expect(signal?.aborted).toBe(true);
    await expect(head).rejects.toThrow();
  });

  it("a failed fetch rejects with fixed text, never the URL or the key", async () => {
    const fake = async (url: unknown) => {
      throw new Error(`could not reach ${String(url)} with ${TEST_KEY}`);
    };
    const bridge = makeFetchBridge(fake as unknown as typeof fetch);
    const error = await bridge
      .start(
        6,
        "POST",
        "https://secret-host.invalid/path?q=1",
        [["x-api-key", TEST_KEY]],
        new Uint8Array(),
      )
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(Error);
    expect(String(error)).not.toContain("secret-host");
    expect(String(error)).not.toContain(TEST_KEY);
  });

  it("an unknown id reads null and aborts nothing", async () => {
    const bridge = makeFetchBridge(localFetch(mock));
    expect(await bridge.read(99)).toBeNull();
    expect(() => bridge.abort(99)).not.toThrow();
  });

  it("abortAll stops every request still running, before and after the head", async () => {
    mock.scripts.push({ events: [chunk({ part: 1 })], stall: true });
    const goneBefore = mock.gone;
    const bridge = makeFetchBridge(localFetch(mock));
    expect(
      await bridge.start(7, "POST", `${mock.url}/v1/chat/completions`, [], new Uint8Array([1])),
    ).toBe(200);
    let signal: AbortSignal | undefined;
    const slow = makeFetchBridge(
      ((_url: unknown, init?: RequestInit) =>
        new Promise<Response>((_, reject) => {
          signal = init!.signal!;
          signal.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
        })) as unknown as typeof fetch,
    );
    const head = slow.start(8, "POST", "https://example.invalid/x", [], new Uint8Array());
    // Handled before the abort rejects it, so it's never an unhandled rejection.
    const headRejects = expect(head).rejects.toThrow();
    bridge.abortAll!();
    slow.abortAll!();
    await expect.poll(() => mock.gone).toBeGreaterThan(goneBefore);
    expect(await bridge.read(7)).toBeNull();
    expect(signal?.aborted).toBe(true);
    await headRejects;
  });
});
