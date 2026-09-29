import { describe, expect, it } from "vitest";
import { MIN_VERSION_LIMIT, clampVersionLimit } from "./version-limit";

describe("clampVersionLimit", () => {
  it("raises a limit below the minimum to the minimum", () => {
    for (const n of [0, 1, 2, 8, 9, -5]) expect(clampVersionLimit(n)).toBe(MIN_VERSION_LIMIT);
  });

  it("keeps a limit at or above the minimum, as a whole number", () => {
    expect(clampVersionLimit(10)).toBe(10);
    expect(clampVersionLimit(100)).toBe(100);
    expect(clampVersionLimit(250.7)).toBe(250);
  });

  it("treats an empty or non-numeric input as the minimum", () => {
    expect(clampVersionLimit(null)).toBe(MIN_VERSION_LIMIT);
    expect(clampVersionLimit(Number.NaN)).toBe(MIN_VERSION_LIMIT);
  });
});
