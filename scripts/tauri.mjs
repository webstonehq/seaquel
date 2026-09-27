#!/usr/bin/env node
// `npm run tauri …`: runs the Tauri CLI, but for `tauri dev` builds the
// seaquel-cli sidecar first. `tauri dev` waits at most ~180 s for Vite, and
// beforeDevCommand only starts Vite after the sidecar is built, so on a clean
// clone the first run would time out. Building here, before Tauri starts,
// removes that wait; SEAQUEL_CLI_PREBUILT=1 then makes beforeDevCommand's
// build-cli.mjs only check the file.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
const env = { ...process.env };

if (args[0] === "dev") {
  // `tauri dev`'s own --release and --target/-t (not the ones after `--`).
  const own = args.includes("--") ? args.slice(0, args.indexOf("--")) : args;
  const cliArgs = [];
  if (own.includes("--release")) cliArgs.push("--release");
  own.forEach((a, i) => {
    if ((a === "--target" || a === "-t") && own[i + 1]) cliArgs.push("--target", own[i + 1]);
    else if (a.startsWith("--target=")) cliArgs.push(a);
  });
  const script = join(dirname(fileURLToPath(import.meta.url)), "build-cli.mjs");
  const r = spawnSync(process.execPath, [script, ...cliArgs], { stdio: "inherit" });
  if (r.status !== 0) process.exit(r.status ?? 1);
  env.SEAQUEL_CLI_PREBUILT = "1";
}

const tauri = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
const r = spawnSync(process.execPath, [tauri, ...args], { stdio: "inherit", env });
process.exit(r.status ?? 1);
