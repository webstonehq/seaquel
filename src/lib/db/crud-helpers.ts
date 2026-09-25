/**
 * Pure helper functions for generating CRUD SQL statements.
 * Two strategies: inline (MSSQL/DuckDB) and parameterized (Postgres/MySQL/SQLite).
 */

import type { SqlWithBindings } from "$lib/types/generated/SqlWithBindings";
import { SqlDecimal, jsonReplacer } from "$lib/values";

export type { SqlWithBindings };

/**
 * A callback that returns the SQL type name for a column, or undefined
 * if no CAST is needed (e.g. text types, user-defined types).
 * Used by parameterized adapters (Postgres) to wrap `$N` in `CAST($N AS type)`.
 */
export type CastLookup = (column: string) => string | undefined;

type QuoteIdFn = (id: string) => string;

/**
 * A callback that returns the placeholder string for the Nth bind parameter
 * (1-indexed). Postgres/SQLite use `$N`; MySQL uses `?`.
 */
export type PlaceholderFn = (index: number) => string;

const defaultPlaceholder: PlaceholderFn = (i) => `$${i}`;

/**
 * Serializer for inline-strategy adapters. Defaults to `formatLiteralValue`,
 * which emits `TRUE`/`FALSE` for booleans — correct for Postgres/DuckDB-style
 * SQL. Adapters whose engines don't accept those keywords (e.g. T-SQL BIT,
 * which wants `1`/`0`) pass their own formatter.
 */
export type ValueFormatter = (v: unknown) => string;

// ─── Shared ──────────────────────────────────────────────────────────

export function formatLiteralValue(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "number" || typeof v === "bigint") {
    const s = String(v);
    // Ensure it's actually a numeric literal to prevent injection via crafted toString
    if (/^-?\d+(\.\d+)?$/.test(s)) return s;
    return `'${s.replace(/'/g, "''")}'`;
  }
  if (typeof v === "string") return `'${v.replace(/'/g, "''")}'`;
  if (v instanceof SqlDecimal) {
    if (/^-?\d+(\.\d+)?$/.test(v.value)) return v.value;
    return `'${v.value.replace(/'/g, "''")}'`; // NaN, Infinity, -Infinity
  }
  // DuckDB blob literal: one `\xHH` escape per byte.
  if (v instanceof Uint8Array) return `'${blobEscapes(v)}'::BLOB`;
  // Objects/arrays: serialize as JSON string to avoid [object Object]
  return `'${JSON.stringify(v, jsonReplacer).replace(/'/g, "''")}'`;
}

function blobEscapes(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += `\\x${b.toString(16).padStart(2, "0").toUpperCase()}`;
  return out;
}

/** T-SQL binary literal: `0x0102FF` (unquoted). */
export function formatMssqlBinary(bytes: Uint8Array): string {
  let out = "0x";
  for (const b of bytes) out += b.toString(16).padStart(2, "0").toUpperCase();
  return out;
}

// ─── Inline strategy (MSSQL, DuckDB) ────────────────────────────────
// Values are escaped and embedded directly in SQL. No bind values.

function buildInlineWhereClause(
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  formatValue: ValueFormatter,
): string {
  return primaryKeys.map((pk) => `${qi(pk)} = ${formatValue(row[pk])}`).join(" AND ");
}

export function buildInlineUpdate(
  schema: string,
  table: string,
  column: string,
  newValue: unknown,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  formatValue: ValueFormatter = formatLiteralValue,
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi, formatValue);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = ${formatValue(newValue)} WHERE ${whereClause}`;
  return { sql };
}

export function buildInlineSetDefault(
  schema: string,
  table: string,
  column: string,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  formatValue: ValueFormatter = formatLiteralValue,
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi, formatValue);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = DEFAULT WHERE ${whereClause}`;
  return { sql };
}

export function buildInlineInsert(
  schema: string,
  table: string,
  values: Record<string, unknown>,
  qi: QuoteIdFn,
  formatValue: ValueFormatter = formatLiteralValue,
): SqlWithBindings {
  const columns = Object.keys(values);
  const columnNames = columns.map((c) => qi(c)).join(", ");
  const valuesList = Object.values(values)
    .map((v) => formatValue(v))
    .join(", ");
  const sql = `INSERT INTO ${qi(schema)}.${qi(table)} (${columnNames}) VALUES (${valuesList})`;
  return { sql };
}

export function buildInlineDelete(
  schema: string,
  table: string,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  formatValue: ValueFormatter = formatLiteralValue,
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi, formatValue);
  const sql = `DELETE FROM ${qi(schema)}.${qi(table)} WHERE ${whereClause}`;
  return { sql };
}

// ─── Parameterized strategy (Postgres, MySQL, SQLite) ────────────────
// Values use $N placeholders with a separate bindValues array.

function getCastPlaceholder(
  paramIndex: number,
  column: string,
  placeholderFn: PlaceholderFn,
  castLookup?: CastLookup,
): string {
  const placeholder = placeholderFn(paramIndex);
  if (!castLookup) return placeholder;
  const castType = castLookup(column);
  if (!castType) return placeholder;
  return `CAST(${placeholder} AS ${castType})`;
}

export function buildParamUpdate(
  schema: string,
  table: string,
  column: string,
  newValue: unknown,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  castLookup?: CastLookup,
  placeholderFn: PlaceholderFn = defaultPlaceholder,
): SqlWithBindings {
  const valuePlaceholder = getCastPlaceholder(1, column, placeholderFn, castLookup);
  const whereConditions = primaryKeys.map((pk, i) => `${qi(pk)} = ${placeholderFn(i + 2)}`);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = ${valuePlaceholder} WHERE ${whereConditions.join(" AND ")}`;
  const bindValues = [newValue, ...primaryKeys.map((pk) => row[pk])];
  return { sql, bindValues };
}

export function buildParamSetDefault(
  schema: string,
  table: string,
  column: string,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  placeholderFn: PlaceholderFn = defaultPlaceholder,
): SqlWithBindings {
  const whereConditions = primaryKeys.map((pk, i) => `${qi(pk)} = ${placeholderFn(i + 1)}`);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = DEFAULT WHERE ${whereConditions.join(" AND ")}`;
  const bindValues = primaryKeys.map((pk) => row[pk]);
  return { sql, bindValues };
}

export function buildParamInsert(
  schema: string,
  table: string,
  values: Record<string, unknown>,
  qi: QuoteIdFn,
  castLookup?: CastLookup,
  placeholderFn: PlaceholderFn = defaultPlaceholder,
): SqlWithBindings {
  const columns = Object.keys(values);
  const columnNames = columns.map((c) => qi(c)).join(", ");
  const placeholders = columns
    .map((col, i) => getCastPlaceholder(i + 1, col, placeholderFn, castLookup))
    .join(", ");
  const sql = `INSERT INTO ${qi(schema)}.${qi(table)} (${columnNames}) VALUES (${placeholders})`;
  return { sql, bindValues: Object.values(values) };
}

export function buildParamDelete(
  schema: string,
  table: string,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
  placeholderFn: PlaceholderFn = defaultPlaceholder,
): SqlWithBindings {
  const whereConditions = primaryKeys.map((pk, i) => `${qi(pk)} = ${placeholderFn(i + 1)}`);
  const sql = `DELETE FROM ${qi(schema)}.${qi(table)} WHERE ${whereConditions.join(" AND ")}`;
  const bindValues = primaryKeys.map((pk) => row[pk]);
  return { sql, bindValues };
}
