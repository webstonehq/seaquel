/**
 * The Node side of `/internal/license/*`: requests, the 5-second per-user
 * gate cache and its invalidation, error mapping, and the connection retry
 * while the Rust service starts. `fetch` is stubbed; the Rust side is
 * tested in `crates/seaquel-server/tests/internal_license.rs`.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const openAuthDb = vi.fn();
vi.mock("./auth", () => ({ openAuthDb: () => openAuthDb() }));

process.env.SEAQUEL_INTERNAL_SECRET = "test-secret";
const client = await import("./license-client");

const GATE_OK = {
  state: "ok",
  tenant: null,
  member: { isOwner: true, revoked: false },
  hasTenant: true,
  bundlePresent: false,
};

interface Seen {
  url: URL;
  method: string;
  headers: Record<string, string>;
  body: string | Uint8Array | null;
}

let seen: Seen[] = [];
let reply: (s: Seen) => Response = () => Response.json(GATE_OK);

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

beforeEach(() => {
  seen = [];
  reply = () => Response.json(GATE_OK);
  openAuthDb.mockReset();
  client._resetLicenseClient();
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: URL, init: RequestInit = {}) => {
      const s: Seen = {
        url,
        method: init.method ?? "GET",
        headers: (init.headers as Record<string, string>) ?? {},
        body: (init.body as string | Uint8Array | undefined) ?? null,
      };
      seen.push(s);
      return reply(s);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("requests", () => {
  it("calls the loopback service's /internal/license routes", async () => {
    await client.gate("u_1");
    expect(seen[0].url.toString()).toBe("http://127.0.0.1:8788/internal/license/gate?user=u_1");
    expect(seen[0].method).toBe("GET");
  });

  it("leaves out ?user= for a signed-out request and encodes ids", async () => {
    await client.gate(null);
    await client.airgapStatus("a b&c");
    expect(seen[0].url.search).toBe("");
    expect(seen[1].url.searchParams.get("user")).toBe("a b&c");
  });

  it("sends JSON bodies for the signup steps", async () => {
    reply = () => Response.json({ ok: true });
    await client.registerInstall("KEY-1");
    await client.signupCheck("KEY-1", "a@b.test");
    await client.bindMember({
      licenseKey: "KEY-1",
      userId: "u_1",
      email: "a@b.test",
      role: "owner",
      firstOwner: true,
    });
    await client.unbindMember("u_2");
    expect(seen.map((s) => [s.method, s.url.pathname, s.body])).toEqual([
      ["POST", "/internal/license/register-install", '{"licenseKey":"KEY-1"}'],
      ["POST", "/internal/license/signup-check", '{"licenseKey":"KEY-1","email":"a@b.test"}'],
      [
        "POST",
        "/internal/license/bind-member",
        '{"licenseKey":"KEY-1","userId":"u_1","email":"a@b.test","role":"owner","firstOwner":true}',
      ],
      ["POST", "/internal/license/unbind-member", '{"userId":"u_2"}'],
    ]);
    for (const s of seen) expect(s.headers["content-type"]).toBe("application/json");
  });

  it("sends the bundle's bytes as they are", async () => {
    reply = () => Response.json({ status: 200, body: { ok: true }, revokedUserIds: [] });
    const bytes = new TextEncoder().encode('{"payload":"x"}');
    const out = await client.airgapUpload(bytes, "u_owner");
    expect(seen[0].url.pathname).toBe("/internal/license/airgap/upload");
    expect(seen[0].url.searchParams.get("user")).toBe("u_owner");
    expect(seen[0].body).toBe(bytes);
    expect(out).toEqual({ status: 200, body: { ok: true }, revokedUserIds: [] });
  });

  it("opens auth.db once before the first call, so Rust finds its tables", async () => {
    await client.gate("u_1");
    await client.installStatus();
    expect(openAuthDb).toHaveBeenCalledTimes(1);
  });
});

describe("the per-boot secret", () => {
  it("goes on every call", async () => {
    await client.gate("u_1");
    await client.registerInstall("k");
    for (const s of seen) expect(s.headers["x-seaquel-internal"]).toBe("test-secret");
  });

  it("without it nothing is sent and the service counts as unavailable", async () => {
    const saved = process.env.SEAQUEL_INTERNAL_SECRET;
    delete process.env.SEAQUEL_INTERNAL_SECRET;
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      await expect(client.gate("u_1")).rejects.toMatchObject({ code: "UPSTREAM_UNAVAILABLE" });
      expect(seen).toHaveLength(0);
    } finally {
      process.env.SEAQUEL_INTERNAL_SECRET = saved;
      errors.mockRestore();
    }
  });
});

describe("the gate cache", () => {
  it("a gate call that started before an invalidation doesn't store its answer", async () => {
    // The gate answer is in flight when a signup binds the user.
    let release: (r: Response) => void = () => {};
    reply = () => new Response(null);
    vi.stubGlobal(
      "fetch",
      vi.fn(
        (url: URL) =>
          new Promise<Response>((resolve) => {
            seen.push({ url, method: "GET", headers: {}, body: null });
            release = resolve;
          }),
      ),
    );
    const stale = client.gate("u_1");
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    client.invalidateGate();
    release(Response.json({ ...GATE_OK, member: null }));
    expect((await stale).member).toBeNull();

    // The next call asks again instead of reading the stale answer.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: URL) => {
        seen.push({ url, method: "GET", headers: {}, body: null });
        return Response.json(GATE_OK);
      }),
    );
    expect((await client.gate("u_1")).member).toEqual(GATE_OK.member);
    expect(seen).toHaveLength(2);
    // And that fresh answer is cached.
    await client.gate("u_1");
    expect(seen).toHaveLength(2);
  });

  it("answers one user from the cache for 5 seconds", async () => {
    vi.useFakeTimers();
    await client.gate("u_1");
    await client.gate("u_1");
    expect(seen).toHaveLength(1);
    vi.advanceTimersByTime(4_999);
    await client.gate("u_1");
    expect(seen).toHaveLength(1);
    vi.advanceTimersByTime(1);
    await client.gate("u_1");
    expect(seen).toHaveLength(2);
  });

  it("keeps users (and the signed-out key) apart", async () => {
    await client.gate("u_1");
    await client.gate("u_2");
    await client.gate(null);
    await client.gate(null);
    expect(seen.map((s) => s.url.searchParams.get("user"))).toEqual(["u_1", "u_2", null]);
  });

  it("every licensing change drops the whole cache", async () => {
    const changes: Array<[string, () => Promise<unknown>]> = [
      ["registerInstall", () => client.registerInstall("k")],
      [
        "bindMember",
        () =>
          client.bindMember({
            licenseKey: "k",
            userId: "u",
            email: "e",
            role: "member",
            firstOwner: false,
          }),
      ],
      ["unbindMember", () => client.unbindMember("u")],
      ["airgapUpload", () => client.airgapUpload(new Uint8Array([1]), null)],
      ["airgapClear", () => client.airgapClear("u")],
    ];
    for (const [name, change] of changes) {
      client._resetLicenseClient();
      seen = [];
      reply = (s) =>
        s.url.pathname.endsWith("/gate")
          ? Response.json(GATE_OK)
          : Response.json({ status: 200, body: {}, revokedUserIds: [] });
      await client.gate("u_1");
      await client.gate("u_2");
      await change();
      await client.gate("u_1");
      await client.gate("u_2");
      const gates = seen.filter((s) => s.url.pathname.endsWith("/gate"));
      expect(gates, name).toHaveLength(4);
    }
  });

  it("a failed change still drops the cache", async () => {
    await client.gate("u_1");
    reply = () => jsonResponse(502, { code: "NETWORK_ERROR", message: "unreachable" });
    await expect(client.registerInstall("k")).rejects.toThrow();
    reply = () => Response.json(GATE_OK);
    await client.gate("u_1");
    expect(seen.filter((s) => s.url.pathname.endsWith("/gate"))).toHaveLength(2);
  });

  it("errors aren't cached", async () => {
    reply = () => jsonResponse(503, { code: "NOT_READY", message: "no tables yet" });
    await expect(client.gate("u_1")).rejects.toMatchObject({ code: "NOT_READY", status: 503 });
    reply = () => Response.json(GATE_OK);
    await expect(client.gate("u_1")).resolves.toEqual(GATE_OK);
  });
});

describe("errors", () => {
  it("carries Rust's code, message and status", async () => {
    reply = () => jsonResponse(409, { code: "NO_OWNER", message: "no owner license bound" });
    const e = await client.listMembers().catch((x: unknown) => x);
    expect(e).toBeInstanceOf(client.LicenseClientError);
    expect(e).toMatchObject({ code: "NO_OWNER", message: "no owner license bound", status: 409 });
  });

  it("isNetworkFailure is Rust's NETWORK_ERROR and nothing else", async () => {
    reply = () =>
      jsonResponse(502, { code: "NETWORK_ERROR", message: "control plane unreachable" });
    const network = await client.registerInstall("k").catch((x: unknown) => x);
    expect(client.isNetworkFailure(network)).toBe(true);

    reply = () =>
      jsonResponse(502, { code: "CONTROL_PLANE_ERROR", message: "register-install failed: 400" });
    const http = await client.registerInstall("k").catch((x: unknown) => x);
    expect(client.isNetworkFailure(http)).toBe(false);

    expect(client.isNetworkFailure(new TypeError("fetch failed"))).toBe(false);
    expect(client.isNetworkFailure(null)).toBe(false);
  });

  it("an answer that isn't {code, message} is HTTP_<status>", async () => {
    reply = () => new Response("oops", { status: 500 });
    await expect(client.installStatus()).rejects.toMatchObject({ code: "HTTP_500", status: 500 });
  });

  it("retries while the service refuses connections, then gives up", async () => {
    vi.useFakeTimers();
    let calls = 0;
    const refused = Object.assign(new TypeError("fetch failed"), {
      cause: { code: "ECONNREFUSED" },
    });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        calls++;
        if (calls < 3) throw refused;
        return Response.json({ hasTenant: false, bundlePresent: false });
      }),
    );
    const pending = client.installStatus();
    await vi.runAllTimersAsync();
    await expect(pending).resolves.toEqual({ hasTenant: false, bundlePresent: false });
    expect(calls).toBe(3);

    calls = -100;
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const failing = client.installStatus().catch((x: unknown) => x);
    await vi.runAllTimersAsync();
    expect(await failing).toMatchObject({ code: "UPSTREAM_UNAVAILABLE", status: 502 });
    expect(client.isNetworkFailure(await failing)).toBe(false);
    errors.mockRestore();
  });

  it("never retries a request that may have reached Rust", async () => {
    let calls = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        calls++;
        throw Object.assign(new TypeError("fetch failed"), { cause: { code: "ECONNRESET" } });
      }),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(client.registerInstall("k")).rejects.toMatchObject({
      code: "UPSTREAM_UNAVAILABLE",
    });
    expect(calls).toBe(1);
    errors.mockRestore();
  });

  it("never logs a license key", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed");
      }),
    );
    await client.registerInstall("SQ-SECRET-KEY").catch(() => {});
    await client.signupCheck("SQ-SECRET-KEY", "e").catch(() => {});
    for (const call of errors.mock.calls) {
      expect(String(call.map((c: unknown) => String(c)).join(" "))).not.toContain("SQ-SECRET-KEY");
    }
    errors.mockRestore();
  });
});
