// UTF-16 offsets across the seaquel-wasm boundary. Rust works in UTF-8 bytes
// and sqlparser counts columns in chars; everything the wrapper returns must
// be in UTF-16 units, as JS strings and Monaco count them.
import { describe, expect, it } from "vitest";
import type { DatabaseType } from "$lib/types";
import { SqlDecimal } from "$lib/values";
import {
  getParseError,
  getStatementAtOffset,
  parseSql,
  splitSqlStatements,
  substituteParameters,
  type ParsedStatement,
} from "./index";

const ENGINES: DatabaseType[] = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"];
const MIXED = "SELECT '東京' AS city;\nSELECT '😀' AS face;\nSELECT 3";

/**
 * A pure-JS reference for input with no quoted `;` and no comments: split on
 * `;` with the TS splitter's offsets, and the TS rules for the cursor.
 */
function referenceSplit(sql: string): ParsedStatement[] {
  const out: ParsedStatement[] = [];
  let start = 0;
  for (let i = 0; i <= sql.length; i++) {
    if (i < sql.length && sql[i] !== ";") continue;
    const text = sql.slice(start, i);
    if (text.trim()) {
      const end = i < sql.length ? i : sql.length - 1;
      out.push({ sql: text.trim(), index: out.length, startOffset: start, endOffset: end });
    }
    start = i + 1;
  }
  return out;
}

function referenceAt(sql: string, offset: number): ParsedStatement | null {
  const all = referenceSplit(sql);
  if (!all.length) return null;
  const found = all.find((s) => offset >= s.startOffset && offset <= s.endOffset);
  if (found) return found;
  if (offset < all[0].startOffset) return all[0];
  const last = all[all.length - 1];
  if (offset > last.endOffset) return last;
  return all.find((s) => s.startOffset > offset) ?? last;
}

describe("statement offsets are UTF-16", () => {
  it("splits the mixed string as a pure-JS reference does", () => {
    for (const e of ENGINES) {
      expect(splitSqlStatements(MIXED, e)).toEqual(referenceSplit(MIXED));
    }
  });

  it("gives the reference's statement at every UTF-16 offset", () => {
    for (const e of ENGINES) {
      for (let offset = 0; offset <= MIXED.length + 2; offset++) {
        expect(getStatementAtOffset(MIXED, offset, e), `${e} @${offset}`).toEqual(
          referenceAt(MIXED, offset),
        );
      }
    }
  });

  it("puts the cursor at the start of statement 2 in statement 2 (UTF-16 21, byte 25)", () => {
    expect(MIXED.indexOf("SELECT '😀'")).toBe(21);
    expect(new TextEncoder().encode(MIXED.slice(0, 21)).length).toBe(25);
    const s = getStatementAtOffset(MIXED, 21, "postgres");
    expect(s?.index).toBe(1);
    expect(s?.sql).toBe("SELECT '😀' AS face");
  });

  it("slices statement text from the input, so ranges line up past 東京 and 😀", () => {
    const sql = "-- 😀 東京\nSELECT '😀';\n\n  SELECT '東京'  ;SELECT 3 -- ✓";
    for (const e of ENGINES) {
      const stmts = splitSqlStatements(sql, e);
      expect(stmts.map((s) => s.sql)).toEqual([
        "-- 😀 東京\nSELECT '😀'",
        "SELECT '東京'",
        "SELECT 3 -- ✓",
      ]);
      expect(stmts.map((s) => [s.startOffset, s.endOffset])).toEqual([
        [0, sql.indexOf(";")],
        [sql.indexOf(";") + 1, sql.indexOf(";", sql.indexOf("東京'  ;"))],
        [sql.lastIndexOf(";") + 1, sql.length - 1],
      ]);
    }
  });

  it("takes any number as an offset", () => {
    const sql = "SELECT 1; SELECT '😀'";
    for (const [offset, index] of [
      [-1, 0],
      [-Infinity, 0],
      [8.5, 1],
      [sql.length, 1],
      [sql.length + 100, 1],
      [Infinity, 1],
      [NaN, 1],
      [Number.MAX_SAFE_INTEGER, 1],
    ]) {
      expect(getStatementAtOffset(sql, offset, "postgres")?.index, `offset ${offset}`).toBe(index);
    }
    expect(getStatementAtOffset("", 0, "postgres")).toBeNull();
    expect(getStatementAtOffset("  -- only a comment", 5, "postgres")).toBeNull();
  });
});

describe("lone surrogates typed mid-edit", () => {
  const lone = "\uD83D";

  it("keep ranges aligned mid-statement, and the text keeps the surrogate", () => {
    const sql = `SELECT '${lone}x'; SELECT '😀${lone}' AS a; SELECT 3`;
    for (const e of ENGINES) {
      const stmts = splitSqlStatements(sql, e);
      expect(stmts).toEqual(referenceSplit(sql));
      expect(stmts[0].sql).toBe(`SELECT '${lone}x'`);
      expect(stmts[1].sql).toBe(`SELECT '😀${lone}' AS a`);
      for (let offset = 0; offset <= sql.length; offset++) {
        expect(getStatementAtOffset(sql, offset, e), `${e} @${offset}`).toEqual(
          referenceAt(sql, offset),
        );
      }
    }
  });

  it("keep ranges aligned at the end of the buffer", () => {
    for (const sql of [`SELECT 1; SELECT '${lone}`, `SELECT 1; SELECT 2${lone}`, lone, `\uDE00`]) {
      for (const e of ENGINES) {
        const stmts = splitSqlStatements(sql, e);
        const last = stmts[stmts.length - 1];
        expect(last.sql.endsWith(sql.slice(-1)), `${JSON.stringify(sql)} [${e}]`).toBe(true);
        expect(last.endOffset).toBe(sql.length - 1);
        expect(getStatementAtOffset(sql, sql.length, e)).toEqual(last);
      }
    }
  });

  it("a low surrogate before a high one is two lone surrogates, not a pair", () => {
    const sql = "SELECT '\uDE00\uD83D'; SELECT 2";
    const stmts = splitSqlStatements(sql, "postgres");
    expect(stmts[0].sql).toBe("SELECT '\uDE00\uD83D'");
    expect(stmts[1].startOffset).toBe(sql.indexOf(";") + 1);
  });

  it("don't break parameter values or table names sent as JSON", () => {
    const out = substituteParameters(
      "SELECT {{p}}",
      [{ name: "p", value: `a${lone}b` }],
      "postgres",
    );
    // What the database would get anyway: the text encoder's U+FFFD.
    expect(out.bindValues).toEqual(["a�b"]);
    // Both the SQL and the valid table names arrive with U+FFFD, so they match.
    const sql = `SELECT a FROM "t${lone}"`;
    const parsed = parseSql(sql, { validTableNames: [`t${lone}`] });
    expect(parsed?.tables.map((t) => t.tableName)).toEqual(["t\uFFFD"]);
    expect(parseSql(sql, { validTableNames: ["t"] })?.tables).toEqual([]);
  });
});

describe("parse errors count columns in UTF-16", () => {
  it("rewrites sqlparser's char column", () => {
    const sql = "SELECT '😀😀' x y";
    const message = getParseError(sql, "postgres");
    // `y` is char column 15; the editor counts each emoji as two.
    expect(message).toMatch(/at Line: 1, Column: 17$/);
    expect(sql.indexOf("y") + 1).toBe(17);
  });

  it("on the error's own line", () => {
    const sql = "SELECT '😀' AS a,\n  '東京😀' b c";
    const message = getParseError(sql, "postgres");
    const line = sql.split("\n")[1];
    expect(message).toMatch(new RegExp(`at Line: 2, Column: ${line.indexOf("c") + 1}$`));
  });
});

describe("parameter values cross in the wire format", () => {
  it("round-trips a bigint, a SqlDecimal, a Date, NaN and null", () => {
    const date = new Date("2026-09-25T12:34:56.789Z");
    const values = [
      { name: "big", value: 9007199254740993n },
      { name: "dec", value: new SqlDecimal("12.50") },
      { name: "date", value: date },
      { name: "nan", value: NaN },
      { name: "nul", value: null },
    ];
    const sql = "SELECT {{big}}, {{dec}}, {{date}}, {{nan}}, {{nul}}";
    const bound = substituteParameters(sql, values, "postgres");
    expect(bound.sql).toBe("SELECT $1, $2, $3, $4, $5");
    expect(bound.bindValues).toHaveLength(5);
    expect(bound.bindValues[0]).toBe(9007199254740993n);
    expect(bound.bindValues[1]).toBeInstanceOf(SqlDecimal);
    expect(String(bound.bindValues[1])).toBe("12.50");
    // A Date goes in as its ISO text (decision 11).
    expect(bound.bindValues[2]).toBe("2026-09-25T12:34:56.789Z");
    expect(bound.bindValues[3]).toBeNaN();
    expect(bound.bindValues[4]).toBeNull();

    const inline = substituteParameters(sql, values, "postgres", true);
    expect(inline).toEqual({
      sql: "SELECT 9007199254740993, '12.50', '2026-09-25T12:34:56.789Z', NULL, NULL",
      bindValues: [],
    });
  });
});
