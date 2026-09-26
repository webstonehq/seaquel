//! Online vs air-gap routing and the control-plane contract, ported from
//! `licensing.test.ts` and run against a local fake control plane. Nothing
//! here reaches the real one.

mod common;

use common::{bundle_payload, closed_url, seed_user, Env};
use seaquel_license::server::airgap::verify::verify_bundle;
use seaquel_license::server::{BindArgs, Role, ServerErrorCode};
use serde_json::{json, Value};

const NOW: i64 = 1_750_000_000;
const OWNER_KEY: &str = "SQ-OWNER-KEY-7f3c";

fn fake_tenant() -> Value {
    json!({
        "tenantId": "tenant_abc",
        "slug": "acme",
        "status": "active",
        "publicUrl": "https://example.test",
        "anchorLicenseId": "lic_anchor",
        "subscriptionId": "sub_123",
        "tier": "team",
        "ownerEmail": "owner@example.test",
        "seatLimit": 5,
        "currentPeriodEnd": "2027-01-01T00:00:00.000Z",
    })
}

async fn import_bundle(env: &Env) {
    let signed = env.sign(&bundle_payload(NOW + 60 * 60 * 24 * 365));
    let verified = verify_bundle(&signed.envelope, &signed.trust(), NOW).unwrap();
    env.server
        .write_bundle(&verified, &signed.payload_sha256())
        .await
        .unwrap();
}

async fn bind_owner(env: &Env) {
    let mut node = env.node().await;
    seed_user(&mut node, "u_owner", "owner@example.test").await;
    env.server
        .insert_member("u_owner", OWNER_KEY, NOW, "tm_owner", true)
        .await
        .unwrap();
}

// ── Bundle present: everything routes locally ──

#[tokio::test]
async fn with_a_bundle_every_call_routes_to_the_local_path() {
    let env = Env::new(NOW).await;
    import_bundle(&env).await;
    bind_owner(&env).await;

    let registered = env
        .server
        .register_install_dispatch("owner_key_abc")
        .await
        .unwrap();
    assert_eq!(
        registered.tenant_id.as_deref(),
        Some("airgap-sub_test_0001")
    );
    let info = env.server.tenant_info().await.unwrap().unwrap();
    assert_eq!(info.tenant_id.as_deref(), Some("airgap-sub_test_0001"));
    let verified = env
        .server
        .verify_membership("member_key_xyz", "u@example.test")
        .await
        .unwrap();
    assert!(verified.get().starts_with(r#"{"ok":true"#));
    let id = env
        .server
        .bind_member_dispatch(&BindArgs {
            license_key: "k".into(),
            container_user_id: "u_1".into(),
            email: "u@example.test".into(),
            role: Role::Member,
            auth_key: None,
        })
        .await
        .unwrap();
    assert!(id.starts_with("local-"));
    env.server.unbind_member_dispatch("u_1").await.unwrap();
    let members: Vec<Value> =
        serde_json::from_str(env.server.list_members().await.unwrap().get()).unwrap();
    assert_eq!(members.len(), 1);

    assert!(
        env.fake.seen().is_empty(),
        "no control-plane call with a bundle"
    );
}

// ── Bundle absent: the control plane, with today's contract ──

#[tokio::test]
async fn register_install_posts_install_id_and_key_and_parses_the_tenant() {
    let env = Env::new(NOW).await;
    env.fake
        .reply("/api/cloud/register-install", 200, fake_tenant());
    let tenant = env
        .server
        .register_install_dispatch(OWNER_KEY)
        .await
        .unwrap();
    assert_eq!(tenant.tenant_id.as_deref(), Some("tenant_abc"));
    assert_eq!(tenant.seat_limit, Some(5));

    let seen = env.fake.seen();
    assert_eq!(seen.len(), 1);
    let install_id = env.server.install_id().await.unwrap();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].headers["x-install-id"], install_id);
    assert_eq!(seen[0].headers["x-license-key"], OWNER_KEY);
    assert_eq!(seen[0].headers["content-type"], "application/json");
    // `JSON.stringify({installId, licenseKey})`, byte for byte.
    assert_eq!(
        seen[0].body,
        format!(r#"{{"installId":"{install_id}","licenseKey":"{OWNER_KEY}"}}"#)
    );
}

#[tokio::test]
async fn verify_membership_posts_key_and_email_and_passes_the_answer_through() {
    let env = Env::new(NOW).await;
    let answer =
        json!({ "ok": true, "subscriptionId": "sub_123", "tier": "team", "role": "owner" });
    env.fake
        .reply("/api/cloud/verify-membership-license", 200, answer.clone());
    let out = env
        .server
        .verify_membership("k\"1", "u@example.test")
        .await
        .unwrap();
    // Passed through as the control plane wrote it.
    assert_eq!(out.get(), answer.to_string());
    let seen = env.fake.seen();
    assert_eq!(seen[0].headers["x-license-key"], "k\"1");
    assert_eq!(
        seen[0].body,
        r#"{"licenseKey":"k\"1","signupEmail":"u@example.test"}"#
    );
}

#[tokio::test]
async fn bind_member_uses_the_owner_key_or_the_auth_key_override() {
    let env = Env::new(NOW).await;
    env.fake.reply(
        "/api/cloud/bind-member",
        200,
        json!({ "tenantMemberId": "tm_9" }),
    );
    let mut args = BindArgs {
        license_key: "MEMBER-1".into(),
        container_user_id: "u_9".into(),
        email: "m@example.test".into(),
        role: Role::Member,
        auth_key: None,
    };
    // No owner bound and no override: refused before any request.
    let err = env.server.bind_member_dispatch(&args).await.unwrap_err();
    assert_eq!(err.code, ServerErrorCode::NoOwner);
    assert_eq!(
        err.message,
        "no owner license bound — call registerInstall first"
    );
    assert!(env.fake.seen().is_empty());

    // First-owner signup: the presented key authenticates.
    args.auth_key = Some("FIRST-OWNER".into());
    assert_eq!(
        env.server.bind_member_dispatch(&args).await.unwrap(),
        "tm_9"
    );
    // Later: the bound owner's key.
    bind_owner(&env).await;
    args.auth_key = None;
    env.server.bind_member_dispatch(&args).await.unwrap();

    let seen = env.fake.seen();
    assert_eq!(seen[0].headers["x-license-key"], "FIRST-OWNER");
    assert_eq!(seen[1].headers["x-license-key"], OWNER_KEY);
    for s in &seen {
        // `authKey` never reaches the body.
        assert_eq!(
            s.body,
            r#"{"licenseKey":"MEMBER-1","containerUserId":"u_9","email":"m@example.test","role":"member"}"#
        );
    }
}

#[tokio::test]
async fn unbind_list_and_tenant_info_use_the_owner_key() {
    let env = Env::new(NOW).await;
    for (err, what) in [
        (
            env.server.unbind_member_dispatch("u_1").await.unwrap_err(),
            "unbind",
        ),
        (env.server.list_members().await.unwrap_err(), "list"),
        (env.server.tenant_info().await.unwrap_err(), "tenant-info"),
    ] {
        assert_eq!(err.code, ServerErrorCode::NoOwner, "{what}");
    }
    assert!(env.fake.seen().is_empty());

    bind_owner(&env).await;
    env.fake.reply("/api/cloud/unbind-member", 200, json!({}));
    let members = json!([{ "tenantMemberId": "tm_1", "containerUserId": "u_1", "email": "a@x", "role": "owner", "boundAt": null, "maskedLicenseKey": "••••-aaaa" }]);
    env.fake.reply("/api/cloud/members", 200, members.clone());
    env.fake.reply("/api/cloud/tenant-info", 200, fake_tenant());

    env.server.unbind_member_dispatch("u_1").await.unwrap();
    assert_eq!(
        env.server.list_members().await.unwrap().get(),
        members.to_string()
    );
    assert!(env.server.tenant_info().await.unwrap().is_some());

    let seen = env.fake.seen();
    let summary: Vec<_> = seen
        .iter()
        .map(|s| (s.method.as_str(), s.path.as_str(), s.body.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                "POST",
                "/api/cloud/unbind-member",
                r#"{"containerUserId":"u_1"}"#
            ),
            ("GET", "/api/cloud/members", ""),
            ("GET", "/api/cloud/tenant-info", ""),
        ]
    );
    for s in &seen {
        assert_eq!(s.headers["x-license-key"], OWNER_KEY);
        // GETs carry no content type, as the TS set it only with a body.
        assert_eq!(s.headers.contains_key("content-type"), s.method == "POST");
    }
}

#[tokio::test]
async fn a_network_failure_is_network_error() {
    let env = Env::new(NOW).await;
    let server = env.server_with(|c| c.with_control_url(&closed_url()));
    let err = server.register_install_dispatch("k").await.unwrap_err();
    assert_eq!(err.code, ServerErrorCode::NetworkError);
    assert!(err.is_network_failure());
    let err = server.verify_membership("k", "e").await.unwrap_err();
    assert!(err.is_network_failure());
}

#[tokio::test]
async fn an_http_error_is_not_a_network_failure() {
    let env = Env::new(NOW).await;
    env.fake.reply(
        "/api/cloud/register-install",
        400,
        json!({ "error": "bad_request" }),
    );
    let err = env.server.register_install_dispatch("k").await.unwrap_err();
    assert_eq!(err.code, ServerErrorCode::ControlPlaneError);
    assert!(!err.is_network_failure());
    assert_eq!(
        err.message,
        r#"register-install failed: 400 {"error":"bad_request"}"#
    );

    for (path, status, message) in [(
        "/api/cloud/verify-membership-license",
        500,
        "verify-membership-license failed: 500",
    )] {
        env.fake.reply(path, status, json!({}));
        let err = env.server.verify_membership("k", "e").await.unwrap_err();
        assert_eq!(err.message, message);
    }

    bind_owner(&env).await;
    env.fake.reply("/api/cloud/tenant-info", 401, json!({}));
    assert_eq!(
        env.server.tenant_info().await.unwrap_err().message,
        "control plane rejected license key (revoked or wrong install?)"
    );
    env.fake.reply("/api/cloud/tenant-info", 503, json!({}));
    assert_eq!(
        env.server.tenant_info().await.unwrap_err().message,
        "tenant-info failed: 503"
    );
    env.fake.reply_text("/api/cloud/bind-member", 409, "taken");
    let err = env
        .server
        .bind_member_dispatch(&BindArgs {
            license_key: "k".into(),
            container_user_id: "u".into(),
            email: "e".into(),
            role: Role::Member,
            auth_key: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.message, "bind-member failed: 409 taken");
    env.fake
        .reply_text("/api/cloud/unbind-member", 409, "owner");
    assert_eq!(
        env.server
            .unbind_member_dispatch("u")
            .await
            .unwrap_err()
            .message,
        "unbind-member failed: 409 owner"
    );
    env.fake.reply("/api/cloud/members", 500, json!({}));
    assert_eq!(
        env.server.list_members().await.unwrap_err().message,
        "members failed: 500"
    );
}

#[tokio::test]
async fn a_tenant_info_of_null_is_no_tenant() {
    let env = Env::new(NOW).await;
    bind_owner(&env).await;
    env.fake.reply("/api/cloud/tenant-info", 200, Value::Null);
    assert!(env.server.tenant_info().await.unwrap().is_none());
}

#[tokio::test]
async fn a_trailing_slash_on_the_control_url_is_dropped_once() {
    let env = Env::new(NOW).await;
    let url = format!("{}/", env.control_url);
    let server = env.server_with(|c| c.with_control_url(&url));
    env.fake
        .reply("/api/cloud/register-install", 200, fake_tenant());
    server.register_install_dispatch("k").await.unwrap();
    assert_eq!(env.fake.seen()[0].path, "/api/cloud/register-install");
}

// ── No in-memory caching of the mode decision ──

#[tokio::test]
async fn flipping_bundle_presence_flips_the_route_on_the_next_call() {
    let env = Env::new(NOW).await;
    env.fake
        .reply("/api/cloud/register-install", 200, fake_tenant());
    let online = env
        .server
        .register_install_dispatch("owner_key_abc")
        .await
        .unwrap();
    assert_eq!(online.tenant_id.as_deref(), Some("tenant_abc"));
    assert_eq!(env.fake.seen().len(), 1);

    import_bundle(&env).await;
    env.fake.clear();
    let airgap = env
        .server
        .register_install_dispatch("owner_key_abc")
        .await
        .unwrap();
    assert_eq!(airgap.tenant_id.as_deref(), Some("airgap-sub_test_0001"));
    assert_eq!(airgap.slug.as_deref(), Some("acme"));
    let info = env.server.tenant_info().await.unwrap().unwrap();
    assert_eq!(info.tenant_id.as_deref(), Some("airgap-sub_test_0001"));
    assert!(env.fake.seen().is_empty());
}

// ── The install id ──

#[tokio::test]
async fn the_install_id_is_made_once_and_kept() {
    let env = Env::new(NOW).await;
    let first = env.server.install_id().await.unwrap();
    assert_eq!(first.len(), 36, "a UUID");
    assert_eq!(env.server.install_id().await.unwrap(), first);
    // A restart reads it back.
    let restarted = env.server_with(|c| c);
    assert_eq!(restarted.install_id().await.unwrap(), first);
    let mut node = env.node().await;
    let (id, created_at): (String, i64) =
        sqlx::query_as("SELECT install_id, created_at FROM install WHERE id = 1")
            .fetch_one(&mut node)
            .await
            .unwrap();
    assert_eq!(id, first);
    assert_eq!(created_at, NOW);
}

#[tokio::test]
async fn concurrent_first_calls_make_one_install_id() {
    let env = Env::new(NOW).await;
    let (a, b, c) = tokio::join!(
        env.server.install_id(),
        env.server.install_id(),
        env.server.install_id()
    );
    assert_eq!(a.as_ref().unwrap(), b.as_ref().unwrap());
    assert_eq!(b.unwrap(), c.unwrap());
}

// ── Not ready ──

#[tokio::test]
async fn before_node_creates_auth_db_every_call_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let server = seaquel_license::server::LicenseServer::new(
        seaquel_license::server::ServerConfig::new(dir.path().join("auth.db")),
    );
    let err = server.gate(None).await.unwrap_err();
    assert_eq!(err.code, ServerErrorCode::NotReady);
    assert!(
        !dir.path().join("auth.db").exists(),
        "Rust never creates auth.db"
    );

    // A file without the license tables (Node opened it but no migrations
    // yet) is not ready either.
    let mut conn = common::node_conn_create(&dir.path().join("auth.db")).await;
    common::exec(&mut conn, "CREATE TABLE unrelated (x)").await;
    let err = server.install_status().await.unwrap_err();
    assert_eq!(err.code, ServerErrorCode::NotReady);

    // Once the migrations have run, the same server works.
    drop(conn);
    common::create_auth_db(dir.path()).await;
    assert!(server.install_status().await.is_ok());
}

// ── Redaction ──

#[tokio::test]
async fn license_keys_never_appear_in_debug_or_errors() {
    let env = Env::new(NOW).await;
    bind_owner(&env).await;
    let member = env.server.find_member("u_owner").await.unwrap().unwrap();
    let shown = format!("{member:?}");
    assert!(!shown.contains(OWNER_KEY), "{shown}");
    let args = BindArgs {
        license_key: OWNER_KEY.into(),
        container_user_id: "u".into(),
        email: "e".into(),
        role: Role::Owner,
        auth_key: Some(OWNER_KEY.into()),
    };
    assert!(!format!("{args:?}").contains(OWNER_KEY));
    let server = env.server_with(|c| c.with_control_url(&closed_url()));
    let err = server
        .register_install_dispatch(OWNER_KEY)
        .await
        .unwrap_err();
    assert!(!err.to_string().contains(OWNER_KEY));
    assert!(!format!("{err:?}").contains(OWNER_KEY));
    assert!(!format!("{:?}", env.server).contains(OWNER_KEY));
}

#[tokio::test]
async fn a_bad_node_extra_ca_certs_file_doesnt_break_the_client() {
    let env = Env::new(NOW).await;
    let server = env.server_with(|c| c.with_extra_ca_file("/nonexistent/seaquel-ca.pem"));
    env.fake
        .reply("/api/cloud/register-install", 200, fake_tenant());
    server.register_install_dispatch("k").await.unwrap();
}
