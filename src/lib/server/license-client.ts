/**
 * The web build's licensing, served by the Rust service's
 * `/internal/license/*` (`crates/seaquel-server/src/routes/internal_license.rs`
 * over `seaquel_license::server`).
 *
 * Rust owns every read and write of the license tables in `auth.db`
 * (`member_license`, `install`, `install_cache`, `airgap_bundle`), the
 * control-plane client, the TTL ladder and air-gap bundle verification.
 * Node keeps Better Auth, applies the auth.db migrations, purges sessions,
 * and runs each route's steps in order, calling one function here per step.
 *
 * - **The gate is cached for 5 seconds per user** (`gate`), so a burst of
 *   requests from one browser makes one call. Every step that changes
 *   licensing state (`registerInstall`, `bindMember`, `unbindMember`, the
 *   air-gap upload and clear) drops the whole cache.
 * - **`/internal/*` is loopback only and needs the per-boot secret.**
 *   `server.js` generates `SEAQUEL_INTERNAL_SECRET` at startup and gives it
 *   to both processes; every call here sends it in `X-Seaquel-Internal`.
 *   This module is the one caller; no proxy forwards `/internal/*`. A
 *   non-loopback `SEAQUEL_RUST_URL` (Rust on another host) is refused.
 * - **License keys never reach a log** from here: errors carry Rust's code
 *   and message, which never include one.
 */

import { openAuthDb } from "./auth";

const RUST_BASE_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";

/** The header the per-boot secret goes in (`SECRET_HEADER` in Rust). */
const SECRET_HEADER = "x-seaquel-internal";

// ---------------------------------------------------------------------------
// Types (the JSON Rust answers with)

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

export type LicenseStateKind = "ok" | "suspended" | "revalidate" | "unregistered";

/** A user's `member_license` row, without the key. */
export interface MemberState {
  isOwner: boolean;
  /** Revoked by a bundle import: not a bound member for the API gate. */
  revoked: boolean;
}

export interface GateAnswer {
  state: LicenseStateKind;
  /** For `ok` and `suspended`. */
  tenant: TenantContext | null;
  /** The user's row, when a user was given and has one. */
  member: MemberState | null;
  hasTenant: boolean;
  bundlePresent: boolean;
}

export interface InstallStatus {
  hasTenant: boolean;
  bundlePresent: boolean;
}

export type VerifyResult =
  | { ok: true; subscriptionId: string; tier: string; role: "owner" | "member" }
  | {
      ok: false;
      error:
        | "license_not_found"
        | "license_inactive"
        | "wrong_subscription"
        | "license_already_in_other_tenant";
    };

export interface MemberView {
  tenantMemberId: string;
  containerUserId: string;
  email: string;
  role: "owner" | "member";
  boundAt: string | null;
  maskedLicenseKey: string;
}

export interface BundleStatus {
  tier: string;
  seats: number;
  notAfter: number;
  issuedAt: number;
  importedAt: number;
  pubkeyFingerprint: string;
  payloadSha256: string;
  revokedKeyCount: number;
  expired: boolean;
}

export interface AirgapStatus {
  mode: "online" | "airgap";
  lastValidatedAt: number | null;
  graceUntil: number | null;
  member: MemberState | null;
  bundle: BundleStatus | null;
}

export interface UploadOutcome {
  /** 200, 400 or 409: the status `/api/airgap/bundle` answers with. */
  status: number;
  /** The JSON body `/api/airgap/bundle` answers with. */
  body: Record<string, unknown>;
  /** Users whose rows this import revoked; purge their sessions. */
  revokedUserIds: string[];
}

// ---------------------------------------------------------------------------
// Errors

/**
 * A failed license call. `code` is Rust's (`NETWORK_ERROR`,
 * `CONTROL_PLANE_ERROR`, `NOT_READY`, `NO_OWNER`, `UNAUTHORIZED`,
 * `FORBIDDEN`, …), `UPSTREAM_UNAVAILABLE` when the Rust service can't be
 * reached, or `HTTP_<status>` for an answer that isn't `{code, message}`.
 */
export class LicenseClientError extends Error {
  readonly code: string;
  readonly status: number;
  constructor(code: string, message: string, status: number) {
    super(message);
    this.name = "LicenseClientError";
    this.code = code;
    this.status = status;
  }
}

/**
 * True when the control plane was never reached (DNS, connect, TLS,
 * timeout): Rust's `NETWORK_ERROR`. The Rust service itself being down is
 * not a control-plane network failure.
 */
export function isNetworkFailure(e: unknown): boolean {
  return e instanceof LicenseClientError && e.code === "NETWORK_ERROR";
}

// ---------------------------------------------------------------------------
// Transport

/**
 * How long to keep retrying when the Rust service refuses the connection:
 * `server.js` starts it alongside Node, so the first requests after a start
 * can arrive before it listens.
 */
const CONNECT_RETRY_DELAYS_MS = [50, 100, 200, 400, 800, 1600];

let schemaReady = false;

/**
 * Rust answers `NOT_READY` until auth.db has its tables, which Node creates
 * on first open. Open it (idempotent) before the first call.
 */
function ensureAuthSchema(): void {
  if (schemaReady) return;
  openAuthDb();
  schemaReady = true;
}

function isConnectionRefused(e: unknown): boolean {
  const cause = (e as { cause?: { code?: unknown } } | null)?.cause;
  return cause?.code === "ECONNREFUSED";
}

interface CallOptions {
  method?: "GET" | "POST";
  /** Sent as `?user=` when set. */
  user?: string | null;
  json?: unknown;
  body?: Uint8Array;
}

async function call<T>(path: string, options: CallOptions = {}): Promise<T> {
  ensureAuthSchema();
  const url = new URL(`${RUST_BASE_URL}/internal/license/${path}`);
  if (options.user) url.searchParams.set("user", options.user);
  // Read per call: server.js sets it before any request arrives.
  const secret = process.env.SEAQUEL_INTERNAL_SECRET;
  if (!secret) {
    console.error(
      "[seaquel] SEAQUEL_INTERNAL_SECRET isn't set; the license service refuses every call without it (server.js sets it, as does `npm run dev:web:full`)",
    );
    throw new LicenseClientError("UPSTREAM_UNAVAILABLE", "the license service is unavailable", 502);
  }
  const headers: Record<string, string> = { [SECRET_HEADER]: secret };
  const init: RequestInit = { method: options.method ?? "GET", headers };
  if (options.json !== undefined) {
    headers["content-type"] = "application/json";
    init.body = JSON.stringify(options.json);
  } else if (options.body !== undefined) {
    headers["content-type"] = "application/octet-stream";
    init.body = options.body as Uint8Array<ArrayBuffer>;
  }

  let response: Response | undefined;
  for (let attempt = 0; ; attempt++) {
    try {
      response = await fetch(url, init);
      break;
    } catch (e) {
      // Retry only when the connection was refused, so a request that may
      // have reached Rust (a reset mid-answer) never runs twice.
      const delay = isConnectionRefused(e) ? CONNECT_RETRY_DELAYS_MS[attempt] : undefined;
      if (delay === undefined) {
        console.error("[seaquel] license service unreachable:", e);
        throw new LicenseClientError(
          "UPSTREAM_UNAVAILABLE",
          "the license service is unavailable",
          502,
        );
      }
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }

  const text = await response.text();
  let parsed: unknown;
  try {
    parsed = text ? JSON.parse(text) : null;
  } catch {
    parsed = undefined;
  }
  if (!response.ok) {
    const err = parsed as { code?: unknown; message?: unknown } | undefined;
    if (err && typeof err.code === "string" && typeof err.message === "string") {
      throw new LicenseClientError(err.code, err.message, response.status);
    }
    throw new LicenseClientError(
      `HTTP_${response.status}`,
      `license service answered ${response.status}`,
      response.status,
    );
  }
  if (parsed === undefined) {
    throw new LicenseClientError(
      "PROTOCOL_ERROR",
      "license service answered with invalid JSON",
      response.status,
    );
  }
  return parsed as T;
}

// ---------------------------------------------------------------------------
// The gate, cached per user

const GATE_TTL_MS = 5_000;
const GATE_CACHE_MAX = 1_000;
const gateCache = new Map<string, { at: number; answer: GateAnswer }>();
/**
 * Bumped by `invalidateGate`. A gate call that started before an
 * invalidation doesn't store its (possibly stale) answer.
 */
let gateGeneration = 0;

/**
 * The license state, the user's membership row and the install facts the
 * `(app)` layout redirects on. Cached for 5 seconds per user (`null` is the
 * signed-out key); only successful answers are kept.
 */
export async function gate(userId: string | null): Promise<GateAnswer> {
  const key = userId ?? "";
  const now = Date.now();
  const hit = gateCache.get(key);
  if (hit && now - hit.at < GATE_TTL_MS) return hit.answer;

  const generation = gateGeneration;
  const answer = await call<GateAnswer>("gate", { user: userId });
  if (generation !== gateGeneration) return answer;
  if (gateCache.size >= GATE_CACHE_MAX) {
    for (const [k, v] of gateCache) {
      if (now - v.at >= GATE_TTL_MS) gateCache.delete(k);
    }
    if (gateCache.size >= GATE_CACHE_MAX) gateCache.clear();
  }
  gateCache.set(key, { at: now, answer });
  return answer;
}

/** Drop every cached gate answer: licensing state just changed. */
export function invalidateGate(): void {
  gateGeneration++;
  gateCache.clear();
}

// ---------------------------------------------------------------------------
// Signup

/** Whether the install has a tenant, and whether a bundle is stored, now. */
export function installStatus(): Promise<InstallStatus> {
  return call<InstallStatus>("install");
}

/**
 * First-owner registration: the control plane (or the bundle) creates the
 * tenant, and Rust writes the install cache.
 */
export async function registerInstall(licenseKey: string): Promise<void> {
  try {
    await call("register-install", { method: "POST", json: { licenseKey } });
  } finally {
    invalidateGate();
  }
}

/** `verifyMembershipLicense`: may this key join, and as what? */
export function signupCheck(licenseKey: string, email: string): Promise<VerifyResult> {
  return call<VerifyResult>("signup-check", { method: "POST", json: { licenseKey, email } });
}

/**
 * Bind a new user upstream and record their row (`isOwner` for the first
 * owner). `bound: false` when they already had a row.
 */
export async function bindMember(args: {
  licenseKey: string;
  userId: string;
  email: string;
  role: "owner" | "member";
  firstOwner: boolean;
}): Promise<{ bound: boolean }> {
  try {
    return await call<{ bound: boolean }>("bind-member", { method: "POST", json: args });
  } finally {
    invalidateGate();
  }
}

// ---------------------------------------------------------------------------
// Team

export function listMembers(): Promise<MemberView[]> {
  return call<MemberView[]>("members");
}

/**
 * Remove a member upstream, then their `member_license` row.
 * `localCleanupFailed` when the upstream removal worked but the row
 * couldn't be deleted (Rust logged why).
 */
export async function unbindMember(userId: string): Promise<{ localCleanupFailed: boolean }> {
  try {
    return await call<{ localCleanupFailed: boolean }>("unbind-member", {
      method: "POST",
      json: { userId },
    });
  } finally {
    invalidateGate();
  }
}

// ---------------------------------------------------------------------------
// Air gap

/** Mode, TTL timestamps, the user's row and the active bundle. */
export function airgapStatus(userId: string | null): Promise<AirgapStatus> {
  return call<AirgapStatus>("airgap/status", { user: userId });
}

/**
 * Verify and import a bundle. Refusals the route answers as they are (400,
 * 409) come back as an outcome; `UNAUTHORIZED` and `FORBIDDEN` throw.
 */
export async function airgapUpload(
  envelope: Uint8Array,
  userId: string | null,
): Promise<UploadOutcome> {
  try {
    return await call<UploadOutcome>("airgap/upload", {
      method: "POST",
      user: userId,
      body: envelope,
    });
  } finally {
    invalidateGate();
  }
}

/** Clear the bundle and go back online. Owner only (throws otherwise). */
export async function airgapClear(userId: string | null): Promise<void> {
  try {
    await call("airgap/clear", { method: "POST", user: userId });
  } finally {
    invalidateGate();
  }
}

/** Test-only: forget the gate cache and the auth.db check. */
export function _resetLicenseClient(): void {
  gateCache.clear();
  schemaReady = false;
}
