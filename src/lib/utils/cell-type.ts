import { SqlDecimal, cellText, jsonReplacer, toHex } from "$lib/values";

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
  /^\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(:\d{2})?(\.\d+)?\s*(Z|[+-]\d{2}:?\d{2}(:\d{2})?)?$/;
const TIME_RE = /^\d{2}:\d{2}(:\d{2})?(\.\d+)?$/;
const LONG_TEXT_THRESHOLD = 100;

export function detectCellType(value: unknown): CellType {
  if (value === null || value === undefined) return "null";
  if (typeof value === "boolean") return "boolean";
  if (typeof value === "number") return Number.isInteger(value) ? "integer" : "float";
  if (typeof value === "bigint") return "integer";
  if (value instanceof SqlDecimal) return "float";
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
  if (/^(bytea|blob|varbinary|binary|image)$/.test(t)) return "binary";
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

export function formatDateTime(s: string): string {
  const d = new Date(normalizeDatetime(s));
  if (isNaN(d.getTime())) return s;
  return dateTimeFormatter.format(d);
}

/**
 * Normalize PostgreSQL-style timestamps like "2026-02-15 21:40:23.568684 +00:00:00"
 * into a format that `new Date()` can parse.
 */
function normalizeDatetime(s: string): string {
  // Replace space separator with T for ISO compatibility
  let normalized = s.replace(/^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2})/, "$1T$2");
  // Remove the seconds part from timezone offset (+00:00:00 → +00:00)
  normalized = normalized.replace(/\s*([+-]\d{2}:\d{2}):\d{2}$/, "$1");
  return normalized;
}

export function formatTime(s: string): string {
  const d = new Date(`1970-01-01T${s}`);
  if (isNaN(d.getTime())) return s;
  return timeFormatter.format(d);
}

export function formatByteSize(value: Uint8Array | string): string {
  // Bytes decoded by the providers know their size; MSSQL still sends base64
  // strings, whose decoded size is estimated from the length.
  const bytes = value instanceof Uint8Array ? value.byteLength : Math.ceil((value.length * 3) / 4);
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
export function getFormattedCellText(value: unknown, columnType: CellType): string {
  if (value === null || value === undefined) return "NULL";
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
    if (value instanceof Uint8Array)
      return `${formatBinaryPreview(value)} ${formatByteSize(value)}`;
    return formatByteSize(stringify(value));
  }
  if (columnType === "long_text") return truncateText(stringify(value), 80);
  return stringify(value);
}
