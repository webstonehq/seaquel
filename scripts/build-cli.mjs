#!/usr/bin/env node
// Builds a standalone terminal binary's release asset, `seaquel-cli`
// (crates/seaquel-cli) or `seaquel-tui` (crates/seaquel-tui), and copies it to
// src-tauri/binaries/<bin>-<target-triple>[.exe]. The release workflow uploads
// each separately; the desktop app downloads the CLI on request.
//
//   node scripts/build-cli.mjs                     debug build of seaquel-cli for the host
//   node scripts/build-cli.mjs --bin seaquel-tui   the TUI instead (`npm run tui:build`)
//   node scripts/build-cli.mjs --release           release build (the `terminal-release` profile)
//   node scripts/build-cli.mjs --target <triple>   cross build (cargo --target)
// Without an explicit --target, a host build runs without `--target`, so it
// shares target/debug with other host builds. A release build uses the
// `terminal-release` profile from the root Cargo.toml (fat LTO, one codegen
// unit, stripped), so it lands in target/[<triple>/]terminal-release/.
// Respects CARGO_TARGET_DIR.
//
// Node, not bash, because the release builds on Windows too.
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  closeSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binariesDir = join(root, "src-tauri", "binaries");

/** The binaries this script builds, each with its Cargo package. */
export const BINS = {
  "seaquel-cli": "seaquel-cli",
  "seaquel-tui": "seaquel-tui",
};
const BIN_LIST = Object.keys(BINS).join(", ");

/** The Cargo profile `--release` builds with (root Cargo.toml). */
export const RELEASE_PROFILE = "terminal-release";

/**
 * The `cargo build` arguments for a binary.
 * @param {{ bin: string, release: boolean, target: string | null }} args
 *   `target` only when one was asked for, so host builds share the host cache.
 */
export function cargoBuildArgs({ bin, release, target }) {
  const args = ["build", "-p", BINS[bin], "--bin", bin];
  if (release) args.push("--profile", RELEASE_PROFILE);
  if (target) args.push("--target", target);
  return args;
}

/**
 * Where Cargo puts the binary: `<targetDir>/[<target>/]<profile dir>/<bin>[.exe]`.
 * The dev profile's directory is `debug`; a named profile's is its name.
 * @param {string} targetDir
 * @param {{ bin: string, release: boolean, target: string | null, triple: string }} args
 */
export function builtPath(targetDir, { bin, release, target, triple }) {
  const exe = triple.includes("windows") ? ".exe" : "";
  return join(
    targetDir,
    ...(target ? [target] : []),
    release ? RELEASE_PROFILE : "debug",
    `${bin}${exe}`,
  );
}

/** The release asset's file name: `<bin>-<triple>`, `.exe` on Windows. */
export function assetName(bin, triple) {
  return `${bin}-${triple}${triple.includes("windows") ? ".exe" : ""}`;
}

/**
 * The command line, or an `Error` whose message says what's wrong.
 * @param {string[]} argv
 */
export function parseArgs(argv) {
  const args = {
    release: false,
    target: null,
    bin: "seaquel-cli",
    help: false,
  };
  const takeBin = (bin) => {
    if (!bin) throw new Error(`--bin needs a binary: ${BIN_LIST}.`);
    if (!Object.hasOwn(BINS, bin))
      throw new Error(`unknown --bin \`${bin}\`. Use one of: ${BIN_LIST}.`);
    args.bin = bin;
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--release") args.release = true;
    else if (arg === "--target") {
      args.target = argv[++i];
      if (!args.target)
        throw new Error("--target needs a target triple, e.g. --target aarch64-apple-darwin.");
    } else if (arg.startsWith("--target=")) args.target = arg.slice("--target=".length);
    else if (arg === "--bin") takeBin(argv[++i]);
    else if (arg.startsWith("--bin=")) takeBin(arg.slice("--bin=".length));
    else if (arg === "--help" || arg === "-h") args.help = true;
    else throw new Error(`unknown argument \`${arg}\`. Run with --help.`);
  }
  return args;
}

function fail(message) {
  console.error(`\nbuild-cli: ${message}\n`);
  process.exit(1);
}

function usage() {
  console.log(
    `usage: node scripts/build-cli.mjs [--bin ${Object.keys(BINS).join("|")}] [--release] [--target <triple>]\n` +
      "Builds the binary (seaquel-cli by default) and copies it to src-tauri/binaries/<bin>-<triple>[.exe].",
  );
}

// Compared after resolving symlinks: npm, a linked checkout or a bin dir can
// start the script through one, and then argv[1] isn't this file's path.
const invokedDirectly =
  process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
if (invokedDirectly) main();

function main() {
  // --- arguments -----------------------------------------------------------------

  let parsed;
  try {
    parsed = parseArgs(process.argv.slice(2));
  } catch (e) {
    fail(e.message);
  }
  if (parsed.help) {
    usage();
    process.exit(0);
  }
  const { release, bin: BIN } = parsed;
  const PACKAGE = BINS[BIN];
  const explicitTarget = parsed.target;

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
  const triple = explicitTarget || host;
  if (triple.startsWith("universal-")) {
    fail(
      `${triple} isn't supported: build ${BIN} for aarch64-apple-darwin and x86_64-apple-darwin ` +
        `and join them with \`lipo -create\` into src-tauri/binaries/${BIN}-universal-apple-darwin.`,
    );
  }
  const exe = triple.includes("windows") ? ".exe" : "";
  const dest = join(binariesDir, assetName(BIN, triple));
  const rel = (p) => p.slice(root.length + 1);

  // --- build ---------------------------------------------------------------------

  const meta = run("cargo", ["metadata", "--format-version", "1", "--no-deps"], { capture: true });
  if (meta.error) fail("cargo not found. Install Rust from https://rustup.rs.");
  if (meta.status !== 0) fail(`cargo metadata failed:\n${meta.stderr}`);
  const targetDir = JSON.parse(meta.stdout).target_directory;

  // `--target` only when asked for, so local builds share the host target cache.
  const passTarget = explicitTarget !== null;
  const profile = release ? RELEASE_PROFILE : "debug";
  const target = passTarget ? triple : null;
  const cargoArgs = cargoBuildArgs({ bin: BIN, release, target });

  // `tauri build` exports MACOSX_DEPLOYMENT_TARGET (bundle.macOS.minimumSystemVersion,
  // 10.13 by default) for the app; `tauri dev` doesn't. Changing it recompiles the
  // C/C++ crates the terminal binaries share with the app (duckdb, sqlite, ring), so a release
  // standalone build in release.yml uses the same value.
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

  const built = builtPath(targetDir, { bin: BIN, release, target, triple });
  if (!existsSync(built)) fail(`cargo finished but ${built} doesn't exist.`);

  // Keep the release asset's timestamp stable when Cargo produced identical bytes.
  function sameFileContents(a, b) {
    if (!existsSync(b) || statSync(a).size !== statSync(b).size) return false;
    const left = openSync(a, "r");
    const right = openSync(b, "r");
    const leftChunk = Buffer.allocUnsafe(1024 * 1024);
    const rightChunk = Buffer.allocUnsafe(leftChunk.length);
    try {
      while (true) {
        const count = readSync(left, leftChunk, 0, leftChunk.length, null);
        if (count === 0) return true;
        if (readSync(right, rightChunk, 0, count, null) !== count) return false;
        if (!leftChunk.subarray(0, count).equals(rightChunk.subarray(0, count))) return false;
      }
    } finally {
      closeSync(left);
      closeSync(right);
    }
  }

  if (sameFileContents(built, dest)) {
    console.log(`build-cli: ${rel(dest)} is up to date (${profile})`);
    return;
  }

  // Copy to a temp name and rename, so a failed copy never leaves a truncated
  // release asset.
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
}
