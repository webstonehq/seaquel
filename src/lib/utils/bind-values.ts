import { cellText, decodeCell } from "$lib/values";

/**
 * `value` cut so formatting it costs at most about `budget` characters: a
 * string's start, a byte array's first `budget / 2` bytes (two hex digits
 * each), an array's first `budget` items, each cut the same way.
 */
function cut(value: unknown, budget: number): unknown {
  if (typeof value === "string") return value.length > budget ? value.slice(0, budget) : value;
  if (value instanceof Uint8Array) {
    const n = Math.ceil(budget / 2);
    return value.length > n ? value.subarray(0, n) : value;
  }
  if (Array.isArray(value)) return value.slice(0, budget).map((v) => cut(v, budget));
  return value;
}

/** A bind value as the SQL view lists it beside the statement. */
function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "string") return `'${value}'`;
  return cellText(value);
}

/**
 * A statement's values (decoded), numbered in bind order, e.g. `1: 'a'  2: 5`:
 * the pending-changes sheet's and the history list's "Values: …" line. With
 * `maxLength`, formatting stops once the line reaches it (each value is cut
 * before it's formatted) and the line ends with "…".
 */
export function formatBindValues(
  values: readonly unknown[] | undefined,
  maxLength = Infinity,
): string {
  return formatEach(values, maxLength, (v, budget) => cut(v, budget));
}

function formatEach(
  values: readonly unknown[] | undefined,
  maxLength: number,
  prepare: (value: unknown, budget: number) => unknown,
): string {
  let out = "";
  for (let i = 0; i < (values?.length ?? 0); i++) {
    const budget = maxLength - out.length;
    const part = `${i === 0 ? "" : "  "}${i + 1}: ${formatValue(prepare(values![i], budget))}`;
    out += part;
    if (out.length >= maxLength) {
      return out.length > maxLength || i < values!.length - 1 ? out.slice(0, maxLength) + "…" : out;
    }
  }
  return out;
}

/**
 * A wire value cut like `cut` before it's decoded, so a long one is never
 * decoded whole: a string's start, a `bytes` tag's first base64 groups, an
 * array's first items. Returns a new value; the input isn't changed.
 */
function cutWire(value: unknown, budget: number): unknown {
  if (typeof value === "string") return value.length > budget ? value.slice(0, budget) : value;
  if (Array.isArray(value)) return value.slice(0, budget).map((v) => cutWire(v, budget));
  if (
    value !== null &&
    typeof value === "object" &&
    (value as { $sq?: unknown }).$sq === "bytes" &&
    typeof (value as { v?: unknown }).v === "string"
  ) {
    const b64 = (value as { v: string }).v;
    const keep = Math.ceil(Math.ceil(budget / 2) / 3) * 4;
    return b64.length > keep ? { $sq: "bytes", v: b64.slice(0, keep) } : value;
  }
  return value;
}

/**
 * `formatBindValues` for values still in the cell wire format (a history
 * row's): each is cut, then decoded, then formatted. Doesn't change them.
 */
export function formatWireBindValues(
  values: readonly unknown[] | undefined,
  maxLength = Infinity,
): string {
  return formatEach(values, maxLength, (v, budget) => cut(decodeCell(cutWire(v, budget)), budget));
}
