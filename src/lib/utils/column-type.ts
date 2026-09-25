/**
 * Splits a column type as introspection reports it (`varchar(255)`,
 * `decimal(10,2)`, `nvarchar(max)`) into the table editor's type and its
 * length or precision. The editor rebuilds the type as `type(length)` or
 * `type(precision)` (`build_column_type` in crates/seaquel-engine/src/ddl.rs),
 * so a type splits only when that gives it back unchanged:
 *
 * - one parenthesized group right after the name and nothing after it
 *   (`int(11) unsigned`, `decimal(10,2) zerofill` and `DECIMAL(10,2)[]` keep
 *   their suffix by staying whole);
 * - the group holds a length (a number, or `max` for SQL Server) or a
 *   precision (`p,s`). Anything else (enum values, which may hold `)` or
 *   `,`; `STRUCT(...)`, `MAP(...)` and other nested types) stays whole.
 *
 * A type without parentheses stays as it is: SQL Server reports a length for
 * every type that has one (`nvarchar(100)`, `nvarchar(max)`), so a bare
 * `nvarchar` isn't given one here.
 */
export interface SplitColumnType {
  type: string;
  length?: string;
  precision?: string;
}

/** The type's name: words (`character varying`, `double precision`), no parentheses. */
const HEAD = /^\s*([A-Za-z_][\w$]*(?:\s+[A-Za-z_][\w$]*)*)\s*\(/;
const LENGTH = /^\s*(\d+|max)\s*$/i;
const PRECISION = /^\s*\d+\s*,\s*-?\d+\s*$/;

/**
 * The index of the `)` closing the `(` at `open`, skipping quoted strings
 * (`'…'` with `''`) and quoted names (`"…"`, `` `…` ``), or -1.
 */
function closingParen(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    const ch = s[i];
    if (ch === "'" || ch === '"' || ch === "`") {
      i++;
      while (i < s.length) {
        if (s[i] === ch) {
          if (s[i + 1] === ch) {
            i += 2;
            continue;
          }
          break;
        }
        // MySQL escapes a quote in an enum value with a backslash too.
        if (s[i] === "\\" && ch === "'") i++;
        i++;
      }
      if (i >= s.length) return -1;
    } else if (ch === "(") {
      depth++;
    } else if (ch === ")") {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

export function splitColumnType(columnType: string): SplitColumnType {
  const head = HEAD.exec(columnType);
  if (!head) return { type: columnType };
  const open = head[0].length - 1;
  const close = closingParen(columnType, open);
  if (close === -1 || columnType.slice(close + 1).trim() !== "") return { type: columnType };

  const params = columnType.slice(open + 1, close).trim();
  const type = head[1];
  if (LENGTH.test(params)) return { type, length: params };
  if (PRECISION.test(params)) return { type, precision: params };
  return { type: columnType };
}
