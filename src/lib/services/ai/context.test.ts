import { describe, expect, it } from "vitest";
import { buildDataContext, readOnlyError } from "./context";
import type { DatabaseType } from "$lib/types";
import { SqlDecimal } from "$lib/values";

describe("buildDataContext", () => {
  it("prints decoded values and summarises bytes", () => {
    const ctx = buildDataContext(
      [{ id: 9007199254740993n, price: new SqlDecimal("1.50"), blob: new Uint8Array(3), x: null }],
      ["id", "price", "blob", "x"],
    );
    expect(ctx.split("\n")[3]).toBe("| 9007199254740993 | 1.50 | <3 bytes> | NULL |");
  });
});

describe("readOnlyError (the run_query tool's check)", () => {
  const refusal = "Only read-only SELECT queries are permitted";

  it("lets read-only queries through", () => {
    expect(readOnlyError("SELECT 1", "postgres")).toBeNull();
    expect(readOnlyError("WITH t AS (SELECT 1) SELECT * FROM t", "mysql")).toBeNull();
  });

  // Fix 14: every statement is checked, on tokens.
  it("refuses a second statement that writes", () => {
    expect(readOnlyError("SELECT 1; COPY t TO PROGRAM 'curl x'", "postgres")).toBe(refusal);
    expect(readOnlyError("SELECT 1; DELETE FROM t", "sqlite")).toBe(refusal);
    expect(readOnlyError("SELECT 1 AS k KILL 9999", "mssql")).toBe(refusal);
  });

  it("ignores keywords inside strings", () => {
    expect(readOnlyError("SELECT 'DROP TABLE' AS s", "postgres")).toBeNull();
  });

  it("refuses SELECT … INTO", () => {
    expect(readOnlyError("SELECT * INTO t2 FROM t", "mssql")).toBe(refusal);
  });

  it("fails closed without a known connection type", () => {
    expect(readOnlyError("SELECT 1", undefined)).toBe(refusal);
    expect(readOnlyError("SELECT 1", "oracle" as DatabaseType)).toBe(refusal);
  });
});
