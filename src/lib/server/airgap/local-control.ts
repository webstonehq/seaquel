/**
 * Air-gap mode mirror of the six functions in `../licensing.ts`.
 *
 * Each function here has the same return-shape (and async-ness) as its
 * online counterpart so Task 6's dispatcher can `await` either one
 * transparently. The bundle is the only ground truth in air-gap mode:
 *
 *   - online       → talk to the control plane
 *   - airgap       → read from `airgap_bundle` + local `member_license`
 *
 * The dispatcher is the only call site for these functions; nothing in
 * here writes to the bundle store or revokes keys (that's the bundle
 * import path in Task 8). The single side effect we DO take is in
 * {@link registerInstallLocal}: it writes the `install_cache` row so
 * `resolveLicenseState()` has something to return on the very first page
 * load after a successful air-gap signup.
 */

import { createHash } from "node:crypto";

import { openAuthDb } from "../auth";
import { writeInstallCache } from "../license-cache";
import type { BindResult, MemberView, TenantContext, VerifyResult } from "../licensing";
import { readActiveBundle } from "./bundle-store";
import type { BundlePayload } from "./types";

// Mirrors seaquel-app/main/packages/marketing/src/lib/server/control/env.ts
// `DEFAULT_AIRGAP_GRACE_SECONDS`. The control plane bakes `not_after =
// currentPeriodEnd + grace` into the bundle; subtracting grace here is
// our best-effort recovery of the underlying billing-period end so the
// `/settings/account` UI can show a sensible date. If ops ever lowers
// the upstream grace this stays accurate to within one bundle refresh.
const SEAQUEL_AIRGAP_GRACE_SECONDS = 2_592_000;

function projectTenantContext(payload: BundlePayload): TenantContext {
  const periodEndSeconds = payload.not_after - SEAQUEL_AIRGAP_GRACE_SECONDS;
  // ISO string of the recovered period end. If the subtraction would
  // wrap to a non-positive value (e.g. an absurdly short `not_after`
  // produced by tests), fall back to null rather than emitting a date
  // before the epoch.
  const currentPeriodEnd =
    periodEndSeconds > 0 ? new Date(periodEndSeconds * 1000).toISOString() : null;
  return {
    tenantId: `airgap-${payload.subscription_id}`,
    slug: payload.tenant_slug,
    status: "active",
    publicUrl: "",
    anchorLicenseId: "",
    subscriptionId: payload.subscription_id,
    tier: payload.tier,
    ownerEmail: "",
    seatLimit: payload.seats,
    currentPeriodEnd,
  };
}

function sha256Hex(input: string): string {
  return createHash("sha256").update(input).digest("hex");
}

function syntheticTenantMemberId(licenseKey: string): string {
  return `local-${sha256Hex(licenseKey).slice(0, 16)}`;
}

function maskKey(key: string): string {
  if (!key) return "";
  if (key.length <= 4) return key;
  return `••••-${key.slice(-4)}`;
}

/**
 * Air-gap signup entry point. Verifies the presented key is the bundle's
 * owner seat, projects the bundle into a `TenantContext`, persists the
 * `install_cache` row in air-gap mode, and returns the context to the
 * caller (signup).
 *
 * Throws:
 *   - "no_airgap_bundle"  → no bundle has been imported yet.
 *   - "license_not_found" → the presented key isn't the owner seat in
 *     the imported bundle (we don't bind anyone but the owner on
 *     register-install — members come in via `verifyMembershipLicense`).
 */
export async function registerInstallLocal(licenseKey: string): Promise<TenantContext> {
  const bundle = await readActiveBundle();
  if (!bundle) throw new Error("no_airgap_bundle");

  const owner = bundle.payload.seat_tokens.find((t) => t.role === "owner");
  if (!owner || owner.key !== licenseKey) {
    throw new Error("license_not_found");
  }

  const tenant = projectTenantContext(bundle.payload);
  writeInstallCache(tenant, "airgap");
  return tenant;
}

/**
 * Read-only projection of the current bundle into a `TenantContext`.
 * Returns null when:
 *   - no bundle is imported, or
 *   - the bundle is past `not_after` (the dispatcher's outer
 *     `resolveLicenseState()` ladder treats this as "revalidate" and
 *     surfaces the upload-fresh-bundle screen).
 *
 * Intentionally has NO side effects — `tenantInfo()` is called on every
 * request and must not write to the cache.
 */
export async function localTenantInfo(): Promise<TenantContext | null> {
  const bundle = await readActiveBundle();
  if (!bundle) return null;
  const now = Math.floor(Date.now() / 1000);
  if (now > bundle.payload.not_after) return null;
  return projectTenantContext(bundle.payload);
}

/**
 * Air-gap variant of `verifyMembershipLicense`. The `signupEmail`
 * argument is unused offline (no remote scoping) but kept for signature
 * parity with the online path so the dispatcher in Task 6 can hand off
 * its full argument tuple without conditionals.
 */
export async function verifyLocalMembershipLicense(
  licenseKey: string,
  _signupEmail: string,
): Promise<VerifyResult> {
  const bundle = await readActiveBundle();
  if (!bundle) return { ok: false, error: "license_not_found" };

  if (bundle.payload.revoked_keys.includes(licenseKey)) {
    return { ok: false, error: "license_inactive" };
  }

  // Synthetic vacant placeholders ship in every bundle to pad up to the
  // seat count; they aren't real seats and must not be bindable.
  if (licenseKey.startsWith("airgap-vacant-")) {
    return { ok: false, error: "license_not_found" };
  }

  const seat = bundle.payload.seat_tokens.find((t) => t.key === licenseKey);
  if (!seat) return { ok: false, error: "license_not_found" };

  // Tenant-uniqueness check: if this key is already bound to a different
  // local user, refuse. Single-tenant container, so this is a 1:1
  // license-key → user mapping enforced locally.
  const existing = findMemberByLicenseKey(licenseKey);
  if (existing) {
    return { ok: false, error: "license_already_in_other_tenant" };
  }

  return {
    ok: true,
    subscriptionId: bundle.payload.subscription_id,
    tier: bundle.payload.tier,
    role: seat.role,
  };
}

/**
 * Air-gap variant of `bindMember`. The actual `member_license` row is
 * inserted by the signup handler (Task 7) via `insertMemberLicense`; in
 * online mode the control plane returns the upstream member id, so the
 * local row carries the upstream's `tenant_members.id` value.
 *
 * Offline there is no upstream id, so we synthesise a deterministic one
 * from the license key (sha256, first 64 bits as hex) — same key always
 * resolves to the same id, which keeps debugging and idempotent retries
 * sane.
 */
export async function bindLocal(args: {
  licenseKey: string;
  containerUserId: string;
  email: string;
  role: "owner" | "member";
}): Promise<BindResult> {
  return { tenantMemberId: syntheticTenantMemberId(args.licenseKey) };
}

/**
 * Air-gap variant of `unbindMember`. True no-op: the actual local
 * `member_license` row deletion happens in
 * `/api/team/[containerUserId]/+server.ts` (Task 9 / existing code).
 * Returning `void` matches the online side, which `await`s the control
 * plane only to record that the deletion happened upstream.
 */
export async function unbindLocal(_containerUserId: string): Promise<void> {
  return;
}

/**
 * Local member listing for `/settings/team`. Joins `member_license`
 * onto Better Auth's `user` table to surface email + role. Filters out
 * tombstoned (revoked) rows — those stay in the DB so we can prove a
 * past binding existed, but the UI shouldn't list them as members.
 */
export async function listLocalMembers(): Promise<MemberView[]> {
  const rows = openAuthDb()
    .prepare(
      `SELECT ml.user_id     AS userId,
              ml.license_key AS licenseKey,
              ml.bound_at    AS boundAt,
              ml.is_owner    AS isOwner,
              u.email        AS email
         FROM member_license ml
         JOIN "user" u ON u.id = ml.user_id
        WHERE ml.revoked_at IS NULL
        ORDER BY ml.bound_at ASC`,
    )
    .all() as Array<{
    userId: string;
    licenseKey: string;
    boundAt: number;
    isOwner: number;
    email: string;
  }>;

  return rows.map((r) => ({
    tenantMemberId: syntheticTenantMemberId(r.licenseKey),
    containerUserId: r.userId,
    email: r.email,
    role: r.isOwner === 1 ? "owner" : "member",
    boundAt: new Date(r.boundAt * 1000).toISOString(),
    maskedLicenseKey: maskKey(r.licenseKey),
  }));
}

// ---------------------------------------------------------------------------
// Helpers

interface MemberLicenseRow {
  userId: string;
  licenseKey: string;
  boundAt: number;
  isOwner: number;
}

/**
 * Lookup helper. `member-license.ts` doesn't currently expose a
 * key-keyed finder (only `findByUserId`), and adding one there is out
 * of scope for this task — Task 6 may consolidate. Inline SELECT for
 * now; same SQL shape as `findByUserId` so a future refactor moves it
 * over cleanly.
 */
function findMemberByLicenseKey(licenseKey: string): MemberLicenseRow | null {
  const row = openAuthDb()
    .prepare(
      `SELECT user_id     AS userId,
              license_key AS licenseKey,
              bound_at    AS boundAt,
              is_owner    AS isOwner
         FROM member_license
        WHERE license_key = ?`,
    )
    .get(licenseKey) as MemberLicenseRow | undefined;
  return row ?? null;
}

/** Test-only — kept colocated with the helpers so the test file can exercise them. */
export const _internal = {
  projectTenantContext,
  syntheticTenantMemberId,
  maskKey,
  findMemberByLicenseKey,
  SEAQUEL_AIRGAP_GRACE_SECONDS,
};
