/**
 * `CoreEditService`: what `db.planEdits`, `db.applyChanges`, `db.tablePage`
 * and `db.duckdbExtension` carry. Edit values reach Core in the cell wire
 * format (`encodeParam`), keyed by the primary key's pairs only.
 */
import { describe, expect, it, vi } from "vitest";
import { SqlDecimal } from "$lib/values";
import { CoreEditService } from "./core-service";
import {
  deleteRowEdit,
  dropObjectEdit,
  insertRowEdit,
  setDefaultEdit,
  truncateTableEdit,
  updateCellEdit,
} from "./intents";
import { scriptedCore } from "./scripted-core";

const table = { schema: "public", name: "t", primaryKeys: ["id", "k"] };

describe("edit intents", () => {
  it("an update tags bytes, bigint and decimal values and sends only the primary-key pairs", () => {
    const row = { k: new SqlDecimal("1.50"), blob: "old", id: 9007199254740993n, other: "x" };
    expect(updateCellEdit(table, row, "blob", new Uint8Array([1, 2, 255]))).toEqual({
      type: "updateCell",
      target: { schema: "public", table: "t" },
      // The primary key's order, not the row's.
      key: [
        ["id", { $sq: "bigint", v: "9007199254740993" }],
        ["k", { $sq: "decimal", v: "1.50" }],
      ],
      column: "blob",
      value: { $sq: "bytes", v: "AQL/" },
    });
  });

  it("a JSON object value is tagged json; undefined goes as null", () => {
    const one = { schema: "s", name: "t", primaryKeys: ["id"] };
    expect(updateCellEdit(one, { id: 1 }, "doc", { a: [1] })).toMatchObject({
      value: { $sq: "json", v: { a: [1] } },
    });
    expect(updateCellEdit(one, { id: 1 }, "c", undefined)).toMatchObject({ value: null });
    // A key column the row lacks binds NULL rather than vanishing.
    expect(deleteRowEdit(one, {})).toMatchObject({ key: [["id", null]] });
  });

  it("set default and delete carry the key only; insert every value in the row's order", () => {
    const one = { schema: "s", name: "t", primaryKeys: ["id"] };
    expect(setDefaultEdit(one, { c: 5, id: new Uint8Array([1, 2]) }, "c")).toEqual({
      type: "setDefault",
      target: { schema: "s", table: "t" },
      key: [["id", { $sq: "bytes", v: "AQI=" }]],
      column: "c",
    });
    expect(deleteRowEdit(one, { id: 1, other: "x" })).toEqual({
      type: "deleteRow",
      target: { schema: "s", table: "t" },
      key: [["id", 1]],
    });
    expect(insertRowEdit(one, { b: 2n, a: null, n: Number.NaN })).toEqual({
      type: "insertRow",
      target: { schema: "s", table: "t" },
      values: [
        ["b", { $sq: "bigint", v: "2" }],
        ["a", null],
        ["n", { $sq: "float", v: "NaN" }],
      ],
    });
  });

  it("the sidebar's intents name the table and, for a drop, what it is", () => {
    expect(truncateTableEdit({ schema: "cat.main", name: "t" })).toEqual({
      type: "truncateTable",
      target: { schema: "cat.main", table: "t" },
    });
    expect(dropObjectEdit({ schema: "public", name: "mv" }, "materializedView")).toEqual({
      type: "dropObject",
      target: { schema: "public", table: "mv" },
      kind: "materializedView",
    });
  });
});

describe("CoreEditService", () => {
  it("sends planEdits and applyChanges as given", async () => {
    const core = scriptedCore();
    const service = new CoreEditService(() => core.client);
    const edit = updateCellEdit(table, { id: 1n, k: "a" }, "c", { x: 1 });
    await service.plan({ connectionId: "pc-1", edits: [edit] });
    await service.apply({
      connectionId: "pc-1",
      changes: [{ type: "edit", id: "c1", edit }],
      confirmed: true,
    });
    expect(core.calls).toEqual([
      { method: "planEdits", params: { connectionId: "pc-1", edits: [edit] } },
      {
        method: "applyChanges",
        params: {
          connectionId: "pc-1",
          changes: [{ type: "edit", id: "c1", edit }],
          confirmed: true,
        },
      },
    ]);
    // The JSON body carries the tags as they are.
    expect(JSON.stringify(core.calls[1].params)).toContain('{"$sq":"bigint","v":"1"}');
    expect(JSON.stringify(core.calls[1].params)).toContain('{"$sq":"json","v":{"x":1}}');
  });

  it("streams tablePage with the caller's signal", () => {
    const stream = vi.fn((..._args: unknown[]) => ({ async *[Symbol.asyncIterator]() {} }));
    const client = { stream } as unknown as ReturnType<typeof scriptedCore>["client"];
    const service = new CoreEditService(() => client);
    const signal = new AbortController().signal;
    const params = {
      connectionId: "pc-1",
      streamId: "s1",
      query: {
        target: { schema: "public", table: "t" },
        filters: [{ column: "id", op: "IN" as const, value: "1, 2" }],
        logic: "AND" as const,
        sort: [],
      },
      page: 2,
      pageSize: 50,
    };
    service.tablePage(params, signal);
    expect(stream.mock.calls).toEqual([
      [{ method: "db", params: { method: "tablePage", params } }, { signal }],
    ]);
  });

  it("decodes an extension listing into row objects; an action answers nothing", async () => {
    const core = scriptedCore({
      duckdbExtension: ({ action }) =>
        action.type === "list"
          ? { columns: ["extension_name", "installed"], rows: [["json", true]] }
          : null,
    });
    const service = new CoreEditService(() => core.client);
    expect(await service.duckdbExtension("pc-1", { type: "list" })).toEqual([
      { extension_name: "json", installed: true },
    ]);
    expect(await service.duckdbExtension("pc-1", { type: "load", name: "json" })).toBeNull();
    expect(core.of("duckdbExtension")).toEqual([
      { connectionId: "pc-1", action: { type: "list" } },
      { connectionId: "pc-1", action: { type: "load", name: "json" } },
    ]);
  });
});
