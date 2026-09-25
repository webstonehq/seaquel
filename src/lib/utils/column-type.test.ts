import { describe, expect, it } from "vitest";
import { splitColumnType, type SplitColumnType } from "./column-type";

/** What the table editor rebuilds (`build_column_type`): precision, else length. */
function rebuild({ type, length, precision }: SplitColumnType): string {
  if (precision) return `${type}(${precision})`;
  if (length) return `${type}(${length})`;
  return type;
}

type Case = [string, SplitColumnType];

// Type strings as each engine's Rust introspection reports them (the
// parse-columns and bugfixes fixtures of each engine crate), plus the
// Task 18 cases.
const CASES: Record<string, Case[]> = {
  postgres: [
    ["integer", { type: "integer" }],
    ["character varying", { type: "character varying" }],
    ["timestamp with time zone", { type: "timestamp with time zone" }],
    ["numeric", { type: "numeric" }],
    ["ARRAY", { type: "ARRAY" }],
    ["numeric(10,2)", { type: "numeric", precision: "10,2" }],
    ["timestamp(3) with time zone", { type: "timestamp(3) with time zone" }],
  ],
  mysql: [
    ["int", { type: "int" }],
    ["int(11)", { type: "int", length: "11" }],
    ["varchar(255)", { type: "varchar", length: "255" }],
    ["char(4)", { type: "char", length: "4" }],
    ["decimal(10,2)", { type: "decimal", precision: "10,2" }],
    ["enum('click','view','buy')", { type: "enum('click','view','buy')" }],
    ["int(11) unsigned", { type: "int(11) unsigned" }],
    ["int unsigned", { type: "int unsigned" }],
    ["decimal(10,2) zerofill", { type: "decimal(10,2) zerofill" }],
    ["decimal(10,2) unsigned zerofill", { type: "decimal(10,2) unsigned zerofill" }],
    ["enum('a)','b,c')", { type: "enum('a)','b,c')" }],
    ["enum('it''s','x')", { type: "enum('it''s','x')" }],
    ["set('a','b')", { type: "set('a','b')" }],
    ["enum('1')", { type: "enum('1')" }],
  ],
  sqlite: [
    ["INTEGER", { type: "INTEGER" }],
    ["NUMERIC(10,2)", { type: "NUMERIC", precision: "10,2" }],
    ["VARCHAR (20)", { type: "VARCHAR", length: "20" }],
  ],
  mssql: [
    ["nvarchar(100)", { type: "nvarchar", length: "100" }],
    ["nvarchar(max)", { type: "nvarchar", length: "max" }],
    ["varbinary(max)", { type: "varbinary", length: "max" }],
    ["decimal(12,2)", { type: "decimal", precision: "12,2" }],
    ["numeric(18,0)", { type: "numeric", precision: "18,0" }],
    ["datetime2(7)", { type: "datetime2", length: "7" }],
    ["time(0)", { type: "time", length: "0" }],
    // Bare types have no length to split off (and none is made up).
    ["nvarchar", { type: "nvarchar" }],
    ["uniqueidentifier", { type: "uniqueidentifier" }],
  ],
  duckdb: [
    ["DECIMAL(10,2)", { type: "DECIMAL", precision: "10,2" }],
    ["DECIMAL(10,2)[]", { type: "DECIMAL(10,2)[]" }],
    ["INTEGER[3]", { type: "INTEGER[3]" }],
    ["VARCHAR[][]", { type: "VARCHAR[][]" }],
    ["ENUM('sad', 'ok', 'happy')", { type: "ENUM('sad', 'ok', 'happy')" }],
    ["MAP(VARCHAR, INTEGER)", { type: "MAP(VARCHAR, INTEGER)" }],
    ['STRUCT("name" VARCHAR, age INTEGER)', { type: 'STRUCT("name" VARCHAR, age INTEGER)' }],
    ["STRUCT(a DECIMAL(10,2), b INTEGER)", { type: "STRUCT(a DECIMAL(10,2), b INTEGER)" }],
    ["UNION(num INTEGER, str VARCHAR)", { type: "UNION(num INTEGER, str VARCHAR)" }],
    ["TIME WITH TIME ZONE", { type: "TIME WITH TIME ZONE" }],
  ],
};

describe("splitColumnType", () => {
  for (const [engine, cases] of Object.entries(CASES)) {
    it.each(cases)(`${engine}: %s`, (input, expected) => {
      const split = splitColumnType(input);
      expect(split).toEqual(expected);
      // The editor gives the type back unchanged (modulo spacing).
      expect(rebuild(split).replace(/\s+\(/, "(")).toBe(input.replace(/\s+\(/, "("));
    });
  }

  it("keeps malformed types whole", () => {
    for (const t of ["varchar(", "varchar(10", "(10)", "enum('a", "decimal(10,2))"]) {
      expect(splitColumnType(t)).toEqual({ type: t });
    }
  });
});
