/**
 * Local mirror of "this user holds this license in this tenant".
 *
 * Authoritative copy lives upstream in seaquel-app's `tenant_members`
 * table; the local row backs the `(app)/+layout.server.ts` membership
 * gate so we don't need a network round-trip per page load.
 *
 * Shape matches `migrations/007_member_license.sql`. Lives in the same
 * `auth.db` as Better Auth's user/session tables — one row per user.
 */

import { openAuthDb } from "./auth";

export interface MemberLicense {
  userId: string;
  licenseKey: string;
  boundAt: number; // unix seconds
  controlMemberId: string;
  isOwner: boolean;
  revokedAt: number | null; // unix seconds; set by bundle import in air-gap mode
}

export function findByUserId(userId: string): MemberLicense | null {
  const row = openAuthDb()
    .prepare(
      `SELECT user_id AS userId, license_key AS licenseKey,
              bound_at AS boundAt, control_member_id AS controlMemberId,
              is_owner AS isOwner, revoked_at AS revokedAt
         FROM member_license WHERE user_id = ?`,
    )
    .get(userId) as (Omit<MemberLicense, "isOwner"> & { isOwner: number }) | undefined;
  return row ? { ...row, isOwner: row.isOwner === 1 } : null;
}

export function insert(row: Omit<MemberLicense, "revokedAt">): void {
  // `revoked_at` is intentionally omitted from the INSERT — it stays NULL
  // until a bundle-import revocation walk explicitly stamps it. Accepting
  // the row without `revokedAt` lets existing callers (signup) keep working
  // unchanged while still surfacing the field on reads.
  openAuthDb()
    .prepare(
      `INSERT INTO member_license
         (user_id, license_key, bound_at, control_member_id, is_owner)
       VALUES (@userId, @licenseKey, @boundAt, @controlMemberId, @isOwner)`,
    )
    .run({ ...row, isOwner: row.isOwner ? 1 : 0 });
}

export function deleteByUserId(userId: string): void {
  openAuthDb().prepare(`DELETE FROM member_license WHERE user_id = ?`).run(userId);
}

/**
 * Stamp `revoked_at = now()` on every `member_license` row whose
 * `license_key` appears in {@link licenseKeys} and isn't already revoked.
 * Returns the count of rows actually flipped plus the set of `user_id`
 * values affected (so callers can invalidate sessions, log, etc.).
 *
 * Used by `/api/airgap/bundle` POST after a bundle is verified — every
 * key in the bundle's `revocations` list flows through here.
 */
export function markRevoked(licenseKeys: string[]): { rowsRevoked: number; userIds: string[] } {
  if (licenseKeys.length === 0) {
    return { rowsRevoked: 0, userIds: [] };
  }

  const now = Math.floor(Date.now() / 1000);
  const placeholders = licenseKeys.map(() => "?").join(", ");
  const rows = openAuthDb()
    .prepare(
      `UPDATE member_license
          SET revoked_at = ?
        WHERE license_key IN (${placeholders})
          AND revoked_at IS NULL
        RETURNING user_id AS userId`,
    )
    .all(now, ...licenseKeys) as Array<{ userId: string }>;

  return { rowsRevoked: rows.length, userIds: rows.map((r) => r.userId) };
}
