/**
 * Smoke test for the vault crypto primitives. Run with:
 *
 *     node scripts/verify-vault-crypto.ts
 *
 * Node 22+ strips the TypeScript annotations natively. This script exercises
 * the same module that the browser uses, so any regression in the crypto
 * surface will surface here first.
 */
import {
  DEFAULT_KDF_PARAMS,
  VERIFIER_PLAINTEXT,
  decrypt,
  decryptToString,
  deriveVaultKey,
  encrypt,
  fromBase64,
  randomSalt,
  toBase64,
} from "../src/lib/services/vault/crypto.ts";

function assert(condition: unknown, message: string): void {
  if (!condition) {
    console.error(`FAIL: ${message}`);
    process.exit(1);
  }
  console.log(`ok:   ${message}`);
}

async function main(): Promise<void> {
  // Use smaller KDF params for this smoke test so it finishes in a
  // reasonable amount of time. Production params are DEFAULT_KDF_PARAMS.
  const testParams = { ...DEFAULT_KDF_PARAMS, t: 1, m: 8192, p: 1 };

  const salt = randomSalt();
  assert(salt.length === 16, "salt is 16 bytes");

  const key = await deriveVaultKey("correct horse battery staple", salt, testParams);
  assert(key.extractable === false, "vault key is non-extractable");
  assert(key.algorithm.name === "AES-GCM", "vault key is AES-GCM");

  // Round-trip a plaintext secret.
  const secret = "hunter2-is-a-bad-password";
  const blob = await encrypt(key, secret);
  assert(blob.nonce.length === 12, "nonce is 12 bytes");
  assert(blob.ciphertext.length >= secret.length + 16, "ciphertext includes auth tag");

  const roundTripped = await decryptToString(key, blob);
  assert(roundTripped === secret, "decrypt round-trip matches plaintext");

  // Same passphrase + same salt must derive the same key (decrypt succeeds).
  const key2 = await deriveVaultKey("correct horse battery staple", salt, testParams);
  const roundTripped2 = await decryptToString(key2, blob);
  assert(roundTripped2 === secret, "re-derived key decrypts earlier ciphertext");

  // Wrong passphrase must produce a different VK; decrypt must throw (GCM
  // auth failure is how we surface a bad passphrase).
  const wrongKey = await deriveVaultKey("incorrect battery horse staple", salt, testParams);
  let threw = false;
  try {
    await decrypt(wrongKey, blob);
  } catch {
    threw = true;
  }
  assert(threw, "wrong passphrase is rejected at decrypt time");

  // Verifier pattern: encrypt a known plaintext at setup, decrypt to
  // confirm passphrase on unlock.
  const verifier = await encrypt(key, VERIFIER_PLAINTEXT);
  const confirmed = await decryptToString(key2, verifier);
  assert(confirmed === VERIFIER_PLAINTEXT, "verifier round-trip confirms passphrase");

  // Base64 round-trip (used by the wire format for /api/storage/*).
  const b64 = toBase64(blob.ciphertext);
  const decoded = fromBase64(b64);
  assert(decoded.length === blob.ciphertext.length, "base64 round-trip length matches");
  for (let i = 0; i < decoded.length; i++) {
    if (decoded[i] !== blob.ciphertext[i]) {
      assert(false, `base64 round-trip byte ${i} differs`);
    }
  }
  assert(true, "base64 round-trip bytes match");

  // Different salt → different VK even with same passphrase.
  const otherSalt = randomSalt();
  const otherKey = await deriveVaultKey("correct horse battery staple", otherSalt, testParams);
  let threwOnWrongSalt = false;
  try {
    await decrypt(otherKey, blob);
  } catch {
    threwOnWrongSalt = true;
  }
  assert(threwOnWrongSalt, "different salt yields different key — decrypt fails");

  console.log("\nvault crypto: all checks passed");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
