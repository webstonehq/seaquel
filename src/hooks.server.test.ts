/**
 * `handle`'s licensing behaviour: `/health` never reaches the license
 * service, and an unreachable license service fails closed with a 503
 * (JSON for the API, a page otherwise) instead of a generic 500.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.stubEnv("VITE_BUILD_TARGET", "web");
vi.mock("$app/environment", () => ({ building: false, dev: false }));
vi.mock("$lib/paraglide/server", () => ({
  paraglideMiddleware: (
    request: Request,
    fn: (a: { request: Request; locale: string }) => unknown,
  ) => fn({ request, locale: "en" }),
}));
// SvelteKit's `sequence` needs its request store; chain the handles plainly.
vi.mock("@sveltejs/kit/hooks", () => ({
  sequence:
    (...handles: Array<(i: { event: unknown; resolve: (e: unknown) => unknown }) => unknown>) =>
    ({ event, resolve }: { event: unknown; resolve: (e: unknown) => unknown }) => {
      const at = (i: number): unknown =>
        i === handles.length ? resolve(event) : handles[i]({ event, resolve: () => at(i + 1) });
      return at(0);
    },
}));
vi.mock("$lib/server/auth", () => ({
  auth: { api: { getSession: vi.fn(async () => null) } },
}));
vi.mock("$lib/server/license-client", async () => {
  const actual = await vi.importActual<typeof import("$lib/server/license-client")>(
    "$lib/server/license-client",
  );
  return { LicenseClientError: actual.LicenseClientError, gate: vi.fn() };
});

const client = await import("$lib/server/license-client");
const { handle } = await import("./hooks.server");

function run(path: string, init?: RequestInit) {
  const url = new URL(`http://localhost${path}`);
  const event = { url, request: new Request(url, init), locals: {} } as never;
  const resolve = vi.fn(async () => new Response("resolved"));
  return { result: handle({ event, resolve } as never), resolve };
}

beforeEach(() => {
  vi.mocked(client.gate).mockReset();
});

describe("handle", () => {
  it("/health answers without the license service", async () => {
    vi.mocked(client.gate).mockRejectedValue(new Error("must not be called"));
    const { result, resolve } = run("/health");
    expect(await (await result).text()).toBe("resolved");
    expect(resolve).toHaveBeenCalled();
    expect(client.gate).not.toHaveBeenCalled();
  });

  it("an unreachable license service is a 503, JSON on /api", async () => {
    vi.mocked(client.gate).mockRejectedValue(
      new client.LicenseClientError("UPSTREAM_UNAVAILABLE", "unavailable", 502),
    );
    const api = await run("/api/rpc").result;
    expect(api.status).toBe(503);
    expect(await api.json()).toEqual({ code: "license_service_unavailable" });
    const page = await run("/").result;
    expect(page.status).toBe(503);
    expect(await page.text()).toContain("license service isn't responding");
  });

  it("other license errors still fail (no quiet revalidate)", async () => {
    vi.mocked(client.gate).mockRejectedValue(
      new client.LicenseClientError("LICENSE_DB_ERROR", "disk full", 500),
    );
    await expect(run("/").result).rejects.toMatchObject({ code: "LICENSE_DB_ERROR" });
  });

  it("a working gate lets the request through", async () => {
    vi.mocked(client.gate).mockResolvedValue({
      state: "unregistered",
      tenant: null,
      member: null,
      hasTenant: false,
      bundlePresent: false,
    });
    const { result, resolve } = run("/login");
    expect(await (await result).text()).toBe("resolved");
    expect(resolve).toHaveBeenCalled();
  });

  it("refuses a cross-site /api mutation before the session or license", async () => {
    vi.mocked(client.gate).mockRejectedValue(new Error("must not be called"));
    const { result, resolve } = run("/api/team/u_2", {
      method: "DELETE",
      headers: { origin: "https://evil.example", host: "localhost" },
    });
    const res = await result;
    expect(res.status).toBe(403);
    expect(await res.json()).toMatchObject({ code: "ORIGIN_NOT_ALLOWED" });
    expect(resolve).not.toHaveBeenCalled();
  });

  it("lets a same-origin /api mutation through to the gate", async () => {
    vi.mocked(client.gate).mockResolvedValue({
      state: "unregistered",
      tenant: null,
      member: null,
      hasTenant: false,
      bundlePresent: false,
    });
    const res = await run("/api/rpc", {
      method: "POST",
      headers: { origin: "http://localhost", host: "localhost" },
      body: "{}",
    }).result;
    // No session: the API gate's 401, not the Origin gate's 403.
    expect(res.status).toBe(401);
  });
});
