/**
 * Canonical-JSON encoder for air-gapped bundles.
 *
 * Rules (applied recursively):
 *   - Object keys are sorted lexicographically (ASCII byte order).
 *   - No whitespace anywhere.
 *   - No trailing newline.
 *   - Strings use standard JSON escaping (\", \\, \b, \f, \n, \r, \t, and
 *     \u00XX for other control chars 0x00..0x1F).
 *   - Numbers are integers in our schema; emitted as base-10 with no leading
 *     zeros and no fractional part. Negative integers are allowed but only
 *     for `not_before` arithmetic edge cases.
 *   - Arrays are emitted in original order.
 *   - Empty arrays (e.g. `revoked_keys: []`) are always emitted, never omitted.
 *   - `null` values are emitted as `null`.
 *
 * IMPORTANT: this file is duplicated verbatim in
 *   seaquel-app/main/packages/marketing/src/lib/server/airgap/canonical.ts
 * The two repos share no package, but the bytes produced here must match
 * byte-for-byte across them. The golden-vector test in both repos pins this.
 */

export type CanonicalValue =
  | string
  | number
  | boolean
  | null
  | CanonicalValue[]
  | { [key: string]: CanonicalValue };

const HEX = "0123456789abcdef";

function escapeString(input: string): string {
  let out = '"';
  for (let i = 0; i < input.length; i++) {
    const ch = input.charCodeAt(i);
    switch (ch) {
      case 0x22: // "
        out += '\\"';
        break;
      case 0x5c: // \
        out += "\\\\";
        break;
      case 0x08:
        out += "\\b";
        break;
      case 0x09:
        out += "\\t";
        break;
      case 0x0a:
        out += "\\n";
        break;
      case 0x0c:
        out += "\\f";
        break;
      case 0x0d:
        out += "\\r";
        break;
      default:
        if (ch < 0x20) {
          out += "\\u00" + HEX[(ch >> 4) & 0xf] + HEX[ch & 0xf];
        } else {
          out += input[i];
        }
    }
  }
  out += '"';
  return out;
}

function encodeNumber(n: number): string {
  if (!Number.isFinite(n) || !Number.isInteger(n)) {
    throw new Error(`canonical: non-integer number ${String(n)}`);
  }
  // Base-10 with no leading zeros. JS toString already does this for ints,
  // including the single zero case ("0") and negatives ("-1").
  return n.toString(10);
}

function encode(value: CanonicalValue): string {
  if (value === null) return "null";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return encodeNumber(value);
  if (typeof value === "string") return escapeString(value);
  if (Array.isArray(value)) {
    let out = "[";
    for (let i = 0; i < value.length; i++) {
      if (i > 0) out += ",";
      out += encode(value[i]);
    }
    out += "]";
    return out;
  }
  if (typeof value === "object") {
    const keys = Object.keys(value).sort();
    let out = "{";
    for (let i = 0; i < keys.length; i++) {
      if (i > 0) out += ",";
      const k = keys[i];
      out += escapeString(k);
      out += ":";
      out += encode(value[k]);
    }
    out += "}";
    return out;
  }
  throw new Error(`canonical: unsupported value type ${typeof value}`);
}

/**
 * Produce the canonical UTF-8 byte representation of `value`.
 * Used by the signer (over the input payload) and by the verifier (to
 * re-encode the parsed payload and confirm byte-equality with the verified
 * `payload` field).
 */
export function canonicalize(value: CanonicalValue): Uint8Array {
  return new TextEncoder().encode(encode(value));
}

/** Lowercase hex of `bytes`. */
export function bytesToHex(bytes: Uint8Array): string {
  let hex = "";
  for (const b of bytes) hex += b.toString(16).padStart(2, "0");
  return hex;
}

/**
 * Compute the lowercase hex of the first 16 bytes (128 bits) of
 * `sha256(pubkey)`. Returns a 32-character string.
 *
 * Lives here (rather than alongside the signer or verifier) so the two repos
 * import the same byte-for-byte copy via the verbatim-synced file. Uses Web
 * Crypto so the result is identical on Node and Cloudflare Workers.
 */
export async function fingerprintPubkey(pubkey: Uint8Array): Promise<string> {
  // `crypto.subtle.digest` wants a `BufferSource`. TS 5.x flags
  // `Uint8Array<ArrayBufferLike>` as too wide for that signature
  // (SharedArrayBuffer is structurally compatible), so we narrow via cast.
  const digest = await crypto.subtle.digest("SHA-256", pubkey as Uint8Array<ArrayBuffer>);
  return bytesToHex(new Uint8Array(digest).slice(0, 16));
}
