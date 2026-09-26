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
 * The gate (`hooks.server.ts: handleApiGate`) passes `/api/airgap/`
 * through because POST needs to accept calls on a fresh install (no session
 * yet). The auth policy:
 *   - POST: loose-auth on first-run (no `install_cache.tenantId`), owner-
 *     only after an owner is bound. The Rust service applies it, after the
 *     signature check, so garbage from an anonymous caller never writes.
 *   - DELETE: owner-only, always (Rust).
 *   - GET:    must be authed AND have a non-revoked `member_license`
 *     row (any role), checked here.
 *
 * Verification, the bundle row, the revocation walk and the mode flip are
 * in the Rust service (`/internal/license/airgap/*`). The CSRF and
 * rate-limit guards and the session purge for revoked users stay here.
 */

import { error, json, type RequestHandler } from "@sveltejs/kit";

import { isOriginTrusted, openAuthDb } from "$lib/server/auth";
import { checkRateLimit } from "$lib/server/rate-limit";
import {
  airgapClear,
  airgapStatus,
  airgapUpload,
  LicenseClientError,
} from "$lib/server/license-client";

/** Rust's 401/403 (`must be signed in`, `owner only`) as SvelteKit errors. */
function rethrowAuth(e: unknown): never {
  if (e instanceof LicenseClientError && (e.status === 401 || e.status === 403)) {
    throw error(e.status, e.message);
  }
  throw e;
}

/**
 * Sign out every user a bundle just revoked. Better Auth's session column
 * is `userId` (migration 006); the ids are bound, never spliced in.
 */
function purgeSessions(userIds: string[]): void {
  if (userIds.length === 0) return;
  const db = openAuthDb();
  const hasSessionTable = !!db
    .prepare(`SELECT 1 FROM sqlite_master WHERE type='table' AND name='session' LIMIT 1`)
    .get();
  if (!hasSessionTable) return;
  const placeholders = userIds.map(() => "?").join(", ");
  db.prepare(`DELETE FROM session WHERE userId IN (${placeholders})`).run(...userIds);
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
  // `application/octet-stream`. Both are read as raw bytes and re-decoded
  // by the verifier.
  let envelopeBytes: Uint8Array;
  try {
    envelopeBytes = new Uint8Array(await request.arrayBuffer());
  } catch {
    throw error(400, "could not read request body");
  }

  // Verify, auth (loose on a fresh install), replay and subscription
  // checks, then the bundle row and revocations in one transaction and the
  // mode flip.
  const outcome = await airgapUpload(envelopeBytes, locals.user?.id ?? null).catch(rethrowAuth);

  // Sign out every revoked user (Rust lists them all, not only this
  // import's). Their rows are already marked, so the API gate refuses them
  // even if this fails; a retry of the upload answers `unchanged` with the
  // same list and purges them then.
  try {
    purgeSessions(outcome.revokedUserIds);
  } catch (e) {
    console.error("[airgap-bundle] session purge failed; retry the upload to finish it:", e);
  }

  return json(outcome.body, { status: outcome.status });
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
  await airgapClear(locals.user.id).catch(rethrowAuth);

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
  const status = await airgapStatus(event.locals.user.id);
  if (!status.member || status.member.revoked) {
    throw error(403, "not a bound member of this install");
  }

  const active = status.bundle;
  if (!active) {
    return json({ present: false });
  }
  return json({
    present: true,
    tier: active.tier,
    seats: active.seats,
    notAfter: active.notAfter,
    issuedAt: active.issuedAt,
    importedAt: active.importedAt,
    pubkeyFingerprint: active.pubkeyFingerprint,
    payloadSha256: active.payloadSha256,
    revokedKeyCount: active.revokedKeyCount,
    expired: active.expired,
  });
};
