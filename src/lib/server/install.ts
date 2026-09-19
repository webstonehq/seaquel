/**
 * Local install identity. Generated once at first boot, persisted in
 * auth.db, never read from env. Both Cloud and self-hosted use this
 * same value.
 */
import { randomUUID } from "node:crypto";
import { openAuthDb } from "./auth";

let cached: string | null = null;

export function getOrCreateInstallId(): string {
  if (cached) return cached;
  const db = openAuthDb();
  const existing = db.prepare(`SELECT install_id FROM install WHERE id = 1`).get() as
    | { install_id: string }
    | undefined;
  if (existing) {
    cached = existing.install_id;
    return cached;
  }

  const installId = randomUUID();

  db.prepare(`INSERT INTO install (id, install_id, created_at) VALUES (1, ?, ?)`).run(
    installId,
    Math.floor(Date.now() / 1000),
  );
  cached = installId;
  return installId;
}

/** Test-only — clears the in-process cache. */
export function _resetInstallCache(): void {
  cached = null;
}
