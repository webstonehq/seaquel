import { describe, expect, it } from "vitest";
import { escapeSQLValue, getExportContent } from "./export-formats";
import { SqlDecimal } from "$lib/values";

const columns = ["id", "price", "blob"];
const rows = [[9007199254740993n, new SqlDecimal("12.50"), new Uint8Array([1, 255])]];

describe("export with decoded values", () => {
  it("JSON keeps bigint digits, decimal text and hex bytes", () => {
    // The DuckDB-WASM demo produces bigint rows; JSON.stringify used to throw.
    const json = getExportContent("json", columns, rows);
    expect(JSON.parse(json)).toEqual([{ id: "9007199254740993", price: "12.50", blob: "\\x01ff" }]);
  });

  it("CSV writes bigint and decimal as text, bytes as hex", () => {
    expect(getExportContent("csv", columns, rows)).toBe(
      "id,price,blob\n9007199254740993,12.50,\\x01ff",
    );
  });

  it("SQL leaves numbers unquoted and casts bytes for Postgres", () => {
    expect(getExportContent("sql", columns, rows, "t", "postgres")).toBe(
      "INSERT INTO t (id, price, blob) VALUES (9007199254740993, 12.50, '\\x01ff'::bytea);",
    );
  });

  it("SQL quotes the hex text for other engines", () => {
    expect(escapeSQLValue(new Uint8Array([1, 255]), "duckdb")).toBe("'\\x01ff'");
    expect(escapeSQLValue(new Uint8Array([1, 255]))).toBe("'\\x01ff'");
  });

  it("Markdown shows hex bytes", () => {
    expect(getExportContent("markdown", columns, rows)).toBe(
      "| id | price | blob |\n| --- | --- | --- |\n| 9007199254740993 | 12.50 | \\x01ff |",
    );
  });

  it("is unchanged for ordinary values", () => {
    expect(escapeSQLValue(1)).toBe("1");
    expect(escapeSQLValue(true)).toBe("TRUE");
    expect(escapeSQLValue("it's")).toBe("'it''s'");
    expect(escapeSQLValue(null)).toBe("NULL");
    expect(getExportContent("csv", ["a", "b"], [["x,y", null]])).toBe('a,b\n"x,y",');
  });

  it("quotes non-finite decimals", () => {
    expect(escapeSQLValue(new SqlDecimal("NaN"), "postgres")).toBe("'NaN'::numeric");
    expect(escapeSQLValue(new SqlDecimal("-Infinity"), "postgres")).toBe("'-Infinity'::numeric");
    expect(escapeSQLValue(new SqlDecimal("Infinity"))).toBe("'Infinity'");
    expect(escapeSQLValue(new SqlDecimal("-0.5"))).toBe("-0.5");
  });

  it("writes JSON cells as JSON text", () => {
    const obj = { a: "x|y", n: 1 };
    expect(escapeSQLValue(obj)).toBe(`'{"a":"x|y","n":1}'`);
    expect(escapeSQLValue({ q: "it's" })).toBe(`'{"q":"it''s"}'`);
    expect(getExportContent("csv", ["j"], [[obj]])).toBe('j\n"{""a"":""x|y"",""n"":1}"');
    expect(getExportContent("markdown", ["j"], [[obj]])).toBe(
      '| j |\n| --- |\n| {"a":"x\\|y","n":1} |',
    );
  });
});
