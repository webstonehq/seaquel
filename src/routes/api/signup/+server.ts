/**
 * POST /api/signup — unified license-anchored signup endpoint.
 *
 * One flow for Cloud and self-hosted alike. The deployment-mode switch
 * is gone: every install talks to the same control plane and requires a
 * license key at signup. The control plane decides what the key buys.
 *
 *   - The very first owner-tier key presented for an install (one with
 *     no `install_cache` row yet) triggers `registerInstall`, which
 *     creates the tenant upstream and writes the local install_cache
 *     so subsequent calls can verify against this install's seat pool.
 *   - Every signup (including the first) then runs
 *     `verifyMembershipLicense`, creates the Better Auth user, calls
 *     `bind-member` upstream, and persists a local `member_license`
 *     row. The first owner of an install is recorded with
 *     `is_owner = 1` so admin calls can authenticate as the owner key
 *     later on.
 *
 * If anything between user creation and binding fails, we roll back
 * the Better Auth user so the visitor can retry without tripping a
 * duplicate-email error.
 *
 * Uses Better Auth's `signUpEmail` with `asResponse: true` so the
 * Set-Cookie session header is returned to the browser exactly the way
 * the standard `/api/auth/sign-up/email` route would.
 */

import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";
import { auth, isOriginTrusted, openAuthDb } from "$lib/server/auth";
import { checkRateLimit } from "$lib/server/rate-limit";
import {
  bindMember,
  isNetworkFailure,
  registerInstall,
  verifyMembershipLicense,
} from "$lib/server/licensing";
import type { TenantContext } from "$lib/server/licensing";
import { isBundleDriven } from "$lib/server/airgap/bundle-store";
import { readInstallCache, writeInstallCache } from "$lib/server/license-cache";
import { findByUserId, insert as insertMemberLicense } from "$lib/server/member-license";

interface SignupBody {
  email: string;
  password: string;
  name: string;
  licenseKey?: string;
}

export const POST: RequestHandler = async ({ request, getClientAddress }) => {
  // CSRF guard. This endpoint triggers `registerInstall` (which binds the
  // install permanently to a license key) and creates Better Auth users —
  // both are state-changing side effects, so a CSRF attack could (a) claim
  // an un-registered install with the attacker's license, or (b) create
  // rogue users via a tab on a malicious site. Better Auth's `/api/auth/*`
  // handler already enforces trustedOrigins on its routes; mirror that here
  // for the custom signup path.
  if (!isOriginTrusted(request.headers.get("origin"))) {
    throw error(403, "request origin not allowed");
  }

  // Rate-limit per IP. Signup creates an upstream license binding so
  // brute-forcing license keys is the threat we're mitigating; 5 attempts
  // per minute is well above a human re-typing a typo and well below a
  // useful attack rate.
  const rl = checkRateLimit(getClientAddress(), { windowMs: 60_000, max: 5 });
  if (!rl.ok) {
    return new Response("too many signup attempts", {
      status: 429,
      headers: { "Retry-After": String(rl.retryAfter) },
    });
  }

  let body: SignupBody;
  try {
    body = (await request.json()) as SignupBody;
  } catch {
    throw error(400, "invalid JSON body");
  }
  const email = body.email?.trim().toLowerCase() ?? "";
  const password = body.password ?? "";
  const name = body.name?.trim() ?? "";
  const licenseKey = body.licenseKey?.trim() ?? "";

  if (!email || !password || !name) {
    throw error(400, "email, password, and name are required");
  }
  if (password.length < 8) {
    throw error(400, "password must be at least 8 characters");
  }

  // Both modes require a license key now.
  if (!licenseKey) {
    throw error(400, "license key is required");
  }

  // Determine whether this is the install's first owner sign-up.
  // If install_cache has no tenant row yet, the first owner-tier key
  // wins — we self-register the install with the control plane.
  const cache = readInstallCache();
  const isFirstOwner = !cache || !cache.tenantId;

  if (isFirstOwner) {
    let registered: TenantContext;
    try {
      registered = await registerInstall(licenseKey);
    } catch (e) {
      console.error("[signup] register-install failed", e);
      // Transport failure with no bundle loaded → the install genuinely has
      // no way to reach the control plane right now. Return a structured
      // 503 so the frontend can surface the "Import a bundle at
      // /airgap-setup" CTA. Any other failure (control plane responded but
      // rejected the key, e.g. 400/404) keeps the original 400 path.
      if (isNetworkFailure(e) && !isBundleDriven()) {
        return json({ ok: false, error: "control_plane_unreachable_no_bundle" }, { status: 503 });
      }
      throw error(400, "could not register install with the control plane");
    }
    writeInstallCache(registered, isBundleDriven() ? "airgap" : "online");
  }

  // Verify the membership license against the (now-registered) install.
  let verified: Awaited<ReturnType<typeof verifyMembershipLicense>>;
  try {
    verified = await verifyMembershipLicense(licenseKey, email);
  } catch (e) {
    console.error("[signup] verify-membership-license failed", e);
    // Symmetric with the register-install branch above: a transport failure
    // on the first-owner path with no bundle means the install can't reach
    // the control plane. For non-first-owner signups this should be rare
    // (an established install would already have an active bundle or
    // working network), so fall through to the existing error semantics.
    if (isFirstOwner && isNetworkFailure(e) && !isBundleDriven()) {
      return json({ ok: false, error: "control_plane_unreachable_no_bundle" }, { status: 503 });
    }
    throw error(500, "could not verify membership license");
  }
  if (!verified.ok) {
    return json({ ok: false, error: verified.error }, { status: 400 });
  }

  // Create the Better Auth user.
  const authResponse = await auth.api.signUpEmail({
    body: { email, password, name },
    asResponse: true,
  });
  if (!authResponse.ok) {
    const text = await authResponse.text();
    return new Response(text, {
      status: authResponse.status,
      headers: authResponse.headers,
    });
  }

  let userId: string | null = null;
  try {
    const data = (await authResponse.clone().json()) as {
      user?: { id?: string };
    };
    userId = data.user?.id ?? null;
  } catch {
    /* fall through */
  }
  if (!userId) {
    throw error(500, "signup succeeded but Better Auth response had no user id");
  }

  if (findByUserId(userId)) {
    return authResponse;
  }

  try {
    const bound = await bindMember({
      licenseKey,
      containerUserId: userId,
      email,
      role: verified.role,
      ...(isFirstOwner ? { authKey: licenseKey } : {}),
    });
    insertMemberLicense({
      userId,
      licenseKey,
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: bound.tenantMemberId,
      isOwner: isFirstOwner,
    });
  } catch (e) {
    console.error("[signup] post-signup binding failed, rolling back user", e);
    try {
      const db = openAuthDb();
      db.prepare(`DELETE FROM "user" WHERE id = ?`).run(userId);
    } catch (delErr) {
      console.error("[signup] rollback delete also failed", delErr);
    }
    throw error(500, "signup completed but membership binding failed; please retry");
  }

  return authResponse;
};
