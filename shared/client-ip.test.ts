import { describe, expect, it } from "vitest";
import { parseTrustedProxies, resolveClientIp } from "./client-ip.js";

const req = (remoteAddress: string, xff?: string) => ({
  socket: { remoteAddress },
  headers: xff === undefined ? {} : { "x-forwarded-for": xff },
});

describe("resolveClientIp", () => {
  it("uses the socket address when no proxies are trusted, ignoring X-Forwarded-For", () => {
    const trusted = parseTrustedProxies("");
    expect(resolveClientIp(req("203.0.113.9", "1.2.3.4"), trusted)).toBe("203.0.113.9");
  });

  it("strips IPv4-mapped IPv6 prefixes", () => {
    const trusted = parseTrustedProxies("");
    expect(resolveClientIp(req("::ffff:203.0.113.9"), trusted)).toBe("203.0.113.9");
  });

  it("ignores X-Forwarded-For from an untrusted peer", () => {
    const trusted = parseTrustedProxies("10.0.0.0/8");
    expect(resolveClientIp(req("203.0.113.9", "1.2.3.4"), trusted)).toBe("203.0.113.9");
  });

  it("takes the rightmost untrusted hop behind a trusted proxy", () => {
    const trusted = parseTrustedProxies("10.0.0.0/8, 192.168.1.5");
    // Client spoofed 1.2.3.4; the real client is 198.51.100.7, appended by the proxy.
    expect(resolveClientIp(req("10.0.0.2", "1.2.3.4, 198.51.100.7, 192.168.1.5"), trusted)).toBe(
      "198.51.100.7",
    );
  });

  it("falls back to the last trusted hop when the chain is all trusted or has garbage", () => {
    const trusted = parseTrustedProxies("10.0.0.0/8");
    expect(resolveClientIp(req("10.0.0.2", "10.0.0.3"), trusted)).toBe("10.0.0.3");
    expect(resolveClientIp(req("10.0.0.2", "not-an-ip"), trusted)).toBe("10.0.0.2");
    expect(resolveClientIp(req("10.0.0.2"), trusted)).toBe("10.0.0.2");
  });

  it("supports IPv6 proxies", () => {
    const trusted = parseTrustedProxies("fd00::/8");
    expect(resolveClientIp(req("fd00::1", "2001:db8::5"), trusted)).toBe("2001:db8::5");
  });

  it("rejects malformed trusted-proxy entries", () => {
    expect(() => parseTrustedProxies("nonsense")).toThrow();
    expect(() => parseTrustedProxies("10.0.0.0/99")).toThrow();
  });
});
