/**
 * A reply's segments (phase 6, Q7): its text and one line per tool call, in
 * order. Built from a turn's events as they arrive (`appendText`,
 * `startTool`, `waitTool`, `finishTool`), and read back from a stored
 * reply's `parts` (Decision 23), so a chat reloaded later shows the same
 * lines. Pure: the view model holds the result.
 */
import type { AIMessage, AiSegment, AiToolLine } from "$lib/types";

type ToolSegment = Extract<AiSegment, { type: "tool" }>;

/** `delta` added to the last text segment, or as a new one after a tool line. */
export function appendText(segments: AiSegment[], delta: string): AiSegment[] {
  if (!delta) return segments;
  const last = segments.at(-1);
  if (last?.type === "text")
    return [...segments.slice(0, -1), { ...last, text: last.text + delta }];
  return [...segments, { type: "text", text: delta }];
}

/** A tool call's line, running. */
export function startTool(
  segments: AiSegment[],
  call: { callId: string; name: string; sql?: string },
): AiSegment[] {
  const line: ToolSegment = {
    type: "tool",
    callId: call.callId,
    name: call.name,
    state: "running",
  };
  if (call.sql !== undefined) line.sql = call.sql;
  return [...segments, line];
}

function update(
  segments: AiSegment[],
  callId: string,
  change: (s: ToolSegment) => ToolSegment,
): AiSegment[] {
  return segments.map((s) => (s.type === "tool" && s.callId === callId ? change(s) : s));
}

/** The call waits for the approval card. */
export function waitTool(segments: AiSegment[], callId: string): AiSegment[] {
  return update(segments, callId, (s) => ({ ...s, state: "waiting" }));
}

/** The call is back to running (approved). */
export function resumeTool(segments: AiSegment[], callId: string): AiSegment[] {
  return update(segments, callId, (s) => (s.state === "waiting" ? { ...s, state: "running" } : s));
}

/** A call's outcome (`toolDone`). */
export function finishTool(
  segments: AiSegment[],
  done: { callId: string; ok: boolean; rows?: number; truncated?: boolean; code?: string },
): AiSegment[] {
  return update(segments, done.callId, (s) => {
    const next: ToolSegment = { ...s, state: done.ok ? "ok" : "error" };
    if (done.rows !== undefined) next.rows = done.rows;
    if (done.truncated !== undefined) next.truncated = done.truncated;
    if (done.code !== undefined) next.code = done.code;
    return next;
  });
}

/** Calls still running or waiting become stopped ones (the turn ended without them). */
export function settleTools(segments: AiSegment[]): AiSegment[] {
  return segments.map((s) =>
    s.type === "tool" && (s.state === "running" || s.state === "waiting")
      ? { ...s, state: "error", code: "CANCELLED" }
      : s,
  );
}

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/** What a stored result says of its rows: MCP's JSON (`rowCount`, `truncated`). */
function rowsOf(result: string): Pick<AiToolLine, "rows" | "truncated"> {
  try {
    const parsed: unknown = JSON.parse(result);
    if (isObject(parsed) && typeof parsed.rowCount === "number") {
      const out: Pick<AiToolLine, "rows" | "truncated"> = { rows: parsed.rowCount };
      if (typeof parsed.truncated === "boolean") out.truncated = parsed.truncated;
      return out;
    }
  } catch {
    // A result cut at 16 KB, or one that isn't rows: no count.
  }
  return {};
}

/**
 * A stored reply's `parts` as segments, or `undefined` when there are none
 * (no tool call got a result: Core stores no `parts` then, and an older
 * release's row has none).
 */
export function segmentsFromParts(parts: unknown[] | null | undefined): AiSegment[] | undefined {
  if (!Array.isArray(parts)) return undefined;
  let out: AiSegment[] = [];
  for (const part of parts) {
    if (!isObject(part)) continue;
    if (part.type === "text" && typeof part.text === "string") {
      out = appendText(out, part.text);
      continue;
    }
    if (part.type !== "tool" || typeof part.callId !== "string" || typeof part.name !== "string") {
      continue;
    }
    const input = isObject(part.input) ? part.input : {};
    const line: ToolSegment = {
      type: "tool",
      callId: part.callId,
      name: part.name,
      state: part.ok === true ? "ok" : "error",
    };
    if (typeof input.sql === "string") line.sql = input.sql;
    const result = typeof part.result === "string" ? part.result : "";
    if (part.ok === true) Object.assign(line, rowsOf(result));
    else {
      const code = /^([A-Z][A-Z0-9_]*):/.exec(result)?.[1];
      if (code) line.code = code;
    }
    out.push(line);
  }
  return out.some((s) => s.type === "tool") ? out : undefined;
}

/** Stored segments, with what the live events knew of each call (rows, a cut). */
export function mergeSegments(
  stored: AiSegment[] | undefined,
  live: AiSegment[] | undefined,
): AiSegment[] | undefined {
  if (!stored) return undefined;
  if (!live) return stored;
  const known = new Map(
    live.filter((s): s is ToolSegment => s.type === "tool").map((s) => [s.callId, s]),
  );
  return stored.map((s) => {
    if (s.type !== "tool") return s;
    const seen = known.get(s.callId);
    if (!seen) return s;
    const next: ToolSegment = { ...s };
    if (seen.rows !== undefined) next.rows = seen.rows;
    if (seen.truncated !== undefined) next.truncated = seen.truncated;
    if (seen.code !== undefined && next.code === undefined) next.code = seen.code;
    return next;
  });
}

/**
 * A stored message as the page shows it, over what the page showed of it:
 * the stored row wins (text, time, dashboard id, `parts`), and the live
 * view keeps what isn't stored: each call's rows and the turn's worded
 * error or cut (Decision 31: an error travels with the turn).
 */
export function withLiveView(stored: AIMessage, live: AIMessage | undefined): AIMessage {
  if (!live) return stored;
  const next: AIMessage = { ...stored };
  const segments = mergeSegments(stored.segments, live.segments);
  if (segments) next.segments = segments;
  if (live.error) next.error = live.error;
  if (live.truncated) next.truncated = live.truncated;
  return next;
}
