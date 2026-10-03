/**
 * Oversized replies (phase 6 probe F2). Core cuts a reply at the most a
 * stored reply may hold and ends it with its note (`REPLY_CUT_NOTE`); the
 * page shows its own wording instead of the note, live and after a
 * reload. A reply over 64 KiB is shown as plain text, and so is one
 * `marked` can't render.
 */
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import { messageDraft, messageFromWire } from "../library/convert";
import { PLAIN_REPLY_BYTES, REPLY_CUT_NOTE, replyHtml, splitCutNote } from "./reply";

describe("the cut note", () => {
  it("is Core's, byte for byte", () => {
    const source = readFileSync("crates/seaquel-workspace/src/ai.rs", "utf8");
    const match = /pub const REPLY_CUT_NOTE: &str =\s*"((?:[^"\\]|\\.)*)";/.exec(source);
    expect(match).not.toBeNull();
    expect(JSON.parse(`"${match![1]}"`)).toBe(REPLY_CUT_NOTE);
  });

  it("is taken off a stored reply, which is marked cut", () => {
    expect(splitCutNote(`Half an answer${REPLY_CUT_NOTE}`)).toEqual({
      content: "Half an answer",
      cut: true,
    });
    expect(splitCutNote("A whole answer")).toEqual({ content: "A whole answer", cut: false });
  });

  it("a stored reply reads as cut, and goes back with its note", () => {
    const message = messageFromWire({
      id: "a-1",
      chatId: "chat-1",
      role: "assistant",
      content: `Half an answer${REPLY_CUT_NOTE}`,
      timestamp: "2026-01-01T00:00:00.000Z",
    });
    expect(message).toMatchObject({ content: "Half an answer", cut: true });
    expect(messageDraft(message).content).toBe(`Half an answer${REPLY_CUT_NOTE}`);
    // A user's message is never read as cut.
    const user = messageFromWire({
      id: "u-1",
      chatId: "chat-1",
      role: "user",
      content: `Typed${REPLY_CUT_NOTE}`,
      timestamp: "2026-01-01T00:00:00.000Z",
    });
    expect(user.content).toBe(`Typed${REPLY_CUT_NOTE}`);
    expect(user.cut).toBeUndefined();
  });
});

describe("rendering a reply", () => {
  const parse = (text: string) => `<p>${text}</p>`;
  const sanitize = (html: string) => html;

  it("renders Markdown up to 64 KiB", () => {
    expect(replyHtml("**hi**", parse, sanitize)).toBe("<p>**hi**</p>");
    expect(replyHtml("x".repeat(PLAIN_REPLY_BYTES), parse, sanitize)).not.toBeNull();
  });

  it("shows a longer reply as plain text, without calling marked", () => {
    const spy = vi.fn(parse);
    expect(PLAIN_REPLY_BYTES).toBe(64 * 1024);
    expect(replyHtml("x".repeat(PLAIN_REPLY_BYTES + 1), spy, sanitize)).toBeNull();
    // Counted in UTF-8 bytes: 22k three-byte characters are past it.
    expect(replyHtml("€".repeat(22_000), spy, sanitize)).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("falls back to plain text when marked throws", () => {
    const throwing = () => {
      throw new RangeError("Maximum call stack size exceeded");
    };
    expect(replyHtml("deeply\n".repeat(10), throwing, sanitize)).toBeNull();
  });
});
