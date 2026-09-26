//! The verifier for air-gap license bundles signed by the seaquel-app
//! control plane, ported from `verify.ts`.
//!
//! Pure: raw envelope bytes, a trust set of Ed25519 public keys keyed by
//! fingerprint, and the time, in; the parsed payload or one of four error
//! codes out. Expiry is not checked here; callers use [`is_expired`].
//!
//! It follows the TypeScript's JavaScript semantics where they decide the
//! answer: UTF-8 decoding drops a leading BOM (as `TextDecoder` does),
//! numbers are JavaScript numbers (so `3.0` is the integer 3 and a digit
//! string past 2^53 rounds), base64url is decoded as `atob` did (lenient
//! trailing bits, a length of 4n + 1 refused), and the payload must
//! re-canonicalise to exactly the signed bytes.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Serialize;
use serde_json::{Map, Value};

use super::canonical::{canonicalize, CanonicalValue};
use super::TrustSet;
use crate::server::js::number_value;

/// How far in the future (seconds) a payload's `not_before` may be before
/// the bundle counts as forged (a clock-rollback attempt).
pub const CLOCK_TOLERANCE_SECONDS: i64 = 3600;

/// Why a bundle was refused. The wire strings are the TypeScript's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleVerifyError {
    MalformedEnvelope,
    UntrustedSigner,
    BadSignature,
    SchemaMismatch,
}

impl BundleVerifyError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MalformedEnvelope => "malformed_envelope",
            Self::UntrustedSigner => "untrusted_signer",
            Self::BadSignature => "bad_signature",
            Self::SchemaMismatch => "schema_mismatch",
        }
    }
}

impl std::fmt::Display for BundleVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for BundleVerifyError {}

/// A seat's role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeatRole {
    Owner,
    Member,
}

/// One seat: a license key and its role. `Debug` hides the key.
#[derive(Clone, PartialEq, Serialize)]
pub struct SeatToken {
    pub key: String,
    pub role: SeatRole,
}

impl std::fmt::Debug for SeatToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeatToken")
            .field("key", &"<redacted>")
            .field("role", &self.role)
            .finish()
    }
}

/// A verified payload. Numbers are JavaScript numbers (`f64`), always
/// integers here. Serializes in `parsePayload`'s key order, as
/// `JSON.stringify` wrote it. `Debug` hides every license key.
#[derive(Clone, PartialEq)]
pub struct BundlePayload {
    pub issued_at: f64,
    pub not_before: f64,
    pub not_after: f64,
    pub subscription_id: String,
    pub tenant_slug: String,
    pub tier: String,
    pub seats: f64,
    pub seat_tokens: Vec<SeatToken>,
    pub revoked_keys: Vec<String>,
    pub issued_by_install_id: Option<String>,
}

impl std::fmt::Debug for BundlePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundlePayload")
            .field("issued_at", &self.issued_at)
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .field("subscription_id", &self.subscription_id)
            .field("tenant_slug", &self.tenant_slug)
            .field("tier", &self.tier)
            .field("seats", &self.seats)
            .field("seat_tokens", &self.seat_tokens)
            .field("revoked_keys", &self.revoked_keys.len())
            .field("issued_by_install_id", &self.issued_by_install_id)
            .finish()
    }
}

impl Serialize for BundlePayload {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("BundlePayload", 11)?;
        st.serialize_field("version", &1)?;
        st.serialize_field("issued_at", &number_value(self.issued_at))?;
        st.serialize_field("not_before", &number_value(self.not_before))?;
        st.serialize_field("not_after", &number_value(self.not_after))?;
        st.serialize_field("subscription_id", &self.subscription_id)?;
        st.serialize_field("tenant_slug", &self.tenant_slug)?;
        st.serialize_field("tier", &self.tier)?;
        st.serialize_field("seats", &number_value(self.seats))?;
        st.serialize_field("seat_tokens", &self.seat_tokens)?;
        st.serialize_field("revoked_keys", &self.revoked_keys)?;
        st.serialize_field("issued_by_install_id", &self.issued_by_install_id)?;
        st.end()
    }
}

impl BundlePayload {
    /// The payload as the canonicaliser sees it.
    pub fn to_canonical(&self) -> CanonicalValue {
        use CanonicalValue as C;
        let s = |v: &str| C::String(v.to_string());
        C::Object(vec![
            ("version".into(), C::Number(1.0)),
            ("issued_at".into(), C::Number(self.issued_at)),
            ("not_before".into(), C::Number(self.not_before)),
            ("not_after".into(), C::Number(self.not_after)),
            ("subscription_id".into(), s(&self.subscription_id)),
            ("tenant_slug".into(), s(&self.tenant_slug)),
            ("tier".into(), s(&self.tier)),
            ("seats".into(), C::Number(self.seats)),
            (
                "seat_tokens".into(),
                C::Array(
                    self.seat_tokens
                        .iter()
                        .map(|t| {
                            C::Object(vec![
                                ("key".into(), s(&t.key)),
                                (
                                    "role".into(),
                                    s(match t.role {
                                        SeatRole::Owner => "owner",
                                        SeatRole::Member => "member",
                                    }),
                                ),
                            ])
                        })
                        .collect(),
                ),
            ),
            (
                "revoked_keys".into(),
                C::Array(self.revoked_keys.iter().map(|k| s(k)).collect()),
            ),
            (
                "issued_by_install_id".into(),
                self.issued_by_install_id.as_deref().map_or(C::Null, s),
            ),
        ])
    }

    /// `sha256(canonicalize(payload))`, lowercase hex: the store's cache
    /// key and the upload response's `payloadSha256`.
    pub fn sha256_hex(&self) -> String {
        // A verified payload's numbers are integers, so this can't fail.
        let bytes = canonicalize(&self.to_canonical()).unwrap_or_default();
        super::canonical::sha256_hex(&bytes)
    }
}

/// A bundle that passed [`verify_bundle`].
#[derive(Debug, Clone)]
pub struct VerifiedBundle {
    pub payload: BundlePayload,
    pub raw_envelope: Vec<u8>,
    pub pubkey_fingerprint: String,
}

/// Whether the bundle is past its `not_after` at `now` (unix seconds).
pub fn is_expired(bundle: &VerifiedBundle, now: i64) -> bool {
    now as f64 > bundle.payload.not_after
}

/// `TextDecoder("utf-8", { fatal: true })`: strict UTF-8, one leading BOM
/// dropped.
fn decode_utf8(bytes: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.strip_prefix('\u{FEFF}').unwrap_or(text))
}

/// base64url as `verify.ts` decoded it: only `[A-Za-z0-9_-]`, padding added
/// back, then `atob`, which ignores leftover bits and refuses a length of
/// 4n + 1.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    if !input
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    ENGINE.decode(input).ok()
}

/// A JSON number that is a JavaScript integer (`Number.isInteger`).
fn int_field(raw: &Map<String, Value>, key: &str) -> Result<f64, BundleVerifyError> {
    match raw.get(key).and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n.fract() == 0.0 => Ok(n),
        _ => Err(BundleVerifyError::SchemaMismatch),
    }
}

fn string_field(raw: &Map<String, Value>, key: &str) -> Result<String, BundleVerifyError> {
    match raw.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(BundleVerifyError::SchemaMismatch),
    }
}

const ALLOWED_KEYS: [&str; 11] = [
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
];

/// `parsePayload`: every rule gives `schema_mismatch`.
fn parse_payload(raw: &Value) -> Result<BundlePayload, BundleVerifyError> {
    use BundleVerifyError::SchemaMismatch;
    let Value::Object(raw) = raw else {
        return Err(SchemaMismatch);
    };
    if raw.keys().any(|k| !ALLOWED_KEYS.contains(&k.as_str())) {
        return Err(SchemaMismatch);
    }
    if raw.get("version").and_then(Value::as_f64) != Some(1.0) {
        return Err(SchemaMismatch);
    }
    let issued_at = int_field(raw, "issued_at")?;
    let not_before = int_field(raw, "not_before")?;
    let not_after = int_field(raw, "not_after")?;
    let subscription_id = string_field(raw, "subscription_id")?;
    let tenant_slug = string_field(raw, "tenant_slug")?;
    let tier = string_field(raw, "tier")?;
    let seats = int_field(raw, "seats")?;

    if !(0.0..=10000.0).contains(&seats) {
        return Err(SchemaMismatch);
    }
    if issued_at < 0.0 || not_before < 0.0 || not_after < 0.0 {
        return Err(SchemaMismatch);
    }
    if not_after < not_before || issued_at > not_after {
        return Err(SchemaMismatch);
    }

    let Some(Value::Array(tokens)) = raw.get("seat_tokens") else {
        return Err(SchemaMismatch);
    };
    let seat_tokens = tokens
        .iter()
        .map(|entry| {
            let Value::Object(entry) = entry else {
                return Err(SchemaMismatch);
            };
            if entry.len() != 2 {
                return Err(SchemaMismatch);
            }
            let key = match entry.get("key") {
                Some(Value::String(k)) => k.clone(),
                _ => return Err(SchemaMismatch),
            };
            let role = match entry.get("role").and_then(Value::as_str) {
                Some("owner") => SeatRole::Owner,
                Some("member") => SeatRole::Member,
                _ => return Err(SchemaMismatch),
            };
            Ok(SeatToken { key, role })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let Some(Value::Array(revoked)) = raw.get("revoked_keys") else {
        return Err(SchemaMismatch);
    };
    let revoked_keys = revoked
        .iter()
        .map(|k| match k {
            Value::String(s) => Ok(s.clone()),
            _ => Err(SchemaMismatch),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let issued_by_install_id = match raw.get("issued_by_install_id") {
        Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => return Err(SchemaMismatch),
    };

    Ok(BundlePayload {
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
    })
}

/// The envelope's three strings.
struct Envelope {
    payload: String,
    sig: String,
    pubkey_fingerprint: String,
}

fn parse_envelope(raw: &[u8]) -> Result<Envelope, BundleVerifyError> {
    use BundleVerifyError::MalformedEnvelope;
    let text = decode_utf8(raw).ok_or(MalformedEnvelope)?;
    let parsed: Value = serde_json::from_str(text).map_err(|_| MalformedEnvelope)?;
    let Value::Object(obj) = parsed else {
        return Err(MalformedEnvelope);
    };
    let field = |k: &str| match obj.get(k) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(MalformedEnvelope),
    };
    Ok(Envelope {
        payload: field("payload")?,
        sig: field("sig")?,
        pubkey_fingerprint: field("pubkey_fingerprint")?,
    })
}

fn signature_ok(pubkey: &[u8], sig: &[u8], message: &[u8]) -> bool {
    let Ok(pubkey) = <[u8; 32]>::try_from(pubkey) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&pubkey) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(sig) else {
        return false;
    };
    // RFC 8032 verification. noble (the TS side) used ZIP-215 rules, which
    // differ only for non-canonical point encodings and small-order
    // components that a signature from a trusted key never has.
    key.verify(message, &sig).is_ok()
}

/// Verify a signed bundle envelope against `trusted` at `now` (unix
/// seconds). Never fails for expiry; see [`is_expired`].
pub fn verify_bundle(
    raw_envelope: &[u8],
    trusted: &TrustSet,
    now: i64,
) -> Result<VerifiedBundle, BundleVerifyError> {
    let envelope = parse_envelope(raw_envelope)?;

    let pubkey = trusted
        .get(&envelope.pubkey_fingerprint)
        .ok_or(BundleVerifyError::UntrustedSigner)?;

    let payload_bytes =
        base64url_decode(&envelope.payload).ok_or(BundleVerifyError::MalformedEnvelope)?;
    let sig_bytes = base64url_decode(&envelope.sig).ok_or(BundleVerifyError::MalformedEnvelope)?;

    if !signature_ok(pubkey, &sig_bytes, &payload_bytes) {
        return Err(BundleVerifyError::BadSignature);
    }

    let parsed: Value = decode_utf8(&payload_bytes)
        .and_then(|t| serde_json::from_str(t).ok())
        .ok_or(BundleVerifyError::SchemaMismatch)?;
    let payload = parse_payload(&parsed)?;

    // Re-canonicalise and compare with the signed bytes: a non-canonical
    // signing (key order, whitespace, `3.0`) fails here.
    let re_canon =
        canonicalize(&payload.to_canonical()).map_err(|_| BundleVerifyError::BadSignature)?;
    if re_canon != payload_bytes {
        return Err(BundleVerifyError::BadSignature);
    }

    // Clock rollback: a `not_before` further ahead than the tolerance.
    if (now as f64) < payload.not_before - CLOCK_TOLERANCE_SECONDS as f64 {
        return Err(BundleVerifyError::BadSignature);
    }

    Ok(VerifiedBundle {
        payload,
        raw_envelope: raw_envelope.to_vec(),
        pubkey_fingerprint: envelope.pubkey_fingerprint,
    })
}
