#!/usr/bin/env node
// `release.yml`'s `check-release` job (the desktop DuckDB helper plan, Q11 B,
// Decision 15, Task 9): reads the draft release after every other job and
// fails, naming each problem, unless every target ships a complete set:
//
// - the app, built and pinned to this target's DuckDB helper (the matrix
//   job's record from scripts/release-pin.mjs, with `app.pinned`);
// - the helper's `.gz` in the draft, byte for byte the file the app was
//   pinned to (size and SHA-256 of the downloaded asset);
// - the CLI and the TUI;
// - an entry in latest.json (and latest.json names no other platform, points
//   only at assets the draft has, and carries the tag's version);
// - every job before this one succeeded;
// - the release is still a pre-release (`release.yml` makes it one).
//
// The title is the gate (review I1): `tauri-action` creates the draft as
// "NOT CHECKED: Release <tag>", and the job sets "Release <tag>" only when
// this passes ("NOT READY (check-release failed): …" when it doesn't). The
// release is a pre-release (I3), which the website's update check skips;
// promoting it to the latest release, after the manual checks, is the
// deliberate step that reaches users.
//
//   node scripts/check-release.mjs --tag v2026.10.0 --dir release-check \
//     --job publish-tauri=success --job publish-cli=success ...
//
// `--dir` holds `pins/*.json` (the matrix jobs' records), `release.json`
// (`gh release view <tag> --json assets,isDraft,isPrerelease`) and `download/` (the draft's
// `seaquel-duckdb-*.gz` and `latest.json`). A missing piece is a problem,
// never a pass.
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, realpathSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { assetName } from "./build-cli.mjs";
import { helperAsset, recordName } from "./release-pin.mjs";

/** The targets `release.yml`'s matrix builds (a test keeps the two equal). */
export const RELEASE_TARGETS = [
  "aarch64-apple-darwin",
  "x86_64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "aarch64-unknown-linux-gnu",
  "x86_64-pc-windows-msvc",
  "aarch64-pc-windows-msvc",
];

const OS_OF = { darwin: "apple-darwin", linux: "unknown-linux-gnu", windows: "pc-windows-msvc" };

/**
 * The target a latest.json platform key stands for: `<os>-<arch>` with an
 * optional installer suffix (`darwin-aarch64`, `linux-x86_64-appimage`,
 * `windows-x86_64-nsis`, …), or `null` for a key that isn't one.
 */
export function targetOfPlatform(key) {
  const [os, arch] = key.split("-");
  if (!OS_OF[os] || !["x86_64", "aarch64"].includes(arch)) return null;
  return `${arch}-${OS_OF[os]}`;
}

/** The last path segment of an updater URL, decoded. */
function urlAsset(url) {
  try {
    return decodeURIComponent(new URL(url).pathname.split("/").pop() ?? "");
  } catch {
    return null;
  }
}

/**
 * Every problem with the draft, as sentences; empty when it is complete.
 * @param {{
 *   tag: string,
 *   targets?: string[],
 *   jobs: Record<string, string>,
 *   records: Record<string, any>,
 *   assets: {name: string, size: number}[] | null,
 *   release?: {isDraft: boolean, isPrerelease: boolean} | null,
 *   files: Record<string, {size: number, sha256: string}>,
 *   latest: any,
 * }} input
 */
export function checkRelease({
  tag,
  targets = RELEASE_TARGETS,
  jobs,
  records,
  assets,
  release = null,
  files,
  latest,
}) {
  const problems = [];
  for (const [job, result] of Object.entries(jobs)) {
    if (result !== "success") problems.push(`job ${job} ended "${result}", not "success"`);
  }
  if (assets === null) {
    problems.push(`no draft release ${tag} could be read`);
    assets = [];
  }
  const byName = new Map(assets.map((a) => [a.name, a]));
  if (release && release.isPrerelease !== true) {
    problems.push(
      `${tag} isn't a pre-release: promoting it is the step after the manual checks, not before this one`,
    );
  }

  for (const target of targets) {
    const record = records[target];
    const helper = helperAsset(target);
    if (!record) {
      problems.push(`${target}: no record of its release job (it failed or didn't finish)`);
    } else if (record.target !== target || record.asset !== helper) {
      problems.push(`${target}: its record names ${record.target} / ${record.asset}`);
    } else {
      if (record.app?.pinned !== true) {
        problems.push(
          `${target}: the app isn't pinned to its helper (${record.app?.reason ?? "not checked: the app's build didn't finish"})`,
        );
      }
      const asset = byName.get(helper);
      const file = files[helper];
      if (!asset) {
        problems.push(`${target}: ${helper} is missing from the draft`);
      } else if (asset.size !== record.size) {
        problems.push(
          `${target}: ${helper} is ${asset.size} bytes in the draft, the app was pinned to ${record.size}`,
        );
      } else if (!file) {
        problems.push(`${target}: ${helper} couldn't be downloaded from the draft`);
      } else if (file.size !== record.size || file.sha256 !== record.sha256) {
        problems.push(
          `${target}: ${helper} in the draft has sha256 ${file.sha256}, the app was pinned to ${record.sha256}`,
        );
      }
    }
    for (const bin of ["seaquel-cli", "seaquel-tui"]) {
      const name = assetName(bin, target);
      if (!byName.has(name)) problems.push(`${target}: ${name} is missing from the draft`);
    }
  }

  for (const name of byName.keys()) {
    const m = /^seaquel-duckdb-(.+?)(\.exe)?\.gz$/.exec(name);
    if (m && !targets.includes(m[1]))
      problems.push(`${name} is in the draft but no release job builds ${m[1]}`);
  }

  if (
    !latest ||
    typeof latest !== "object" ||
    typeof latest.platforms !== "object" ||
    !latest.platforms
  ) {
    problems.push("latest.json is missing from the draft or isn't the updater's manifest");
  } else {
    const version = tag.replace(/^v/, "");
    if (latest.version !== version)
      problems.push(`latest.json's version is "${latest.version}", not "${version}"`);
    const covered = new Set();
    for (const [key, entry] of Object.entries(latest.platforms)) {
      const target = targetOfPlatform(key);
      if (!target || !targets.includes(target)) {
        problems.push(`latest.json names platform ${key}, which no release job builds`);
        continue;
      }
      covered.add(target);
      const name = urlAsset(entry?.url);
      if (!name || !byName.has(name))
        problems.push(
          `latest.json's ${key} points at ${name ?? entry?.url}, which isn't in the draft`,
        );
      if (typeof entry?.signature !== "string" || entry.signature === "")
        problems.push(`latest.json's ${key} has no signature`);
    }
    for (const target of targets) {
      if (!covered.has(target))
        problems.push(`${target}: latest.json has no entry for it, so its app doesn't update`);
    }
  }
  return problems;
}

/** Reads `--dir`'s pieces; anything missing is left for checkRelease to report. */
export function readReleaseDir(dir, targets = RELEASE_TARGETS) {
  const records = {};
  for (const target of targets) {
    const path = join(dir, "pins", recordName(target));
    if (existsSync(path)) {
      try {
        records[target] = JSON.parse(readFileSync(path, "utf8"));
      } catch {
        records[target] = { target: "(unreadable record)" };
      }
    }
  }
  let assets = null;
  let release = null;
  const releasePath = join(dir, "release.json");
  if (existsSync(releasePath)) {
    try {
      const parsed = JSON.parse(readFileSync(releasePath, "utf8"));
      if (Array.isArray(parsed?.assets)) {
        assets = parsed.assets.map((a) => ({ name: a.name, size: a.size }));
        release = { isDraft: parsed.isDraft, isPrerelease: parsed.isPrerelease };
      }
    } catch {
      assets = null;
    }
  }
  const files = {};
  const download = join(dir, "download");
  if (existsSync(download)) {
    for (const name of readdirSync(download)) {
      if (!/^seaquel-duckdb-.*\.gz$/.test(name)) continue;
      const bytes = readFileSync(join(download, name));
      files[name] = {
        size: bytes.length,
        sha256: createHash("sha256").update(bytes).digest("hex"),
      };
    }
  }
  let latest = null;
  const latestPath = join(download, "latest.json");
  if (existsSync(latestPath)) {
    try {
      latest = JSON.parse(readFileSync(latestPath, "utf8"));
    } catch {
      latest = null;
    }
  }
  return { records, assets, release, files, latest };
}

function parseArgs(argv) {
  const out = { tag: null, dir: null, jobs: {} };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--tag") out.tag = argv[++i];
    else if (arg === "--dir") out.dir = argv[++i];
    else if (arg === "--job") {
      const [name, result] = (argv[++i] ?? "").split("=");
      if (!name) throw new Error("--job takes <name>=<result>");
      out.jobs[name] = result ?? "";
    } else throw new Error(`unknown argument ${arg}`);
  }
  if (!out.tag || !out.dir)
    throw new Error("usage: check-release.mjs --tag <tag> --dir <dir> [--job name=result]…");
  return out;
}

function main() {
  let args;
  try {
    args = parseArgs(process.argv.slice(2));
  } catch (e) {
    console.error(`check-release: ${e.message}`);
    process.exit(2);
  }
  const problems = checkRelease({
    tag: args.tag,
    jobs: args.jobs,
    ...readReleaseDir(resolve(args.dir)),
  });
  if (problems.length === 0) {
    console.log(
      `check-release: ${args.tag} is complete: every target has its app, pinned helper, CLI, TUI and updater entry.`,
    );
    return;
  }
  for (const p of problems) console.log(`::error::${p}`);
  console.error(
    `check-release: ${args.tag} is NOT ready to publish (${problems.length} problem${problems.length === 1 ? "" : "s"}).`,
  );
  process.exit(1);
}

const invokedDirectly =
  process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
if (invokedDirectly) main();
