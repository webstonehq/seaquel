/**
 * Tests for the air-gap bundle store + trusted-pubkey loader.
 *
 * Strategy: redirect `auth.db` into a per-test-file tempdir BEFORE
 * importing any module that calls `openAuthDb()`. The existing
 * migrations bundle bootstraps the schema via `import.meta.glob`, so we
 * get the real schema for free.
 */

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as ed from "@noble/ed25519";
import { afterAll, beforeEach, describe, expect, it } from "vitest";

const tmp = mkdtempSync(join(tmpdir(), "seaquel-bundle-store-"));
process.env.DATA_DIR = tmp;

// Set up a dev trust anchor BEFORE the bundle-store module is loaded so
// the first `loadTrustedPubkeys()` snapshot includes it.
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
const FINGERPRINT = await (async () => {
  const { fingerprintPubkey } = await import("./canonical");
  return fingerprintPubkey(PUBKEY);
})();
process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = `${FINGERPRINT}:${bytesToHex(PUBKEY)}`;

const { canonicalize } = await import("./canonical");
const { openAuthDb } = await import("../auth");
const {
  readActiveBundle,
  writeBundle,
  clearBundle,
  isBundleDriven,
  loadTrustedPubkeys,
  _resetBundleStoreCache,
} = await import("./bundle-store");

afterAll(() => {
  rmSync(tmp, { recursive: true, force: true });
});

async function buildEnvelope(overrides: Record<string, unknown> = {}): Promise<{
  rawEnvelope: Uint8Array;
  payload: import("./types").BundlePayload;
  payloadSha256: string;
}> {
  const issuedAt = 1_700_000_000;
  const payload: import("./types").BundlePayload = {
    version: 1,
    issued_at: issuedAt,
    not_before: issuedAt - 60,
    not_after: issuedAt + 60 * 60 * 24 * 365, // 1y
    subscription_id: "sub_test_0001",
    tenant_slug: "acme",
    tier: "team",
    seats: 3,
    seat_tokens: [
      { key: "owner_key_abc", role: "owner" },
      { key: "member_key_xyz", role: "member" },
    ],
    revoked_keys: [],
    issued_by_install_id: null,
    ...overrides,
  } as import("./types").BundlePayload;

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
  // Use a stable hash function — the real POST will compute this; the
  // store doesn't depend on a specific hash algorithm, only on the
  // string round-tripping.
  const { createHash } = await import("node:crypto");
  const payloadSha256 = createHash("sha256").update(canonical).digest("hex");
  return { rawEnvelope, payload, payloadSha256 };
}

beforeEach(() => {
  // Reset DB row + module cache between tests for hermetic state.
  openAuthDb().prepare(`DELETE FROM airgap_bundle`).run();
  openAuthDb().prepare(`DELETE FROM install_cache`).run();
  // Don't reset env / trust set — we want to keep the dev key active.
  _resetBundleStoreCache();
  // …but re-prime the trust set since _resetBundleStoreCache also cleared
  // the env-spec cache. Re-reading is fine, it's how the loader works.
});

describe("bundle-store", () => {
  it("writeBundle then readActiveBundle returns the same payload", async () => {
    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);

    const active = await readActiveBundle();
    expect(active).not.toBeNull();
    expect(active!.payload).toEqual(payload);
    expect(active!.pubkeyFingerprint).toBe(FINGERPRINT);
    expect(active!.payloadSha256).toBe(payloadSha256);
    expect(active!.rawEnvelope).toEqual(rawEnvelope);
  });

  it("clearBundle makes readActiveBundle return null", async () => {
    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);
    expect(await readActiveBundle()).not.toBeNull();

    clearBundle();
    expect(await readActiveBundle()).toBeNull();
  });

  it("re-verifies on each read: corrupting raw_envelope returns null", async () => {
    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);
    expect(await readActiveBundle()).not.toBeNull();

    // Drop the in-process cache so the next read actually hits the DB.
    _resetBundleStoreCache();

    // Replace the raw_envelope with bytes that are still JSON but not a
    // valid envelope — verifyBundle should reject as malformed_envelope.
    const garbage = Buffer.from('{"payload":"AA","sig":"AA","pubkey_fingerprint":"deadbeef"}');
    openAuthDb().prepare(`UPDATE airgap_bundle SET raw_envelope = ? WHERE id = 1`).run(garbage);

    const active = await readActiveBundle();
    expect(active).toBeNull();
  });

  it("returns null when the trust anchor is no longer in the trust set", async () => {
    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);
    expect(await readActiveBundle()).not.toBeNull();

    // Drop the env spec so the trust set goes back to the (empty) prod set.
    _resetBundleStoreCache();
    const saved = process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
    delete process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
    try {
      const active = await readActiveBundle();
      expect(active).toBeNull();
    } finally {
      process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = saved;
      _resetBundleStoreCache();
    }
  });

  it("isBundleDriven mirrors readActiveBundle() != null (in the steady state)", async () => {
    expect(isBundleDriven()).toBe(false);
    expect(await readActiveBundle()).toBeNull();

    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);

    expect(isBundleDriven()).toBe(true);
    expect(await readActiveBundle()).not.toBeNull();

    clearBundle();
    expect(isBundleDriven()).toBe(false);
    expect(await readActiveBundle()).toBeNull();
  });

  it("caches the parsed bundle within a single payload_sha256", async () => {
    const { rawEnvelope, payload, payloadSha256 } = await buildEnvelope();
    writeBundle({ payload, rawEnvelope, pubkeyFingerprint: FINGERPRINT }, payloadSha256);

    const a = await readActiveBundle();
    const b = await readActiveBundle();
    expect(a).toBe(b); // same reference — served from cache
  });
});

describe("loadTrustedPubkeys", () => {
  beforeEach(() => {
    _resetBundleStoreCache();
  });

  // PROD_TRUSTED_PUBKEYS is expected to change over time — it gains the
  // production anchor, and carries two entries during a key rotation. So
  // these tests assert *deltas* and presence rather than absolute map
  // sizes; counting the whole map breaks on every rotation.

  /**
   * Run `fn` with the env override set to `spec` (or unset when undefined),
   * resetting the module cache on the way in and out.
   */
  function withEnv<T>(spec: string | undefined, fn: () => T): T {
    const saved = process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
    const restore = (value: string | undefined) => {
      // Assigning `undefined` would write the literal string "undefined",
      // leaking a bogus spec into later tests — delete instead.
      if (value === undefined) {
        delete process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
      } else {
        process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY = value;
      }
    };
    restore(spec);
    _resetBundleStoreCache();
    try {
      return fn();
    } finally {
      restore(saved);
      _resetBundleStoreCache();
    }
  }

  /** The built-in production set, with no env override applied. */
  function prodTrust(): Map<string, Uint8Array> {
    return withEnv(undefined, () => new Map(loadTrustedPubkeys()));
  }

  it("parses a single SEAQUEL_BUNDLE_TRUSTED_PUBKEY entry", () => {
    const base = prodTrust().size;
    withEnv(`aa11:${"00".repeat(32)}`, () => {
      const trust = loadTrustedPubkeys();
      expect(trust.size).toBe(base + 1);
      const key = trust.get("aa11");
      expect(key).toBeDefined();
      expect(key!.length).toBe(32);
      expect(Array.from(key!)).toEqual(Array.from({ length: 32 }, () => 0));
    });
  });

  it("parses multiple comma-separated entries", () => {
    const base = prodTrust().size;
    const a = `aa11:${"01".repeat(32)}`;
    const b = `bb22:${"02".repeat(32)}`;
    withEnv(`${a},${b}`, () => {
      const trust = loadTrustedPubkeys();
      expect(trust.size).toBe(base + 2);
      expect(trust.get("aa11")).toBeDefined();
      expect(trust.get("bb22")).toBeDefined();
      expect(Array.from(trust.get("aa11")!)).toEqual(Array.from({ length: 32 }, () => 1));
      expect(Array.from(trust.get("bb22")!)).toEqual(Array.from({ length: 32 }, () => 2));
    });
  });

  it("ignores malformed entries but keeps valid ones", () => {
    const good = `aa11:${"03".repeat(32)}`;
    withEnv(`no-colon-here, ${good}, :only-value, only-key:, bb22:xyz`, () => {
      const trust = loadTrustedPubkeys();
      expect(trust.get("aa11")).toBeDefined();
      expect(trust.has("only-value")).toBe(false);
      expect(trust.has("only-key")).toBe(false);
      expect(trust.has("bb22")).toBe(false);
    });
  });

  it("merges the env override on top of the built-in production set", () => {
    // Supersedes an older test that asserted the map was empty when the env
    // var was unset — that only held while PROD_TRUSTED_PUBKEYS was empty.
    const prod = prodTrust();
    withEnv(`aa11:${"00".repeat(32)}`, () => {
      const trust = loadTrustedPubkeys();
      for (const [fingerprint, pubkey] of prod) {
        expect(trust.has(fingerprint)).toBe(true);
        expect(Array.from(trust.get(fingerprint)!)).toEqual(Array.from(pubkey));
      }
      expect(trust.has("aa11")).toBe(true);
    });
    // With no override, the env-only anchor is absent.
    withEnv(undefined, () => {
      expect(loadTrustedPubkeys().has("aa11")).toBe(false);
    });
  });

  it("ships a well-formed built-in production set", async () => {
    for (const [fingerprint, pubkey] of prodTrust()) {
      expect(pubkey.length).toBe(32);
      expect(fingerprint).toMatch(/^[0-9a-f]{32}$/);
      // A fingerprint that isn't SHA-256(pubkey)[0..16] can never be looked
      // up: the verifier keys off the fingerprint carried in the envelope,
      // so a mistyped anchor would silently reject every real bundle.
      const digest = new Uint8Array(
        await crypto.subtle.digest("SHA-256", pubkey as Uint8Array<ArrayBuffer>),
      );
      expect(fingerprint).toBe(bytesToHex(digest.slice(0, 16)));
    }
  });
});
