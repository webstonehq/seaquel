/**
 * Tests for /api/airgap/bundle (POST, DELETE, GET).
 *
 * Strategy: same DB-redirect trick the other airgap tests use — point
 * `DATA_DIR` at a per-file tempdir BEFORE importing the handler, so all
 * `openAuthDb()` calls land on a fresh schema. The auth.db migration
 * bundle does the table creation. Real Ed25519 signing happens here in
 * the test file using @noble/ed25519 so we exercise the actual verifier
 * end-to-end (no `vi.mock` on `verifyBundle`).
 *
 * The handler reads `event.locals.user` directly. We construct
 * `RequestEvent`-shaped stubs with just the surface the handler touches
 * (locals, request, getClientAddress) and cast through `unknown`.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { afterAll, beforeEach, describe, expect, it } from "vitest";

// -- DB redirect must happen first ---------------------------------------
const tmp = mkdtempSync(join(tmpdir(), "seaquel-airgap-bundle-api-"));
process.env.DATA_DIR = tmp;

// -- Trust anchor for the test signer ------------------------------------
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

// Make every request look like it's coming from a trusted origin so we
// don't tangle the origin check into every individual test.
process.env.SEAQUEL_TRUSTED_ORIGINS = "http://localhost:5173";

const { canonicalize } = await import("$lib/server/airgap/canonical");
const { openAuthDb } = await import("$lib/server/auth");
const { _resetBundleStoreCache } = await import("$lib/server/airgap/bundle-store");
const memberLicense = await import("$lib/server/member-license");
const licenseCache = await import("$lib/server/license-cache");
const { _resetRateLimit } = await import("$lib/server/rate-limit");
const { POST, DELETE, GET } = await import("./+server");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

interface BuildOptions {
  issuedAt?: number;
  subscriptionId?: string;
  revokedKeys?: string[];
  seed?: Uint8Array; // override to produce an untrusted-signer bundle
  fingerprint?: string; // override to mismatch the pubkey
}

async function buildBundleBytes(opts: BuildOptions = {}): Promise<{
  bytes: Uint8Array;
  payload: import("$lib/server/airgap/types").BundlePayload;
}> {
  const issuedAt = opts.issuedAt ?? Math.floor(Date.now() / 1000);
  const subscriptionId = opts.subscriptionId ?? "sub_test_0001";
  const revoked_keys = opts.revokedKeys ?? [];
  const seed = opts.seed ?? SEED;
  const payload: import("$lib/server/airgap/types").BundlePayload = {
    version: 1,
    issued_at: issuedAt,
    not_before: issuedAt - 60,
    not_after: issuedAt + 60 * 60 * 24 * 365,
    subscription_id: subscriptionId,
    tenant_slug: "acme",
    tier: "team",
    seats: 3,
    seat_tokens: [
      { key: "owner_key_abc", role: "owner" },
      { key: "member_key_xyz", role: "member" },
    ],
    revoked_keys,
    issued_by_install_id: null,
  };
  const canonical = canonicalize(
    payload as unknown as import("$lib/server/airgap/canonical").CanonicalValue,
  );
  const sig = await ed.signAsync(canonical, seed);
  const fingerprint =
    opts.fingerprint ??
    (seed === SEED
      ? FINGERPRINT
      : await (
          await import("$lib/server/airgap/canonical")
        ).fingerprintPubkey(await ed.getPublicKeyAsync(seed)));
  const b64Url = (bytes: Uint8Array): string => {
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const envelope = {
    payload: b64Url(canonical),
    sig: b64Url(sig),
    pubkey_fingerprint: fingerprint,
  };
  return {
    bytes: new TextEncoder().encode(JSON.stringify(envelope)),
    payload,
  };
}

function makeRequest(bodyBytes: Uint8Array | null, method: string): Request {
  const init: RequestInit = {
    method,
    headers: { origin: "http://localhost:5173" },
  };
  if (bodyBytes) {
    // Re-encode as text and pass via a string body — `Request` accepts
    // strings universally, and the handler reads via `arrayBuffer()`
    // anyway, so the byte-level fidelity is preserved (Latin-1 / UTF-8
    // safe because all signed envelopes are ASCII JSON).
    init.body = new TextDecoder().decode(bodyBytes);
    (init.headers as Record<string, string>)["Content-Type"] = "application/json";
  }
  return new Request("http://localhost:5173/api/airgap/bundle", init);
}

interface LocalsOverride {
  userId?: string | null;
}

function makeEvent(request: Request, locals: LocalsOverride = {}): Parameters<typeof POST>[0] {
  const user =
    locals.userId === undefined
      ? null
      : locals.userId === null
        ? null
        : { id: locals.userId, email: `${locals.userId}@example.test`, name: locals.userId };
  return {
    request,
    getClientAddress: () => "127.0.0.1",
    locals: {
      user,
      session: null,
      tenant: null,
      licenseState: "unregistered",
    },
  } as unknown as Parameters<typeof POST>[0];
}

function insertUser(userId: string): void {
  const now = new Date().toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, email, name, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    )
    .run(userId, `${userId}@example.test`, userId, now, now);
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

function insertOwner(userId: string, licenseKey: string): void {
  insertUser(userId);
  memberLicense.insert({
    userId,
    licenseKey,
    boundAt: Math.floor(Date.now() / 1000),
    controlMemberId: `tm_${userId}`,
    isOwner: true,
  });
}

function markInstallEstablished(): void {
  // Stamp install_cache.tenant_id so the POST handler flips into
  // owner-only auth mode. Use raw SQL to avoid coupling the test to the
  // full TenantContext shape required by writeInstallCache().
  const now = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `INSERT INTO install_cache
         (id, tenant_id, slug, status, tier, seat_limit,
          current_period_end, last_validated_at, grace_until, mode)
       VALUES (1, 'tenant_test', 'acme', 'active', 'team', 5,
               NULL, ?, ?, 'online')
       ON CONFLICT(id) DO UPDATE SET
         tenant_id = excluded.tenant_id,
         slug      = excluded.slug,
         status    = excluded.status,
         tier      = excluded.tier`,
    )
    .run(now, now + 86_400);
}

beforeEach(() => {
  openAuthDb().prepare(`DELETE FROM "session"`).run();
  openAuthDb().prepare(`DELETE FROM member_license`).run();
  openAuthDb().prepare(`DELETE FROM "user"`).run();
  openAuthDb().prepare(`DELETE FROM airgap_bundle`).run();
  openAuthDb().prepare(`DELETE FROM install_cache`).run();
  _resetBundleStoreCache();
  _resetRateLimit();
});

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

describe("POST /api/airgap/bundle — round-trip", () => {
  it("verifies + stores + returns the hot fields on a fresh install", async () => {
    const { bytes, payload } = await buildBundleBytes();
    const res = await POST(makeEvent(makeRequest(bytes, "POST")));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toMatchObject({
      ok: true,
      unchanged: false,
      tier: payload.tier,
      seats: payload.seats,
      notAfter: payload.not_after,
      issuedAt: payload.issued_at,
      pubkeyFingerprint: FINGERPRINT,
      revokedKeyCount: 0,
      rowsRevoked: 0,
    });
    // Mode flipped to airgap.
    expect(licenseCache.readInstallCache()?.mode).toBe("airgap");
  });
});

describe("POST /api/airgap/bundle — verifier error mapping", () => {
  it("untrusted signer → 400 untrusted_signer", async () => {
    // Sign with a key NOT in the trust set.
    const evilSeed = new Uint8Array(32).fill(7);
    const { bytes } = await buildBundleBytes({ seed: evilSeed });
    const res = await POST(makeEvent(makeRequest(bytes, "POST")));
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ ok: false, error: "untrusted_signer" });
  });

  it("malformed body (empty) → 400 malformed_envelope", async () => {
    const res = await POST(makeEvent(makeRequest(new Uint8Array(0), "POST")));
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ ok: false, error: "malformed_envelope" });
  });

  it("bad signature → 400 bad_signature", async () => {
    // Build a valid envelope then flip a byte in the `sig` field. We
    // can't just mutate one byte in the binary form because base64url
    // would re-decode to almost-valid bytes; build the envelope, parse
    // it, replace `sig` with a clearly wrong one.
    const { bytes } = await buildBundleBytes();
    const decoded = JSON.parse(new TextDecoder().decode(bytes));
    decoded.sig = "AA"; // 1-byte signature → fails Ed25519 verify
    const tampered = new TextEncoder().encode(JSON.stringify(decoded));
    const res = await POST(makeEvent(makeRequest(tampered, "POST")));
    expect(res.status).toBe(400);
    const body = await res.json();
    expect(body.ok).toBe(false);
    expect(["bad_signature", "malformed_envelope"]).toContain(body.error);
  });
});

describe("POST /api/airgap/bundle — replay + idempotency + subscription", () => {
  it("rejects an older issued_at with 409 bundle_older_than_current", async () => {
    const t1 = Math.floor(Date.now() / 1000);
    const newer = await buildBundleBytes({ issuedAt: t1 });
    const first = await POST(makeEvent(makeRequest(newer.bytes, "POST")));
    expect(first.status).toBe(200);

    const older = await buildBundleBytes({ issuedAt: t1 - 1000 });
    const res = await POST(makeEvent(makeRequest(older.bytes, "POST")));
    expect(res.status).toBe(409);
    expect(await res.json()).toEqual({
      ok: false,
      error: "bundle_older_than_current",
    });
  });

  it("re-importing the SAME bundle returns unchanged: true", async () => {
    const { bytes } = await buildBundleBytes();
    const first = await POST(makeEvent(makeRequest(bytes, "POST")));
    expect(first.status).toBe(200);
    expect((await first.json()).unchanged).toBe(false);

    const second = await POST(makeEvent(makeRequest(bytes, "POST")));
    expect(second.status).toBe(200);
    const body = await second.json();
    expect(body.unchanged).toBe(true);
    expect(body.rowsRevoked).toBe(0);
  });

  it("rejects a bundle from a different subscription with 409 subscription_mismatch", async () => {
    const t1 = Math.floor(Date.now() / 1000);
    const a = await buildBundleBytes({ issuedAt: t1, subscriptionId: "sub_A" });
    const ra = await POST(makeEvent(makeRequest(a.bytes, "POST")));
    expect(ra.status).toBe(200);

    const b = await buildBundleBytes({
      issuedAt: t1 + 1000,
      subscriptionId: "sub_B",
    });
    const rb = await POST(makeEvent(makeRequest(b.bytes, "POST")));
    expect(rb.status).toBe(409);
    expect(await rb.json()).toEqual({
      ok: false,
      error: "subscription_mismatch",
    });
  });
});

describe("POST /api/airgap/bundle — revocation walk", () => {
  it("stamps revoked_at on matching member_license rows and purges sessions", async () => {
    insertOwner("u_alpha", "lic_alpha");
    insertUser("u_beta");
    memberLicense.insert({
      userId: "u_beta",
      licenseKey: "lic_beta",
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: "tm_u_beta",
      isOwner: false,
    });
    insertSession("sess_alpha_1", "u_alpha");
    insertSession("sess_alpha_2", "u_alpha");
    insertSession("sess_beta_1", "u_beta");

    const { bytes } = await buildBundleBytes({ revokedKeys: ["lic_beta"] });
    const res = await POST(makeEvent(makeRequest(bytes, "POST"), { userId: "u_alpha" }));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.rowsRevoked).toBe(1);
    expect(body.revokedKeyCount).toBe(1);

    const beta = memberLicense.findByUserId("u_beta");
    expect(beta).not.toBeNull();
    expect(beta!.revokedAt).not.toBeNull();
    const alpha = memberLicense.findByUserId("u_alpha");
    expect(alpha!.revokedAt).toBeNull();

    // Sessions for the revoked user are gone; the owner's stay.
    expect(countSessionsFor("u_beta")).toBe(0);
    expect(countSessionsFor("u_alpha")).toBe(2);
  });
});

describe("DELETE /api/airgap/bundle", () => {
  it("requires authentication", async () => {
    await expect(DELETE(makeEvent(makeRequest(null, "DELETE")))).rejects.toMatchObject({
      status: 401,
    });
  });

  it("requires owner role", async () => {
    insertUser("u_member");
    memberLicense.insert({
      userId: "u_member",
      licenseKey: "lic_member",
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: "tm_member",
      isOwner: false,
    });
    await expect(
      DELETE(makeEvent(makeRequest(null, "DELETE"), { userId: "u_member" })),
    ).rejects.toMatchObject({ status: 403 });
  });

  it("flips mode back to online but keeps member_license rows", async () => {
    insertOwner("u_owner", "lic_owner");
    // First seed an active bundle via POST.
    const { bytes } = await buildBundleBytes();
    const postRes = await POST(makeEvent(makeRequest(bytes, "POST"), { userId: "u_owner" }));
    expect(postRes.status).toBe(200);
    expect(licenseCache.readInstallCache()?.mode).toBe("airgap");

    const res = await DELETE(makeEvent(makeRequest(null, "DELETE"), { userId: "u_owner" }));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    expect(licenseCache.readInstallCache()?.mode).toBe("online");

    // member_license row for the owner is still there.
    const owner = memberLicense.findByUserId("u_owner");
    expect(owner).not.toBeNull();
    expect(owner!.licenseKey).toBe("lic_owner");
  });
});

describe("GET /api/airgap/bundle", () => {
  it("requires authentication", async () => {
    await expect(GET(makeEvent(makeRequest(null, "GET")))).rejects.toMatchObject({ status: 401 });
  });

  it("requires bound membership", async () => {
    insertUser("u_lonely");
    await expect(
      GET(makeEvent(makeRequest(null, "GET"), { userId: "u_lonely" })),
    ).rejects.toMatchObject({ status: 403 });
  });

  it("returns { present: false } when no bundle is loaded", async () => {
    insertOwner("u_o", "lic_o");
    const res = await GET(makeEvent(makeRequest(null, "GET"), { userId: "u_o" }));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ present: false });
  });

  it("returns the full status block after an import", async () => {
    insertOwner("u_o", "lic_o");
    const { bytes, payload } = await buildBundleBytes();
    await POST(makeEvent(makeRequest(bytes, "POST"), { userId: "u_o" }));

    const res = await GET(makeEvent(makeRequest(null, "GET"), { userId: "u_o" }));
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body).toMatchObject({
      present: true,
      tier: payload.tier,
      seats: payload.seats,
      notAfter: payload.not_after,
      issuedAt: payload.issued_at,
      pubkeyFingerprint: FINGERPRINT,
      revokedKeyCount: 0,
      expired: false,
    });
    expect(typeof body.payloadSha256).toBe("string");
    expect(typeof body.importedAt).toBe("number");
  });
});

describe("POST /api/airgap/bundle — first-run loose auth vs established install", () => {
  it("accepts unauthenticated POST when install_cache has no tenantId", async () => {
    // No install_cache row → loose-auth branch.
    const { bytes } = await buildBundleBytes();
    const res = await POST(makeEvent(makeRequest(bytes, "POST")));
    expect(res.status).toBe(200);
  });

  it("rejects unauthenticated POST once an owner is bound", async () => {
    // Simulate a fully-registered install: install_cache has a tenant
    // id and an owner row exists. Both `if (installEstablished)` and
    // the subsequent `findByUserId` check must trigger.
    markInstallEstablished();
    insertOwner("u_o", "lic_o");

    const { bytes } = await buildBundleBytes();
    await expect(POST(makeEvent(makeRequest(bytes, "POST")))).rejects.toMatchObject({
      status: 401,
    });
  });
});

// Mark `markInstallEstablished` as touched even on test runs where it
// isn't strictly reachable — the helper is the cleanest expression of
// "the install has gone through registerInstall" and we want it to stay
// visible to readers.
void markInstallEstablished;
