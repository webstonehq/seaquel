#!/usr/bin/env node
/**
 * Run a command with a fresh SEAQUEL_INTERNAL_SECRET in its environment, for
 * development setups that start the Rust service and the SvelteKit server as
 * separate processes (`npm run dev:web:full`). In production `server.js`
 * generates the secret and hands it to both.
 *
 *   node scripts/with-internal-secret.mjs <command> [args...]
 */

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error("usage: with-internal-secret.mjs <command> [args...]");
  process.exit(2);
}

const child = spawn(command, args, {
  stdio: "inherit",
  shell: process.platform === "win32",
  env: { ...process.env, SEAQUEL_INTERNAL_SECRET: randomBytes(32).toString("hex") },
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => child.kill(sig));
}
