// Started as a copy of src/lib/engine/sql-scan.test.ts, run against $lib/sql
// (seaquel-sql in WebAssembly). The original was deleted with the TS in phase 2b.
import { describe, expect, it } from "vitest";
import type { DatabaseType } from "$lib/types";
import { countQuery, extractTableFromSelect, hasRowLimit } from "$lib/sql";

// `stripTrailingOrderBy` and `sqlTokens` are internal to seaquel-sql now
// (their unit tests are in crates/seaquel-sql/src/scan.rs). The cases that
// called them check the same thing through the exported functions:
// `countQuery` on SQL Server wraps exactly what `stripTrailingOrderBy` gives.
const wrap = (sql: string) => `SELECT COUNT(*) as total FROM (${sql}) AS count_query`;
const stripTrailingOrderBy = (sql: string, type: DatabaseType) => {
  const counted = countQuery(sql, type);
  expect(counted.startsWith("SELECT COUNT(*) as total FROM (")).toBe(true);
  return counted.slice("SELECT COUNT(*) as total FROM (".length, -") AS count_query".length);
};

const ALL: DatabaseType[] = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"];

describe("hasRowLimit", () => {
  it.each(ALL)("%s: finds a top-level LIMIT or OFFSET", (type) => {
    expect(hasRowLimit("SELECT * FROM t LIMIT 10", type)).toBe(true);
    expect(hasRowLimit("select * from t limit 10 offset 5", type)).toBe(true);
    expect(hasRowLimit("SELECT * FROM t\nLIMIT\n10", type)).toBe(true);
    expect(hasRowLimit("SELECT * FROM t", type)).toBe(false);
  });

  it.each(ALL)("%s: ignores LIMIT in strings, comments, subqueries and after a dot", (type) => {
    expect(hasRowLimit("SELECT 'no limit here' FROM t", type)).toBe(false);
    expect(hasRowLimit("SELECT * FROM t -- LIMIT 10\n", type)).toBe(false);
    expect(hasRowLimit("SELECT * FROM t /* LIMIT 10 */", type)).toBe(false);
    expect(hasRowLimit("SELECT * FROM (SELECT * FROM t LIMIT 5) s", type)).toBe(false);
    expect(hasRowLimit("SELECT t.limit, t.offset FROM t", type)).toBe(false);
    expect(hasRowLimit("SELECT * FROM t WHERE a = 'it''s LIMIT'", type)).toBe(false);
  });

  it("ignores LIMIT in each engine's quoted names", () => {
    expect(hasRowLimit('SELECT "limit" FROM t', "postgres")).toBe(false);
    expect(hasRowLimit('SELECT "a""limit" FROM t', "duckdb")).toBe(false);
    expect(hasRowLimit("SELECT `limit` FROM t", "mysql")).toBe(false);
    expect(hasRowLimit("SELECT `a``limit` FROM t", "mariadb")).toBe(false);
    expect(hasRowLimit('SELECT [limit], `offset`, "top" FROM t', "sqlite")).toBe(false);
    expect(hasRowLimit("SELECT [a]]limit] FROM t", "mssql")).toBe(false);
    expect(hasRowLimit('SELECT "top" FROM t', "mssql")).toBe(false);
  });

  it("follows each engine's string escapes", () => {
    // MySQL: backslash escapes and double-quoted strings.
    expect(hasRowLimit("SELECT 'a\\' LIMIT 1' FROM t", "mysql")).toBe(false);
    expect(hasRowLimit('SELECT "x LIMIT 1" FROM t', "mysql")).toBe(false);
    expect(hasRowLimit("SELECT * FROM t # LIMIT 1\n", "mysql")).toBe(false);
    // `#` starts a comment mid-word too on MySQL; on SQL Server it's a name (`#temp`).
    expect(hasRowLimit("SELECT a#b LIMIT 1\nFROM t", "mysql")).toBe(false);
    expect(hasRowLimit("SELECT a#b\nFROM t LIMIT 1", "mariadb")).toBe(true);
    // Was `sqlTokens(…)`: `x#y FROM t` is `x` and a comment on MySQL; `#tmp` a name on SQL Server.
    expect(extractTableFromSelect("SELECT x#y FROM t", "mysql")).toBeNull();
    expect(extractTableFromSelect("SELECT * FROM #tmp", "mssql")).toEqual({ table: "#tmp" });
    // `--` needs a space after it on MySQL: `5--1` is arithmetic.
    expect(hasRowLimit("SELECT 5--1\nFROM t LIMIT 1", "mysql")).toBe(true);
    // Postgres: E'' escapes, dollar quotes, nested comments.
    expect(hasRowLimit("SELECT E'a\\' LIMIT 1' FROM t", "postgres")).toBe(false);
    expect(hasRowLimit("SELECT $$ LIMIT 1 $$ FROM t", "postgres")).toBe(false);
    expect(hasRowLimit("SELECT $fn$ it's LIMIT 1 $fn$ FROM t", "duckdb")).toBe(false);
    expect(hasRowLimit("SELECT 1 /* a /* LIMIT 1 */ LIMIT 2 */", "postgres")).toBe(false);
    expect(hasRowLimit("SELECT $1::int LIMIT 5", "postgres")).toBe(true);
    // A backslash is an ordinary character in a Postgres/SQL Server string.
    expect(hasRowLimit("SELECT 'a\\' FROM t LIMIT 1", "postgres")).toBe(true);
    expect(hasRowLimit("SELECT 'a\\' FROM t ORDER BY a OFFSET 0 ROWS", "mssql")).toBe(true);
    // SQLite doesn't nest block comments.
    expect(hasRowLimit("SELECT 1 /* a /* b */ LIMIT 2", "sqlite")).toBe(true);
  });

  it("finds SQL Server's TOP and OFFSET … FETCH", () => {
    expect(hasRowLimit("SELECT TOP 10 * FROM t", "mssql")).toBe(true);
    expect(hasRowLimit("SELECT DISTINCT TOP (5) a FROM t", "mssql")).toBe(true);
    expect(
      hasRowLimit("SELECT * FROM t ORDER BY a OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY", "mssql"),
    ).toBe(true);
    expect(hasRowLimit("SELECT * FROM (SELECT TOP 5 * FROM t ORDER BY a) s", "mssql")).toBe(false);
    expect(hasRowLimit("SELECT 'TOP 5' FROM t", "mssql")).toBe(false);
    // TOP is a limit on SQL Server only.
    expect(hasRowLimit("SELECT top FROM t", "postgres")).toBe(false);
  });

  it("finds FETCH FIRST", () => {
    expect(hasRowLimit("SELECT * FROM t FETCH FIRST 5 ROWS ONLY", "postgres")).toBe(true);
  });
});

describe("stripTrailingOrderBy / countQuery (SQL Server)", () => {
  it("strips a top-level ORDER BY", () => {
    expect(stripTrailingOrderBy("SELECT * FROM t ORDER BY a DESC, b", "mssql")).toBe(
      "SELECT * FROM t",
    );
    expect(
      stripTrailingOrderBy("SELECT a FROM t\nUNION ALL SELECT a FROM u\norder by 1", "mssql"),
    ).toBe("SELECT a FROM t\nUNION ALL SELECT a FROM u");
  });

  it("keeps ORDER BY in OVER(), subqueries, strings and names", () => {
    const over = "SELECT ROW_NUMBER() OVER (ORDER BY a) AS rn FROM t";
    expect(stripTrailingOrderBy(over, "mssql")).toBe(over);
    const sub = "SELECT * FROM (SELECT TOP 3 * FROM t ORDER BY a) s WHERE x = 'ORDER BY'";
    expect(stripTrailingOrderBy(sub, "mssql")).toBe(sub);
    expect(stripTrailingOrderBy(`${over} ORDER BY rn`, "mssql")).toBe(over);
    expect(stripTrailingOrderBy("SELECT [order by] FROM t", "mssql")).toBe(
      "SELECT [order by] FROM t",
    );
  });

  it("keeps ORDER BY when TOP or OFFSET goes with it", () => {
    const top = "SELECT TOP 5 * FROM t ORDER BY a";
    expect(stripTrailingOrderBy(top, "mssql")).toBe(top);
    const offset = "SELECT * FROM t ORDER BY a OFFSET 5 ROWS";
    expect(stripTrailingOrderBy(offset, "mssql")).toBe(offset);
  });

  // The strings `row_count_without_trailing_order_by` in
  // crates/seaquel-engine-mssql/tests/smoke.rs runs on SQL Server.
  it("gives the count queries run live on SQL Server", () => {
    const over = "SELECT name, ROW_NUMBER() OVER (ORDER BY name) AS rn FROM sys.types";
    const sub = "SELECT * FROM (SELECT TOP 3 name FROM sys.types ORDER BY name) s";
    const top = "SELECT TOP 5 name FROM sys.types ORDER BY name";
    expect(countQuery(`${over} ORDER BY rn`, "mssql")).toBe(wrap(over));
    expect(countQuery(over, "mssql")).toBe(wrap(over));
    expect(countQuery(`${sub} ORDER BY name DESC`, "mssql")).toBe(wrap(sub));
    expect(countQuery(top, "mssql")).toBe(wrap(top));
  });

  it("strips only on SQL Server", () => {
    expect(countQuery("SELECT * FROM t ORDER BY a", "mssql")).toBe(
      "SELECT COUNT(*) as total FROM (SELECT * FROM t) AS count_query",
    );
    expect(countQuery("SELECT * FROM t ORDER BY a", "postgres")).toBe(
      "SELECT COUNT(*) as total FROM (SELECT * FROM t ORDER BY a) AS count_query",
    );
  });
});

// Was `describe("sqlTokens")`: depth, skipped comments and unclosed quotes,
// through the exported functions.
describe("tokens", () => {
  it("tracks depth and skips comments", () => {
    expect(hasRowLimit("SELECT (a) -- x\n FROM t LIMIT 1", "postgres")).toBe(true);
    expect(hasRowLimit("SELECT (a LIMIT 1) -- x\n FROM t", "postgres")).toBe(false);
    expect(extractTableFromSelect("SELECT (a) -- x\n FROM t", "postgres")).toEqual({ table: "t" });
  });

  it("runs an unclosed quote to the end", () => {
    expect(hasRowLimit("SELECT 'abc LIMIT 1", "mssql")).toBe(false);
    expect(hasRowLimit("SELECT [abc LIMIT 1", "mssql")).toBe(false);
  });
});
