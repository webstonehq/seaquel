//! Bundle store and trusted-key loader, ported 1:1 from
//! `airgap/bundle-store.test.ts` over a temp auth.db built from migrations
//! 006–012.

mod common;

use std::sync::Arc;

use common::{bundle_payload, exec, hex, Env};
use seaquel_license::server::airgap::bundle_store::{load_trusted_pubkeys, PROD_TRUSTED_PUBKEYS};
use seaquel_license::server::airgap::canonical::fingerprint_pubkey;
use seaquel_license::server::airgap::verify::verify_bundle;
use seaquel_license::server::airgap::TrustSet;
use serde_json::json;

const NOW: i64 = 1_750_000_000;

/// The TS helper's bundle: issued at 1,700,000,000, valid for a year.
fn envelope(env: &Env) -> common::Signed {
    env.sign(&bundle_payload(1_700_000_000 + 60 * 60 * 24 * 365))
}

async fn write(env: &Env, signed: &common::Signed) {
    let verified = verify_bundle(&signed.envelope, &signed.trust(), 1_700_000_000).unwrap();
    env.server
        .write_bundle(&verified, &signed.payload_sha256())
        .await
        .unwrap();
}

#[tokio::test]
async fn write_bundle_then_read_active_bundle_returns_the_same_payload() {
    let env = Env::new(NOW).await;
    let signed = envelope(&env);
    write(&env, &signed).await;

    let active = env.server.read_active_bundle().await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&active.payload).unwrap(),
        bundle_payload(1_700_000_000 + 60 * 60 * 24 * 365)
    );
    assert_eq!(active.pubkey_fingerprint, signed.fingerprint);
    assert_eq!(active.payload_sha256, signed.payload_sha256());
    assert_eq!(active.raw_envelope, signed.envelope);
    assert_eq!(active.imported_at, NOW);
}

#[tokio::test]
async fn clear_bundle_makes_read_active_bundle_return_none() {
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    assert!(env.server.read_active_bundle().await.unwrap().is_some());

    env.server.clear_bundle().await.unwrap();
    assert!(env.server.read_active_bundle().await.unwrap().is_none());
}

#[tokio::test]
async fn re_verifies_on_each_read_corrupting_raw_envelope_returns_none() {
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    assert!(env.server.read_active_bundle().await.unwrap().is_some());

    // A fresh server has no in-process cache, so the next read hits the DB
    // (the TS test called `_resetBundleStoreCache()`).
    let server = env.server_with(|c| c);
    let mut node = env.node().await;
    exec(
        &mut node,
        r#"UPDATE airgap_bundle SET raw_envelope = CAST('{"payload":"AA","sig":"AA","pubkey_fingerprint":"deadbeef"}' AS BLOB) WHERE id = 1"#,
    )
    .await;
    assert!(server.read_active_bundle().await.unwrap().is_none());
}

#[tokio::test]
async fn returns_none_when_the_trust_anchor_is_no_longer_in_the_trust_set() {
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    assert!(env.server.read_active_bundle().await.unwrap().is_some());

    // The production set only, as with the env override removed.
    let server = env.server_with(|c| c.with_trusted(load_trusted_pubkeys(None)));
    assert!(server.read_active_bundle().await.unwrap().is_none());
}

#[tokio::test]
async fn is_bundle_driven_mirrors_read_active_bundle_in_the_steady_state() {
    let env = Env::new(NOW).await;
    assert!(!env.server.is_bundle_driven().await.unwrap());
    assert!(env.server.read_active_bundle().await.unwrap().is_none());

    write(&env, &envelope(&env)).await;
    assert!(env.server.is_bundle_driven().await.unwrap());
    assert!(env.server.read_active_bundle().await.unwrap().is_some());

    env.server.clear_bundle().await.unwrap();
    assert!(!env.server.is_bundle_driven().await.unwrap());
    assert!(env.server.read_active_bundle().await.unwrap().is_none());
}

#[tokio::test]
async fn caches_the_parsed_bundle_within_a_single_payload_sha256() {
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    let a = env.server.read_active_bundle().await.unwrap().unwrap();
    let b = env.server.read_active_bundle().await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&a, &b), "served from cache");
}

#[tokio::test]
async fn the_stored_row_matches_what_node_wrote() {
    // Not in the TS suite: the columns `writeBundle` filled, so either side
    // can read the other's row.
    let env = Env::new(NOW).await;
    let signed = envelope(&env);
    write(&env, &signed).await;
    let mut node = env.node().await;
    let row: (Vec<u8>, String, String, i64, i64, String, i64) = sqlx::query_as(
        "SELECT raw_envelope, verified_payload, pubkey_fingerprint, imported_at, not_after,
                payload_sha256, issued_at FROM airgap_bundle WHERE id = 1",
    )
    .fetch_one(&mut node)
    .await
    .unwrap();
    assert_eq!(row.0, signed.envelope);
    // `JSON.stringify(payload)` in parsePayload's key order.
    assert_eq!(
        row.1,
        r#"{"version":1,"issued_at":1700000000,"not_before":1699999940,"not_after":1731536000,"subscription_id":"sub_test_0001","tenant_slug":"acme","tier":"team","seats":3,"seat_tokens":[{"key":"owner_key_abc","role":"owner"},{"key":"member_key_xyz","role":"member"}],"revoked_keys":[],"issued_by_install_id":null}"#
    );
    assert_eq!(row.2, signed.fingerprint);
    assert_eq!(row.3, NOW);
    assert_eq!(row.4, 1_731_536_000);
    assert_eq!(row.5, signed.payload_sha256());
    assert_eq!(row.6, 1_700_000_000);
}

// ── loadTrustedPubkeys ──

fn prod_trust() -> TrustSet {
    load_trusted_pubkeys(None)
}

#[test]
fn parses_a_single_trusted_pubkey_entry() {
    let base = prod_trust().len();
    let trust = load_trusted_pubkeys(Some(&format!("aa11:{}", "00".repeat(32))));
    assert_eq!(trust.len(), base + 1);
    assert_eq!(trust.get("aa11").unwrap(), &vec![0u8; 32]);
}

#[test]
fn parses_multiple_comma_separated_entries() {
    let base = prod_trust().len();
    let spec = format!("aa11:{},bb22:{}", "01".repeat(32), "02".repeat(32));
    let trust = load_trusted_pubkeys(Some(&spec));
    assert_eq!(trust.len(), base + 2);
    assert_eq!(trust.get("aa11").unwrap(), &vec![1u8; 32]);
    assert_eq!(trust.get("bb22").unwrap(), &vec![2u8; 32]);
}

#[test]
fn ignores_malformed_entries_but_keeps_valid_ones() {
    let good = format!("aa11:{}", "03".repeat(32));
    let trust = load_trusted_pubkeys(Some(&format!(
        "no-colon-here, {good}, :only-value, only-key:, bb22:xyz"
    )));
    assert!(trust.contains_key("aa11"));
    assert!(!trust.contains_key("only-value"));
    assert!(!trust.contains_key("only-key"));
    assert!(!trust.contains_key("bb22"));
}

#[test]
fn merges_the_env_override_on_top_of_the_built_in_production_set() {
    let prod = prod_trust();
    let trust = load_trusted_pubkeys(Some(&format!("aa11:{}", "00".repeat(32))));
    for (fingerprint, pubkey) in &prod {
        assert_eq!(trust.get(fingerprint), Some(pubkey));
    }
    assert!(trust.contains_key("aa11"));
    assert!(!load_trusted_pubkeys(None).contains_key("aa11"));
}

#[test]
fn ships_a_well_formed_built_in_production_set() {
    assert!(!PROD_TRUSTED_PUBKEYS.is_empty());
    for (fingerprint, pubkey) in prod_trust() {
        assert_eq!(pubkey.len(), 32);
        assert!(
            fingerprint.len() == 32
                && fingerprint
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{fingerprint}"
        );
        // A fingerprint that isn't SHA-256(pubkey)[0..16] can never be
        // looked up.
        assert_eq!(fingerprint, fingerprint_pubkey(&pubkey));
    }
    // The anchor the TypeScript shipped, unchanged.
    assert_eq!(
        hex(prod_trust()
            .get("937769290e77859410e977c3f762aa0c")
            .unwrap()),
        "dab5fab2c0c17da9a5c0f3ce2a710a496ff3db92578e09b2d786f44e99411a5e"
    );
}

#[test]
fn hex_is_parsed_the_way_js_parse_int_read_it() {
    // Not in the TS suite. `hexToBytes` used `parseInt(pair, 16)` per byte,
    // which reads a leading digit and stops ("1z" is 1), takes a sign ("-1"
    // stored into a Uint8Array is 255) and strips a `0x` prefix.
    let trust = load_trusted_pubkeys(Some("aa:0x1z-1ff"));
    assert_eq!(trust.get("aa").unwrap(), &vec![1u8, 255, 255]);
    // A pair with no digits ("0x" alone, "g1") refuses the entry.
    assert!(!load_trusted_pubkeys(Some("aa:g1")).contains_key("aa"));
    assert!(!load_trusted_pubkeys(Some("aa:0x0x")).contains_key("aa"));
    // Whitespace around each part is trimmed; empty entries are skipped.
    let t = load_trusted_pubkeys(Some(" , bb : 0102 ,"));
    assert_eq!(t.get("bb").unwrap(), &vec![1u8, 2]);
}

#[tokio::test]
async fn a_bundle_signed_by_an_untrusted_key_is_never_active() {
    // Written by a server that trusted the key, read by one that doesn't.
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    let other = env.server_with(|c| {
        c.with_trusted(load_trusted_pubkeys(Some(&format!(
            "aa11:{}",
            "00".repeat(32)
        ))))
    });
    assert!(other.read_active_bundle().await.unwrap().is_none());
    // Presence alone still counts for the dispatcher, as in TS.
    assert!(other.is_bundle_driven().await.unwrap());
    let _ = json!(null);
}

#[tokio::test]
async fn a_payload_sha256_that_doesnt_match_the_payload_reads_as_no_bundle() {
    let env = Env::new(NOW).await;
    write(&env, &envelope(&env)).await;
    assert!(env.server.read_active_bundle().await.unwrap().is_some());
    let mut node = env.node().await;
    exec(
        &mut node,
        "UPDATE airgap_bundle SET payload_sha256 = 'deadbeef' WHERE id = 1",
    )
    .await;
    // Even the server that cached the bundle: the cache is keyed by the
    // row's hash, so the edited row misses it and re-verifies.
    assert!(env.server.read_active_bundle().await.unwrap().is_none());
    assert!(env
        .server_with(|c| c)
        .read_active_bundle()
        .await
        .unwrap()
        .is_none());
}
