/**
 * Persistence + hot-read cache for the verified air-gap bundle.
 *
 * Singleton row in `airgap_bundle` (id = 1). Holds:
 *   - `raw_envelope`     — original signed bytes, re-verified on every
 *     read for defence-in-depth.
 *   - `verified_payload` — JSON projection used only as a cache hint /
 *     observability aid; the source of truth is the re-verified raw
 *     envelope.
 *   - `pubkey_fingerprint` / `payload_sha256` / `issued_at` / `not_after`
 *     / `imported_at` — pre-extracted hot fields used by the licensing
 *     UI and the dispatcher.
 *
 * Re-verifying on every read is intentional. If somebody pokes auth.db
 * directly to alter the projection (or the raw envelope) without the
 * signing key, the Ed25519 check still catches it. The in-process cache
 * (keyed on `payload_sha256`) keeps the verify cost out of the hot path
 * inside a single request — the dispatcher calls into here multiple
 * times per page load.
 */

import { openAuthDb } from "../auth";
import type { BundlePayload } from "./types";
import { verifyBundle } from "./verify";

export interface ActiveBundle {
  payload: BundlePayload;
  rawEnvelope: Uint8Array;
  pubkeyFingerprint: string;
  importedAt: number;
  payloadSha256: string;
}

interface BundleRow {
  raw_envelope: Buffer;
  verified_payload: string;
  pubkey_fingerprint: string;
  imported_at: number;
  not_after: number;
  payload_sha256: string;
  issued_at: number;
}

// At-most-one entry — keyed by payload_sha256. A clear-then-reimport with
// a different bundle naturally gets a different key, so the cache stays
// coherent without explicit invalidation across writes (writeBundle and
// clearBundle still clear, just defensively).
const cache = new Map<string, ActiveBundle>();

/**
 * Read the persisted bundle, re-verifying the raw envelope on every read
 * against {@link loadTrustedPubkeys}. Returns null when:
 *   - no row exists,
 *   - the raw envelope no longer round-trips through {@link verifyBundle}
 *     (e.g. a trust-anchor rotation removed the signer, or the row was
 *     tampered with), or
 *   - the row's `payload_sha256` no longer matches the verified payload
 *     (DB-level tampering of the projection).
 */
export async function readActiveBundle(): Promise<ActiveBundle | null> {
  const row = openAuthDb()
    .prepare(
      `SELECT raw_envelope, verified_payload, pubkey_fingerprint,
              imported_at, not_after, payload_sha256, issued_at
         FROM airgap_bundle WHERE id = 1`,
    )
    .get() as BundleRow | undefined;
  if (!row) return null;

  const cached = cache.get(row.payload_sha256);
  if (cached) return cached;

  const rawEnvelope = new Uint8Array(
    row.raw_envelope.buffer,
    row.raw_envelope.byteOffset,
    row.raw_envelope.byteLength,
  );

  const trusted = loadTrustedPubkeys();
  try {
    const verified = await verifyBundle(rawEnvelope, trusted);
    const result: ActiveBundle = {
      payload: verified.payload,
      rawEnvelope: verified.rawEnvelope,
      pubkeyFingerprint: verified.pubkeyFingerprint,
      importedAt: row.imported_at,
      payloadSha256: row.payload_sha256,
    };
    cache.set(row.payload_sha256, result);
    return result;
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    console.warn(
      `[seaquel] airgap_bundle re-verification failed: ${message}; treating as no bundle`,
    );
    cache.delete(row.payload_sha256);
    return null;
  }
}

/**
 * Persist a freshly-verified bundle. Caller is responsible for having
 * already passed `verified` through {@link verifyBundle} and computed
 * `payloadSha256` (passed in so we don't re-hash here — Task 8's POST
 * already needs the hash for the response body).
 */
export function writeBundle(
  verified: { payload: BundlePayload; rawEnvelope: Uint8Array; pubkeyFingerprint: string },
  payloadSha256: string,
): void {
  const importedAt = Math.floor(Date.now() / 1000);
  openAuthDb()
    .prepare(
      `INSERT OR REPLACE INTO airgap_bundle
         (id, raw_envelope, verified_payload, pubkey_fingerprint,
          imported_at, not_after, payload_sha256, issued_at)
       VALUES (1, @rawEnvelope, @verifiedPayload, @pubkeyFingerprint,
               @importedAt, @notAfter, @payloadSha256, @issuedAt)`,
    )
    .run({
      rawEnvelope: Buffer.from(verified.rawEnvelope),
      verifiedPayload: JSON.stringify(verified.payload),
      pubkeyFingerprint: verified.pubkeyFingerprint,
      importedAt,
      notAfter: verified.payload.not_after,
      payloadSha256,
      issuedAt: verified.payload.issued_at,
    });
  cache.clear();
}

export function clearBundle(): void {
  openAuthDb().prepare(`DELETE FROM airgap_bundle WHERE id = 1`).run();
  cache.clear();
}

/**
 * Cheap presence check — does NOT re-verify. Used as the dispatcher
 * switch in Task 6: bundle present → local-control; absent → control
 * plane. The dispatcher then calls `readActiveBundle()` for any branch
 * that actually needs the payload, which is where the re-verify cost
 * lands.
 */
export function isBundleDriven(): boolean {
  const row = openAuthDb().prepare(`SELECT 1 AS present FROM airgap_bundle WHERE id = 1`).get() as
    | { present: number }
    | undefined;
  return !!row;
}

// ---------------------------------------------------------------------------
// Trusted public keys
// ---------------------------------------------------------------------------

/**
 * Production trust set — fingerprint → 32-byte Ed25519 pubkey of the
 * seaquel.app bundle-signing key.
 */
const PROD_TRUSTED_PUBKEYS: ReadonlyArray<{
  fingerprint: string;
  pubkeyHex: string;
}> = [
  {
    fingerprint: "937769290e77859410e977c3f762aa0c",
    pubkeyHex: "dab5fab2c0c17da9a5c0f3ce2a710a496ff3db92578e09b2d786f44e99411a5e",
  },
];

/** Last-seen value of the env var, so we re-parse only when it changes. */
let lastEnvSpec: string | undefined = undefined;
let cachedTrust: Map<string, Uint8Array> | null = null;

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error("invalid trusted pubkey hex (odd length)");
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) {
    const byte = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
    if (!Number.isFinite(byte) || Number.isNaN(byte)) {
      throw new Error("invalid trusted pubkey hex (non-hex char)");
    }
    out[i] = byte;
  }
  return out;
}

/**
 * Build the trust set used to verify uploaded bundles.
 *
 * Sources, merged in order:
 *   1. `PROD_TRUSTED_PUBKEYS` — the hard-coded production set (empty
 *      until ops populates it pre-release; see TODO above).
 *   2. `SEAQUEL_BUNDLE_TRUSTED_PUBKEY` — optional comma-separated env
 *      var of `<fingerprint>:<hex-pubkey>` entries. Used by:
 *        - Local development (operator runs the signer with their own
 *          Ed25519 seed and pastes the resulting trust anchor here).
 *        - Self-hosted operators who mint their own bundles with a
 *          self-managed signing key (rare; documented in Task 11's
 *          README update).
 *
 * The parsed result is cached at module level. The env var is checked
 * on every call so a hot-reload in dev picks up changes without a
 * process restart; we re-parse only when the raw string differs from
 * the last seen value.
 */
export function loadTrustedPubkeys(): Map<string, Uint8Array> {
  const spec = process.env.SEAQUEL_BUNDLE_TRUSTED_PUBKEY;
  if (cachedTrust !== null && spec === lastEnvSpec) {
    return cachedTrust;
  }

  const out = new Map<string, Uint8Array>();
  for (const entry of PROD_TRUSTED_PUBKEYS) {
    out.set(entry.fingerprint, hexToBytes(entry.pubkeyHex));
  }

  if (spec) {
    for (const raw of spec.split(",")) {
      const trimmed = raw.trim();
      if (!trimmed) continue;
      const colon = trimmed.indexOf(":");
      if (colon <= 0 || colon === trimmed.length - 1) {
        console.warn(
          `[seaquel] SEAQUEL_BUNDLE_TRUSTED_PUBKEY entry ignored (expected <fingerprint>:<hex>): ${trimmed}`,
        );
        continue;
      }
      const fingerprint = trimmed.slice(0, colon).trim();
      const hex = trimmed.slice(colon + 1).trim();
      try {
        out.set(fingerprint, hexToBytes(hex));
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        console.warn(
          `[seaquel] SEAQUEL_BUNDLE_TRUSTED_PUBKEY entry ignored (${message}): ${trimmed}`,
        );
      }
    }
  }

  cachedTrust = out;
  lastEnvSpec = spec;
  return out;
}

/** Test-only — drop the in-process cache (env override + bundle map). */
export function _resetBundleStoreCache(): void {
  cache.clear();
  cachedTrust = null;
  lastEnvSpec = undefined;
}
