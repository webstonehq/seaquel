/**
 * `wellFormed` (phase 5b, Decision 3): a run's text goes to Core with each
 * lone surrogate replaced by U+FFFD, which keeps every UTF-16 offset, so the
 * cursor still picks the statement it picked in the editor.
 */
import { describe, expect, it } from "vitest";
import { getStatementAtOffset } from "$lib/sql";
import { wellFormed } from "./client";

const withoutNative = (text: string) => {
  // The fallback, as in a WebView without `toWellFormed`.
  const proto = String.prototype as { toWellFormed?: unknown };
  const saved = proto.toWellFormed;
  proto.toWellFormed = undefined;
  try {
    return wellFormed(text);
  } finally {
    proto.toWellFormed = saved;
  }
};

describe("wellFormed", () => {
  const cases: [string, string][] = [
    ["plain", "plain"],
    ["a\uD83D", "a�"],
    ["\uDE00b", "�b"],
    ["\uDE00\uD83D", "��"],
    ["ok 😀 pair", "ok 😀 pair"],
    ["x\uD83Dy😀\uDC00", "x�y😀�"],
  ];

  it.each(cases)("%j (native and fallback)", (input, expected) => {
    expect(wellFormed(input)).toBe(expected);
    expect(withoutNative(input)).toBe(expected);
    expect(wellFormed(input).length).toBe(input.length);
  });

  it("serialises to JSON Core accepts", () => {
    expect(JSON.stringify(wellFormed("'\uD83D'"))).toBe(`"'�'"`);
  });

  it("a lone surrogate is replaced before sending and the cursor still picks the same statement", () => {
    const text = "SELECT '\uD83D' AS a;\nSELECT '😀東京' AS b;\nSELECT 3 AS c";
    const sent = wellFormed(text);
    for (let cursor = 0; cursor <= text.length; cursor++) {
      const before = getStatementAtOffset(text, cursor, "postgres");
      const after = getStatementAtOffset(sent, cursor, "postgres");
      expect(after?.index, `cursor ${cursor}`).toBe(before?.index);
    }
  });
});
