/**
 * Tests for the bundle-presence dispatcher in `licensing.ts`.
 *
 * Two test groups:
 *   1. "dispatch — unit" — uses `vi.mock` to stub out
 *      `airgap/bundle-store` (so `isBundleDriven()` is a controllable
 *      boolean) and `airgap/local-control` (so we can observe which
 *      branch the dispatcher took). `fetch` is stubbed globally.
 *
 *   2. "dispatch — integration" — uses the *real* bundle-store against a
 *      per-file tempdir auth.db (same DB-redirect trick as
 *      bundle-store.test.ts). Proves the dispatcher reads
 *      `isBundleDriven()` on every call rather than caching the mode
 *      decision in memory.
 *
 * `isNetworkFailure` is asserted alongside the no-bundle / fetch-throws
 * scenarios so the helper's TypeError detection stays in lockstep with
 * how the dispatcher's caller (Task 7's signup handler) will use it.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tmp = mkdtempSync(join(tmpdir(), "seaquel-licensing-dispatch-"));
process.env.DATA_DIR = tmp;

// ---------------------------------------------------------------------------
// Group 1 — pure dispatcher unit tests with mocked airgap modules.
// ---------------------------------------------------------------------------

vi.mock("./airgap/bundle-store", () => ({
  isBundleDriven: vi.fn(() => false),
}));
vi.mock("./airgap/local-control", () => ({
  registerInstallLocal: vi.fn(),
  localTenantInfo: vi.fn(),
  verifyLocalMembershipLicense: vi.fn(),
  bindLocal: vi.fn(),
  unbindLocal: vi.fn(),
  listLocalMembers: vi.fn(),
}));
// install.ts touches the DB on first call; stub the install-id getter so
// the unit tests don't need to open auth.db at all.
vi.mock("./install", () => ({
  getOrCreateInstallId: vi.fn(() => "test-install-id"),
}));

const bundleStore = await import("./airgap/bundle-store");
const airgapLocal = await import("./airgap/local-control");
const {
  registerInstall,
  tenantInfo,
  verifyMembershipLicense,
  bindMember,
  unbindMember,
  listMembers,
  isNetworkFailure,
} = await import("./licensing");

function mockFetchJson(status: number, body: unknown): typeof fetch {
  return vi.fn(
    async () =>
      new Response(JSON.stringify(body), {
        status,
        headers: { "Content-Type": "application/json" },
      }),
  ) as unknown as typeof fetch;
}

function mockFetchThrows(err: unknown): typeof fetch {
  return vi.fn(async () => {
    throw err;
  }) as unknown as typeof fetch;
}

beforeEach(() => {
  vi.mocked(bundleStore.isBundleDriven).mockReturnValue(false);
  vi.mocked(airgapLocal.registerInstallLocal).mockReset();
  vi.mocked(airgapLocal.localTenantInfo).mockReset();
  vi.mocked(airgapLocal.verifyLocalMembershipLicense).mockReset();
  vi.mocked(airgapLocal.bindLocal).mockReset();
  vi.mocked(airgapLocal.unbindLocal).mockReset();
  vi.mocked(airgapLocal.listLocalMembers).mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const FAKE_TENANT = {
  tenantId: "tenant_abc",
  slug: "acme",
  status: "active" as const,
  publicUrl: "https://example.test",
  anchorLicenseId: "lic_anchor",
  subscriptionId: "sub_123",
  tier: "team",
  ownerEmail: "owner@example.test",
  seatLimit: 5,
  currentPeriodEnd: "2027-01-01T00:00:00.000Z",
};

const FAKE_BIND = { tenantMemberId: "tm_123" };
const FAKE_VERIFY = {
  ok: true as const,
  subscriptionId: "sub_123",
  tier: "team",
  role: "owner" as const,
};
const FAKE_MEMBERS = [
  {
    tenantMemberId: "tm_123",
    containerUserId: "u_1",
    email: "a@example.test",
    role: "owner" as const,
    boundAt: "2026-01-01T00:00:00.000Z",
    maskedLicenseKey: "••••-aaaa",
  },
];

describe("dispatch — bundle present routes to airgap.*Local", () => {
  beforeEach(() => {
    vi.mocked(bundleStore.isBundleDriven).mockReturnValue(true);
    // Make fetch explode if anyone tries to call it.
    vi.stubGlobal(
      "fetch",
      vi.fn(() => {
        throw new Error("fetch must not be called when bundle present");
      }),
    );
  });

  it("registerInstall → airgap.registerInstallLocal", async () => {
    vi.mocked(airgapLocal.registerInstallLocal).mockResolvedValue(FAKE_TENANT);
    const out = await registerInstall("owner_key_abc");
    expect(out).toEqual(FAKE_TENANT);
    expect(airgapLocal.registerInstallLocal).toHaveBeenCalledWith("owner_key_abc");
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });

  it("tenantInfo → airgap.localTenantInfo", async () => {
    vi.mocked(airgapLocal.localTenantInfo).mockResolvedValue(FAKE_TENANT);
    const out = await tenantInfo();
    expect(out).toEqual(FAKE_TENANT);
    expect(airgapLocal.localTenantInfo).toHaveBeenCalled();
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });

  it("verifyMembershipLicense → airgap.verifyLocalMembershipLicense", async () => {
    vi.mocked(airgapLocal.verifyLocalMembershipLicense).mockResolvedValue(FAKE_VERIFY);
    const out = await verifyMembershipLicense("k", "u@example.test");
    expect(out).toEqual(FAKE_VERIFY);
    expect(airgapLocal.verifyLocalMembershipLicense).toHaveBeenCalledWith("k", "u@example.test");
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });

  it("bindMember → airgap.bindLocal", async () => {
    vi.mocked(airgapLocal.bindLocal).mockResolvedValue(FAKE_BIND);
    const args = {
      licenseKey: "k",
      containerUserId: "u_1",
      email: "u@example.test",
      role: "member" as const,
    };
    const out = await bindMember(args);
    expect(out).toEqual(FAKE_BIND);
    expect(airgapLocal.bindLocal).toHaveBeenCalledWith(args);
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });

  it("unbindMember → airgap.unbindLocal", async () => {
    vi.mocked(airgapLocal.unbindLocal).mockResolvedValue(undefined);
    await expect(unbindMember("u_1")).resolves.toBeUndefined();
    expect(airgapLocal.unbindLocal).toHaveBeenCalledWith("u_1");
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });

  it("listMembers → airgap.listLocalMembers", async () => {
    vi.mocked(airgapLocal.listLocalMembers).mockResolvedValue(FAKE_MEMBERS);
    const out = await listMembers();
    expect(out).toEqual(FAKE_MEMBERS);
    expect(airgapLocal.listLocalMembers).toHaveBeenCalled();
    expect(globalThis.fetch).not.toHaveBeenCalled();
  });
});

describe("dispatch — bundle absent routes to control-plane fetch", () => {
  beforeEach(() => {
    vi.mocked(bundleStore.isBundleDriven).mockReturnValue(false);
  });

  it("registerInstall: fetch 200 → returns parsed body, airgap NOT called", async () => {
    vi.stubGlobal("fetch", mockFetchJson(200, FAKE_TENANT));
    const out = await registerInstall("k");
    expect(out).toEqual(FAKE_TENANT);
    expect(airgapLocal.registerInstallLocal).not.toHaveBeenCalled();
    expect(globalThis.fetch).toHaveBeenCalledTimes(1);
  });

  it("verifyMembershipLicense: fetch 200 → returns parsed body", async () => {
    vi.stubGlobal("fetch", mockFetchJson(200, FAKE_VERIFY));
    const out = await verifyMembershipLicense("k", "u@example.test");
    expect(out).toEqual(FAKE_VERIFY);
    expect(airgapLocal.verifyLocalMembershipLicense).not.toHaveBeenCalled();
  });

  it("fetch throws TypeError → error propagates, isNetworkFailure detects it", async () => {
    const err = new TypeError("fetch failed");
    vi.stubGlobal("fetch", mockFetchThrows(err));
    let caught: unknown = null;
    try {
      await registerInstall("k");
    } catch (e) {
      caught = e;
    }
    expect(caught).toBe(err);
    expect(isNetworkFailure(caught)).toBe(true);
    expect(airgapLocal.registerInstallLocal).not.toHaveBeenCalled();
  });

  it("fetch returns 400 → throws, isNetworkFailure returns false", async () => {
    vi.stubGlobal("fetch", mockFetchJson(400, { error: "bad_request" }));
    let caught: unknown = null;
    try {
      await registerInstall("k");
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(Error);
    expect((caught as Error).message).toMatch(/register-install failed: 400/);
    expect(isNetworkFailure(caught)).toBe(false);
  });
});

describe("isNetworkFailure", () => {
  it("returns true for TypeError (undici DNS/connect/TLS)", () => {
    expect(isNetworkFailure(new TypeError("fetch failed"))).toBe(true);
  });

  it("returns true for AbortError / TimeoutError by name", () => {
    const abort = Object.assign(new Error("aborted"), { name: "AbortError" });
    const timeout = Object.assign(new Error("timed out"), { name: "TimeoutError" });
    expect(isNetworkFailure(abort)).toBe(true);
    expect(isNetworkFailure(timeout)).toBe(true);
  });

  it("returns true for ECONNREFUSED / ENOTFOUND / ETIMEDOUT codes", () => {
    expect(isNetworkFailure(Object.assign(new Error(), { code: "ECONNREFUSED" }))).toBe(true);
    expect(isNetworkFailure(Object.assign(new Error(), { code: "ENOTFOUND" }))).toBe(true);
    expect(isNetworkFailure(Object.assign(new Error(), { code: "ETIMEDOUT" }))).toBe(true);
  });

  it("returns false for ordinary errors and HTTP-error wrappers", () => {
    expect(isNetworkFailure(null)).toBe(false);
    expect(isNetworkFailure(undefined)).toBe(false);
    expect(isNetworkFailure(new Error("register-install failed: 400 bad request"))).toBe(false);
    expect(isNetworkFailure(Object.assign(new Error(), { code: "EACCES" }))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Group 2 — integration test: real bundle-store flip mid-session.
//
// Uses a fresh dynamic import path (via `vi.resetModules`) so the
// `vi.mock` hoists above don't apply. This proves the dispatcher
// re-reads `isBundleDriven()` on every call rather than caching it.
// ---------------------------------------------------------------------------

describe("dispatch — no in-memory caching of the mode decision", () => {
  it("flipping bundle presence flips the dispatch route on the next call", async () => {
    vi.resetModules();
    vi.doUnmock("./airgap/bundle-store");
    vi.doUnmock("./airgap/local-control");
    vi.doUnmock("./install");

    // Set up an env-injected dev trust anchor BEFORE re-importing the
    // bundle-store so its first trust snapshot includes our seed.
    const SEED = new Uint8Array([
      0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
      26, 27, 28, 29, 30, 31,
    ]);
    const PUBKEY = await ed.getPublicKeyAsync(SEED);
    const { canonicalize, fingerprintPubkey } = await import("./airgap/canonical");
    const FINGERPRINT = await fingerprintPubkey(PUBKEY);
    function bytesToHex(b: Uint8Array): string {
      let out = "";
      for (const x of b) out += x.toString(16).padStart(2, "0");
      return out;
    }
    const savedTrust = process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
    process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = `${FINGERPRINT}:${bytesToHex(PUBKEY)}`;

    const { openAuthDb } = await import("./auth");
    const { writeBundle, clearBundle, _resetBundleStoreCache } =
      await import("./airgap/bundle-store");
    const real = await import("./licensing");

    // Hermetic state — no bundle, no install_cache, no member_license rows.
    openAuthDb().prepare(`DELETE FROM airgap_bundle`).run();
    openAuthDb().prepare(`DELETE FROM install_cache`).run();
    openAuthDb().prepare(`DELETE FROM member_license`).run();
    _resetBundleStoreCache();

    // First call: no bundle → goes to fetch path. We stub fetch to
    // return a valid TenantContext-shaped body.
    vi.stubGlobal("fetch", mockFetchJson(200, FAKE_TENANT));
    const online = await real.registerInstall("owner_key_abc");
    expect(online).toEqual(FAKE_TENANT);
    expect(globalThis.fetch).toHaveBeenCalledTimes(1);

    // Now write a real bundle, no module reload. `isBundleDriven()`
    // should return true on the next dispatcher call, even though the
    // module-level state in licensing.ts hasn't changed.
    const issuedAt = 1_700_000_000;
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
      seat_tokens: [
        { key: "owner_key_abc", role: "owner" as const },
        { key: "member_key_xyz", role: "member" as const },
      ],
      revoked_keys: [] as string[],
      issued_by_install_id: null,
    };
    const canonical = canonicalize(
      payload as unknown as import("./airgap/canonical").CanonicalValue,
    );
    const sig = await ed.signAsync(canonical, SEED);
    function b64Url(bytes: Uint8Array): string {
      let bin = "";
      for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
      return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    }
    const envelope = {
      payload: b64Url(canonical),
      sig: b64Url(sig),
      pubkey_fingerprint: FINGERPRINT,
    };
    const rawEnvelope = new TextEncoder().encode(JSON.stringify(envelope));
    const { createHash } = await import("node:crypto");
    const payloadSha256 = createHash("sha256").update(canonical).digest("hex");
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);

    // Reset fetch so any accidental call would explode.
    vi.stubGlobal(
      "fetch",
      vi.fn(() => {
        throw new Error("fetch must not be called after bundle import");
      }),
    );

    // Second call: bundle now present → goes to the airgap path. The
    // local-control owner-seat check passes because we used
    // "owner_key_abc" as the owner seat in the payload above.
    const airgap = await real.registerInstall("owner_key_abc");
    expect(airgap.tenantId).toBe("airgap-sub_test_0001");
    expect(airgap.slug).toBe("acme");

    // And `tenantInfo()` also routes locally — no fetch call.
    const ti = await real.tenantInfo();
    expect(ti).not.toBeNull();
    expect(ti!.tenantId).toBe("airgap-sub_test_0001");

    // Clean up DB / env for any other tests.
    clearBundle();
    openAuthDb().prepare(`DELETE FROM install_cache`).run();
    _resetBundleStoreCache();
    if (savedTrust === undefined) {
      delete process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
    } else {
      process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = savedTrust;
    }
  });
});
