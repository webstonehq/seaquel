// What each $lib/sql function does when the module fails: a trap that
// `callWasm` rethrows (after re-instantiating), or a `SeaquelSqlError` for a
// bad call. The functions the UI calls from `$derived`/`$effect` or per
// keystroke keep the TS's "couldn't parse" answer; the read-only check fails
// closed; substitution throws `ParameterSubstitutionError`; the checks on the
// run path throw. `wasm-trap.test.ts` covers the real trap and the recovery.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DatabaseType } from "$lib/types";

const failing = vi.hoisted(() => ({ on: false }));

vi.mock("$lib/wasm", async (importOriginal) => {
  const real = await importOriginal<typeof import("$lib/wasm")>();
  return {
    ...real,
    callWasm: <T>(fn: Parameters<typeof real.callWasm<T>>[0]): T => {
      if (failing.on) throw new WebAssembly.RuntimeError("unreachable");
      return real.callWasm(fn);
    },
  };
});

const sql = await import("./index");

const cases = (engine: DatabaseType) => ({
  splitSqlStatements: () => sql.splitSqlStatements("SELECT 1; SELECT 2", engine),
  getStatementAtOffset: () => sql.getStatementAtOffset("SELECT 1", 0, engine),
  extractParameters: () => sql.extractParameters("SELECT {{p}}"),
  hasParameters: () => sql.hasParameters("SELECT {{p}}"),
  detectQueryType: () => sql.detectQueryType("SELECT 1", engine),
  isSelectQuery: () => sql.isSelectQuery("SELECT 1", engine),
  extractTableFromSelect: () => sql.extractTableFromSelect("SELECT * FROM t", engine),
  validateReadOnlyQuery: () => sql.validateReadOnlyQuery("SELECT 1", engine),
  parseCreateTableSql: () => sql.parseCreateTableSql("CREATE TABLE t (a int)"),
  parseSql: () => sql.parseSql("SELECT * FROM products", { engine }),
  parseQueryForVisualization: () => sql.parseQueryForVisualization("SELECT 1", engine),
  getParseError: () => sql.getParseError("SELECT 1", engine),
  parseVisualQuery: () => sql.parseVisualQuery("SELECT 1", engine),
  resolveColumnSources: () => sql.resolveColumnSources("SELECT a FROM t", engine, []),
});

const fallbacks: Record<keyof ReturnType<typeof cases>, unknown> = {
  splitSqlStatements: [],
  getStatementAtOffset: null,
  extractParameters: [],
  hasParameters: false,
  detectQueryType: "other",
  isSelectQuery: false,
  extractTableFromSelect: null,
  validateReadOnlyQuery: "Only read-only SELECT queries are permitted",
  parseCreateTableSql: null,
  parseSql: null,
  parseQueryForVisualization: null,
  getParseError: "Unable to parse SQL query",
  parseVisualQuery: { visual: null, parseError: "Unable to parse SQL query" },
  resolveColumnSources: undefined,
};

describe.each([
  ["a trap", "postgres" as DatabaseType, true],
  ["a bad call (SeaquelSqlError)", "oracle" as DatabaseType, false],
])("after %s", (_, engine, trap) => {
  let log: ReturnType<typeof vi.spyOn>;
  beforeEach(() => {
    failing.on = trap;
    log = vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    failing.on = false;
    log.mockRestore();
  });

  it.each(Object.keys(fallbacks) as (keyof typeof fallbacks)[])(
    "%s returns the TS's answer for input it can't read, and logs",
    (name) => {
      if (!trap && ["extractParameters", "hasParameters", "parseCreateTableSql"].includes(name)) {
        return; // no engine argument: a bad call can't happen
      }
      expect(cases(engine)[name]()).toEqual(fallbacks[name]);
      expect(log).toHaveBeenCalled();
    },
  );

  it("substituteParameters throws ParameterSubstitutionError", () => {
    expect(() =>
      sql.substituteParameters("SELECT {{p}}", [{ name: "p", value: 1 }], engine),
    ).toThrow(sql.ParameterSubstitutionError);
  });

  it("the run-path checks throw", () => {
    for (const fn of [
      () => sql.isDestructiveStatement("DELETE FROM t", engine),
      () =>
        sql.findDestructiveStatements(
          [{ sql: "DELETE FROM t", index: 0, startOffset: 0, endOffset: 12 }],
          engine,
        ),
      () => sql.hasRowLimit("SELECT 1", engine),
      () => sql.countQuery("SELECT 1", engine),
    ]) {
      expect(fn).toThrow(trap ? WebAssembly.RuntimeError : sql.SeaquelSqlError);
    }
  });
});

describe("with the module working", () => {
  it("parseVisualQuery gives the AST and the parse error from one parse", () => {
    expect(sql.parseVisualQuery("SELECT 1").visual?.type).toBe("select");
    expect(sql.parseVisualQuery("SELECT 1").parseError).toBeNull();
    const bad = sql.parseVisualQuery("SELECT '😀😀' x y", "postgres");
    expect(bad.visual).toBeNull();
    expect(bad.parseError).toBe(sql.getParseError("SELECT '😀😀' x y", "postgres"));
    expect(bad.parseError).toMatch(/Column: 17$/);
  });
});
