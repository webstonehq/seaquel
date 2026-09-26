// Parity of $lib/sql with the TypeScript it replaces, end to end through the
// seaquel-wasm module: the frozen fixtures in crates/seaquel-sql/tests/fixtures
// (see its README), with bugfixes.json's hand-written cases applied, as the
// Rust parity tests apply them. The Rust tests cover seaquel-sql itself; this
// covers the boundary: UTF-16 offsets, JSON in and out, the wire format for
// values, and the TS halves of the wrapper (slicing statement text, the
// primary-key lookup, the tutorial schema).
//
// Every fixture file runs in full. `statement-at.json` checks every UTF-16
// offset of every input on every engine.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { TUTORIAL_SCHEMA, getTableNames } from "$lib/tutorial/schema";
import type { DatabaseType, SchemaTable } from "$lib/types";
import { decodeCell, encodeParam } from "$lib/values";
import {
  ParameterSubstitutionError,
  countQuery,
  detectQueryType,
  extractParameters,
  extractTableFromSelect,
  getParseError,
  getStatementAtOffset,
  hasParameters,
  hasRowLimit,
  isDestructiveStatement,
  parseCreateTableSql,
  parseQueryForVisualization,
  parseSql,
  resolveColumnSources,
  splitSqlStatements,
  substituteParameters,
  validateReadOnlyQuery,
} from "./index";

const ENGINES: DatabaseType[] = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"];

interface Case {
  name: string;
  input: Record<string, unknown> & { sql: string };
  output: unknown;
}

interface Fixture {
  cases: Case[];
  [key: string]: unknown;
}

function fixture(name: string): Fixture {
  const path = fileURLToPath(
    new URL(`../../../crates/seaquel-sql/tests/fixtures/${name}`, import.meta.url),
  );
  return JSON.parse(readFileSync(path, "utf8")) as Fixture;
}

interface HandCase extends Case {
  kind: string;
  replaces?: string;
}

/** The fixture's cases with bugfixes.json's replacements, plus its standalone cases. */
function withFixes(file: string, kind: string): Case[] {
  const hand = (fixture("bugfixes.json").cases as HandCase[]).filter((c) => c.kind === kind);
  const replaces = new Map<string, HandCase>();
  for (const c of hand) {
    if (!c.replaces) continue;
    const [f, name] = c.replaces.split(": ");
    expect(f).toBe(file);
    replaces.set(name, c);
  }
  const out = fixture(file).cases.map((c) => {
    const r = replaces.get(c.name);
    if (!r) return c;
    expect(r.input.sql).toBe(c.input.sql);
    replaces.delete(c.name);
    return { name: c.name, input: r.input, output: r.output };
  });
  expect([...replaces.keys()], "bugfixes.json replaces a missing case").toEqual([]);
  return [...out, ...hand.filter((c) => !c.replaces)];
}

const engineOf = (c: Case) => (c.input.engine as DatabaseType | undefined) ?? "postgres";

/** Compares every case and reports all mismatches at once. */
interface Result {
  name: string;
  got: unknown;
  want: unknown;
}

function expectAll(what: string, results: Result[]) {
  const failures = results.filter((r) => !isDeepEqual(r.got, r.want));
  expect(
    failures.map(
      (f) => `${f.name}\n  got:  ${JSON.stringify(f.got)}\n  want: ${JSON.stringify(f.want)}`,
    ),
    `${what}: ${failures.length} of ${results.length} differ`,
  ).toEqual([]);
}

/** Structural equality for JSON-like values (a key holding `undefined` counts). */
function isDeepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  return ka.every(
    (k) =>
      Object.hasOwn(b, k) &&
      isDeepEqual((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]),
  );
}

/** A JSON round trip: `undefined` fields dropped, as in the fixtures. */
const plain = (v: unknown): unknown => (v === undefined ? null : JSON.parse(JSON.stringify(v)));

describe("scanner fixtures", () => {
  it("split.json: every statement, per engine", () => {
    const results = fixture("split.json").cases.flatMap((c) =>
      ENGINES.map((e) => ({
        name: `${c.name} [${e}]`,
        got: splitSqlStatements(c.input.sql, e),
        want: (c.output as Record<string, unknown>)[e],
      })),
    );
    expect(results).toHaveLength(392 * 6);
    expectAll("split.json", results);
  });

  it("statement-at.json: every UTF-16 offset, per engine", () => {
    const split = new Map(fixture("split.json").cases.map((c) => [c.name, c.output]));
    let offsets = 0;
    const results = fixture("statement-at.json").cases.flatMap((c) => {
      const sql = c.input.sql;
      return ENGINES.flatMap((e) => {
        const runs = (c.output as Record<string, [number, number, number | null][]>)[e];
        const statements = (split.get(c.name) as Record<string, unknown[]>)[e];
        const wrong: Result[] = [];
        let covered = 0;
        for (const [from, to, index] of runs) {
          for (let offset = from; offset <= to; offset++) {
            covered++;
            const got = getStatementAtOffset(sql, offset, e);
            const want = index === null ? null : statements[index];
            if (!isDeepEqual(got, want)) {
              wrong.push({ name: `${c.name} [${e}] @${offset}`, got, want });
            }
          }
        }
        // The runs cover 0..=length.
        expect(covered, `${c.name} [${e}]`).toBe(sql.length + 1);
        offsets += covered;
        return wrong;
      });
    });
    expect(offsets).toBeGreaterThan(100_000);
    expectAll("statement-at.json", results);
  });

  it("row-limit.json, count-query.json, read-only.json, statements.json, per engine", () => {
    const results = [
      ...fixture("row-limit.json").cases.flatMap((c) =>
        ENGINES.map((e) => ({
          name: `row-limit ${c.name} [${e}]`,
          got: hasRowLimit(c.input.sql, e),
          want: (c.output as Record<string, unknown>)[e],
        })),
      ),
      ...fixture("count-query.json").cases.flatMap((c) =>
        ENGINES.map((e) => ({
          name: `count-query ${c.name} [${e}]`,
          got: countQuery(c.input.sql, e),
          want: (c.output as Record<string, unknown>)[e],
        })),
      ),
      ...fixture("read-only.json").cases.flatMap((c) =>
        ENGINES.map((e) => ({
          name: `read-only ${c.name} [${e}]`,
          got: validateReadOnlyQuery(c.input.sql, e),
          want: (c.output as Record<string, unknown>)[e],
        })),
      ),
      ...fixture("statements.json").cases.flatMap((c) =>
        ENGINES.map((e) => ({
          name: `statements ${c.name} [${e}]`,
          got: {
            queryType: detectQueryType(c.input.sql, e),
            destructive: isDestructiveStatement(c.input.sql, e),
            table: extractTableFromSelect(c.input.sql, e),
          },
          want: (c.output as Record<string, unknown>)[e],
        })),
      ),
    ];
    expect(results).toHaveLength((392 * 3 + 386) * 6);
    expectAll("scanner checks", results);
  });
});

describe("params.json", () => {
  /**
   * A bind value as `Value` sees it: the fixtures' JS `bigint` within
   * ±(2^53−1) and a number past 2^53 are the same `Value::Int`, and come back
   * as a number and a bigint. The Rust test (`as_values` in
   * tests/params_parity.rs) pins the same 8 cases.
   */
  function asValue(v: unknown): unknown {
    if (typeof v === "bigint" && v >= -(2n ** 53n - 1n) && v <= 2n ** 53n - 1n) return Number(v);
    if (typeof v === "number" && Number.isInteger(v) && !Number.isSafeInteger(v)) {
      const b = BigInt(v);
      if (b >= -(2n ** 63n) && b < 2n ** 63n) return b;
    }
    return v;
  }

  it("extracts, and substitutes on every engine, with and without forceInline", () => {
    let deviations = 0;
    const results = fixture("params.json").cases.flatMap((c): Result[] => {
      const sql = c.input.sql;
      if (c.input.kind === "extract") {
        return [
          {
            name: c.name,
            got: { parameters: extractParameters(sql), hasParameters: hasParameters(sql) },
            want: c.output,
          },
        ];
      }
      const values = (c.input.values as { name: string; value: unknown }[]).map((p) => ({
        name: p.name,
        value: decodeCell(structuredClone(p.value)),
      }));
      return ENGINES.map((e) => {
        const want = (c.output as Record<string, { sql?: string; bindValues?: unknown[] }>)[e];
        let got: unknown;
        try {
          const out = substituteParameters(sql, values, e, c.input.forceInline as boolean);
          got = { sql: out.sql, bindValues: out.bindValues.map(encodeParam) };
        } catch (err) {
          expect(err).toBeInstanceOf(ParameterSubstitutionError);
          got = { error: (err as Error).message };
        }
        if (want.bindValues) {
          const binds = want.bindValues.map((v) => encodeParam(asValue(decodeCell(v))));
          if (!isDeepEqual(binds, want.bindValues)) deviations++;
          return { name: `${c.name} [${e}]`, got, want: { ...want, bindValues: binds } };
        }
        return { name: `${c.name} [${e}]`, got, want };
      });
    });
    expect(results).toHaveLength(392 + 246 * 6);
    expectAll("params.json", results);
    expect(deviations).toBe(8);
  });
});

describe("create-table.json", () => {
  it("parses every statement, with a fresh UUID for every id", () => {
    const ids = new Set<string>();
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    const withoutIds = (def: unknown) => {
      if (!def) return def;
      const d = def as Record<string, { id?: string }[]>;
      for (const key of ["columns", "indexes", "foreignKeys"]) {
        for (const item of d[key]) {
          expect(item.id).toMatch(uuid);
          expect(ids.has(item.id!)).toBe(false);
          ids.add(item.id!);
          delete item.id;
        }
      }
      return d;
    };
    const results = fixture("create-table.json").cases.map((c) => ({
      name: c.name,
      got: withoutIds(parseCreateTableSql(c.input.sql)),
      want: c.output,
    }));
    expect(results).toHaveLength(124);
    expectAll("create-table.json", results);
    expect(ids.size).toBeGreaterThan(300);
  });
});

describe("AST fixtures", () => {
  it("the wrapper's tutorial schema and tables are the fixture's", () => {
    const t = fixture("tutorial.json");
    expect(getTableNames()).toEqual(t.tutorialTables);
    expect(
      Object.fromEntries(TUTORIAL_SCHEMA.map((s) => [s.name, s.columns.map((c) => c.name)])),
    ).toEqual(t.tutorialSchema);
  });

  it("tutorial.json: parseSql in PostgreSQL mode", () => {
    // tests/ast_parity.rs lists these four non-tutorial entries, which parse
    // differently in PostgreSQL mode (none is a numbered fix) and pins what
    // they give; here it's enough that they're the only differences.
    const deviations = ["rt:sub-in:mysql", "ed:mssql-offset", "ed:duckdb-basic", "ed:duckdb-list"];
    const results = withFixes("tutorial.json", "tutorial").map((c) => ({
      name: c.name,
      got: plain(
        parseSql(c.input.sql, c.input.validTables === null ? { validTableNames: null } : undefined),
      ),
      want: c.output,
    }));
    expect(results).toHaveLength(172);
    const differ = results.filter((r) => !isDeepEqual(r.got, r.want)).map((r) => r.name);
    expect(differ).toEqual(deviations);
    // The rest match.
    expectAll(
      "tutorial.json",
      results.filter((r) => !deviations.includes(r.name)),
    );
  });

  it("builder.json: parseSql in the connection's dialect (fix 9)", () => {
    const results = withFixes("builder.json", "builder").map((c) => ({
      name: c.name,
      got: plain(
        parseSql(c.input.sql, {
          engine: engineOf(c),
          ...(c.input.validTables === null ? { validTableNames: null } : {}),
        }),
      ),
      want: (c.output as { parsed: unknown }).parsed,
    }));
    expect(results.length).toBeGreaterThanOrEqual(57);
    expectAll("builder.json", results);
  });

  it("visual.json: parseQueryForVisualization (fixes 1–8, 15)", () => {
    const results = withFixes("visual.json", "visual").map((c) => ({
      name: c.name,
      got: plain(parseQueryForVisualization(c.input.sql, engineOf(c))),
      want: c.output,
    }));
    expect(results.length).toBeGreaterThanOrEqual(172);
    expectAll("visual.json", results);
  });

  it("parse-error.json: getParseError gives a message or null", () => {
    const results = withFixes("parse-error.json", "parse-error").map((c) => ({
      name: c.name,
      got: getParseError(c.input.sql, engineOf(c)) !== null,
      want: c.output,
    }));
    expectAll("parse-error.json", results);
  });

  it("column-sources.json: resolveColumnSources, with the TS primary-key lookup", () => {
    const schemas = fixture("column-sources.json").schemas as SchemaTable[];
    const results = withFixes("column-sources.json", "column-sources").map((c) => ({
      name: c.name,
      got: plain(resolveColumnSources(c.input.sql, engineOf(c), schemas)),
      want: c.output,
    }));
    expect(results.length).toBeGreaterThanOrEqual(172);
    expect(results.filter((r) => Array.isArray(r.want)).length).toBeGreaterThan(50);
    expectAll("column-sources.json", results);
  });

  it("acceptance.json: 138 of 140 statements we ship parse in their engine's dialect", () => {
    const results = fixture("acceptance.json").cases.map((c) => ({
      name: c.name,
      got: { parses: getParseError(c.input.sql, engineOf(c)) === null },
      want: c.output,
    }));
    expect(results.filter((r) => (r.want as { parses: boolean }).parses)).toHaveLength(138);
    expectAll("acceptance.json", results);
  });
});
