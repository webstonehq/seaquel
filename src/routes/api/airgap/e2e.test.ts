/**
 * End-to-end integration test for the air-gap flow.
 *
 * Exercises the full lifecycle (bundle upload → signup → revocation
 * → bundle delete → bundle expiry/revalidate → re-upload with revocation)
 * against the REAL airgap module (`verifyBundle`, `bundle-store`,
 * `local-control`, `markRevoked`, `setMode`, `localTenantInfo`,
 * `resolveLicenseState`) and the REAL SvelteKit handlers
 * (`/api/airgap/bundle` POST/DELETE, `/api/signup` POST).
 *
 * Strategy:
 *   - Per-file tempdir auth.db (same DB-redirect trick the other airgap
 *     tests use). Migrations bootstrap the schema via `import.meta.glob`.
 *   - Real Ed25519 signing (`@noble/ed25519`) with the test pubkey
 *     advertised to the verifier via `SEAQUEL_BUNDLE_TRUSTED_PUBKEY`.
 *   - `SEAQUEL_CONTROL_URL` points at `http://127.0.0.1:1` so any
 *     accidental control-plane call would fail. On top of that, global
 *     `fetch` is stubbed to throw immediately — a single fetch through
 *     the spy fails the test.
 *   - Better Auth's `auth.api.signUpEmail` is stubbed (same pattern as
 *     the existing `signup.test.ts`) so the test isn't gated on the
 *     real Better Auth code path; everything BEHIND the stub — the
 *     `insert(memberLicense, ...)` call, install-cache writes, the
 *     verify dispatcher — runs for real.
 *
 * The 11 steps from plan §D.3 are encoded as individual `it(...)` blocks
 * inside one `describe(...)` so vitest preserves their order and each
 * step inherits the side effects of the ones before it.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

// ---------------------------------------------------------------------------
// 1. Per-test-file tempdir + env wiring — must happen before any module
//    that calls `openAuthDb()` is imported.
// ---------------------------------------------------------------------------

const tmp = mkdtempSync(join(tmpdir(), "seaquel-airgap-e2e-"));
process.env.DATA_DIR = tmp;
// Trusted-origin gate for `/api/signup` and `/api/airgap/bundle` POST/DELETE.
process.env.SEAQUEL_TRUSTED_ORIGINS = "http://localhost:5173";
// Point the control plane at an unreachable host. Combined with the
// global `fetch` spy below, any escape from airgap mode is loud.
process.env.SEAQUEL_CONTROL_URL = "http://127.0.0.1:1";

// ---------------------------------------------------------------------------
// 2. Test signing key + trust anchor (Step 1 from the plan).
// ---------------------------------------------------------------------------

const SEED = new Uint8Array([
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
  27, 28, 29, 30, 31,
]);

function hexOf(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

const PUBKEY = await ed.getPublicKeyAsync(SEED);
const FINGERPRINT = await (async () => {
  const { fingerprintPubkey } = await import("$lib/server/airgap/canonical");
  return fingerprintPubkey(PUBKEY);
})();
process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = `${FINGERPRINT}:${hexOf(PUBKEY)}`;

// ---------------------------------------------------------------------------
// 3. Mock Better Auth's `auth.api.signUpEmail` (and the rate limiter)
//    BEFORE the handlers under test are imported, mirroring the pattern
//    in `signup.test.ts`. Everything else — the airgap module, the
//    licensing dispatcher, `member-license` inserts, `setMode`,
//    `markRevoked` — runs for real.
//
//    We DO NOT mock `$lib/server/licensing` or `$lib/server/airgap/*`.
//    The whole point of this file is to exercise them end-to-end.
// ---------------------------------------------------------------------------

vi.mock("$lib/server/rate-limit", () => ({
  checkRateLimit: vi.fn(() => ({ ok: true, retryAfter: 0 })),
  _resetRateLimit: vi.fn(),
}));

// Mutable holder so each `signUpEmail` invocation can return a fresh
// userId. The handler body uses `asResponse: true`; we hand it a
// real `Response` whose JSON exposes the userId we want to bind.
let nextSignupUserId = "u_owner";
function makeAuthResponse(userId: string): Response {
  return new Response(JSON.stringify({ user: { id: userId, email: `${userId}@example.test` } }), {
    status: 200,
    headers: {
      "Content-Type": "application/json",
      // Match the shape Better Auth would emit so the handler's
      // forward of the response carries a session-style cookie.
      "set-cookie": `seaquel.session=session_${userId}; Path=/; HttpOnly`,
    },
  });
}

vi.mock("$lib/server/auth", async () => {
  // Use the REAL `openAuthDb` + `isOriginTrusted` — they're plain
  // helpers that don't pull in Better Auth's runtime. Only `auth` (the
  // Better Auth instance) gets stubbed. The default implementation
  // resolves the most-recent `nextSignupUserId`; each step overrides
  // it via `mockImplementationOnce` inside `withSignupAuthMock`.
  const actual = await vi.importActual<typeof import("$lib/server/auth")>("$lib/server/auth");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const signUpEmail: any = vi.fn(async () => makeAuthResponse(nextSignupUserId));
  return {
    ...actual,
    auth: {
      api: {
        signUpEmail,
      },
    },
  };
});

// ---------------------------------------------------------------------------
// 4. Dynamic imports — must happen AFTER the env vars + mocks are in
//    place so the modules pick them up on first load.
// ---------------------------------------------------------------------------

const { canonicalize } = await import("$lib/server/airgap/canonical");
const { openAuthDb } = await import("$lib/server/auth");
const bundleStore = await import("$lib/server/airgap/bundle-store");
const licenseCache = await import("$lib/server/license-cache");
const memberLicense = await import("$lib/server/member-license");
const licensing = await import("$lib/server/licensing");
const { POST: BUNDLE_POST, DELETE: BUNDLE_DELETE } = await import("./bundle/+server");
const { POST: SIGNUP_POST } = await import("../signup/+server");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// 5. Helpers.
// ---------------------------------------------------------------------------

interface BundleOptions {
  issuedAt?: number;
  notAfter?: number;
  revokedKeys?: string[];
  subscriptionId?: string;
}

async function buildBundleBytes(opts: BundleOptions = {}): Promise<{
  bytes: Uint8Array;
  payload: import("$lib/server/airgap/types").BundlePayload;
}> {
  const issuedAt = opts.issuedAt ?? Math.floor(Date.now() / 1000);
  const notAfter = opts.notAfter ?? issuedAt + 86_400;
  const payload: import("$lib/server/airgap/types").BundlePayload = {
    version: 1,
    issued_at: issuedAt,
    not_before: issuedAt - 60,
    not_after: notAfter,
    subscription_id: opts.subscriptionId ?? "sub_test_001",
    tenant_slug: "self-testbed",
    tier: "business",
    seats: 5,
    seat_tokens: [
      { key: "OWNER-KEY-001", role: "owner" },
      { key: "MEMBER-KEY-001", role: "member" },
    ],
    revoked_keys: opts.revokedKeys ?? [],
    issued_by_install_id: null,
  };
  const canonical = canonicalize(
    payload as unknown as import("$lib/server/airgap/canonical").CanonicalValue,
  );
  const sig = await ed.signAsync(canonical, SEED);
  const b64Url = (bytes: Uint8Array): string => {
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const envelope = {
    payload: b64Url(canonical),
    sig: b64Url(sig),
    pubkey_fingerprint: FINGERPRINT,
  };
  return {
    bytes: new TextEncoder().encode(JSON.stringify(envelope)),
    payload,
  };
}

function makeBundleRequest(bodyBytes: Uint8Array | null, method: "POST" | "DELETE"): Request {
  const init: RequestInit = {
    method,
    headers: { origin: "http://localhost:5173" },
  };
  if (bodyBytes) {
    init.body = new TextDecoder().decode(bodyBytes);
    (init.headers as Record<string, string>)["Content-Type"] = "application/json";
  }
  return new Request("http://localhost:5173/api/airgap/bundle", init);
}

function makeSignupRequest(body: Record<string, unknown>): Request {
  return new Request("http://localhost:5173/api/signup", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      origin: "http://localhost:5173",
    },
    body: JSON.stringify(body),
  });
}

interface LocalsOverride {
  userId?: string;
}

// Loosely-typed event factory. The bundle and signup handlers have
// route-specific RouteParam unions in their `RequestEvent` types, so
// we cast via `unknown` per call site (same pattern used in
// `signup.test.ts` and `bundle.test.ts`). The returned shape is the
// minimal surface both handlers touch — request, getClientAddress,
// locals.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function makeEvent(request: Request, locals: LocalsOverride = {}): any {
  const user = locals.userId
    ? {
        id: locals.userId,
        email: `${locals.userId}@example.test`,
        name: locals.userId,
      }
    : null;
  return {
    request,
    getClientAddress: () => "127.0.0.1",
    locals: {
      user,
      session: null,
      tenant: null,
      licenseState: "unregistered",
    },
  };
}

function insertSession(sessionId: string, userId: string): void {
  const now = new Date().toISOString();
  const expires = new Date(Date.now() + 86_400_000).toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "session" (id, expiresAt, token, createdAt, updatedAt, userId)
       VALUES (?, ?, ?, ?, ?, ?)`,
    )
    .run(sessionId, expires, `tok_${sessionId}`, now, now, userId);
}

function countSessionsFor(userId: string): number {
  const row = openAuthDb()
    .prepare(`SELECT COUNT(*) AS n FROM "session" WHERE userId = ?`)
    .get(userId) as { n: number };
  return row.n;
}

function memberLicenseRowRaw(userId: string): {
  user_id: string;
  is_owner: number;
  revoked_at: number | null;
} | null {
  const row = openAuthDb()
    .prepare(
      `SELECT user_id, is_owner, revoked_at
         FROM member_license
        WHERE user_id = ?`,
    )
    .get(userId) as { user_id: string; is_owner: number; revoked_at: number | null } | undefined;
  return row ?? null;
}

// ---------------------------------------------------------------------------
// 6. Outbound-`fetch` spy — set BEFORE any handler runs.
//
//    The spy throws a `TypeError` to mimic what undici would do when
//    actually trying to connect to `http://127.0.0.1:1` (the unreachable
//    host configured via `SEAQUEL_CONTROL_URL`). `isNetworkFailure(e)`
//    in `licensing.ts` matches on `instanceof TypeError`, so this makes
//    step 3 (signup without a bundle) produce the 503
//    `control_plane_unreachable_no_bundle` response.
//
//    From step 4 onward — once a bundle is loaded — the dispatcher in
//    `licensing.ts` routes every call to the local airgap path and
//    `fetch` must NEVER fire. Each of those steps clears the spy and
//    asserts `not.toHaveBeenCalled()` at the end. A regression that
//    accidentally hits the network in airgap mode will still produce a
//    `TypeError` (which is the right "network fail" behaviour) but the
//    spy assertion will flag the call.
// ---------------------------------------------------------------------------

const fetchSpy = vi.fn(() => {
  throw new TypeError("fetch should not be called in airgap mode");
});

beforeAll(() => {
  vi.stubGlobal("fetch", fetchSpy);
  // Seed Better Auth's `user` table with rows for the userIds we'll
  // bind. The real `signUpEmail` would do this; with the mock we have
  // to insert by hand so the FK constraint on `session.userId` is
  // satisfied during the revocation walk. Inserts happen lazily inside
  // each step that needs them — see the individual `it(...)` blocks.
});

afterAll(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// 7. Pre-insert helper: signup creates a Better Auth `user` row in the
//    real flow. Our stub returns a userId but does NOT create the row,
//    so we INSERT it manually before each signup call. This is the
//    only "fake" we maintain — the rest of the flow (member_license
//    insert, install_cache writes, bundle verify, revocation walk)
//    runs against the real code.
// ---------------------------------------------------------------------------

function ensureUser(userId: string, email: string): void {
  const exists = openAuthDb().prepare(`SELECT 1 FROM "user" WHERE id = ?`).get(userId);
  if (exists) return;
  const now = new Date().toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, email, name, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    )
    .run(userId, email, userId, now, now);
}

// Wrap `signUpEmail` so the FK-satisfying user row is inserted right
// before the response is constructed. This keeps the per-step setup
// minimal — each test just sets `nextSignupUserId` and the user row
// materialises automatically.
async function withSignupAuthMock(
  userId: string,
  run: () => Response | Promise<Response>,
): Promise<Response> {
  const auth = await import("$lib/server/auth");
  nextSignupUserId = userId;
  // The Better Auth `signUpEmail` return type is the typed
  // `{token,user}` object; with `asResponse:true` the runtime hands us
  // a `Response` instead. Cast through `unknown as never` (same dance
  // signup.test.ts uses) so the mock-overload resolution accepts the
  // Response value.
  vi.mocked(auth.auth.api.signUpEmail).mockImplementationOnce((async () => {
    ensureUser(userId, `${userId}@example.test`);
    return makeAuthResponse(userId);
  }) as unknown as never);
  return await run();
}

// ---------------------------------------------------------------------------
// 8. The 11-step scenario.
// ---------------------------------------------------------------------------

describe("airgap E2E — bundle import → signup → revoke → expiry", () => {
  it("step 3: signup without a bundle → 503 control_plane_unreachable_no_bundle", async () => {
    // Step 2 (build the bundle) is folded into the helpers above; we
    // don't need to materialise it until step 4. This `it` is the
    // pre-bundle baseline — `registerInstall` hits the unreachable
    // control URL through real `fetch`, which our spy turns into a
    // throw. The spy throws synchronously inside the fetch call, so
    // `licensingFetch` propagates the error → `registerInstall` throws
    // → the signup handler catches via `isNetworkFailure(e)` and emits
    // the 503.
    expect(bundleStore.isBundleDriven()).toBe(false);

    const req = makeSignupRequest({
      email: "outside@example.test",
      password: "password123",
      name: "Outside",
      licenseKey: "OWNER-KEY-001",
    });
    const res = await SIGNUP_POST(makeEvent(req));

    expect(res.status).toBe(503);
    expect(await res.json()).toEqual({
      ok: false,
      error: "control_plane_unreachable_no_bundle",
    });
    // The fetch attempt did fire (and threw via the spy) — that's the
    // 503-trigger. We deliberately do NOT assert call count here; the
    // strict no-fetch invariant kicks in once the bundle is loaded.
  });

  it("step 4: upload the bundle → 200, mode flips to airgap", async () => {
    fetchSpy.mockClear();
    const { bytes, payload } = await buildBundleBytes();
    const res = await BUNDLE_POST(makeEvent(makeBundleRequest(bytes, "POST")));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toMatchObject({
      ok: true,
      unchanged: false,
      tier: payload.tier,
      seats: payload.seats,
    });
    expect(licenseCache.readInstallCache()?.mode).toBe("airgap");
    expect(bundleStore.isBundleDriven()).toBe(true);

    // From step 4 onward, no fetch should have been called.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 5: signup with owner key → 200 + member_license is_owner=1", async () => {
    fetchSpy.mockClear();
    const req = makeSignupRequest({
      email: "owner@example.test",
      password: "password123",
      name: "Owner",
      licenseKey: "OWNER-KEY-001",
    });
    const res = await withSignupAuthMock("u_owner", () => SIGNUP_POST(makeEvent(req)));
    expect(res.status).toBe(200);

    const row = memberLicenseRowRaw("u_owner");
    expect(row).not.toBeNull();
    expect(row!.is_owner).toBe(1);
    expect(row!.revoked_at).toBeNull();

    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 6: signup with member key → 200 + second member_license is_owner=0", async () => {
    fetchSpy.mockClear();
    const req = makeSignupRequest({
      email: "member@example.test",
      password: "password123",
      name: "Member",
      licenseKey: "MEMBER-KEY-001",
    });
    const res = await withSignupAuthMock("u_member", () => SIGNUP_POST(makeEvent(req)));
    expect(res.status).toBe(200);

    const row = memberLicenseRowRaw("u_member");
    expect(row).not.toBeNull();
    expect(row!.is_owner).toBe(0);
    expect(row!.revoked_at).toBeNull();

    const allRows = openAuthDb().prepare(`SELECT COUNT(*) AS n FROM member_license`).get() as {
      n: number;
    };
    expect(allRows.n).toBe(2);

    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 7: signup with a random key → 400 license_not_found", async () => {
    fetchSpy.mockClear();
    const req = makeSignupRequest({
      email: "random@example.test",
      password: "password123",
      name: "Random",
      licenseKey: "RANDOM-NOT-IN-BUNDLE",
    });
    const res = await SIGNUP_POST(makeEvent(req));

    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body).toMatchObject({ ok: false, error: "license_not_found" });

    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 8: fetch spy was not invoked across steps 4–7", () => {
    // Independent witness: the cumulative call count across the
    // bundle-driven steps (4, 5, 6, 7) is still zero. Each step
    // also asserts this locally, but consolidating it here documents
    // the invariant explicitly.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 9: DELETE the bundle as owner → 200, mode=online, member_licenses preserved", async () => {
    fetchSpy.mockClear();
    // Insert a session for the owner so the DELETE handler's auth
    // gate (locals.user + member_license owner row) passes. We bypass
    // the actual Better Auth session lookup by injecting `locals.user`
    // directly via makeEvent — same shape `handleAuth` would produce
    // for a real signed-in owner.
    insertSession("sess_owner_1", "u_owner");

    const res = await BUNDLE_DELETE(
      makeEvent(makeBundleRequest(null, "DELETE"), { userId: "u_owner" }),
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });

    expect(licenseCache.readInstallCache()?.mode).toBe("online");
    expect(await bundleStore.readActiveBundle()).toBeNull();

    // Both member_license rows survive the delete.
    const owner = memberLicense.findByUserId("u_owner");
    const member = memberLicense.findByUserId("u_member");
    expect(owner).not.toBeNull();
    expect(member).not.toBeNull();
    expect(owner!.licenseKey).toBe("OWNER-KEY-001");
    expect(member!.licenseKey).toBe("MEMBER-KEY-001");

    // DELETE doesn't reach the control plane — still no fetch.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 10: re-upload an expired bundle → resolveLicenseState returns 'revalidate'", async () => {
    fetchSpy.mockClear();
    // Build a bundle whose `not_after` is in the past so
    // `localTenantInfo()` returns null.
    const now = Math.floor(Date.now() / 1000);
    const { bytes } = await buildBundleBytes({
      issuedAt: now - 2000,
      notAfter: now - 1000,
    });

    // The owner is still signed in; the install_cache row was created
    // by step 5 (registerInstall via airgap.registerInstallLocal) and
    // persisted across step 9 (DELETE only flips `mode`).
    const res = await BUNDLE_POST(
      makeEvent(makeBundleRequest(bytes, "POST"), { userId: "u_owner" }),
    );
    // Task 1's verifier accepts an expired bundle — the caller decides
    // what to do with `not_after`. So the upload still 200s; the
    // resolveLicenseState ladder below is what catches the expiry.
    expect(res.status).toBe(200);

    // Force the resolve ladder into the "fell off grace" branch:
    //   - softFresh fails because lastValidatedAt is old enough.
    //   - tenantInfo() returns null (bundle is past `not_after`).
    //   - grace check fails because we rewrite grace_until into the past.
    openAuthDb()
      .prepare(
        `UPDATE install_cache
            SET last_validated_at = ?,
                grace_until       = ?
          WHERE id = 1`,
      )
      .run(now - 10 * 86_400, now - 1);

    const state = await licensing.resolveLicenseState();
    expect(state.kind).toBe("revalidate");

    // Still airgap-driven → no control-plane fetch.
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 11: re-upload bundle with revocation → member is revoked, sessions purged, owner untouched", async () => {
    fetchSpy.mockClear();
    // Seed sessions to prove the revocation walk purges them.
    insertSession("sess_member_1", "u_member");
    insertSession("sess_member_2", "u_member");
    insertSession("sess_owner_2", "u_owner");
    expect(countSessionsFor("u_member")).toBe(2);
    expect(countSessionsFor("u_owner")).toBeGreaterThanOrEqual(1);

    // Fresh bundle with a strictly-newer issued_at to clear the
    // bundle-older-than-current replay guard, and with MEMBER-KEY-001
    // in `revoked_keys`.
    const now = Math.floor(Date.now() / 1000);
    const { bytes } = await buildBundleBytes({
      issuedAt: now + 10,
      notAfter: now + 86_400,
      revokedKeys: ["MEMBER-KEY-001"],
    });

    const res = await BUNDLE_POST(
      makeEvent(makeBundleRequest(bytes, "POST"), { userId: "u_owner" }),
    );
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toMatchObject({
      ok: true,
      rowsRevoked: 1,
      revokedKeyCount: 1,
    });

    // Member row stamped, owner untouched.
    const member = memberLicense.findByUserId("u_member");
    const owner = memberLicense.findByUserId("u_owner");
    expect(member).not.toBeNull();
    expect(member!.revokedAt).not.toBeNull();
    expect(owner!.revokedAt).toBeNull();

    // Member sessions gone; owner sessions stay.
    expect(countSessionsFor("u_member")).toBe(0);
    expect(countSessionsFor("u_owner")).toBeGreaterThanOrEqual(1);

    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("step 11 (continued): revoked member would fail the gate's membership check", () => {
    // The `handleApiGate` middleware checks `findByUserId(locals.user.id)`
    // and treats `revokedAt != null` as "not a bound member of this
    // install", returning 403. We assert the data-side invariant the
    // gate consumes — the gate itself is exercised in
    // `src/hooks.server.test.ts`.
    const revokedMember = memberLicense.findByUserId("u_member");
    expect(revokedMember).not.toBeNull();
    expect(revokedMember!.revokedAt).not.toBeNull();
    // Mirror the exact condition the gate runs:
    const blockedByGate = !revokedMember || revokedMember.revokedAt != null;
    expect(blockedByGate).toBe(true);
  });
});
