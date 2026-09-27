/**
 * Helpers shared by the providers: the stream-frame error text, columnar rows
 * to row objects, and `selectReadOnly`'s collector. The wire shapes
 * themselves are generated from the Rust crates (`src/lib/types/generated`).
 */

import type { ReadOnlyRows } from "./types";
import { dedupeColumnNames } from "$lib/utils/row-access";

/** True for the error a SQLite connect/test returns when the database file doesn't exist. */
export function isFileNotFoundError(message: string | null): boolean {
  return message?.startsWith("FILE_NOT_FOUND:") ?? false;
}

// -------- Stream-frame helpers --------

/**
 * Format a `type: "error"` stream frame into the `"CODE: message"` shape the
 * UI expects. Missing or non-string fields would otherwise surface to the
 * user as `"undefined: undefined"`.
 */
export function formatStreamErrorFrame(frame: { code?: unknown; message?: unknown }): string {
  const code = typeof frame.code === "string" ? frame.code : "ERROR";
  const message = typeof frame.message === "string" ? frame.message : "unknown stream error";
  return `${code}: ${message}`;
}

/**
 * Format an unexpected (`type` not in the known set) stream frame. Returned
 * as the `error` field of the terminal result so the stream caller fails
 * loudly rather than hanging.
 */
export function formatUnknownStreamFrame(frame: unknown): string {
  const type = (frame as { type?: unknown })?.type;
  return `unexpected stream event: ${typeof type === "string" ? type : "<missing>"}`;
}

// -------- Rows --------

/**
 * Columnar rows → row objects. Column names are deduped first so
 * `SELECT a.id, b.id FROM a JOIN b` keeps both values (`{ id, id_2 }`)
 * instead of the second overwriting the first.
 */
export function toRowObjects(columns: string[], rows: unknown[][]): Record<string, unknown>[] {
  const names = dedupeColumnNames(columns);
  return rows.map((row) => {
    const obj: Record<string, unknown> = {};
    for (let i = 0; i < names.length; i++) {
      obj[names[i]] = row[i];
    }
    return obj;
  });
}

/** One batch as a provider's `selectStream` hands it to `onBatch`. */
export interface StreamBatch {
  columns: string[] | null;
  rows: unknown[][];
  isFinal: boolean;
  /**
   * Only on the final batch of a read-only query run with `maxRows`: it had
   * more rows than that. Absent everywhere else.
   */
  truncated?: boolean;
}

/** What a provider's stream resolves with: `selectStream`'s result. */
export interface StreamOutcome {
  aborted: boolean;
  error?: string;
}

/** The rejection of a read-only query the caller's signal cancelled. */
export function queryCancelled(): DOMException {
  return new DOMException("Query cancelled", "AbortError");
}

/**
 * `selectReadOnly` over a Core stream: `run` starts the read-only stream with this `onBatch` and the caller's
 * signal, and this collects its batches into row objects, with the final
 * batch's `truncated`. Rejects with the stream's error (`"READ_ONLY: …"`),
 * or with an `AbortError` when the signal cancelled it.
 */
export async function collectReadOnly(
  run: (onBatch: (batch: StreamBatch) => boolean) => Promise<StreamOutcome>,
  signal?: AbortSignal,
): Promise<ReadOnlyRows> {
  if (signal?.aborted) throw queryCancelled();
  let columns: string[] | null = null;
  const rows: unknown[][] = [];
  let truncated = false;
  const outcome = await run((batch) => {
    columns ??= batch.columns;
    // Not `push(...batch.rows)`: a 100,000-row batch overflows the stack.
    for (const row of batch.rows) rows.push(row);
    if (batch.isFinal) truncated = batch.truncated === true;
    return true;
  });
  if (outcome.error !== undefined) throw new Error(outcome.error);
  if (outcome.aborted || signal?.aborted) throw queryCancelled();
  return { rows: toRowObjects(columns ?? [], rows), truncated };
}
