// Started as a copy of src/lib/db/query-params.test.ts, run against $lib/sql
// (seaquel-sql in WebAssembly). The original was deleted with the TS in phase 2b.
import { describe, expect, it } from "vitest";
import { ParameterSubstitutionError, substituteParameters } from "$lib/sql";
import { SqlDecimal } from "$lib/values";

const values = (v: Record<string, unknown>) =>
  Object.entries(v).map(([name, value]) => ({ name, value }));

describe("substituteParameters: Postgres/SQLite", () => {
  it("numbers placeholders and concatenates inside string literals", () => {
    expect(
      substituteParameters(
        "SELECT * FROM t WHERE a = {{a}} AND b LIKE '%{{b}}%' AND c = {{a}}",
        values({ a: 1, b: "x" }),
        "postgres",
      ),
    ).toEqual({
      sql: "SELECT * FROM t WHERE a = $1 AND b LIKE '%' || $2 || '%' AND c = $1",
      bindValues: [1, "x"],
    });
  });
});

describe.each(["mysql", "mariadb"] as const)("substituteParameters: %s", (dbType) => {
  it("uses ? placeholders, one bind value per occurrence", () => {
    expect(
      substituteParameters(
        "SELECT * FROM t WHERE a = {{a}} AND b = {{b}} OR c = {{a}}",
        values({ a: 1, b: "two" }),
        dbType,
      ),
    ).toEqual({
      sql: "SELECT * FROM t WHERE a = ? AND b = ? OR c = ?",
      bindValues: [1, "two", 1],
    });
  });

  it("binds a whole string literal holding a parameter (no || concatenation)", () => {
    const { sql, bindValues } = substituteParameters(
      "SELECT * FROM t WHERE name LIKE '%{{name}}%' AND id > {{id}}",
      values({ name: "Ann", id: 5 }),
      dbType,
    );
    expect(sql).toBe("SELECT * FROM t WHERE name LIKE ? AND id > ?");
    expect(sql).not.toContain("||");
    expect(bindValues).toEqual(["%Ann%", 5]);
  });

  it("fills several parameters into one literal, and resolves its escapes", () => {
    expect(
      substituteParameters(
        "SELECT '{{a}}-{{b}} it''s \\'x\\' 100\\% a\\\\b'",
        values({ a: "x", b: 2 }),
        dbType,
      ),
    ).toEqual({ sql: "SELECT ?", bindValues: ["x-2 it's 'x' 100\\% a\\b"] });
  });

  it("handles double-quoted string literals too", () => {
    expect(substituteParameters('SELECT "a{{p}}"', values({ p: "b" }), dbType)).toEqual({
      sql: "SELECT ?",
      bindValues: ["ab"],
    });
  });

  it("binds NULL for a literal holding a NULL value, as CONCAT would", () => {
    expect(substituteParameters("SELECT '%{{p}}%'", values({ p: null }), dbType)).toEqual({
      sql: "SELECT ?",
      bindValues: [null],
    });
    expect(substituteParameters("SELECT '{{missing}}'", [], dbType).bindValues).toEqual([null]);
  });

  it("formats booleans in a literal as CONCAT does", () => {
    expect(
      substituteParameters("SELECT 'v={{p}}'", values({ p: true }), dbType).bindValues,
    ).toEqual(["v=1"]);
  });

  it("leaves literals without parameters, comments and backtick names alone", () => {
    expect(
      substituteParameters(
        "SELECT 'it''s {x}', `we'ird` FROM t -- {{a}}\n# {{a}}\n/* {{a}} */ WHERE a = {{a}}",
        values({ a: 7 }),
        dbType,
      ),
    ).toEqual({
      sql: "SELECT 'it''s {x}', `we'ird` FROM t -- {{a}}\n# {{a}}\n/* {{a}} */ WHERE a = ?",
      bindValues: [7],
    });
  });

  it("leaves a parameter inside backticks alone, so binds match placeholders", () => {
    expect(
      substituteParameters(
        "SELECT `{{c}}`, `a``{{c}}` FROM t WHERE a = {{c}}",
        values({ c: 1 }),
        dbType,
      ),
    ).toEqual({ sql: "SELECT `{{c}}`, `a``{{c}}` FROM t WHERE a = ?", bindValues: [1] });
  });

  it.each([
    ["N'{{c}}'", "SELECT ?"],
    ["n'{{c}}'", "SELECT ?"],
    ["_utf8mb4'{{c}}'", "SELECT ?"],
    ["_utf8mb4 '{{c}}'", "SELECT ?"],
    ["_utf8mb4\n  '{{c}}'", "SELECT ?"],
    ['_latin1"{{c}}"', "SELECT ?"],
  ])("puts the charset introducer of %s inside the placeholder", (literal, sql) => {
    expect(substituteParameters(`SELECT ${literal}`, values({ c: "x" }), dbType)).toEqual({
      sql,
      bindValues: ["x"],
    });
  });

  it("takes N only when it touches the quote", () => {
    // `N '…'` isn't an introducer: N is a name (here a column alias).
    expect(substituteParameters("SELECT 1 N '{{c}}'", values({ c: "x" }), dbType)).toEqual({
      sql: "SELECT 1 N ?",
      bindValues: ["x"],
    });
  });

  it("keeps introducers of literals without parameters and identifiers ending in N", () => {
    expect(
      substituteParameters("SELECT N'a', _utf8mb4'b', colN = {{c}}", values({ c: 1 }), dbType),
    ).toEqual({ sql: "SELECT N'a', _utf8mb4'b', colN = ?", bindValues: [1] });
  });

  it("folds adjacent literals into one placeholder", () => {
    expect(
      substituteParameters("SELECT '{{c}}' 'z'\n  \"y\", 1", values({ c: "x" }), dbType),
    ).toEqual({ sql: "SELECT ?, 1", bindValues: ["xzy"] });
    // Only the first literal of a run carries an introducer.
    expect(substituteParameters("SELECT N'a' '{{c}}'", values({ c: "x" }), dbType)).toEqual({
      sql: "SELECT ?",
      bindValues: ["ax"],
    });
  });

  it("substitutes inside executable comments, not ordinary ones", () => {
    expect(
      substituteParameters(
        "SELECT /*! {{a}}, '%{{a}}' */ /*!80000 {{a}} */ /* {{a}} */ 1",
        values({ a: 2 }),
        dbType,
      ),
    ).toEqual({ sql: "SELECT /*! ?, ? */ /*!80000 ? */ /* {{a}} */ 1", bindValues: [2, "%2", 2] });
  });

  it("still inlines when forced (visualisation/parsing)", () => {
    expect(
      substituteParameters("SELECT {{a}}, '%{{b}}%'", values({ a: 1, b: "x" }), dbType, true),
    ).toEqual({ sql: "SELECT 1, '%x%'", bindValues: [] });
  });
});

describe("substituteParameters: mssql", () => {
  const sub = (sql: string, v: Record<string, unknown>) =>
    substituteParameters(sql, values(v), "mssql");

  it("inlines values as literals, strings as N'…', with no bind values", () => {
    expect(
      sub("SELECT * FROM t WHERE a = {{a}} AND b = {{b}} OR c = {{a}} AND d = {{d}}", {
        a: 1,
        b: "東京",
        d: null,
      }),
    ).toEqual({
      sql: "SELECT * FROM t WHERE a = 1 AND b = N'東京' OR c = 1 AND d = NULL",
      bindValues: [],
    });
    expect(
      sub("SELECT {{t}}, {{big}}, {{dec}}", {
        t: true,
        big: 9007199254740993n,
        dec: new SqlDecimal("-0.50"),
      }).sql,
    ).toBe("SELECT 1, 9007199254740993, (-0.50)");
    // Changed from the TS (decision 11 in the phase 2b plan): a bytes value is
    // refused. The TS inlined it as `0x00ff10`; no caller passes one.
    expect(() => sub("SELECT {{bin}}", { bin: new Uint8Array([0, 255, 16]) })).toThrow(
      ParameterSubstitutionError,
    );
  });

  it("doubles a ' in the value", () => {
    expect(sub("SELECT {{p}}, 'x{{p}}'", { p: "it's" }).sql).toBe("SELECT N'it''s', 'xit''s'");
  });

  it("works where a bound parameter can't: TOP, CREATE VIEW", () => {
    expect(sub("SELECT TOP {{n}} * FROM t", { n: 5 }).sql).toBe("SELECT TOP 5 * FROM t");
    expect(sub("CREATE VIEW v AS SELECT * FROM t WHERE name = {{v}}", { v: "a" }).sql).toBe(
      "CREATE VIEW v AS SELECT * FROM t WHERE name = N'a'",
    );
  });

  it("leaves a user's own @p1 alone", () => {
    expect(
      sub("DECLARE @p1 int = {{a}}; SELECT @p1, @P1 WHERE x = {{b}}", { a: 3, b: "y" }),
    ).toEqual({ sql: "DECLARE @p1 int = 3; SELECT @p1, @P1 WHERE x = N'y'", bindValues: [] });
  });

  it("fills a literal and adds N when it then holds non-ASCII text", () => {
    expect(sub("SELECT * FROM t WHERE name LIKE N'%{{p}}%'", { p: "東京" }).sql).toBe(
      "SELECT * FROM t WHERE name LIKE N'%東京%'",
    );
    expect(sub("SELECT * FROM t WHERE name LIKE '%{{p}}%'", { p: "東京" }).sql).toBe(
      "SELECT * FROM t WHERE name LIKE N'%東京%'",
    );
    // ASCII stays a plain literal; NULL fills in as nothing, as before.
    expect(sub("SELECT '%{{p}}%', '{{q}}'", { p: "Ann", q: null }).sql).toBe("SELECT '%Ann%', ''");
    // `WHEN'…'`: the N is a separate word, not `WHENN`.
    expect(sub("SELECT CASE x WHEN'{{a}}' THEN 1 END", { a: "é" }).sql).toBe(
      "SELECT CASE x WHEN N'é' THEN 1 END",
    );
  });

  it("ignores quotes in comments and quoted names", () => {
    const x = "1; DROP TABLE t --";
    expect(sub("-- it's\nSELECT * FROM t WHERE a = {{x}} -- ok'", { x }).sql).toBe(
      "-- it's\nSELECT * FROM t WHERE a = N'1; DROP TABLE t --' -- ok'",
    );
    expect(sub("SELECT [it's], [a]]'{{x}}], {{x}}", { x }).sql).toBe(
      "SELECT [it's], [a]]'{{x}}], N'1; DROP TABLE t --'",
    );
    expect(sub('SELECT "it\'s", "a""\'", {{x}}', { x }).sql).toBe(
      'SELECT "it\'s", "a""\'", N\'1; DROP TABLE t --\'',
    );
    expect(sub("/* don't /* nest'ed */ */ SELECT {{x}} /* {{x}} */", { x: "é" }).sql).toBe(
      "/* don't /* nest'ed */ */ SELECT N'é' /* {{x}} */",
    );
  });

  it("parenthesizes negative numbers, so 5-{{x}} isn't a comment", () => {
    expect(
      sub("SELECT 5-{{a}}, 5-{{b}}, 5-{{c}}, 5-{{d}}", {
        a: -1,
        b: -9007199254740993n,
        c: new SqlDecimal("-0.5"),
        d: 2,
      }).sql,
    ).toBe("SELECT 5-(-1), 5-(-9007199254740993), 5-(-0.5), 5-2");
  });

  it("keeps a word and a literal apart", () => {
    expect(sub("SELECT * FROM t WHERE a LIKE{{p}} AND TOP{{n}}", { p: "a", n: 5 }).sql).toBe(
      "SELECT * FROM t WHERE a LIKE N'a' AND TOP 5",
    );
  });

  it("writes decimals in exponent form as plain digits", () => {
    const dec = (v: string) => sub("SELECT {{d}}, '{{d}}'", { d: new SqlDecimal(v) }).sql;
    expect(dec("1e5")).toBe("SELECT 100000, '100000'");
    expect(dec("-1.5E-3")).toBe("SELECT (-0.0015), '-0.0015'");
    expect(dec("1.25e1")).toBe("SELECT 12.5, '12.5'");
    expect(dec("12.50")).toBe("SELECT 12.50, '12.50'");
  });

  it("inlines the same way when forced (visualization)", () => {
    expect(substituteParameters("SELECT {{a}}", values({ a: "x" }), "mssql", true)).toEqual({
      sql: "SELECT N'x'",
      bindValues: [],
    });
  });
});

describe("substituteParameters: duckdb", () => {
  const sub = (sql: string, v: Record<string, unknown>) =>
    substituteParameters(sql, values(v), "duckdb");

  it("inlines values as literals, with no bind values", () => {
    expect(
      sub("SELECT * FROM t WHERE a = {{a}} AND b = {{b}} AND c = {{c}}", {
        a: 1,
        b: "it's",
        c: null,
      }),
    ).toEqual({
      sql: "SELECT * FROM t WHERE a = 1 AND b = 'it''s' AND c = NULL",
      bindValues: [],
    });
    expect(
      sub("SELECT {{big}}, {{dec}}, {{t}}", {
        big: 9007199254740993n,
        dec: new SqlDecimal("1e-3"),
        t: true,
      }).sql,
    ).toBe("SELECT 9007199254740993, 0.001, 1");
    // Changed from the TS (decision 11 in the phase 2b plan): a bytes value is
    // refused. The TS inlined it as `from_hex('00ff')`; no caller passes one.
    expect(() => sub("SELECT {{bin}}", { bin: new Uint8Array([0, 255]) })).toThrow(
      ParameterSubstitutionError,
    );
  });

  it("fills a parameter inside a string literal", () => {
    expect(sub("SELECT * FROM t WHERE n LIKE '%{{p}}%'", { p: "it's" }).sql).toBe(
      "SELECT * FROM t WHERE n LIKE '%it''s%'",
    );
    // E'…' takes backslash escapes: a backslash in the value is doubled.
    expect(sub("SELECT E'a\\'{{p}}'", { p: "x\\y" }).sql).toBe("SELECT E'a\\'x\\\\y'");
    // A dollar-quoted string gets the raw text.
    expect(sub("SELECT $$it's {{p}}$$", { p: "a'b" }).sql).toBe("SELECT $$it's a'b$$");
  });

  it("skips comments and quoted names, so their quotes open no literal", () => {
    expect(sub("SELECT {{a}} -- it's\n, {{b}}", { a: 1, b: "x" }).sql).toBe(
      "SELECT 1 -- it's\n, 'x'",
    );
    expect(sub("SELECT /* it's /* nested */ {{a}} */ {{a}}", { a: 1 }).sql).toBe(
      "SELECT /* it's /* nested */ {{a}} */ 1",
    );
    expect(sub('SELECT "it\'s ""{{a}}""" FROM t WHERE x = {{a}}', { a: "v" }).sql).toBe(
      'SELECT "it\'s ""{{a}}""" FROM t WHERE x = \'v\'',
    );
    // Before: the apostrophe in the comment opened a fake literal and the
    // value went in unquoted.
    expect(sub("-- don't\nSELECT {{p}}", { p: "x" }).sql).toBe("-- don't\nSELECT 'x'");
  });

  it("refuses a value holding the closing dollar-quote tag", () => {
    // It would end the string: `$$ AS v, 42 AS injected --` added a column.
    expect(() => sub("SELECT $${{p}}$$ AS v", { p: "$$ AS v, 42 AS injected --" })).toThrow(
      ParameterSubstitutionError,
    );
    expect(() => sub("SELECT $${{p}}$$ AS v", { p: "a$$b" })).toThrow(
      "The value for {{p}} contains the dollar-quote tag $$; use a different tag or a '…' string",
    );
    expect(() => sub("SELECT $t${{p}}$t$", { p: "x $t$ y" })).toThrow(
      "The value for {{p}} contains the dollar-quote tag $t$",
    );
  });

  it("takes a value with a lone $ or another tag inside dollar quotes", () => {
    expect(sub("SELECT $${{p}}$$", { p: "costs $5" }).sql).toBe("SELECT $$costs $5$$");
    expect(sub("SELECT $t${{p}}$t$", { p: "a $$ b $u$ c $" }).sql).toBe(
      "SELECT $t$a $$ b $u$ c $$t$",
    );
  });

  it("keeps $1 placeholders apart from dollar quotes", () => {
    expect(sub("SELECT $1, {{a}}", { a: 2 }).sql).toBe("SELECT $1, 2");
  });

  it("parenthesizes negative numbers and spaces values off words", () => {
    expect(sub("SELECT 5-{{x}}", { x: -1 }).sql).toBe("SELECT 5-(-1)");
    expect(sub("SELECT 5-{{x}}", { x: new SqlDecimal("-0.5") }).sql).toBe("SELECT 5-(-0.5)");
    expect(sub("SELECT * FROM t LIMIT{{n}}", { n: 3 }).sql).toBe("SELECT * FROM t LIMIT 3");
  });
});
