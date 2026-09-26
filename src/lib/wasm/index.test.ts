import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { callWasm, wasm } from "./index";

const crateVersion = readFileSync(
  fileURLToPath(new URL("../../../crates/seaquel-wasm/Cargo.toml", import.meta.url)),
  "utf8",
).match(/^version = "([^"]+)"/m)?.[1];

const wasmBytes = () =>
  readFileSync(fileURLToPath(new URL("./pkg/seaquel_wasm_bg.wasm", import.meta.url)));

describe("seaquel-wasm", () => {
  it("is loaded by the vitest setup file", () => {
    expect(crateVersion).toBeTruthy();
    expect(wasm().version()).toBe(crateVersion);
  });

  it("initSeaquelWasm fetches and instantiates the module", async () => {
    vi.resetModules();
    const fresh = await import("./index");
    expect(() => fresh.wasm()).toThrow("seaquel-wasm used before init");

    const urls: string[] = [];
    const fakeFetch = (async (input: RequestInfo | URL) => {
      urls.push(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      return new Response(wasmBytes(), { headers: { "content-type": "application/wasm" } });
    }) as typeof fetch;
    await fresh.initSeaquelWasm(fakeFetch);

    expect(urls).toHaveLength(1);
    expect(urls[0]).toMatch(/seaquel_wasm_bg\.wasm/);
    expect(fresh.wasm().version()).toBe(crateVersion);

    // A second call is a no-op.
    await fresh.initSeaquelWasm(fakeFetch);
    expect(urls).toHaveLength(1);
  });

  it("initSeaquelWasm retries after a failed attempt", async () => {
    vi.resetModules();
    const fresh = await import("./index");
    let calls = 0;
    const flakyFetch = (async () => {
      calls++;
      return calls === 1
        ? new Response(null, { status: 404, statusText: "Not Found" })
        : new Response(wasmBytes(), { headers: { "content-type": "application/wasm" } });
    }) as typeof fetch;

    await expect(fresh.initSeaquelWasm(flakyFetch)).rejects.toThrow(/404/);
    expect(() => fresh.wasm()).toThrow("seaquel-wasm used before init");
    await fresh.initSeaquelWasm(flakyFetch);
    expect(fresh.wasm().version()).toBe(crateVersion);
    expect(calls).toBe(2);
  });

  it("callWasm re-instantiates the module after a trap and rethrows", () => {
    const before = callWasm((m) => m.version());
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const trap = new WebAssembly.RuntimeError("unreachable");
    expect(() =>
      callWasm(() => {
        throw trap;
      }),
    ).toThrow(trap);
    expect(spy).toHaveBeenCalledOnce();
    spy.mockRestore();
    expect(callWasm((m) => m.version())).toBe(before);
  });

  it("__seaquel_reinstantiate swaps in a fresh instance", async () => {
    const bindings = await import("./pkg/seaquel_wasm.js");
    const first = bindings.__seaquel_reinstantiate();
    const second = bindings.__seaquel_reinstantiate();
    expect(second.memory).not.toBe(first.memory);
    expect(bindings.version()).toBe(crateVersion);
  });

  it.each([
    ["RangeError (V8 stack overflow)", new RangeError("Maximum call stack size exceeded")],
    ["TypeError (glue error)", new TypeError("boom")],
  ])("callWasm re-instantiates after a %s too", (_, err) => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() =>
      callWasm(() => {
        throw err;
      }),
    ).toThrow(err);
    expect(spy).toHaveBeenCalledOnce();
    spy.mockRestore();
    expect(callWasm((m) => m.version())).toBe(crateVersion);
  });
});
