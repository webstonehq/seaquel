/**
 * Air-gapped bundle schema. The control plane (seaquel-app) signs envelopes
 * matching this shape; self-hosted Seaquel instances verify them.
 *
 * IMPORTANT: this file is duplicated verbatim in
 *   seaquel-app/main/packages/marketing/src/lib/server/airgap/types.ts
 * The two repos share no package, but the schema must stay byte-for-byte
 * compatible. If you change this file, change it there too.
 */

export interface BundlePayload {
  version: 1;
  /** Unix seconds. */
  issued_at: number;
  /** Unix seconds. Conventionally `issued_at - 60`. */
  not_before: number;
  /** Unix seconds. Verifier rejects clock-rollback attacks before this. */
  not_after: number;
  subscription_id: string;
  tenant_slug: string;
  tier: string;
  seats: number;
  seat_tokens: Array<{ key: string; role: "owner" | "member" }>;
  /** Always emitted, even when empty. */
  revoked_keys: string[];
  /** Null when the bundle is issued by the control plane itself rather than a delegated installer. */
  issued_by_install_id: string | null;
}

export interface SignedEnvelope {
  /** base64url of canonical-JSON bytes, without padding (RFC 4648 §5). */
  payload: string;
  /** base64url of Ed25519 signature over the canonical-JSON bytes, without padding (RFC 4648 §5). */
  sig: string;
  /** lowercase hex of the first 16 bytes (128 bits) of `sha256(pubkey)`. 32 hex characters. */
  pubkey_fingerprint: string;
}
