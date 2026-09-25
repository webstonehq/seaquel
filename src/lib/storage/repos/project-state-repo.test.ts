import { describe, expect, it } from "vitest";
import { projectStateRepo } from "./project-state-repo";
import type { SqliteDatabase } from "../sqlite-types";
import type { PersistedProjectState } from "$lib/types";
import type { SavedWorkflow } from "$lib/types/workflow";
import { SqlDecimal } from "$lib/values";

/** In-memory stand-in that records the saved canvases and serves them back. */
function fakeDb() {
  const canvases: { id: string; data: string }[] = [];
  const db: SqliteDatabase = {
    execute: () => Promise.resolve(0),
    close: () => Promise.resolve(),
    transaction(statements) {
      for (const s of statements) {
        if (s.sql.startsWith("INSERT INTO saved_canvases")) {
          const [id, , data] = s.params as [string, string, string];
          canvases.push({ id, data });
        }
      }
      return Promise.resolve();
    },
    query<T>(sql: string): Promise<T[]> {
      if (sql.includes("FROM saved_canvases")) return Promise.resolve(canvases as T[]);
      if (sql.includes("FROM tabs")) return Promise.resolve([]);
      return Promise.resolve([{ active_view: "query", tab_order: "[]" }] as T[]);
    },
  };
  return { db, canvases };
}

describe("projectStateRepo saved workflows", () => {
  it("round-trips result rows holding bigint, bytes and decimals", async () => {
    const rows = [
      [9007199254740993n, new Uint8Array([1, 255]), new SqlDecimal("12.50"), { $sq: "user" }, 1],
    ];
    const workflow = {
      id: "wf-1",
      name: "wf",
      nodes: [
        {
          id: "n1",
          type: "result",
          position: { x: 0, y: 0 },
          data: { columns: ["a", "b", "c", "d", "e"], rows },
        },
      ],
      edges: [],
    } as unknown as SavedWorkflow;
    const state = {
      projectId: "p1",
      activeView: "query",
      tabOrder: [],
      queryTabs: [],
      schemaTabs: [],
      explainTabs: [],
      erdTabs: [],
      savedWorkflows: [workflow],
    } as unknown as PersistedProjectState;

    const { db, canvases } = fakeDb();
    await projectStateRepo.save(db, state);
    expect(canvases).toHaveLength(1);

    const loaded = await projectStateRepo.load(db, "p1");
    const data = loaded?.savedWorkflows?.[0].nodes[0].data as { rows: unknown[][] };
    expect(data.rows).toEqual(rows);
    expect(data.rows[0][0]).toBe(9007199254740993n);
    expect(data.rows[0][2]).toBeInstanceOf(SqlDecimal);
  });
});
