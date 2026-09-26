//! Air-gap local control, ported 1:1 from `airgap/local-control.test.ts`
//! over a temp auth.db built from migrations 006–012.

mod common;

use common::{bundle_payload, exec, seed_user, Env};
use seaquel_license::server::airgap::local_control::{
    mask_key, project_tenant_context, synthetic_tenant_member_id, AIRGAP_GRACE_SECONDS,
};
use seaquel_license::server::airgap::verify::verify_bundle;
use seaquel_license::server::{BindArgs, Role, ServerErrorCode};
use serde_json::{json, Value};
use sqlx::SqliteConnection;

const NOW: i64 = 1_750_000_000;

/// Import a bundle the way the TS helper did: issued at 1,700,000,000,
/// `not_after` a year from now unless overridden.
async fn import_bundle(env: &Env, overrides: Value) {
    let mut payload = bundle_payload(env.clock.now() + 60 * 60 * 24 * 365);
    for (k, v) in overrides.as_object().unwrap() {
        payload[k] = v.clone();
    }
    let signed = env.sign(&payload);
    let verified = verify_bundle(&signed.envelope, &signed.trust(), env.clock.now()).unwrap();
    env.server
        .write_bundle(&verified, &signed.payload_sha256())
        .await
        .unwrap();
}

async fn bind(node: &mut SqliteConnection, user: &str, key: &str, is_owner: bool) {
    sqlx::query(
        "INSERT INTO member_license (user_id, license_key, bound_at, control_member_id, is_owner)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(key)
    .bind(NOW)
    .bind(synthetic_tenant_member_id(key))
    .bind(is_owner as i64)
    .execute(node)
    .await
    .unwrap();
}

// ── registerInstallLocal ──

#[tokio::test]
async fn register_install_local_fails_no_airgap_bundle_without_a_bundle() {
    let env = Env::new(NOW).await;
    let err = env
        .server
        .register_install_local("owner_key_abc")
        .await
        .unwrap_err();
    assert_eq!(err.code, ServerErrorCode::NoAirgapBundle);
    assert_eq!(err.message, "no_airgap_bundle");
}

#[tokio::test]
async fn register_install_local_fails_license_not_found_for_a_non_owner_key() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    for key in ["member_key_xyz", "unknown"] {
        let err = env.server.register_install_local(key).await.unwrap_err();
        assert_eq!(err.code, ServerErrorCode::LicenseNotFound);
        assert_eq!(err.message, "license_not_found");
    }
}

#[tokio::test]
async fn register_install_local_projects_the_bundle_and_writes_the_install_cache() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    let ctx = env
        .server
        .register_install_local("owner_key_abc")
        .await
        .unwrap();
    assert_eq!(ctx.tenant_id, "airgap-sub_test_0001");
    assert_eq!(ctx.slug, "acme");
    assert_eq!(ctx.tier, "team");
    assert_eq!(ctx.seat_limit, 3);
    assert_eq!(ctx.status, "active");
    assert_eq!(ctx.subscription_id, "sub_test_0001");

    let cache = env.server.read_install_cache().await.unwrap().unwrap();
    assert_eq!(cache.mode.as_str(), "airgap");
}

// ── localTenantInfo ──

#[tokio::test]
async fn local_tenant_info_is_none_without_a_bundle() {
    let env = Env::new(NOW).await;
    assert!(env.server.local_tenant_info().await.unwrap().is_none());
}

#[tokio::test]
async fn local_tenant_info_is_none_past_not_after() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({ "not_after": NOW - 60 })).await;
    assert!(env.server.local_tenant_info().await.unwrap().is_none());
}

#[tokio::test]
async fn local_tenant_info_projects_the_bundle_and_does_not_write_install_cache() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    let ctx = env.server.local_tenant_info().await.unwrap().unwrap();
    assert_eq!(ctx.tenant_id, "airgap-sub_test_0001");
    assert!(env.server.read_install_cache().await.unwrap().is_none());
}

// ── verifyLocalMembershipLicense ──

async fn verify_local(env: &Env, key: &str) -> Value {
    let raw = env
        .server
        .verify_local_membership_license(key, "user@example.com")
        .await
        .unwrap();
    serde_json::from_str(raw.get()).unwrap()
}

#[tokio::test]
async fn verify_local_is_license_not_found_without_a_bundle() {
    let env = Env::new(NOW).await;
    assert_eq!(
        verify_local(&env, "anything").await,
        json!({ "ok": false, "error": "license_not_found" })
    );
}

#[tokio::test]
async fn verify_local_is_license_not_found_for_a_key_not_in_the_bundle() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    assert_eq!(
        verify_local(&env, "missing").await,
        json!({ "ok": false, "error": "license_not_found" })
    );
}

#[tokio::test]
async fn verify_local_is_license_inactive_for_a_revoked_key() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({ "revoked_keys": ["member_key_xyz"] })).await;
    assert_eq!(
        verify_local(&env, "member_key_xyz").await,
        json!({ "ok": false, "error": "license_inactive" })
    );
}

#[tokio::test]
async fn verify_local_rejects_synthetic_vacant_placeholders() {
    let env = Env::new(NOW).await;
    import_bundle(
        &env,
        json!({ "seat_tokens": [
            { "key": "owner_key_abc", "role": "owner" },
            { "key": "airgap-vacant-acme-1", "role": "member" },
        ] }),
    )
    .await;
    assert_eq!(
        verify_local(&env, "airgap-vacant-acme-1").await,
        json!({ "ok": false, "error": "license_not_found" })
    );
}

#[tokio::test]
async fn verify_local_is_license_already_in_other_tenant_when_bound_locally() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    let mut node = env.node().await;
    seed_user(&mut node, "u_existing", "existing@example.com").await;
    bind(&mut node, "u_existing", "member_key_xyz", false).await;
    assert_eq!(
        env.server
            .verify_local_membership_license("member_key_xyz", "new@example.com")
            .await
            .unwrap()
            .get(),
        r#"{"ok":false,"error":"license_already_in_other_tenant"}"#
    );
}

#[tokio::test]
async fn verify_local_is_ok_with_subscription_tier_and_role_for_an_unbound_seat() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    // Key order as the TS object literal had it.
    let member = env
        .server
        .verify_local_membership_license("member_key_xyz", "e")
        .await
        .unwrap();
    assert_eq!(
        member.get(),
        r#"{"ok":true,"subscriptionId":"sub_test_0001","tier":"team","role":"member"}"#
    );
    assert_eq!(
        verify_local(&env, "owner_key_abc").await,
        json!({ "ok": true, "subscriptionId": "sub_test_0001", "tier": "team", "role": "owner" })
    );
}

// ── bindLocal / unbindLocal ──

#[test]
fn bind_local_returns_a_deterministic_member_id_from_the_license_key() {
    let a = synthetic_tenant_member_id("owner_key_abc");
    assert_eq!(a, synthetic_tenant_member_id("owner_key_abc"));
    assert!(
        a.len() == 22
            && a.starts_with("local-")
            && a[6..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{a}"
    );
    assert_ne!(synthetic_tenant_member_id("different_key"), a);
}

#[tokio::test]
async fn bind_local_uses_the_synthetic_id_whatever_the_other_args() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    let args = |user: &str, role| BindArgs {
        license_key: "owner_key_abc".into(),
        container_user_id: user.into(),
        email: format!("{user}@example.com"),
        role,
        auth_key: None,
    };
    let a = env
        .server
        .bind_member_dispatch(&args("u_alpha", Role::Owner))
        .await
        .unwrap();
    let b = env
        .server
        .bind_member_dispatch(&args("u_beta", Role::Member))
        .await
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(a, synthetic_tenant_member_id("owner_key_abc"));
}

#[tokio::test]
async fn unbind_local_is_a_no_op() {
    let env = Env::new(NOW).await;
    import_bundle(&env, json!({})).await;
    env.server
        .unbind_member_dispatch("any-user-id")
        .await
        .unwrap();
    assert!(env.fake.seen().is_empty());
}

// ── listLocalMembers ──

#[tokio::test]
async fn list_local_members_is_empty_when_no_one_is_bound() {
    let env = Env::new(NOW).await;
    assert_eq!(env.server.list_local_members().await.unwrap().get(), "[]");
}

#[tokio::test]
async fn list_local_members_masks_keys_and_omits_revoked_rows() {
    let env = Env::new(NOW).await;
    let mut node = env.node().await;
    seed_user(&mut node, "u_owner", "owner@example.com").await;
    seed_user(&mut node, "u_member", "member@example.com").await;
    seed_user(&mut node, "u_revoked", "revoked@example.com").await;
    bind(&mut node, "u_owner", "owner_key_abc", true).await;
    bind(&mut node, "u_member", "member_key_xyz", false).await;
    bind(&mut node, "u_revoked", "old_key_zzzz", false).await;
    exec(
        &mut node,
        &format!("UPDATE member_license SET revoked_at = {NOW} WHERE user_id = 'u_revoked'"),
    )
    .await;

    let raw = env.server.list_local_members().await.unwrap();
    let members: Vec<Value> = serde_json::from_str(raw.get()).unwrap();
    assert_eq!(members.len(), 2);
    let find = |id: &str| {
        members
            .iter()
            .find(|m| m["containerUserId"] == id)
            .unwrap()
            .clone()
    };
    let owner = find("u_owner");
    assert_eq!(owner["role"], "owner");
    assert_eq!(owner["email"], "owner@example.com");
    assert_eq!(owner["maskedLicenseKey"], "••••-_abc");
    let member = find("u_member");
    assert_eq!(member["role"], "member");
    assert_eq!(member["maskedLicenseKey"], "••••-_xyz");
    assert_eq!(
        owner["tenantMemberId"],
        json!(synthetic_tenant_member_id("owner_key_abc"))
    );
    // The whole row, in the TS key order, with `boundAt` as toISOString().
    assert!(raw.get().contains(&format!(
            r#"{{"tenantMemberId":"{}","containerUserId":"u_owner","email":"owner@example.com","role":"owner","boundAt":"2025-06-15T15:06:40.000Z","maskedLicenseKey":"••••-_abc"}}"#,
            synthetic_tenant_member_id("owner_key_abc")
        )));
    let _ = owner;
}

// ── _internal helpers ──

#[test]
fn synthetic_tenant_member_id_is_stable_per_key() {
    assert_eq!(
        synthetic_tenant_member_id("k1"),
        synthetic_tenant_member_id("k1")
    );
    assert_ne!(
        synthetic_tenant_member_id("k1"),
        synthetic_tenant_member_id("k2")
    );
}

#[test]
fn mask_key_returns_last_4_with_bullet_prefix() {
    assert_eq!(mask_key(""), "");
    assert_eq!(mask_key("abcd"), "abcd"); // ≤ 4 chars: not masked
    assert_eq!(mask_key("abcdef"), "••••-cdef");
    // UTF-16 lengths, as `key.slice(-4)` counted them.
    assert_eq!(mask_key("ab😀"), "ab😀");
    assert_eq!(mask_key("abc😀"), "••••-bc😀");
}

#[test]
fn project_tenant_context_recovers_the_period_end() {
    let payload = verify_bundle(
        &common::sign_canonical(&bundle_payload(1_702_592_000), &common::test_seed()).envelope,
        &common::sign_canonical(&bundle_payload(1_702_592_000), &common::test_seed()).trust(),
        1_700_000_000,
    )
    .unwrap()
    .payload;
    let ctx = project_tenant_context(&payload);
    assert_eq!(AIRGAP_GRACE_SECONDS, 2_592_000);
    assert_eq!(
        ctx.current_period_end.as_deref(),
        Some("2023-11-14T22:13:20.000Z")
    );
    assert_eq!(
        serde_json::to_string(&ctx).unwrap(),
        r#"{"tenantId":"airgap-sub_test_0001","slug":"acme","status":"active","publicUrl":"","anchorLicenseId":"","subscriptionId":"sub_test_0001","tier":"team","ownerEmail":"","seatLimit":3,"currentPeriodEnd":"2023-11-14T22:13:20.000Z"}"#
    );
    // A period end at or before the epoch is null.
    let early = verify_bundle(
        &common::sign_canonical(&json!({"version":1,"issued_at":0,"not_before":0,"not_after":2_592_000,"subscription_id":"s","tenant_slug":"t","tier":"x","seats":1,"seat_tokens":[],"revoked_keys":[],"issued_by_install_id":null}), &common::test_seed()).envelope,
        &common::sign_canonical(&bundle_payload(0), &common::test_seed()).trust(),
        0,
    )
    .unwrap()
    .payload;
    assert_eq!(project_tenant_context(&early).current_period_end, None);
}
