/**
 * Air-gap bundle upload / clear / status endpoint.
 *
 *   POST   /api/airgap/bundle — verify + persist a signed envelope, walk
 *                                revocations, and flip the install into
 *                                air-gap mode.
 *   DELETE /api/airgap/bundle — clear the active bundle and flip back to
 *                                online mode. Does NOT purge
 *                                `member_license` rows — bindings stick
 *                                until the next online refresh.
 *   GET    /api/airgap/bundle — read-only status: presence, hot payload
 *                                fields, expiry, and revocation count.
 *
 * The gate (`hooks.server.ts: handleApiGate`) pass-throughs `/api/airgap/`
 * because POST needs to accept calls on a fresh install (no session yet);
 * each handler in here enforces its own auth policy:
 *   - POST: loose-auth on first-run (no `install_cache.tenantId`), owner-
 *     only after an owner is bound.
 *   - DELETE: owner-only, always. No loose-auth branch.
 *   - GET:    must be authed AND have a non-revoked `member_license`
 *     row (any role).
 *
 * The 503 path used by `/api/signup` for `register-install` failures has
 * no analogue here — every error is either a 4xx (bad request, denied)
 * or a 500 (internal). Bundle imports never touch the control plane.
 */

import { createHash } from "node:crypto";
import { error, json, type RequestHandler } from "@sveltejs/kit";

import { isOriginTrusted, openAuthDb } from "$lib/server/auth";
import { checkRateLimit } from "$lib/server/rate-limit";
import {
  clearBundle,
  loadTrustedPubkeys,
  readActiveBundle,
  writeBundle,
} from "$lib/server/airgap/bundle-store";
import { verifyBundle } from "$lib/server/airgap/verify";
import { canonicalize, type CanonicalValue } from "$lib/server/airgap/canonical";
import { readInstallCache, setMode } from "$lib/server/license-cache";
import { findByUserId, markRevoked } from "$lib/server/member-license";

const KNOWN_VERIFY_ERRORS = new Set([
  "untrusted_signer",
  "bad_signature",
  "malformed_envelope",
  "schema_mismatch",
]);

async function hashPayload(payload: CanonicalValue): Promise<string> {
  // sha256(canonicalize(payload)) → lowercase hex. Mirrors the convention
  // used everywhere else in the airgap module (the bundle-store cache key
  // is the same string).
  return createHash("sha256")
    .update(Buffer.from(canonicalize(payload)))
    .digest("hex");
}

export const POST: RequestHandler = async (event) => {
  const { request, getClientAddress, locals } = event;

  // CSRF guard. Bundle upload mutates trust state for the entire install
  // (revocation walk + mode flip), so a cross-site POST must not be able
  // to push an attacker-prepared envelope through here. Mirrors the gate
  // applied in `/api/signup`.
  if (!isOriginTrusted(request.headers.get("origin"))) {
    throw error(403, "request origin not allowed");
  }

  // Rate-limit per IP. Verification is cheap (one Ed25519 verify) but the
  // surface area is sensitive — keep brute-forcing of trust-anchor
  // guesses or envelope fuzzing out of reach for a human-paced attacker.
  const rl = checkRateLimit(getClientAddress(), { windowMs: 60_000, max: 5 });
  if (!rl.ok) {
    return new Response("too many bundle uploads", {
      status: 429,
      headers: { "Retry-After": String(rl.retryAfter) },
    });
  }

  // Accept either `application/json` (raw envelope JSON in the body) or
  // `application/octet-stream`. We don't actually branch on the header —
  // both are read as raw bytes and re-decoded by the verifier. The header
  // contract just documents intent; a curl with neither still works.
  let envelopeBytes: Uint8Array;
  try {
    const buf = await request.arrayBuffer();
    envelopeBytes = new Uint8Array(buf);
  } catch {
    throw error(400, "could not read request body");
  }
  if (envelopeBytes.byteLength === 0) {
    return json({ ok: false, error: "malformed_envelope" }, { status: 400 });
  }

  // Verify signature + schema before any auth check. The auth gate below
  // only kicks in once an owner has been bound (loose auth otherwise);
  // doing the cryptographic check first means an unauthenticated attacker
  // hitting a fresh install can't trigger any DB writes by sending
  // garbage — they get a 400 and stop here.
  let verified: Awaited<ReturnType<typeof verifyBundle>>;
  try {
    verified = await verifyBundle(envelopeBytes, loadTrustedPubkeys());
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (KNOWN_VERIFY_ERRORS.has(msg)) {
      return json({ ok: false, error: msg }, { status: 400 });
    }
    console.error("[airgap-bundle] verifyBundle threw an unexpected error", e);
    throw error(500, "bundle verification failed unexpectedly");
  }

  // Auth gate. Loose-auth on the first import (no owner has registered
  // an install yet — the operator is bootstrapping); owner-only once the
  // install is established. Treating "no tenantId" as fresh mirrors the
  // signup handler's `isFirstOwner` check.
  const cache = readInstallCache();
  const installEstablished = !!(cache && cache.tenantId);
  if (installEstablished) {
    if (!locals.user) {
      throw error(401, "must be signed in");
    }
    const ml = findByUserId(locals.user.id);
    if (!ml || !ml.isOwner || ml.revokedAt != null) {
      throw error(403, "owner only");
    }
  }

  // Replay protection. A bundle whose `issued_at` predates the current
  // active bundle is either an operator mistake (uploading an older
  // export) or an attacker trying to roll the install back to a state
  // before a revocation took effect. Reject both.
  const current = await readActiveBundle();
  if (current && verified.payload.issued_at < current.payload.issued_at) {
    return json({ ok: false, error: "bundle_older_than_current" }, { status: 409 });
  }

  const payloadSha256 = await hashPayload(verified.payload as unknown as CanonicalValue);

  // Idempotency. Re-importing the exact same bytes is a common operator
  // pattern (clicking "Import" twice, or a retry after a network blip on
  // the upload page). Short-circuit before any DB writes so the
  // revocation walk doesn't re-fire and produce spurious session purges.
  if (current && current.payloadSha256 === payloadSha256) {
    return json({
      ok: true,
      unchanged: true,
      tier: verified.payload.tier,
      seats: verified.payload.seats,
      notAfter: verified.payload.not_after,
      issuedAt: verified.payload.issued_at,
      pubkeyFingerprint: verified.pubkeyFingerprint,
      revokedKeyCount: verified.payload.revoked_keys.length,
      rowsRevoked: 0,
    });
  }

  // Subscription continuity. Replacing one subscription's bundle with
  // another's is almost certainly a mistake (the operator grabbed the
  // wrong export from the dashboard). Force them to DELETE first so the
  // intent is explicit and we don't silently re-anchor the install to a
  // different upstream subscription.
  if (current && current.payload.subscription_id !== verified.payload.subscription_id) {
    return json({ ok: false, error: "subscription_mismatch" }, { status: 409 });
  }

  // Atomic write: bundle row + revocation stamp + session purge happen
  // inside one SQLite transaction so a crash mid-import can't leave the
  // install in a state where the bundle is recorded but revocations were
  // never applied.
  const db = openAuthDb();
  let rowsRevoked = 0;
  const hasSessionTable = !!db
    .prepare(`SELECT 1 FROM sqlite_master WHERE type='table' AND name='session' LIMIT 1`)
    .get();

  const tx = db.transaction(() => {
    writeBundle(verified, payloadSha256);

    const result = markRevoked(verified.payload.revoked_keys);
    rowsRevoked = result.rowsRevoked;

    if (result.userIds.length > 0 && hasSessionTable) {
      // Better Auth's session table column is `userId` (camelCase, from
      // migration 006). Build a parameterised IN-list so we don't have
      // to escape user-controlled ids.
      const placeholders = result.userIds.map(() => "?").join(", ");
      db.prepare(`DELETE FROM session WHERE userId IN (${placeholders})`).run(...result.userIds);
    }
  });
  tx();

  // Flip the install into air-gap mode. Idempotent — already-airgap
  // installs no-op here. Done after the transaction so a transaction
  // rollback (which would re-throw above) doesn't leave a stale mode.
  setMode("airgap");

  return json({
    ok: true,
    unchanged: false,
    tier: verified.payload.tier,
    seats: verified.payload.seats,
    notAfter: verified.payload.not_after,
    issuedAt: verified.payload.issued_at,
    pubkeyFingerprint: verified.pubkeyFingerprint,
    revokedKeyCount: verified.payload.revoked_keys.length,
    rowsRevoked,
  });
};

export const DELETE: RequestHandler = async (event) => {
  const { request, getClientAddress, locals } = event;

  if (!isOriginTrusted(request.headers.get("origin"))) {
    throw error(403, "request origin not allowed");
  }

  const rl = checkRateLimit(getClientAddress(), { windowMs: 60_000, max: 5 });
  if (!rl.ok) {
    return new Response("too many bundle deletes", {
      status: 429,
      headers: { "Retry-After": String(rl.retryAfter) },
    });
  }

  // DELETE has no loose-auth branch. Clearing the bundle drops the
  // install back to online mode, which means the control plane becomes
  // authoritative again — only the owner should make that call.
  if (!locals.user) {
    throw error(401, "must be signed in");
  }
  const ml = findByUserId(locals.user.id);
  if (!ml || !ml.isOwner || ml.revokedAt != null) {
    throw error(403, "owner only");
  }

  clearBundle();
  setMode("online");

  // Intentionally do NOT purge `member_license` rows. Members bound
  // while offline keep their local rows; the next online tenantInfo()
  // refresh repopulates `install_cache` from the control plane and any
  // stale rows get reconciled there.

  return json({ ok: true });
};

export const GET: RequestHandler = async (event) => {
  // `/api/airgap/` is on the gate pass-through list (so POST can run
  // unauthenticated on a fresh install), so this handler must enforce
  // its own auth. Status info exposes tier / seats / pubkey fingerprint
  // — fine to share with any bound member, but not with anonymous
  // callers.
  if (!event.locals.user) {
    throw error(401, "must be signed in");
  }
  const ml = findByUserId(event.locals.user.id);
  if (!ml || ml.revokedAt != null) {
    throw error(403, "not a bound member of this install");
  }

  const active = await readActiveBundle();
  if (!active) {
    return json({ present: false });
  }
  return json({
    present: true,
    tier: active.payload.tier,
    seats: active.payload.seats,
    notAfter: active.payload.not_after,
    issuedAt: active.payload.issued_at,
    importedAt: active.importedAt,
    pubkeyFingerprint: active.pubkeyFingerprint,
    payloadSha256: active.payloadSha256,
    revokedKeyCount: active.payload.revoked_keys.length,
    expired: Math.floor(Date.now() / 1000) > active.payload.not_after,
  });
};
