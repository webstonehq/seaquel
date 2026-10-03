/**
 * The editor's inline prompt on Core (phase 6 Task 7, Q9 and Decision 18):
 * `ai.generate` with the active saved connection, the SQL inserted at the
 * cursor and never run, and the box saying how to run it. Replays
 * `generate.json`'s page-view values (`inserted`, `executed`, `notice`,
 * `error`, `toasts`) with `changes.json`'s in place: Core's answer is the
 * case's expected SQL (Core's replay pins the extraction), or its error.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryEditorContext } from "./types.js";

const toasts = vi.hoisted(() => [] as string[]);
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/shortcuts/platform", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/shortcuts/platform")>()),
  isMac: () => true,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { createAIInlinePrompt } = await import("./ai-inline-prompt.svelte.js");
const { setAi } = await import("$lib/hooks/database/ai/index");
const { FakeAi } = await import("$lib/hooks/database/ai/testing");
const { CoreCallError } = await import("$lib/storage/rust-client");

const FIXTURES = join(process.cwd(), "crates/seaquel-ai/tests/fixtures/ts-baseline");
const read = <T>(f: string) => JSON.parse(readFileSync(join(FIXTURES, f), "utf8")) as T;

interface GenerateCase {
  name: string;
  input: {
    provider: { id: string } | null;
    model: string | null;
    request: string;
    existingQuery: string;
  };
  inserted: string[];
  executed: number;
  error: string | null;
  toasts: string[];
  notice: string | null;
}
const cases = read<GenerateCase[]>("generate.json");
const changes = read<Record<string, { expected: Record<string, unknown> }>>("changes.json");

function setup(input: GenerateCase["input"], existing: string) {
  const inserted: string[] = [];
  const executed = 0;
  const opened: string[] = [];
  const connection = {
    id: "conn-1",
    name: "Local",
    activeAIProviderId: input.provider?.id ?? null,
    activeAIModel: input.model,
  };
  const ctx = {
    db: {
      state: { activeConnection: connection },
      settingsTabs: { open: (section: string, item: string) => opened.push(`${section}/${item}`) },
    },
    getMonacoRef: () => ({ getCursorOffset: () => 0, insertText: (t: string) => inserted.push(t) }),
    getActiveTab: () => ({ query: existing }),
  } as unknown as QueryEditorContext;
  // The editor's Run isn't the prompt's to call any more (Q9): nothing can
  // run the tab from here, so `executed` stays 0.
  const prompt = createAIInlinePrompt(ctx);
  return { prompt, inserted, opened, executed: () => executed };
}

beforeEach(() => {
  toasts.length = 0;
});

describe("generate.json, on Core", () => {
  for (const c of cases) {
    const expected = changes[`generate/${c.name}`]?.expected ?? {};
    it(c.name, async () => {
      const ai = new FakeAi();
      setAi(ai);
      const code = expected.code as string | undefined;
      // Core's answer: the recorded insert (its extraction is pinned in
      // Rust), or the case's error.
      ai.generateAnswer = async () => {
        if (code) throw new CoreCallError({ code, message: String(expected.message) });
        return c.inserted[0];
      };
      const { prompt, inserted, executed } = setup(c.input, c.input.existingQuery);
      prompt.handleOpen();
      prompt.text = c.input.request;
      await prompt.submit();

      expect(ai.generates).toEqual([
        {
          connectionId: "conn-1",
          providerId: c.input.provider?.id ?? null,
          request: c.input.request,
          existingQuery: c.input.existingQuery,
        },
      ]);
      expect(inserted).toEqual((expected.inserted as string[] | undefined) ?? c.inserted);
      expect(executed()).toBe((expected.executed as number | undefined) ?? c.executed);
      expect(prompt.notice).toBe(
        expected.notice !== undefined ? expected.notice : (c.notice ?? null),
      );
      expect(prompt.error?.message ?? null).toBe(
        expected.error !== undefined ? expected.error : (c.error ?? null),
      );
      expect(toasts).toEqual((expected.toasts as string[] | undefined) ?? c.toasts);
    });
  }

  it("a key error offers Settings → AI", async () => {
    const ai = new FakeAi();
    setAi(ai);
    ai.generateAnswer = async () => {
      throw new CoreCallError({
        code: "NO_API_KEY",
        message: "No API key is set for this provider.",
      });
    };
    const { prompt, opened } = setup(
      { provider: { id: "prov-1" }, model: "m", request: "x", existingQuery: "" },
      "",
    );
    prompt.handleOpen();
    prompt.text = "x";
    await prompt.submit();
    expect(prompt.error?.action?.label).toBe("Settings → AI");
    prompt.error!.action!.fn();
    expect(opened).toEqual(["app/ai-provider"]);
  });
});
