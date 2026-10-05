import { describe, expect, it } from "vitest";
import { sizeText } from "./size-text";

describe("sizeText", () => {
  it("says sizes in decimal units", () => {
    expect(sizeText(11_503_115)).toBe("11.5 MB");
    expect(sizeText(640_001)).toBe("641 KB");
    expect(sizeText(12)).toBe("12 bytes");
  });
});
