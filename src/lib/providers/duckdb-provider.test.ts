import { describe, expect, it } from "vitest";
import { rowsAffected } from "./duckdb-provider";

/** An Arrow result as DuckDB-WASM returns it, with one column. */
function result(column: string, values: unknown[]) {
  return {
    numRows: values.length,
    schema: { fields: [{ name: column }] },
    getChildAt: () => ({ get: (i: number) => values[i] }),
  };
}

describe("rowsAffected", () => {
  it("reads the Count DuckDB answers an INSERT, UPDATE or DELETE with", () => {
    expect(rowsAffected(result("Count", [0n]))).toBe(0);
    expect(rowsAffected(result("Count", [2n]))).toBe(2);
  });

  it("falls back to the number of result rows otherwise", () => {
    expect(rowsAffected(result("Count", []))).toBe(0);
    expect(rowsAffected(result("n", [5n]))).toBe(1);
  });
});
