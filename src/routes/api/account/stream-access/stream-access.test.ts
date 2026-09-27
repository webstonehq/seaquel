/**
 * /api/account/stream-access authorises the /api/rpc/stream upgrade: a
 * session (the API gate has already checked license and membership) and an
 * Origin this install trusts.
 */

import { describe, expect, it, vi } from "vitest";

vi.mock("$lib/server/auth", () => ({
  // The configured origin, or the install's own (an Origin naming the Host).
  isOriginTrusted: (origin: string | null | undefined, host?: string | null) =>
    origin === "https://app.example.com" || (!!host && origin === `http://${host}`),
}));

const { GET } = await import("./+server");
const { UPGRADE_HOST_HEADER } = await import("$shared/rpc-stream-proxy.js");

type Event = Parameters<typeof GET>[0];

function event(userId: string | null, origin?: string, extra: Record<string, string> = {}): Event {
  const headers: Record<string, string> = { ...extra };
  if (origin) headers.origin = origin;
  return {
    locals: { user: userId ? { id: userId } : null },
    request: new Request("http://localhost/api/account/stream-access", { headers }),
  } as unknown as Event;
}

async function statusOf(run: () => unknown): Promise<number> {
  try {
    const res = await run();
    return (res as Response).status;
  } catch (e) {
    return (e as { status: number }).status;
  }
}

describe("/api/account/stream-access", () => {
  it("answers the user for a session from a trusted origin", async () => {
    const res = (await GET(event("alice", "https://app.example.com"))) as Response;
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ userId: "alice" });
  });

  it("refuses without a session", async () => {
    expect(await statusOf(() => GET(event(null, "https://app.example.com")))).toBe(401);
  });

  it.each([undefined, "https://evil.example", "null", "https://app.example.com.evil.example"])(
    "refuses origin %s with 403",
    async (origin) => {
      expect(await statusOf(() => GET(event("alice", origin)))).toBe(403);
    },
  );

  it("trusts the upgrade's own origin from the Host the proxy passes", async () => {
    // The proxy's lookup arrives from loopback; the browser's Host rides along.
    const res = (await GET(
      event("alice", "http://localhost:8787", {
        host: "127.0.0.1:8787",
        [UPGRADE_HOST_HEADER]: "localhost:8787",
      }),
    )) as Response;
    expect(res.status).toBe(200);
  });

  it("compares with the upgrade's Host, not the lookup's", async () => {
    const run = () =>
      GET(
        event("alice", "http://127.0.0.1:8787", {
          host: "127.0.0.1:8787",
          [UPGRADE_HOST_HEADER]: "localhost:8787",
        }),
      );
    expect(await statusOf(run)).toBe(403);
  });

  it("falls back to the request's Host without the proxy's header", async () => {
    const res = (await GET(
      event("alice", "http://seaquel.test", { host: "seaquel.test" }),
    )) as Response;
    expect(res.status).toBe(200);
  });
});
