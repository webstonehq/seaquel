// A real trap inside seaquel-wasm, and `callWasm` recovering from it.
//
// The app's module has no way to trap on purpose (nothing in it may panic on
// user input), so this test builds its own: seaquel-wasm with the `test-trap`
// cargo feature, which adds a hidden `__test_trap(kind)` export. It's a debug
// build (a few seconds cold, well under one warm), run through the same
// wasm-bindgen and the same glue patch as `scripts/build-wasm.mjs`, and it
// stands in for `src/lib/wasm/pkg/` in this file only. The release module
// never has the export.
//
// Needs cargo, the wasm32-unknown-unknown target and wasm-bindgen-cli at the
// Cargo.lock version (WASM_BINDGEN overrides the binary), as
// `npm run wasm:build` does. Without them the test is skipped, except in CI,
// where it fails.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const outDir = join(root, "node_modules/.cache/seaquel-wasm-test-trap");
const pkgGlue = join(root, "src/lib/wasm/pkg/seaquel_wasm.js");

function run(cmd: string, args: string[]) {
  const r = spawnSync(cmd, args, { cwd: root, encoding: "utf8" });
  // A missing binary gives an `error` and no output.
  return { status: r.status, stdout: r.stdout ?? "", stderr: r.stderr ?? String(r.error ?? "") };
}

/** Why the toolchain can't build the test module, or `null` if it can. */
function missingToolchain(): string | null {
  if (run("cargo", ["--version"]).status !== 0) return "cargo isn't installed";
  const sysroot = run("rustc", ["--print", "sysroot"]).stdout.trim();
  if (!existsSync(join(sysroot, "lib/rustlib/wasm32-unknown-unknown"))) {
    return "the wasm32-unknown-unknown target isn't installed";
  }
  const want = run(process.execPath, ["scripts/build-wasm.mjs", "--bindgen-version"]).stdout.trim();
  const have = run(process.env.WASM_BINDGEN || "wasm-bindgen", ["--version"]).stdout.split(
    /\s+/,
  )[1];
  if (have !== want) return `wasm-bindgen ${want} isn't installed (found ${have ?? "none"})`;
  return null;
}

const missing = missingToolchain();
if (missing && process.env.CI) throw new Error(`wasm-trap.test.ts: ${missing}`);

/** `scripts/build-wasm.mjs`'s glue patch, read from the script so there's one copy. */
function reinstantiatePatch(): string {
  const script = readFileSync(join(root, "scripts/build-wasm.mjs"), "utf8");
  const js = script.match(/^const REINIT_JS = `([\s\S]*?)`;$/m)?.[1];
  if (!js) throw new Error("REINIT_JS not found in scripts/build-wasm.mjs");
  return js;
}

function buildTrapModule(): { glue: string; wasm: Buffer } {
  const meta = run("cargo", ["metadata", "--format-version", "1", "--no-deps"]);
  if (meta.status !== 0) throw new Error(meta.stderr);
  const targetDir = (JSON.parse(meta.stdout) as { target_directory: string }).target_directory;
  const cargo = run("cargo", [
    "build",
    "--target",
    "wasm32-unknown-unknown",
    "-p",
    "seaquel-wasm",
    "--features",
    "test-trap",
  ]);
  if (cargo.status !== 0) throw new Error(`cargo build failed:\n${cargo.stderr}`);
  mkdirSync(outDir, { recursive: true });
  const bindgen = run(process.env.WASM_BINDGEN || "wasm-bindgen", [
    "--target",
    "web",
    "--out-dir",
    outDir,
    join(targetDir, "wasm32-unknown-unknown/debug/seaquel_wasm.wasm"),
  ]);
  if (bindgen.status !== 0) throw new Error(`wasm-bindgen failed:\n${bindgen.stderr}`);
  const glue = join(outDir, "seaquel_wasm.js");
  const src = readFileSync(glue, "utf8");
  if (
    !/^let wasmModule, wasmInstance, wasm;$/m.test(src) ||
    !/^function initSync\(module\)/m.test(src)
  ) {
    throw new Error(
      "wasm-bindgen's glue has changed shape; see patchGlue in scripts/build-wasm.mjs",
    );
  }
  writeFileSync(glue, src + reinstantiatePatch());
  return { glue, wasm: readFileSync(join(outDir, "seaquel_wasm_bg.wasm")) };
}

type Glue = typeof import("$lib/wasm/pkg/seaquel_wasm.js") & {
  __test_trap(kind: string): string;
};

describe.skipIf(missing !== null)("a real trap in seaquel-wasm", () => {
  let wasmIndex: typeof import("$lib/wasm");
  let sql: typeof import("./index");
  let reinstantiate: ReturnType<typeof vi.fn>;

  beforeAll(async () => {
    const built = buildTrapModule();
    const glue = (await import(/* @vite-ignore */ pathToFileURL(built.glue).href)) as Glue;
    reinstantiate = vi.fn(() => glue.__seaquel_reinstantiate());
    // `$lib/wasm` imports the generated glue as ./pkg/seaquel_wasm.js; in
    // this file it gets the test module's glue instead.
    vi.doMock(pkgGlue, () => ({ ...glue, __seaquel_reinstantiate: reinstantiate }));
    vi.resetModules();
    wasmIndex = await import("$lib/wasm");
    sql = await import("./index");
    wasmIndex.initSeaquelWasmSync(built.wasm);
  }, 600_000);

  afterEach(() => {
    vi.restoreAllMocks();
    reinstantiate.mockClear();
  });

  const trap = (kind: string) =>
    wasmIndex.callWasm((m) => (m as unknown as Glue).__test_trap(kind));

  const works = () => {
    expect(sql.splitSqlStatements("SELECT '東京'; SELECT 2", "postgres").map((s) => s.sql)).toEqual(
      ["SELECT '東京'", "SELECT 2"],
    );
    expect(sql.getParseError("SELECT 1", "postgres")).toBeNull();
  };

  it("is the test module, and it works before any trap", () => {
    expect(trap("nothing")).toBe('{"error":"unknown trap kind \\"nothing\\""}');
    works();
  });

  it.each([
    ["panic", WebAssembly.RuntimeError, /unreachable/],
    ["stack", RangeError, /call stack/],
    ["shadow-stack", WebAssembly.RuntimeError, /out of bounds/],
  ] as const)("recovers from a %s trap", (kind, errorClass, message) => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    let thrown: unknown;
    try {
      trap(kind);
    } catch (e) {
      thrown = e;
    }
    expect(thrown).toBeInstanceOf(errorClass);
    expect(String(thrown)).toMatch(message);
    expect(log).toHaveBeenCalledOnce();
    expect(reinstantiate).toHaveBeenCalledOnce();
    // The next call gets a correct result, on a fresh instance.
    works();
    expect(reinstantiate).toHaveBeenCalledOnce();
  });

  it("recovers from traps in a row", () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    for (let i = 0; i < 20; i++) {
      expect(() => trap(i % 2 ? "stack" : "panic")).toThrow();
    }
    expect(reinstantiate).toHaveBeenCalledTimes(20);
    works();
  });
});
