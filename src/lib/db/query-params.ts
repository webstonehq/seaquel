/**
 * Query parameter utilities.
 * Handles extraction and substitution of {{param_name}} placeholders.
 * @module db/query-params
 */

import { SqlDecimal } from "$lib/values";
import type { QueryParameter, QueryParameterType, ParameterValue, DatabaseType } from "$lib/types";

/**
 * Regex to match {{param_name}} placeholders.
 * Captures the parameter name from within the double braces.
 */
const PARAM_REGEX = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;

/**
 * A parameter value that can't be substituted safely. `substituteParameters`
 * throws it; callers show its message (`errorToast`, or the statement's
 * error result) instead of running the query.
 */
export class ParameterSubstitutionError extends Error {
  override name = "ParameterSubstitutionError";
}

/**
 * Extract parameter names from a query string.
 * Returns unique parameter names in order of first appearance.
 */
export function extractParameters(query: string): string[] {
  const params: string[] = [];
  const seen = new Set<string>();

  let match;
  while ((match = PARAM_REGEX.exec(query)) !== null) {
    const name = match[1];
    if (!seen.has(name)) {
      seen.add(name);
      params.push(name);
    }
  }

  PARAM_REGEX.lastIndex = 0; // Reset regex state
  return params;
}

/**
 * Check if a query contains parameters.
 */
export function hasParameters(query: string): boolean {
  const result = PARAM_REGEX.test(query);
  PARAM_REGEX.lastIndex = 0;
  return result;
}

/**
 * Escape a value for inline substitution in SQL queries.
 * Handles strings, numbers, booleans, null, and dates.
 * @param value The value to escape
 * @param insideString If true, don't wrap strings in quotes (for parameters inside string literals)
 */
export function escapeValueForInline(value: unknown, insideString: boolean = false): string {
  if (value === null || value === undefined) {
    return insideString ? "" : "NULL";
  }

  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      return insideString ? "" : "NULL";
    }
    return String(value);
  }

  if (typeof value === "boolean") {
    return value ? "1" : "0";
  }

  if (value instanceof Date) {
    const isoStr = value.toISOString();
    return insideString ? isoStr.replace(/'/g, "''") : `'${isoStr}'`;
  }

  // String - escape single quotes by doubling them
  // oxlint-disable-next-line typescript/no-base-to-string
  const str = String(value).replace(/'/g, "''");
  return insideString ? str : `'${str}'`;
}

/**
 * @deprecated Use escapeValueForInline instead
 */
export function escapeValueForMssql(value: unknown): string {
  return escapeValueForInline(value, false);
}

/**
 * Check if a position in the query is inside a string literal.
 * Handles single-quoted strings with escaped quotes ('').
 */
function isInsideStringLiteral(query: string, position: number): boolean {
  let insideString = false;
  let i = 0;

  while (i < position && i < query.length) {
    if (query[i] === "'") {
      // Check for escaped quote ('')
      if (i + 1 < query.length && query[i + 1] === "'") {
        i += 2; // Skip escaped quote
        continue;
      }
      insideString = !insideString;
    }
    i++;
  }

  return insideString;
}

/**
 * Substitute parameters in a query.
 * For PostgreSQL/SQLite: replaces {{name}} with $1, $2, etc. and returns bind values.
 * For MySQL/MariaDB: replaces {{name}} with `?`, one bind value per occurrence.
 * For MSSQL/DuckDB: replaces {{name}} with escaped inline values (MSSQL strings as `N'…'`),
 *   skipping comments and quoted names.
 *
 * Special handling for parameters inside string literals:
 * - For PostgreSQL/SQLite: breaks the string to concatenate with parameter: '%{{name}}%' -> '%' || $1 || '%'
 * - For MySQL/MariaDB: `||` is OR there, so the whole literal becomes one `?`
 *   bound to its text with the values filled in: '%{{name}}%' -> ? bound to '%Ann%'
 * - For MSSQL/DuckDB: substitutes the value directly without adding quotes (MSSQL adds
 *   an `N` prefix to a literal that then holds non-ASCII text)
 *
 * @param forceInline If true, always use inline substitution (useful for visualization/parsing)
 */
export function substituteParameters(
  query: string,
  parameterValues: ParameterValue[],
  dbType: DatabaseType,
  forceInline: boolean = false,
): { sql: string; bindValues: unknown[] } {
  const paramMap = new Map(parameterValues.map((p) => [p.name, p.value]));
  if (!forceInline && (dbType === "mysql" || dbType === "mariadb")) {
    return substituteMysqlParameters(query, paramMap);
  }
  if (dbType === "mssql") {
    return substituteMssqlInline(query, paramMap);
  }
  if (dbType === "duckdb") {
    return substituteDuckdbInline(query, paramMap);
  }
  const useInlineSubstitution = forceInline;

  // Forced inline substitution (visualization/parsing) for the other engines
  if (useInlineSubstitution) {
    let result = "";
    let lastIndex = 0;
    let match;

    PARAM_REGEX.lastIndex = 0;
    while ((match = PARAM_REGEX.exec(query)) !== null) {
      const paramName = match[1];
      const value = paramMap.get(paramName);
      const insideString = isInsideStringLiteral(query, match.index);

      result += query.slice(lastIndex, match.index);
      result += escapeValueForInline(value, insideString);
      lastIndex = match.index + match[0].length;
    }
    result += query.slice(lastIndex);

    PARAM_REGEX.lastIndex = 0;
    return { sql: result, bindValues: [] };
  }

  // For databases using parameterized queries (PostgreSQL/SQLite)
  const bindValues: unknown[] = [];
  const usedParams = new Map<string, number>(); // name -> position (1-indexed)

  let result = "";
  let lastIndex = 0;
  let match;

  PARAM_REGEX.lastIndex = 0;
  while ((match = PARAM_REGEX.exec(query)) !== null) {
    const paramName = match[1];
    const insideString = isInsideStringLiteral(query, match.index);

    // Get or assign parameter position
    if (!usedParams.has(paramName)) {
      bindValues.push(paramMap.get(paramName) ?? null);
      usedParams.set(paramName, bindValues.length);
    }
    const paramPosition = usedParams.get(paramName)!;

    result += query.slice(lastIndex, match.index);

    if (insideString) {
      // Parameter is inside a string literal - need to break out and concatenate
      // Find the quote before and after to properly break the string
      // e.g., '%{{name}}%' becomes '%' || $1 || '%'
      result += `' || $${paramPosition} || '`;
    } else {
      result += `$${paramPosition}`;
    }

    lastIndex = match.index + match[0].length;
  }
  result += query.slice(lastIndex);

  PARAM_REGEX.lastIndex = 0;
  return { sql: result, bindValues };
}

/** MySQL's backslash escapes in string literals (the default sql_mode). */
const MYSQL_ESCAPES: Record<string, string> = {
  "0": "\0",
  b: "\b",
  n: "\n",
  r: "\r",
  t: "\t",
  Z: "\x1a",
  // Kept with their backslash, so LIKE patterns still see an escaped % or _.
  "%": "\\%",
  _: "\\_",
};

/** A value as the text MySQL's CONCAT would make of it; `null` for SQL NULL. */
function concatText(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "number" && !Number.isFinite(value)) return null;
  if (typeof value === "boolean") return value ? "1" : "0";
  if (value instanceof Date) return value.toISOString();
  // oxlint-disable-next-line typescript/no-base-to-string
  return String(value);
}

/**
 * MySQL/MariaDB: `?` placeholders, which are positional, so a parameter used
 * twice is bound twice.
 *
 * A string literal holding parameters becomes a single `?` bound to the
 * literal's text with the values filled in, as CONCAT would (NULL if any value
 * is NULL). The `?` also swallows a charset introducer in front of it
 * (`N'…'`, `_utf8mb4'…'` or `_utf8mb4 '…'`) and adjacent literals, which
 * MySQL joins (`'a' 'b'` is `'ab'`), so neither is left next to a placeholder.
 * Parameters in hex and bit literals (`X'{{c}}'`, `B'{{c}}'`) aren't
 * supported: they'd bind as a string next to the `X`/`B`.
 *
 * Comments and backtick identifiers are copied as they are: a `?` there isn't
 * a placeholder, so substituting one would misalign the bind values.
 * Executable comments (`/*! … *\/`, `/*!80000 … *\/`) run on the server and are
 * substituted like the rest of the SQL.
 *
 * Assumes the default sql_mode: backslash escapes in string literals (not
 * `NO_BACKSLASH_ESCAPES`) and `"…"` as a string (not `ANSI_QUOTES`).
 */
function substituteMysqlParameters(
  query: string,
  paramMap: Map<string, unknown>,
): { sql: string; bindValues: unknown[] } {
  const bindValues: unknown[] = [];
  const value = (name: string) => paramMap.get(name) ?? null;

  let sql = "";
  let plain = 0; // start of the text not yet copied
  let i = 0;
  /** Copy `query[plain, end)` with its parameters replaced by `?`. */
  const flushPlain = (end: number) => {
    sql += query.slice(plain, end).replace(PARAM_REGEX, (_m, name: string) => {
      bindValues.push(value(name));
      return "?";
    });
    plain = end;
  };
  /** Copy `query[i, stop)` verbatim. */
  const copyVerbatim = (stop: number) => {
    flushPlain(i);
    sql += query.slice(i, stop);
    i = plain = stop;
  };

  while (i < query.length) {
    const ch = query[i];
    const next = query[i + 1];

    // Comments: `-- ` (dash dash + whitespace), `#`, `/* … */`. An executable
    // comment `/*! … */` is SQL: scan on inside it.
    if ((ch === "-" && next === "-" && /\s/.test(query[i + 2] ?? "")) || ch === "#") {
      const end = query.indexOf("\n", i);
      copyVerbatim(end === -1 ? query.length : end);
      continue;
    }
    if (ch === "/" && next === "*") {
      if (query[i + 2] === "!") {
        i += 3;
        continue;
      }
      const end = query.indexOf("*/", i + 2);
      copyVerbatim(end === -1 ? query.length : end + 2);
      continue;
    }

    // Backtick identifiers (a doubled backtick is an escaped one).
    if (ch === "`") {
      let j = i + 1;
      while (j < query.length) {
        if (query[j] === "`" && query[j + 1] === "`") j += 2;
        else if (query[j] === "`") {
          j++;
          break;
        } else j++;
      }
      copyVerbatim(j);
      continue;
    }

    if (ch === "'" || ch === '"') {
      // A run of adjacent literals, which MySQL joins into one string.
      const texts: string[] = [];
      let end = i;
      let j = i;
      while (j < query.length && (query[j] === "'" || query[j] === '"')) {
        const lit = scanMysqlLiteral(query, j);
        texts.push(lit.text);
        end = lit.end;
        j = end;
        while (j < query.length && /\s/.test(query[j])) j++;
      }
      const text = texts.join("");
      PARAM_REGEX.lastIndex = 0;
      const hasParams = PARAM_REGEX.test(text);
      PARAM_REGEX.lastIndex = 0;
      if (!hasParams) {
        copyVerbatim(end);
        continue;
      }
      // Take a charset introducer (`N`, `_utf8mb4`) right before the quote along.
      // `N` must touch the quote; `_charset` may have whitespace before it.
      const intro = /(?:^|[^\w$])([Nn]|_[A-Za-z0-9_]+\s*)$/.exec(query.slice(plain, i));
      flushPlain(intro ? i - intro[1].length : i);
      let isNull = false;
      const filled = text.replace(PARAM_REGEX, (_m, name: string) => {
        const t = concatText(value(name));
        if (t === null) isNull = true;
        return t ?? "";
      });
      bindValues.push(isNull ? null : filled);
      sql += "?";
      i = plain = end;
      continue;
    }

    i++;
  }
  flushPlain(query.length);
  PARAM_REGEX.lastIndex = 0;
  return { sql, bindValues };
}

/** One MySQL string literal starting at `start` (a quote): its end and its text with escapes resolved. */
function scanMysqlLiteral(query: string, start: number): { end: number; text: string } {
  const quote = query[start];
  let j = start + 1;
  let text = "";
  while (j < query.length) {
    const c = query[j];
    if (c === "\\" && j + 1 < query.length) {
      const e = query[j + 1];
      text += MYSQL_ESCAPES[e] ?? e;
      j += 2;
    } else if (c === quote && query[j + 1] === quote) {
      text += quote;
      j += 2;
    } else if (c === quote) {
      return { end: j + 1, text };
    } else {
      text += c;
      j++;
    }
  }
  return { end: j, text };
}

/**
 * Decimal text without an exponent: `1e5` → `100000`, `-1.5E-3` → `-0.0015`,
 * so it inlines as an exact numeric, not a float. Text that isn't a decimal
 * comes back unchanged.
 */
function plainDecimal(text: string): string {
  const m = /^([+-]?)(\d*)(?:\.(\d*))?[eE]([+-]?\d+)$/.exec(text.trim());
  if (!m || (!m[2] && !m[3])) return text;
  const [, sign, int, frac = ""] = m;
  const exp = Number(m[4]);
  let digits = int + frac;
  let point = int.length + exp; // position of the decimal point in `digits`
  if (point < 0) {
    digits = "0".repeat(-point) + digits;
    point = 0;
  } else if (point > digits.length) {
    digits += "0".repeat(point - digits.length);
  }
  const whole = digits.slice(0, point).replace(/^0+(?=\d)/, "") || "0";
  const rest = digits.slice(point);
  return `${sign === "-" ? "-" : ""}${whole}${rest ? `.${rest}` : ""}`;
}

/**
 * A value as a T-SQL literal: strings as `N'…'`, so text outside the
 * database's code page survives. A negative number is parenthesized, so
 * `5-{{x}}` can't become the comment `5--1`.
 */
function mssqlLiteral(value: unknown): string {
  if (value === null || value === undefined) return "NULL";
  let text: string;
  if (typeof value === "bigint") text = String(value);
  else if (value instanceof SqlDecimal) text = plainDecimal(value.value);
  else if (value instanceof Uint8Array) {
    return `0x${Array.from(value, (b) => b.toString(16).padStart(2, "0")).join("")}`;
  } else if (typeof value === "string" || value instanceof Date) {
    return `N${escapeValueForInline(value)}`;
  } else text = escapeValueForInline(value);
  return text.startsWith("-") ? `(${text})` : text;
}

/** A value's text inside an existing string literal, `'` doubled; NULL is empty, as before. */
function mssqlLiteralText(value: unknown): string {
  if (typeof value === "bigint") return String(value);
  if (value instanceof SqlDecimal) return plainDecimal(value.value);
  return escapeValueForInline(value, true);
}

/** Whether `c` continues a T-SQL word (identifier, keyword, number, `@var`). */
const isTsqlWord = (c: string | undefined) => c !== undefined && /[\w@#$]/.test(c);

/**
 * SQL Server: parameters are inlined, not bound, so they work anywhere a
 * literal does (`TOP {{n}}`, `CREATE VIEW`, a `DEFAULT`) and a user's own
 * `@p1` can't collide with a placeholder. Strings become `N'…'`; a `{{p}}`
 * inside an existing `'…'` literal is filled into it, and a literal that then
 * holds non-ASCII text gets the `N` prefix (a plain `'…'` literal is varchar
 * in the database's code page, where `東京` turns into `??`). Comments
 * (`--`, nested block comments) and quoted names (`[…]`, `"…"`) are copied as they
 * are, so a quote in them can't start a literal.
 */
function substituteMssqlInline(
  query: string,
  paramMap: Map<string, unknown>,
): { sql: string; bindValues: unknown[] } {
  const value = (name: string) => paramMap.get(name);
  let sql = "";
  /** Append `text`, separated by a space when both sides would run into one word. */
  const append = (text: string) => {
    sql += isTsqlWord(sql.at(-1)) && isTsqlWord(text[0]) ? ` ${text}` : text;
  };
  /** Copy SQL text outside literals, comments and names, with its parameters inlined. */
  const plainText = (text: string) => {
    let last = 0;
    for (const m of text.matchAll(PARAM_REGEX)) {
      sql += text.slice(last, m.index);
      append(mssqlLiteral(value(m[1])));
      last = m.index + m[0].length;
    }
    sql += text.slice(last);
  };
  /** The end of a quoted run starting at `start`, where a doubled `close` is escaped. */
  const quotedEnd = (start: number, close: string) => {
    let j = start + 1;
    while (j < query.length) {
      if (query[j] === close && query[j + 1] === close) j += 2;
      else if (query[j] === close) return j + 1;
      else j++;
    }
    return j;
  };

  let plain = 0; // start of the text not yet copied
  let i = 0;
  while (i < query.length) {
    const ch = query[i];
    const next = query[i + 1];
    let end: number;
    if (ch === "-" && next === "-") {
      const nl = query.indexOf("\n", i);
      end = nl === -1 ? query.length : nl;
    } else if (ch === "/" && next === "*") {
      let level = 1;
      end = i + 2;
      while (end < query.length && level > 0) {
        if (query[end] === "/" && query[end + 1] === "*") {
          level++;
          end += 2;
        } else if (query[end] === "*" && query[end + 1] === "/") {
          level--;
          end += 2;
        } else end++;
      }
    } else if (ch === "[" || ch === '"') {
      end = quotedEnd(i, ch === "[" ? "]" : '"');
    } else if (ch === "'") {
      end = quotedEnd(i, "'");
      const literal = query.slice(i, end);
      PARAM_REGEX.lastIndex = 0;
      const hasParams = PARAM_REGEX.test(literal);
      PARAM_REGEX.lastIndex = 0;
      if (hasParams) {
        plainText(query.slice(plain, i));
        const filled = literal.replace(PARAM_REGEX, (_m, name: string) =>
          mssqlLiteralText(value(name)),
        );
        // Already N'…' (the N ends the text before, not a longer word)?
        const prefixed = /(?:^|[^\w@#$])[Nn]$/.test(sql);
        // oxlint-disable-next-line no-control-regex
        if (!prefixed && /[^\x00-\x7f]/.test(filled)) append("N");
        sql += filled;
        plain = i = end;
        continue;
      }
    } else {
      i++;
      continue;
    }
    // A comment, a quoted name or a literal without parameters: copied as is.
    plainText(query.slice(plain, i));
    sql += query.slice(i, end);
    plain = i = end;
  }
  plainText(query.slice(plain));
  PARAM_REGEX.lastIndex = 0;
  return { sql, bindValues: [] };
}

/**
 * A value as a DuckDB literal. A negative number is parenthesized, so
 * `5-{{x}}` can't become the comment `5--1`.
 */
function duckdbLiteral(value: unknown): string {
  let text: string;
  if (typeof value === "bigint") text = String(value);
  else if (value instanceof SqlDecimal) text = plainDecimal(value.value);
  else if (value instanceof Uint8Array) {
    return `from_hex('${Array.from(value, (b) => b.toString(16).padStart(2, "0")).join("")}')`;
  } else text = escapeValueForInline(value);
  return text.startsWith("-") ? `(${text})` : text;
}

/**
 * A value's text inside an existing literal: `'` doubled (NULL is empty, as
 * before), and `\` doubled too in an `E'…'` literal.
 */
function duckdbLiteralText(value: unknown, backslashEscapes: boolean): string {
  let text: string;
  if (typeof value === "bigint") text = String(value);
  else if (value instanceof SqlDecimal) text = plainDecimal(value.value);
  else text = escapeValueForInline(value, true);
  return backslashEscapes ? text.replaceAll("\\", "\\\\") : text;
}

/** Whether `c` continues a DuckDB word (identifier, keyword, number). */
const isDuckdbWord = (c: string | undefined) => c !== undefined && /[\w$]/.test(c);

/**
 * DuckDB (desktop, web and the demo alike): parameters are inlined, not
 * bound. A `{{p}}` inside a `'…'` literal is filled into it; one inside a
 * `$tag$…$tag$` string gets the value's raw text. Comments (`--`, nested
 * block comments) and quoted names (`"…"`, `""` for `"`) are copied as they
 * are, so a quote in them can't start a literal and a `{{p}}` in them stays.
 * A value is spaced off a word it would run into, and a negative number is
 * parenthesized.
 */
function substituteDuckdbInline(
  query: string,
  paramMap: Map<string, unknown>,
): { sql: string; bindValues: unknown[] } {
  const value = (name: string) => paramMap.get(name);
  let sql = "";
  /** Append `text`, separated by a space when both sides would run into one word. */
  const append = (text: string) => {
    sql += isDuckdbWord(sql.at(-1)) && isDuckdbWord(text[0]) ? ` ${text}` : text;
  };
  /** Copy SQL text outside literals, comments and names, with its parameters inlined. */
  const plainText = (text: string) => {
    let last = 0;
    for (const m of text.matchAll(PARAM_REGEX)) {
      sql += text.slice(last, m.index);
      append(duckdbLiteral(value(m[1])));
      last = m.index + m[0].length;
    }
    sql += text.slice(last);
  };
  /** `text` with each parameter replaced by `fn(name)`. */
  const fill = (text: string, fn: (name: string) => string) =>
    text.replace(PARAM_REGEX, (_m, name: string) => fn(name));

  let plain = 0; // start of the text not yet copied
  let i = 0;
  while (i < query.length) {
    const ch = query[i];
    const next = query[i + 1];
    let end: number;
    if (ch === "-" && next === "-") {
      const nl = query.indexOf("\n", i);
      end = nl === -1 ? query.length : nl;
    } else if (ch === "/" && next === "*") {
      let level = 1;
      end = i + 2;
      while (end < query.length && level > 0) {
        if (query[end] === "/" && query[end + 1] === "*") {
          level++;
          end += 2;
        } else if (query[end] === "*" && query[end + 1] === "/") {
          level--;
          end += 2;
        } else end++;
      }
    } else if (ch === '"') {
      end = i + 1;
      while (end < query.length) {
        if (query[end] === '"' && query[end + 1] === '"') end += 2;
        else if (query[end] === '"') {
          end++;
          break;
        } else end++;
      }
    } else if (ch === "$" && !isDuckdbWord(query[i - 1])) {
      // A dollar-quoted string `$tag$…$tag$`; `$1` is a placeholder.
      const tag = /^\$(?:[A-Za-z_]\w*)?\$/.exec(query.slice(i));
      if (!tag) {
        i++;
        continue;
      }
      const close = query.indexOf(tag[0], i + tag[0].length);
      end = close === -1 ? query.length : close + tag[0].length;
      plainText(query.slice(plain, i));
      sql += fill(query.slice(i, end), (name) => {
        const v = value(name);
        if (v === null || v === undefined) return "";
        if (v instanceof SqlDecimal) return plainDecimal(v.value);
        // oxlint-disable-next-line typescript/no-base-to-string
        const text = v instanceof Date ? v.toISOString() : String(v);
        // The raw text can't hold the closing tag: it would end the string.
        if (text.includes(tag[0])) {
          throw new ParameterSubstitutionError(
            `The value for {{${name}}} contains the dollar-quote tag ${tag[0]}; use a different tag or a '…' string`,
          );
        }
        return text;
      });
      plain = i = end;
      continue;
    } else if (ch === "'") {
      // E'…' takes backslash escapes.
      const escapes = /^[Ee]$/.test(query[i - 1] ?? "") && !isDuckdbWord(query[i - 2]);
      end = i + 1;
      while (end < query.length) {
        if (escapes && query[end] === "\\") end += 2;
        else if (query[end] === "'" && query[end + 1] === "'") end += 2;
        else if (query[end] === "'") {
          end++;
          break;
        } else end++;
      }
      plainText(query.slice(plain, i));
      sql += fill(query.slice(i, end), (name) => duckdbLiteralText(value(name), escapes));
      plain = i = end;
      continue;
    } else {
      i++;
      continue;
    }
    // A comment or a quoted name: copied as is.
    plainText(query.slice(plain, i));
    sql += query.slice(i, end);
    plain = i = end;
  }
  plainText(query.slice(plain));
  PARAM_REGEX.lastIndex = 0;
  return { sql, bindValues: [] };
}

/**
 * Create default parameter definitions from extracted parameter names.
 * All parameters default to 'text' type.
 */
export function createDefaultParameters(paramNames: string[]): QueryParameter[] {
  return paramNames.map((name) => ({
    name,
    type: "text" as const,
    defaultValue: undefined,
    description: undefined,
  }));
}

/**
 * Coerce a string value to the appropriate type based on parameter definition.
 */
export function coerceValue(value: string, type: QueryParameterType): unknown {
  if (value === "" || value === null || value === undefined) {
    return null;
  }

  switch (type) {
    case "number": {
      const num = parseFloat(value);
      return isNaN(num) ? null : num;
    }
    case "boolean":
      return value.toLowerCase() === "true" || value === "1";
    case "date":
    case "datetime":
      // Keep as ISO string for database
      return value;
    case "text":
    default:
      return value;
  }
}
