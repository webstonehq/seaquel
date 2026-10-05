/**
 * How the page words a turn's or a call's error code.
 * `errors.json` recorded the TypeScript wording, and
 * `changes.json` left `shown` `$absent` until this task: each case's
 * Core code and message now show as below (the ts-baseline README's
 * "Corrections" records the same table). Every code Core can end a turn
 * with is worded, never echoed raw; only a provider's own message (cut at
 * 1 KiB by Core) is shown inside the sentence.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { aiErrorText, inlineErrorOf, toolLineText, AI_ERROR_CODES } from "./messages";

const FIXTURES = join(process.cwd(), "crates/seaquel-ai/tests/fixtures/ts-baseline");
const read = (f: string) => JSON.parse(readFileSync(join(FIXTURES, f), "utf8"));

interface ErrorCase {
  name: string;
  input: string;
}
const errors = read("errors.json") as ErrorCase[];
const changes = read("changes.json") as Record<
  string,
  { expected: { code?: string; message?: string; shown?: unknown } }
>;

/** What each `errors.json` case shows now. */
const SHOWN: Record<string, string> = {
  "assistant/no-provider":
    "This chat has no AI provider. Add one in Settings → AI, then pick a model below.",
  "assistant/no-api-key": "No API key is set for this provider. Add your key in Settings → AI.",
  "assistant/rate-limit":
    "The provider is limiting requests (Rate limited). Wait a moment and try again.",
  "assistant/tool-limit":
    "The model asked for more than 20 tool calls in this turn, so the turn was stopped.",
  "assistant/provider-body": "AI provider error: Internal server error",
  "assistant/fetch-failed": "AI provider error: Could not reach the provider.",
  "assistant/no-stream": "AI provider error: The provider's stream ended before the reply did.",
};

describe("errors.json, worded (Decision 15)", () => {
  for (const c of errors) {
    const name = `errors/${c.name}`;
    const expected = changes[name]?.expected;
    if (!expected?.code) {
      it(`${c.name} stays the page's own check`, () => {
        // `connection-removed`: the page stops the send itself; its text is
        // pinned by `page/connection-removed` (`page-replay`).
        expect(c.name).toBe("assistant/connection-removed");
      });
      continue;
    }
    it(`${c.name}: ${expected.code}`, () => {
      expect(expected.shown).toEqual({ $absent: expect.any(String) });
      expect(aiErrorText(expected.code!, expected.message!)).toBe(SHOWN[c.name]);
    });
  }
});

describe("every code a turn or a call ends with is worded", () => {
  for (const code of AI_ERROR_CODES) {
    it(code, () => {
      const text = aiErrorText(code, "RAW-CORE-MESSAGE-xyz");
      expect(text).not.toBe("");
      // Only the provider's own message is ever shown inside the sentence.
      const echoes = ["PROVIDER_ERROR", "RATE_LIMITED"].includes(code);
      expect(text.includes("RAW-CORE-MESSAGE-xyz")).toBe(echoes);
      expect(text).not.toContain(code);
    });
  }

  it("a key for a provider the connection no longer uses (review I1)", () => {
    expect(aiErrorText("AI_PROVIDER_CHANGED", "The connection's AI provider changed.")).toBe(
      "The connection's AI provider changed. Send your message again.",
    );
  });

  it("a message past the web's cap says so (F1/F2/F5 review P1)", () => {
    expect(
      aiErrorText(
        "MESSAGE_TOO_LONG",
        "The message is longer than allowed here (max_message_bytes: 1048576 bytes).",
      ),
    ).toBe("Your message is too long to send.");
  });

  it("an unknown code says something went wrong, naming the code but never Core's message", () => {
    expect(aiErrorText("SOMETHING_NEW", "It broke.")).toBe(
      "Something went wrong (SOMETHING_NEW). Try again.",
    );
  });

  it("a limit's internal name never reaches the user (probe F2)", () => {
    const text = aiErrorText(
      "INVALID_ARGUMENT",
      "The message is longer than allowed here (max_message_bytes: 1048576 bytes).",
    );
    expect(text).not.toContain("max_message_bytes");
    expect(text).not.toContain("1048576");
  });
});

describe("the inline prompt's errors (generate.json's pinned `error` and `toasts`)", () => {
  it("words Core's codes as the prompt box did", () => {
    expect(inlineErrorOf("NO_PROVIDER", "No AI provider is configured.")).toEqual({
      message: "No AI provider configured.",
      action: "configure",
    });
    expect(inlineErrorOf("NO_MODEL", "No model is chosen for this connection.")).toEqual({
      message: "No model selected. Pick one from the model switcher to the right.",
    });
    expect(inlineErrorOf("NO_API_KEY", "No API key is set for this provider.")).toEqual({
      message: "No API key configured.",
      action: "settings",
    });
    expect(inlineErrorOf("RATE_LIMITED", "Rate limited")).toEqual({
      message: "Rate limit reached. Please wait and try again.",
    });
    expect(inlineErrorOf("PROVIDER_ERROR", "Internal server error")).toEqual({
      message: "Something went wrong. Please try again.",
      toast: "Internal server error",
    });
  });
});

describe("Q7's tool lines", () => {
  it("names the tool, then its rows or its error", () => {
    expect(toolLineText({ name: "run_query", state: "running" })).toBe("Running…");
    expect(toolLineText({ name: "run_query", state: "waiting" })).toBe("Waiting for your approval");
    expect(toolLineText({ name: "run_query", state: "ok", rows: 1 })).toBe("1 row");
    expect(
      toolLineText({
        name: "run_query",
        state: "ok",
        rows: 87,
        truncated: true,
      }),
    ).toBe("87 rows (cut)");
    expect(toolLineText({ name: "list_tables", state: "ok" })).toBe("Done");
    expect(toolLineText({ name: "run_query", state: "error", code: "DENIED" })).toBe("Denied");
    expect(toolLineText({ name: "run_query", state: "error", code: "READ_ONLY" })).toBe(
      "Failed (READ_ONLY)",
    );
  });
});
