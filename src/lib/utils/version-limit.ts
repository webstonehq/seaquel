/**
 * The smallest `query_version_limit` / `dashboard_version_limit` the
 * settings accept. The inputs say `min="10"`, which a browser doesn't
 * enforce on the value, so a save clamps it too.
 */
export const MIN_VERSION_LIMIT = 10;

/** A typed version limit as saved: a whole number, at least `MIN_VERSION_LIMIT`. */
export function clampVersionLimit(value: number | null | undefined): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return MIN_VERSION_LIMIT;
  return Math.max(MIN_VERSION_LIMIT, Math.trunc(value));
}
