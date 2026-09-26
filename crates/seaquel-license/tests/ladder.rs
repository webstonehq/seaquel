//! The license gate's TTL ladder (`resolveLicenseState`), which no
//! TypeScript test covered: unregistered, soft-fresh without a network call,
//! stale then success, stale then failure within grace, stale then failure
//! after grace, and suspended. Plus the gate answer the Node hooks read.

mod common;

use common::{bundle_payload, closed_url, exec, seed_user, Env};
use seaquel_license::server::airgap::verify::verify_bundle;
use seaquel_license::server::{
    parse_env_seconds, GateState, Mode, ServerConfig, TenantFields, DEFAULT_GRACE_TTL_SECONDS,
    DEFAULT_SOFT_TTL_SECONDS,
};
use serde_json::{json, Value};

const NOW: i64 = 1_750_000_000;
const DAY: i64 = 86_400;

fn tenant(status: &str) -> TenantFields {
    TenantFields {
        tenant_id: Some("tenant_abc".into()),
        slug: Some("acme".into()),
        status: Some(status.into()),
        tier: Some("team".into()),
        seat_limit: Some(5),
        current_period_end: Some("2027-01-01T00:00:00.000Z".into()),
    }
}

fn control_tenant(status: &str, slug: &str) -> Value {
    json!({
        "tenantId": "tenant_abc", "slug": slug, "status": status,
        "publicUrl": "https://example.test", "anchorLicenseId": "lic", "subscriptionId": "sub",
        "tier": "business", "ownerEmail": "o@example.test", "seatLimit": 9,
        "currentPeriodEnd": null,
    })
}

async fn registered(env: &Env, status: &str) {
    env.server
        .write_install_cache(&tenant(status), Mode::Online)
        .await
        .unwrap();
    let mut node = env.node().await;
    seed_user(&mut node, "u_owner", "owner@example.test").await;
    env.server
        .insert_member("u_owner", "OWNER-KEY", NOW, "tm_owner", true)
        .await
        .unwrap();
}

#[test]
fn defaults_are_24_hours_and_14_days() {
    assert_eq!(DEFAULT_SOFT_TTL_SECONDS, 24 * 60 * 60);
    assert_eq!(DEFAULT_GRACE_TTL_SECONDS, 14 * 24 * 60 * 60);
    let c = ServerConfig::new("auth.db");
    assert_eq!((c.soft_ttl, c.grace_ttl), (DAY, 14 * DAY));
}

#[test]
fn env_overrides_read_like_parse_int() {
    // parseInt(raw, 10), used when finite and > 0; else the default.
    for (raw, want) in [
        (None, 7),
        (Some(""), 7),
        (Some("60"), 60),
        (Some(" 12abc"), 12),
        (Some("1e3"), 1),
        (Some("+5"), 5),
        (Some("0x10"), 7),
        (Some("0"), 7),
        (Some("-5"), 7),
        (Some("abc"), 7),
    ] {
        assert_eq!(parse_env_seconds(raw, 7), want, "{raw:?}");
    }
}

#[tokio::test]
async fn no_cache_row_is_unregistered() {
    let env = Env::new(NOW).await;
    let state = env.server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Unregistered);
    assert!(state.tenant.is_none());
}

#[tokio::test]
async fn a_row_without_a_tenant_is_unregistered() {
    // `setMode` before signup makes a row with a NULL tenant.
    let env = Env::new(NOW).await;
    env.server.set_mode(Mode::Airgap).await.unwrap();
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Unregistered
    );
    // An empty slug fails `toState`'s truthiness check the same way.
    let mut t = tenant("active");
    t.slug = Some(String::new());
    env.server
        .write_install_cache(&t, Mode::Online)
        .await
        .unwrap();
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Unregistered
    );
}

#[tokio::test]
async fn soft_fresh_answers_from_the_cache_without_a_network_call() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.clock.advance(DAY - 1);
    let state = env.server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Ok);
    assert!(env.fake.seen().is_empty());
    // `toState`'s projection: only the cached fields, the rest blank.
    assert_eq!(
        serde_json::to_string(&state.tenant.unwrap()).unwrap(),
        r#"{"tenantId":"tenant_abc","slug":"acme","status":"active","publicUrl":"","anchorLicenseId":"","subscriptionId":"","tier":"team","ownerEmail":"","seatLimit":5,"currentPeriodEnd":"2027-01-01T00:00:00.000Z"}"#
    );
}

#[tokio::test]
async fn stale_then_success_refreshes_the_cache() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.clock.advance(DAY); // exactly the soft TTL: stale
    env.fake.reply(
        "/api/cloud/tenant-info",
        200,
        control_tenant("active", "acme-2"),
    );
    let state = env.server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Ok);
    let t = state.tenant.unwrap();
    assert_eq!(
        (t.slug.as_str(), t.tier.as_str(), t.seat_limit),
        ("acme-2", "business", 9)
    );
    assert_eq!(t.current_period_end, None);

    let seen = env.fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/api/cloud/tenant-info");
    assert_eq!(seen[0].headers["x-license-key"], "OWNER-KEY");
    let cache = env.server.read_install_cache().await.unwrap().unwrap();
    assert_eq!(cache.last_validated_at, NOW + DAY);
    assert_eq!(cache.grace_until, NOW + DAY + 14 * DAY);
    assert_eq!(cache.mode, Mode::Online);

    // Fresh again: no second call.
    env.server.resolve_license_state().await.unwrap();
    assert_eq!(env.fake.seen().len(), 1);
}

#[tokio::test]
async fn stale_then_failure_within_grace_keeps_the_cached_state() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    let server = env.server_with(|c| c.with_control_url(&closed_url()));
    env.clock.advance(14 * DAY - 1);
    let state = server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Ok);
    assert_eq!(state.tenant.unwrap().slug, "acme");
    // Nothing was written.
    let cache = server.read_install_cache().await.unwrap().unwrap();
    assert_eq!(cache.last_validated_at, NOW);
}

#[tokio::test]
async fn stale_then_http_failure_within_grace_keeps_the_cached_state() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.clock.advance(2 * DAY);
    env.fake.reply("/api/cloud/tenant-info", 500, json!({}));
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Ok
    );
}

#[tokio::test]
async fn stale_then_failure_after_grace_is_revalidate() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    let server = env.server_with(|c| c.with_control_url(&closed_url()));
    env.clock.advance(14 * DAY); // now == grace_until: past grace
    let state = server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Revalidate);
    assert!(state.tenant.is_none());
}

#[tokio::test]
async fn a_null_tenant_info_falls_through_to_grace() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.fake.reply("/api/cloud/tenant-info", 200, Value::Null);
    env.clock.advance(2 * DAY);
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Ok
    );
    env.clock.advance(14 * DAY);
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Revalidate
    );
}

#[tokio::test]
async fn suspended_from_the_cache_or_a_refresh() {
    let env = Env::new(NOW).await;
    registered(&env, "suspended").await;
    let state = env.server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Suspended);
    assert_eq!(state.tenant.unwrap().status, "suspended");

    // An active install the control plane suspends on refresh.
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.clock.advance(DAY);
    env.fake.reply(
        "/api/cloud/tenant-info",
        200,
        control_tenant("suspended", "acme"),
    );
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Suspended
    );
}

#[tokio::test]
async fn ttl_overrides_move_the_rungs() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    let server = env.server_with(|c| c.with_control_url(&closed_url()).with_ttls(60, 120));
    env.clock.advance(59);
    assert_eq!(
        server.resolve_license_state().await.unwrap().kind,
        GateState::Ok
    );
    // The row was written with the default grace (14 d), so a shorter
    // override only changes the soft rung here.
    env.clock.advance(1);
    assert_eq!(
        server.resolve_license_state().await.unwrap().kind,
        GateState::Ok
    );

    // A row written under the override gets its grace.
    server
        .write_install_cache(&tenant("active"), Mode::Online)
        .await
        .unwrap();
    env.clock.advance(120);
    assert_eq!(
        server.resolve_license_state().await.unwrap().kind,
        GateState::Revalidate
    );
}

#[tokio::test]
async fn air_gap_refresh_reads_the_bundle_and_keeps_airgap_mode() {
    let env = Env::new(NOW).await;
    let signed = env.sign(&bundle_payload(NOW + 5 * DAY));
    let verified = verify_bundle(&signed.envelope, &signed.trust(), NOW).unwrap();
    env.server
        .write_bundle(&verified, &signed.payload_sha256())
        .await
        .unwrap();
    env.server
        .register_install_local("owner_key_abc")
        .await
        .unwrap();
    env.clock.advance(2 * DAY);
    let state = env.server.resolve_license_state().await.unwrap();
    assert_eq!(state.kind, GateState::Ok);
    assert_eq!(state.tenant.unwrap().tenant_id, "airgap-sub_test_0001");
    let cache = env.server.read_install_cache().await.unwrap().unwrap();
    assert_eq!(
        (cache.mode, cache.last_validated_at),
        (Mode::Airgap, NOW + 2 * DAY)
    );
    assert!(env.fake.seen().is_empty());

    // Past the bundle's not_after, local tenant info is null: grace, then
    // revalidate.
    env.clock.set(NOW + 6 * DAY);
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Ok
    );
    assert_eq!(
        env.server
            .read_install_cache()
            .await
            .unwrap()
            .unwrap()
            .last_validated_at,
        NOW + 2 * DAY,
        "an expired bundle refreshes nothing"
    );
    env.clock.set(NOW + 2 * DAY + 14 * DAY);
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Revalidate
    );
}

// ── The gate answer ──

#[tokio::test]
async fn the_gate_answers_state_member_and_install_facts() {
    let env = Env::new(NOW).await;
    let anon = env.server.gate(None).await.unwrap();
    assert_eq!(
        serde_json::to_value(&anon).unwrap(),
        json!({ "state": "unregistered", "tenant": null, "member": null, "hasTenant": false, "bundlePresent": false })
    );

    registered(&env, "active").await;
    let mut node = env.node().await;
    seed_user(&mut node, "u_member", "m@example.test").await;
    env.server
        .insert_member("u_member", "MEMBER-KEY", NOW, "tm_m", false)
        .await
        .unwrap();

    let owner = serde_json::to_value(env.server.gate(Some("u_owner")).await.unwrap()).unwrap();
    assert_eq!(owner["state"], "ok");
    assert_eq!(owner["tenant"]["slug"], "acme");
    assert_eq!(
        owner["member"],
        json!({ "isOwner": true, "revoked": false })
    );
    assert_eq!(owner["hasTenant"], true);
    let member = serde_json::to_value(env.server.gate(Some("u_member")).await.unwrap()).unwrap();
    assert_eq!(
        member["member"],
        json!({ "isOwner": false, "revoked": false })
    );
    let stranger = serde_json::to_value(env.server.gate(Some("u_x")).await.unwrap()).unwrap();
    assert_eq!(stranger["member"], Value::Null);

    // A revoked row stays, flagged: handleApiGate's 403.
    exec(
        &mut node,
        &format!("UPDATE member_license SET revoked_at = {NOW} WHERE user_id = 'u_member'"),
    )
    .await;
    let revoked = serde_json::to_value(env.server.gate(Some("u_member")).await.unwrap()).unwrap();
    assert_eq!(
        revoked["member"],
        json!({ "isOwner": false, "revoked": true })
    );
}

#[tokio::test]
async fn backfill_runs_once_rust_finds_the_tables() {
    // `backfillExistingMembers` moved here from Node's `openAuthDb`.
    let env = Env::new(NOW).await;
    let mut node = env.node().await;
    seed_user(&mut node, "u_old", "old@example.test").await;
    exec(
        &mut node,
        "INSERT INTO member_license (user_id, license_key, bound_at, control_member_id)
         VALUES ('u_old', 'OLD', 1, 'tm')",
    )
    .await;
    env.server.gate(None).await.unwrap();
    let row: (Option<i64>, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT last_validated_at, grace_until, cached_status FROM member_license WHERE user_id = 'u_old'",
    )
    .fetch_one(&mut node)
    .await
    .unwrap();
    assert_eq!(
        row,
        (Some(NOW), Some(NOW + 14 * DAY), Some("active".into()))
    );
}

#[tokio::test]
async fn rust_waits_out_a_write_lock_held_by_the_other_process() {
    // Node's better-sqlite3 handle stands in as a second connection that
    // holds the write lock for a while; Rust's write waits (busy timeout)
    // instead of failing with SQLITE_BUSY.
    let env = Env::new(NOW).await;
    env.server.gate(None).await.unwrap(); // opened and ready
    let mut node = env.node().await;
    exec(&mut node, "BEGIN IMMEDIATE").await;
    exec(
        &mut node,
        "INSERT INTO install (id, install_id, created_at) VALUES (1, 'from-node', 1)",
    )
    .await;
    let write = env.server.set_mode(Mode::Airgap);
    let release = async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        exec(&mut node, "COMMIT").await;
    };
    let (written, ()) = tokio::join!(write, release);
    written.unwrap();
    assert_eq!(
        env.server.read_install_cache().await.unwrap().unwrap().mode,
        Mode::Airgap
    );
    // And Rust reads what Node committed.
    assert_eq!(env.server.install_id().await.unwrap(), "from-node");
}

#[tokio::test]
async fn a_refresh_without_a_tenant_id_is_a_failure_not_a_wipe() {
    let env = Env::new(NOW).await;
    registered(&env, "active").await;
    env.clock.advance(2 * DAY);
    for body in [
        json!({ "slug": "acme", "status": "active", "tier": "team", "seatLimit": 5 }),
        json!({ "tenantId": null, "slug": "acme" }),
        json!({ "tenantId": "", "slug": "acme" }),
    ] {
        env.fake.reply("/api/cloud/tenant-info", 200, body.clone());
        let state = env.server.resolve_license_state().await.unwrap();
        assert_eq!(state.kind, GateState::Ok, "{body}");
        assert_eq!(state.tenant.unwrap().tenant_id, "tenant_abc");
        let cache = env.server.read_install_cache().await.unwrap().unwrap();
        assert_eq!(cache.tenant_id.as_deref(), Some("tenant_abc"));
        assert_eq!(cache.last_validated_at, NOW, "nothing written");
    }
    // Past grace it's revalidate, as for any failed refresh.
    env.clock.advance(14 * DAY);
    assert_eq!(
        env.server.resolve_license_state().await.unwrap().kind,
        GateState::Revalidate
    );
}
