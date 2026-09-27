#!/usr/bin/env node
// Builds the `seaquel-cli` binary (crates/seaquel-cli) and copies it to
// src-tauri/binaries/seaquel-cli-<target-triple>[.exe], where Tauri's
// `bundle.externalBin` picks it up as a sidecar. tauri-build refuses to build
// the desktop app while that file is missing, so `cargo check -p seaquel` needs
// it too (src-tauri/build.rs says so and points here).
//
//   node scripts/build-cli.mjs                     debug build for the host
//   node scripts/build-cli.mjs --release           release build
//   node scripts/build-cli.mjs --target <triple>   cross build (cargo --target)
//   SEAQUEL_CLI_PREBUILT=1 node scripts/...        check the file exists and stop
//                                                  (release.yml builds it in its own step)
//
// Run by `beforeDevCommand` and `beforeBuildCommand` in tauri.conf.json. There,
// Tauri's TAURI_ENV_TARGET_TRIPLE picks the triple and TAURI_ENV_DEBUG the
// profile (release unless `tauri build --debug` or `tauri dev`). An explicit
// flag wins over both.
//
// Without an explicit --target, a host build runs without `--target`, so it
// shares target/debug (or target/release) with the app's own cargo build
// instead of compiling every dependency a second time. Respects
// CARGO_TARGET_DIR.
//
// Node, not bash, because the release builds on Windows too.
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PACKAGE = "seaquel-cli";
const BIN = "seaquel-cli";
const binariesDir = join(root, "src-tauri", "binaries");

function fail(message) {
  console.error(`\nbuild-cli: ${message}\n`);
  process.exit(1);
}

function usage() {
  console.log(
    "usage: node scripts/build-cli.mjs [--release] [--target <triple>]\n" +
      "Builds seaquel-cli and copies it to src-tauri/binaries/seaquel-cli-<triple>[.exe].",
  );
}

// --- arguments -----------------------------------------------------------------

let release = false;
let explicitTarget = null;
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const arg = argv[i];
  if (arg === "--release") release = true;
  else if (arg === "--target") {
    explicitTarget = argv[++i];
    if (!explicitTarget)
      fail("--target needs a target triple, e.g. --target aarch64-apple-darwin.");
  } else if (arg.startsWith("--target=")) explicitTarget = arg.slice("--target=".length);
  else if (arg === "--help" || arg === "-h") {
    usage();
    process.exit(0);
  } else fail(`unknown argument \`${arg}\`. Run with --help.`);
}

// Under `tauri build`/`tauri dev` (beforeBuildCommand/beforeDevCommand).
const tauriTriple = process.env.TAURI_ENV_TARGET_TRIPLE || null;
if (!release && tauriTriple && process.env.TAURI_ENV_DEBUG !== "true") release = true;

function run(cmd, args, { capture = false } = {}) {
  return spawnSync(cmd, args, {
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

// rustc from the repo root, so rust-toolchain.toml picks the toolchain.
function hostTriple() {
  const r = run("rustc", ["-vV"], { capture: true });
  if (r.error || r.status !== 0) {
    fail("rustc not found. Install Rust from https://rustup.rs.");
  }
  const m = r.stdout.match(/^host: (\S+)$/m);
  if (!m) fail(`couldn't read the host triple from \`rustc -vV\`:\n${r.stdout}`);
  return m[1];
}

const host = hostTriple();
const triple = explicitTarget || tauriTriple || host;
if (triple.startsWith("universal-")) {
  fail(
    `${triple} isn't supported: build seaquel-cli for aarch64-apple-darwin and x86_64-apple-darwin ` +
      "and join them with `lipo -create` into src-tauri/binaries/seaquel-cli-universal-apple-darwin.",
  );
}
const exe = triple.includes("windows") ? ".exe" : "";
const dest = join(binariesDir, `${BIN}-${triple}${exe}`);
const rel = (p) => p.slice(root.length + 1);

if (process.env.SEAQUEL_CLI_PREBUILT === "1") {
  if (!existsSync(dest)) {
    fail(
      `SEAQUEL_CLI_PREBUILT=1 but ${rel(dest)} is missing. ` +
        `Run \`node scripts/build-cli.mjs${release ? " --release" : ""} --target ${triple}\` first.`,
    );
  }
  console.log(`build-cli: SEAQUEL_CLI_PREBUILT=1, using the existing ${rel(dest)}`);
  process.exit(0);
}

// --- build ---------------------------------------------------------------------

const meta = run("cargo", ["metadata", "--format-version", "1", "--no-deps"], { capture: true });
if (meta.error) fail("cargo not found. Install Rust from https://rustup.rs.");
if (meta.status !== 0) fail(`cargo metadata failed:\n${meta.stderr}`);
const targetDir = JSON.parse(meta.stdout).target_directory;

// `--target` only when asked for, or when Tauri targets another triple than the
// host (it passes `--target` to its own cargo build then too).
const passTarget = explicitTarget !== null || triple !== host;
const profile = release ? "release" : "debug";
const cargoArgs = ["build", "-p", PACKAGE, "--bin", BIN];
if (release) cargoArgs.push("--release");
if (passTarget) cargoArgs.push("--target", triple);

// `tauri build` exports MACOSX_DEPLOYMENT_TARGET (bundle.macOS.minimumSystemVersion,
// 10.13 by default) for the app; `tauri dev` doesn't. Changing it recompiles the
// C/C++ crates the CLI shares with the app (duckdb, sqlite, ring), so a release
// build run on its own (release.yml's sidecar step) uses the same value.
if (release && triple.includes("apple-darwin") && !process.env.MACOSX_DEPLOYMENT_TARGET) {
  const conf = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
  process.env.MACOSX_DEPLOYMENT_TARGET = conf.bundle?.macOS?.minimumSystemVersion ?? "10.13";
}

console.log(`build-cli: cargo ${cargoArgs.join(" ")}`);
const cargo = run("cargo", cargoArgs);
if (cargo.error) fail(`couldn't run cargo: ${cargo.error.message}`);
if (cargo.status !== 0) {
  fail(
    `cargo build of ${PACKAGE} failed (exit ${cargo.status ?? cargo.signal}); see the errors above.` +
      (passTarget && triple !== host
        ? `\nFor a cross build, the Rust target must be installed: rustup target add ${triple}`
        : ""),
  );
}

const built = join(targetDir, ...(passTarget ? [triple] : []), profile, `${BIN}${exe}`);
if (!existsSync(built)) fail(`cargo finished but ${built} doesn't exist.`);

// Copy to a temp name and rename, so a build that dies halfway never leaves a
// truncated sidecar for tauri-build to bundle.
mkdirSync(binariesDir, { recursive: true });
const tmp = `${dest}.tmp-${process.pid}`;
try {
  copyFileSync(built, tmp);
  if (!exe) chmodSync(tmp, 0o755);
  rmSync(dest, { force: true });
  renameSync(tmp, dest);
} catch (e) {
  rmSync(tmp, { force: true });
  fail(`couldn't copy ${built} to ${rel(dest)}: ${e.message}`);
}
console.log(`build-cli: ${rel(dest)} (${profile})`);
