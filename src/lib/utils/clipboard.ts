import { toast } from "svelte-sonner";
import { m } from "$lib/paraglide/messages.js";

/**
 * Copy a single cell value to clipboard.
 */
export async function copyCell(value: unknown): Promise<void> {
  // oxlint-disable-next-line typescript/no-base-to-string
  const text = value === null || value === undefined ? "" : String(value);
  await navigator.clipboard.writeText(text);
  toast.success(m.query_cell_copied());
}

/**
 * Copy an entire row as formatted JSON to clipboard.
 */
export async function copyRowAsJSON(row: Record<string, unknown>): Promise<void> {
  await navigator.clipboard.writeText(JSON.stringify(row, null, 2));
  toast.success(m.query_row_copied());
}

/**
 * Copy all values of a column to clipboard, one per line.
 * Rows are columnar — we take `columns` to resolve the column position once.
 */
export async function copyColumn(
  column: string,
  columns: string[],
  rows: unknown[][],
): Promise<void> {
  const colIdx = columns.indexOf(column);
  if (colIdx === -1) {
    await navigator.clipboard.writeText("");
    toast.success(m.query_column_copied());
    return;
  }
  const values = rows
    .map((row) => row[colIdx])
    // oxlint-disable-next-line typescript/no-base-to-string
    .map((v) => (v === null || v === undefined ? "" : String(v)))
    .join("\n");
  await navigator.clipboard.writeText(values);
  toast.success(m.query_column_copied());
}
