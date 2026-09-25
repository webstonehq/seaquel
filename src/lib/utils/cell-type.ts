import { SqlDecimal, cellText, fromHex, jsonReplacer, toHex } from "$lib/values";

export type CellType =
  | "null"
  | "boolean"
  | "integer"
  | "float"
  | "date"
  | "datetime"
  | "time"
  | "uuid"
  | "json"
  | "array"
  | "binary"
  | "long_text"
  | "text";

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const DATETIME_RE =
  /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(:\d{2})?(\.\d+)?\s*(Z|[+-]\d{2}(:?\d{2}(:\d{2})?)?)?$/;
// An offset for TIME WITH TIME ZONE: DuckDB's `12:00:00+02`, `12:00:00-05:30`.
const TIME_RE = /^\d{2}:\d{2}(:\d{2})?(\.\d+)?([+-]\d{2}(:\d{2}){0,2})?$/;
const LONG_TEXT_THRESHOLD = 100;

export function detectCellType(value: unknown): CellType {
  if (value === null || value === undefined) return "null";
  if (typeof value === "boolean") return "boolean";
  if (typeof value === "number") return Number.isInteger(value) ? "integer" : "float";
  if (typeof value === "bigint") return "integer";
  // An integral decimal is an integer: MySQL BIGINT UNSIGNED above 2^63-1
  // arrives as the digits of a SqlDecimal (the wire has no unsigned int64).
  if (value instanceof SqlDecimal) return /^[+-]?\d+$/.test(value.value) ? "integer" : "float";
  if (value instanceof Uint8Array) return "binary";
  if (Array.isArray(value)) return "array";
  if (typeof value === "object") return "json";
  if (typeof value === "string") {
    if (UUID_RE.test(value)) return "uuid";
    if (DATE_RE.test(value)) return "date";
    if (DATETIME_RE.test(value)) return "datetime";
    if (TIME_RE.test(value)) return "time";
    if (value.length > LONG_TEXT_THRESHOLD) return "long_text";
    return "text";
  }
  return "text";
}

/**
 * Infer a CellType from a declared SQL column type (e.g. "BOOLEAN",
 * "timestamp", "varchar(255)"). Returns null when the declared type doesn't
 * map cleanly to a visual category — callers should fall back to value-based
 * sampling in that case.
 */
export function cellTypeFromColumnType(dbType: string): CellType | null {
  // BIT(n) with n > 1 holds a number (MySQL decodes it as one), not a flag.
  const bits = /^\s*bit\s*\(\s*(\d+)\s*\)/i.exec(dbType);
  if (bits && Number(bits[1]) > 1) return null;
  const t = dbType
    .toLowerCase()
    .replace(/\(.*\)/, "")
    .replace(/\s+/g, " ")
    .trim();

  if (/^(boolean|bool|bit)$/.test(t)) return "boolean";
  if (/timestamp|datetime|smalldatetime|datetimeoffset/.test(t)) return "datetime";
  if (t === "date") return "date";
  if (/^time\b/.test(t)) return "time";
  if (t === "uuid" || t === "uniqueidentifier") return "uuid";
  if (/^(json|jsonb)$/.test(t)) return "json";
  if (t.endsWith("[]")) return "array";
  if (/^(bytea|(tiny|medium|long)?blob|varbinary|binary|image)$/.test(t)) return "binary";
  return null;
}

export function detectColumnTypes(
  columns: string[],
  rows: unknown[][],
  declaredTypes?: Record<string, string>,
): Record<string, CellType> {
  const result: Record<string, CellType> = {};
  const sampleSize = 5;

  for (let colIdx = 0; colIdx < columns.length; colIdx++) {
    const column = columns[colIdx];

    // Prefer declared-type classification when available — SQLite stores
    // booleans as 0/1 integers, so value sampling alone would mis-classify
    // BOOLEAN columns as "integer".
    const declared = declaredTypes?.[column];
    const fromDeclared = declared ? cellTypeFromColumnType(declared) : null;
    if (fromDeclared) {
      result[column] = fromDeclared;
      continue;
    }

    const typeCounts = new Map<CellType, number>();
    let sampled = 0;

    for (const row of rows) {
      if (sampled >= sampleSize) break;
      const val = row[colIdx];
      if (val === null || val === undefined) continue;
      const type = detectCellType(val);
      typeCounts.set(type, (typeCounts.get(type) ?? 0) + 1);
      sampled++;
    }

    if (typeCounts.size === 0) {
      result[column] = "null";
    } else {
      let maxCount = 0;
      let maxType: CellType = "text";
      for (const [type, count] of typeCounts) {
        if (count > maxCount) {
          maxCount = count;
          maxType = type;
        }
      }
      result[column] = maxType;
    }
  }

  return result;
}

const numberFormatter = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 10,
});

const dateFormatter = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
});

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
});

const timeFormatter = new Intl.DateTimeFormat(undefined, {
  hour: "numeric",
  minute: "2-digit",
  second: "2-digit",
});

export function formatNumber(n: number | bigint): string {
  return numberFormatter.format(n);
}

/**
 * Display text for an integer/float cell. bigint is formatted exactly (Intl
 * accepts it), a `SqlDecimal` shows its exact text, anything else goes
 * through `Number()` as before.
 */
export function formatCellNumber(value: unknown): string {
  if (typeof value === "bigint") return formatNumber(value);
  if (value instanceof SqlDecimal) return value.value;
  return formatNumber(Number(value));
}

export function formatDate(s: string): string {
  const d = new Date(s + "T00:00:00");
  if (isNaN(d.getTime())) return s;
  return dateFormatter.format(d);
}

/**
 * What `formatDateTime` hands to `new Date()`. Anything else is shown as
 * the database printed it: V8's fallback parser reads DuckDB's
 * `0044-03-15 (BC) 12:00:00` as the year 2044 (the parentheses are a
 * comment to it).
 */
const ISO_DATETIME_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})?$/;

export function formatDateTime(s: string): string {
  const normalized = normalizeDatetime(s);
  if (!ISO_DATETIME_RE.test(normalized)) return s;
  const d = new Date(normalized);
  if (isNaN(d.getTime())) return s;
  return dateTimeFormatter.format(d);
}

/**
 * Normalize PostgreSQL-style timestamps like "2026-02-15 21:40:23.568684 +00:00:00"
 * and DuckDB's "2026-02-15 21:40:23.568684+00" into a format that `new Date()`
 * can parse.
 */
function normalizeDatetime(s: string): string {
  // Replace space separator with T for ISO compatibility
  const normalized = s.replace(/^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2})/, "$1T$2");
  return normalizeOffset(normalized);
}

/**
 * A timestamp's UTC offset as `new Date()` parses it: `+HH:MM`. Drops the
 * seconds part (+00:00:00 → +00:00), completes an hour-only offset
 * (DuckDB's +00 → +00:00) and adds the colon to `+0530`.
 */
function normalizeOffset(s: string): string {
  const time = String.raw`(\d{2}:\d{2}(?::\d{2})?(?:\.\d+)?)\s*`;
  return s
    .replace(/\s*([+-]\d{2}:\d{2}):\d{2}$/, "$1")
    .replace(new RegExp(`${time}([+-]\\d{2})$`), "$1$2:00")
    .replace(new RegExp(`${time}([+-]\\d{2})(\\d{2})$`), "$1$2:$3");
}

/** A time of day with a UTC offset: `12:00:00+02`, `12:00:00-05:30:15`. */
const TIME_WITH_ZONE_RE = /[+-]\d{2}(:?\d{2}){0,2}$/;

/**
 * A time of day in the viewer's format. A time with a zone (TIMETZ) is
 * shown as the database printed it: it has no date, so converting it to
 * local time would pick one (and drop offset seconds).
 */
export function formatTime(s: string): string {
  if (TIME_WITH_ZONE_RE.test(s)) return s;
  const d = new Date(`1970-01-01T${s}`);
  if (isNaN(d.getTime())) return s;
  return timeFormatter.format(d);
}

/**
 * How a binary column's string cells hold their bytes:
 * - `utf8`: MySQL/MariaDB send binary strings that are clean UTF-8 as text
 *   (the wire can't tell `VARBINARY` from a `*_bin` collation), so their bytes
 *   are the UTF-8 of the text.
 * - `text`: a string is text, not bytes. Every other engine, e.g. SQLite,
 *   which reports untyped columns as BLOB but keeps TEXT values in them.
 *   Postgres bytea, MSSQL binary and DuckDB BLOB (native, and the
 *   DuckDB-WASM demo) always arrive as a `Uint8Array`.
 */
export type BinaryStringEncoding = "utf8" | "text";

/** The encoding of string cells in binary columns for a connection type. */
export function binaryStringEncoding(dbType: string | undefined): BinaryStringEncoding {
  if (dbType === "mysql" || dbType === "mariadb") return "utf8";
  return "text";
}

/**
 * The bytes of a cell in a binary column: a `Uint8Array` as is, a string
 * as its UTF-8 under `utf8`. `null` for anything else, and for a string
 * under `text`.
 */
export function binaryCellBytes(
  value: unknown,
  encoding: BinaryStringEncoding = "text",
): Uint8Array | null {
  if (value instanceof Uint8Array) return value;
  if (typeof value !== "string") return null;
  if (encoding === "utf8") return new TextEncoder().encode(value);
  return null;
}

/**
 * The type to display a cell as. Under `text` (e.g. SQLite, whose untyped
 * columns report BLOB but hold any value), a value in a binary column that
 * isn't a `Uint8Array` is classified by its value, so text shows as text and
 * a number as a number; everything else keeps `columnType`.
 */
export function displayCellType(
  value: unknown,
  columnType: CellType,
  encoding: BinaryStringEncoding = "text",
): CellType {
  if (columnType === "binary" && encoding === "text" && !(value instanceof Uint8Array)) {
    return detectCellType(value);
  }
  return columnType;
}

/**
 * The value to save for an inline edit. Bytes are edited as `\x…` hex (see
 * `toHex`, and editable-cell's `formatValue`), so hex typed into a cell that
 * held bytes is saved as those bytes; saved as text, MySQL would store the
 * characters `\x…` themselves. Any other cell (a string, or NULL) counts as
 * bytes only in a binary column whose strings are bytes (`utf8`: MySQL and
 * MariaDB), never under `text` (every other engine), where `\x…` in SQLite's untyped "BLOB" column would
 * store a blob over text. Anything else is saved as typed.
 */
export function editedCellValue(
  original: unknown,
  columnType: CellType,
  input: string,
  encoding: BinaryStringEncoding = "text",
): unknown {
  const holdsBytes =
    original instanceof Uint8Array || (columnType === "binary" && encoding !== "text");
  if (holdsBytes) {
    const bytes = fromHex(input);
    if (bytes) return bytes;
  }
  return input;
}

export function formatByteSize(value: Uint8Array | string): string {
  // A value that isn't bytes (e.g. a number in a MySQL binary column) is
  // sized by its text's UTF-8.
  const bytes =
    value instanceof Uint8Array ? value.byteLength : new TextEncoder().encode(value).byteLength;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

const BINARY_PREVIEW_BYTES = 16;

/** Hex of the first 16 bytes, with "…" when there are more. */
export function formatBinaryPreview(bytes: Uint8Array): string {
  if (bytes.byteLength <= BINARY_PREVIEW_BYTES) return toHex(bytes);
  return `${toHex(bytes.subarray(0, BINARY_PREVIEW_BYTES))}…`;
}

/**
 * Maps a detected CellType to an appropriate HTML input type attribute.
 */
export function inputTypeForCellType(cellType: CellType): string {
  switch (cellType) {
    case "integer":
    case "float":
      return "number";
    case "date":
      return "date";
    case "datetime":
      return "datetime-local";
    case "time":
      return "time";
    default:
      return "text";
  }
}

/**
 * Classify a database column type string into a category for input purposes.
 * Handles all common SQL type names across PostgreSQL, MySQL, SQLite, MSSQL, and DuckDB.
 */
function classifyColumnType(
  dbType: string,
): "integer" | "float" | "date" | "datetime" | "time" | "text" {
  // Normalise: lowercase, strip parenthesised params, collapse whitespace
  const t = dbType
    .toLowerCase()
    .replace(/\(.*\)/, "")
    .replace(/\s+/g, " ")
    .trim();

  // Order matters: check more specific patterns before general ones.

  // Date + time (check before bare "date" and "time" so "datetime" isn't split)
  if (/timestamp|datetime|smalldatetime|datetimeoffset/.test(t)) return "datetime";

  // Date-only
  if (t === "date") return "date";

  // Time-only
  if (/^time\b/.test(t)) return "time";

  // Integer types — match whole words so "point" doesn't match "int"
  if (
    /\b(int|integer|int2|int4|int8|int16|int32|int64|bigint|smallint|tinyint|mediumint|serial|bigserial|smallserial|hugeint|uinteger|ubigint|usmallint|utinyint|uhugeint)\b/.test(
      t,
    )
  )
    return "integer";

  // Float / decimal types
  if (/\b(real|float|float4|float8|double|numeric|decimal|number|money|smallmoney)\b/.test(t))
    return "float";

  return "text";
}

/**
 * Maps a database column type string (e.g. "INTEGER", "varchar(255)", "timestamp")
 * to an appropriate HTML input type attribute.
 */
export function inputTypeForColumnType(dbType: string): string {
  const kind = classifyColumnType(dbType);
  switch (kind) {
    case "integer":
    case "float":
      return "number";
    case "date":
      return "date";
    case "datetime":
      return "datetime-local";
    case "time":
      return "time";
    default:
      return "text";
  }
}

/**
 * Returns the HTML input `step` attribute for numeric column types.
 * Integer types get "1", float types get "any", others undefined.
 */
export function inputStepForColumnType(dbType: string): string | undefined {
  const kind = classifyColumnType(dbType);
  if (kind === "integer") return "1";
  if (kind === "float") return "any";
  return undefined;
}

export function truncateText(s: string, max: number = 50): string {
  if (s.length <= max) return s;
  return s.slice(0, max) + "...";
}

function stringify(value: unknown): string {
  if (value instanceof Uint8Array || value instanceof SqlDecimal) return cellText(value);
  if (typeof value === "object" && value !== null) return JSON.stringify(value, jsonReplacer);
  return String(value as string | number | bigint | boolean | symbol);
}

/**
 * Returns the display text for a cell value given its detected type.
 * Used for estimating column widths.
 */
export function getFormattedCellText(
  value: unknown,
  columnType: CellType,
  binaryStrings: BinaryStringEncoding = "text",
): string {
  if (value === null || value === undefined) return "NULL";
  const shown = displayCellType(value, columnType, binaryStrings);
  if (shown !== columnType) return getFormattedCellText(value, shown);
  if (columnType === "boolean") return "false"; // checkbox, fixed width
  if (columnType === "integer" || columnType === "float") return formatCellNumber(value);
  if (columnType === "date") return formatDate(stringify(value));
  if (columnType === "datetime") return formatDateTime(stringify(value));
  if (columnType === "time") return formatTime(stringify(value));
  if (columnType === "uuid") return stringify(value);
  if (columnType === "json") return truncateText(stringify(value));
  if (columnType === "array" && Array.isArray(value))
    return (
      value.slice(0, 3).map(cellText).join("  ") +
      (value.length > 3 ? `  +${value.length - 3}` : "")
    );
  if (columnType === "binary") {
    const bytes = binaryCellBytes(value, binaryStrings);
    if (bytes) return `${formatBinaryPreview(bytes)} ${formatByteSize(bytes)}`;
    return formatByteSize(stringify(value));
  }
  if (columnType === "long_text") return truncateText(stringify(value), 80);
  return stringify(value);
}
