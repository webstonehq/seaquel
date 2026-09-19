/**
 * Process-local per-IP rate limiter.
 *
 * Suitable for the per-tenant container model: each container is one
 * Node process serving one tenant's worth of traffic. A `Map<ip, hits[]>`
 * gives us a fixed-window limiter without an external store. For a
 * horizontally-scaled deployment, swap the storage to Redis or a tenant-
 * adjacent durable object.
 *
 * Used by `/api/signup` to make license-key brute-forcing infeasible at
 * the rates a real user would generate; Better Auth's own rate-limiting
 * covers its `/api/auth/*` endpoints (sign-in, forgot-password, etc.) via
 * the `rateLimit` config in `auth.ts`.
 */

interface RateLimitConfig {
  /** Window size, in milliseconds. */
  windowMs: number;
  /** Maximum allowed hits per IP within the window. */
  max: number;
}

export interface RateLimitResult {
  ok: boolean;
  /** Seconds the caller should wait before retrying. 0 when `ok = true`. */
  retryAfter: number;
}

// IP → recent hit timestamps (ms). Trimmed lazily on each check, plus a
// periodic full sweep so an attacker hitting many distinct IPs once
// each can't leave entries that age out but never get collected.
const HITS = new Map<string, number[]>();
const SWEEP_INTERVAL_MS = 60_000;
let lastSweep = 0;

export function checkRateLimit(ip: string, cfg: RateLimitConfig): RateLimitResult {
  if (!ip) return { ok: true, retryAfter: 0 };

  const now = Date.now();
  const windowStart = now - cfg.windowMs;

  if (now - lastSweep > SWEEP_INTERVAL_MS) {
    sweep(windowStart);
    lastSweep = now;
  }

  const recent = (HITS.get(ip) ?? []).filter((t) => t > windowStart);
  if (recent.length >= cfg.max) {
    const oldest = recent[0];
    const retryAfterMs = oldest + cfg.windowMs - now;
    return { ok: false, retryAfter: Math.max(1, Math.ceil(retryAfterMs / 1000)) };
  }
  recent.push(now);
  HITS.set(ip, recent);
  return { ok: true, retryAfter: 0 };
}

function sweep(cutoff: number): void {
  for (const [ip, hits] of HITS.entries()) {
    const kept = hits.filter((t) => t > cutoff);
    if (kept.length === 0) HITS.delete(ip);
    else HITS.set(ip, kept);
  }
}

/** Test-only: clear all tracked hits. */
export function _resetRateLimit(): void {
  HITS.clear();
  lastSweep = 0;
}
