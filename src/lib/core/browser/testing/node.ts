/**
 * The Node harness for the browser module's tests (vitest only; nothing in
 * the app imports it):
 *
 * - `loadTestModule()` loads `src/lib/wasm/browser-test-pkg/` (the module
 *   with its test exports, `npm run wasm:build:browser-test`, which `pretest`
 *   runs) from its bytes. vitest isolates each test file's modules, so each
 *   file gets its own instance. Without the build it returns `null`, and
 *   the suites skip, except under `CI`, where it throws.
 * - `bootDuckDb()` starts DuckDB-WASM's Node build (the version the demo
 *   loads from jsDelivr) on a worker thread.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { Worker } from "node:worker_threads";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import type { BrowserModule } from "../transport";

const root = fileURLToPath(new URL("../../../../../", import.meta.url));
const pkg = join(root, "src/lib/wasm/browser-test-pkg");

/** What the module is built from: a change to any of it means a rebuild. */
const SOURCES = [
  "Cargo.lock",
  "scripts/build-wasm.mjs",
  ...[
    "seaquel-browser",
    "seaquel-ai",
    "seaquel-core",
    "seaquel-rpc",
    "seaquel-storage",
    "seaquel-engine-duckdb",
    "seaquel-engine",
    "seaquel-types",
    "seaquel-sql",
    "seaquel-workspace",
    "seaquel-runtime",
    "seaquel-macros",
  ].flatMap((crate) => [
    `crates/${crate}/Cargo.toml`,
    `crates/${crate}/build.rs`,
    `crates/${crate}/src`,
    `crates/${crate}/migrations`,
  ]),
];

/** The newest modification time under `path` (a file or a directory), or 0. */
function newest(path: string): number {
  if (!existsSync(path)) return 0;
  const stat = statSync(path);
  if (!stat.isDirectory()) return stat.mtimeMs;
  let latest = 0;
  for (const entry of readdirSync(path)) latest = Math.max(latest, newest(join(path, entry)));
  return latest;
}

/**
 * Why the test module can't be used, or `null` if it can: it isn't built
 * (the suites then skip, except under `CI`), or it is older than a source
 * it's built from, which throws: a stale module would test old code.
 */
export function testModuleMissing(): string | null {
  const stamp = join(pkg, ".stamp");
  if (!existsSync(join(pkg, "seaquel_browser_bg.wasm")) || !existsSync(stamp)) {
    return "src/lib/wasm/browser-test-pkg/ is missing: run `npm run wasm:build:browser-test`";
  }
  const built = statSync(stamp).mtimeMs;
  const stale = SOURCES.find((source) => newest(join(root, source)) > built);
  if (stale) {
    throw new Error(
      `src/lib/wasm/browser-test-pkg/ is older than ${stale}: run \`npm run wasm:build:browser-test\``,
    );
  }
  return null;
}

/** Test-only exports of the `test-hooks` build. */
export interface TestHooks {
  __test_trap(kind: "sync" | "async"): Promise<unknown>;
  __test_side_open(bridge: unknown): Promise<void>;
  __test_side_call(body: Uint8Array): Promise<string>;
  __test_side_stream(body: Uint8Array, onEvent: (json: string) => void): Promise<number>;
  __test_call_dropped_after(body: Uint8Array, ms: number, side: boolean): Promise<string>;
  __test_stream_dropped_after(body: Uint8Array, ms: number, side: boolean): Promise<string>;
  __test_connect_restricted(side: boolean): Promise<string>;
  __test_explain_read_only(connectionId: string, sql: string): Promise<string>;
  /** `call` under another write origin (a window id), for replays of several windows. */
  __test_call_as(body: Uint8Array, origin: string): Promise<string>;
}

export type TestModule = BrowserModule &
  TestHooks & { initSync(o: { module: BufferSource }): unknown };

declare global {
  /**
   * How many traps a test expects from an async export of the module (a
   * panic in wasm-bindgen-futures' task queue, which is otherwise an
   * uncaught exception and fails the run). Each one swallowed decrements
   * it; with none expected, nothing is swallowed.
   */
  var __seaquelExpectedTraps: number | undefined;
  /** Test builds: the next N `open`s panic (`maybe_panic_on_open`). */
  var __seaquelTestPanicOpens: number | undefined;
  /** Called with each trap the harness swallows, before anything else runs. */
  var __seaquelOnSwallowedTrap: ((error: unknown) => void) | undefined;
}

let catcherInstalled = false;

/**
 * Wraps `queueMicrotask`, which the glue schedules the module's task queue
 * with, so a `RuntimeError` thrown from it is swallowed only while
 * `__seaquelExpectedTraps` is above 0 (and counted down). A stray trap still
 * reaches vitest and fails the run.
 */
function installTrapCatcher(): void {
  if (catcherInstalled) return;
  catcherInstalled = true;
  const original = globalThis.queueMicrotask.bind(globalThis);
  globalThis.queueMicrotask = (callback: VoidFunction) =>
    original(() => {
      try {
        callback();
      } catch (error) {
        const expected = globalThis.__seaquelExpectedTraps ?? 0;
        if (error instanceof WebAssembly.RuntimeError && expected > 0) {
          globalThis.__seaquelExpectedTraps = expected - 1;
          globalThis.__seaquelOnSwallowedTrap?.(error);
          return;
        }
        throw error;
      }
    });
}

/** The test module, instantiated; `null` (or a throw under CI) without the build. */
export async function loadTestModule(): Promise<TestModule | null> {
  const missing = testModuleMissing();
  if (missing) {
    if (process.env.CI) throw new Error(missing);
    return null;
  }
  const glue = (await import(
    /* @vite-ignore */ pathToFileURL(join(pkg, "seaquel_browser.js")).href
  )) as TestModule;
  installTrapCatcher();
  glue.initSync({ module: readFileSync(join(pkg, "seaquel_browser_bg.wasm")) });
  return glue;
}

const dist = join(root, "node_modules/@duckdb/duckdb-wasm/dist");

/** A `Worker` as DuckDB-WASM expects one, over `worker_threads`. */
class NodeWorker {
  private readonly worker: Worker;
  private readonly handlers = new Map<unknown, (data: unknown) => void>();
  constructor(file: string) {
    const boot = [
      "const { parentPort } = require('node:worker_threads');",
      "globalThis.postMessage = (m, t) => parentPort.postMessage(m, t);",
      "parentPort.on('message', (data) => globalThis.onmessage && globalThis.onmessage({ data }));",
      `require(${JSON.stringify(file)});`,
    ].join("\n");
    this.worker = new Worker(boot, { eval: true });
  }
  postMessage(message: unknown, transfer?: Transferable[]) {
    this.worker.postMessage(message, transfer as never);
  }
  addEventListener(type: string, fn: (event: unknown) => void) {
    const handler = (data: unknown) => fn(type === "message" ? { data } : data);
    this.handlers.set(fn, handler);
    this.worker.on(type, handler);
  }
  removeEventListener(type: string, fn: (event: unknown) => void) {
    const handler = this.handlers.get(fn);
    if (handler) this.worker.off(type, handler);
  }
  terminate() {
    void this.worker.terminate();
  }
}

/** DuckDB-WASM's Node build, started. `terminate()` it when done. */
export async function bootDuckDb(): Promise<AsyncDuckDB> {
  const require = createRequire(import.meta.url);
  const duckdb = require(join(dist, "duckdb-node.cjs")) as typeof import("@duckdb/duckdb-wasm");
  const worker = new NodeWorker(join(dist, "duckdb-node-eh.worker.cjs"));
  const db = new duckdb.AsyncDuckDB(
    new duckdb.VoidLogger(),
    worker as unknown as globalThis.Worker,
  );
  await db.instantiate(join(dist, "duckdb-eh.wasm"));
  return db;
}

/** A request's JSON bytes, `method` first at both levels. */
export function body(group: string, method: string, params?: unknown): Uint8Array {
  const inner = params === undefined ? { method } : { method, params };
  return new TextEncoder().encode(JSON.stringify({ method: group, params: inner }));
}
