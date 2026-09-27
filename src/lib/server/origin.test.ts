/**
 * The Origin allow-list: configured origins, the install's own origin, and
 * the dev servers' origins only in a dev build.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const build = vi.hoisted(() => ({ dev: false }));
vi.mock("$app/environment", () => ({
  get dev() {
    return build.dev;
  },
}));

const {
  DEV_ORIGINS,
  betterAuthTrustedOrigins,
  getTrustedOrigins,
  isOriginTrusted,
  isIpOrLoopbackHost,
  sameHostOrigin,
} = await import("./origin");

const ENV_KEYS = ["SEAQUEL_TRUSTED_ORIGINS", "BETTER_AUTH_URL", "ORIGIN"] as const;
let saved: Record<string, string | undefined> = {};

beforeEach(() => {
  saved = Object.fromEntries(ENV_KEYS.map((k) => [k, process.env[k]]));
  for (const k of ENV_KEYS) delete process.env[k];
  build.dev = false;
});

afterEach(() => {
  for (const k of ENV_KEYS) {
    if (saved[k] === undefined) delete process.env[k];
    else process.env[k] = saved[k];
  }
});

describe("getTrustedOrigins", () => {
  it("has no localhost origins in a production build", () => {
    expect(getTrustedOrigins()).toEqual([]);
    for (const origin of DEV_ORIGINS) expect(isOriginTrusted(origin)).toBe(false);
  });

  it("trusts the dev servers' origins in a dev build", () => {
    build.dev = true;
    expect(getTrustedOrigins()).toEqual(DEV_ORIGINS);
    expect(isOriginTrusted("http://localhost:5173")).toBe(true);
    expect(isOriginTrusted("http://127.0.0.1:8787")).toBe(true);
  });

  it("reads SEAQUEL_TRUSTED_ORIGINS, BETTER_AUTH_URL and ORIGIN", () => {
    process.env.SEAQUEL_TRUSTED_ORIGINS = " https://a.example , ,https://b.example";
    process.env.BETTER_AUTH_URL = "https://seaquel.example/some/path";
    process.env.ORIGIN = "https://origin.example";
    expect(getTrustedOrigins()).toEqual([
      "https://a.example",
      "https://b.example",
      "https://seaquel.example",
      "https://origin.example",
    ]);
  });

  it("ignores a BETTER_AUTH_URL or ORIGIN that isn't an http(s) URL", () => {
    process.env.BETTER_AUTH_URL = "not a url";
    process.env.ORIGIN = "file:///etc";
    expect(getTrustedOrigins()).toEqual([]);
  });
});

describe("sameHostOrigin", () => {
  it.each([
    ["http://localhost:8787", "localhost:8787"],
    ["https://seaquel.example", "seaquel.example"],
    ["https://seaquel.example", "SEAQUEL.example"],
    ["http://[::1]:8787", "[::1]:8787"],
  ])("%s matches Host %s", (origin, host) => {
    expect(sameHostOrigin(origin, host)).toBe(origin);
  });

  it.each([
    ["https://evil.example", "seaquel.example"],
    ["http://localhost:5173", "localhost:8787"],
    ["http://localhost", "localhost:8787"],
    ["https://seaquel.example.evil.example", "seaquel.example"],
    ["null", "seaquel.example"],
    ["file://seaquel.example", "seaquel.example"],
    ["https://seaquel.example/path", "seaquel.example"],
    [null, "seaquel.example"],
    ["https://seaquel.example", null],
    ["https://seaquel.example", ""],
  ])("%s doesn't match Host %s", (origin, host) => {
    expect(sameHostOrigin(origin, host)).toBeNull();
  });
});

describe("isIpOrLoopbackHost", () => {
  it.each(["localhost:8787", "LOCALHOST", "127.0.0.1:8787", "10.0.0.5", "[::1]:8787", "[fd00::1]"])(
    "%s is",
    (host) => expect(isIpOrLoopbackHost(host)).toBe(true),
  );
  it.each([
    "seaquel.example",
    "evil.example:8787",
    "localhost.evil.example",
    "127.0.0.1.nip.io",
    "",
  ])("%s isn't", (host) => expect(isIpOrLoopbackHost(host)).toBe(false));
});

describe("isOriginTrusted", () => {
  it("trusts a loopback or IP install's own origin with nothing configured", () => {
    expect(isOriginTrusted("http://localhost:8787", "localhost:8787")).toBe(true);
    expect(isOriginTrusted("http://127.0.0.1:8787", "127.0.0.1:8787")).toBe(true);
    expect(isOriginTrusted("http://192.168.1.20:8787", "192.168.1.20:8787")).toBe(true);
    expect(isOriginTrusted("http://[::1]:8787", "[::1]:8787")).toBe(true);
  });

  it("refuses a domain Host's own origin (DNS rebinding) with nothing configured", () => {
    // A rebinding page on evil.example reaches the install with its own
    // name in both headers.
    expect(isOriginTrusted("http://evil.example:8787", "evil.example:8787")).toBe(false);
    expect(isOriginTrusted("https://db.corp.example", "db.corp.example")).toBe(false);
  });

  it("has no Host fallback once BETTER_AUTH_URL or ORIGIN is set", () => {
    process.env.BETTER_AUTH_URL = "https://seaquel.example";
    expect(isOriginTrusted("https://seaquel.example", "seaquel.example")).toBe(true);
    expect(isOriginTrusted("http://localhost:8787", "localhost:8787")).toBe(false);
    expect(isOriginTrusted("http://127.0.0.1:8787", "127.0.0.1:8787")).toBe(false);
    delete process.env.BETTER_AUTH_URL;
    process.env.ORIGIN = "https://seaquel.example";
    expect(isOriginTrusted("http://localhost:8787", "localhost:8787")).toBe(false);
    expect(isOriginTrusted("https://seaquel.example", "anything")).toBe(true);
  });

  it("refuses another origin, even from localhost", () => {
    expect(isOriginTrusted("http://localhost:5173", "localhost:8787")).toBe(false);
    expect(isOriginTrusted("https://evil.example", "localhost:8787")).toBe(false);
  });

  it("refuses a missing or empty Origin", () => {
    expect(isOriginTrusted(null, "localhost:8787")).toBe(false);
    expect(isOriginTrusted("", "localhost:8787")).toBe(false);
    expect(isOriginTrusted(undefined)).toBe(false);
  });

  it("trusts a configured origin whatever the Host", () => {
    process.env.SEAQUEL_TRUSTED_ORIGINS = "https://seaquel.example";
    expect(isOriginTrusted("https://seaquel.example", "127.0.0.1:8787")).toBe(true);
    expect(isOriginTrusted("https://seaquel.example")).toBe(true);
  });
});

describe("betterAuthTrustedOrigins", () => {
  function request(headers: Record<string, string>): Request {
    return new Request("https://ignored.example/api/auth/sign-in/email", { headers });
  }

  it("is the configured list at startup", () => {
    process.env.SEAQUEL_TRUSTED_ORIGINS = "https://seaquel.example";
    expect(betterAuthTrustedOrigins()).toEqual(["https://seaquel.example"]);
  });

  it("adds the request's own origin", () => {
    expect(
      betterAuthTrustedOrigins(
        request({ origin: "http://localhost:8787", host: "localhost:8787" }),
      ),
    ).toEqual(["http://localhost:8787"]);
  });

  it("doesn't add a domain Host's origin, or any once BETTER_AUTH_URL is set", () => {
    expect(
      betterAuthTrustedOrigins(
        request({ origin: "http://evil.example:8787", host: "evil.example:8787" }),
      ),
    ).toEqual([]);
    process.env.BETTER_AUTH_URL = "https://seaquel.example";
    expect(
      betterAuthTrustedOrigins(
        request({ origin: "http://localhost:8787", host: "localhost:8787" }),
      ),
    ).toEqual(["https://seaquel.example"]);
  });

  it("doesn't add another site's origin", () => {
    expect(
      betterAuthTrustedOrigins(request({ origin: "https://evil.example", host: "localhost:8787" })),
    ).toEqual([]);
  });
});
