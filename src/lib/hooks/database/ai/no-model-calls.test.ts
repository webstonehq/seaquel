/**
 * Model calls left the page (phase 6, Decisions 7 and 19): no provider URL
 * or provider header in the page's code under `src/`, and the desktop CSP lets the page
 * reach only itself, the IPC bridge, the dev server's HMR socket and the
 * DuckDB community extension list, which must still load.
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = process.cwd();
const SRC = join(ROOT, "src");
/**
 * Generated Rust modules (`npm run wasm:build`) carry Core's own strings, and
 * the browser module's mock provider (test only; the check below keeps the
 * app from importing it) sends Anthropic's fixed URL to itself.
 */
const SKIP = [
  "src/lib/wasm/pkg",
  "src/lib/wasm/browser-pkg",
  "src/lib/wasm/browser-test-pkg",
  "src/lib/core/browser/testing/mock-provider.ts",
];

/**
 * Test support outside `core/browser/testing/` that may import it: each is
 * itself imported only by tests (checked below).
 */
const TEST_SUPPORT = ["src/lib/hooks/database/library/fixture-support.ts"];

function files(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    const rel = relative(ROOT, path);
    if (name === "node_modules" || SKIP.some((s) => rel.startsWith(s))) continue;
    if (statSync(path).isDirectory()) out.push(...files(path));
    // Tests may name what Core sends (`core/browser/ai-turn.test.ts` checks
    // the module's request); the page's own code may not.
    else if (/\.(ts|js|svelte)$/.test(name) && !name.endsWith(".test.ts")) out.push(path);
  }
  return out;
}

describe("no model call from the page", () => {
  it("names no provider URL or provider header in src/", () => {
    const banned = [
      "api.anthropic.com",
      "api.openai.com",
      "anthropic-version",
      "anthropic-dangerous-direct-browser-access",
      "/chat/completions",
      "x-api-key",
    ];
    const hits: string[] = [];
    for (const path of files(SRC)) {
      const text = readFileSync(path, "utf8");
      for (const b of banned) if (text.includes(b)) hits.push(`${relative(ROOT, path)}: ${b}`);
    }
    expect(hits).toEqual([]);
  });
});

describe("the browser module's test harness stays out of the app", () => {
  const all = files(SRC).concat(
    // `files` leaves the mock out of the scan above; it's still harness.
    [join(SRC, "lib/core/browser/testing/mock-provider.ts")],
  );
  const appFiles = all.filter(
    (path) => !relative(ROOT, path).startsWith("src/lib/core/browser/testing/"),
  );

  it("no app file imports core/browser/testing", () => {
    const hits = appFiles
      .map((path) => relative(ROOT, path))
      .filter((rel) => !TEST_SUPPORT.includes(rel))
      .filter((rel) =>
        /core\/browser\/testing|from "\.\/testing\//.test(readFileSync(join(ROOT, rel), "utf8")),
      );
    expect(hits).toEqual([]);
  });

  it("the test support that does is imported only by tests", () => {
    for (const support of TEST_SUPPORT) {
      const name = support.split("/").at(-1)!.replace(/\.ts$/, "");
      const importers = appFiles
        .map((path) => relative(ROOT, path))
        .filter((rel) => rel !== support && readFileSync(join(ROOT, rel), "utf8").includes(name));
      expect(importers, support).toEqual([]);
    }
  });
});

describe("the desktop CSP (Decision 19)", () => {
  const conf = JSON.parse(readFileSync(join(ROOT, "src-tauri/tauri.conf.json"), "utf8")) as {
    app: { security: { csp: string } };
  };
  const directives = new Map(
    conf.app.security.csp.split(";").map((d) => {
      const [name, ...values] = d.trim().split(/\s+/);
      return [name, values] as const;
    }),
  );

  it("connect-src is the page, IPC, the HMR socket and duckdb.org only", () => {
    expect(directives.get("connect-src")).toEqual([
      "'self'",
      "ipc:",
      "http://ipc.localhost",
      "ws://localhost:1420",
      "https://duckdb.org",
    ]);
  });

  it("still lets the DuckDB community extension list load", () => {
    const tab = readFileSync(
      join(SRC, "lib/hooks/database/extensions-duckdb-tabs.svelte.ts"),
      "utf8",
    );
    const urls = [...tab.matchAll(/fetch\("([^"]+)"/g)].map((m) => new URL(m[1]).origin);
    expect(urls).toEqual(["https://duckdb.org"]);
    for (const origin of urls) expect(directives.get("connect-src")).toContain(origin);
  });

  it("keeps compiling WebAssembly without eval", () => {
    expect(directives.get("script-src")).toEqual(["'self'", "'wasm-unsafe-eval'"]);
  });
});
