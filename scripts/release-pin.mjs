#!/usr/bin/env node
// The DuckDB helper's pin in a release job (the desktop DuckDB helper plan,
// Q1 B, Decision 4, Task 9). Each `release.yml` matrix job builds, signs and
// gzips its target's helper first, then:
//
//   node scripts/release-pin.mjs export --target <triple>
//       hashes src-tauri/binaries/seaquel-duckdb-<triple>[.exe].gz, appends
//       SEAQUEL_DUCKDB_HELPER_SIZE (plain decimal) and
//       SEAQUEL_DUCKDB_HELPER_SHA256 (64 lowercase hex, no prefix) to
//       $GITHUB_ENV for the app's build, and writes the record
//       release-pins/duckdb-pin-<triple>.json that `check-release` reads.
//   node scripts/release-pin.mjs verify-app --target <triple>
//       after the app's build: the built `seaquel` binary must hold the
//       pin's compiled text (`<size>:<sha256>`, src-tauri/build.rs), so the
//       app that ships downloads exactly this .gz. On macOS the bundle's
//       copy (`bundle/macos/<productName>.app/Contents/MacOS/*`, what the
//       updater's .app.tar.gz carries) must hold it too. Records the answer.
//
// The upload (scripts/upload-helper-asset.mjs) re-hashes the .gz against the
// record and uploads those same bytes.
//
// Node, not bash, because the release builds on Windows too.
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipName } from "./build-cli.mjs";

// The repository; tests point it at a scratch folder.
const root = process.env.RELEASE_PIN_ROOT
  ? resolve(process.env.RELEASE_PIN_ROOT)
  : resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Where the release job keeps its records (uploaded as workflow artifacts). */
export const RECORD_DIR = "release-pins";

/** The helper's release asset for a target: `seaquel-duckdb-<triple>[.exe].gz`. */
export function helperAsset(target) {
  return gzipName("seaquel-duckdb", target);
}

/** The record's file name for a target. */
export function recordName(target) {
  return `duckdb-pin-${target}.json`;
}

/** The file's size and SHA-256 (lowercase hex), from its bytes. */
export function pinOf(bytes) {
  return { size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
}

/** The lines `export` appends to `$GITHUB_ENV`. */
export function envLines(pin) {
  return `SEAQUEL_DUCKDB_HELPER_SIZE=${pin.size}\nSEAQUEL_DUCKDB_HELPER_SHA256=${pin.sha256}\n`;
}

/** What src-tauri/build.rs compiles into the app for this pin (`pin_text`). */
export function compiledText(pin) {
  return `${pin.size}:${pin.sha256}`;
}

/** Whether a built binary's bytes hold the pin's compiled text. */
export function carriesPin(binary, pin) {
  return binary.includes(Buffer.from(compiledText(pin), "latin1"));
}

/**
 * Where the app's build leaves its binary for a target: the workspace's
 * target folder (src-tauri is a member), or `CARGO_TARGET_DIR`.
 */
export function appBinaryCandidates(target, { repo = root, cargoTargetDir } = {}) {
  const exe = target.includes("windows") ? ".exe" : "";
  const dirs = [cargoTargetDir, join(repo, "target"), join(repo, "src-tauri", "target")].filter(
    Boolean,
  );
  return [...new Set(dirs.map((d) => join(resolve(repo, d), target, "release", `seaquel${exe}`)))];
}

/** The app's product name, from src-tauri/tauri.conf.json. */
export function productName(repo = root) {
  const name = JSON.parse(
    readFileSync(join(repo, "src-tauri", "tauri.conf.json"), "utf8"),
  ).productName;
  if (typeof name !== "string" || name === "")
    throw new Error("tauri.conf.json has no productName");
  return name;
}

/**
 * The files that must hold the pin: the built binary, and on macOS every
 * file in the bundle's `Contents/MacOS` (the copy the updater ships). An
 * empty or missing bundle folder is an error, not nothing to check.
 */
export function pinnedFiles(target, binary, { repo = root } = {}) {
  if (!target.includes("apple-darwin")) return [binary];
  const dir = join(
    dirname(binary),
    "bundle",
    "macos",
    `${productName(repo)}.app`,
    "Contents",
    "MacOS",
  );
  const inBundle = existsSync(dir)
    ? readdirSync(dir)
        .map((n) => join(dir, n))
        .filter((p) => statSync(p).isFile())
    : [];
  if (inBundle.length === 0) throw new Error(`no app bundle binary in ${relative(repo, dir)}`);
  return [binary, ...inBundle];
}

function fail(message) {
  console.error(`release-pin: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  let target = null;
  for (let i = 0; i < rest.length; i++) {
    if (rest[i] === "--target") target = rest[++i] ?? null;
    else if (rest[i].startsWith("--target=")) target = rest[i].slice("--target=".length);
    else fail(`unknown argument ${rest[i]}`);
  }
  if (!["export", "verify-app"].includes(command)) {
    fail("usage: release-pin.mjs export|verify-app --target <triple>");
  }
  if (!target || !/^[a-z0-9_]+(-[a-z0-9_]+){2,3}$/.test(target))
    fail("--target <triple> is required");
  return { command, target };
}

function readRecord(path) {
  if (!existsSync(path)) fail(`no record at ${relative(root, path)}: run "export" first`);
  return JSON.parse(readFileSync(path, "utf8"));
}

function writeRecord(path, record) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(record, null, 2)}\n`);
}

function main() {
  const { command, target } = parseArgs(process.argv.slice(2));
  const gz = join(root, "src-tauri", "binaries", helperAsset(target));
  const recordPath = join(root, RECORD_DIR, recordName(target));

  if (command === "export") {
    if (!existsSync(gz))
      fail(`${relative(root, gz)} doesn't exist: build, sign and gzip the helper first`);
    const pin = pinOf(readFileSync(gz));
    const env = process.env.GITHUB_ENV;
    if (!env) fail("GITHUB_ENV isn't set (this runs in a release job)");
    appendFileSync(env, envLines(pin));
    writeRecord(recordPath, {
      target,
      asset: helperAsset(target),
      size: pin.size,
      sha256: pin.sha256,
    });
    console.log(`release-pin: ${helperAsset(target)} is ${pin.size} bytes, sha256 ${pin.sha256}`);
    return;
  }

  // verify-app
  const record = readRecord(recordPath);
  const unpinned = (reason) => {
    writeRecord(recordPath, { ...record, app: { pinned: false, reason } });
    fail(`${target}: ${reason}`);
  };
  const candidates = appBinaryCandidates(target, { cargoTargetDir: process.env.CARGO_TARGET_DIR });
  const binary = candidates.find((p) => existsSync(p));
  if (!binary)
    unpinned(
      `no app binary found; looked at ${candidates.map((p) => relative(root, p)).join(", ")}`,
    );
  let files;
  try {
    files = pinnedFiles(target, binary);
  } catch (e) {
    unpinned(e.message);
  }
  for (const file of files) {
    if (!carriesPin(readFileSync(file), record)) {
      unpinned(`${relative(root, file)} doesn't hold ${compiledText(record)}`);
    }
  }
  writeRecord(recordPath, { ...record, app: { pinned: true } });
  console.log(
    `release-pin: ${files.map((f) => relative(root, f)).join(", ")} pinned to ${helperAsset(target)}`,
  );
}

const invokedDirectly =
  process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
if (invokedDirectly) main();
