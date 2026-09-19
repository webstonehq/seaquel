/**
 * Tests for the unified /api/signup endpoint, focused on the air-gap
 * branch points added in Task 7:
 *
 *   1. Happy path — control plane responsive, normal 200.
 *   2. Transport failure + no bundle → 503 with the exact
 *      { ok: false, error: "control_plane_unreachable_no_bundle" } body
 *      that the frontend (Task 9) keys on.
 *   3. Transport failure + bundle present — the dispatcher in licensing.ts
 *      would have routed to airgap.registerInstallLocal in real code, so
 *      registerInstall doesn't throw. Asserts the 503 is NOT emitted, i.e.
 *      bundle preference wins.
 *   4. Non-transport failure (control plane responded with 4xx) keeps the
 *      existing generic 400 path.
 *
 * Better Auth, the rate-limiter, and every `$lib/server/*` dependency the
 * handler reaches are stubbed via `vi.mock` so the test only exercises the
 * handler's own branching.
 */

import { describe, expect, it, vi, beforeEach } from "vitest";

// ---------------------------------------------------------------------------
// Mocks. Defined before the dynamic `import("./+server")` further down so the
// SvelteKit handler picks them up.
// ---------------------------------------------------------------------------

vi.mock("$lib/server/auth", () => ({
  // CSRF guard — origin trust check. Always true in tests.
  isOriginTrusted: vi.fn(() => true),
  // openAuthDb is only used in the rollback path; return a stub that
  // satisfies `db.prepare(...).run(...)`.
  openAuthDb: vi.fn(() => ({
    prepare: () => ({ run: vi.fn() }),
  })),
  auth: {
    api: {
      signUpEmail: vi.fn(),
    },
  },
}));

vi.mock("$lib/server/rate-limit", () => ({
  checkRateLimit: vi.fn(() => ({ ok: true, retryAfter: 0 })),
}));

vi.mock("$lib/server/licensing", () => ({
  registerInstall: vi.fn(),
  verifyMembershipLicense: vi.fn(),
  bindMember: vi.fn(),
  isNetworkFailure: vi.fn((e: unknown) => e instanceof TypeError),
}));

vi.mock("$lib/server/airgap/bundle-store", () => ({
  isBundleDriven: vi.fn(() => false),
}));

vi.mock("$lib/server/license-cache", () => ({
  readInstallCache: vi.fn(() => null), // first-owner path by default
  writeInstallCache: vi.fn(),
}));

vi.mock("$lib/server/member-license", () => ({
  findByUserId: vi.fn(() => null),
  insert: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Imports after the mocks are registered.
// ---------------------------------------------------------------------------

const auth = await import("$lib/server/auth");
const licensing = await import("$lib/server/licensing");
const bundleStore = await import("$lib/server/airgap/bundle-store");
const licenseCache = await import("$lib/server/license-cache");
const memberLicense = await import("$lib/server/member-license");
const { POST } = await import("./+server");

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

const FAKE_TENANT = {
  tenantId: "tenant_abc",
  slug: "acme",
  status: "active" as const,
  publicUrl: "https://example.test",
  anchorLicenseId: "lic_anchor",
  subscriptionId: "sub_123",
  tier: "team",
  ownerEmail: "owner@example.test",
  seatLimit: 5,
  currentPeriodEnd: "2027-01-01T00:00:00.000Z",
};

const FAKE_VERIFY = {
  ok: true as const,
  subscriptionId: "sub_123",
  tier: "team",
  role: "owner" as const,
};

function makeAuthResponse(userId = "u_test"): Response {
  return new Response(JSON.stringify({ user: { id: userId, email: "a@example.test" } }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

function makeRequest(body: Record<string, unknown>): Request {
  return new Request("http://localhost/api/signup", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      origin: "http://localhost",
    },
    body: JSON.stringify(body),
  });
}

function makeEvent(body: Record<string, unknown>): Parameters<typeof POST>[0] {
  // The handler only touches { request, getClientAddress } so a partial
  // stub is enough — cast through unknown to keep the test isolated from
  // SvelteKit's RequestEvent surface.
  return {
    request: makeRequest(body),
    getClientAddress: () => "127.0.0.1",
  } as unknown as Parameters<typeof POST>[0];
}

const VALID_BODY = {
  email: "new@example.test",
  password: "password123",
  name: "New User",
  licenseKey: "lic_first_owner",
};

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

beforeEach(() => {
  vi.mocked(auth.isOriginTrusted).mockReturnValue(true);
  vi.mocked(bundleStore.isBundleDriven).mockReturnValue(false);
  vi.mocked(licenseCache.readInstallCache).mockReturnValue(null);
  vi.mocked(licenseCache.writeInstallCache).mockReset();
  vi.mocked(memberLicense.findByUserId).mockReturnValue(null);
  vi.mocked(memberLicense.insert).mockReset();
  vi.mocked(licensing.registerInstall).mockReset();
  vi.mocked(licensing.verifyMembershipLicense).mockReset();
  vi.mocked(licensing.bindMember).mockReset();
  vi.mocked(auth.auth.api.signUpEmail).mockReset();
});

describe("POST /api/signup — first-owner air-gap branches", () => {
  it("happy path: control plane responsive → 200", async () => {
    vi.mocked(licensing.registerInstall).mockResolvedValue(FAKE_TENANT);
    vi.mocked(licensing.verifyMembershipLicense).mockResolvedValue(FAKE_VERIFY);
    vi.mocked(licensing.bindMember).mockResolvedValue({ tenantMemberId: "tm_1" });
    // Better Auth's typed return is a {token,user} object; the handler uses
    // asResponse:true so we resolve a real Response. Cast through unknown
    // for the mock's overload-resolution.
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_1") as unknown as never,
    );

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(200);
    expect(licensing.registerInstall).toHaveBeenCalledWith(VALID_BODY.licenseKey);
    // Mode is plumbed: online when no bundle is present.
    expect(licenseCache.writeInstallCache).toHaveBeenCalledWith(FAKE_TENANT, "online");
  });

  it("transport failure + no bundle → 503 control_plane_unreachable_no_bundle", async () => {
    vi.mocked(bundleStore.isBundleDriven).mockReturnValue(false);
    vi.mocked(licensing.registerInstall).mockRejectedValue(new TypeError("fetch failed"));

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(503);
    const body = await res.json();
    expect(body).toEqual({
      ok: false,
      error: "control_plane_unreachable_no_bundle",
    });
    // Should NOT have continued to verify/auth/bind.
    expect(licensing.verifyMembershipLicense).not.toHaveBeenCalled();
    expect(auth.auth.api.signUpEmail).not.toHaveBeenCalled();
  });

  it("bundle present → real code routes through airgap and 503 is NOT emitted", async () => {
    // Simulate the production reality: with a bundle present, the
    // dispatcher in licensing.ts would call registerInstallLocal and
    // succeed offline. So registerInstall resolves normally and writeInstallCache
    // is asked for "airgap" mode.
    vi.mocked(bundleStore.isBundleDriven).mockReturnValue(true);
    vi.mocked(licensing.registerInstall).mockResolvedValue(FAKE_TENANT);
    vi.mocked(licensing.verifyMembershipLicense).mockResolvedValue(FAKE_VERIFY);
    vi.mocked(licensing.bindMember).mockResolvedValue({ tenantMemberId: "tm_2" });
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_2") as unknown as never,
    );

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(200);
    expect(licenseCache.writeInstallCache).toHaveBeenCalledWith(FAKE_TENANT, "airgap");
  });

  it("non-transport failure (4xx from control plane) keeps the 400 path", async () => {
    vi.mocked(bundleStore.isBundleDriven).mockReturnValue(false);
    // Generic Error — isNetworkFailure returns false.
    vi.mocked(licensing.registerInstall).mockRejectedValue(
      new Error("register-install failed: status 400"),
    );

    // SvelteKit's `error(400, ...)` throws an HttpError. In production
    // the framework catches it and serialises to a 400 response; in the
    // unit context we observe the throw directly. Either way the result
    // must NOT be a 503 with the air-gap CTA payload.
    await expect(POST(makeEvent(VALID_BODY))).rejects.toMatchObject({
      status: 400,
      body: { message: "could not register install with the control plane" },
    });
    // No 503 path: verify wasn't even reached.
    expect(licensing.verifyMembershipLicense).not.toHaveBeenCalled();
  });
});
