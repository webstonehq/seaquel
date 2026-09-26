/**
 * The air-gap flow, Node side: bundle upload → signup → revocation →
 * bundle delete → expiry/revalidate → re-upload with revocation, through
 * the REAL SvelteKit handlers (`/api/airgap/bundle` POST/DELETE,
 * `/api/signup` POST) and the `(app)` layout's redirects.
 *
 * Licensing itself runs in the Rust service now. Its half of every step
 * (verification, the bundle row, `member_license`, the install cache, the
 * TTL ladder) is tested end to end against the real code in
 * `crates/seaquel-server/tests/internal_license.rs` (`air_gap_lifecycle`,
 * with auth.db built from the same migrations). Starting the Rust binary
 * from vitest would mean compiling the whole server (DuckDB included) for
 * `vitest run`, so here `license-client` is a small in-memory stand-in with
 * the answers Rust gives, and this file checks what Node still owns:
 *
 *   - the signup route's order of steps, its 503 CTA and its rollback;
 *   - Better Auth (stubbed, as before) and the session purge on revocation,
 *     against a real temp auth.db;
 *   - the redirects in `(app)/+layout.server.ts` for every license state.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, describe, expect, it, vi } from "vitest";

// ---------------------------------------------------------------------------
// 1. Per-test-file tempdir + env wiring — before anything opens auth.db.
// ---------------------------------------------------------------------------

const tmp = mkdtempSync(join(tmpdir(), "seaquel-airgap-e2e-"));
process.env.DATA_DIR = tmp;
process.env.SEAQUEL_TRUSTED_ORIGINS = "http://localhost:5173";
vi.stubEnv("VITE_BUILD_TARGET", "web");

// ---------------------------------------------------------------------------
// 2. The Rust service's answers, from a small model of its state.
// ---------------------------------------------------------------------------

const rust = vi.hoisted(() => {
  const OWNER = "OWNER-KEY-001";
  const MEMBER = "MEMBER-KEY-001";
  const state = {
    bundle: null as null | { issuedAt: number; notAfter: number; revoked: string[] },
    hasTenant: false,
    members: new Map<string, { key: string; isOwner: boolean; revoked: boolean }>(),
    gateState: "unregistered" as "ok" | "suspended" | "revalidate" | "unregistered",
    failNextBind: false,
    calls: [] as string[],
  };
  const err = (code: string, message: string, status: number) =>
    Object.assign(new Error(message), { code, status });
  return { OWNER, MEMBER, state, err };
});

vi.mock("$lib/server/license-client", async () => {
  const actual = await vi.importActual<typeof import("$lib/server/license-client")>(
    "$lib/server/license-client",
  );
  const { state, err, OWNER, MEMBER } = rust;
  const seats: Record<string, "owner" | "member"> = { [OWNER]: "owner", [MEMBER]: "member" };
  const member = (userId: string | null) => {
    const m = userId ? state.members.get(userId) : undefined;
    return m ? { isOwner: m.isOwner, revoked: m.revoked } : null;
  };
  const requireOwner = (userId: string | null) => {
    if (!userId) throw new actual.LicenseClientError("UNAUTHORIZED", "must be signed in", 401);
    const m = state.members.get(userId);
    if (!m || !m.isOwner || m.revoked) {
      throw new actual.LicenseClientError("FORBIDDEN", "owner only", 403);
    }
  };
  return {
    LicenseClientError: actual.LicenseClientError,
    isNetworkFailure: actual.isNetworkFailure,
    invalidateGate: () => {},
    gate: async (userId: string | null) => {
      state.calls.push("gate");
      return {
        state: state.gateState,
        tenant: null,
        member: member(userId),
        hasTenant: state.hasTenant,
        bundlePresent: !!state.bundle,
      };
    },
    installStatus: async () => {
      state.calls.push("installStatus");
      return { hasTenant: state.hasTenant, bundlePresent: !!state.bundle };
    },
    registerInstall: async (key: string) => {
      state.calls.push("registerInstall");
      if (!state.bundle) {
        throw new actual.LicenseClientError("NETWORK_ERROR", "control plane unreachable", 502);
      }
      if (seats[key] !== "owner") throw err("LICENSE_NOT_FOUND", "license_not_found", 400);
      state.hasTenant = true;
      state.gateState = "ok";
    },
    signupCheck: async (key: string) => {
      state.calls.push("signupCheck");
      if (!state.bundle || !seats[key]) return { ok: false, error: "license_not_found" };
      if ([...state.members.values()].some((m) => m.key === key)) {
        return { ok: false, error: "license_already_in_other_tenant" };
      }
      return { ok: true, subscriptionId: "sub_test_001", tier: "business", role: seats[key] };
    },
    bindMember: async (args: { licenseKey: string; userId: string; firstOwner: boolean }) => {
      state.calls.push("bindMember");
      if (state.failNextBind) {
        state.failNextBind = false;
        throw new actual.LicenseClientError("CONTROL_PLANE_ERROR", "bind-member failed: 500", 502);
      }
      if (state.members.has(args.userId)) return { bound: false };
      state.members.set(args.userId, {
        key: args.licenseKey,
        isOwner: args.firstOwner,
        revoked: false,
      });
      return { bound: true };
    },
    airgapUpload: async (bytes: Uint8Array, userId: string | null) => {
      state.calls.push("airgapUpload");
      const bundle = JSON.parse(new TextDecoder().decode(bytes)) as {
        issuedAt: number;
        notAfter: number;
        revoked: string[];
      };
      if (state.hasTenant) requireOwner(userId);
      if (state.bundle && bundle.issuedAt < state.bundle.issuedAt) {
        return {
          status: 409,
          body: { ok: false, error: "bundle_older_than_current" },
          revokedUserIds: [],
        };
      }
      state.bundle = bundle;
      const revokedUserIds: string[] = [];
      for (const [id, m] of state.members) {
        if (!m.revoked && bundle.revoked.includes(m.key)) {
          m.revoked = true;
          revokedUserIds.push(id);
        }
      }
      return {
        status: 200,
        body: { ok: true, unchanged: false, rowsRevoked: revokedUserIds.length },
        revokedUserIds,
      };
    },
    airgapClear: async (userId: string | null) => {
      state.calls.push("airgapClear");
      requireOwner(userId);
      state.bundle = null;
    },
  };
});

// ---------------------------------------------------------------------------
// 3. Better Auth's `signUpEmail` and the rate limiter are stubbed; the real
//    `openAuthDb` (temp auth.db, real migrations) backs the session purge
//    and the rollback.
// ---------------------------------------------------------------------------

vi.mock("$lib/server/rate-limit", () => ({
  checkRateLimit: vi.fn(() => ({ ok: true, retryAfter: 0 })),
  _resetRateLimit: vi.fn(),
}));

vi.mock("$lib/server/auth", async () => {
  const actual = await vi.importActual<typeof import("$lib/server/auth")>("$lib/server/auth");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const signUpEmail: any = vi.fn();
  return { ...actual, auth: { api: { signUpEmail } } };
});

vi.mock("$app/environment", () => ({ building: false }));

const { openAuthDb } = await import("$lib/server/auth");
const auth = await import("$lib/server/auth");
const { POST: BUNDLE_POST, DELETE: BUNDLE_DELETE } = await import("./bundle/+server");
const { POST: SIGNUP_POST } = await import("../signup/+server");
const { load: layoutLoad } = await import("../../(app)/+layout.server");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// 4. Helpers.
// ---------------------------------------------------------------------------

function bundleBytes(opts: { issuedAt?: number; notAfter?: number; revoked?: string[] } = {}) {
  const issuedAt = opts.issuedAt ?? Math.floor(Date.now() / 1000);
  return new TextEncoder().encode(
    JSON.stringify({
      issuedAt,
      notAfter: opts.notAfter ?? issuedAt + 86_400,
      revoked: opts.revoked ?? [],
    }),
  );
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function makeEvent(request: Request, userId: string | null = null): any {
  const user = userId ? { id: userId, email: `${userId}@example.test`, name: userId } : null;
  return {
    request,
    getClientAddress: () => "127.0.0.1",
    locals: { user, session: null, tenant: null, licenseState: rust.state.gateState },
  };
}

function bundleRequest(body: Uint8Array | null, method: "POST" | "DELETE"): Request {
  return new Request("http://localhost:5173/api/airgap/bundle", {
    method,
    headers: { origin: "http://localhost:5173", "content-type": "application/json" },
    body: body ? new TextDecoder().decode(body) : undefined,
  });
}

function signupRequest(licenseKey: string, email: string): Request {
  return new Request("http://localhost:5173/api/signup", {
    method: "POST",
    headers: { "content-type": "application/json", origin: "http://localhost:5173" },
    body: JSON.stringify({ email, password: "password123", name: email, licenseKey }),
  });
}

function ensureUser(userId: string): void {
  const exists = openAuthDb().prepare(`SELECT 1 FROM "user" WHERE id = ?`).get(userId);
  if (exists) return;
  const now = new Date().toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, email, name, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    )
    .run(userId, `${userId}@example.test`, userId, now, now);
}

/** Better Auth's signUpEmail, for one call: makes the user row. */
function nextSignup(userId: string): void {
  vi.mocked(auth.auth.api.signUpEmail).mockImplementationOnce((async () => {
    ensureUser(userId);
    return new Response(JSON.stringify({ user: { id: userId } }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as unknown as never);
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
  return (
    openAuthDb().prepare(`SELECT COUNT(*) AS n FROM "session" WHERE userId = ?`).get(userId) as {
      n: number;
    }
  ).n;
}

/** The layout's answer: its redirect location, or "render". */
async function layoutFor(userId: string | null, path = "/"): Promise<string> {
  const url = new URL(`http://localhost:5173${path}`);
  try {
    await layoutLoad({ ...makeEvent(new Request(url), userId), url } as never);
    return "render";
  } catch (e) {
    const r = e as { status?: number; location?: string };
    if (r.status === 302 && r.location) return r.location;
    throw e;
  }
}

// ---------------------------------------------------------------------------
// 5. The scenario.
// ---------------------------------------------------------------------------

describe("airgap E2E (Node side) — bundle import → signup → revoke → expiry", () => {
  it("before anything: signed-in users land on /airgap-setup, then /signup", async () => {
    expect(await layoutFor(null, "/")).toBe("/login?redirect=%2F");
    expect(await layoutFor("u_x", "/")).toBe("/airgap-setup?redirect=%2F");
    expect(await layoutFor("u_x", "/airgap-setup")).toBe("render");
    expect(await layoutFor("u_x", "/signup")).toBe("render");
  });

  it("step 3: signup without a bundle → 503 control_plane_unreachable_no_bundle", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const res = await SIGNUP_POST(makeEvent(signupRequest(rust.OWNER, "outside@example.test")));
    errors.mockRestore();
    expect(res.status).toBe(503);
    expect(await res.json()).toEqual({ ok: false, error: "control_plane_unreachable_no_bundle" });
    expect(auth.auth.api.signUpEmail).not.toHaveBeenCalled();
  });

  it("step 4: upload the bundle on the fresh install, signed out → 200", async () => {
    const res = await BUNDLE_POST(makeEvent(bundleRequest(bundleBytes(), "POST")));
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ ok: true, unchanged: false });
    // With a bundle but no tenant, signed-in users go to /signup.
    expect(await layoutFor("u_x", "/")).toBe("/signup?redirect=%2F");
  });

  it("step 5: the owner signs up → 200, bound as the owner", async () => {
    rust.state.calls = [];
    nextSignup("u_owner");
    const res = await SIGNUP_POST(makeEvent(signupRequest(rust.OWNER, "owner@example.test")));
    expect(res.status).toBe(200);
    // The route's order of steps, unchanged.
    expect(rust.state.calls).toEqual([
      "installStatus",
      "registerInstall",
      "signupCheck",
      "bindMember",
    ]);
    expect(rust.state.members.get("u_owner")).toMatchObject({ isOwner: true });
    expect(await layoutFor("u_owner", "/")).toBe("render");
  });

  it("step 6: a member signs up → 200, not the owner, no registration", async () => {
    rust.state.calls = [];
    nextSignup("u_member");
    const res = await SIGNUP_POST(makeEvent(signupRequest(rust.MEMBER, "member@example.test")));
    expect(res.status).toBe(200);
    expect(rust.state.calls).toEqual(["installStatus", "signupCheck", "bindMember"]);
    expect(rust.state.members.get("u_member")).toMatchObject({ isOwner: false });
  });

  it("step 7: a key that isn't in the bundle → 400 license_not_found, no user made", async () => {
    const res = await SIGNUP_POST(makeEvent(signupRequest("RANDOM", "random@example.test")));
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ ok: false, error: "license_not_found" });
    expect(openAuthDb().prepare(`SELECT 1 FROM "user" WHERE id = 'u_random'`).get()).toBe(
      undefined,
    );
  });

  it("a signed-in user without a seat is sent to /signup?reason=membership", async () => {
    expect(await layoutFor("u_stranger", "/settings")).toBe(
      "/signup?reason=membership&redirect=%2Fsettings",
    );
  });

  it("step 9: a member can't delete the bundle; the owner can", async () => {
    await expect(
      BUNDLE_DELETE(makeEvent(bundleRequest(null, "DELETE"), "u_member")),
    ).rejects.toMatchObject({ status: 403 });
    const res = await BUNDLE_DELETE(makeEvent(bundleRequest(null, "DELETE"), "u_owner"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });

  it("step 10: past grace the layout sends everyone to /revalidate", async () => {
    const now = Math.floor(Date.now() / 1000);
    const res = await BUNDLE_POST(
      makeEvent(
        bundleRequest(bundleBytes({ issuedAt: now - 2000, notAfter: now - 1000 }), "POST"),
        "u_owner",
      ),
    );
    expect(res.status).toBe(200);
    rust.state.gateState = "revalidate";
    expect(await layoutFor("u_owner", "/")).toBe("/revalidate");
    expect(await layoutFor("u_owner", "/revalidate")).toBe("render");
    rust.state.gateState = "suspended";
    expect(await layoutFor("u_owner", "/")).toBe("/suspended");
    expect(await layoutFor("u_owner", "/suspended")).toBe("render");
    rust.state.gateState = "ok";
  });

  it("an established install refuses a signed-out upload (401)", async () => {
    await expect(
      BUNDLE_POST(makeEvent(bundleRequest(bundleBytes(), "POST"))),
    ).rejects.toMatchObject({ status: 401 });
  });

  it("step 11: a revoking re-upload purges the member's sessions, not the owner's", async () => {
    insertSession("sess_member_1", "u_member");
    insertSession("sess_member_2", "u_member");
    insertSession("sess_owner_1", "u_owner");
    const now = Math.floor(Date.now() / 1000);
    const res = await BUNDLE_POST(
      makeEvent(
        bundleRequest(bundleBytes({ issuedAt: now + 10, revoked: [rust.MEMBER] }), "POST"),
        "u_owner",
      ),
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ ok: true, rowsRevoked: 1 });
    expect(countSessionsFor("u_member")).toBe(0);
    expect(countSessionsFor("u_owner")).toBe(1);
    // A revoked row still renders the shell (as before); the API gate is
    // what refuses it (api-gate.test.ts).
    expect(await layoutFor("u_member", "/")).toBe("render");
  });

  it("a failed bind rolls the Better Auth user back", async () => {
    rust.state.members.delete("u_member");
    rust.state.failNextBind = true;
    nextSignup("u_rollback");
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    await expect(
      SIGNUP_POST(makeEvent(signupRequest(rust.MEMBER, "rb@example.test"))),
    ).rejects.toMatchObject({ status: 500 });
    errors.mockRestore();
    expect(openAuthDb().prepare(`SELECT 1 FROM "user" WHERE id = 'u_rollback'`).get()).toBe(
      undefined,
    );
  });
});
