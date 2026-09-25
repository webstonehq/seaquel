/**
 * The TypeScript half of the cell value wire format. The Rust half is
 * `crates/seaquel-types/src/value.rs`.
 *
 * Cells arrive as plain JSON wherever JavaScript holds the value exactly, and
 * as a tagged object `{"$sq": kind, "v": …}` otherwise:
 *
 * | tag       | `v`                         | JS value     |
 * |-----------|-----------------------------|--------------|
 * | `bigint`  | integer digits              | `bigint`     |
 * | `float`   | `"NaN"`, `"inf"`, `"-inf"`  | `number`     |
 * | `decimal` | exact decimal text          | `SqlDecimal` |
 * | `bytes`   | base64                      | `Uint8Array` |
 * | `json`    | any JSON                    | `v` as-is    |
 *
 * The providers decode rows with `decodeRows` before any UI code sees them,
 * and encode parameters with `encodeParam` before sending them.
 */

/** Exact decimal from the database. Keeps the text Postgres sent (scale included). */
export class SqlDecimal {
  constructor(readonly value: string) {}
  toString(): string {
    return this.value;
  }
  toJSON(): string {
    return this.value;
  }
}

interface Tagged {
  $sq: string;
  v?: unknown;
}

function isTagged(v: object): v is Tagged {
  return typeof (v as { $sq?: unknown }).$sq === "string";
}

function tag(kind: string, v: unknown): Tagged {
  return { $sq: kind, v };
}

function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes;
}

function bytesToBase64(bytes: Uint8Array): string {
  // Chunked so large blobs don't overflow the argument limit of fromCharCode.
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}

function decodeTagged(t: Tagged): unknown {
  // Same rule as Rust's `from_wire`: a tag without `v` is malformed.
  if (!("v" in t)) throw new Error(`tagged value "${t.$sq}" has no "v"`);
  switch (t.$sq) {
    case "bigint":
      return BigInt(t.v as string);
    case "float":
      if (t.v === "NaN") return NaN;
      if (t.v === "inf") return Infinity;
      if (t.v === "-inf") return -Infinity;
      throw new Error(`invalid float value: ${String(t.v)}`);
    case "decimal":
      return new SqlDecimal(t.v as string);
    case "bytes":
      return base64ToBytes(t.v as string);
    case "json":
      return t.v;
    default:
      throw new Error(`unknown value tag "${t.$sq}"`);
  }
}

/** Wire → JS. Recurses into arrays (decoding them in place). */
export function decodeCell(v: unknown): unknown {
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) {
    for (let i = 0; i < v.length; i++) {
      const item: unknown = v[i];
      if (item !== null && typeof item === "object") v[i] = decodeCell(item);
    }
    return v;
  }
  return isTagged(v) ? decodeTagged(v) : v;
}

/**
 * Decode every cell of a columnar batch in place and return it. Only object
 * cells (tags and arrays) are touched, so primitive-only rows cost one
 * `typeof` per cell.
 */
export function decodeRows(rows: unknown[][]): unknown[][] {
  for (const row of rows) {
    for (let i = 0; i < row.length; i++) {
      const cell = row[i];
      if (cell !== null && typeof cell === "object") row[i] = decodeCell(cell);
    }
  }
  return rows;
}

/** JS → wire, for query parameters. */
export function encodeParam(v: unknown): unknown {
  if (typeof v === "bigint") return tag("bigint", v.toString());
  if (typeof v === "number" && !Number.isFinite(v)) {
    return tag("float", Number.isNaN(v) ? "NaN" : v > 0 ? "inf" : "-inf");
  }
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) return v.map(encodeParam);
  if (v instanceof Uint8Array) return tag("bytes", bytesToBase64(v));
  if (v instanceof SqlDecimal) return tag("decimal", v.value);
  // A Date serializes as its ISO string, which binds as text, as it always has.
  if (v instanceof Date) return v;
  return tag("json", v);
}

/** Encode a parameter list for the wire (`undefined` sends no parameters). */
export function encodeParams(params: unknown[] | undefined): unknown[] {
  return params ? params.map(encodeParam) : [];
}

/** Postgres bytea hex format: `\x0102ff`. */
export function toHex(bytes: Uint8Array): string {
  let hex = "\\x";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return hex;
}

/**
 * `JSON.stringify` replacer for rows holding decoded values: bigint becomes
 * its digits, `Uint8Array` its hex, `SqlDecimal` its text.
 */
export function jsonReplacer(_key: string, v: unknown): unknown {
  if (typeof v === "bigint") return v.toString();
  if (v instanceof Uint8Array) return toHex(v);
  if (v instanceof SqlDecimal) return v.value;
  return v;
}

/**
 * Stable string key for comparing cell values (PK matching). Different types
 * give different keys on purpose: `cellKey(10n) !== cellKey(10)`.
 *
 * Known edge case: PK values are matched against the decoded row values, so
 * they agree as long as both sides come from the same result. An inline edit
 * of a bigint PK column (only |i| > 2^53 decodes to bigint) writes the edited
 * *string* back into the row, and `cellKey("…")` no longer equals the bigint's
 * key, so a pending change recorded against the old bigint stops matching
 * that row until the result is re-queried.
 */
export function cellKey(v: unknown): string {
  // Strings, numbers and SqlDecimal share keys on purpose ("10" matches 10, "1.50" matches SqlDecimal("1.50")) for PK matching; only bigint differs.
  if (v === null) return "\u0000null";
  if (v === undefined) return "\u0000undefined";
  if (typeof v === "string") return v;
  if (typeof v === "bigint") return `${v}n`;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (v instanceof SqlDecimal) return v.value;
  if (v instanceof Uint8Array) return toHex(v);
  return JSON.stringify(v, jsonReplacer);
}

function isPlainObject(v: object): v is Record<string, unknown> {
  const proto: unknown = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
}

/**
 * Number for charts and KPIs. bigint loses precision past 2^53, which is
 * accepted there; everything else behaves like `Number(v)`.
 */
export function toNumber(v: unknown): number {
  if (typeof v === "bigint") return Number(v);
  if (v instanceof SqlDecimal) return Number(v.value);
  return Number(v);
}

/**
 * Plain text for a decoded cell (CSV, clipboard, FK filters). Same as
 * `String(v)` except that bytes become hex, plain objects (JSON cells) become
 * JSON text instead of "[object Object]" (both also inside arrays), and
 * null/undefined become "".
 */
export function cellText(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (v instanceof Uint8Array) return toHex(v);
  if (Array.isArray(v)) return v.map(cellText).join(",");
  if (typeof v === "object" && isPlainObject(v)) return JSON.stringify(v, jsonReplacer);
  // oxlint-disable-next-line typescript/no-base-to-string
  return String(v);
}

const STORAGE_TAGS = new Set(["bigint", "float", "decimal", "bytes", "json"]);

/**
 * Prepare app state holding decoded cells (saved workflows keep their result
 * rows) for `JSON.stringify`: bigint, bytes, decimals and non-finite numbers
 * become the wire tags, which `fromStorable` turns back into the same values.
 * A plain object that already has a `$sq` key is wrapped in a `json` tag so
 * it can't be mistaken for a tag on load. Other objects (Date, …) are left
 * for `JSON.stringify` to handle as before.
 */
export function toStorable(v: unknown): unknown {
  if (typeof v === "bigint" || v instanceof Uint8Array || v instanceof SqlDecimal) {
    return encodeParam(v);
  }
  if (typeof v === "number") return Number.isFinite(v) ? v : encodeParam(v);
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) return v.map(toStorable);
  if (!isPlainObject(v)) return v;
  if ("$sq" in v) return tag("json", v);
  const out: Record<string, unknown> = {};
  for (const [k, item] of Object.entries(v)) out[k] = toStorable(item);
  return out;
}

/** Inverse of `toStorable`, for the result of `JSON.parse`. Untagged data passes through. */
export function fromStorable(v: unknown): unknown {
  if (v === null || typeof v !== "object") return v;
  if (Array.isArray(v)) return v.map(fromStorable);
  const obj = v as Record<string, unknown>;
  if (typeof obj.$sq === "string" && STORAGE_TAGS.has(obj.$sq) && "v" in obj) {
    return decodeTagged(obj as unknown as Tagged);
  }
  const out: Record<string, unknown> = {};
  for (const [k, item] of Object.entries(obj)) out[k] = fromStorable(item);
  return out;
}
