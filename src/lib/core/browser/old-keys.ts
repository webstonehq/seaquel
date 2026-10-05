/**
 * The old demo file (phase 8). Before phase 8 the demo kept
 * its metadata (a SQLite file) as base64 in `localStorage["seaquel_db"]` (and builds
 * may have left `seaquel_db.*` keys). The new demo never reads them: it
 * removes them on every start, so a stale tab of an older build that writes
 * the key again loses it on the next start. Other keys (`seaquel-theme-cache`,
 * the locale, …) stay.
 */

/** Whether `key` is the old file or one of its siblings. */
export function isOldKey(key: string): boolean {
  return key === "seaquel_db" || key.startsWith("seaquel_db.");
}

/**
 * Removes the old keys from `storage` without reading them; returns how
 * many went. Never throws: blocked storage counts as nothing to remove.
 */
export function deleteOldKeys(storage: Storage | null | undefined = globalStorage()): number {
  if (!storage) return 0;
  try {
    const keys: string[] = [];
    for (let i = 0; i < storage.length; i++) {
      const key = storage.key(i);
      if (key !== null && isOldKey(key)) keys.push(key);
    }
    for (const key of keys) storage.removeItem(key);
    return keys.length;
  } catch {
    return 0;
  }
}

function globalStorage(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    // A sandboxed frame or blocked site data throws on access.
    return null;
  }
}
