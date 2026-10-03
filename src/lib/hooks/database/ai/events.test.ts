/**
 * The reply's segments (phase 6 Task 7, Q7): text and one line per tool
 * call, built as a turn's events arrive and read back from a stored
 * reply's `parts` (Decision 23), so a reloaded chat shows the same lines.
 */
import { describe, expect, it } from "vitest";
import {
  appendText,
  finishTool,
  mergeSegments,
  segmentsFromParts,
  startTool,
  waitTool,
} from "./events";
import type { AiSegment } from "$lib/types";

describe("live segments", () => {
  it("text joins the last text segment; a tool call starts a line", () => {
    let s: AiSegment[] = [];
    s = appendText(s, "Check");
    s = appendText(s, "ing. ");
    s = startTool(s, { callId: "c1", name: "run_query", sql: "SELECT 1" });
    s = waitTool(s, "c1");
    expect(s.at(-1)).toMatchObject({ type: "tool", state: "waiting" });
    s = finishTool(s, { callId: "c1", ok: true, rows: 3, truncated: false });
    s = appendText(s, "Done.");
    expect(s).toEqual([
      { type: "text", text: "Checking. " },
      {
        type: "tool",
        callId: "c1",
        name: "run_query",
        sql: "SELECT 1",
        state: "ok",
        rows: 3,
        truncated: false,
      },
      { type: "text", text: "Done." },
    ]);
  });

  it("a failed call keeps its code", () => {
    const s = finishTool(startTool([], { callId: "c1", name: "run_query" }), {
      callId: "c1",
      ok: false,
      code: "DENIED",
    });
    expect(s).toEqual([
      { type: "tool", callId: "c1", name: "run_query", state: "error", code: "DENIED" },
    ]);
  });
});

describe("stored parts", () => {
  const parts = [
    { round: 0, type: "text", text: "Checking. " },
    {
      round: 0,
      type: "tool",
      callId: "call_1",
      name: "run_query",
      input: { sql: "SELECT count(*) AS n FROM users" },
      ok: false,
      result: "DENIED: User denied query execution",
    },
    { round: 1, type: "text", text: "Checking. " },
    {
      round: 1,
      type: "tool",
      callId: "call_2",
      name: "run_query",
      input: { sql: "SELECT 1 AS n" },
      ok: true,
      result: '{"columns":["n"],"rowCount":1,"rows":[[1]],"truncated":false}',
    },
    { round: 2, type: "text", text: "Done." },
  ];

  it("read back as the same lines (page/approval-card's stored reply)", () => {
    expect(segmentsFromParts(parts)).toEqual([
      { type: "text", text: "Checking. " },
      {
        type: "tool",
        callId: "call_1",
        name: "run_query",
        sql: "SELECT count(*) AS n FROM users",
        state: "error",
        code: "DENIED",
      },
      { type: "text", text: "Checking. " },
      {
        type: "tool",
        callId: "call_2",
        name: "run_query",
        sql: "SELECT 1 AS n",
        state: "ok",
        rows: 1,
        truncated: false,
      },
      { type: "text", text: "Done." },
    ]);
  });

  it("no parts (an older release's row, or a reply without calls) is no segments", () => {
    expect(segmentsFromParts(undefined)).toBeUndefined();
    expect(segmentsFromParts([])).toBeUndefined();
    expect(segmentsFromParts([{ type: "text", text: "x" }])).toBeUndefined();
  });

  it("ignores what doesn't read as a part", () => {
    expect(segmentsFromParts([null, 3, { type: "tool" }, ...parts.slice(3, 4)])).toEqual([
      {
        type: "tool",
        callId: "call_2",
        name: "run_query",
        sql: "SELECT 1 AS n",
        state: "ok",
        rows: 1,
        truncated: false,
      },
    ]);
  });

  it("a result cut at 16 KB still says its rows when they read", () => {
    const cut = { ...parts[3], result: '{"columns":["n"],"rowCount":40,"rows":[["xx' };
    expect(segmentsFromParts([cut])).toEqual([
      { type: "tool", callId: "call_2", name: "run_query", sql: "SELECT 1 AS n", state: "ok" },
    ]);
  });

  it("merging keeps what the live events knew", () => {
    const stored = segmentsFromParts([{ ...parts[3], result: "not json" }])!;
    const live: AiSegment[] = [
      { type: "tool", callId: "call_2", name: "run_query", state: "ok", rows: 5, truncated: true },
    ];
    expect(mergeSegments(stored, live)).toEqual([
      {
        type: "tool",
        callId: "call_2",
        name: "run_query",
        sql: "SELECT 1 AS n",
        state: "ok",
        rows: 5,
        truncated: true,
      },
    ]);
  });
});
