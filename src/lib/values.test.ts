import { describe, expect, it } from "vitest";
import {
  SqlDecimal,
  cellKey,
  cellText,
  decodeCell,
  decodeRows,
  encodeParam,
  encodeParams,
  fromStorable,
  jsonReplacer,
  toHex,
  toNumber,
  toStorable,
} from "./values";

// One entry per row of the "Value wire format" table in
// docs/plans/2026-09-25-rust-core-phase-1-plan.md: [wire JSON, decoded JS value].
const WIRE_ROWS: Array<[string, unknown, unknown]> = [
  ["null", null, null],
  ["bool", true, true],
  ["safe int", 9007199254740991, 9007199254740991],
  ["bigint", { $sq: "bigint", v: "9007199254740993" }, 9007199254740993n],
  ["i64::MIN", { $sq: "bigint", v: "-9223372036854775808" }, -9223372036854775808n],
  ["finite float", 1.5, 1.5],
  ["inf", { $sq: "float", v: "inf" }, Infinity],
  ["-inf", { $sq: "float", v: "-inf" }, -Infinity],
  ["decimal", { $sq: "decimal", v: "12.50" }, new SqlDecimal("12.50")],
  ["text", "hello", "hello"],
  ["bytes", { $sq: "bytes", v: "AP8Q" }, new Uint8Array([0, 255, 16])],
  ["json", { $sq: "json", v: { a: [1, 2] } }, { a: [1, 2] }],
  [
    "array",
    [1, { $sq: "bigint", v: "9223372036854775807" }, [{ $sq: "bytes", v: "AQ==" }]],
    [1, 9223372036854775807n, [new Uint8Array([1])]],
  ],
];

describe("decodeCell", () => {
  it.each(WIRE_ROWS)("decodes %s", (_name, wire, js) => {
    expect(decodeCell(structuredClone(wire))).toEqual(js);
  });

  it("decodes NaN", () => {
    expect(decodeCell({ $sq: "float", v: "NaN" })).toBeNaN();
  });

  it("keeps the decimal text exactly", () => {
    const d = decodeCell({ $sq: "decimal", v: "12.50" });
    expect(d).toBeInstanceOf(SqlDecimal);
    expect(String(d)).toBe("12.50");
  });

  it("returns JSON cells as the same object the UI got before tagging", () => {
    const inner = { a: 1 };
    expect(decodeCell({ $sq: "json", v: inner })).toBe(inner);
  });

  it("rejects an unknown tag", () => {
    expect(() => decodeCell({ $sq: "date", v: "2026-01-01" })).toThrow(/date/);
  });

  it("rejects a tag without v, like Rust does", () => {
    expect(() => decodeCell({ $sq: "decimal" })).toThrow(/no "v"/);
  });
});

describe("encodeParam", () => {
  // Round trip: JS value → wire → JS value.
  it.each(WIRE_ROWS)("round-trips %s", (_name, wire, js) => {
    const encoded = JSON.parse(JSON.stringify(encodeParam(js)));
    expect(encoded).toEqual(wire);
    expect(decodeCell(encoded)).toEqual(js);
  });

  it("tags bigint, bytes and decimals", () => {
    expect(encodeParam(10n)).toEqual({ $sq: "bigint", v: "10" });
    expect(encodeParam(new Uint8Array([0, 255, 16]))).toEqual({ $sq: "bytes", v: "AP8Q" });
    expect(encodeParam(new SqlDecimal("1.50"))).toEqual({ $sq: "decimal", v: "1.50" });
  });

  it("tags plain objects as JSON", () => {
    expect(encodeParam({ a: 1 })).toEqual({ $sq: "json", v: { a: 1 } });
  });

  it("sends plain values as they are", () => {
    for (const v of [null, true, 42, 1.5, "s"]) expect(encodeParam(v)).toBe(v);
    expect(encodeParam([1, "a"])).toEqual([1, "a"]);
  });

  it("encodes array elements", () => {
    expect(encodeParam([1n, [new Uint8Array([1])]])).toEqual([
      { $sq: "bigint", v: "1" },
      [{ $sq: "bytes", v: "AQ==" }],
    ]);
  });

  it("tags non-finite numbers instead of letting JSON turn them into null", () => {
    expect(encodeParam(NaN)).toEqual({ $sq: "float", v: "NaN" });
    expect(encodeParam(-Infinity)).toEqual({ $sq: "float", v: "-inf" });
  });

  it("encodes a parameter list, treating undefined as none", () => {
    expect(encodeParams(undefined)).toEqual([]);
    expect(encodeParams([1n, "a"])).toEqual([{ $sq: "bigint", v: "1" }, "a"]);
  });

  it("leaves Date alone so it still serializes as an ISO string", () => {
    const d = new Date("2026-01-01T00:00:00Z");
    expect(encodeParam(d)).toBe(d);
  });
});

describe("decodeRows", () => {
  it("leaves primitive cells untouched", () => {
    const row = [1, "a", null, true, 2.5];
    const rows = [row];
    const out = decodeRows(rows);
    expect(out).toBe(rows);
    expect(out[0]).toBe(row);
    expect(row).toEqual([1, "a", null, true, 2.5]);
  });

  it("decodes tagged cells in place", () => {
    const rows: unknown[][] = [[1, { $sq: "bigint", v: "9007199254740993" }]];
    decodeRows(rows);
    expect(rows[0][1]).toBe(9007199254740993n);
  });
});

describe("jsonReplacer", () => {
  it("never throws for bigint, bytes or decimals", () => {
    const row = {
      id: 9007199254740993n,
      blob: new Uint8Array([1, 2, 255]),
      n: new SqlDecimal("1.50"),
    };
    expect(() => JSON.stringify(row)).toThrow();
    expect(JSON.parse(JSON.stringify(row, jsonReplacer))).toEqual({
      id: "9007199254740993",
      blob: "\\x0102ff",
      n: "1.50",
    });
  });
});

describe("toHex", () => {
  it("uses the Postgres bytea hex format", () => {
    expect(toHex(new Uint8Array([1, 2, 255]))).toBe("\\x0102ff");
    expect(toHex(new Uint8Array([]))).toBe("\\x");
  });
});

describe("cellKey", () => {
  it("distinguishes bigint from number deliberately", () => {
    expect(cellKey(10n) === cellKey(10)).toBe(false);
  });

  it("uses the decimal text", () => {
    expect(cellKey(new SqlDecimal("1.50"))).toBe("1.50");
  });

  it("is stable for equal values", () => {
    expect(cellKey(new Uint8Array([1, 2]))).toBe(cellKey(new Uint8Array([1, 2])));
    expect(cellKey({ a: 1 })).toBe(cellKey({ a: 1 }));
    expect(cellKey(null)).not.toBe(cellKey("null"));
    expect(cellKey(5)).toBe(cellKey(5));
  });
});

describe("cellKey for PK matching", () => {
  it("matches a decoded bigint PK against the same decoded value", () => {
    expect(cellKey(9007199254740993n)).toBe(cellKey(9007199254740993n));
    expect(cellKey(9007199254740993n)).not.toBe(cellKey(9007199254740992n));
  });

  it("keeps today's String() matching for strings and numbers", () => {
    expect(cellKey("10")).toBe(cellKey(10));
    expect(cellKey(true)).toBe(cellKey("true"));
  });
});

describe("toNumber", () => {
  it("converts bigint and SqlDecimal", () => {
    expect(toNumber(42n)).toBe(42);
    expect(toNumber(9007199254740993n)).toBe(9007199254740992);
    expect(toNumber(new SqlDecimal("12.50"))).toBe(12.5);
    expect(toNumber(new SqlDecimal("NaN"))).toBeNaN();
  });

  it("behaves like Number() for everything else", () => {
    expect(toNumber(3.5)).toBe(3.5);
    expect(toNumber("7")).toBe(7);
    expect(toNumber("abc")).toBeNaN();
    expect(toNumber(null)).toBe(0);
    expect(toNumber(undefined)).toBeNaN();
    expect(toNumber(true)).toBe(1);
  });
});

describe("cellText", () => {
  it("renders the new value types", () => {
    expect(cellText(9007199254740993n)).toBe("9007199254740993");
    expect(cellText(new SqlDecimal("1.50"))).toBe("1.50");
    expect(cellText(new Uint8Array([1, 255]))).toBe("\\x01ff");
    expect(cellText([1n, new Uint8Array([2])])).toBe("1,\\x02");
  });

  it("matches String() for ordinary values", () => {
    for (const v of ["a", 1, 1.5, true, [1, 2], [null, "x"], [[1, 2], [3]], new Date(0)]) {
      // oxlint-disable-next-line typescript/no-base-to-string
      expect(cellText(v)).toBe(String(v));
    }
    expect(cellText(null)).toBe("");
    expect(cellText(undefined)).toBe("");
  });

  it("writes JSON cells as JSON text", () => {
    expect(cellText({ a: 1, big: 2n })).toBe('{"a":1,"big":"2"}');
    expect(cellText([{ a: 1 }, 2])).toBe('{"a":1},2');
  });
});

describe("toStorable / fromStorable", () => {
  const roundTrip = (v: unknown) => fromStorable(JSON.parse(JSON.stringify(toStorable(v))));

  it("round-trips decoded cells inside nested state", () => {
    const state = {
      rows: [[1n, new Uint8Array([7]), new SqlDecimal("0.10"), NaN, -Infinity, "x", null, 2.5]],
      nested: { deep: [9007199254740993n] },
    };
    expect(roundTrip(state)).toEqual(state);
  });

  it("keeps a user object with a $sq key as it was", () => {
    const cell = { $sq: "bigint", v: "1" };
    expect(roundTrip({ cell })).toEqual({ cell });
  });

  it("leaves ordinary JSON unchanged", () => {
    const plain = { a: [1, "b", true, null, { c: 2 }] };
    expect(JSON.stringify(toStorable(plain))).toBe(JSON.stringify(plain));
    expect(fromStorable(plain)).toEqual(plain);
    const d = new Date(0);
    expect(JSON.stringify(toStorable({ d }))).toBe(JSON.stringify({ d }));
  });
});
