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

/**
 * Thin shells over Core. `seaquel` is the Tauri app in src-tauri/;
 * `seaquel-mcp` is the MCP server library behind `seaquel-cli mcp`;
 * `seaquel-terminal` is what the terminal binaries `seaquel-cli` and
 * `seaquel-tui` share (phase 7a).
 */
const INTERFACES = new Set([
  "seaquel",
  "seaquel-server",
  "seaquel-mcp",
  "seaquel-cli",
  "seaquel-terminal",
  "seaquel-tui",
]);

/**
 * The browser demo's module (phase 8): an interface like the others, except
 * that it builds Core with DuckDB's browser driver itself
 * (`seaquel_engine_duckdb::browser_engine` over the page's DuckDB-WASM),
 * since Core's `browser` feature registers no engine. That one engine crate
 * is all it may name beyond an interface's crates.
 */
const BROWSER_INTERFACE = new Set(["seaquel-browser"]);
const BROWSER_MAY_ALSO_USE = new Set(["seaquel-engine-duckdb"]);

/**
 * Code shared by the interfaces (the wire types and dispatcher behind the
 * Tauri command and the server route). Like an interface it goes through
 * Core; unlike one it may also use the `Dialect` trait from seaquel-engine.
 */
const INTERFACE_GLUE = new Set(["seaquel-rpc"]);

/**
 * Binaries that host one engine out of process:
 * `seaquel-duckdb` runs DuckDB for the terminal binaries, which talk to it
 * over pipes. Each may depend on its own engine crate and the pure crates
 * only, and nothing may depend on it.
 */
const ENGINE_HOSTS = {
  "seaquel-duckdb": {
    engine: "seaquel-engine-duckdb",
    why: "the DuckDB helper may depend only on seaquel-engine-duckdb and the pure crates",
  },
};

/** Engine-agnostic test support. */
const TESTKIT = new Set(["seaquel-engine-testkit"]);

/**
 * Domain and infrastructure crates. They may depend on anything but engine
 * crates, and only Core may depend on them: interfaces reach them through
 * Core (`core.license_server()`, not `seaquel-license`). List new ones here
 * as they're created.
 */
const DOMAIN_AND_INFRA = new Set([
  "seaquel-workspace",
  "seaquel-storage",
  "seaquel-secrets",
  "seaquel-ssh",
  "seaquel-git",
  "seaquel-license",
  "seaquel-http",
  "seaquel-ai",
]);

/**
 * Domain crates that build for wasm32 (phase 6's `seaquel-ai`, which the
 * demo's module links): on top of the domain rules, they may depend only on
 * pure crates and seaquel-workspace, never on a native infrastructure crate
 * (seaquel-http, seaquel-secrets, ...).
 */
const WASM_DOMAIN = new Set(["seaquel-ai"]);
const WASM_DOMAIN_MAY_USE = new Set([...PURE, "seaquel-workspace"]);

const ENGINE_MAY_USE = new Set([
  "seaquel-engine",
  "seaquel-runtime",
  "seaquel-types",
  "seaquel-sql",
]);
/**
 * Interface crates that other interfaces may build on: `seaquel-cli` serves
 * the MCP server from `seaquel-mcp`, and both terminal binaries build on
 * `seaquel-terminal`. They follow the interface rules themselves.
 */
const INTERFACE_LIBS = new Set(["seaquel-mcp", "seaquel-terminal"]);

/**
 * Narrower rules for some interfaces (phase 7a): the TUI
 * doesn't link the MCP server (and so rmcp), and seaquel-terminal holds
 * policy over Core only, so neither binary picks up the other's extras
 * through it.
 */
const INTERFACE_LIMITS = {
  "seaquel-tui": {
    forbids: new Set(["seaquel-mcp"]),
    why: "seaquel-tui doesn't link the MCP server (rmcp); only seaquel-cli does",
  },
  "seaquel-terminal": {
    only: new Set(["seaquel-core", "seaquel-runtime", "seaquel-types"]),
    why: "seaquel-terminal may use only seaquel-core, seaquel-runtime and seaquel-types",
  },
};

const INTERFACE_MAY_USE = new Set([
  "seaquel-core",
  "seaquel-runtime",
  "seaquel-types",
  "seaquel-rpc",
  ...INTERFACE_LIBS,
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
  if (BROWSER_INTERFACE.has(name)) return "browser-interface";
  if (INTERFACE_GLUE.has(name)) return "interface-glue";
  if (TESTKIT.has(name)) return "testkit";
  if (Object.hasOwn(ENGINE_HOSTS, name)) return "engine-host";
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

    for (const dep of deps) {
      if (Object.hasOwn(ENGINE_HOSTS, dep)) {
        errors.push(`${pkg.name} -> ${dep}: ${dep} is a binary; nothing depends on it`);
      }
    }

    switch (kind) {
      case "engine-host": {
        const host = ENGINE_HOSTS[pkg.name];
        forbid((d) => d === host.engine || PURE.has(d), host.why);
        break;
      }
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
        for (const dep of deps) {
          const limit = INTERFACE_LIMITS[pkg.name];
          if (
            limit &&
            (limit.forbids?.has(dep) || (limit.only && !limit.only.has(dep))) &&
            !DOMAIN_AND_INFRA.has(dep)
          ) {
            errors.push(`${pkg.name} -> ${dep}: ${limit.why}`);
          } else if (DOMAIN_AND_INFRA.has(dep)) {
            errors.push(
              `${pkg.name} -> ${dep}: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())`,
            );
          } else if (!INTERFACE_MAY_USE.has(dep)) {
            errors.push(`${pkg.name} -> ${dep}: interfaces reach everything through seaquel-core`);
          }
        }
        break;
      case "browser-interface":
        for (const dep of deps) {
          if (DOMAIN_AND_INFRA.has(dep)) {
            errors.push(
              `${pkg.name} -> ${dep}: interfaces reach infrastructure crates through seaquel-core (e.g. core.license_server())`,
            );
          } else if (isEngine(dep) && !BROWSER_MAY_ALSO_USE.has(dep)) {
            errors.push(
              `${pkg.name} -> ${dep}: the browser module may name only seaquel-engine-duckdb's browser driver; other engines go through seaquel-core`,
            );
          } else if (!INTERFACE_MAY_USE.has(dep) && !BROWSER_MAY_ALSO_USE.has(dep)) {
            errors.push(`${pkg.name} -> ${dep}: interfaces reach everything through seaquel-core`);
          }
        }
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
        if (WASM_DOMAIN.has(pkg.name)) {
          forbid(
            (d) => isEngine(d) || WASM_DOMAIN_MAY_USE.has(d),
            `${pkg.name} builds for wasm32; it may only depend on pure crates and seaquel-workspace`,
          );
        }
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
