/**
 * Tests for the air-gap local-control module.
 *
 * Shares the same DB-redirect trick as bundle-store.test.ts: a per-file
 * tempdir DATA_DIR + an env-injected dev trust anchor. The tests write
 * a verified bundle directly via `writeBundle`, then exercise each
 * local-control function against it.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { afterAll, beforeEach, describe, expect, it } from "vitest";

const tmp = mkdtempSync(join(tmpdir(), "seaquel-local-control-"));
process.env.DATA_DIR = tmp;

const SEED = new Uint8Array([
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
  27, 28, 29, 30, 31,
]);

function bytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

const PUBKEY = await ed.getPublicKeyAsync(SEED);
const { fingerprintPubkey, canonicalize } = await import("./canonical");
const FINGERPRINT = await fingerprintPubkey(PUBKEY);
process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = `${FINGERPRINT}:${bytesToHex(PUBKEY)}`;

const { openAuthDb } = await import("../auth");
const { writeBundle, clearBundle, _resetBundleStoreCache } = await import("./bundle-store");
const {
  registerInstallLocal,
  localTenantInfo,
  verifyLocalMembershipLicense,
  bindLocal,
  unbindLocal,
  listLocalMembers,
  _internal,
} = await import("./local-control");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

interface BundleOverrides {
  not_after?: number;
  revoked_keys?: string[];
  seat_tokens?: Array<{ key: string; role: "owner" | "member" }>;
}

async function importBundle(overrides: BundleOverrides = {}): Promise<void> {
  const issuedAt = 1_700_000_000;
  // Default `not_after` lands a year in the future relative to *now*
  // (not relative to `issued_at`) so `localTenantInfo()`'s expiry check
  // doesn't fire while tests run.
  const nowSec = Math.floor(Date.now() / 1000);
  const payload = {
    version: 1 as const,
    issued_at: issuedAt,
    not_before: issuedAt - 60,
    not_after: nowSec + 60 * 60 * 24 * 365,
    subscription_id: "sub_test_0001",
    tenant_slug: "acme",
    tier: "team",
    seats: 3,
    seat_tokens: overrides.seat_tokens ?? [
      { key: "owner_key_abc", role: "owner" as const },
      { key: "member_key_xyz", role: "member" as const },
    ],
    revoked_keys: overrides.revoked_keys ?? [],
    issued_by_install_id: null,
    ...(overrides.not_after !== undefined ? { not_after: overrides.not_after } : {}),
  };

  const canonical = canonicalize(payload as unknown as import("./canonical").CanonicalValue);
  const sig = await ed.signAsync(canonical, SEED);
  const b64Url = (bytes: Uint8Array): string => {
    let bin = "";
    for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const envelope = {
    payload: b64Url(canonical),
    sig: b64Url(sig),
    pubkey_fingerprint: FINGERPRINT,
  };
  const rawEnvelope = new TextEncoder().encode(JSON.stringify(envelope));
  const { createHash } = await import("node:crypto");
  const payloadSha256 = createHash("sha256").update(canonical).digest("hex");

  writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);
}

function seedUser(userId: string, email: string): void {
  // Better Auth's `user` table uses non-default column names + types. The
  // tests only need a row that satisfies the FK from member_license, so
  // a minimal insert is fine.
  openAuthDb()
    .prepare(
      `INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
       VALUES (?, ?, ?, 0, datetime('now'), datetime('now'))`,
    )
    .run(userId, email.split("@")[0], email);
}

function bindMember(userId: string, licenseKey: string, isOwner: boolean): void {
  openAuthDb()
    .prepare(
      `INSERT INTO member_license (user_id, license_key, bound_at, control_member_id, is_owner)
       VALUES (?, ?, ?, ?, ?)`,
    )
    .run(
      userId,
      licenseKey,
      Math.floor(Date.now() / 1000),
      _internal.syntheticTenantMemberId(licenseKey),
      isOwner ? 1 : 0,
    );
}

beforeEach(() => {
  // Reset all state — DB rows + bundle-store cache.
  const db = openAuthDb();
  db.prepare(`DELETE FROM member_license`).run();
  db.prepare(`DELETE FROM "user"`).run();
  db.prepare(`DELETE FROM airgap_bundle`).run();
  db.prepare(`DELETE FROM install_cache`).run();
  _resetBundleStoreCache();
});

describe("registerInstallLocal", () => {
  it("throws no_airgap_bundle when no bundle is present", async () => {
    await expect(registerInstallLocal("owner_key_abc")).rejects.toThrow("no_airgap_bundle");
  });

  it("throws license_not_found when the key isn't the owner seat", async () => {
    await importBundle();
    await expect(registerInstallLocal("member_key_xyz")).rejects.toThrow("license_not_found");
    await expect(registerInstallLocal("unknown")).rejects.toThrow("license_not_found");
  });

  it("projects the bundle into a TenantContext and writes the install_cache", async () => {
    await importBundle();
    const ctx = await registerInstallLocal("owner_key_abc");
    expect(ctx.tenantId).toBe("airgap-sub_test_0001");
    expect(ctx.slug).toBe("acme");
    expect(ctx.tier).toBe("team");
    expect(ctx.seatLimit).toBe(3);
    expect(ctx.status).toBe("active");
    expect(ctx.subscriptionId).toBe("sub_test_0001");

    // install_cache row written in airgap mode
    const cache = openAuthDb().prepare(`SELECT mode FROM install_cache WHERE id = 1`).get() as
      | { mode: string }
      | undefined;
    expect(cache?.mode).toBe("airgap");
  });
});

describe("localTenantInfo", () => {
  it("returns null when no bundle is present", async () => {
    expect(await localTenantInfo()).toBeNull();
  });

  it("returns null when the bundle is past not_after", async () => {
    const past = Math.floor(Date.now() / 1000) - 60;
    // not_after must be >= not_before — use a wide enough range to satisfy
    // verifyBundle's invariants while still landing in the past.
    await importBundle({ not_after: past });
    expect(await localTenantInfo()).toBeNull();
  });

  it("projects the bundle and does NOT write install_cache", async () => {
    await importBundle();
    const ctx = await localTenantInfo();
    expect(ctx).not.toBeNull();
    expect(ctx!.tenantId).toBe("airgap-sub_test_0001");
    const cache = openAuthDb().prepare(`SELECT id FROM install_cache WHERE id = 1`).get();
    expect(cache).toBeUndefined();
  });
});

describe("verifyLocalMembershipLicense", () => {
  it("returns license_not_found when no bundle is present", async () => {
    const r = await verifyLocalMembershipLicense("anything", "user@example.com");
    expect(r).toEqual({ ok: false, error: "license_not_found" });
  });

  it("returns license_not_found when the key is not in the bundle", async () => {
    await importBundle();
    const r = await verifyLocalMembershipLicense("missing", "user@example.com");
    expect(r).toEqual({ ok: false, error: "license_not_found" });
  });

  it("returns license_inactive when the key is in revoked_keys", async () => {
    await importBundle({ revoked_keys: ["member_key_xyz"] });
    const r = await verifyLocalMembershipLicense("member_key_xyz", "user@example.com");
    expect(r).toEqual({ ok: false, error: "license_inactive" });
  });

  it("rejects synthetic vacant placeholders as license_not_found", async () => {
    await importBundle({
      seat_tokens: [
        { key: "owner_key_abc", role: "owner" },
        { key: "airgap-vacant-acme-1", role: "member" },
      ],
    });
    const r = await verifyLocalMembershipLicense("airgap-vacant-acme-1", "user@example.com");
    expect(r).toEqual({ ok: false, error: "license_not_found" });
  });

  it("returns license_already_in_other_tenant when already locally bound", async () => {
    await importBundle();
    seedUser("u_existing", "existing@example.com");
    bindMember("u_existing", "member_key_xyz", false);

    const r = await verifyLocalMembershipLicense("member_key_xyz", "new@example.com");
    expect(r).toEqual({ ok: false, error: "license_already_in_other_tenant" });
  });

  it("returns ok with subscriptionId/tier/role for an unbound seat token", async () => {
    await importBundle();
    const r = await verifyLocalMembershipLicense("member_key_xyz", "new@example.com");
    expect(r).toEqual({
      ok: true,
      subscriptionId: "sub_test_0001",
      tier: "team",
      role: "member",
    });

    const owner = await verifyLocalMembershipLicense("owner_key_abc", "new@example.com");
    expect(owner).toEqual({
      ok: true,
      subscriptionId: "sub_test_0001",
      tier: "team",
      role: "owner",
    });
  });
});

describe("bindLocal / unbindLocal", () => {
  it("bindLocal returns a deterministic tenantMemberId derived from the license key", async () => {
    const a = await bindLocal({
      licenseKey: "owner_key_abc",
      containerUserId: "u_alpha",
      email: "a@example.com",
      role: "owner",
    });
    const b = await bindLocal({
      licenseKey: "owner_key_abc",
      containerUserId: "u_beta",
      email: "b@example.com",
      role: "member",
    });
    expect(a.tenantMemberId).toBe(b.tenantMemberId);
    expect(a.tenantMemberId).toMatch(/^local-[0-9a-f]{16}$/);

    const c = await bindLocal({
      licenseKey: "different_key",
      containerUserId: "u_gamma",
      email: "c@example.com",
      role: "member",
    });
    expect(c.tenantMemberId).not.toBe(a.tenantMemberId);
  });

  it("unbindLocal is a no-op", async () => {
    // Should resolve without throwing regardless of input.
    await expect(unbindLocal("any-user-id")).resolves.toBeUndefined();
  });
});

describe("listLocalMembers", () => {
  it("returns an empty array when no members are bound", async () => {
    expect(await listLocalMembers()).toEqual([]);
  });

  it("lists active members with masked license keys, omitting revoked rows", async () => {
    seedUser("u_owner", "owner@example.com");
    seedUser("u_member", "member@example.com");
    seedUser("u_revoked", "revoked@example.com");

    bindMember("u_owner", "owner_key_abc", true);
    bindMember("u_member", "member_key_xyz", false);
    bindMember("u_revoked", "old_key_zzzz", false);
    openAuthDb()
      .prepare(`UPDATE member_license SET revoked_at = ? WHERE user_id = ?`)
      .run(Math.floor(Date.now() / 1000), "u_revoked");

    const members = await listLocalMembers();
    expect(members).toHaveLength(2);

    const ownerView = members.find((m) => m.containerUserId === "u_owner")!;
    expect(ownerView.role).toBe("owner");
    expect(ownerView.email).toBe("owner@example.com");
    expect(ownerView.maskedLicenseKey).toBe("••••-_abc");

    const memberView = members.find((m) => m.containerUserId === "u_member")!;
    expect(memberView.role).toBe("member");
    expect(memberView.maskedLicenseKey).toBe("••••-_xyz");

    // Each member's id matches the deterministic synth from bindLocal.
    expect(ownerView.tenantMemberId).toMatch(/^local-[0-9a-f]{16}$/);
  });
});

describe("_internal helpers", () => {
  it("syntheticTenantMemberId is stable for a given license key", () => {
    expect(_internal.syntheticTenantMemberId("k1")).toBe(_internal.syntheticTenantMemberId("k1"));
    expect(_internal.syntheticTenantMemberId("k1")).not.toBe(
      _internal.syntheticTenantMemberId("k2"),
    );
  });

  it("maskKey returns last-4 with bullet prefix", () => {
    expect(_internal.maskKey("")).toBe("");
    expect(_internal.maskKey("abcd")).toBe("abcd"); // ≤ 4 chars: not masked
    expect(_internal.maskKey("abcdef")).toBe("••••-cdef");
  });
});

afterAll(() => {
  clearBundle();
});
