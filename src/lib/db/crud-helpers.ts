/**
 * CRUD SQL for the demo's DuckDB adapter (`duckdb.ts`): values are escaped and
 * inlined, with no bind values. Demo-only; desktop and web build CRUD SQL in
 * Rust (`crates/seaquel-engine/src/crud.rs`).
 */

import type { SqlWithBindings } from "$lib/types/generated/SqlWithBindings";
import { SqlDecimal, jsonReplacer } from "$lib/values";

export type { SqlWithBindings };

type QuoteIdFn = (id: string) => string;

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

// ─── Builders ────────────────────────────────────────────────────────

function buildInlineWhereClause(
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
): string {
  return primaryKeys.map((pk) => `${qi(pk)} = ${formatLiteralValue(row[pk])}`).join(" AND ");
}

export function buildInlineUpdate(
  schema: string,
  table: string,
  column: string,
  newValue: unknown,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = ${formatLiteralValue(newValue)} WHERE ${whereClause}`;
  return { sql };
}

export function buildInlineSetDefault(
  schema: string,
  table: string,
  column: string,
  primaryKeys: string[],
  row: Record<string, unknown>,
  qi: QuoteIdFn,
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi);
  const sql = `UPDATE ${qi(schema)}.${qi(table)} SET ${qi(column)} = DEFAULT WHERE ${whereClause}`;
  return { sql };
}

export function buildInlineInsert(
  schema: string,
  table: string,
  values: Record<string, unknown>,
  qi: QuoteIdFn,
): SqlWithBindings {
  const columns = Object.keys(values);
  const columnNames = columns.map((c) => qi(c)).join(", ");
  const valuesList = Object.values(values)
    .map((v) => formatLiteralValue(v))
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
): SqlWithBindings {
  const whereClause = buildInlineWhereClause(primaryKeys, row, qi);
  const sql = `DELETE FROM ${qi(schema)}.${qi(table)} WHERE ${whereClause}`;
  return { sql };
}
