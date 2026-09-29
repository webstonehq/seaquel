/**
 * Edit intents (phase 5c, Decision 1) from the grid's decoded rows: a table,
 * the key as `[column, value]` pairs in the primary-key list's order, a
 * column and a value, every value in the cell wire format (`encodeParam`:
 * bigint, decimal, bytes, NaN/±inf and JSON objects tagged; `undefined`
 * sent as `null`). Only the primary-key pairs of a row go out, never the
 * rest of it.
 */
import { encodeParam } from "$lib/values";
import type { Edit } from "$lib/types/generated/Edit";
import type { ObjectKind } from "$lib/types/generated/ObjectKind";
import type { TableTarget } from "$lib/types/generated/TableTarget";

/** A table as the grid knows it: its schema, name and primary-key columns. */
export interface EditTable {
  schema: string;
  name: string;
  primaryKeys: string[];
}

/** One cell value for the wire; `undefined` (a column the row lacks) is `null`. */
export function wireValue(value: unknown): unknown {
  return value === undefined ? null : encodeParam(value);
}

function target(table: { schema: string; name: string }): TableTarget {
  return { schema: table.schema, table: table.name };
}

/** The row's primary-key pairs, in the primary-key list's order. */
export function keyOf(table: EditTable, row: Record<string, unknown>): Array<[string, unknown]> {
  return table.primaryKeys.map((pk) => [pk, wireValue(row[pk])]);
}

export function updateCellEdit(
  table: EditTable,
  row: Record<string, unknown>,
  column: string,
  value: unknown,
): Edit {
  return {
    type: "updateCell",
    target: target(table),
    key: keyOf(table, row),
    column,
    value: wireValue(value),
  };
}

export function setDefaultEdit(
  table: EditTable,
  row: Record<string, unknown>,
  column: string,
): Edit {
  return { type: "setDefault", target: target(table), key: keyOf(table, row), column };
}

/** Every value, in the row's (the grid's) column order. */
export function insertRowEdit(
  table: { schema: string; name: string },
  values: Record<string, unknown>,
): Edit {
  return {
    type: "insertRow",
    target: target(table),
    values: Object.entries(values).map(([column, value]) => [column, wireValue(value)]),
  };
}

export function deleteRowEdit(table: EditTable, row: Record<string, unknown>): Edit {
  return { type: "deleteRow", target: target(table), key: keyOf(table, row) };
}

export function truncateTableEdit(table: { schema: string; name: string }): Edit {
  return { type: "truncateTable", target: target(table) };
}

export function dropObjectEdit(table: { schema: string; name: string }, kind: ObjectKind): Edit {
  return { type: "dropObject", target: target(table), kind };
}
