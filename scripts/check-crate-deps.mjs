#!/usr/bin/env node
/**
 * Enforces the crate dependency rules from
 * docs/plans/2026-09-24-rust-core-plugin-architecture-design.md
 * ("Dependency rules"). Run from the repo root:
 *
 *   node scripts/check-crate-deps.mjs
 *
 * Only normal and build dependencies count; dev-dependencies (tests) are free.
 * Every workspace crate must be classified below, so a new crate fails this
 * check until someone decides which rules apply to it.
 */

import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

/** Must build for wasm32. May only depend on each other. */
const PURE = new Set([
  "seaquel-macros",
  "seaquel-runtime",
  "seaquel-types",
  "seaquel-engine",
  "seaquel-sql",
]);

/**
 * wasm-bindgen glue that exposes pure crates to the Svelte app as one
 * WebAssembly module. It may depend on pure crates only.
 */
const WASM_GLUE = new Set(["seaquel-wasm"]);

/** Registers the default plugins, so it's the one crate allowed to name engines. */
const CORE = new Set(["seaquel-core"]);

/** Thin shells over Core. `seaquel` is the Tauri app in src-tauri/. */
const INTERFACES = new Set(["seaquel", "seaquel-server"]);

/**
 * Code shared by the interfaces (the wire types and dispatcher behind the
 * Tauri command and the server route). Like an interface it goes through
 * Core; unlike one it may also use the `Dialect` trait from seaquel-engine.
 */
const INTERFACE_GLUE = new Set(["seaquel-rpc"]);

/** Engine-agnostic test support. */
const TESTKIT = new Set(["seaquel-engine-testkit"]);

/**
 * Domain and infrastructure crates (seaquel-storage, seaquel-workspace, …)
 * arrive in later phases. List them here as they're created.
 */
const DOMAIN_AND_INFRA = new Set([]);

const ENGINE_MAY_USE = new Set([
  "seaquel-engine",
  "seaquel-runtime",
  "seaquel-types",
  "seaquel-sql",
]);
const INTERFACE_MAY_USE = new Set([
  "seaquel-core",
  "seaquel-runtime",
  "seaquel-types",
  "seaquel-rpc",
]);

const INTERFACE_GLUE_MAY_USE = new Set([
  "seaquel-core",
  "seaquel-engine",
  "seaquel-runtime",
  "seaquel-types",
]);

const isEngine = (name) => name.startsWith("seaquel-engine-") && !TESTKIT.has(name);

function classify(name) {
  if (PURE.has(name)) return "pure";
  if (WASM_GLUE.has(name)) return "wasm-glue";
  if (CORE.has(name)) return "core";
  if (INTERFACES.has(name)) return "interface";
  if (INTERFACE_GLUE.has(name)) return "interface-glue";
  if (TESTKIT.has(name)) return "testkit";
  if (isEngine(name)) return "engine";
  if (DOMAIN_AND_INFRA.has(name)) return "domain";
  return null;
}

/**
 * @param {{ name: string, dependencies: { name: string, kind: string | null }[] }[]} packages
 *   `packages` from `cargo metadata --no-deps`.
 * @returns {string[]} One message per violation. Empty means OK.
 */
export function checkCrateDeps(packages) {
  const workspace = new Set(packages.map((p) => p.name));
  const errors = [];

  for (const pkg of packages) {
    const kind = classify(pkg.name);
    if (!kind) {
      errors.push(`${pkg.name}: unclassified crate. Add it to scripts/check-crate-deps.mjs.`);
      continue;
    }
    const deps = pkg.dependencies
      .filter((d) => d.kind !== "dev" && workspace.has(d.name))
      .map((d) => d.name);
    const forbid = (allowed, why) => {
      for (const dep of deps) {
        if (!allowed(dep)) errors.push(`${pkg.name} -> ${dep}: ${why}`);
      }
    };

    switch (kind) {
      case "pure":
        forbid((d) => PURE.has(d), "pure crates may only depend on other pure crates");
        break;
      case "wasm-glue":
        forbid((d) => PURE.has(d), "wasm glue may only depend on pure crates");
        break;
      case "engine":
        forbid(
          (d) => ENGINE_MAY_USE.has(d),
          "engine crates may only depend on seaquel-engine, seaquel-runtime, seaquel-types and seaquel-sql",
        );
        break;
      case "interface":
        forbid((d) => INTERFACE_MAY_USE.has(d), "interfaces reach everything through seaquel-core");
        break;
      case "interface-glue":
        forbid(
          (d) => INTERFACE_GLUE_MAY_USE.has(d),
          "interface glue may only depend on seaquel-core, seaquel-engine, seaquel-runtime and seaquel-types",
        );
        break;
      case "core":
        break;
      case "testkit":
      case "domain":
        forbid((d) => !isEngine(d), "reach engines through EngineRegistry, never by crate name");
        break;
    }
  }
  return errors;
}

const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) {
  const metadata = execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const { packages } = JSON.parse(metadata);
  const errors = checkCrateDeps(packages);
  if (errors.length > 0) {
    console.error(errors.join("\n"));
    process.exit(1);
  }
  console.log(`crate dependency rules: ${packages.length} crates OK`);
}
