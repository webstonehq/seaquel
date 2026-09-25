import { describe, expect, it } from "vitest";
import { buildDataContext } from "./context";
import { SqlDecimal } from "$lib/values";

describe("buildDataContext", () => {
  it("prints decoded values and summarises bytes", () => {
    const ctx = buildDataContext(
      [{ id: 9007199254740993n, price: new SqlDecimal("1.50"), blob: new Uint8Array(3), x: null }],
      ["id", "price", "blob", "x"],
    );
    expect(ctx.split("\n")[3]).toBe("| 9007199254740993 | 1.50 | <3 bytes> | NULL |");
  });
});
