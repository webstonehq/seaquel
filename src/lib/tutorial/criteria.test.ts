// The tutorial and the query builder, end to end on the TS side: SQL goes
// through `parseSql` (seaquel-wasm), `applyParsedSqlToState` (the canvas) and
// then either the lesson criteria or `buildSql`. The expected answers are the
// frozen fixtures in crates/seaquel-sql/tests/fixtures, recorded from the
// TypeScript parser this replaced:
//
// - criteria.json: every lesson criterion's verdict on the 91 tutorial entries.
// - builder.json: the SQL the builder regenerates for the 57 round trips,
//   with bugfixes.json's fix 9 cases (SQL Server bracket names) applied.
//
// The criteria and the canvas hooks stay in TS, so this test stays after the
// recorder is gone. A change to the parser that changes a lesson's verdict
// shows up here.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseSql, type ParsedQuery } from "$lib/sql";
import { applyParsedSqlToState } from "$lib/hooks/query-builder-parsed-sql";
import { buildSql, type TableRef } from "$lib/hooks/query-builder-sql";
import { LESSONS } from "$lib/tutorial/lessons";
import { TUTORIAL_SCHEMA } from "$lib/tutorial/schema";
import type { Challenge, DatabaseType, QueryBuilderSnapshot } from "$lib/types";
import { tutorialToQueryBuilder } from "$lib/utils/schema-adapter";

interface Case {
  name: string;
  input: Record<string, unknown> & { sql: string };
  output: unknown;
}

interface HandCase extends Case {
  kind: string;
  replaces?: string;
}

function fixture(name: string): { cases: Case[] } {
  const path = fileURLToPath(
    new URL(`../../../crates/seaquel-sql/tests/fixtures/${name}`, import.meta.url),
  );
  return JSON.parse(readFileSync(path, "utf8")) as { cases: Case[] };
}

const builderSchema = tutorialToQueryBuilder(TUTORIAL_SCHEMA);

const challenges = new Map<string, Challenge>();
for (const lesson of Object.values(LESSONS))
  for (const c of lesson.challenges) challenges.set(c.id, c);

/** The canvas with nothing on it: what the tutorial keeps when the SQL doesn't parse. */
function emptySnapshot(): QueryBuilderSnapshot {
  return {
    tables: [],
    joins: [],
    filters: [],
    groupBy: [],
    having: [],
    orderBy: [],
    limit: null,
    selectAggregates: [],
    subqueries: [],
    ctes: [],
  };
}

/** The schema-qualified table names the builder writes per engine (as recorded). */
const TABLE_REFS: Record<string, TableRef> = {
  postgres: (n) => (n === "customers" ? `"Sales"."customers"` : `public.${n}`),
  mysql: (n) => (n === "customers" ? "`Sales`.`customers`" : `shop.${n}`),
  mssql: (n) => (n === "customers" ? "[Sales].[customers]" : `dbo.${n}`),
  sqlite: (n) => `main.${n}`,
  duckdb: (n) => (n === "customers" ? `"Sales".customers` : `main.${n}`),
};

function regenerate(parsed: ParsedQuery | null, tableRef: string): string | null {
  if (!parsed) return null;
  const s = applyParsedSqlToState(builderSchema, parsed, [], [], []);
  return buildSql(
    s.tables,
    s.joins,
    s.filters,
    s.groupBy,
    s.having,
    s.orderBy,
    s.limit,
    s.selectAggregates,
    s.subqueries,
    s.ctes,
    tableRef === "bare" ? undefined : TABLE_REFS[tableRef],
  );
}

describe("tutorial lesson criteria", () => {
  it("criteria.json: every verdict matches, with the tutorial's parse (PostgreSQL)", () => {
    const cases = fixture("criteria.json").cases;
    expect(cases).toHaveLength(91);
    const failures: string[] = [];
    for (const c of cases) {
      const challenge = challenges.get(c.input.challengeId as string);
      expect(challenge, `no challenge ${String(c.input.challengeId)}`).toBeDefined();
      const parsed = parseSql(c.input.sql);
      const snap = parsed
        ? applyParsedSqlToState(builderSchema, parsed, [], [], [])
        : emptySnapshot();
      const got = Object.fromEntries(
        challenge!.criteria.map((cr) => [cr.id, cr.check(snap, c.input.sql)]),
      );
      if (JSON.stringify(got) !== JSON.stringify(c.output)) {
        failures.push(
          `${c.name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(c.output)}`,
        );
      }
    }
    expect(failures).toEqual([]);
  });
});

describe("query builder round trips", () => {
  it("builder.json: the regenerated SQL matches, parsed in the connection's dialect (fix 9)", () => {
    const hand = (fixture("bugfixes.json").cases as HandCase[]).filter((c) => c.kind === "builder");
    const replaced = new Map(
      hand.filter((c) => c.replaces).map((c) => [c.replaces!.split(": ")[1], c]),
    );
    const cases = fixture("builder.json").cases;
    expect(cases).toHaveLength(57);
    const all = [
      ...cases.map((c) => replaced.get(c.name) ?? c),
      ...hand.filter((c) => !c.replaces),
    ];
    const failures: string[] = [];
    for (const c of all) {
      const parsed = parseSql(c.input.sql, {
        engine: (c.input.engine as DatabaseType | undefined) ?? "postgres",
        ...(c.input.validTables === null ? { validTableNames: null } : {}),
      });
      const got = regenerate(parsed, (c.input.tableRef as string | undefined) ?? "bare");
      const want = (c.output as { sql: string | null }).sql;
      if (got !== want) {
        failures.push(`${c.name}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(want)}`);
      }
    }
    expect(failures).toEqual([]);
  });
});
