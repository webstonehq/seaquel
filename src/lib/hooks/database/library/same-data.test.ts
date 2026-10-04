import { describe, expect, it } from "vitest";
import { keepSame, sameData } from "./same-data";

describe("sameData", () => {
  it("compares primitives, dates, arrays and plain objects by value", () => {
    expect(sameData(1, 1)).toBe(true);
    expect(sameData(1n, 1n)).toBe(true);
    expect(sameData("a", "b")).toBe(false);
    expect(sameData(new Date(5), new Date(5))).toBe(true);
    expect(sameData(new Date(5), new Date(6))).toBe(false);
    expect(sameData([1, { a: [2] }], [1, { a: [2] }])).toBe(true);
    expect(sameData([1, 2], [1, 2, 3])).toBe(false);
    expect(sameData({ a: 1 }, { a: 1, b: 2 })).toBe(false);
    expect(sameData({ a: 1, b: undefined }, { a: 1 })).toBe(true);
    expect(sameData({ a: null }, { a: undefined })).toBe(false);
    expect(sameData(new Date(5), { getTime: 5 })).toBe(false);
  });

  it("keepSame keeps the shown object only when nothing changed", () => {
    const shown = { id: "d1", widgets: [{ id: "w1" }] };
    expect(keepSame(shown, { id: "d1", widgets: [{ id: "w1" }] })).toBe(shown);
    const changed = { id: "d1", widgets: [{ id: "w2" }] };
    expect(keepSame(shown, changed)).toBe(changed);
    expect(keepSame(undefined, changed)).toBe(changed);
  });
});
