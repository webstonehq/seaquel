/**
 * The schema-cache lookups the view model runs on the table and column
 * references a run's `statementStart` carries (phase 5b, Decision 9).
 */
import { describe, expect, it } from "vitest";
import type { SchemaTable } from "$lib/types";
import {
  columnRefs,
  columnSourcesFromRefs,
  resolveColumnSources,
  sourceTableFromRef,
} from "./index";

const table = (schema: string, name: string, pk: string[] = ["id"]) =>
  ({
    schema,
    name,
    columns: ["id", "name"].map((c) => ({ name: c, isPrimaryKey: pk.includes(c) })),
  }) as unknown as SchemaTable;

const schemas = [table("public", "users"), table("audit", "users"), table("public", "logs", [])];

describe("sourceTableFromRef", () => {
  it("finds a qualified table", () => {
    expect(sourceTableFromRef({ schema: "audit", table: "users" }, schemas)).toEqual({
      schema: "audit",
      name: "users",
      primaryKeys: ["id"],
    });
  });

  it("takes the first cached table of that name without a schema", () => {
    expect(sourceTableFromRef({ table: "users" }, schemas)?.schema).toBe("public");
  });

  it("has nothing for no ref, an unknown table or no primary key", () => {
    expect(sourceTableFromRef(undefined, schemas)).toBeUndefined();
    expect(sourceTableFromRef({ table: "nope" }, schemas)).toBeUndefined();
    expect(sourceTableFromRef({ table: "logs" }, schemas)).toBeUndefined();
  });
});

describe("columnSourcesFromRefs", () => {
  it("resolves each ref and leaves the rest undefined", () => {
    expect(
      columnSourcesFromRefs(
        [{ table: "users", column: "name" }, null, { table: "logs", column: "id" }],
        schemas,
      ),
    ).toEqual([
      { schema: "public", table: "users", primaryKeys: ["id"], column: "name" },
      undefined,
      undefined,
    ]);
    expect(columnSourcesFromRefs(null, schemas)).toBeUndefined();
  });

  it("is resolveColumnSources over columnRefs", () => {
    const sql = "SELECT u.id, u.name, 1 FROM audit.users u";
    expect(columnSourcesFromRefs(columnRefs(sql, "postgres"), schemas)).toEqual(
      resolveColumnSources(sql, "postgres", schemas),
    );
    expect(resolveColumnSources(sql, "postgres", schemas)?.[0]).toEqual({
      schema: "audit",
      table: "users",
      primaryKeys: ["id"],
      column: "id",
    });
  });
});
