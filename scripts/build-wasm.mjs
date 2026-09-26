#!/usr/bin/env node
// Builds crates/seaquel-wasm into src/lib/wasm/pkg/ (gitignored), which the
// app imports. Every npm script that runs Vite or svelte-check runs this first.
//
//   node scripts/build-wasm.mjs                   cargo build, wasm-bindgen, wasm-opt
//   node scripts/build-wasm.mjs --bindgen-version print the wasm-bindgen version from Cargo.lock
//   node scripts/build-wasm.mjs --opt-only        finish a pkg/ that wasm-bindgen wrote elsewhere:
//                                                 the glue patch and wasm-opt only (Docker)
//   SEAQUEL_WASM_PREBUILT=1 node scripts/...      check that a finished pkg/ exists and stop
//                                                 (Docker, or a machine without the Rust toolchain)
//
// Needs the wasm32-unknown-unknown Rust target and wasm-bindgen-cli at the
// exact version in Cargo.lock (WASM_BINDGEN=<path> overrides the binary).
// wasm-opt comes from the `binaryen` npm package. Respects CARGO_TARGET_DIR.
// If that toolchain is missing but a finished pkg/ exists, it warns and keeps
// the existing pkg/, so frontend-only work goes on; without pkg/ it exits 1.
//
// When the raw .wasm cargo produced hasn't changed since the last run (a stamp
// in pkg/, which also covers the wasm-bindgen and binaryen versions), the
// wasm-bindgen and wasm-opt steps are skipped.
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
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { brotliCompressSync, constants as zlib, gzipSync } from "node:zlib";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pkgDir = join(root, "src/lib/wasm/pkg");
// Per process, so two builds started at once (say `dev` and `check`) don't
// write into the same directory.
const tmpDir = join(root, `src/lib/wasm/.pkg-tmp-${process.pid}`);
const NAME = "seaquel_wasm";
const TARGET = "wasm32-unknown-unknown";
const PROFILE = "wasm-release";
const PKG_FILES = [`${NAME}.js`, `${NAME}.d.ts`, `${NAME}_bg.wasm`, `${NAME}_bg.wasm.d.ts`];
const STAMP = join(pkgDir, ".stamp");
const WASM_OPT_FLAGS = [
  "-Oz",
  "--enable-bulk-memory",
  "--enable-nontrapping-float-to-int",
  "--enable-sign-ext",
];

const args = new Set(process.argv.slice(2));

function fail(message) {
  console.error(`\nbuild-wasm: ${message}\n`);
  process.exit(1);
}

function run(cmd, cmdArgs, { capture = false } = {}) {
  const r = spawnSync(cmd, cmdArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
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

function wasmOpt(file) {
  const out = `${file}.opt`;
  // binaryen's bin/wasm-opt is a Node script; run it with this Node so it
  // works on Windows, where node_modules/.bin has a .cmd shim instead.
  const r = run(process.execPath, [wasmOptPath(), ...WASM_OPT_FLAGS, file, "-o", out]);
  if (r.status !== 0) fail("wasm-opt failed.");
  retrying(() => renameSync(out, file));
}

function pkgFiles() {
  return PKG_FILES.filter((f) => !existsSync(join(pkgDir, f)));
}

function checkPkg() {
  const missing = pkgFiles();
  if (missing.length) {
    fail(`src/lib/wasm/pkg/ is missing ${missing.join(", ")}. Run \`npm run wasm:build\`.`);
  }
}

// A pkg/ the app can use: every file, and the glue has the reinstantiate patch.
function pkgFinished() {
  return (
    pkgFiles().length === 0 &&
    readFileSync(join(pkgDir, `${NAME}.js`), "utf8").includes(REINIT_MARKER)
  );
}

// crates/seaquel-wasm/build.rs links the module with this stack. On
// wasm32-unknown-unknown the stack comes first in memory, so the first mutable
// i32 global (`__stack_pointer`) starts at the stack size.
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
        "crates/seaquel-wasm/build.rs sets it with -zstack-size.",
    );
  }
}

function printSizes(raw) {
  const wasm = readFileSync(join(pkgDir, `${NAME}_bg.wasm`));
  const kb = (n) => `${(n / 1024).toFixed(1)} KB`;
  const gz = gzipSync(wasm, { level: 9 }).length;
  const br = brotliCompressSync(wasm, {
    params: { [zlib.BROTLI_PARAM_QUALITY]: 11, [zlib.BROTLI_PARAM_SIZE_HINT]: wasm.length },
  }).length;
  const lines = [];
  if (raw !== undefined) lines.push(`cargo output ${kb(raw)}`);
  lines.push(`${NAME}_bg.wasm ${kb(wasm.length)}`, `gzip -9 ${kb(gz)}`, `brotli 11 ${kb(br)}`);
  lines.push(`JS glue ${kb(statSync(join(pkgDir, `${NAME}.js`)).size)}`);
  lines.push(`stack ${kb(stackSize(wasm) ?? 0)}`);
  console.log(`build-wasm: ${lines.join(", ")}`);
}

// Lets src/lib/wasm/index.ts re-instantiate the module after a trap.
// wasm-bindgen's initSync returns early once `wasm` is set, and its
// --experimental-reset-state-function output calls a `__wbindgen_start` this
// module doesn't export, so the glue gets one small function of our own.
const REINIT_MARKER = "export function __seaquel_reinstantiate()";
const REINIT_JS = `
// Added by scripts/build-wasm.mjs: a fresh instance of the same compiled
// module, for after a trap (see src/lib/wasm/index.ts).
export function __seaquel_reinstantiate() {
    const module = wasmModule;
    wasm = undefined;
    return initSync({ module });
}
`;
const REINIT_DTS = `
/** A fresh instance of the same compiled module, for after a trap. */
export function __seaquel_reinstantiate(): InitOutput;
`;

function patchGlue(dir) {
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
  if (
    !/^let wasmModule, wasmInstance, wasm;$/m.test(src) ||
    !/^function initSync\(module\)/m.test(src)
  ) {
    fail(
      "wasm-bindgen's glue has changed shape: `let wasmModule, wasmInstance, wasm;` or `function initSync(module)` " +
        "is missing, so __seaquel_reinstantiate can't be added. Update patchGlue in scripts/build-wasm.mjs.",
    );
  }
  writeFileSync(js, src + REINIT_JS);
  const dts = join(dir, `${NAME}.d.ts`);
  writeFileSync(dts, readFileSync(dts, "utf8") + REINIT_DTS);
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
      "src/lib/wasm/pkg/ hasn't been finished: the glue lacks __seaquel_reinstantiate. " +
        "Run `node scripts/build-wasm.mjs --opt-only` on it first.",
    );
  }
  console.log("build-wasm: SEAQUEL_WASM_PREBUILT=1, using the existing src/lib/wasm/pkg/");
  process.exit(0);
}

if (args.has("--opt-only")) {
  checkPkg();
  patchGlue(pkgDir);
  wasmOpt(join(pkgDir, `${NAME}_bg.wasm`));
  checkStack(readFileSync(join(pkgDir, `${NAME}_bg.wasm`)));
  printSizes();
  process.exit(0);
}

// --- toolchain checks ---------------------------------------------------------

const wantBindgen = bindgenVersion();
const installBindgen = `cargo install wasm-bindgen-cli --version ${wantBindgen} --locked`;

// A missing toolchain is fatal only when there's no pkg/ to fall back on.
function toolchainMissing(message) {
  if (!pkgFinished()) fail(message);
  const bar = "!".repeat(78);
  console.warn(
    `\n${bar}\nbuild-wasm: ${message}\n\nKeeping the existing src/lib/wasm/pkg/, which may be out of date: ` +
      `changes in crates/seaquel-sql or crates/seaquel-wasm won't show up until this builds.\n` +
      `Set SEAQUEL_WASM_PREBUILT=1 to use pkg/ as it is without this warning.\n${bar}\n`,
  );
  process.exit(0);
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
      ? `wasm-bindgen ${haveVersion} is installed, but Cargo.lock has ${wantBindgen}. They must match exactly.`
      : `wasm-bindgen-cli isn't installed (looked for \`${bindgen}\`).`) +
      `\nInstall it with:\n  ${installBindgen}`,
  );
}

wasmOptPath();

// --- build ---------------------------------------------------------------------

const meta = run("cargo", ["metadata", "--format-version", "1", "--no-deps"], { capture: true });
if (meta.status !== 0) fail(`cargo metadata failed:\n${meta.stderr}`);
const targetDir = JSON.parse(meta.stdout).target_directory;

const cargo = run("cargo", [
  "build",
  "--profile",
  PROFILE,
  "--target",
  TARGET,
  "-p",
  "seaquel-wasm",
]);
if (cargo.status !== 0) fail("cargo build failed.");

const rawPath = join(targetDir, TARGET, PROFILE, `${NAME}.wasm`);
const raw = readFileSync(rawPath);
checkStack(raw);
const stamp = createHash("sha256")
  .update(raw)
  .update(wantBindgen)
  .update(binaryenVersion())
  .update(readFileSync(fileURLToPath(import.meta.url)))
  .digest("hex");

const upToDate =
  existsSync(STAMP) &&
  readFileSync(STAMP, "utf8").trim() === stamp &&
  PKG_FILES.every((f) => existsSync(join(pkgDir, f)));

if (upToDate) {
  console.log("build-wasm: src/lib/wasm/pkg/ is up to date");
} else {
  process.on("exit", () => rmSync(tmpDir, { recursive: true, force: true }));
  mkdirSync(tmpDir, { recursive: true });
  const bg = run(bindgen, ["--target", "web", "--out-dir", tmpDir, rawPath]);
  if (bg.status !== 0) fail("wasm-bindgen failed.");
  patchGlue(tmpDir);
  wasmOpt(join(tmpDir, `${NAME}_bg.wasm`));
  checkStack(readFileSync(join(tmpDir, `${NAME}_bg.wasm`)));
  writeFileSync(join(tmpDir, ".stamp"), `${stamp}\n`);
  retrying(() => rmSync(pkgDir, { recursive: true, force: true }));
  retrying(() => renameSync(tmpDir, pkgDir));
}

printSizes(raw.length);
