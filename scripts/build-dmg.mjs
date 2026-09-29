#!/usr/bin/env node
// Local macOS DMG build. On some Macs, hdiutil rejects Tauri's default image
// when its volume and .app have the same base name. A distinct volume label
// avoids that failure while keeping the installed app named Seaquel.app.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "darwin") {
  console.error("build-dmg: macOS is required to build a DMG");
  process.exit(1);
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const config = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
const appName = `${config.productName}.app`;

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} exited with status ${result.status}`);
}

run(process.execPath, [
  join(root, "scripts", "tauri.mjs"),
  "build",
  "--bundles",
  "app",
  "--config",
  '{"bundle":{"createUpdaterArtifacts":false}}',
]);

const metadata = spawnSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
  cwd: root,
  encoding: "utf8",
});
if (metadata.error || metadata.status !== 0) {
  console.error(`build-dmg: cargo metadata failed: ${metadata.stderr ?? metadata.error}`);
  process.exit(1);
}

const targetDir = JSON.parse(metadata.stdout).target_directory;
const app = join(targetDir, "release", "bundle", "macos", appName);
if (!existsSync(app)) {
  console.error(`build-dmg: missing app bundle: ${app}`);
  process.exit(1);
}

const arch = process.arch === "arm64" ? "aarch64" : process.arch === "x64" ? "x64" : process.arch;
const dmgDir = join(targetDir, "release", "bundle", "dmg");
const dmg = join(dmgDir, `${config.productName}_${config.version}_${arch}.dmg`);
const stage = mkdtempSync(join(tmpdir(), "seaquel-dmg-"));
try {
  run("ditto", [app, join(stage, appName)]);
  symlinkSync("/Applications", join(stage, "Applications"));
  mkdirSync(dmgDir, { recursive: true });
  run("hdiutil", [
    "create",
    "-srcfolder",
    stage,
    "-volname",
    `${config.productName} Installer`,
    "-fs",
    "HFS+",
    "-format",
    "UDZO",
    "-imagekey",
    "zlib-level=9",
    "-ov",
    dmg,
  ]);
  console.log(`build-dmg: ${dmg}`);
} finally {
  rmSync(stage, { recursive: true, force: true });
}
