import { describe, expect, it } from "vitest";
import type { CanvasJoin, CanvasTable } from "$lib/types";
import { buildSql } from "./query-builder-sql";

function table(tableName: string, columns: string[] = [], cteId?: string): CanvasTable {
  return {
    id: tableName,
    tableName,
    position: { x: 0, y: 0 },
    selectedColumns: new Set(columns),
    columnAggregates: new Map(),
    ...(cteId ? { cteId } : {}),
  };
}

const join: CanvasJoin = {
  id: "j1",
  sourceTable: "orders",
  sourceColumn: "customer_id",
  targetTable: "Customers",
  targetColumn: "id",
  joinType: "INNER",
};

function sql(tables: CanvasTable[], joins: CanvasJoin[], tableRef?: (name: string) => string) {
  return buildSql(tables, joins, [], [], [], [], null, [], [], [], tableRef);
}

describe("buildSql table names", () => {
  it("names tables bare without a resolver (the tutorial)", () => {
    expect(sql([table("orders", ["id"]), table("Customers")], [join])).toBe(
      "SELECT orders.id\nFROM orders\n  INNER JOIN Customers ON orders.customer_id = Customers.id",
    );
  });

  it("qualifies FROM and JOIN targets, keeping column references bare", () => {
    const names: Record<string, string> = {
      orders: "sales.orders",
      Customers: '"sales"."Customers"',
    };
    expect(sql([table("orders", ["id"]), table("Customers")], [join], (n) => names[n] ?? n)).toBe(
      'SELECT orders.id\nFROM sales.orders\n  INNER JOIN "sales"."Customers" ON orders.customer_id = Customers.id',
    );
  });

  it("never qualifies a CTE reference", () => {
    expect(sql([table("orders", [], "cte-1")], [], (n) => `public.${n}`)).toBe(
      "SELECT orders.*\nFROM orders",
    );
  });
});
