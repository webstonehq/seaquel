/**
 * Verifier tests — must match the matrix in
 *   seaquel-app/main/packages/marketing/src/lib/server/airgap/bundle-signer.test.ts
 * The golden-vector block at the top is the cross-runtime regression net:
 * the same payload, signed with the same seed, must produce the same bytes on
 * both sides. If you change anything here, mirror it there.
 */
import * as ed from "@noble/ed25519";
import { describe, expect, it } from "vitest";

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { canonicalize, type CanonicalValue } from "./canonical";
import type { BundlePayload } from "./types";
import { fingerprintPubkey, isExpired, verifyBundle, type BundleVerifyError } from "./verify";

// ---------------------------------------------------------------------------
// Golden vectors — keep byte-for-byte identical with the seaquel-app copy.
// ---------------------------------------------------------------------------

const GOLDEN_PAYLOAD: BundlePayload = {
  version: 1,
  issued_at: 1700000000,
  not_before: 1699999940,
  not_after: 1702592000,
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
};

const GOLDEN_CANONICAL_JSON =
  '{"issued_at":1700000000,"issued_by_install_id":null,"not_after":1702592000,"not_before":1699999940,"revoked_keys":[],"seat_tokens":[{"key":"owner_key_abc","role":"owner"},{"key":"member_key_xyz","role":"member"}],"seats":3,"subscription_id":"sub_test_0001","tenant_slug":"acme","tier":"team","version":1}';

const GOLDEN_SEED_HEX = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const GOLDEN_PUBKEY_HEX = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";
const GOLDEN_PUBKEY_FINGERPRINT = "56475aa75463474c0285df5dbf2bcab7";
const GOLDEN_PAYLOAD_B64URL =
  "eyJpc3N1ZWRfYXQiOjE3MDAwMDAwMDAsImlzc3VlZF9ieV9pbnN0YWxsX2lkIjpudWxsLCJub3RfYWZ0ZXIiOjE3MDI1OTIwMDAsIm5vdF9iZWZvcmUiOjE2OTk5OTk5NDAsInJldm9rZWRfa2V5cyI6W10sInNlYXRfdG9rZW5zIjpbeyJrZXkiOiJvd25lcl9rZXlfYWJjIiwicm9sZSI6Im93bmVyIn0seyJrZXkiOiJtZW1iZXJfa2V5X3h5eiIsInJvbGUiOiJtZW1iZXIifV0sInNlYXRzIjozLCJzdWJzY3JpcHRpb25faWQiOiJzdWJfdGVzdF8wMDAxIiwidGVuYW50X3NsdWciOiJhY21lIiwidGllciI6InRlYW0iLCJ2ZXJzaW9uIjoxfQ";
const GOLDEN_SIG_B64URL =
  "hmH2dfTnY_77RrUnFm8fKb92jG3ibu4iV3JZvn7UNoRZpkFET2M2Xh2vR7UVAPQZLR2vAgrSXYBw60RBlTdQAg";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function bytesToB64Url(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64UrlToBytes(input: string): Uint8Array {
  const padded = input + "===".slice((input.length + 3) % 4);
  const std = padded.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(std);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Sign a payload using ONLY noble primitives + the local canonicaliser. We do
 *  this here instead of importing the seaquel-app signer to keep this test
 *  hermetic (the repos do not share code). */
async function signLocally(
  payload: BundlePayload,
  seed: Uint8Array,
): Promise<{ envelopeBytes: Uint8Array; pubkey: Uint8Array; fingerprint: string }> {
  const canonical = canonicalize(payload as unknown as CanonicalValue);
  const sig = await ed.signAsync(canonical, seed);
  const pubkey = await ed.getPublicKeyAsync(seed);
  const fingerprint = await fingerprintPubkey(pubkey);
  const envelope = {
    payload: bytesToB64Url(canonical),
    sig: bytesToB64Url(sig),
    pubkey_fingerprint: fingerprint,
  };
  const envelopeBytes = new TextEncoder().encode(JSON.stringify(envelope));
  return { envelopeBytes, pubkey, fingerprint };
}

function trustSet(fingerprint: string, pubkey: Uint8Array): Map<string, Uint8Array> {
  return new Map<string, Uint8Array>([[fingerprint, pubkey]]);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("canonicalize", () => {
  it("matches the golden canonical JSON byte-for-byte", () => {
    const bytes = canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue);
    const text = new TextDecoder().decode(bytes);
    expect(text).toBe(GOLDEN_CANONICAL_JSON);
  });

  it("is stable across key ordering of the input", () => {
    const reordered = {
      tier: "team",
      issued_at: 1700000000,
      seats: 3,
      version: 1,
      tenant_slug: "acme",
      issued_by_install_id: null,
      seat_tokens: [
        { role: "owner", key: "owner_key_abc" },
        { role: "member", key: "member_key_xyz" },
      ],
      not_before: 1699999940,
      not_after: 1702592000,
      revoked_keys: [] as string[],
      subscription_id: "sub_test_0001",
    } as unknown as CanonicalValue;
    const a = canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue);
    const b = canonicalize(reordered);
    expect(b).toEqual(a);
  });

  it("emits empty arrays explicitly", () => {
    const bytes = canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue);
    const text = new TextDecoder().decode(bytes);
    expect(text).toContain('"revoked_keys":[]');
  });

  it("emits null values explicitly", () => {
    const bytes = canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue);
    const text = new TextDecoder().decode(bytes);
    expect(text).toContain('"issued_by_install_id":null');
  });
});

describe("golden vector", () => {
  it("matches the published pubkey hex", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const pub = await ed.getPublicKeyAsync(seed);
    let hex = "";
    for (const b of pub) hex += b.toString(16).padStart(2, "0");
    expect(hex).toBe(GOLDEN_PUBKEY_HEX);
  });

  it("matches the published payload + signature when re-signed", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const canonical = canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue);
    const sig = await ed.signAsync(canonical, seed);
    expect(bytesToB64Url(canonical)).toBe(GOLDEN_PAYLOAD_B64URL);
    expect(bytesToB64Url(sig)).toBe(GOLDEN_SIG_B64URL);
  });

  it("matches the published pubkey fingerprint", async () => {
    const pub = hexToBytes(GOLDEN_PUBKEY_HEX);
    const fp = await fingerprintPubkey(pub);
    expect(fp).toBe(GOLDEN_PUBKEY_FINGERPRINT);
  });

  it("verifies an envelope built from the golden vector", async () => {
    const envelope = JSON.stringify({
      payload: GOLDEN_PAYLOAD_B64URL,
      sig: GOLDEN_SIG_B64URL,
      pubkey_fingerprint: GOLDEN_PUBKEY_FINGERPRINT,
    });
    const envelopeBytes = new TextEncoder().encode(envelope);
    const pubkey = hexToBytes(GOLDEN_PUBKEY_HEX);
    const verified = await verifyBundle(envelopeBytes, trustSet(GOLDEN_PUBKEY_FINGERPRINT, pubkey));
    expect(verified.payload).toEqual(GOLDEN_PAYLOAD);
    expect(verified.pubkeyFingerprint).toBe(GOLDEN_PUBKEY_FINGERPRINT);
  });
});

describe("verifyBundle", () => {
  it("roundtrips a freshly signed bundle", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const { envelopeBytes, pubkey, fingerprint } = await signLocally(GOLDEN_PAYLOAD, seed);
    const verified = await verifyBundle(envelopeBytes, trustSet(fingerprint, pubkey));
    expect(verified.payload).toEqual(GOLDEN_PAYLOAD);
  });

  it("rejects a tampered payload byte as bad_signature", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const { envelopeBytes, pubkey, fingerprint } = await signLocally(GOLDEN_PAYLOAD, seed);
    const text = new TextDecoder().decode(envelopeBytes);
    const parsed = JSON.parse(text) as { payload: string; sig: string; pubkey_fingerprint: string };
    const decoded = b64UrlToBytes(parsed.payload);
    // Flip one byte of the canonical payload — Ed25519 catches this regardless
    // of where the flip lands.
    decoded[10] = decoded[10] ^ 0x01;
    const tampered = JSON.stringify({ ...parsed, payload: bytesToB64Url(decoded) });
    await expect(
      verifyBundle(new TextEncoder().encode(tampered), trustSet(fingerprint, pubkey)),
    ).rejects.toThrow("bad_signature");
  });

  it("rejects an untrusted signer", async () => {
    const seedA = hexToBytes(GOLDEN_SEED_HEX);
    const { envelopeBytes } = await signLocally(GOLDEN_PAYLOAD, seedA);
    const seedB = hexToBytes("1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100");
    const pubB = await ed.getPublicKeyAsync(seedB);
    const fpB = await fingerprintPubkey(pubB);
    await expect(verifyBundle(envelopeBytes, trustSet(fpB, pubB))).rejects.toThrow(
      "untrusted_signer",
    );
  });

  it("rejects an envelope missing the sig field", async () => {
    const envelope = JSON.stringify({
      payload: GOLDEN_PAYLOAD_B64URL,
      pubkey_fingerprint: GOLDEN_PUBKEY_FINGERPRINT,
    });
    const pubkey = hexToBytes(GOLDEN_PUBKEY_HEX);
    await expect(
      verifyBundle(new TextEncoder().encode(envelope), trustSet(GOLDEN_PUBKEY_FINGERPRINT, pubkey)),
    ).rejects.toThrow("malformed_envelope");
  });

  it("rejects an envelope that isn't JSON", async () => {
    const pubkey = hexToBytes(GOLDEN_PUBKEY_HEX);
    await expect(
      verifyBundle(
        new TextEncoder().encode("not json"),
        trustSet(GOLDEN_PUBKEY_FINGERPRINT, pubkey),
      ),
    ).rejects.toThrow("malformed_envelope");
  });

  it("rejects schema mismatch (version=2)", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const badPayload = { ...GOLDEN_PAYLOAD, version: 2 } as unknown as BundlePayload;
    const canonical = canonicalize(badPayload as unknown as CanonicalValue);
    const sig = await ed.signAsync(canonical, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(canonical),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    await expect(
      verifyBundle(new TextEncoder().encode(envelope), trustSet(fp, pubkey)),
    ).rejects.toThrow("schema_mismatch");
  });

  it("rejects schema mismatch (extra unknown field)", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const extra = { ...GOLDEN_PAYLOAD, surprise: "hi" } as unknown as BundlePayload;
    const canonical = canonicalize(extra as unknown as CanonicalValue);
    const sig = await ed.signAsync(canonical, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(canonical),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    await expect(
      verifyBundle(new TextEncoder().encode(envelope), trustSet(fp, pubkey)),
    ).rejects.toThrow("schema_mismatch");
  });

  it("treats clock-rollback (not_before far in future) as bad_signature", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const future = Math.floor(Date.now() / 1000) + 60 * 60 * 24 * 365; // +1 year
    const rolled: BundlePayload = {
      ...GOLDEN_PAYLOAD,
      issued_at: future,
      not_before: future,
      not_after: future + 60,
    };
    const { envelopeBytes, pubkey, fingerprint } = await signLocally(rolled, seed);
    await expect(verifyBundle(envelopeBytes, trustSet(fingerprint, pubkey))).rejects.toThrow(
      "bad_signature",
    );
  });

  it("does not throw on expired bundles (caller handles expiry)", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const past: BundlePayload = {
      ...GOLDEN_PAYLOAD,
      issued_at: 1600000000,
      not_before: 1599999940,
      not_after: 1600000060, // already expired
    };
    const { envelopeBytes, pubkey, fingerprint } = await signLocally(past, seed);
    const verified = await verifyBundle(envelopeBytes, trustSet(fingerprint, pubkey));
    expect(verified.payload.not_after).toBe(1600000060);
    expect(isExpired(verified)).toBe(true);
  });

  it("rejects non-canonical envelopes (re-canonicalisation mismatch)", async () => {
    // Build a payload-bytes that is NOT canonical (keys out of order), sign it,
    // wrap in an envelope. The raw Ed25519 check would pass, but our
    // re-canonicalisation step must reject it as bad_signature.
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const nonCanonical =
      '{"version":1,"issued_at":1700000000,"not_before":1699999940,"not_after":1702592000,"subscription_id":"sub_test_0001","tenant_slug":"acme","tier":"team","seats":3,"seat_tokens":[{"key":"owner_key_abc","role":"owner"},{"key":"member_key_xyz","role":"member"}],"revoked_keys":[],"issued_by_install_id":null}';
    const payloadBytes = new TextEncoder().encode(nonCanonical);
    const sig = await ed.signAsync(payloadBytes, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(payloadBytes),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    await expect(
      verifyBundle(new TextEncoder().encode(envelope), trustSet(fp, pubkey)),
    ).rejects.toThrow("bad_signature");
  });

  it('rejects payload bytes encoding `"seats":3.0` as bad_signature via re-canonicalisation', async () => {
    // Construct payload bytes that encode `"seats":3.0` while the canonical
    // form is `"seats":3`. Sign the raw bytes directly so the raw Ed25519
    // check passes; the re-canonicalise-and-compare step must reject as
    // bad_signature.
    //
    // We splice the drifted literal into the otherwise-canonical text so the
    // mismatch is unambiguously the `3.0` vs `3` drift. JSON.parse accepts
    // `3.0` and yields the JS number 3, which `Number.isInteger` reports as
    // an integer — so parsePayload accepts it and we reach re-canon, which
    // re-emits `"seats":3` and detects the byte-level mismatch.
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const canonicalText = new TextDecoder().decode(
      canonicalize(GOLDEN_PAYLOAD as unknown as CanonicalValue),
    );
    const driftedText = canonicalText.replace('"seats":3,', '"seats":3.0,');
    expect(driftedText).not.toBe(canonicalText);
    const payloadBytes = new TextEncoder().encode(driftedText);
    const sig = await ed.signAsync(payloadBytes, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(payloadBytes),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    await expect(
      verifyBundle(new TextEncoder().encode(envelope), trustSet(fp, pubkey)),
    ).rejects.toThrow("bad_signature");
  });
});

describe("BundleVerifyError union", () => {
  it('does not contain "expired"', () => {
    // Compile-time check via the `Exclude<...> never` trick. If
    // `BundleVerifyError` ever re-introduces `"expired"`, this expression
    // resolves to a non-`never` type and the assertion fails to typecheck.
    type Forbidden = Exclude<BundleVerifyError, Exclude<BundleVerifyError, "expired">>;
    const _proof: Forbidden extends never ? true : false = true;
    expect(_proof).toBe(true);
  });

  it('verify.ts source does not throw the literal "expired"', () => {
    // Belt-and-braces runtime check: scan the verifier source for any
    // `throw new Error("expired")` (or `'expired'`) we might have missed.
    const src = readFileSync(resolve(__dirname, "./verify.ts"), "utf-8");
    expect(src).not.toMatch(/throw\s+new\s+Error\s*\(\s*["']expired["']/);
  });
});

describe("parsePayload numeric bounds", () => {
  async function signWithBoundsViolation(overrides: Partial<BundlePayload>): Promise<Uint8Array> {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const bad: BundlePayload = { ...GOLDEN_PAYLOAD, ...overrides };
    const canonical = canonicalize(bad as unknown as CanonicalValue);
    const sig = await ed.signAsync(canonical, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(canonical),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    return new TextEncoder().encode(envelope);
  }

  async function trustSetForGoldenSeed(): Promise<Map<string, Uint8Array>> {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    return trustSet(fp, pubkey);
  }

  it("rejects negative seats", async () => {
    const env = await signWithBoundsViolation({ seats: -1 });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("rejects seats above the 10000 ceiling", async () => {
    const env = await signWithBoundsViolation({ seats: 10001 });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("accepts the seats ceiling exactly (10000)", async () => {
    const env = await signWithBoundsViolation({ seats: 10000 });
    const verified = await verifyBundle(env, await trustSetForGoldenSeed());
    expect(verified.payload.seats).toBe(10000);
  });

  it("rejects negative issued_at", async () => {
    const env = await signWithBoundsViolation({
      issued_at: -1,
      not_before: -61,
      not_after: 1702592000,
    });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("rejects negative not_before", async () => {
    const env = await signWithBoundsViolation({
      issued_at: 1700000000,
      not_before: -1,
      not_after: 1702592000,
    });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("rejects negative not_after", async () => {
    const env = await signWithBoundsViolation({
      issued_at: 1700000000,
      not_before: 1699999940,
      not_after: -1,
    });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("rejects not_after < not_before", async () => {
    const env = await signWithBoundsViolation({
      issued_at: 1700000000,
      not_before: 1700000000,
      not_after: 1699999999,
    });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });

  it("rejects issued_at > not_after", async () => {
    const env = await signWithBoundsViolation({
      issued_at: 1800000000,
      not_before: 1699999940,
      not_after: 1702592000,
    });
    await expect(verifyBundle(env, await trustSetForGoldenSeed())).rejects.toThrow(
      "schema_mismatch",
    );
  });
});

describe("seat_tokens edge cases", () => {
  it("round-trips an empty seat_tokens array with seats: 0", async () => {
    const seed = hexToBytes(GOLDEN_SEED_HEX);
    const empty: BundlePayload = {
      ...GOLDEN_PAYLOAD,
      seats: 0,
      seat_tokens: [],
    };
    const canonical = canonicalize(empty as unknown as CanonicalValue);
    const text = new TextDecoder().decode(canonical);
    expect(text).toContain('"seat_tokens":[]');
    expect(text).not.toContain('"seat_tokens":null');

    const sig = await ed.signAsync(canonical, seed);
    const pubkey = await ed.getPublicKeyAsync(seed);
    const fp = await fingerprintPubkey(pubkey);
    const envelope = JSON.stringify({
      payload: bytesToB64Url(canonical),
      sig: bytesToB64Url(sig),
      pubkey_fingerprint: fp,
    });
    const verified = await verifyBundle(new TextEncoder().encode(envelope), trustSet(fp, pubkey));
    expect(verified.payload.seats).toBe(0);
    expect(verified.payload.seat_tokens).toEqual([]);
  });
});
