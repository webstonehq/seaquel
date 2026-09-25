import { describe, expect, it } from "vitest";
import {
  detectCellType,
  detectColumnTypes,
  formatBinaryPreview,
  formatByteSize,
  formatCellNumber,
  formatNumber,
  getFormattedCellText,
} from "./cell-type";
import { SqlDecimal } from "$lib/values";

describe("detectCellType", () => {
  it("classifies decoded values", () => {
    expect(detectCellType(9007199254740993n)).toBe("integer");
    expect(detectCellType(new SqlDecimal("12.50"))).toBe("float");
    expect(detectCellType(new Uint8Array([1, 2]))).toBe("binary");
  });

  it("is unchanged for ordinary values", () => {
    expect(detectCellType(null)).toBe("null");
    expect(detectCellType(true)).toBe("boolean");
    expect(detectCellType(3)).toBe("integer");
    expect(detectCellType(3.5)).toBe("float");
    expect(detectCellType([1])).toBe("array");
    expect(detectCellType({ a: 1 })).toBe("json");
    expect(detectCellType("hello")).toBe("text");
  });

  it("drives column detection", () => {
    const types = detectColumnTypes(
      ["id", "price", "blob"],
      [[1n, new SqlDecimal("1.00"), new Uint8Array([0])]],
    );
    expect(types).toEqual({ id: "integer", price: "float", blob: "binary" });
  });
});

describe("formatCellNumber", () => {
  it("formats bigint exactly through Intl.NumberFormat", () => {
    expect(formatCellNumber(9007199254740993n)).toBe(
      new Intl.NumberFormat(undefined, { maximumFractionDigits: 10 }).format(9007199254740993n),
    );
    // No precision loss: the last digit survives.
    expect(formatCellNumber(9007199254740993n).endsWith("3")).toBe(true);
  });

  it("shows the decimal text as-is", () => {
    expect(formatCellNumber(new SqlDecimal("12.50"))).toBe("12.50");
  });

  it("formats other values like before", () => {
    expect(formatCellNumber(1234.5)).toBe(formatNumber(1234.5));
    expect(formatCellNumber("42")).toBe(formatNumber(42));
  });
});

describe("formatByteSize", () => {
  it("uses byteLength for a Uint8Array", () => {
    expect(formatByteSize(new Uint8Array(10))).toBe("10 B");
    expect(formatByteSize(new Uint8Array(2048))).toBe("2.0 KB");
  });

  it("estimates from a base64 string (MSSQL)", () => {
    expect(formatByteSize("AAAA")).toBe("3 B");
  });
});

describe("formatBinaryPreview", () => {
  it("shows up to 16 bytes as hex", () => {
    expect(formatBinaryPreview(new Uint8Array([1, 255]))).toBe("\\x01ff");
    const long = formatBinaryPreview(new Uint8Array(20).fill(0xab));
    expect(long).toBe(`\\x${"ab".repeat(16)}…`);
  });
});

describe("getFormattedCellText", () => {
  it("formats the new types", () => {
    expect(getFormattedCellText(new SqlDecimal("1.50"), "float")).toBe("1.50");
    expect(getFormattedCellText(10n, "integer")).toBe(formatNumber(10));
    expect(getFormattedCellText(new Uint8Array([1, 2]), "binary")).toBe("\\x0102 2 B");
    expect(getFormattedCellText("AAAA", "binary")).toBe("3 B");
    expect(getFormattedCellText(new Uint8Array([7]), "text")).toBe("\\x07");
    expect(getFormattedCellText([1n, new SqlDecimal("2.0")], "array")).toBe("1  2.0");
  });

  it("is unchanged for ordinary values", () => {
    expect(getFormattedCellText(null, "integer")).toBe("NULL");
    expect(getFormattedCellText(1234, "integer")).toBe(formatNumber(1234));
    expect(getFormattedCellText({ a: 1 }, "json")).toBe('{"a":1}');
    expect(getFormattedCellText([1, 2, 3, 4], "array")).toBe("1  2  3  +1");
    expect(getFormattedCellText("abc", "text")).toBe("abc");
  });
});
