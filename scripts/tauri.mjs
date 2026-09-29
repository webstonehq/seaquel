#!/usr/bin/env node
// `npm run tauri …`: runs the Tauri CLI.
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";

const args = process.argv.slice(2);

const tauri = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
const r = spawnSync(process.execPath, [tauri, ...args], { stdio: "inherit" });
process.exit(r.status ?? 1);
