// The editor calls splitSqlStatements (the statement count, a $derived) and
// getStatementAtOffset on every keystroke. Through the module, on an M-series
// Mac: a 100 KB script (415 statements) splits in ~0.6 ms and finds the
// statement at the cursor in ~0.5 ms; a 1 MB one (4,051 statements) in ~6 ms
// and ~5 ms. The TS splitter it replaces took ~0.7 ms and ~7 ms. The bounds
// here are ~20x that, to catch a quadratic regression, not to benchmark.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { getStatementAtOffset, splitSqlStatements } from "./index";

const schema = readFileSync(
  fileURLToPath(new URL("../../../e2e/test-databases/postgresql/schema.sql", import.meta.url)),
  "utf8",
);

function script(chars: number): string {
  let sql = "SELECT '東京' AS city; -- 😀\n";
  while (sql.length < chars) sql += schema;
  return sql;
}

function averageMs(fn: () => unknown, runs: number): number {
  fn();
  const start = performance.now();
  for (let i = 0; i < runs; i++) fn();
  return (performance.now() - start) / runs;
}

describe("keystroke-rate calls on a large buffer", () => {
  it.each([
    [100_000, 15],
    [1_000_000, 150],
  ])("a %d-char script splits and finds the cursor's statement in time", (chars, boundMs) => {
    const sql = script(chars);
    const cursor = sql.length >> 1;
    expect(splitSqlStatements(sql, "postgres").length).toBeGreaterThan(chars / 300);
    expect(getStatementAtOffset(sql, cursor, "postgres")).not.toBeNull();
    const runs = chars >= 1_000_000 ? 5 : 20;
    expect(averageMs(() => splitSqlStatements(sql, "postgres"), runs)).toBeLessThan(boundMs);
    expect(averageMs(() => getStatementAtOffset(sql, cursor, "postgres"), runs)).toBeLessThan(
      boundMs,
    );
  });
});
