import { describe, expect, it } from "vitest";
import { formatBindValues, formatWireBindValues } from "./bind-values";

describe("formatBindValues", () => {
  it("numbers the values in bind order, strings quoted and NULL spelled out", () => {
    expect(formatBindValues(["Jonson", 1, null, true])).toBe("1: 'Jonson'  2: 1  3: NULL  4: true");
  });

  it("is empty without values", () => {
    expect(formatBindValues(undefined)).toBe("");
    expect(formatBindValues([])).toBe("");
  });
});

describe("formatBindValues with a limit", () => {
  it("stops at the limit, cutting a long value before formatting it", () => {
    const long = "x".repeat(1_000_000);
    const out = formatBindValues([long, 2], 200);
    expect(out.length).toBeLessThanOrEqual(201);
    expect(out.startsWith("1: 'xxx")).toBe(true);
    expect(out.endsWith("…")).toBe(true);
  });

  it("cuts bytes and lists before formatting them", () => {
    const out = formatBindValues([new Uint8Array(1_000_000), Array(1_000_000).fill(1)], 200);
    expect(out.length).toBeLessThanOrEqual(201);
  });

  it("keeps short values whole", () => {
    expect(formatBindValues(["a", 1], 200)).toBe("1: 'a'  2: 1");
  });
});

describe("formatWireBindValues", () => {
  it("cuts long wire values before decoding them", () => {
    const out = formatWireBindValues(
      ["y".repeat(1_000_000), { $sq: "bytes", v: "AAAA".repeat(250_000) }],
      200,
    );
    expect(out.length).toBeLessThanOrEqual(201);
    expect(out.startsWith("1: 'yyy")).toBe(true);
  });

  it("never writes to the values (deep $state throws state_unsafe_mutation in a template)", () => {
    // Vitest resolves Svelte's server build, where $state isn't a proxy, so
    // a deeply frozen list stands in for it: any write throws.
    const wire = Object.freeze([
      Object.freeze([Object.freeze({ $sq: "bigint", v: "9007199254740993" })]),
    ]);
    expect(formatWireBindValues(wire, 200)).toBe("1: 9007199254740993");
  });

  it("doesn't change the values it's given (a history row's, deep $state)", () => {
    const wire = [[{ $sq: "bigint", v: "9007199254740993" }]];
    const before = JSON.stringify(wire);
    expect(formatWireBindValues(wire)).toBe("1: 9007199254740993");
    expect(JSON.stringify(wire)).toBe(before);
  });

  it("decodes the wire format first", () => {
    expect(
      formatWireBindValues([
        { $sq: "bigint", v: "9007199254740993" },
        { $sq: "decimal", v: "12.50" },
        "x",
      ]),
    ).toBe("1: 9007199254740993  2: 12.50  3: 'x'");
  });
});
