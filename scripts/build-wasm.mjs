#!/usr/bin/env node
// Builds the app's two WebAssembly modules (both gitignored):
//
// - the editor module, crates/seaquel-wasm, into src/lib/wasm/pkg/, which the
//   app imports everywhere. Every npm script that runs Vite or svelte-check
//   runs this first.
// - the browser module, crates/seaquel-browser (phase 8: Core in the demo's
//   page), into src/lib/wasm/browser-pkg/. Only the demo's scripts build it
//   (`predev:demo`, `prebuild:demo`), and `pretest` builds its test variant
//   (`--test-hooks`, into src/lib/wasm/browser-test-pkg/). It compiles
//   SQLite's C for wasm32, so it also needs a clang with the wasm32 backend
//   (see "clang" below).
//
//   node scripts/build-wasm.mjs                   the editor module: cargo build, wasm-bindgen, wasm-opt
//   node scripts/build-wasm.mjs --module browser  the browser module, the same steps; wasm-opt is kept
//                                                 only when it makes the brotli size smaller, and the
//                                                 brotli size must stay within BROWSER_BUDGET_BYTES
//   ... --module browser --test-hooks             the browser module with its test exports
//                                                 (`__test_trap`, …), for vitest; no budget
//   node scripts/build-wasm.mjs --bindgen-version print the wasm-bindgen version from Cargo.lock
//   node scripts/build-wasm.mjs --opt-only        finish an editor pkg/ that wasm-bindgen wrote
//                                                 elsewhere: the glue patch and wasm-opt only (Docker)
//   SEAQUEL_WASM_PREBUILT=1 node scripts/...      check that the module's finished pkg/ exists and stop
//                                                 (Docker, or a machine without the Rust toolchain)
//   SEAQUEL_WASM_BUDGET_BYTES=<n>                 the browser module's brotli budget instead of
//                                                 BROWSER_BUDGET_BYTES (to see the check fail)
//
// Needs the wasm32-unknown-unknown Rust target and wasm-bindgen-cli at the
// exact version in Cargo.lock (WASM_BINDGEN=<path> overrides the binary).
// wasm-opt comes from the `binaryen` npm package. Respects CARGO_TARGET_DIR.
// If that toolchain is missing it exits 1, even when an older pkg/ exists: a
// stale pkg/ would silently run old SQL checks (the AI read-only check among
// them). SEAQUEL_WASM_PREBUILT=1 is the way to use an existing pkg/ on purpose.
//
// clang (the browser module only): sqlite-wasm-rs compiles SQLite with
// `clang --target=wasm32-unknown-unknown`, and Apple's clang has no wasm32
// backend. The script uses, in order: CC_wasm32_unknown_unknown, Homebrew's
// llvm (`brew install llvm`), or a `clang` on PATH whose `--print-targets`
// lists wasm32; and AR_wasm32_unknown_unknown, else the llvm-ar next to that
// clang, `llvm-ar`, or `llvm-ar-<major>` (wasm-ld rejects GNU ar's archives).
// It stops with the fix when it finds none.
//
// When the raw .wasm cargo produced hasn't changed since the last run (a stamp
// in the pkg directory, which also covers the wasm-bindgen and binaryen
// versions, this script and the module's features), the wasm-bindgen and
// wasm-opt steps are skipped.
//
// Node, not bash, because the release builds on Windows too.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { brotliCompressSync, constants as zlib, gzipSync } from "node:zlib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const TARGET = "wasm32-unknown-unknown";
const PROFILE = "wasm-release";
const WASM_OPT_FLAGS = [
  "-Oz",
  "--enable-bulk-memory",
  "--enable-nontrapping-float-to-int",
  "--enable-sign-ext",
];

/**
 * The browser module's size budget, brotli 11, in bytes (2.0 MB). The build fails past it.
 */
const BROWSER_BUDGET_BYTES = 2_000_000;

const argv = process.argv.slice(2);
const args = new Set(argv);

function fail(message) {
  console.error(`\nbuild-wasm: ${message}\n`);
  process.exit(1);
}

function option(name) {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
}

const moduleName = option("--module") ?? "editor";
const testHooks = args.has("--test-hooks");
if (!["editor", "browser"].includes(moduleName)) {
  fail(`unknown --module ${moduleName}: expected editor or browser.`);
}
if (testHooks && moduleName !== "browser") fail("--test-hooks is for --module browser only.");

/**
 * What each module is. `wasmOpt`: "always" (the editor module ships -Oz) or
 * "smaller" (kept only when it makes the brotli size smaller; S3 found -Oz
 * made the browser module larger compressed). `clang`: it compiles C for
 * wasm32 (SQLite).
 */
const MODULE = {
  editor: {
    crate: "seaquel-wasm",
    name: "seaquel_wasm",
    pkg: "src/lib/wasm/pkg",
    features: [],
    wasmOpt: "always",
    clang: false,
    budget: null,
    patch: patchEditorGlue,
  },
  browser: {
    crate: "seaquel-browser",
    name: "seaquel_browser",
    pkg: testHooks ? "src/lib/wasm/browser-test-pkg" : "src/lib/wasm/browser-pkg",
    features: testHooks ? ["test-hooks"] : [],
    // The test variant is for vitest in Node: no wasm-opt, no budget.
    wasmOpt: testHooks ? "never" : "smaller",
    clang: true,
    budget: testHooks
      ? null
      : Number(process.env.SEAQUEL_WASM_BUDGET_BYTES || BROWSER_BUDGET_BYTES),
    patch: patchBrowserGlue,
  },
}[moduleName];

const NAME = MODULE.name;
const pkgDir = join(root, MODULE.pkg);
// Per process, so two builds started at once (say `dev` and `check`) don't
// write into the same directory.
const tmpDir = join(root, `src/lib/wasm/.pkg-tmp-${process.pid}`);
const PKG_FILES = [`${NAME}.js`, `${NAME}.d.ts`, `${NAME}_bg.wasm`, `${NAME}_bg.wasm.d.ts`];
const STAMP = join(pkgDir, ".stamp");

function run(cmd, cmdArgs, { capture = false, env } = {}) {
  const r = spawnSync(cmd, cmdArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
    env: env ? { ...process.env, ...env } : process.env,
  });
  return r;
}

function bindgenVersion() {
  const lock = readFileSync(join(root, "Cargo.lock"), "utf8");
  const m = lock.match(/\[\[package\]\]\s*\nname = "wasm-bindgen"\s*\nversion = "([^"]+)"/);
  if (!m) fail("couldn't find wasm-bindgen in Cargo.lock.");
  return m[1];
}

function wasmOptPath() {
  try {
    const require = createRequire(join(root, "package.json"));
    const path = join(dirname(require.resolve("binaryen/package.json")), "bin", "wasm-opt");
    if (existsSync(path)) return path;
  } catch {
    // fall through
  }
  fail("wasm-opt not found. It comes from the `binaryen` npm package: run `npm install`.");
}

function binaryenVersion() {
  const require = createRequire(join(root, "package.json"));
  return JSON.parse(readFileSync(require.resolve("binaryen/package.json"), "utf8")).version;
}

// On Windows, an editor, indexer or the Vite watcher can hold a file in pkg/
// open for a moment, and rm/rename fail with EPERM or EBUSY. Retry briefly.
function retrying(fn) {
  for (let attempt = 0; ; attempt++) {
    try {
      return fn();
    } catch (e) {
      if (attempt >= 20 || !["EPERM", "EBUSY", "EACCES"].includes(e?.code)) throw e;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100 * (attempt + 1));
    }
  }
}

function brotliSize(wasm) {
  return brotliCompressSync(wasm, {
    params: { [zlib.BROTLI_PARAM_QUALITY]: 11, [zlib.BROTLI_PARAM_SIZE_HINT]: wasm.length },
  }).length;
}

/**
 * Runs wasm-opt on `file` in place. With `onlyIfSmaller`, the result is
 * kept only when its brotli size is smaller; returns whether it was kept.
 */
function wasmOpt(file, onlyIfSmaller = false) {
  const out = `${file}.opt`;
  // binaryen's bin/wasm-opt is a Node script; run it with this Node so it
  // works on Windows, where node_modules/.bin has a .cmd shim instead.
  const r = run(process.execPath, [wasmOptPath(), ...WASM_OPT_FLAGS, file, "-o", out]);
  if (r.status !== 0) fail("wasm-opt failed.");
  if (onlyIfSmaller) {
    const before = brotliSize(readFileSync(file));
    const after = brotliSize(readFileSync(out));
    const kb = (n) => `${(n / 1024).toFixed(1)} KB`;
    if (after >= before) {
      rmSync(out, { force: true });
      console.log(
        `build-wasm: wasm-opt skipped (brotli ${kb(after)} with it, ${kb(before)} without)`,
      );
      return false;
    }
    console.log(`build-wasm: wasm-opt kept (brotli ${kb(after)} with it, ${kb(before)} without)`);
  }
  retrying(() => renameSync(out, file));
  return true;
}

function pkgFiles() {
  return PKG_FILES.filter((f) => !existsSync(join(pkgDir, f)));
}

function checkPkg() {
  const missing = pkgFiles();
  if (missing.length) {
    const how =
      moduleName === "editor"
        ? "npm run wasm:build"
        : `node scripts/build-wasm.mjs ${argv.join(" ")}`;
    fail(`${MODULE.pkg}/ is missing ${missing.join(", ")}. Run \`${how}\`.`);
  }
}

// A pkg/ the app can use: every file, and the glue has the reinstantiate patch.
function pkgFinished() {
  return (
    pkgFiles().length === 0 &&
    readFileSync(join(pkgDir, `${NAME}.js`), "utf8").includes(REINIT_MARKER)
  );
}

// The crate's build.rs links the module with this stack. On
// wasm32-unknown-unknown the stack comes first in memory, so the first
// mutable i32 global (`__stack_pointer`) starts at the stack size.
const STACK_SIZE = 2 * 1024 * 1024;

function readLeb(buf, pos, signed) {
  let result = 0;
  let shift = 0;
  let byte;
  do {
    byte = buf[pos++];
    result |= (byte & 0x7f) << shift;
    shift += 7;
  } while (byte & 0x80);
  if (signed && shift < 32 && byte & 0x40) result |= -1 << shift;
  return [result, pos];
}

function stackSize(wasm) {
  let pos = 8; // magic + version
  while (pos < wasm.length) {
    const id = wasm[pos++];
    let size;
    [size, pos] = readLeb(wasm, pos, false);
    if (id === 6) {
      let count;
      let p = pos;
      [count, p] = readLeb(wasm, p, false);
      for (let i = 0; i < count; i++) {
        const type = wasm[p++];
        const mutable = wasm[p++];
        if (wasm[p] !== 0x41) return null; // not i32.const
        let value;
        [value, p] = readLeb(wasm, p + 1, true);
        if (wasm[p++] !== 0x0b) return null;
        if (type === 0x7f && mutable === 1) return value;
      }
      return null;
    }
    pos += size;
  }
  return null;
}

function checkStack(wasm) {
  const size = stackSize(wasm);
  if (size !== STACK_SIZE) {
    fail(
      `the module's stack is ${size ?? "unknown"} bytes, expected ${STACK_SIZE}: ` +
        `crates/${MODULE.crate}/build.rs sets it with -zstack-size.`,
    );
  }
}

/** Prints the sizes and returns the brotli size. */
function printSizes(raw) {
  const wasm = readFileSync(join(pkgDir, `${NAME}_bg.wasm`));
  const kb = (n) => `${(n / 1024).toFixed(1)} KB`;
  const gz = gzipSync(wasm, { level: 9 }).length;
  const br = brotliSize(wasm);
  const lines = [];
  if (raw !== undefined) lines.push(`cargo output ${kb(raw)}`);
  lines.push(`${NAME}_bg.wasm ${kb(wasm.length)}`, `gzip -9 ${kb(gz)}`, `brotli 11 ${kb(br)}`);
  lines.push(`JS glue ${kb(statSync(join(pkgDir, `${NAME}.js`)).size)}`);
  lines.push(`stack ${kb(stackSize(wasm) ?? 0)}`);
  console.log(`build-wasm: ${lines.join(", ")}`);
  return br;
}

/** Fails the build when the module's brotli size is past its budget. */
function checkBudget(br) {
  if (MODULE.budget === null) return;
  if (br > MODULE.budget) {
    fail(
      `${NAME}_bg.wasm is ${br} bytes brotli, over its budget of ${MODULE.budget} bytes ` +
        "(phase 8, Q3: 2.0 MB). Find what grew (twiggy, or cargo bloat for wasm32) before raising it.",
    );
  }
  console.log(`build-wasm: within the brotli budget (${br} of ${MODULE.budget} bytes)`);
}

const REINIT_MARKER = "export function __seaquel_reinstantiate()";
const REINIT_DTS = `
/** A fresh instance of the same compiled module, for after a trap. */
export function __seaquel_reinstantiate(): InitOutput;
`;
const BROWSER_REINIT_DTS = `${REINIT_DTS}
/** Whether \`error\` is a trap this module's live instance threw through one of its closures. */
export function __seaquel_isCurrentTrap(error: unknown): boolean;
`;

function checkGlueShape(src) {
  if (
    !/^let wasmModule, wasmInstance, wasm;$/m.test(src) ||
    !/^function initSync\(module\)/m.test(src)
  ) {
    fail(
      "wasm-bindgen's glue has changed shape: `let wasmModule, wasmInstance, wasm;` or `function initSync(module)` " +
        "is missing, so __seaquel_reinstantiate can't be added. Update the glue patch in scripts/build-wasm.mjs.",
    );
  }
}

// Lets src/lib/wasm/index.ts re-instantiate the module after a trap.
// wasm-bindgen's initSync returns early once `wasm` is set, and its
// --experimental-reset-state-function output calls a `__wbindgen_start` this
// module doesn't export, so the glue gets one small function of our own.
const REINIT_JS = `
// Added by scripts/build-wasm.mjs: a fresh instance of the same compiled
// module, for after a trap (see src/lib/wasm/index.ts).
export function __seaquel_reinstantiate() {
    const module = wasmModule;
    wasm = undefined;
    return initSync({ module });
}
`;

function patchEditorGlue(dir) {
  const js = join(dir, `${NAME}.js`);
  const src = readFileSync(js, "utf8");
  if (src.includes(REINIT_MARKER)) return;
  // Reinstantiating swaps the instance under the glue. JS objects that point
  // into the old instance's memory (exported structs, which the glue tracks
  // with a FinalizationRegistry) wouldn't survive that, so the exports must
  // stay plain functions over strings and numbers.
  if (/\bFinalizationRegistry\b/.test(src) || /^export class /m.test(src)) {
    fail(
      "the glue exports a class or uses FinalizationRegistry. seaquel-wasm must export only plain " +
        "functions: src/lib/wasm/index.ts re-instantiates the module after a trap, which would leave " +
        "such objects pointing into a dead instance.",
    );
  }
  checkGlueShape(src);
  writeFileSync(js, src + REINIT_JS);
  const dts = join(dir, `${NAME}.d.ts`);
  writeFileSync(dts, readFileSync(dts, "utf8") + REINIT_DTS);
}

// The browser module's version (phase 8). Its async exports
// hand JavaScript closures (promise callbacks, timers) that call back into
// the instance that made them, and the glue routes every such call, and
// every closure's destructor, through its one `wasm` variable. After a swap
// those would reach the new instance with the old instance's pointers. So
// each closure records the instance generation it was made in, and once the
// generation moves on it does nothing: a late DuckDB answer or timer meant
// for the trapped instance is dropped, and its destructor never runs against
// the new one. The trapped instance's memory is simply let go.
//
// What leaks: the glue's JS object heap (`heap`) is shared by every
// instance, and the slots the dead instance held (the bridge, its callbacks,
// promises it was waiting on) are never freed, since only that instance
// could drop them. That is a bounded amount per trap (tens of entries), and
// the transport stops restarting after three traps a minute.
const BROWSER_REINIT_JS = `
// Added by scripts/build-wasm.mjs: a fresh instance of the same compiled
// module, for after a trap (see src/lib/core/browser/transport.ts). Closures
// the old instance made are neutered first (__seaquel_generation).
export function __seaquel_reinstantiate() {
    __seaquel_generation++;
    const module = wasmModule;
    wasm = undefined;
    return initSync({ module });
}

// Traps thrown through this module's closures (its task queue, timers,
// promise callbacks), with the instance generation they came from, so the
// page can tell this module's trap from another's when the stack names no
// module (WebKit) and never restarts twice for one trap.
const __seaquel_traps = new WeakMap();
function __seaquel_noteTrap(error, generation) {
    if (error instanceof WebAssembly.RuntimeError) __seaquel_traps.set(error, generation);
}
export function __seaquel_isCurrentTrap(error) {
    return error !== null && typeof error === "object" && __seaquel_traps.get(error) === __seaquel_generation;
}
`;

/** `src` with `from` replaced by `to`, which must occur exactly once. */
function patchOnce(src, from, to, what) {
  const count =
    typeof from === "string"
      ? src.split(from).length - 1
      : (src.match(new RegExp(from.source, "g")) ?? []).length;
  if (count !== 1) {
    fail(
      `wasm-bindgen's glue has changed shape: expected one ${what}, found ${count}. ` +
        "Update patchBrowserGlue in scripts/build-wasm.mjs.",
    );
  }
  return src.replace(from, to);
}

function patchBrowserGlue(dir) {
  const js = join(dir, `${NAME}.js`);
  let src = readFileSync(js, "utf8");
  if (src.includes(REINIT_MARKER)) return;
  if (/^export class /m.test(src)) {
    fail(
      "the browser module's glue exports a class. It must export only plain functions: the " +
        "transport re-instantiates the module after a trap, which would leave such objects " +
        "pointing into a dead instance.",
    );
  }
  // Only the closures this patch neuters may reach the instance later.
  if (/function makeClosure\(/.test(src)) {
    fail(
      "the glue has makeClosure (an immutable closure), which patchBrowserGlue doesn't guard yet.",
    );
  }
  checkGlueShape(src);
  src = patchOnce(
    src,
    /const CLOSURE_DTORS = \(typeof FinalizationRegistry === 'undefined'\)\n {4}\? \{ register: \(\) => \{\}, unregister: \(\) => \{\} \}\n {4}: new FinalizationRegistry\(state => (wasm\.__wbindgen_export\d+)\(state\.a, state\.b\)\);/,
    "let __seaquel_generation = 0;\n\n" +
      "const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')\n" +
      "    ? { register: () => {}, unregister: () => {} }\n" +
      "    : new FinalizationRegistry(state => { if (state.g === __seaquel_generation) $1(state.a, state.b); });",
    "closure destructor registry",
  );
  src = patchOnce(
    src,
    "const state = { a: arg0, b: arg1, cnt: 1 };\n    const real = (...args) => {\n",
    "const state = { a: arg0, b: arg1, cnt: 1, g: __seaquel_generation };\n    const real = (...args) => {\n" +
      "        if (state.g !== __seaquel_generation) return;\n",
    "makeMutClosure state",
  );
  src = patchOnce(
    src,
    "        try {\n            return f(a, state.b, ...args);\n        } finally {\n",
    "        try {\n            return f(a, state.b, ...args);\n        } catch (e) {\n" +
      "            __seaquel_noteTrap(e, state.g);\n            throw e;\n        } finally {\n",
    "closure call",
  );
  src = patchOnce(
    src,
    "real._wbg_cb_unref = () => {\n",
    "real._wbg_cb_unref = () => {\n        if (state.g !== __seaquel_generation) return;\n",
    "closure unref",
  );
  writeFileSync(js, src + BROWSER_REINIT_JS);
  const dts = join(dir, `${NAME}.d.ts`);
  writeFileSync(dts, readFileSync(dts, "utf8") + BROWSER_REINIT_DTS);
}

// --- clang (the browser module) -----------------------------------------------

/** Whether `clang` runs and lists the wasm32 backend. */
function hasWasmBackend(clang) {
  const r = run(clang, ["--print-targets"], { capture: true });
  return r.status === 0 && /\bwasm32\b/.test(r.stdout);
}

function clangFix() {
  return process.platform === "darwin"
    ? "Install LLVM once with `brew install llvm` (Apple's clang has no wasm32 backend), or point " +
        "CC_wasm32_unknown_unknown and AR_wasm32_unknown_unknown at an LLVM clang and llvm-ar."
    : "Install clang and llvm (e.g. `sudo apt-get install clang llvm`), or point " +
        "CC_wasm32_unknown_unknown and AR_wasm32_unknown_unknown at an LLVM clang and llvm-ar.";
}

/**
 * The clang and archiver for SQLite's C: `{cc, ar, from}`. Order:
 * CC_wasm32_unknown_unknown, Homebrew's llvm, a `clang` on PATH with the
 * wasm32 backend. The archiver: AR_wasm32_unknown_unknown, the llvm-ar next
 * to that clang, `llvm-ar`, `llvm-ar-<major>`.
 */
function findClang() {
  let cc = null;
  let from = null;
  const envCc = process.env.CC_wasm32_unknown_unknown;
  if (envCc) {
    if (!hasWasmBackend(envCc)) {
      fail(
        `CC_wasm32_unknown_unknown is ${envCc}, which doesn't run or has no wasm32 backend ` +
          `(\`${envCc} --print-targets\`). ${clangFix()}`,
      );
    }
    [cc, from] = [envCc, "CC_wasm32_unknown_unknown"];
  }
  if (!cc) {
    const prefixes = [];
    const brew = run("brew", ["--prefix", "llvm"], { capture: true });
    if (brew.status === 0 && brew.stdout.trim()) prefixes.push(brew.stdout.trim());
    prefixes.push("/opt/homebrew/opt/llvm", "/usr/local/opt/llvm");
    for (const prefix of prefixes) {
      const candidate = join(prefix, "bin", "clang");
      if (existsSync(candidate) && hasWasmBackend(candidate)) {
        [cc, from] = [candidate, "Homebrew's llvm"];
        break;
      }
    }
  }
  if (!cc && hasWasmBackend("clang")) [cc, from] = ["clang", "clang on PATH"];
  if (!cc) {
    fail(
      "the browser module compiles SQLite for wasm32 and needs a clang with the wasm32 backend; " +
        `none was found (CC_wasm32_unknown_unknown, Homebrew's llvm, clang on PATH). ${clangFix()}`,
    );
  }

  const runs = (ar) => run(ar, ["--version"], { capture: true }).status === 0;
  let ar = process.env.AR_wasm32_unknown_unknown || null;
  if (ar && !runs(ar)) fail(`AR_wasm32_unknown_unknown is ${ar}, which doesn't run. ${clangFix()}`);
  if (!ar && cc.includes("/")) {
    const sibling = join(dirname(cc), process.platform === "win32" ? "llvm-ar.exe" : "llvm-ar");
    if (existsSync(sibling)) ar = sibling;
  }
  if (!ar && runs("llvm-ar")) ar = "llvm-ar";
  if (!ar) {
    const major = run(cc, ["--version"], { capture: true }).stdout.match(
      /clang version (\d+)/,
    )?.[1];
    if (major && runs(`llvm-ar-${major}`)) ar = `llvm-ar-${major}`;
  }
  if (!ar) {
    fail(
      `found ${cc} (${from}) but no llvm-ar to go with it; wasm-ld rejects GNU ar's archives. ${clangFix()}`,
    );
  }
  return { cc, ar, from };
}

// --- modes -------------------------------------------------------------------

if (args.has("--bindgen-version")) {
  console.log(bindgenVersion());
  process.exit(0);
}

if (process.env.SEAQUEL_WASM_PREBUILT === "1") {
  checkPkg();
  if (!pkgFinished()) {
    fail(
      `${MODULE.pkg}/ hasn't been finished: the glue lacks __seaquel_reinstantiate. ` +
        (moduleName === "editor"
          ? "Run `node scripts/build-wasm.mjs --opt-only` on it first."
          : "Build it with this script."),
    );
  }
  console.log(`build-wasm: SEAQUEL_WASM_PREBUILT=1, using the existing ${MODULE.pkg}/`);
  process.exit(0);
}

if (args.has("--opt-only")) {
  if (moduleName !== "editor") fail("--opt-only is for the editor module (the Docker image) only.");
  checkPkg();
  patchEditorGlue(pkgDir);
  wasmOpt(join(pkgDir, `${NAME}_bg.wasm`));
  checkStack(readFileSync(join(pkgDir, `${NAME}_bg.wasm`)));
  printSizes();
  process.exit(0);
}

// --- toolchain checks ---------------------------------------------------------

const wantBindgen = bindgenVersion();
const installBindgen = `cargo install wasm-bindgen-cli --version ${wantBindgen} --locked`;

// A missing toolchain is fatal. Keeping an older pkg/ would run the app with
// whatever SQL checks it was built with, without anyone noticing.
function toolchainMissing(message) {
  const keep = pkgFinished()
    ? `\n\nTo run with the existing ${MODULE.pkg}/ anyway (it may be out of date: changes in ` +
      "the crates won't be in it), set SEAQUEL_WASM_PREBUILT=1."
    : "";
  fail(message + keep);
}

if (run("cargo", ["--version"], { capture: true }).status !== 0) {
  toolchainMissing(
    "cargo not found. Install Rust from https://rustup.rs, then run:\n  rustup target add wasm32-unknown-unknown\n  " +
      installBindgen,
  );
}

// rustc from the repo root, so rust-toolchain.toml picks the toolchain.
const sysroot = run("rustc", ["--print", "sysroot"], { capture: true });
if (sysroot.status !== 0 || !existsSync(join(sysroot.stdout.trim(), "lib", "rustlib", TARGET))) {
  toolchainMissing(
    `the ${TARGET} Rust target isn't installed. Run:\n  rustup target add ${TARGET}`,
  );
}

const bindgen = process.env.WASM_BINDGEN || "wasm-bindgen";
const haveBindgen = run(bindgen, ["--version"], { capture: true });
const haveVersion = haveBindgen.status === 0 ? haveBindgen.stdout.trim().split(/\s+/)[1] : null;
if (haveVersion !== wantBindgen) {
  toolchainMissing(
    (haveVersion
      ? `wasm-bindgen ${haveVersion} is installed, but Cargo.lock has ${wantBindgen}. They must match exactly ` +
        '(update "cargo:wasm-bindgen-cli" in mise.toml too).'
      : `wasm-bindgen-cli isn't installed (looked for \`${bindgen}\`).`) +
      `\nInstall it with \`mise install\` (mise.toml pins it), or:\n  ${installBindgen}`,
  );
}

wasmOptPath();

let cargoEnv;
if (MODULE.clang) {
  const { cc, ar, from } = findClang();
  console.log(`build-wasm: SQLite's C with ${cc} and ${ar} (${from})`);
  cargoEnv = { CC_wasm32_unknown_unknown: cc, AR_wasm32_unknown_unknown: ar };
}

// --- build ---------------------------------------------------------------------

const meta = run("cargo", ["metadata", "--format-version", "1", "--no-deps"], { capture: true });
if (meta.status !== 0) fail(`cargo metadata failed:\n${meta.stderr}`);
const targetDir = JSON.parse(meta.stdout).target_directory;

const cargoArgs = ["build", "--profile", PROFILE, "--target", TARGET, "-p", MODULE.crate];
if (MODULE.features.length) cargoArgs.push("--features", MODULE.features.join(","));
const cargo = run("cargo", cargoArgs, { env: cargoEnv });
if (cargo.status !== 0) fail("cargo build failed.");

const rawPath = join(targetDir, TARGET, PROFILE, `${NAME}.wasm`);
const raw = readFileSync(rawPath);
checkStack(raw);
const stamp = createHash("sha256")
  .update(raw)
  .update(wantBindgen)
  .update(binaryenVersion())
  .update(readFileSync(fileURLToPath(import.meta.url)))
  .update(JSON.stringify([moduleName, MODULE.features, MODULE.budget]))
  .digest("hex");

const upToDate =
  existsSync(STAMP) &&
  readFileSync(STAMP, "utf8").trim() === stamp &&
  PKG_FILES.every((f) => existsSync(join(pkgDir, f)));

if (upToDate) {
  // Touched, so its time says when the module was last checked against its
  // sources (the browser module's vitest harness compares it with them).
  const now = new Date();
  utimesSync(STAMP, now, now);
  console.log(`build-wasm: ${MODULE.pkg}/ is up to date`);
} else {
  process.on("exit", () => rmSync(tmpDir, { recursive: true, force: true }));
  mkdirSync(tmpDir, { recursive: true });
  const bg = run(bindgen, ["--target", "web", "--out-dir", tmpDir, rawPath]);
  if (bg.status !== 0) fail("wasm-bindgen failed.");
  MODULE.patch(tmpDir);
  if (MODULE.wasmOpt !== "never") {
    wasmOpt(join(tmpDir, `${NAME}_bg.wasm`), MODULE.wasmOpt === "smaller");
  }
  checkStack(readFileSync(join(tmpDir, `${NAME}_bg.wasm`)));
  if (MODULE.budget !== null) {
    // Before the pkg directory is replaced, so an oversized module never
    // lands where the build would pick it up.
    const br = brotliSize(readFileSync(join(tmpDir, `${NAME}_bg.wasm`)));
    if (br > MODULE.budget) checkBudget(br);
  }
  writeFileSync(join(tmpDir, ".stamp"), `${stamp}\n`);
  retrying(() => rmSync(pkgDir, { recursive: true, force: true }));
  retrying(() => renameSync(tmpDir, pkgDir));
}

checkBudget(printSizes(raw.length));
