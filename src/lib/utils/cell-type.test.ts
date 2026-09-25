import { describe, expect, it } from "vitest";
import {
  binaryCellBytes,
  editedCellValue,
  binaryStringEncoding,
  displayCellType,
  cellTypeFromColumnType,
  detectCellType,
  detectColumnTypes,
  formatBinaryPreview,
  formatByteSize,
  formatCellNumber,
  formatDateTime,
  formatNumber,
  formatTime,
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

  it("treats an integral SqlDecimal as an integer", () => {
    // MySQL BIGINT UNSIGNED above 2^63-1 arrives as a SqlDecimal of its digits.
    expect(detectCellType(new SqlDecimal("18446744073709551615"))).toBe("integer");
    expect(detectCellType(new SqlDecimal("-12"))).toBe("integer");
    expect(detectCellType(new SqlDecimal("12.0"))).toBe("float");
    expect(detectCellType(new SqlDecimal("NaN"))).toBe("float");
    expect(formatCellNumber(new SqlDecimal("18446744073709551615"))).toBe("18446744073709551615");
  });

  it("recognizes DuckDB's date and time text", () => {
    expect(detectCellType("2024-01-01 12:00:00+00")).toBe("datetime");
    expect(detectCellType("2024-01-01 12:00:00.123456789")).toBe("datetime");
    expect(detectCellType("12:00:00+02")).toBe("time");
    expect(detectCellType("12:00:00.5-05:30")).toBe("time");
    // BC dates and infinity stay text, shown as DuckDB printed them.
    expect(detectCellType("0044-03-15 (BC)")).toBe("text");
    expect(detectCellType("infinity")).toBe("text");
  });

  it("formats hour-only UTC offsets", () => {
    // DuckDB prints TIMESTAMPTZ in UTC as `+00`, which `new Date()` rejects.
    expect(formatDateTime("2024-01-01 12:00:00+00")).toBe(
      formatDateTime("2024-01-01T12:00:00+00:00"),
    );
    expect(formatDateTime("2024-01-01 12:00:00+00")).not.toBe("2024-01-01 12:00:00+00");
    expect(formatDateTime("2026-02-15 21:40:23.568684 +00:00:00")).toBe(
      formatDateTime("2026-02-15T21:40:23.568684+00:00"),
    );
    // Offsets without a colon (`+0530`).
    expect(formatDateTime("2024-01-01 12:00:00+0530")).toBe(
      formatDateTime("2024-01-01T12:00:00+05:30"),
    );
    expect(formatDateTime("2024-01-01 12:00:00+0530")).not.toBe("2024-01-01 12:00:00+0530");
    // Unparseable text comes back as is.
    expect(formatDateTime("0044-03-15 (BC) 12:00:00+00")).toBe("0044-03-15 (BC) 12:00:00+00");
    expect(formatDateTime("0044-03-15 (BC) 12:00:00")).toBe("0044-03-15 (BC) 12:00:00");
    expect(formatDateTime("infinity")).toBe("infinity");
  });

  it("shows times with a zone as they are", () => {
    // DuckDB TIMETZ.
    for (const t of ["12:00:00+02", "12:00:00.5-05:30", "23:59:59+15:59:59", "12:00:00+00"]) {
      expect(formatTime(t)).toBe(t);
    }
    // Postgres timetz (sqlx prints the offset with seconds).
    for (const t of ["12:00:00+02:00:00", "12:00:00.123-05:30:00", "12:00:00+0530"]) {
      expect(formatTime(t)).toBe(t);
    }
    // A plain time is still formatted.
    expect(formatTime("13:05:00")).not.toBe("13:05:00");
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

  it("sizes a string by its UTF-8", () => {
    expect(formatByteSize("AAAA")).toBe("4 B");
    expect(formatByteSize("é")).toBe("2 B");
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

describe("cellTypeFromColumnType", () => {
  it("shows BIT(1) and bare BIT as booleans, wider BIT(n) by value", () => {
    expect(cellTypeFromColumnType("bit")).toBe("boolean");
    expect(cellTypeFromColumnType("bit(1)")).toBe("boolean");
    expect(cellTypeFromColumnType("BIT(1)")).toBe("boolean");
    // MySQL decodes BIT(12) as a number: a checkbox would save 1 over it.
    expect(cellTypeFromColumnType("bit(12)")).toBeNull();
    expect(cellTypeFromColumnType("bit(64)")).toBeNull();
    expect(detectColumnTypes(["b"], [[2730]], { b: "bit(12)" })).toEqual({ b: "integer" });
  });
});

describe("binary columns holding strings", () => {
  it.each(["binary(4)", "varbinary(16)", "blob", "tinyblob", "mediumblob", "LONGBLOB", "bytea"])(
    "classifies %s as binary",
    (declared) => {
      expect(cellTypeFromColumnType(declared)).toBe("binary");
      expect(detectColumnTypes(["b"], [["hello"]], { b: declared })).toEqual({ b: "binary" });
    },
  );

  it("picks the encoding per engine", () => {
    expect(binaryStringEncoding("mysql")).toBe("utf8");
    expect(binaryStringEncoding("mariadb")).toBe("utf8");
    // MSSQL binary and DuckDB BLOBs arrive as bytes, so their strings are text.
    for (const t of ["postgres", "sqlite", "mssql", "duckdb", undefined]) {
      expect(binaryStringEncoding(t)).toBe("text");
    }
  });

  // MySQL/MariaDB send a VARBINARY value that is clean UTF-8 as text.
  it("shows a MySQL string cell as its UTF-8 bytes with the right size", () => {
    expect(getFormattedCellText("hello", "binary", "utf8")).toBe("\\x68656c6c6f 5 B");
    // 2-byte character: 3 bytes, not 2 characters.
    expect(getFormattedCellText("é!", "binary", "utf8")).toBe("\\xc3a921 3 B");
    expect(getFormattedCellText("", "binary", "utf8")).toBe("\\x 0 B");
    expect(getFormattedCellText("x".repeat(2000), "binary", "utf8")).toBe(
      `\\x${"78".repeat(16)}… 2.0 KB`,
    );
    const bytes = new TextEncoder().encode("abc");
    expect(getFormattedCellText("abc", "binary", "utf8")).toBe(
      getFormattedCellText(bytes, "binary"),
    );
    expect(binaryCellBytes("abc", "utf8")).toEqual(bytes);
    expect(binaryCellBytes(bytes)).toBe(bytes);
  });

  // SQLite reports untyped columns as BLOB and keeps TEXT values in them.
  it("shows a string as text on other engines", () => {
    expect(binaryCellBytes("hello", "text")).toBeNull();
    expect(binaryCellBytes("hello")).toBeNull();
    expect(displayCellType("hello", "binary", "text")).toBe("text");
    expect(displayCellType("2024-01-02", "binary")).toBe("date");
    expect(displayCellType("hello", "binary", "utf8")).toBe("binary");
    expect(displayCellType(new Uint8Array([1]), "binary", "text")).toBe("binary");
    // Any non-bytes value: a number in an untyped SQLite column is a number.
    expect(displayCellType(42, "binary", "text")).toBe("integer");
    expect(displayCellType(1.5, "binary")).toBe("float");
    expect(displayCellType(42, "binary", "utf8")).toBe("binary");
    expect(getFormattedCellText(1234, "binary", "text")).toBe(formatNumber(1234));
    expect(getFormattedCellText("hello", "binary", "text")).toBe("hello");
    expect(getFormattedCellText("hello", "binary")).toBe("hello");
    // Bytes are still bytes.
    expect(getFormattedCellText(new Uint8Array([1]), "binary", "text")).toBe("\\x01 1 B");
  });

  it("is null for non-binary values", () => {
    expect(binaryCellBytes(12, "utf8")).toBeNull();
    expect(binaryCellBytes(null, "utf8")).toBeNull();
  });
});

describe("editedCellValue", () => {
  it("saves hex typed over bytes as bytes, on any engine", () => {
    expect(editedCellValue(new Uint8Array([1]), "binary", "\\x00ff")).toEqual(
      new Uint8Array([0, 255]),
    );
    // Bytes shown in a column whose declared type isn't known (query results).
    expect(editedCellValue(new Uint8Array([1]), "text", "\\xAB")).toEqual(new Uint8Array([0xab]));
  });

  it("saves hex typed into a MySQL binary column that held a string as bytes", () => {
    expect(editedCellValue("hello", "binary", "\\x6869", "utf8")).toEqual(
      new Uint8Array([0x68, 0x69]),
    );
    // A NULL cell in such a column too.
    expect(editedCellValue(null, "binary", "\\x00", "utf8")).toEqual(new Uint8Array([0]));
  });

  it("never turns a text string into bytes on other engines (SQLite's untyped BLOB)", () => {
    expect(editedCellValue("hello", "binary", "\\x6869", "text")).toBe("\\x6869");
    expect(editedCellValue("hello", "binary", "\\x6869")).toBe("\\x6869");
    expect(editedCellValue(null, "binary", "\\x00", "text")).toBe("\\x00");
  });

  it("keeps anything else as typed", () => {
    expect(editedCellValue("hello", "binary", "hi", "utf8")).toBe("hi");
    expect(editedCellValue(new Uint8Array([1]), "binary", "\\xabc")).toBe("\\xabc");
    expect(editedCellValue("a", "text", "\\x6869", "utf8")).toBe("\\x6869");
    expect(editedCellValue(null, "text", "\\x00", "utf8")).toBe("\\x00");
  });
});
