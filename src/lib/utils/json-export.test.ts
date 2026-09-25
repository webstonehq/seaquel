import { describe, expect, it, vi } from "vitest";

vi.mock("svelte-sonner", () => ({ toast: { success: vi.fn() } }));
vi.mock("$lib/paraglide/messages.js", () => ({
  m: {
    query_row_copied: () => "copied",
    query_cell_copied: () => "",
    query_column_copied: () => "",
  },
}));

import { generateJSON } from "./export-formats";
import { copyCell, copyColumn, copyRowAsJSON } from "./clipboard";
import { SqlDecimal } from "$lib/values";

describe("JSON export and copy with decoded values", () => {
  it("generateJSON writes bigint, bytes and decimals as strings", () => {
    const json = generateJSON(
      ["id", "blob", "n"],
      [[9007199254740993n, new Uint8Array([1, 255]), new SqlDecimal("1.50")]],
    );
    expect(JSON.parse(json)).toEqual([{ id: "9007199254740993", blob: "\\x01ff", n: "1.50" }]);
  });

  it("copyRowAsJSON doesn't throw on a bigint", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    await copyRowAsJSON({ id: 9007199254740993n });
    expect(JSON.parse(writeText.mock.calls[0][0] as string)).toEqual({ id: "9007199254740993" });
    vi.unstubAllGlobals();
  });

  it("copyCell and copyColumn write bytes as hex", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    await copyCell(new Uint8Array([1, 255]));
    await copyColumn("v", ["v"], [[10n], [new SqlDecimal("1.50")], [null], [new Uint8Array([2])]]);
    expect(writeText.mock.calls[0][0]).toBe("\\x01ff");
    expect(writeText.mock.calls[1][0]).toBe("10\n1.50\n\n\\x02");
    vi.unstubAllGlobals();
  });
});
