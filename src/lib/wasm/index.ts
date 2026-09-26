// The seaquel-wasm module (crates/seaquel-wasm, built into ./pkg by
// scripts/build-wasm.mjs). The root +layout.ts awaits initSeaquelWasm before
// anything renders, so every call after that is synchronous. Vitest loads it
// in vitest-setup.ts.
import init, * as bindings from "./pkg/seaquel_wasm.js";
import wasmUrl from "./pkg/seaquel_wasm_bg.wasm?url";

export type SeaquelWasm = typeof bindings;

let ready = false;
let pending: Promise<void> | undefined;

/**
 * Fetches, compiles and instantiates the module. Pass `load`'s `fetch`, so
 * SvelteKit doesn't warn about `window.fetch` in `load`. Concurrent calls share
 * one attempt; a failed attempt is forgotten, so calling again retries.
 */
export function initSeaquelWasm(fetchFn: typeof fetch = fetch): Promise<void> {
  if (ready) return Promise.resolve();
  pending ??= init({ module_or_path: fetchFn(wasmUrl) }).then(
    () => {
      ready = true;
    },
    (e: unknown) => {
      pending = undefined;
      throw e;
    },
  );
  return pending;
}

/** Instantiates the module from its bytes or a compiled module. For vitest. */
export function initSeaquelWasmSync(module: BufferSource | WebAssembly.Module): void {
  if (ready) return;
  bindings.initSync({ module });
  ready = true;
}

/** The module's exports. Throws if the module hasn't been initialised. */
export function wasm(): SeaquelWasm {
  if (!ready) throw new Error("seaquel-wasm used before init");
  return bindings;
}

/**
 * Runs `fn` against the module. The exports report bad input as a result
 * (`{error}`), never by throwing, so anything thrown here is unexpected: a
 * trap (`WebAssembly.RuntimeError`, which is what a Rust panic becomes), a
 * stack overflow inside the module (`RangeError` in V8, `InternalError` in
 * Firefox, which also leaks the module's shadow stack), or a glue error. Any
 * of them can leave the instance inconsistent, so this logs it, swaps in a
 * fresh instance of the same compiled module and rethrows. The instance holds
 * no state between calls, so that costs nothing but the instantiation (well
 * under a millisecond). The next call runs on the fresh instance.
 */
export function callWasm<T>(fn: (m: SeaquelWasm) => T): T {
  const m = wasm();
  try {
    return fn(m);
  } catch (e) {
    console.error("seaquel-wasm call failed; re-instantiating the module", e);
    try {
      bindings.__seaquel_reinstantiate();
    } catch (reinit) {
      console.error("seaquel-wasm re-instantiation failed", reinit);
    }
    throw e;
  }
}
