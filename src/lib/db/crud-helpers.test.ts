import { describe, expect, it } from "vitest";
import { getAdapter } from "$lib/db";
import { formatLiteralValue } from "./crud-helpers";
import { SqlDecimal } from "$lib/values";

describe("formatLiteralValue with decoded values", () => {
  it("writes decimals unquoted, non-finite decimals quoted", () => {
    expect(formatLiteralValue(new SqlDecimal("12.50"))).toBe("12.50");
    expect(formatLiteralValue(new SqlDecimal("-0.001"))).toBe("-0.001");
    expect(formatLiteralValue(new SqlDecimal("NaN"))).toBe("'NaN'");
    expect(formatLiteralValue(new SqlDecimal("-Infinity"))).toBe("'-Infinity'");
  });

  it("writes bytes as a DuckDB blob literal", () => {
    expect(formatLiteralValue(new Uint8Array([1, 255]))).toBe("'\\x01\\xFF'::BLOB");
    expect(formatLiteralValue(new Uint8Array([]))).toBe("''::BLOB");
  });

  it("keeps bigint exact and serializes nested bigint in JSON", () => {
    expect(formatLiteralValue(9007199254740993n)).toBe("9007199254740993");
    expect(formatLiteralValue([1n, "it's"])).toBe(`'["1","it''s"]'`);
  });

  it("is unchanged for ordinary values", () => {
    expect(formatLiteralValue(null)).toBe("NULL");
    expect(formatLiteralValue(true)).toBe("TRUE");
    expect(formatLiteralValue(1.5)).toBe("1.5");
    expect(formatLiteralValue("o'k")).toBe("'o''k'");
    expect(formatLiteralValue({ a: 1 })).toBe(`'{"a":1}'`);
  });
});

describe("inline CRUD with byte primary keys", () => {
  it("DuckDB matches the row with a blob literal", () => {
    const { sql } = getAdapter("duckdb").buildDeleteSql("main", "t", ["k"], {
      k: new Uint8Array([0xab]),
    });
    expect(sql).toContain(`= '\\xAB'::BLOB`);
  });

  it("MSSQL uses a 0x binary literal", () => {
    const { sql } = getAdapter("mssql").buildDeleteSql("dbo", "t", ["k"], {
      k: new Uint8Array([0xab, 1]),
    });
    expect(sql).toContain("= 0xAB01");
  });
});
