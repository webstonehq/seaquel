import { describe, expect, it } from "vitest";
import { escapeSQLValue, getExportContent, insertTarget } from "./export-formats";
import type { SchemaTable } from "$lib/types";
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
      'INSERT INTO t ("id", "price", "blob") VALUES (9007199254740993, 12.50, \'\\x01ff\'::bytea);',
    );
  });

  it("SQL quotes the columns for the engine and keeps a quoted table as given", () => {
    const row = [[1]];
    expect(getExportContent("sql", ['we"ird'], row, '"s"."t"', "postgres")).toBe(
      'INSERT INTO "s"."t" ("we""ird") VALUES (1);',
    );
    expect(getExportContent("sql", ["a`b"], row, "`db`.`t`", "mysql")).toBe(
      "INSERT INTO `db`.`t` (`a``b`) VALUES (1);",
    );
    expect(getExportContent("sql", ["order"], row, undefined, "mssql")).toBe(
      "INSERT INTO table_name ([order]) VALUES (1);",
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

describe("insertTarget", () => {
  const users = {
    schema: "public",
    name: "users",
    type: "table",
    columns: ["id", "name", "email"].map((name) => ({ name, type: "text", nullable: true })),
    indexes: [],
  } as unknown as SchemaTable;
  const sourceTable = { schema: "public", name: "users", primaryKeys: ["id"] };
  const src = (column: string) => ({ ...sourceTable, table: "users", column });

  it("is the source table when every column is its own", () => {
    expect(insertTarget({ columns: ["id", "name"], sourceTable }, [users])).toEqual({
      schema: "public",
      name: "users",
    });
    expect(
      insertTarget(
        { columns: ["id", "email"], sourceTable, columnSources: [src("id"), src("email")] },
        [users],
      ),
    ).toEqual({ schema: "public", name: "users" });
  });

  it("is undefined for aggregates, expressions, aliases and unknown tables", () => {
    // SELECT id, count(*) … / SELECT id, upper(name) …
    expect(insertTarget({ columns: ["id", "count"], sourceTable }, [users])).toBeUndefined();
    // SELECT name AS email: the name exists, but it isn't that column.
    expect(
      insertTarget(
        { columns: ["id", "email"], sourceTable, columnSources: [src("id"), src("name")] },
        [users],
      ),
    ).toBeUndefined();
    // A JOIN column from another table.
    expect(
      insertTarget(
        {
          columns: ["id", "name"],
          sourceTable,
          columnSources: [src("id"), { ...src("name"), table: "orders" }],
        },
        [users],
      ),
    ).toBeUndefined();
    expect(insertTarget({ columns: ["id"], sourceTable }, [])).toBeUndefined();
    expect(insertTarget({ columns: ["id"] }, [users])).toBeUndefined();
  });
});
