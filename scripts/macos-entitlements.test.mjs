// The desktop DuckDB helper plan, Task 8 (Q12 B, Decision 14): only the
// DuckDB helper keeps `disable-library-validation`, which lets DuckDB load
// its extensions (signed by DuckDB, not us; issue #114). The app, the CLI and
// the TUI load no DuckDB in process, so they are signed with no entitlements.
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");

const HELPER_ENTITLEMENTS = "src-tauri/macos/duckdb-helper.entitlements.plist";
const LIBRARY_VALIDATION = "com.apple.security.cs.disable-library-validation";

/** The `run:` line of the release workflow's step named `name`. */
function stepRun(workflow, name) {
  const lines = workflow.split("\n");
  const at = lines.findIndex((line) => line.trim() === `- name: ${name}`);
  if (at < 0) throw new Error(`release.yml has no step named "${name}"`);
  for (let i = at + 1; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line.startsWith("- ")) break;
    if (line.startsWith("run:")) return line.slice("run:".length).trim();
  }
  throw new Error(`release.yml's step "${name}" has no one-line run:`);
}

/** The keys set to `<true/>` in a plist's top-level dict. */
function trueKeys(plist) {
  return [...plist.matchAll(/<key>([^<]+)<\/key>\s*<true\/>/g)].map((m) => m[1]);
}

describe("macOS entitlements", () => {
  const workflow = read(".github/workflows/release.yml");

  it("signs the DuckDB helper with the helper's entitlements file", () => {
    const run = stepRun(workflow, "Sign macOS DuckDB helper");
    expect(run).toContain("--options runtime");
    expect(run).toContain(`--entitlements ${HELPER_ENTITLEMENTS}`);
    expect(run).toMatch(/seaquel-duckdb-\$\{\{ matrix\.target \}\}$/);
  });

  it("signs the CLI and the TUI with the hardened runtime and no entitlements", () => {
    for (const name of ["Sign macOS CLI", "Sign macOS TUI"]) {
      const run = stepRun(workflow, name);
      expect(run, name).toContain("--options runtime");
      expect(run, name).not.toContain("--entitlements");
    }
  });

  it("names no other entitlements file anywhere in the release workflow", () => {
    const named = [...workflow.matchAll(/--entitlements\s+(\S+)/g)].map((m) => m[1]);
    expect(named).toEqual([HELPER_ENTITLEMENTS]);
  });

  it("gives the app no entitlements", () => {
    const config = JSON.parse(read("src-tauri/tauri.conf.json"));
    expect(config.bundle?.macOS?.entitlements).toBeUndefined();
    expect(existsSync(resolve(root, "src-tauri/macos/entitlements.plist"))).toBe(false);
  });

  it("keeps exactly disable-library-validation in the helper's file", () => {
    const plist = read(HELPER_ENTITLEMENTS);
    expect(plist).toContain('<plist version="1.0">');
    expect(trueKeys(plist)).toEqual([LIBRARY_VALIDATION]);
    expect([...plist.matchAll(/<key>/g)]).toHaveLength(1);
  });
});
