import { describe, expect, it } from "vitest";
import type { DatabaseType } from "$lib/types";
import pgQuote from "../../../crates/seaquel-engine-postgres/tests/fixtures/quote.json";
import mysqlQuote from "../../../crates/seaquel-engine-mysql/tests/fixtures/mysql/quote.json";
import sqliteQuote from "../../../crates/seaquel-engine-sqlite/tests/fixtures/quote.json";
import mssqlQuote from "../../../crates/seaquel-engine-mssql/tests/fixtures/quote.json";
import duckdbQuote from "../../../crates/seaquel-engine-duckdb/tests/fixtures/quote.json";
import {
  duckdbQualifiedTable,
  duckdbQuoteSchema,
  editorQualifiedTable,
  isBareName,
  parseDotted,
  plainQualifiedTable,
  quoteIdent,
  selectPreview,
} from "./qualified-table";

type QuoteFixture = { cases: Array<{ name: string; input: { id: string }; output: string }> };

// The recorded `quote_ident` cases of each Rust dialect (MariaDB shares MySQL's).
const QUOTE_FIXTURES: Array<[DatabaseType, QuoteFixture]> = [
  ["postgres", pgQuote],
  ["mysql", mysqlQuote],
  ["mariadb", mysqlQuote],
  ["sqlite", sqliteQuote],
  ["mssql", mssqlQuote],
  ["duckdb", duckdbQuote],
];

describe("quoteIdent matches each dialect's quote fixture", () => {
  for (const [type, fixture] of QUOTE_FIXTURES) {
    it.each(fixture.cases.map((c) => [c.name, c.input.id, c.output]))(
      `${type}: %s`,
      (_name, id, output) => {
        expect(quoteIdent(type, id)).toBe(output);
      },
    );
  }
});

describe("quoteIdent escapes the quote character (Task 18)", () => {
  it.each<[DatabaseType, string, string]>([
    ["postgres", 'a"b', '"a""b"'],
    ["sqlite", 'a"b', '"a""b"'],
    ["duckdb", 'a"b', '"a""b"'],
    ["mysql", "a`b", "`a``b`"],
    ["mariadb", 'a"b', '`a"b`'],
    ["mssql", "a]b", "[a]]b]"],
    ["mssql", 'a"b', '[a"b]'],
  ])("%s %j", (type, name, quoted) => {
    expect(quoteIdent(type, name)).toBe(quoted);
  });
});

// Mirrors the unit tests of parse_dotted and quote_schema in
// crates/seaquel-engine-duckdb/src/dialect.rs.
describe("parseDotted", () => {
  it.each([
    ["main", ["main"]],
    ["fx_aux.main", ["fx_aux", "main"]],
    ['"fx.a.b"', ["fx.a.b"]],
    ['"fx.we""ird".main', ['fx.we"ird', "main"]],
    ["a.b.c", ["a", "b", "c"]],
    ['seaquel_test.fx_sales."fx.dotted ""t"""', ["seaquel_test", "fx_sales", 'fx.dotted "t"']],
    [".a", ["", "a"]],
  ])("%s", (name, parts) => {
    expect(parseDotted(name)).toEqual(parts);
  });

  it.each(["", "a.", '"open', '"a"b', '"a".', "a..b."])("rejects %j", (name) => {
    expect(parseDotted(name)).toBeNull();
  });
});

describe("duckdbQuoteSchema / duckdbQualifiedTable", () => {
  it.each([
    ["main", '"main"'],
    ['"a.b"', '"a.b"'],
    ["fx_aux.main", '"fx_aux"."main"'],
    ['"fx.we""ird".main', '"fx.we""ird"."main"'],
    ["a.b.c", '"a.b.c"'],
    ['say "hi', '"say ""hi"'],
  ])("%s", (schema, quoted) => {
    expect(duckdbQuoteSchema(schema)).toBe(quoted);
  });

  it("doubles quotes in the table name", () => {
    expect(duckdbQualifiedTable("fx_aux.main", 'it\'s "x"')).toBe('"fx_aux"."main"."it\'s ""x"""');
  });
});

describe("plainQualifiedTable", () => {
  it("quotes the schema as one identifier, escaping both parts", () => {
    expect(plainQualifiedTable("postgres", 'my"schema', 'a"b')).toBe('"my""schema"."a""b"');
    expect(plainQualifiedTable("duckdb", "fx_aux.main", "t")).toBe('"fx_aux.main"."t"');
    expect(plainQualifiedTable("mysql", "db", "we`ird")).toBe("`db`.`we``ird`");
    expect(plainQualifiedTable("mariadb", "db", "t")).toBe("`db`.`t`");
    expect(plainQualifiedTable("mssql", "dbo", "odd]name")).toBe("[dbo].[odd]]name]");
    expect(plainQualifiedTable("sqlite", "main", "t")).toBe('"main"."t"');
  });
});

describe("editorQualifiedTable", () => {
  const plain = (type: DatabaseType) => (s: string, t: string) => plainQualifiedTable(type, s, t);
  const edit = (type: DatabaseType, schema: string, table: string) =>
    editorQualifiedTable(
      type,
      type === "duckdb" ? duckdbQualifiedTable : plain(type),
      schema,
      table,
    );

  it("leaves plain lower-case names bare", () => {
    expect(edit("postgres", "public", "users")).toBe("public.users");
    expect(edit("mssql", "dbo", "order_items")).toBe("dbo.order_items");
    expect(edit("mysql", "shop", "users")).toBe("shop.users");
  });

  it("quotes names that need it, with the engine's quoting", () => {
    expect(edit("postgres", "public", "Users")).toBe('"public"."Users"');
    expect(edit("postgres", "public", "order items")).toBe('"public"."order items"');
    expect(edit("duckdb", "fx_aux.main", "t")).toBe('"fx_aux"."main"."t"');
    expect(edit("duckdb", '"a.b"', "t")).toBe('"a.b"."t"');
  });

  // Each rejected bare by its engine, checked live (Task 18 review).
  it.each<[DatabaseType, string, string, string]>([
    ["sqlite", "main", "transaction", '"main"."transaction"'],
    ["sqlite", "main", "returning", '"main"."returning"'],
    ["mssql", "dbo", "plan", "[dbo].[plan]"],
    ["mssql", "dbo", "file", "[dbo].[file]"],
    ["mssql", "dbo", "percent", "[dbo].[percent]"],
    ["postgres", "window", "t", '"window"."t"'],
    ["postgres", "lateral", "t", '"lateral"."t"'],
    ["duckdb", "window", "t", '"window"."t"'],
    ["mysql", "select", "t", "`select`.`t`"],
  ])("%s: %s.%s", (type, schema, table, quoted) => {
    expect(edit(type, schema, table)).toBe(quoted);
  });

  it("uses each engine's own words", () => {
    // `plan` and `file` are only SQL Server's; `window` isn't SQL Server's.
    expect(edit("postgres", "public", "plan")).toBe("public.plan");
    expect(edit("postgres", "public", "file")).toBe("public.file");
    expect(edit("mssql", "window", "t")).toBe("window.t");
    expect(isBareName("mssql", "transaction")).toBe(false);
    expect(isBareName("sqlite", "transaction")).toBe(false);
    expect(isBareName("postgres", "transaction")).toBe(true);
  });

  it("isBareName", () => {
    expect(isBareName("postgres", "users_2")).toBe(true);
    expect(isBareName("postgres", "_x")).toBe(true);
    for (const name of ["", "2x", "Users", "a-b", "a.b", "user", "select", "café"]) {
      expect(isBareName("postgres", name)).toBe(false);
    }
  });
});

describe("selectPreview", () => {
  it("uses TOP on SQL Server and LIMIT elsewhere", () => {
    expect(selectPreview("mssql", "[dbo].[t]", 100)).toBe("SELECT TOP 100 * FROM [dbo].[t]");
    expect(selectPreview("postgres", '"public"."t"', 100)).toBe(
      'SELECT * FROM "public"."t" LIMIT 100',
    );
  });
});
