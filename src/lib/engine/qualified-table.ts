/**
 * Identifier quoting, run locally like `paginate`: `quoteIdent` quotes one
 * name and `qualifiedTable` a table as the schema tree lists it
 * (`"schema"."table"`). Each engine's `quoteIdent` is byte for byte its Rust
 * dialect's `quote_ident` (checked against tests/fixtures/quote.json of each
 * engine crate): `"…"` with `""` for Postgres, SQLite and DuckDB, backticks
 * with ` `` ` for MySQL and MariaDB, `[…]` with `]]` for SQL Server.
 *
 * DuckDB (Rust, desktop and web) lists an attached catalog's schemas as
 * `catalog.schema`, with a part holding `.` or `"` double-quoted
 * (`"fx.we""ird".main`, or `"a.b"` for a default-catalog schema named `a.b`).
 * `duckdbQualifiedTable` splits that quote-aware and quotes each part, byte
 * for byte what `quote_schema`/`qualified_table` in
 * crates/seaquel-engine-duckdb/src/dialect.rs produce. Every other engine,
 * and DuckDB in the demo (whose TypeScript adapter lists schemas bare),
 * quotes the schema as one identifier (`plainQualifiedTable`).
 */

import type { DatabaseType } from "$lib/types";
import { RESERVED_WORDS } from "./reserved-words";

/**
 * The parts of a dotted name in DuckDB's identifier syntax: `"…"` parts with
 * `""` for `"`, bare parts up to the next dot. `null` when it doesn't parse.
 */
export function parseDotted(name: string): string[] | null {
  const parts: string[] = [];
  let i = 0;
  while (i < name.length) {
    let part = "";
    if (name[i] === '"') {
      i++;
      for (;;) {
        if (i >= name.length) return null;
        if (name[i] === '"') {
          if (name[i + 1] === '"') {
            part += '"';
            i += 2;
            continue;
          }
          i++;
          break;
        }
        part += name[i++];
      }
    } else {
      while (i < name.length && name[i] !== ".") part += name[i++];
    }
    parts.push(part);
    if (i < name.length) {
      if (name[i] !== ".") return null;
      i++;
      if (i === name.length) return null;
    }
  }
  return parts.length > 0 ? parts : null;
}

const doubleQuoted = (s: string) => `"${s.replaceAll('"', '""')}"`;

/** DuckDB's `quote_schema`: one or two parts quoted each, anything else quoted whole. */
export function duckdbQuoteSchema(schema: string): string {
  const parts = parseDotted(schema);
  return parts && parts.length <= 2 ? parts.map(doubleQuoted).join(".") : doubleQuoted(schema);
}

/** DuckDB's `qualified_table`. */
export function duckdbQualifiedTable(schema: string, table: string): string {
  return `${duckdbQuoteSchema(schema)}.${doubleQuoted(table)}`;
}

/** One identifier, quoted the way `type`'s Rust dialect `quote_ident` does. */
export function quoteIdent(type: DatabaseType, name: string): string {
  switch (type) {
    case "mysql":
    case "mariadb":
      return `\`${name.replaceAll("`", "``")}\``;
    case "mssql":
      return `[${name.replaceAll("]", "]]")}]`;
    default:
      return doubleQuoted(name);
  }
}

/** `schema.table` with the schema quoted as one identifier (every engine but Rust DuckDB). */
export function plainQualifiedTable(type: DatabaseType, schema: string, table: string): string {
  return `${quoteIdent(type, schema)}.${quoteIdent(type, table)}`;
}

/**
 * Names the SQL editor can take bare on `type`: lower-case ASCII words (no
 * engine folds or rejects them) that aren't in the engine's reserved words
 * (`./reserved-words`; SQL Server and SQLite reject some even after a dot).
 */
const BARE_NAME = /^[a-z_][a-z0-9_]*$/;

export function isBareName(type: DatabaseType, name: string): boolean {
  return BARE_NAME.test(name) && !RESERVED_WORDS[type].has(name);
}

/**
 * `schema.table` for text the user edits (completions, the query builder):
 * bare when both are plain names on `type`, else quoted the way
 * `qualifiedTable` quotes it (a DuckDB `catalog.schema` is always quoted).
 */
export function editorQualifiedTable(
  type: DatabaseType,
  qualifiedTable: (schema: string, table: string) => string,
  schema: string,
  table: string,
): string {
  if (isBareName(type, schema) && isBareName(type, table)) return `${schema}.${table}`;
  return qualifiedTable(schema, table);
}

/**
 * `SELECT * FROM <from>` limited to `limit` rows, for "Query table" actions:
 * `TOP` on SQL Server, `LIMIT` everywhere else. `from` is already quoted
 * (`qualifiedTable`).
 */
export function selectPreview(type: DatabaseType, from: string, limit: number): string {
  return type === "mssql"
    ? `SELECT TOP ${limit} * FROM ${from}`
    : `SELECT * FROM ${from} LIMIT ${limit}`;
}
