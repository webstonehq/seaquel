/**
 * Tests for the membership-gate predicate added to handleApiGate in
 * Task 7: a `member_license` row whose `revoked_at` column is non-null
 * must be treated as "not bound" → 403 response.
 *
 * `handleApiGate` is an internal const inside hooks.server.ts, so we test
 * the predicate it depends on (`findByUserId(...).revokedAt != null`)
 * against the real DB via the same auth.db redirect trick the air-gap
 * tests use. The predicate is one line — but it's load-bearing for the
 * revocation flow, so the explicit assertion gives Task 8 a green light
 * to depend on it.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterAll, beforeEach, describe, expect, it } from "vitest";

const tmp = mkdtempSync(join(tmpdir(), "seaquel-hooks-revoke-"));
process.env.DATA_DIR = tmp;

const { openAuthDb } = await import("$lib/server/auth");
const memberLicense = await import("$lib/server/member-license");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

function insertUser(userId: string): void {
  // Better Auth's `user` table is part of the auth.db schema bundle and
  // gets created by openAuthDb()'s migration on first access. The
  // member_license FK references it, so insert a minimal row first.
  const now = new Date().toISOString();
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, email, name, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, ?, ?)`,
    )
    .run(userId, `${userId}@example.test`, userId, now, now);
}

function stampRevoked(licenseKey: string): void {
  openAuthDb()
    .prepare(`UPDATE member_license SET revoked_at = ? WHERE license_key = ?`)
    .run(Math.floor(Date.now() / 1000), licenseKey);
}

beforeEach(() => {
  // Truncate between tests so each spec sees a clean ledger.
  openAuthDb().prepare(`DELETE FROM member_license`).run();
  openAuthDb().prepare(`DELETE FROM "user"`).run();
});

describe("handleApiGate revocation predicate", () => {
  it("active member_license row passes the predicate", () => {
    insertUser("u_active");
    memberLicense.insert({
      userId: "u_active",
      licenseKey: "lic_active",
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: "tm_active",
      isOwner: true,
    });

    const ml = memberLicense.findByUserId("u_active");
    expect(ml).not.toBeNull();
    // The exact predicate handleApiGate uses:
    expect(!ml || ml.revokedAt != null).toBe(false);
  });

  it("revoked member_license row trips the predicate (would return 403)", () => {
    insertUser("u_revoked");
    memberLicense.insert({
      userId: "u_revoked",
      licenseKey: "lic_revoked",
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: "tm_revoked",
      isOwner: false,
    });
    stampRevoked("lic_revoked");

    const ml = memberLicense.findByUserId("u_revoked");
    expect(ml).not.toBeNull();
    expect(ml!.revokedAt).not.toBeNull();
    expect(!ml || ml.revokedAt != null).toBe(true);
  });

  it("missing row trips the predicate (would return 403)", () => {
    const ml = memberLicense.findByUserId("u_missing");
    expect(ml).toBeNull();
    expect(!ml || ml.revokedAt != null).toBe(true);
  });

  it("markRevoked stamps revoked_at and the predicate flips on the next read", () => {
    insertUser("u_flip");
    memberLicense.insert({
      userId: "u_flip",
      licenseKey: "lic_flip",
      boundAt: Math.floor(Date.now() / 1000),
      controlMemberId: "tm_flip",
      isOwner: false,
    });

    // Pre-revocation: predicate is false.
    let ml = memberLicense.findByUserId("u_flip");
    expect(!ml || ml.revokedAt != null).toBe(false);

    const { rowsRevoked, userIds } = memberLicense.markRevoked(["lic_flip"]);
    expect(rowsRevoked).toBe(1);
    expect(userIds).toEqual(["u_flip"]);

    // Post-revocation: predicate flips.
    ml = memberLicense.findByUserId("u_flip");
    expect(!ml || ml.revokedAt != null).toBe(true);
  });
});
