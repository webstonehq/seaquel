/**
 * The tutorial stays as it was when the demo's assistant came back (phase 6
 * Task 8): Learn's pages, its sidebar, its editor and its database code
 * reach nothing of the assistant or the page's fetch bridge, and the
 * header offers the assistant only outside Learn.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = fileURLToPath(new URL("../../../", import.meta.url));

function files(path: string): string[] {
  const full = join(root, path);
  if (!statSync(full).isDirectory()) return [path];
  return readdirSync(full).flatMap((entry) => files(join(path, entry)));
}

const TUTORIAL = [
  "src/routes/(app)/learn",
  "src/lib/tutorial",
  "src/lib/components/sidebar-learn.svelte",
  "src/lib/components/standalone-query-editor.svelte",
]
  .flatMap(files)
  .filter((f) => /\.(ts|svelte)$/.test(f) && !f.endsWith(".test.ts"));

const AI = [
  "ai-assistant",
  "aiSettingsStore",
  "ai-settings.svelte",
  "hooks/database/ai",
  "getAi(",
  "fetch-bridge",
  "session-keys",
];

describe("the tutorial has no assistant", () => {
  it("no tutorial file reaches the assistant or the fetch bridge", () => {
    expect(TUTORIAL.length).toBeGreaterThan(5);
    const hits = TUTORIAL.flatMap((file) => {
      const text = readFileSync(join(root, file), "utf8");
      return AI.filter((needle) => text.includes(needle)).map((needle) => `${file}: ${needle}`);
    });
    expect(hits).toEqual([]);
  });

  it("the header's assistant button is outside Learn", () => {
    const header = readFileSync(join(root, "src/lib/components/app-header.svelte"), "utf8");
    const gate = header.indexOf("{#if !isLearnPage");
    const button = header.indexOf("{#if aiSettingsStore.available}");
    const end = header.indexOf("{/if}", header.indexOf("{/if}", button) + 1);
    expect(gate).toBeGreaterThan(-1);
    expect(button).toBeGreaterThan(gate);
    expect(end).toBeGreaterThan(button);
  });
});
