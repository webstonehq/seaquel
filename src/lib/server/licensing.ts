/**
 * Server-to-server client for the seaquel-app control plane.
 *
 * Every install — Cloud or self-hosted — talks to the same control plane
 * at `SEAQUEL_CONTROL_URL` (defaults to `https://seaquel.app`). There is
 * no mode switch; the difference between Cloud and self-hosted lives in
 * the license tier that the control plane returns, not in this client.
 *
 * Auth model: every `/api/cloud/*` call carries two headers —
 *   - `X-Install-Id`: this install's persistent UUID (see `install.ts`)
 *   - `X-License-Key`: the license key whose authority is asserted for
 *     this specific call.
 *
 * Signup-time calls (`registerInstall`, `verifyMembershipLicense`)
 * authenticate with the *presented* key from the user's signup form.
 * Post-bind admin calls (`tenantInfo`, `bindMember`, `unbindMember`,
 * `listMembers`) authenticate with the install's owner key, which is
 * read from `member_license` WHERE `is_owner = 1`.
 *
 * tenantInfo() itself is now a thin fetch wrapper. The persisted
 * install_cache (see ./license-cache.ts) is the single source of cached
 * tenant context; `resolveLicenseState()` here is the gate the layout
 * hooks consult on every request.
 */

import { openAuthDb } from "./auth";
import { getOrCreateInstallId } from "./install";
import {
  readInstallCache,
  writeInstallCache,
  softTtlSeconds,
  type InstallCache,
} from "./license-cache";
import * as airgap from "./airgap/local-control";
import { isBundleDriven } from "./airgap/bundle-store";

export interface TenantContext {
  tenantId: string;
  slug: string;
  status: "provisioning" | "active" | "suspended" | "failed" | "deleting";
  publicUrl: string;
  anchorLicenseId: string;
  subscriptionId: string;
  tier: string;
  ownerEmail: string;
  seatLimit: number;
  currentPeriodEnd: string | null;
}

export type VerifyResult =
  | {
      ok: true;
      subscriptionId: string;
      tier: string;
      role: "owner" | "member";
    }
  | {
      ok: false;
      error:
        | "license_not_found"
        | "license_inactive"
        | "wrong_subscription"
        | "license_already_in_other_tenant";
    };

export interface BindResult {
  tenantMemberId: string;
}

export interface MemberView {
  tenantMemberId: string;
  containerUserId: string;
  email: string;
  role: "owner" | "member";
  boundAt: string | null;
  maskedLicenseKey: string;
}

function controlUrl(): string {
  return (process.env.SEAQUEL_CONTROL_URL ?? "https://seaquel.app").replace(/\/$/, "");
}

function readOwnerLicenseKey(): string | null {
  const row = openAuthDb()
    .prepare(`SELECT license_key FROM member_license WHERE is_owner = 1 LIMIT 1`)
    .get() as { license_key: string } | undefined;
  return row?.license_key ?? null;
}

export async function registerInstall(licenseKey: string): Promise<TenantContext> {
  if (isBundleDriven()) return airgap.registerInstallLocal(licenseKey);
  const res = await licensingFetch("POST", "/api/cloud/register-install", licenseKey, {
    installId: getOrCreateInstallId(),
    licenseKey,
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`register-install failed: ${res.status} ${body}`);
  }
  return (await res.json()) as TenantContext;
}

export async function tenantInfo(): Promise<TenantContext | null> {
  if (isBundleDriven()) return airgap.localTenantInfo();
  const ownerKey = readOwnerLicenseKey();
  if (!ownerKey) {
    throw new Error("no owner license bound — call registerInstall first");
  }

  const res = await licensingFetch("GET", "/api/cloud/tenant-info", ownerKey);
  if (!res.ok) {
    if (res.status === 401) {
      throw new Error("control plane rejected license key (revoked or wrong install?)");
    }
    throw new Error(`tenant-info failed: ${res.status}`);
  }
  return (await res.json()) as TenantContext;
}

export async function verifyMembershipLicense(
  licenseKey: string,
  signupEmail: string,
): Promise<VerifyResult> {
  if (isBundleDriven()) return airgap.verifyLocalMembershipLicense(licenseKey, signupEmail);
  const res = await licensingFetch("POST", "/api/cloud/verify-membership-license", licenseKey, {
    licenseKey,
    signupEmail,
  });
  if (!res.ok) {
    throw new Error(`verify-membership-license failed: ${res.status}`);
  }
  return (await res.json()) as VerifyResult;
}

export async function bindMember(args: {
  licenseKey: string; // the member's key, goes in the body
  containerUserId: string;
  email: string;
  role: "owner" | "member";
  authKey?: string; // optional override for the X-License-Key header.
  // Used during first-owner signup when no owner is bound yet.
}): Promise<BindResult> {
  if (isBundleDriven()) return airgap.bindLocal(args);
  const ownerKey = args.authKey ?? readOwnerLicenseKey();
  if (!ownerKey) {
    throw new Error("no owner license bound — call registerInstall first");
  }
  const { authKey: _unused, ...bodyArgs } = args;
  const res = await licensingFetch("POST", "/api/cloud/bind-member", ownerKey, bodyArgs);
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`bind-member failed: ${res.status} ${body}`);
  }
  return (await res.json()) as BindResult;
}

export async function unbindMember(containerUserId: string): Promise<void> {
  if (isBundleDriven()) return airgap.unbindLocal(containerUserId);
  const ownerKey = readOwnerLicenseKey();
  if (!ownerKey) {
    throw new Error("no owner license bound — call registerInstall first");
  }
  const res = await licensingFetch("POST", "/api/cloud/unbind-member", ownerKey, {
    containerUserId,
  });
  if (!res.ok) {
    const body = await res.text();
    throw new Error(`unbind-member failed: ${res.status} ${body}`);
  }
}

export async function listMembers(): Promise<MemberView[]> {
  if (isBundleDriven()) return airgap.listLocalMembers();
  const ownerKey = readOwnerLicenseKey();
  if (!ownerKey) {
    throw new Error("no owner license bound — call registerInstall first");
  }
  const res = await licensingFetch("GET", "/api/cloud/members", ownerKey);
  if (!res.ok) throw new Error(`members failed: ${res.status}`);
  return (await res.json()) as MemberView[];
}

// ---------------------------------------------------------------------------

async function licensingFetch(
  method: "GET" | "POST",
  path: string,
  licenseKey: string,
  body?: unknown,
): Promise<Response> {
  const init: RequestInit = {
    method,
    headers: {
      "X-Install-Id": getOrCreateInstallId(),
      "X-License-Key": licenseKey,
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
  };
  if (body !== undefined) {
    init.body = JSON.stringify(body);
  }
  return fetch(`${controlUrl()}${path}`, init);
}

// ---------------------------------------------------------------------------
// License gate — consumed by hooks.server.ts.

export type LicenseState =
  | { kind: "ok"; tenant: TenantContext }
  | { kind: "suspended"; tenant: TenantContext }
  | { kind: "revalidate" } // grace expired, control plane unreachable
  | { kind: "unregistered" }; // no owner key yet — first-run

/**
 * The shared gate used by hooks.server.ts. Encapsulates the 4-branch
 * ladder from the design doc (soft fresh / soft stale + success / stale
 * + fail within grace / past grace).
 */
export async function resolveLicenseState(): Promise<LicenseState> {
  const cache = readInstallCache();
  if (!cache || cache.tenantId === null) return { kind: "unregistered" };

  const now = Math.floor(Date.now() / 1000);
  const softFresh = now - cache.lastValidatedAt < softTtlSeconds();
  if (softFresh) {
    return toState(cache);
  }

  // Stale — try a control-plane refresh.
  try {
    const fresh = await tenantInfo();
    if (fresh) {
      writeInstallCache(fresh, isBundleDriven() ? "airgap" : "online");
      // Re-read so timestamps reflect the just-written grace window.
      const refreshed = readInstallCache();
      if (refreshed) return toState(refreshed);
    }
  } catch (e) {
    console.warn("[seaquel] license refresh failed:", e);
    // fall through to grace handling
  }

  if (now < cache.graceUntil) return toState(cache);
  return { kind: "revalidate" };
}

function toState(cache: InstallCache): LicenseState {
  if (!cache.tenantId || !cache.slug || !cache.status || !cache.tier) {
    return { kind: "unregistered" };
  }
  const tenant: TenantContext = {
    tenantId: cache.tenantId,
    slug: cache.slug,
    status: cache.status,
    publicUrl: "", // not cached; never used by the gate
    anchorLicenseId: "",
    subscriptionId: "",
    tier: cache.tier,
    ownerEmail: "",
    seatLimit: cache.seatLimit ?? 0,
    currentPeriodEnd: cache.currentPeriodEnd,
  };
  if (cache.status === "suspended") return { kind: "suspended", tenant };
  return { kind: "ok", tenant };
}

/**
 * True when the error is a transport-layer failure (DNS, connect, TLS,
 * timeout) — i.e. we never reached the control plane.
 *
 * Used by /api/signup/+server.ts to translate "network failure + no
 * bundle" into a 503 with a clear "import a bundle" CTA.
 */
export function isNetworkFailure(e: unknown): boolean {
  if (!e) return false;
  if (e instanceof TypeError) return true; // undici DNS/connect/TLS
  const name = (e as { name?: string }).name;
  if (name === "AbortError" || name === "TimeoutError") return true;
  const code = (e as { code?: string }).code;
  if (code === "ECONNREFUSED" || code === "ENOTFOUND" || code === "ETIMEDOUT") return true;
  return false;
}
