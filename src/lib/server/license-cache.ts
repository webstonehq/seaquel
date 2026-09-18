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
  mode: "online" | "airgap";
}

export function readInstallCache(): InstallCache | null {
  const row = openAuthDb()
    .prepare(
      `SELECT tenant_id AS tenantId, slug, status, tier,
              seat_limit AS seatLimit,
              current_period_end AS currentPeriodEnd,
              last_validated_at AS lastValidatedAt,
              grace_until AS graceUntil,
              mode
         FROM install_cache WHERE id = 1`,
    )
    .get() as (Omit<InstallCache, "mode"> & { mode: string | null }) | undefined;
  if (!row) return null;
  // Defensive: column is NOT NULL DEFAULT 'online' but treat anything
  // outside the expected enum as 'online' so a corrupt row doesn't lock
  // the install into air-gap mode.
  const mode: InstallCache["mode"] = row.mode === "airgap" ? "airgap" : "online";
  return { ...row, mode };
}

export function writeInstallCache(ctx: TenantContext, mode: "online" | "airgap" = "online"): void {
  const now = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `INSERT INTO install_cache
         (id, tenant_id, slug, status, tier, seat_limit,
          current_period_end, last_validated_at, grace_until, mode)
       VALUES (1, @tenantId, @slug, @status, @tier, @seatLimit,
               @currentPeriodEnd, @lastValidatedAt, @graceUntil, @mode)
       ON CONFLICT(id) DO UPDATE SET
         tenant_id          = excluded.tenant_id,
         slug               = excluded.slug,
         status             = excluded.status,
         tier               = excluded.tier,
         seat_limit         = excluded.seat_limit,
         current_period_end = excluded.current_period_end,
         last_validated_at  = excluded.last_validated_at,
         grace_until        = excluded.grace_until,
         mode               = excluded.mode`,
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
      mode,
    });
}

/**
 * Flip the install's mode between 'online' and 'airgap'. Idempotent.
 *
 * In air-gap mode, Task 8's `/api/airgap/bundle` POST may need to set
 * the mode *before* signup happens — so there's no `install_cache` row
 * yet. In that case we synthesise a row with NULL tenant context plus
 * the current timestamps; a later online validation (or the bundle's
 * own dispatcher) will overwrite the tenant fields. Without this
 * fallback the UPDATE would silently no-op and the install would stay
 * stuck in 'online'.
 */
export function setMode(mode: "online" | "airgap"): void {
  const result = openAuthDb().prepare(`UPDATE install_cache SET mode = ? WHERE id = 1`).run(mode);
  if (result.changes > 0) return;

  const now = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `INSERT INTO install_cache
         (id, tenant_id, slug, status, tier, seat_limit,
          current_period_end, last_validated_at, grace_until, mode)
       VALUES (1, NULL, NULL, NULL, NULL, NULL,
               NULL, ?, ?, ?)
       ON CONFLICT(id) DO UPDATE SET mode = excluded.mode`,
    )
    .run(now, now + graceTtlSeconds(), mode);
}

function parseEnvSeconds(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const n = parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}

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
