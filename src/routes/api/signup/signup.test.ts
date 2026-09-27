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

vi.mock("$lib/server/license-client", () => ({
  installStatus: vi.fn(async () => ({ hasTenant: false, bundlePresent: false })),
  registerInstall: vi.fn(),
  signupCheck: vi.fn(),
  bindMember: vi.fn(),
  // The real rule: Rust's NETWORK_ERROR code.
  isNetworkFailure: vi.fn((e: unknown) => (e as { code?: string })?.code === "NETWORK_ERROR"),
}));

// ---------------------------------------------------------------------------
// Imports after the mocks are registered.
// ---------------------------------------------------------------------------

const auth = await import("$lib/server/auth");
const licensing = await import("$lib/server/license-client");
const { POST } = await import("./+server");

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/** A license-client error with Rust's code. */
function licenseError(code: string, message: string): Error {
  return Object.assign(new Error(message), { code });
}

function status(hasTenant: boolean, bundlePresent: boolean) {
  return { hasTenant, bundlePresent };
}

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
  vi.mocked(licensing.installStatus).mockReset();
  vi.mocked(licensing.installStatus).mockResolvedValue(status(false, false));
  vi.mocked(licensing.registerInstall).mockReset();
  vi.mocked(licensing.signupCheck).mockReset();
  vi.mocked(licensing.bindMember).mockReset();
  vi.mocked(licensing.bindMember).mockResolvedValue({ bound: true });
  vi.mocked(auth.auth.api.signUpEmail).mockReset();
});

describe("POST /api/signup — first-owner air-gap branches", () => {
  it("happy path: control plane responsive → 200", async () => {
    vi.mocked(licensing.registerInstall).mockResolvedValue(undefined);
    vi.mocked(licensing.signupCheck).mockResolvedValue(FAKE_VERIFY);
    // Better Auth's typed return is a {token,user} object; the handler uses
    // asResponse:true so we resolve a real Response. Cast through unknown
    // for the mock's overload-resolution.
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_1") as unknown as never,
    );

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(200);
    // Rust registers and writes install_cache in the current mode.
    expect(licensing.registerInstall).toHaveBeenCalledWith(VALID_BODY.licenseKey);
    expect(licensing.signupCheck).toHaveBeenCalledWith(VALID_BODY.licenseKey, VALID_BODY.email);
    // The first owner binds with their own key and becomes the owner row.
    expect(licensing.bindMember).toHaveBeenCalledWith({
      licenseKey: VALID_BODY.licenseKey,
      userId: "u_1",
      email: VALID_BODY.email,
      role: "owner",
      firstOwner: true,
    });
  });

  it("transport failure + no bundle → 503 control_plane_unreachable_no_bundle", async () => {
    vi.mocked(licensing.registerInstall).mockRejectedValue(
      licenseError("NETWORK_ERROR", "control plane unreachable"),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(503);
    const body = await res.json();
    expect(body).toEqual({
      ok: false,
      error: "control_plane_unreachable_no_bundle",
    });
    // Should NOT have continued to verify/auth/bind.
    expect(licensing.signupCheck).not.toHaveBeenCalled();
    expect(auth.auth.api.signUpEmail).not.toHaveBeenCalled();
    errors.mockRestore();
  });

  it("transport failure with a bundle stored since → the 400 path, not the 503", async () => {
    // The bundle check runs after the failure, as isBundleDriven() did.
    vi.mocked(licensing.installStatus)
      .mockResolvedValueOnce(status(false, false))
      .mockResolvedValueOnce(status(false, true));
    vi.mocked(licensing.registerInstall).mockRejectedValue(
      licenseError("NETWORK_ERROR", "control plane unreachable"),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(POST(makeEvent(VALID_BODY))).rejects.toMatchObject({ status: 400 });
    errors.mockRestore();
  });

  it("bundle present → Rust routes through the bundle and 503 is NOT emitted", async () => {
    vi.mocked(licensing.installStatus).mockResolvedValue(status(false, true));
    vi.mocked(licensing.registerInstall).mockResolvedValue(undefined);
    vi.mocked(licensing.signupCheck).mockResolvedValue(FAKE_VERIFY);
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_2") as unknown as never,
    );

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(200);
    expect(licensing.registerInstall).toHaveBeenCalledTimes(1);
  });

  it("non-transport failure (4xx from control plane) keeps the 400 path", async () => {
    vi.mocked(licensing.registerInstall).mockRejectedValue(
      licenseError("CONTROL_PLANE_ERROR", "register-install failed: 400"),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});

    // SvelteKit's `error(400, ...)` throws an HttpError. In production
    // the framework catches it and serialises to a 400 response; in the
    // unit context we observe the throw directly. Either way the result
    // must NOT be a 503 with the air-gap CTA payload.
    await expect(POST(makeEvent(VALID_BODY))).rejects.toMatchObject({
      status: 400,
      body: { message: "could not register install with the control plane" },
    });
    // No 503 path: verify wasn't even reached.
    expect(licensing.signupCheck).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});

describe("POST /api/signup — later signups", () => {
  it("an established install skips registration and binds as a member", async () => {
    vi.mocked(licensing.installStatus).mockResolvedValue(status(true, false));
    vi.mocked(licensing.signupCheck).mockResolvedValue({ ...FAKE_VERIFY, role: "member" });
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_3") as unknown as never,
    );

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(200);
    expect(licensing.registerInstall).not.toHaveBeenCalled();
    expect(licensing.bindMember).toHaveBeenCalledWith(
      expect.objectContaining({ userId: "u_3", role: "member", firstOwner: false }),
    );
  });

  it("a network failure verifying a later signup is a 500, not the 503", async () => {
    vi.mocked(licensing.installStatus).mockResolvedValue(status(true, false));
    vi.mocked(licensing.signupCheck).mockRejectedValue(
      licenseError("NETWORK_ERROR", "control plane unreachable"),
    );
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(POST(makeEvent(VALID_BODY))).rejects.toMatchObject({ status: 500 });
    errors.mockRestore();
  });

  it("a refused key is a 400 with the control plane's error", async () => {
    vi.mocked(licensing.installStatus).mockResolvedValue(status(true, false));
    vi.mocked(licensing.signupCheck).mockResolvedValue({ ok: false, error: "license_inactive" });

    const res = await POST(makeEvent(VALID_BODY));

    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ ok: false, error: "license_inactive" });
    expect(auth.auth.api.signUpEmail).not.toHaveBeenCalled();
  });

  it("a failed bind rolls the Better Auth user back", async () => {
    vi.mocked(licensing.installStatus).mockResolvedValue(status(true, false));
    vi.mocked(licensing.signupCheck).mockResolvedValue(FAKE_VERIFY);
    vi.mocked(auth.auth.api.signUpEmail).mockResolvedValue(
      makeAuthResponse("u_4") as unknown as never,
    );
    vi.mocked(licensing.bindMember).mockRejectedValue(
      licenseError("CONTROL_PLANE_ERROR", "bind-member failed: 409 taken"),
    );
    const run = vi.fn();
    vi.mocked(auth.openAuthDb).mockReturnValue({
      prepare: () => ({ run }),
    } as unknown as ReturnType<typeof auth.openAuthDb>);
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});

    await expect(POST(makeEvent(VALID_BODY))).rejects.toMatchObject({ status: 500 });
    expect(run).toHaveBeenCalledWith("u_4");
    errors.mockRestore();
  });
});

describe("POST /api/signup — Origin check", () => {
  it("checks the Origin against the request's own Host", async () => {
    vi.mocked(auth.isOriginTrusted).mockReturnValue(false);
    const request = new Request("http://seaquel.test/api/signup", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        origin: "https://evil.example",
        host: "seaquel.test",
      },
      body: JSON.stringify(VALID_BODY),
    });
    const event = {
      request,
      getClientAddress: () => "127.0.0.1",
    } as unknown as Parameters<typeof POST>[0];

    await expect(POST(event)).rejects.toMatchObject({ status: 403 });
    expect(auth.isOriginTrusted).toHaveBeenCalledWith("https://evil.example", "seaquel.test");
    expect(licensing.registerInstall).not.toHaveBeenCalled();
  });
});
