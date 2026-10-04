/**
 * Keeping a shown row's object when a reload reads it back unchanged (phase
 * 7a Task 1 review). Every `external` event (another process wrote the
 * file) reloads every list, and replacing each row's object with an equal
 * new one would re-render every dashboard, connection and query it touches.
 */

/**
 * Whether `a` and `b` hold the same data: the same primitives, `Date`s with
 * the same time, and arrays and plain objects whose entries are the same
 * data (a key holding `undefined` counts as absent). Shared references (a
 * widget's result rows) compare by identity first, so they cost nothing.
 */
export function sameData(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (a instanceof Date || b instanceof Date) {
    return a instanceof Date && b instanceof Date && a.getTime() === b.getTime();
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((item, i) => sameData(item, b[i]));
  }
  if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
  // A key holding `undefined` counts as absent (`dashboardFromWire` copies a
  // widget's run state, `undefined` until it runs, onto the stored widget).
  const ra = a as Record<string, unknown>;
  const rb = b as Record<string, unknown>;
  const ka = Object.keys(ra).filter((k) => ra[k] !== undefined);
  const kb = Object.keys(rb).filter((k) => rb[k] !== undefined);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => sameData(ra[k], rb[k]));
}

/** `current` when `updated` holds the same data, else `updated`. */
export function keepSame<T>(current: T | undefined, updated: T): T {
  return current !== undefined && sameData(current, updated) ? current : updated;
}
