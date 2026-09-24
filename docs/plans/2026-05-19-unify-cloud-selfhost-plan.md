# Unify Cloud and Self-Hosted Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Cloud and self-hosted run the same code path. Deployment location becomes the only difference.

**Architecture:** Delete the `isCloudTenant()` mode switch entirely. License key + locally-generated `install_id` become the universal anchor. A persisted two-TTL cache (soft re-check + hard grace) replaces the in-memory 5-minute cache so self-hosted survives connectivity gaps with the same code Cloud runs.

**Tech Stack:** SvelteKit 5 + adapter-node (web target), Better Auth, `better-sqlite3` for `auth.db`, oxlint + svelte-check (no test runner — user opted out).

**Design doc:** `docs/plans/2026-05-19-unify-cloud-selfhost-design.md`

## Conventions for this plan

- **No test runner.** Verification at each task end is `npm run check` (svelte-check + sync) and where appropriate `npm run lint`. Behavior verification happens after the relevant phase via the dev server + curl/browser, called out explicitly.
- **No autonomous commits.** "Commit point" markers are where *you* should commit. CLAUDE.md is explicit: Claude never commits.
- **File references** use `path:line` from the seaquel-cloud branch's current working-tree state (which includes the uncommitted cloud-feature work this stacks on).

## Cross-repo prerequisite — call out loudly

This plan **cannot ship to production** until seaquel-app (`/Users/m/projects/github/webstonehq/seaquel-app/main`) exposes:

1. `POST /api/cloud/register-install` — body `{ licenseKey, installId }`, returns `TenantContext`. Idempotent on `(installId, subscriptionId)`.
2. Updated auth on all `/api/cloud/*` endpoints: accept `X-Install-Id` + `X-License-Key` instead of `X-Tenant-Auth`. The old header can stay during overlap.
3. Existing Cloud tenant backfill: every current tenant gets an install record keyed on its existing `tenantId`.

A paired plan in that repo is a prerequisite. The tasks below can be implemented and `npm run check`-clean against the existing seaquel-app endpoints by writing the new client code; only runtime behavior verification is blocked until seaquel-app ships.

---

## Phase 1 — Schema and install_id persistence

### Task 1: Find current migration system and highest-numbered migration

**Files (read):**
- `src/lib/server/auth.ts` (look for migration runner)
- `migrations/` or wherever migration files live

**Step 1:** Locate migration runner. The header in `src/lib/server/member-license.ts` says "Shape matches `migrations/007_member_license.sql`". Open `src/lib/server/auth.ts` and grep `migrations` to find how they're loaded.

**Step 2:** `ls migrations/` (or equivalent dir) and identify the highest-numbered file. New migration will be that number + 1.

**Step 3:** Note findings inline in the next task before writing code.

**No verification needed — pure recon.**

---

### Task 2: Create `008_install_and_validation_cache.sql` migration

**Files:**
- Create: `migrations/008_install_and_validation_cache.sql` (adjust number per Task 1 finding)

**Code:**

```sql
-- One row per install. Generated once at first boot, never injected from env.
CREATE TABLE IF NOT EXISTS install (
  id            INTEGER PRIMARY KEY CHECK (id = 1), -- singleton row
  install_id    TEXT NOT NULL UNIQUE,
  created_at    INTEGER NOT NULL
);

-- Cached install/tenant context. One singleton row alongside `install`.
CREATE TABLE IF NOT EXISTS install_cache (
  id                   INTEGER PRIMARY KEY CHECK (id = 1),
  tenant_id            TEXT,
  slug                 TEXT,
  status               TEXT,           -- provisioning | active | suspended | failed | deleting
  tier                 TEXT,
  seat_limit           INTEGER,
  current_period_end   TEXT,           -- ISO8601 or NULL
  last_validated_at    INTEGER NOT NULL, -- unix seconds
  grace_until          INTEGER NOT NULL  -- unix seconds; cache valid offline until this
);

-- Per-member validation cache. Extends the existing member_license model
-- without altering the original columns so reads from older code paths
-- keep working during rollout.
ALTER TABLE member_license ADD COLUMN last_validated_at INTEGER;
ALTER TABLE member_license ADD COLUMN cached_status     TEXT;
ALTER TABLE member_license ADD COLUMN cached_tier       TEXT;
ALTER TABLE member_license ADD COLUMN grace_until       INTEGER;
```

**Step 1:** Write the file with the SQL above.

**Step 2:** Run `npm run check` — should pass (SQL files aren't type-checked, but make sure nothing broke).

**Commit point.** Suggested message: `feat(db): add install + validation_cache schema`.

---

### Task 3: Add install_id generator/accessor

**Files:**
- Create: `src/lib/server/install.ts`

**Code:**

```ts
/**
 * Local install identity. Generated once at first boot, persisted in
 * auth.db, never read from env. Both Cloud and self-hosted use this
 * same value — Cloud's "provisioning" no longer injects anything.
 */
import { randomUUID } from "node:crypto";
import { openAuthDb } from "./auth";

let cached: string | null = null;

export function getOrCreateInstallId(): string {
  if (cached) return cached;
  const db = openAuthDb();
  const existing = db
    .prepare(`SELECT install_id FROM install WHERE id = 1`)
    .get() as { install_id: string } | undefined;
  if (existing) {
    cached = existing.install_id;
    return cached;
  }

  // Legacy migration shim: if SEAQUEL_TENANT_ID is set (pre-unification
  // Cloud container), adopt it as install_id once so existing tenants
  // don't need to re-bind. Remove this branch after a release.
  const legacy = process.env.SEAQUEL_TENANT_ID?.trim();
  const installId = legacy && legacy.length > 0 ? legacy : randomUUID();

  db.prepare(
    `INSERT INTO install (id, install_id, created_at) VALUES (1, ?, ?)`,
  ).run(installId, Math.floor(Date.now() / 1000));
  cached = installId;
  return installId;
}

/** Test-only — clears the in-process cache. */
export function _resetInstallCache(): void {
  cached = null;
}
```

**Step 1:** Create the file.

**Step 2:** `npm run check` — expect zero errors.

**Step 3:** `npm run lint` — expect clean.

**Commit point.** Suggested message: `feat(server): add install_id generator with legacy SEAQUEL_TENANT_ID shim`.

---

## Phase 2 — Rename and refactor `cloud.ts` → `licensing.ts`

### Task 4: Rename file and update imports

**Files:**
- Rename: `src/lib/server/cloud.ts` → `src/lib/server/licensing.ts`
- Modify imports in:
  - `src/app.d.ts:3` — `import type { TenantContext } from "$lib/server/cloud";`
  - `src/hooks.server.ts:6` — `import { isCloudTenant, tenantInfo } from "$lib/server/cloud";`
  - `src/routes/api/signup/+server.ts:26-31`
  - `src/routes/api/team/[containerUserId]/+server.ts:12`
  - any other importer (grep first)

**Step 1:** `grep -rn 'from "\$lib/server/cloud"' src` to enumerate importers.

**Step 2:** `git mv src/lib/server/cloud.ts src/lib/server/licensing.ts`.

**Step 3:** Update every import path from `"$lib/server/cloud"` to `"$lib/server/licensing"`. Leave `isCloudTenant`/`tenantInfo` names alone in this task — only the path changes.

**Step 4:** `npm run check` — expect clean.

**Commit point.** Suggested message: `refactor(server): rename cloud.ts to licensing.ts (no behavior change)`.

---

### Task 5: Replace `X-Tenant-Auth` with `X-Install-Id` + `X-License-Key` in `licensingFetch`

**Files:**
- Modify: `src/lib/server/licensing.ts:174-188` (the `cloudFetch` function — also rename to `licensingFetch`)

**Code:** Replace the function body with:

```ts
async function licensingFetch(
  cfg: LicensingConfig,
  method: "GET" | "POST",
  path: string,
  body?: unknown,
): Promise<Response> {
  return fetch(`${cfg.controlUrl}${path}`, {
    method,
    headers: {
      "X-Install-Id": cfg.installId,
      "X-License-Key": cfg.ownerLicenseKey,
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}
```

`cfg` shape changes — see Task 6 for the new `LicensingConfig`.

**Step 1:** Edit the function.

**Step 2:** Update the type `CloudConfig` → `LicensingConfig` at `licensing.ts:21-24`:

```ts
interface LicensingConfig {
  controlUrl: string;
  installId: string;
  ownerLicenseKey: string;
}
```

**Step 3:** `npm run check` will fail because callers still pass the old shape. That's expected — Task 6 fixes the resolver. Move on.

**No commit yet — leave broken until Task 6.**

---

### Task 6: Replace `cloudConfig()` with `licensingConfig()` keyed on install_id

**Files:**
- Modify: `src/lib/server/licensing.ts:71-77`

**Code:** Replace `cloudConfig()` with:

```ts
function licensingConfig(): LicensingConfig | null {
  const controlUrl = (
    process.env.SEAQUEL_CONTROL_URL ?? "https://seaquel.app"
  ).replace(/\/$/, "");
  const installId = getOrCreateInstallId();
  const ownerLicenseKey = readOwnerLicenseKey();
  if (!ownerLicenseKey) return null;
  return { controlUrl, installId, ownerLicenseKey };
}
```

`readOwnerLicenseKey()` is a new helper — see Task 7. For now stub it:

```ts
function readOwnerLicenseKey(): string | null {
  // TODO(Task 7): read from install_cache once register-install has run
  return null;
}
```

**Step 1:** Import `getOrCreateInstallId` from `./install`.

**Step 2:** Replace `cloudConfig` with `licensingConfig` (function name change ripples — keep going).

**Step 3:** Delete `isCloudTenant()` (`licensing.ts:80-82`). Every caller will be fixed in later tasks.

**Step 4:** `npm run check` — will show errors at callers of `isCloudTenant()` and `cloudConfig`. Note them for the next tasks.

**No commit yet.**

---

### Task 7: Add `readOwnerLicenseKey()` backed by `install_cache`

**Files:**
- Modify: `src/lib/server/licensing.ts` (replace the stub from Task 6)

**Code:**

```ts
function readOwnerLicenseKey(): string | null {
  // Owner key is persisted at register-install time. We don't store it
  // long-term in the clear — it's already in the user's member_license
  // row (the owner is a user too). Resolve by joining install_cache
  // with the owner's member_license entry.
  const db = openAuthDb();
  const row = db
    .prepare(
      `SELECT ml.license_key AS key
         FROM install_cache ic
         JOIN member_license ml ON ml.user_id = ic.tenant_id IS NOT NULL -- owner marker, see Task 13
         LIMIT 1`,
    )
    .get() as { key: string } | undefined;
  return row?.key ?? null;
}
```

> **Design note:** The owner is identified by a flag added to `member_license` in Task 13 (`is_owner INTEGER NOT NULL DEFAULT 0`). Until Task 13 lands, this query returns null, which is fine — pre-register-install code paths gate on that anyway.

**Step 1:** Replace the stub.

**Step 2:** `npm run check` — still expect failures at call sites of removed `isCloudTenant`. Keep going.

**No commit yet.**

---

### Task 8: Add `registerInstall()`

**Files:**
- Modify: `src/lib/server/licensing.ts` (add new exported function)

**Code:**

```ts
/**
 * First owner-key signup self-registers the install on the control
 * plane. Idempotent: the same (installId, subscription) returns the
 * existing tenant context. Called from /api/signup when the first
 * owner key is submitted.
 */
export async function registerInstall(
  licenseKey: string,
): Promise<TenantContext> {
  const installId = getOrCreateInstallId();
  const controlUrl = (
    process.env.SEAQUEL_CONTROL_URL ?? "https://seaquel.app"
  ).replace(/\/$/, "");
  const res = await fetch(`${controlUrl}/api/cloud/register-install`, {
    method: "POST",
    headers: {
      "X-Install-Id": installId,
      "X-License-Key": licenseKey,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ installId, licenseKey }),
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`register-install failed: ${res.status} ${body}`);
  }
  return (await res.json()) as TenantContext;
}
```

**Step 1:** Add the function after the existing `tenantInfo`/`verifyMembershipLicense` block.

**Step 2:** `npm run check` — same expected failures from removed `isCloudTenant`. Move on.

**No commit yet.**

---

## Phase 3 — Persisted grace-period cache and gate

### Task 9: Add cache read/write helpers

**Files:**
- Create: `src/lib/server/license-cache.ts`

**Code:**

```ts
/**
 * Persisted license validation cache. Replaces the in-memory 5-minute
 * cachedTenant in licensing.ts. Two TTLs:
 *   - Soft TTL (default 24h): re-check interval; layout gate triggers
 *     a control-plane refresh once it lapses.
 *   - Hard TTL (default 14d): grace window; offline access stops here.
 * Both env-overridable, identical in Cloud and self-hosted.
 */
import { openAuthDb } from "./auth";
import type { TenantContext } from "./licensing";

const DEFAULT_SOFT_TTL_S = 24 * 60 * 60;
const DEFAULT_GRACE_TTL_S = 14 * 24 * 60 * 60;

export function softTtlSeconds(): number {
  return parseEnvSeconds("SEAQUEL_LICENSE_SOFT_TTL", DEFAULT_SOFT_TTL_S);
}
export function graceTtlSeconds(): number {
  return parseEnvSeconds("SEAQUEL_LICENSE_GRACE_TTL", DEFAULT_GRACE_TTL_S);
}

export interface InstallCache {
  tenantId: string | null;
  slug: string | null;
  status: TenantContext["status"] | null;
  tier: string | null;
  seatLimit: number | null;
  currentPeriodEnd: string | null;
  lastValidatedAt: number;
  graceUntil: number;
}

export function readInstallCache(): InstallCache | null {
  const row = openAuthDb()
    .prepare(
      `SELECT tenant_id AS tenantId, slug, status, tier,
              seat_limit AS seatLimit,
              current_period_end AS currentPeriodEnd,
              last_validated_at AS lastValidatedAt,
              grace_until AS graceUntil
         FROM install_cache WHERE id = 1`,
    )
    .get() as InstallCache | undefined;
  return row ?? null;
}

export function writeInstallCache(ctx: TenantContext): void {
  const now = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `INSERT INTO install_cache
         (id, tenant_id, slug, status, tier, seat_limit,
          current_period_end, last_validated_at, grace_until)
       VALUES (1, @tenantId, @slug, @status, @tier, @seatLimit,
               @currentPeriodEnd, @lastValidatedAt, @graceUntil)
       ON CONFLICT(id) DO UPDATE SET
         tenant_id          = excluded.tenant_id,
         slug               = excluded.slug,
         status             = excluded.status,
         tier               = excluded.tier,
         seat_limit         = excluded.seat_limit,
         current_period_end = excluded.current_period_end,
         last_validated_at  = excluded.last_validated_at,
         grace_until        = excluded.grace_until`,
    )
    .run({
      tenantId: ctx.tenantId,
      slug: ctx.slug,
      status: ctx.status,
      tier: ctx.tier,
      seatLimit: ctx.seatLimit,
      currentPeriodEnd: ctx.currentPeriodEnd,
      lastValidatedAt: now,
      graceUntil: now + graceTtlSeconds(),
    });
}

function parseEnvSeconds(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}
```

**Step 1:** Create the file.

**Step 2:** `npm run check` — expect failures only at Phase-2 leftovers. The new file should be clean.

**No commit yet.**

---

### Task 10: Add `resolveLicenseState()` — the 4-branch ladder

**Files:**
- Modify: `src/lib/server/licensing.ts` (add new exported function near `tenantInfo`)

**Code:**

```ts
export type LicenseState =
  | { kind: "ok"; tenant: TenantContext }
  | { kind: "suspended"; tenant: TenantContext }
  | { kind: "revalidate" }       // grace expired, control plane unreachable
  | { kind: "unregistered" };    // no owner key yet — first-run

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
      writeInstallCache(fresh);
      return toState({ ...cache, ...fresh } as InstallCache);
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
```

**Step 1:** Add imports: `readInstallCache, writeInstallCache, softTtlSeconds, graceTtlSeconds, type InstallCache` from `./license-cache`.

**Step 2:** Add the function and helper.

**Step 3:** Delete the in-memory `cachedTenant` / `CACHE_TTL_MS` / `invalidateTenantCache` / cache-aware branches inside `tenantInfo` (`licensing.ts:68-69, 84-110`). `tenantInfo` becomes a thin wrapper that just hits the control plane and returns the raw response — caching is now `license-cache.ts`'s job.

**Step 4:** `npm run check` — should now reduce to errors at the original `isCloudTenant` call sites (hooks, layout, signup, team).

**No commit yet — Phase 4 fixes them.**

---

## Phase 4 — Delete the mode switch

### Task 11: Update `hooks.server.ts` — always populate `locals.tenant` via resolver

**Files:**
- Modify: `src/hooks.server.ts:6, 77-91`

**Code:** Replace the import and the cloud-context block:

```ts
import { resolveLicenseState } from "$lib/server/licensing";
```

Replace lines 77-91 with:

```ts
// License/tenant context: always resolved through the persisted cache
// + grace-period ladder. No deployment-mode branch. When no owner has
// registered an install yet, state is "unregistered" and locals.tenant
// stays null — the signup flow handles that case.
const state = await resolveLicenseState();
event.locals.tenant = state.kind === "ok" || state.kind === "suspended"
  ? state.tenant
  : null;
event.locals.licenseState = state.kind;
```

**Step 1:** Add `licenseState` to `App.Locals` in `src/app.d.ts` (see Task 12).

**Step 2:** Make the edit.

**Step 3:** `npm run check` — should clear the hooks errors; layout/signup/team still error.

**No commit yet.**

---

### Task 12: Update `app.d.ts` types

**Files:**
- Modify: `src/app.d.ts`

**Code:** Replace the file body's `Locals` block:

```ts
import type { User, Session } from "$lib/server/session-types";
import type { TenantContext, LicenseState } from "$lib/server/licensing";

declare global {
  namespace App {
    interface Locals {
      /** Authenticated user for this request, or null if unauthenticated. */
      user: User | null;
      /** Active session, or null if unauthenticated. */
      session: Session | null;
      /**
       * Tenant context resolved from install_cache + control plane.
       * Null only when no owner has registered an install yet, or when
       * grace has expired and the control plane is unreachable.
       * Populated in `hooks.server.ts` for every request.
       */
      tenant: TenantContext | null;
      /** Coarse state of the licensing gate for this request. */
      licenseState: LicenseState["kind"];
    }
  }
}

export {};
```

**Step 1:** Edit.

**Step 2:** `npm run check` — layout/signup/team errors remain; hooks ones clear.

**No commit yet.**

---

### Task 13: Add `is_owner` column to `member_license`

**Files:**
- Create: `migrations/009_member_license_is_owner.sql`
- Modify: `src/lib/server/member-license.ts`

**SQL:**

```sql
ALTER TABLE member_license ADD COLUMN is_owner INTEGER NOT NULL DEFAULT 0;
```

**Code change in `member-license.ts`:**

```ts
export interface MemberLicense {
  userId: string;
  licenseKey: string;
  boundAt: number;
  controlMemberId: string;
  isOwner: boolean;
}
```

Update `findByUserId` SELECT to include `is_owner AS isOwner` (cast 0/1 to boolean):

```ts
const row = db.prepare(
  `SELECT user_id AS userId, license_key AS licenseKey,
          bound_at AS boundAt, control_member_id AS controlMemberId,
          is_owner AS isOwner
     FROM member_license WHERE user_id = ?`,
).get(userId) as (Omit<MemberLicense, "isOwner"> & { isOwner: number }) | undefined;
return row ? { ...row, isOwner: row.isOwner === 1 } : null;
```

Update `insert` to write `is_owner`:

```ts
openAuthDb().prepare(
  `INSERT INTO member_license
     (user_id, license_key, bound_at, control_member_id, is_owner)
   VALUES (@userId, @licenseKey, @boundAt, @controlMemberId, @isOwner)`,
).run({ ...row, isOwner: row.isOwner ? 1 : 0 });
```

Also update `readOwnerLicenseKey()` in `licensing.ts` (replace the placeholder query from Task 7):

```ts
function readOwnerLicenseKey(): string | null {
  const row = openAuthDb()
    .prepare(`SELECT license_key AS key FROM member_license WHERE is_owner = 1 LIMIT 1`)
    .get() as { key: string } | undefined;
  return row?.key ?? null;
}
```

**Step 1–4:** Apply all edits.

**Step 5:** `npm run check`.

**Commit point.** Suggested message: `feat(db): mark owner in member_license + read owner key for control-plane auth`.

---

### Task 14: Update `(app)/+layout.server.ts` — always run the gate

**Files:**
- Modify: `src/routes/(app)/+layout.server.ts:1-51`

**Code:** Replace the file entirely:

```ts
import { redirect } from "@sveltejs/kit";
import { building } from "$app/environment";
import type { LayoutServerLoad } from "./$types";
import { findByUserId } from "$lib/server/member-license";

// Server-side gate for the main app shell. One path for Cloud and
// self-hosted alike: anyone authenticated must also be bound to a
// license seat for the install, and the install must be in a usable
// license state (ok / not suspended / not past the grace window).
export const load: LayoutServerLoad = ({ locals, url }) => {
  if (building) return {};
  if (process.env.BUILD_TARGET !== "web") return {};

  if (!locals.user) {
    const here = url.pathname + url.search;
    throw redirect(302, `/login?redirect=${encodeURIComponent(here)}`);
  }

  // License state ladder. "unregistered" → signup will handle the
  // first-owner self-register; "revalidate" → grace expired offline;
  // "suspended" → explicit suspension from the control plane.
  if (locals.licenseState === "unregistered") {
    if (!url.pathname.startsWith("/signup")) {
      throw redirect(302, `/signup?redirect=${encodeURIComponent(url.pathname + url.search)}`);
    }
  } else if (locals.licenseState === "suspended") {
    if (!url.pathname.startsWith("/suspended")) {
      throw redirect(302, "/suspended");
    }
  } else if (locals.licenseState === "revalidate") {
    if (!url.pathname.startsWith("/revalidate")) {
      throw redirect(302, "/revalidate");
    }
  } else {
    // ok — enforce per-user membership binding.
    const bound = findByUserId(locals.user.id);
    if (!bound) {
      const here = url.pathname + url.search;
      throw redirect(302, `/signup?reason=membership&redirect=${encodeURIComponent(here)}`);
    }
  }

  return { tenant: locals.tenant };
};
```

**Step 1:** Replace the file.

**Step 2:** `npm run check` — layout errors clear; signup and team remain.

**No commit yet.**

---

### Task 15: Update `api/signup/+server.ts` — license key always required

**Files:**
- Modify: `src/routes/api/signup/+server.ts:60-152`

**Code:** Replace the body of the `POST` handler from line ~60 down with:

```ts
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
      throw error(400, "could not register install with the control plane");
    }
    writeInstallCache(registered);
  }

  // Verify the membership license against the (now-registered) install.
  const verified = await verifyMembershipLicense(licenseKey, email);
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
    return new Response(text, { status: authResponse.status, headers: authResponse.headers });
  }

  let userId: string | null = null;
  try {
    const data = (await authResponse.clone().json()) as { user?: { id?: string } };
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
    });
    insertMemberLicense({
      userId,
      licenseKey,
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: bound.tenantMemberId,
      isOwner: isFirstOwner, // first owner-key signup marks this user
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
```

**Step 1:** Update imports — drop `isCloudTenant, tenantInfo`; add `registerInstall, readInstallCache, writeInstallCache, type TenantContext`.

**Step 2:** Apply the replacement.

**Step 3:** `npm run check` — signup clears.

**No commit yet.**

---

### Task 16: Update team API — remove `isCloudTenant` guard

**Files:**
- Modify: `src/routes/api/team/[containerUserId]/+server.ts:12, 18`
- Modify: `src/routes/api/team/+server.ts` (check for the same guard)

**Step 1:** Open both files. In each, remove the `isCloudTenant()` import and the `if (!isCloudTenant()) throw error(400, ...)` line.

**Step 2:** `npm run check` — should be clean.

**Step 3:** `npm run lint`.

**Commit point.** Suggested message: `feat: collapse cloud/self-hosted mode switch into a single license-anchored path`.

> This is the big intermediate commit. The codebase compiles cleanly and the mode switch is gone. UI tasks follow.

---

## Phase 5 — UI updates

### Task 17: Update `signup/+page.svelte` — always show the license-key field

**Files:**
- Modify: `src/routes/signup/+page.svelte`

**Step 1:** Remove the `isCloud = $derived(!!data.tenant)` and `isSuspended` derived (or keep `isSuspended` if it's used). Delete every `{#if isCloud}…` / `{#if !isCloud}…` conditional — the key field, copy, and submit body now render unconditionally.

**Step 2:** Update the `fetch("/api/signup", …)` body to always include `licenseKey`: remove the `...(isCloud ? { licenseKey } : {})` spread.

**Step 3:** When `data.tenant` is present, pre-fill the key field if a query param `?key=…` is in the URL — that's how Cloud's provisioning seeds the field. Add:

```ts
import { page } from "$app/state";
// inside the script block:
let licenseKey = $state(page.url.searchParams.get("key") ?? "");
```

**Step 4:** `npm run check`. Use the Svelte MCP autofixer afterwards: `mcp__svelte__svelte-autofixer` with the file contents until clean.

**No commit yet — pair with Task 18.**

---

### Task 18: Update `signup/+page.server.ts` — tenant data optional, no longer mode-switching

**Files:**
- Modify: `src/routes/signup/+page.server.ts`

**Code:** Replace `load`:

```ts
export const load: PageServerLoad = ({ locals }) => {
  // Surface the install/tenant context if it exists so the signup page
  // can render "Join {slug}" copy. Otherwise this is the very first
  // signup on this install — the form still renders, just without
  // tenant-flavored copy.
  return {
    tenant: locals.tenant
      ? {
          slug: locals.tenant.slug,
          status: locals.tenant.status,
          ownerEmail: locals.tenant.ownerEmail,
        }
      : null,
  };
};
```

The header comment about "self-hosted path" is stale — replace it:

```ts
/**
 * Surfaces the install/tenant context (if any) to the signup form so
 * the UI can render "Join {tenant.slug}" copy. Pre-registration (no
 * install_cache row yet) returns tenant: null and the form runs the
 * first-owner self-register flow on submit.
 */
```

**Step 1:** Apply edits.

**Step 2:** `npm run check`.

**Commit point.** Suggested message: `feat(ui): unify signup page — license key always required`.

---

### Task 19: Update `license-section.svelte` — drop the self-hosted branch

**Files:**
- Modify: `src/lib/components/settings/general/license-section.svelte:75-87`

**Step 1:** Find the block:

```svelte
{#if !tenantLicenseStore.loaded}
  <p>Loading…</p>
{:else if tenantLicenseStore.loadError}
  …
{:else if !tenantLicenseStore.tier}
  <p>This instance is self-hosted; no Seaquel license attached.</p>
{:else}
  …
```

Remove the `!tenantLicenseStore.tier` branch entirely. After the `loadError` branch, go straight to the license card. If the tenant store can still produce a no-tier state, render the same loading/error treatment — never "self-hosted, no license."

**Step 2:** `npm run check` + Svelte autofixer pass.

**Commit point.** Suggested message: `feat(ui): remove self-hosted "no license attached" branch from license section`.

---

## Phase 6 — `/revalidate` route

### Task 20: Create the revalidate page

**Files:**
- Create: `src/routes/(app)/revalidate/+page.svelte`
- Create: `src/routes/(app)/revalidate/+page.server.ts`

**`+page.server.ts`:**

```ts
import type { PageServerLoad } from "./$types";
import { readInstallCache } from "$lib/server/license-cache";

export const prerender = false;

export const load: PageServerLoad = () => {
  const cache = readInstallCache();
  return {
    lastValidatedAt: cache?.lastValidatedAt ?? null,
    graceUntil: cache?.graceUntil ?? null,
  };
};
```

**`+page.svelte`:** model after `src/routes/(app)/suspended/+page.svelte` (read its structure first). Copy: "We couldn't confirm your license. Reconnect to the network and reload, or re-enter your license key." Include a button to re-run `/api/signup` re-validation or link to `/signup`. Use `bits-ui` Card + Button, same as suspended.

**Step 1:** Read `src/routes/(app)/suspended/+page.svelte` to match style.

**Step 2:** Create both files.

**Step 3:** `npm run check`. Svelte autofixer pass.

**Commit point.** Suggested message: `feat: add /revalidate page for grace-expired offline state`.

---

## Phase 7 — Legacy migration shim and cache backfill

### Task 21: Cache backfill on first boot post-upgrade

**Files:**
- Modify: `src/lib/server/auth.ts` (or wherever first-boot migration logic lives — verify in Task 1)
- New helper in: `src/lib/server/license-cache.ts`

**Code (in `license-cache.ts`):**

```ts
/**
 * One-time backfill: on first boot after the unification release, every
 * pre-existing member_license row gets last_validated_at = now and
 * grace_until = now + grace_ttl so rolling forward doesn't lock anyone
 * out before the first scheduled re-check. Safe to call repeatedly —
 * only rows with NULL cache columns are touched.
 */
export function backfillExistingMembers(): void {
  const now = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `UPDATE member_license
         SET last_validated_at = COALESCE(last_validated_at, ?),
             grace_until       = COALESCE(grace_until, ?),
             cached_status     = COALESCE(cached_status, 'active')
       WHERE last_validated_at IS NULL`,
    )
    .run(now, now + graceTtlSeconds());
}
```

**Step 1:** Add the function.

**Step 2:** Wire it into the auth-bootstrap path so it runs once after the new migrations are applied (likely inside `openAuthDb()` or the migration runner — confirm from Task 1).

**Step 3:** `npm run check`.

**Commit point.** Suggested message: `feat: backfill validation cache for existing member_license rows`.

---

### Task 22: Verify the legacy `SEAQUEL_TENANT_ID` → `install_id` shim works

> Already implemented in Task 3. This task is a manual verification step.

**Steps:**

1. Stop any local dev server.
2. Move `auth.db` aside: `mv auth.db auth.db.bak`.
3. Start the web dev server with `SEAQUEL_TENANT_ID=test-legacy-tenant npm run dev`.
4. Hit any web route (e.g. `curl http://localhost:5173/`).
5. Verify `auth.db` has `SELECT install_id FROM install` returning `test-legacy-tenant`.
6. Restore: `rm auth.db && mv auth.db.bak auth.db`.

**No commit — verification only.**

---

## Phase 8 — Docs

### Task 23: Update `README.md`

**Files:**
- Modify: `README.md`

**Changes:**

1. **Delete line 170** (`curl http://localhost:8787/api/tenant/status`) and its surrounding block. That endpoint doesn't exist.
2. **Rewrite lines 98-100** ("first user automatically becomes the tenant Owner; subsequent signups require an invite link"). Replace with:

   > The first user to sign up presents an owner license key from a Seaquel subscription — that key registers this install with seaquel.app and binds the user as Owner. Subsequent teammates sign up by pasting their own member license key from the same subscription's seat pool.

3. **Env table (lines 114-127):** add a row:

   | Variable                       | Required | Purpose                                                              |
   | ------------------------------ | -------- | -------------------------------------------------------------------- |
   | `SEAQUEL_CONTROL_URL`          | No       | Override the licensing API base URL. Defaults to `https://seaquel.app`. |
   | `SEAQUEL_LICENSE_SOFT_TTL`     | No       | Seconds between licence re-checks. Default `86400` (24 h).            |
   | `SEAQUEL_LICENSE_GRACE_TTL`    | No       | Offline grace window in seconds. Default `1209600` (14 d).            |

   Remove any `SEAQUEL_TENANT_ID` mention.

4. Add a sentence to the "Self-host (Docker)" intro: "A Seaquel subscription license key is required at first signup — same as Cloud."

**Step 1:** Apply all edits.

**Step 2:** `npm run check` (sanity).

**Commit point.** Suggested message: `docs: align README with unified cloud/self-hosted model`.

---

### Task 24: Mirror env changes into `deploy/docker/.env.example`

**Files:**
- Modify: `deploy/docker/.env.example`

**Step 1:** Open the file. Remove any `SEAQUEL_TENANT_ID=` example. Add commented lines for `SEAQUEL_CONTROL_URL`, `SEAQUEL_LICENSE_SOFT_TTL`, `SEAQUEL_LICENSE_GRACE_TTL` with their defaults.

**Step 2:** Add a comment noting that a license key is required at first signup.

**Commit point.** Suggested message: `docs(deploy): document new licensing env vars`.

---

## Phase 9 — Final cleanup

### Task 25: Search for orphaned `isCloudTenant` references

**Step 1:** `grep -rn "isCloudTenant\|SEAQUEL_TENANT_ID" src src-tauri 2>/dev/null`

**Step 2:** Any remaining hits other than the legacy shim in `install.ts` are bugs — fix or remove.

**Step 3:** `npm run check` + `npm run lint`.

**Commit point if anything was found.** Suggested message: `chore: remove last orphaned cloud-mode references`.

---

### Task 26: End-to-end smoke (manual)

Run the dev server. Walk through:

1. Fresh database (delete `auth.db`): `/signup` → first owner key → register-install → owner-bound; revisiting `/` lands in the app.
2. Add a teammate: sign out, sign up with a member key from the same subscription → bound, in.
3. Bad key → 400 with `license_not_found` or similar.
4. Force grace mode: in the dev shell, `update install_cache set last_validated_at = 0;` and point `SEAQUEL_CONTROL_URL` at a non-routable host (e.g. `http://127.0.0.1:1`); reload `/` → app still works (within `grace_until`).
5. Force `/revalidate`: `update install_cache set grace_until = 0;` → reload `/` → redirected to `/revalidate`.
6. Restore.

This is the only verification of the integrated behavior; without a test runner there's no substitute. Document any surprises before declaring done.

**No commit.**

---

## Done state

When all tasks are committed:

- `grep -rn isCloudTenant src` returns zero hits.
- `grep -rn SEAQUEL_TENANT_ID src` returns one hit — the legacy shim in `install.ts`.
- `npm run check` and `npm run lint` pass clean.
- `/signup`, `/suspended`, `/revalidate`, and `/api/team/*` work in dev with a freshly-deleted `auth.db` against a staging `SEAQUEL_CONTROL_URL`.

The seaquel side is ready to ship the moment the seaquel-app side ships the new endpoints. Until then, behavior verification beyond `npm run check` is blocked.
