/**
 * The page's write origin: well-formed for Node's and Rust's check, stable
 * for the page, and made without `randomUUID` where the page lacks it.
 */
import { describe, expect, it } from "vitest";
import { newOrigin, ORIGIN_PATTERN, pageOrigin, webPageOrigin } from "./origin";

describe("origin", () => {
  it("is well-formed and the same for the whole page", () => {
    const origin = webPageOrigin();
    expect(origin).toMatch(ORIGIN_PATTERN);
    expect(webPageOrigin()).toBe(origin);
    // Not in Tauri: the page's origin is the web one.
    expect(pageOrigin()).toBe(origin);
  });

  it("uses randomUUID when there is one", () => {
    expect(
      newOrigin({
        randomUUID: () => "11111111-2222-4333-8444-555555555555",
        getRandomValues: () => {
          throw new Error("unused");
        },
      } as unknown as Crypto),
    ).toBe("11111111-2222-4333-8444-555555555555");
  });

  it("falls back to 16 random bytes as hex without randomUUID (plain http)", () => {
    const source = {
      getRandomValues: <T extends ArrayBufferView | null>(array: T): T => {
        const bytes = array as unknown as Uint8Array;
        bytes.forEach((_, i) => (bytes[i] = i * 17));
        return array;
      },
    };
    const origin = newOrigin(source);
    expect(origin).toBe("00112233445566778899aabbccddeeff");
    expect(origin).toMatch(ORIGIN_PATTERN);
    // With the real generator, two differ.
    const real = { getRandomValues: crypto.getRandomValues.bind(crypto) };
    expect(newOrigin(real)).toMatch(/^[0-9a-f]{32}$/);
    expect(newOrigin(real)).not.toBe(newOrigin(real));
  });
});
