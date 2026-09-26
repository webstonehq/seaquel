//! Verifier tests, ported case for case from `airgap/verify.test.ts`.
//!
//! The golden-vector block is the cross-runtime regression net shared with
//! seaquel-app (`packages/marketing/src/lib/server/airgap/bundle-signer.test.ts`):
//! the same payload, signed with the same seed, must produce the same bytes on
//! every side. The constants below are copied from the TypeScript word for
//! word. Never change them.

mod common;

use common::{b64url, b64url_decode, hex_to_bytes, now_secs, sign_canonical, sign_raw, trust_set};
use ed25519_dalek::SigningKey;
use seaquel_license::server::airgap::canonical::{
    canonicalize, fingerprint_pubkey, CanonicalValue,
};
use seaquel_license::server::airgap::verify::{
    is_expired, verify_bundle, BundleVerifyError, CLOCK_TOLERANCE_SECONDS,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Golden vectors — keep byte-for-byte identical with the seaquel-app copy.
// ---------------------------------------------------------------------------

fn golden_payload() -> Value {
    json!({
        "version": 1,
        "issued_at": 1700000000,
        "not_before": 1699999940,
        "not_after": 1702592000,
        "subscription_id": "sub_test_0001",
        "tenant_slug": "acme",
        "tier": "team",
        "seats": 3,
        "seat_tokens": [
            { "key": "owner_key_abc", "role": "owner" },
            { "key": "member_key_xyz", "role": "member" },
        ],
        "revoked_keys": [],
        "issued_by_install_id": null,
    })
}

const GOLDEN_CANONICAL_JSON: &str = r#"{"issued_at":1700000000,"issued_by_install_id":null,"not_after":1702592000,"not_before":1699999940,"revoked_keys":[],"seat_tokens":[{"key":"owner_key_abc","role":"owner"},{"key":"member_key_xyz","role":"member"}],"seats":3,"subscription_id":"sub_test_0001","tenant_slug":"acme","tier":"team","version":1}"#;

const GOLDEN_SEED_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const GOLDEN_PUBKEY_HEX: &str = "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";
const GOLDEN_PUBKEY_FINGERPRINT: &str = "56475aa75463474c0285df5dbf2bcab7";
const GOLDEN_PAYLOAD_B64URL: &str = "eyJpc3N1ZWRfYXQiOjE3MDAwMDAwMDAsImlzc3VlZF9ieV9pbnN0YWxsX2lkIjpudWxsLCJub3RfYWZ0ZXIiOjE3MDI1OTIwMDAsIm5vdF9iZWZvcmUiOjE2OTk5OTk5NDAsInJldm9rZWRfa2V5cyI6W10sInNlYXRfdG9rZW5zIjpbeyJrZXkiOiJvd25lcl9rZXlfYWJjIiwicm9sZSI6Im93bmVyIn0seyJrZXkiOiJtZW1iZXJfa2V5X3h5eiIsInJvbGUiOiJtZW1iZXIifV0sInNlYXRzIjozLCJzdWJzY3JpcHRpb25faWQiOiJzdWJfdGVzdF8wMDAxIiwidGVuYW50X3NsdWciOiJhY21lIiwidGllciI6InRlYW0iLCJ2ZXJzaW9uIjoxfQ";
const GOLDEN_SIG_B64URL: &str =
    "hmH2dfTnY_77RrUnFm8fKb92jG3ibu4iV3JZvn7UNoRZpkFET2M2Xh2vR7UVAPQZLR2vAgrSXYBw60RBlTdQAg";

fn golden_seed() -> SigningKey {
    SigningKey::from_bytes(&hex_to_bytes(GOLDEN_SEED_HEX).try_into().unwrap())
}

fn canon(v: &Value) -> Vec<u8> {
    canonicalize(&CanonicalValue::from(v)).unwrap()
}

/// Verify now, with the real clock, the way the TypeScript did.
fn verify(
    bytes: &[u8],
    trust: &seaquel_license::server::airgap::TrustSet,
) -> Result<seaquel_license::server::airgap::verify::VerifiedBundle, BundleVerifyError> {
    verify_bundle(bytes, trust, now_secs())
}

fn payload_json(v: &seaquel_license::server::airgap::verify::VerifiedBundle) -> Value {
    serde_json::to_value(&v.payload).unwrap()
}

// ---------------------------------------------------------------------------
// canonicalize
// ---------------------------------------------------------------------------

#[test]
fn canonicalize_matches_the_golden_canonical_json_byte_for_byte() {
    let text = String::from_utf8(canon(&golden_payload())).unwrap();
    assert_eq!(text, GOLDEN_CANONICAL_JSON);
}

#[test]
fn canonicalize_is_stable_across_key_ordering_of_the_input() {
    // serde_json's map sorts on its own, so build the reordered input by
    // parsing text in a different order.
    let reordered: Value = serde_json::from_str(
        r#"{"tier":"team","issued_at":1700000000,"seats":3,"version":1,"tenant_slug":"acme","issued_by_install_id":null,"seat_tokens":[{"role":"owner","key":"owner_key_abc"},{"role":"member","key":"member_key_xyz"}],"not_before":1699999940,"not_after":1702592000,"revoked_keys":[],"subscription_id":"sub_test_0001"}"#,
    )
    .unwrap();
    assert_eq!(canon(&reordered), canon(&golden_payload()));
}

#[test]
fn canonicalize_emits_empty_arrays_explicitly() {
    let text = String::from_utf8(canon(&golden_payload())).unwrap();
    assert!(text.contains(r#""revoked_keys":[]"#));
}

#[test]
fn canonicalize_emits_null_values_explicitly() {
    let text = String::from_utf8(canon(&golden_payload())).unwrap();
    assert!(text.contains(r#""issued_by_install_id":null"#));
}

/// Beyond the TypeScript suite: non-ASCII text, control characters, JS
/// number formatting and UTF-16 key order. The expected bytes were produced
/// by `canonical.ts` itself (Node 24) before it was deleted.
#[test]
fn canonicalize_matches_canonical_ts_on_unicode_controls_and_numbers() {
    let mut obj = serde_json::Map::new();
    obj.insert("z".into(), json!("é\u{1}\u{1f}\"\\\u{8}\u{c}\n\r\t\u{7f}/"));
    obj.insert(
        "a".into(),
        // -0, 0, 1e21, 2^53 + 1 (rounds to 2^53 as a JS number), -5, 12 digits.
        serde_json::from_str("[-0, 0, 1e21, 9007199254740993, -5, 123456789012]").unwrap(),
    );
    obj.insert("é".into(), Value::Null);
    obj.insert("b".into(), json!(true));
    obj.insert("\u{1F600}".into(), json!(1));
    obj.insert("\u{FFFF}".into(), json!(2));
    obj.insert("".into(), json!([]));
    obj.insert(
        "nested".into(),
        json!({ "y": {}, "x": [{ "b": 1, "a": 2 }] }),
    );
    let bytes = canon(&Value::Object(obj));
    let expected = hex_to_bytes(
        "7b22223a5b5d2c2261223a5b302c302c31652b32312c393030373139393235343734303939322c2d352c3132333435363738393031325d2c2262223a747275652c226e6573746564223a7b2278223a5b7b2261223a322c2262223a317d5d2c2279223a7b7d7d2c227a223a22c3a95c75303030315c75303031665c225c5c5c625c665c6e5c725c747f2f222c22c3a9223a6e756c6c2c22f09f9880223a312c22efbfbf223a327d",
    );
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        String::from_utf8_lossy(&expected)
    );
    assert_eq!(bytes, expected);
}

// ---------------------------------------------------------------------------
// golden vector
// ---------------------------------------------------------------------------

#[test]
fn golden_vector_matches_the_published_pubkey_hex() {
    let pubkey = golden_seed().verifying_key().to_bytes();
    let hex: String = pubkey.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, GOLDEN_PUBKEY_HEX);
}

#[test]
fn golden_vector_matches_the_published_payload_and_signature_when_re_signed() {
    let canonical = canon(&golden_payload());
    let sig = sign_raw(&golden_seed(), &canonical);
    assert_eq!(b64url(&canonical), GOLDEN_PAYLOAD_B64URL);
    assert_eq!(b64url(&sig), GOLDEN_SIG_B64URL);
}

#[test]
fn golden_vector_matches_the_published_pubkey_fingerprint() {
    let pubkey = hex_to_bytes(GOLDEN_PUBKEY_HEX);
    assert_eq!(fingerprint_pubkey(&pubkey), GOLDEN_PUBKEY_FINGERPRINT);
}

#[test]
fn golden_vector_verifies_an_envelope_built_from_it() {
    let envelope = json!({
        "payload": GOLDEN_PAYLOAD_B64URL,
        "sig": GOLDEN_SIG_B64URL,
        "pubkey_fingerprint": GOLDEN_PUBKEY_FINGERPRINT,
    })
    .to_string();
    let pubkey = hex_to_bytes(GOLDEN_PUBKEY_HEX);
    let verified = verify(
        envelope.as_bytes(),
        &trust_set(GOLDEN_PUBKEY_FINGERPRINT, &pubkey),
    )
    .unwrap();
    assert_eq!(payload_json(&verified), golden_payload());
    assert_eq!(verified.pubkey_fingerprint, GOLDEN_PUBKEY_FINGERPRINT);
}

// ---------------------------------------------------------------------------
// verifyBundle
// ---------------------------------------------------------------------------

#[test]
fn roundtrips_a_freshly_signed_bundle() {
    let signed = sign_canonical(&golden_payload(), &golden_seed());
    let verified = verify(&signed.envelope, &signed.trust()).unwrap();
    assert_eq!(payload_json(&verified), golden_payload());
}

#[test]
fn rejects_a_tampered_payload_byte_as_bad_signature() {
    let signed = sign_canonical(&golden_payload(), &golden_seed());
    let mut parsed: Value = serde_json::from_slice(&signed.envelope).unwrap();
    let mut decoded = b64url_decode(parsed["payload"].as_str().unwrap());
    // Flip one byte of the canonical payload — Ed25519 catches this regardless
    // of where the flip lands.
    decoded[10] ^= 0x01;
    parsed["payload"] = json!(b64url(&decoded));
    let tampered = parsed.to_string();
    assert_eq!(
        verify(tampered.as_bytes(), &signed.trust()).unwrap_err(),
        BundleVerifyError::BadSignature
    );
}

#[test]
fn rejects_an_untrusted_signer() {
    let signed = sign_canonical(&golden_payload(), &golden_seed());
    let seed_b = SigningKey::from_bytes(
        &hex_to_bytes("1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100")
            .try_into()
            .unwrap(),
    );
    let pub_b = seed_b.verifying_key().to_bytes();
    let fp_b = fingerprint_pubkey(&pub_b);
    assert_eq!(
        verify(&signed.envelope, &trust_set(&fp_b, &pub_b)).unwrap_err(),
        BundleVerifyError::UntrustedSigner
    );
}

#[test]
fn rejects_an_envelope_missing_the_sig_field() {
    let envelope = json!({
        "payload": GOLDEN_PAYLOAD_B64URL,
        "pubkey_fingerprint": GOLDEN_PUBKEY_FINGERPRINT,
    })
    .to_string();
    let pubkey = hex_to_bytes(GOLDEN_PUBKEY_HEX);
    assert_eq!(
        verify(
            envelope.as_bytes(),
            &trust_set(GOLDEN_PUBKEY_FINGERPRINT, &pubkey)
        )
        .unwrap_err(),
        BundleVerifyError::MalformedEnvelope
    );
}

#[test]
fn rejects_an_envelope_that_isnt_json() {
    let pubkey = hex_to_bytes(GOLDEN_PUBKEY_HEX);
    assert_eq!(
        verify(b"not json", &trust_set(GOLDEN_PUBKEY_FINGERPRINT, &pubkey)).unwrap_err(),
        BundleVerifyError::MalformedEnvelope
    );
}

#[test]
fn rejects_schema_mismatch_version_2() {
    let mut bad = golden_payload();
    bad["version"] = json!(2);
    let signed = sign_canonical(&bad, &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_schema_mismatch_extra_unknown_field() {
    let mut extra = golden_payload();
    extra["surprise"] = json!("hi");
    let signed = sign_canonical(&extra, &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn treats_clock_rollback_not_before_far_in_future_as_bad_signature() {
    let future = now_secs() + 60 * 60 * 24 * 365; // +1 year
    let mut rolled = golden_payload();
    rolled["issued_at"] = json!(future);
    rolled["not_before"] = json!(future);
    rolled["not_after"] = json!(future + 60);
    let signed = sign_canonical(&rolled, &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::BadSignature
    );
}

#[test]
fn clock_tolerance_is_one_hour_either_side_of_the_boundary() {
    // Not in the TypeScript suite: pins the tolerance at its boundary.
    assert_eq!(CLOCK_TOLERANCE_SECONDS, 3600);
    let now = 1_800_000_000;
    for (not_before, ok) in [(now + 3600, true), (now + 3601, false)] {
        let mut p = golden_payload();
        p["issued_at"] = json!(not_before);
        p["not_before"] = json!(not_before);
        p["not_after"] = json!(not_before + 60);
        let signed = sign_canonical(&p, &golden_seed());
        let result = verify_bundle(&signed.envelope, &signed.trust(), now);
        assert_eq!(
            result.is_ok(),
            ok,
            "not_before = now + {}",
            not_before - now
        );
    }
}

#[test]
fn does_not_throw_on_expired_bundles() {
    let mut past = golden_payload();
    past["issued_at"] = json!(1600000000);
    past["not_before"] = json!(1599999940);
    past["not_after"] = json!(1600000060); // already expired
    let signed = sign_canonical(&past, &golden_seed());
    let verified = verify(&signed.envelope, &signed.trust()).unwrap();
    assert_eq!(verified.payload.not_after, 1600000060.0);
    assert!(is_expired(&verified, now_secs()));
}

#[test]
fn rejects_non_canonical_envelopes() {
    // Payload bytes that are NOT canonical (keys out of order), signed; the
    // raw Ed25519 check passes but re-canonicalisation must reject them.
    let non_canonical = r#"{"version":1,"issued_at":1700000000,"not_before":1699999940,"not_after":1702592000,"subscription_id":"sub_test_0001","tenant_slug":"acme","tier":"team","seats":3,"seat_tokens":[{"key":"owner_key_abc","role":"owner"},{"key":"member_key_xyz","role":"member"}],"revoked_keys":[],"issued_by_install_id":null}"#;
    let signed = common::sign_bytes(non_canonical.as_bytes(), &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::BadSignature
    );
}

#[test]
fn rejects_payload_bytes_encoding_seats_3_0_as_bad_signature() {
    // `"seats":3.0` parses as the integer 3 (as JSON.parse does), so the
    // schema accepts it and re-canonicalisation catches the byte drift.
    let canonical_text = String::from_utf8(canon(&golden_payload())).unwrap();
    let drifted = canonical_text.replace(r#""seats":3,"#, r#""seats":3.0,"#);
    assert_ne!(drifted, canonical_text);
    let signed = common::sign_bytes(drifted.as_bytes(), &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::BadSignature
    );
}

// ---------------------------------------------------------------------------
// BundleVerifyError
// ---------------------------------------------------------------------------

#[test]
fn bundle_verify_error_has_no_expired_variant() {
    let all = [
        BundleVerifyError::MalformedEnvelope,
        BundleVerifyError::UntrustedSigner,
        BundleVerifyError::BadSignature,
        BundleVerifyError::SchemaMismatch,
    ];
    // Exhaustive: a new variant fails to compile here until it's listed.
    for e in all {
        match e {
            BundleVerifyError::MalformedEnvelope
            | BundleVerifyError::UntrustedSigner
            | BundleVerifyError::BadSignature
            | BundleVerifyError::SchemaMismatch => {}
        }
        assert_ne!(e.as_str(), "expired");
    }
    assert_eq!(
        all.map(|e| e.as_str()),
        [
            "malformed_envelope",
            "untrusted_signer",
            "bad_signature",
            "schema_mismatch"
        ]
    );
}

#[test]
fn verify_rs_source_never_names_expired_as_an_error() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/server/airgap/verify.rs"
    ))
    .unwrap();
    assert!(!src.contains("\"expired\""));
}

// ---------------------------------------------------------------------------
// parsePayload numeric bounds
// ---------------------------------------------------------------------------

fn with_overrides(overrides: Value) -> Result<Value, BundleVerifyError> {
    let mut p = golden_payload();
    for (k, v) in overrides.as_object().unwrap() {
        p[k] = v.clone();
    }
    let signed = sign_canonical(&p, &golden_seed());
    verify(&signed.envelope, &signed.trust()).map(|v| payload_json(&v))
}

#[test]
fn rejects_negative_seats() {
    assert_eq!(
        with_overrides(json!({ "seats": -1 })).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_seats_above_the_10000_ceiling() {
    assert_eq!(
        with_overrides(json!({ "seats": 10001 })).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn accepts_the_seats_ceiling_exactly() {
    assert_eq!(
        with_overrides(json!({ "seats": 10000 })).unwrap()["seats"],
        json!(10000)
    );
}

#[test]
fn rejects_negative_issued_at() {
    assert_eq!(
        with_overrides(json!({ "issued_at": -1, "not_before": -61, "not_after": 1702592000 }))
            .unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_negative_not_before() {
    assert_eq!(
        with_overrides(
            json!({ "issued_at": 1700000000, "not_before": -1, "not_after": 1702592000 })
        )
        .unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_negative_not_after() {
    assert_eq!(
        with_overrides(
            json!({ "issued_at": 1700000000, "not_before": 1699999940, "not_after": -1 })
        )
        .unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_not_after_before_not_before() {
    assert_eq!(
        with_overrides(
            json!({ "issued_at": 1700000000, "not_before": 1700000000, "not_after": 1699999999 })
        )
        .unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

#[test]
fn rejects_issued_at_after_not_after() {
    assert_eq!(
        with_overrides(
            json!({ "issued_at": 1800000000, "not_before": 1699999940, "not_after": 1702592000 })
        )
        .unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}

// ---------------------------------------------------------------------------
// seat_tokens edge cases
// ---------------------------------------------------------------------------

#[test]
fn round_trips_an_empty_seat_tokens_array_with_seats_0() {
    let mut empty = golden_payload();
    empty["seats"] = json!(0);
    empty["seat_tokens"] = json!([]);
    let text = String::from_utf8(canon(&empty)).unwrap();
    assert!(text.contains(r#""seat_tokens":[]"#));
    assert!(!text.contains(r#""seat_tokens":null"#));
    let signed = sign_canonical(&empty, &golden_seed());
    let verified = verify(&signed.envelope, &signed.trust()).unwrap();
    assert_eq!(verified.payload.seats, 0.0);
    assert!(verified.payload.seat_tokens.is_empty());
}

// ---------------------------------------------------------------------------
// JavaScript parsing rules the verifier keeps (not in the TypeScript suite)
// ---------------------------------------------------------------------------

#[test]
fn a_leading_bom_on_the_envelope_is_ignored_like_text_decoder_does() {
    let signed = sign_canonical(&golden_payload(), &golden_seed());
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice(&signed.envelope);
    assert!(verify(&with_bom, &signed.trust()).is_ok());
}

#[test]
fn envelope_errors_keep_their_codes() {
    let signed = sign_canonical(&golden_payload(), &golden_seed());
    let trust = signed.trust();
    let env: Value = serde_json::from_slice(&signed.envelope).unwrap();
    let with = |k: &str, v: Value| {
        let mut e = env.clone();
        e[k] = v;
        e.to_string()
    };
    for (body, code) in [
        ("[]".to_string(), BundleVerifyError::MalformedEnvelope),
        ("null".to_string(), BundleVerifyError::MalformedEnvelope),
        (with("sig", json!(5)), BundleVerifyError::MalformedEnvelope),
        // base64url only: `+` and `=` are refused, not decoded.
        (
            with("sig", json!("ab+c")),
            BundleVerifyError::MalformedEnvelope,
        ),
        (
            with("payload", json!("ab==")),
            BundleVerifyError::MalformedEnvelope,
        ),
        // A length of 4n + 1 can't be base64 (atob throws).
        (
            with("payload", json!("abcde")),
            BundleVerifyError::MalformedEnvelope,
        ),
        // A short signature fails verification, as noble's throw did.
        (with("sig", json!("AA")), BundleVerifyError::BadSignature),
        (
            with("pubkey_fingerprint", json!("00")),
            BundleVerifyError::UntrustedSigner,
        ),
    ] {
        assert_eq!(verify(body.as_bytes(), &trust).unwrap_err(), code, "{body}");
    }
    // Invalid UTF-8 is malformed.
    assert_eq!(
        verify(&[0xFF, 0xFE], &trust).unwrap_err(),
        BundleVerifyError::MalformedEnvelope
    );
}

#[test]
fn schema_rules_match_parse_payload() {
    for (patch, ok) in [
        (json!({ "version": "1" }), false),
        (json!({ "version": 1.0 }), true),
        (json!({ "tier": 5 }), false),
        (json!({ "issued_by_install_id": "inst_1" }), true),
        (json!({ "issued_by_install_id": 5 }), false),
        (json!({ "revoked_keys": [5] }), false),
        (json!({ "revoked_keys": "k" }), false),
        (
            json!({ "seat_tokens": [{ "key": "k", "role": "admin" }] }),
            false,
        ),
        (json!({ "seat_tokens": [{ "key": "k" }] }), false),
        (
            json!({ "seat_tokens": [{ "key": "k", "role": "member", "x": 1 }] }),
            false,
        ),
        (
            json!({ "seat_tokens": [{ "key": 1, "role": "member" }] }),
            false,
        ),
        (json!({ "seat_tokens": ["k"] }), false),
    ] {
        let result = with_overrides(patch.clone());
        assert_eq!(result.is_ok(), ok, "{patch}");
        if !ok {
            assert_eq!(
                result.unwrap_err(),
                BundleVerifyError::SchemaMismatch,
                "{patch}"
            );
        }
    }
    // A fractional number isn't canonical JSON at all (canonicalize refuses
    // it, as `canonical.ts` threw), so sign the text directly.
    assert!(canonicalize(&CanonicalValue::from(&json!({ "seats": 2.5 }))).is_err());
    let text = String::from_utf8(canon(&golden_payload()))
        .unwrap()
        .replace(r#""seats":3,"#, r#""seats":2.5,"#);
    let signed = common::sign_bytes(text.as_bytes(), &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );

    // A missing field is a mismatch too.
    let mut p = golden_payload();
    p.as_object_mut().unwrap().remove("issued_by_install_id");
    let signed = sign_canonical(&p, &golden_seed());
    assert_eq!(
        verify(&signed.envelope, &signed.trust()).unwrap_err(),
        BundleVerifyError::SchemaMismatch
    );
}
