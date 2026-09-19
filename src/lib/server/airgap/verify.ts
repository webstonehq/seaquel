/**
 * Verifier for air-gapped license bundles produced by the seaquel-app control
 * plane (see `packages/marketing/src/lib/server/airgap/bundle-signer.ts`).
 *
 * The verifier is pure: it takes raw envelope bytes plus a trust set of
 * Ed25519 public keys (keyed by their fingerprint) and returns the parsed
 * payload, or throws an `Error` whose `message` is one of the
 * `BundleVerifyError` strings.
 *
 * Expiry is intentionally NOT checked here. The plan stores the bundle
 * regardless of expiry; callers consult `isExpired()` separately.
 */
import * as ed from "@noble/ed25519";

import { canonicalize, type CanonicalValue } from "./canonical";
import type { BundlePayload, SignedEnvelope } from "./types";

export type { BundlePayload, SignedEnvelope } from "./types";
export { fingerprintPubkey } from "./canonical";

export interface VerifiedBundle {
  payload: BundlePayload;
  rawEnvelope: Uint8Array;
  pubkeyFingerprint: string;
}

export type BundleVerifyError =
  | "malformed_envelope"
  | "untrusted_signer"
  | "bad_signature"
  | "schema_mismatch";

/**
 * Maximum clock skew (in seconds) we accept on the `not_before` field. A
 * payload whose `not_before` is more than this far in the future is treated
 * as a forgery / clock-rollback attempt. One hour matches the signer's
 * `not_before = issued_at - 60` convention with a wide safety margin.
 */
export const SEAQUEL_AIRGAP_CLOCK_TOLERANCE_SECONDS = 3600;

function base64UrlDecode(input: string): Uint8Array {
  // Reject non-base64url characters early so we can throw `malformed_envelope`
  // rather than getting silently wrong bytes via atob's lenient parsing.
  if (!/^[A-Za-z0-9_-]*$/.test(input)) {
    throw new Error("malformed_envelope");
  }
  const padded = input + "===".slice((input.length + 3) % 4);
  const standard = padded.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(standard);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

const ALLOWED_KEYS: ReadonlySet<string> = new Set([
  "version",
  "issued_at",
  "not_before",
  "not_after",
  "subscription_id",
  "tenant_slug",
  "tier",
  "seats",
  "seat_tokens",
  "revoked_keys",
  "issued_by_install_id",
]);

function parsePayload(raw: unknown): BundlePayload {
  if (!isObject(raw)) throw new Error("schema_mismatch");

  // Reject any unknown top-level fields. The plan calls this out explicitly:
  // extra fields must fail `schema_mismatch`, not be silently accepted.
  for (const k of Object.keys(raw)) {
    if (!ALLOWED_KEYS.has(k)) throw new Error("schema_mismatch");
  }

  if (raw.version !== 1) throw new Error("schema_mismatch");

  const intField = (key: string): number => {
    const v = raw[key];
    if (typeof v !== "number" || !Number.isInteger(v)) {
      throw new Error("schema_mismatch");
    }
    return v;
  };
  const stringField = (key: string): string => {
    const v = raw[key];
    if (typeof v !== "string") throw new Error("schema_mismatch");
    return v;
  };

  const issued_at = intField("issued_at");
  const not_before = intField("not_before");
  const not_after = intField("not_after");
  const subscription_id = stringField("subscription_id");
  const tenant_slug = stringField("tenant_slug");
  const tier = stringField("tier");
  const seats = intField("seats");

  // Bounded numeric ranges. These are sanity guards against forged or
  // malformed bundles that nonetheless parse as integers. Failures map to
  // schema_mismatch (not bad_signature) because the signature itself may be
  // valid — the payload is what's nonsensical.
  if (seats < 0) throw new Error("schema_mismatch");
  // 10k seats is well above any plausible tier; treat higher as a forgery
  // attempt or schema confusion.
  if (seats > 10000) throw new Error("schema_mismatch");
  if (issued_at < 0 || not_before < 0 || not_after < 0) {
    throw new Error("schema_mismatch");
  }
  if (not_after < not_before) throw new Error("schema_mismatch");
  if (issued_at > not_after) throw new Error("schema_mismatch");

  const seatTokensRaw = raw.seat_tokens;
  if (!Array.isArray(seatTokensRaw)) throw new Error("schema_mismatch");
  const seat_tokens: BundlePayload["seat_tokens"] = seatTokensRaw.map((entry) => {
    if (!isObject(entry)) throw new Error("schema_mismatch");
    const entryKeys = Object.keys(entry);
    if (entryKeys.length !== 2 || !entryKeys.includes("key") || !entryKeys.includes("role")) {
      throw new Error("schema_mismatch");
    }
    if (typeof entry.key !== "string") throw new Error("schema_mismatch");
    if (entry.role !== "owner" && entry.role !== "member") {
      throw new Error("schema_mismatch");
    }
    return { key: entry.key, role: entry.role };
  });

  const revokedKeysRaw = raw.revoked_keys;
  if (!Array.isArray(revokedKeysRaw)) throw new Error("schema_mismatch");
  const revoked_keys: string[] = revokedKeysRaw.map((entry) => {
    if (typeof entry !== "string") throw new Error("schema_mismatch");
    return entry;
  });

  const issuedByRaw = raw.issued_by_install_id;
  let issued_by_install_id: string | null;
  if (issuedByRaw === null) {
    issued_by_install_id = null;
  } else if (typeof issuedByRaw === "string") {
    issued_by_install_id = issuedByRaw;
  } else {
    throw new Error("schema_mismatch");
  }

  return {
    version: 1,
    issued_at,
    not_before,
    not_after,
    subscription_id,
    tenant_slug,
    tier,
    seats,
    seat_tokens,
    revoked_keys,
    issued_by_install_id,
  };
}

function parseEnvelope(raw: Uint8Array): SignedEnvelope {
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(raw);
  } catch {
    throw new Error("malformed_envelope");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("malformed_envelope");
  }
  if (!isObject(parsed)) throw new Error("malformed_envelope");
  const { payload, sig, pubkey_fingerprint } = parsed as Record<string, unknown>;
  if (
    typeof payload !== "string" ||
    typeof sig !== "string" ||
    typeof pubkey_fingerprint !== "string"
  ) {
    throw new Error("malformed_envelope");
  }
  return { payload, sig, pubkey_fingerprint };
}

/**
 * Verify a signed bundle envelope against a trust set.
 *
 * Throws an `Error` whose `message` is one of the `BundleVerifyError`
 * strings. Does NOT throw `expired` — callers handle expiry via
 * `isExpired()`.
 */
export async function verifyBundle(
  rawEnvelopeBytes: Uint8Array,
  trustedPubkeys: Map<string, Uint8Array>,
): Promise<VerifiedBundle> {
  const envelope = parseEnvelope(rawEnvelopeBytes);

  const pubkey = trustedPubkeys.get(envelope.pubkey_fingerprint);
  if (!pubkey) throw new Error("untrusted_signer");

  let payloadBytes: Uint8Array;
  let sigBytes: Uint8Array;
  try {
    payloadBytes = base64UrlDecode(envelope.payload);
    sigBytes = base64UrlDecode(envelope.sig);
  } catch {
    throw new Error("malformed_envelope");
  }

  let sigOk: boolean;
  try {
    sigOk = await ed.verifyAsync(sigBytes, payloadBytes, pubkey);
  } catch {
    sigOk = false;
  }
  if (!sigOk) throw new Error("bad_signature");

  let parsedRaw: unknown;
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(payloadBytes);
    parsedRaw = JSON.parse(text);
  } catch {
    throw new Error("schema_mismatch");
  }
  const payload = parsePayload(parsedRaw);

  // Re-canonicalise and compare with the verified bytes. This rejects
  // non-canonical signing attempts (e.g. attacker-controlled key order, extra
  // whitespace) which would otherwise pass the raw Ed25519 check.
  const reCanon = canonicalize(payload as unknown as CanonicalValue);
  if (!bytesEqual(reCanon, payloadBytes)) {
    throw new Error("bad_signature");
  }

  // Clock-rollback check. We tolerate up to SEAQUEL_AIRGAP_CLOCK_TOLERANCE_SECONDS
  // of clock skew on the verifier side, but a `not_before` further than that
  // in the future indicates a forged bundle dated to the future to bypass
  // revocation.
  const now = Math.floor(Date.now() / 1000);
  if (now < payload.not_before - SEAQUEL_AIRGAP_CLOCK_TOLERANCE_SECONDS) {
    throw new Error("bad_signature");
  }

  return {
    payload,
    rawEnvelope: rawEnvelopeBytes,
    pubkeyFingerprint: envelope.pubkey_fingerprint,
  };
}

/** Helper for callers that want to know whether a verified bundle is past its expiry. */
export function isExpired(
  bundle: VerifiedBundle,
  nowSeconds: number = Math.floor(Date.now() / 1000),
): boolean {
  return nowSeconds > bundle.payload.not_after;
}
