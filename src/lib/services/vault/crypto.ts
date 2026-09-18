/**
 * Browser-side vault crypto primitives.
 *
 * - Key derivation: Argon2id (t=3, m=64MB, p=4) via `@noble/hashes`.
 * - Data cipher: AES-GCM-256 via WebCrypto with a **non-extractable** CryptoKey,
 *   so the Vault Key cannot be read back out of the JS heap once imported.
 * - 12-byte random nonce per encryption (standard AES-GCM).
 *
 * Wire format: `salt`, `nonce`, and `ciphertext` travel as base64-encoded TEXT
 * through the existing `/api/storage/*` JSON pipe. SQLite doesn't care whether
 * the blob columns are TEXT or BLOB; TEXT keeps the wire layer unchanged.
 *
 * The plan cites XChaCha20-Poly1305 as a stand-in for "an AEAD we trust." We
 * use AES-GCM because WebCrypto ships it natively and supports non-extractable
 * keys — a structurally stronger in-memory property than holding raw 32 bytes
 * that a bug or devtools could read. Security goals (AEAD, 256-bit key, random
 * nonce, authenticated decryption) are the same.
 */
import { argon2id } from "@noble/hashes/argon2.js";

// WebCrypto's BufferSource type wants `ArrayBufferView<ArrayBuffer>`, but
// libraries (noble, native methods) return `Uint8Array<ArrayBufferLike>` —
// a wider TS type that also admits SharedArrayBuffer. At runtime the two
// are identical for our call sites; coerce at the API boundary.
function asBufferSource(u: Uint8Array): BufferSource {
  return u as unknown as BufferSource;
}

export const KDF_VERSION = 1;

export interface KdfParams {
  version: number;
  /** Argon2id iterations. */
  t: number;
  /** Argon2id memory cost in KiB. */
  m: number;
  /** Argon2id parallelism. */
  p: number;
}

export const DEFAULT_KDF_PARAMS: KdfParams = {
  version: KDF_VERSION,
  // NIST SP 800-63B (rev. 4) permits Argon2id at t>=2 when m>=19 MiB; we run
  // at 64 MiB, so raising t to 4 keeps roughly an order-of-magnitude margin
  // over spec without making the unlock dialog feel sluggish (≈2s on a
  // typical laptop). Existing vaults keep the `t` they were set up with —
  // this default only affects new vaults.
  t: 4,
  m: 65536, // 64 MiB
  p: 4,
};

export const SALT_BYTES = 16;
export const NONCE_BYTES = 12;
export const KEY_BYTES = 32;

/**
 * Known plaintext we encrypt with the VK at setup time and store as the
 * `verifier`. Decrypting the verifier on unlock proves the passphrase is
 * correct without having to try decrypting a real secret (which could look
 * "valid" as garbage bytes under AES-GCM only because the auth tag is
 * separate — but that's exactly the point of GCM, so really the verifier is
 * for *user-facing* "wrong passphrase" error handling).
 */
export const VERIFIER_PLAINTEXT = "seaquel:vault:v1";

/**
 * Derive the Vault Key from a passphrase + salt. Returns a non-extractable
 * AES-GCM CryptoKey. The raw derived bytes are zeroed before return.
 */
export async function deriveVaultKey(
  passphrase: string,
  salt: Uint8Array,
  params: KdfParams,
): Promise<CryptoKey> {
  const pwBytes = new TextEncoder().encode(passphrase);
  const raw = argon2id(pwBytes, salt, {
    t: params.t,
    m: params.m,
    p: params.p,
    dkLen: KEY_BYTES,
  });
  try {
    return await crypto.subtle.importKey(
      "raw",
      asBufferSource(raw),
      { name: "AES-GCM", length: 256 },
      false, // non-extractable
      ["encrypt", "decrypt"],
    );
  } finally {
    raw.fill(0);
    pwBytes.fill(0);
  }
}

export interface EncryptedBlob {
  /** Random per-row nonce. */
  nonce: Uint8Array;
  /** AES-GCM ciphertext including the authentication tag. */
  ciphertext: Uint8Array;
}

export async function encrypt(
  key: CryptoKey,
  plaintext: string | Uint8Array,
): Promise<EncryptedBlob> {
  const plain = typeof plaintext === "string" ? new TextEncoder().encode(plaintext) : plaintext;
  const nonce = crypto.getRandomValues(new Uint8Array(NONCE_BYTES));
  const ciphertext = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: "AES-GCM", iv: asBufferSource(nonce) },
      key,
      asBufferSource(plain),
    ),
  );
  return { nonce, ciphertext };
}

export async function decrypt(key: CryptoKey, blob: EncryptedBlob): Promise<Uint8Array> {
  const plain = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: asBufferSource(blob.nonce) },
    key,
    asBufferSource(blob.ciphertext),
  );
  return new Uint8Array(plain);
}

export async function decryptToString(key: CryptoKey, blob: EncryptedBlob): Promise<string> {
  return new TextDecoder().decode(await decrypt(key, blob));
}

export function randomSalt(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(SALT_BYTES));
}

// ---------------------------------------------------------------------------
// Base64 helpers — the /api/storage/* wire layer is JSON, so binary columns
// travel as base64-encoded strings. Kept tiny and dep-free.
// ---------------------------------------------------------------------------

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}

export function fromBase64(b64: string): Uint8Array {
  // `atob` throws on malformed input, but the default `DOMException: The
  // string to be decoded is not correctly encoded` is opaque. Wrap it so
  // truncated ciphertext from the DB — or any other corrupted value —
  // surfaces as a concrete error instead of leaking out as a DOMException.
  let binary: string;
  try {
    binary = atob(b64);
  } catch {
    throw new Error("invalid base64 input");
  }
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
