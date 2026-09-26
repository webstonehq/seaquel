//! `/internal/license/*` through the router: the loopback rule, `NOT_READY`
//! before Node has made auth.db, and the air-gap lifecycle that
//! `src/routes/api/airgap/e2e.test.ts` ran against the TypeScript (bundle
//! upload, owner and member signup, a refused key, bundle delete, expiry
//! and revalidate, a revoking re-upload). The Node halves of those steps
//! (session purge, redirects, the 503 mapping) stay in vitest.
//!
//! auth.db is built from Node's migrations 006–012; the control plane is a
//! closed port, and the steps after the upload assert it was never called.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use seaquel_core::license::server::airgap::canonical::{
    canonicalize, fingerprint_pubkey, CanonicalValue,
};
use seaquel_core::license::server::airgap::TrustSet;
use seaquel_core::license::server::{LicenseServer, ServerConfig};
use seaquel_server::{build_router, AppState, Workspaces};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use tower::ServiceExt;

// ── Setup ──

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/server/migrations")
}

/// auth.db as Node's `openAuthDb` makes it.
async fn create_auth_db(dir: &Path) -> PathBuf {
    let path = dir.join("auth.db");
    let mut conn = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .connect()
        .await
        .unwrap();
    let mut files: Vec<_> = std::fs::read_dir(migrations_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 7, "migrations 006–012");
    for file in files {
        let mut tx = conn.begin().await.unwrap();
        sqlx::raw_sql(&std::fs::read_to_string(file).unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    path
}

fn seed() -> SigningKey {
    SigningKey::from_bytes(&std::array::from_fn(|i| i as u8))
}

fn trust() -> TrustSet {
    let pubkey = seed().verifying_key().to_bytes().to_vec();
    let mut t = TrustSet::new();
    t.insert(fingerprint_pubkey(&pubkey), pubkey);
    t
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

struct BundleOpts {
    issued_at: i64,
    not_after: i64,
    revoked: Vec<&'static str>,
    subscription: &'static str,
}

impl Default for BundleOpts {
    fn default() -> Self {
        let t = now();
        Self {
            issued_at: t,
            not_after: t + 86_400,
            revoked: Vec::new(),
            subscription: "sub_test_001",
        }
    }
}

/// The e2e test's bundle: two seats, OWNER-KEY-001 and MEMBER-KEY-001.
fn bundle(o: BundleOpts) -> Vec<u8> {
    let payload = json!({
        "version": 1,
        "issued_at": o.issued_at,
        "not_before": o.issued_at - 60,
        "not_after": o.not_after,
        "subscription_id": o.subscription,
        "tenant_slug": "self-testbed",
        "tier": "business",
        "seats": 5,
        "seat_tokens": [
            { "key": "OWNER-KEY-001", "role": "owner" },
            { "key": "MEMBER-KEY-001", "role": "member" },
        ],
        "revoked_keys": o.revoked,
        "issued_by_install_id": null,
    });
    let canonical = canonicalize(&CanonicalValue::from(&payload)).unwrap();
    let sig = seed().sign(&canonical).to_bytes();
    json!({
        "payload": b64url(&canonical),
        "sig": b64url(&sig),
        "pubkey_fingerprint": fingerprint_pubkey(&seed().verifying_key().to_bytes()),
    })
    .to_string()
    .into_bytes()
}

fn closed_url() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

struct Env {
    app: axum::Router,
    db: PathBuf,
    _dir: tempfile::TempDir,
}

async fn env_with(make_db: bool, control_url: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let db = if make_db {
        create_auth_db(dir.path()).await
    } else {
        dir.path().join("auth.db")
    };
    let license = LicenseServer::new(
        ServerConfig::new(&db)
            .with_control_url(control_url)
            .with_trusted(trust()),
    );
    let state = AppState {
        core: Arc::new(seaquel_core::Core::builder().build()),
        workspaces: Arc::new(Workspaces::new(dir.path())),
        license: Arc::new(license),
        internal_secret: Some(Arc::from(SECRET)),
    };
    Env {
        app: build_router(state),
        db,
        _dir: dir,
    }
}

async fn env() -> Env {
    env_with(true, &closed_url()).await
}

const LOOPBACK: &str = "127.0.0.1:50000";
const SECRET: &str = "per-boot-secret-for-tests";

impl Env {
    async fn send_from(
        &self,
        peer: Option<&str>,
        method: &str,
        uri: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        self.send_with(peer, Some(SECRET), method, uri, body).await
    }

    async fn send_with(
        &self,
        peer: Option<&str>,
        secret: Option<&str>,
        method: &str,
        uri: &str,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(secret) = secret {
            req = req.header("x-seaquel-internal", secret);
        }
        let mut req = req.body(Body::from(body)).unwrap();
        if let Some(peer) = peer {
            let addr: SocketAddr = peer.parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
        }
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.send_from(Some(LOOPBACK), "GET", uri, Vec::new()).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.send_from(Some(LOOPBACK), "POST", uri, body.to_string().into_bytes())
            .await
    }

    async fn upload(&self, user: Option<&str>, bytes: Vec<u8>) -> (StatusCode, Value) {
        let uri = match user {
            Some(u) => format!("/internal/license/airgap/upload?user={u}"),
            None => "/internal/license/airgap/upload".to_string(),
        };
        self.send_from(Some(LOOPBACK), "POST", &uri, bytes).await
    }

    async fn node(&self) -> SqliteConnection {
        SqliteConnectOptions::new()
            .filename(&self.db)
            .foreign_keys(true)
            .connect()
            .await
            .unwrap()
    }

    /// What Better Auth's signUpEmail does for the test: the user row.
    async fn better_auth_user(&self, id: &str) {
        let mut node = self.node().await;
        sqlx::query(
            r#"INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
               VALUES (?, ?, ?, 0, datetime('now'), datetime('now'))"#,
        )
        .bind(id)
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(&mut node)
        .await
        .unwrap();
    }

    async fn member_row(&self, user: &str) -> Option<(i64, Option<i64>)> {
        sqlx::query_as("SELECT is_owner, revoked_at FROM member_license WHERE user_id = ?")
            .bind(user)
            .fetch_optional(&mut self.node().await)
            .await
            .unwrap()
    }
}

// ── The loopback rule ──

#[tokio::test]
async fn a_non_loopback_peer_gets_403_on_every_route() {
    let env = env().await;
    for (method, uri) in [
        ("GET", "/internal/license/gate"),
        ("GET", "/internal/license/install"),
        ("POST", "/internal/license/register-install"),
        ("POST", "/internal/license/signup-check"),
        ("POST", "/internal/license/bind-member"),
        ("POST", "/internal/license/unbind-member"),
        ("GET", "/internal/license/members"),
        ("GET", "/internal/license/airgap/status"),
        ("POST", "/internal/license/airgap/upload"),
        ("POST", "/internal/license/airgap/clear"),
    ] {
        for peer in [Some("10.0.0.5:4000"), Some("[2001:db8::1]:4000"), None] {
            let (status, body) = env.send_from(peer, method, uri, b"{}".to_vec()).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} from {peer:?}"
            );
            assert_eq!(body["code"], "FORBIDDEN");
        }
    }
    // Nothing reached the database.
    let (_, status) = env.get("/internal/license/airgap/status").await;
    assert_eq!(status["bundle"], Value::Null);
}

#[tokio::test]
async fn a_loopback_peer_without_the_secret_gets_403() {
    // A query inside the Rust process (DuckDB httpfs) reaches /internal from
    // loopback too; the per-boot secret is what keeps it out.
    let env = env().await;
    for secret in [
        None,
        Some(""),
        Some("wrong"),
        Some("per-boot-secret-for-test"),
    ] {
        let (status, body) = env
            .send_with(
                Some(LOOPBACK),
                secret,
                "GET",
                "/internal/license/members",
                Vec::new(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{secret:?}");
        assert_eq!(body["code"], "FORBIDDEN");
    }
    // The right secret from elsewhere is still refused.
    let (status, _) = env
        .send_with(
            Some("10.0.0.5:1"),
            Some(SECRET),
            "GET",
            "/internal/license/install",
            Vec::new(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_server_without_a_secret_refuses_every_internal_call() {
    let dir = tempfile::tempdir().unwrap();
    let db = create_auth_db(dir.path()).await;
    let state = AppState {
        core: Arc::new(seaquel_core::Core::builder().build()),
        workspaces: Arc::new(Workspaces::new(dir.path())),
        license: Arc::new(LicenseServer::new(ServerConfig::new(&db))),
        internal_secret: None,
    };
    let env = Env {
        app: build_router(state.with_internal_secret(Some(String::new()))),
        db,
        _dir: dir,
    };
    for secret in [None, Some(""), Some(SECRET)] {
        let (status, _) = env
            .send_with(
                Some(LOOPBACK),
                secret,
                "GET",
                "/internal/license/install",
                Vec::new(),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{secret:?}");
    }
}

#[tokio::test]
async fn loopback_peers_are_served() {
    let env = env().await;
    for peer in [
        "127.0.0.1:1",
        "[::1]:1",
        "[::ffff:127.0.0.1]:1",
        "127.0.0.2:1",
    ] {
        let (status, _) = env
            .send_from(Some(peer), "GET", "/internal/license/install", Vec::new())
            .await;
        assert_eq!(status, StatusCode::OK, "{peer}");
    }
}

#[tokio::test]
async fn before_node_makes_auth_db_the_routes_are_not_ready() {
    let env = env_with(false, &closed_url()).await;
    let (status, body) = env.get("/internal/license/gate").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "NOT_READY");
    assert!(!env.db.exists());
}

#[tokio::test]
async fn a_bad_body_is_invalid_argument() {
    let env = env().await;
    let (status, body) = env
        .post("/internal/license/register-install", json!({ "key": "x" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "INVALID_ARGUMENT");
}

// ── The air-gap lifecycle (e2e.test.ts's steps 3–11) ──

#[tokio::test]
async fn air_gap_lifecycle() {
    let env = env().await;

    // Step 3: no bundle, first owner, control plane unreachable: the
    // register step fails as a network failure (Node answers 503).
    let (_, install) = env.get("/internal/license/install").await;
    assert_eq!(
        install,
        json!({ "hasTenant": false, "bundlePresent": false })
    );
    let (status, body) = env
        .post(
            "/internal/license/register-install",
            json!({ "licenseKey": "OWNER-KEY-001" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["code"], "NETWORK_ERROR");
    assert!(!body["message"].as_str().unwrap().contains("OWNER-KEY-001"));

    // Step 4: upload the bundle on the fresh install, signed out: 200,
    // air-gap mode.
    let (status, out) = env.upload(None, bundle(BundleOpts::default())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["ok"], true);
    assert_eq!(out["body"]["unchanged"], false);
    assert_eq!(out["body"]["tier"], "business");
    assert_eq!(out["body"]["seats"], 5);
    assert_eq!(out["revokedUserIds"], json!([]));
    let (_, st) = env.get("/internal/license/airgap/status").await;
    assert_eq!(st["mode"], "airgap");
    let (_, install) = env.get("/internal/license/install").await;
    assert_eq!(install["bundlePresent"], true);

    // Step 5: the owner signs up: register (from the bundle), check, Better
    // Auth makes the user, bind as first owner.
    let (status, _) = env
        .post(
            "/internal/license/register-install",
            json!({ "licenseKey": "OWNER-KEY-001" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, verified) = env
        .post(
            "/internal/license/signup-check",
            json!({ "licenseKey": "OWNER-KEY-001", "email": "owner@example.test" }),
        )
        .await;
    assert_eq!(verified["ok"], true);
    assert_eq!(verified["role"], "owner");
    env.better_auth_user("u_owner").await;
    let (status, bound) = env
        .post(
            "/internal/license/bind-member",
            json!({ "licenseKey": "OWNER-KEY-001", "userId": "u_owner", "email": "owner@example.test", "role": "owner", "firstOwner": true }),
        )
        .await;
    assert_eq!((status, bound), (StatusCode::OK, json!({ "bound": true })));
    assert_eq!(env.member_row("u_owner").await, Some((1, None)));
    let (_, install) = env.get("/internal/license/install").await;
    assert_eq!(install["hasTenant"], true);

    // Step 6: a member signs up.
    let (_, verified) = env
        .post(
            "/internal/license/signup-check",
            json!({ "licenseKey": "MEMBER-KEY-001", "email": "member@example.test" }),
        )
        .await;
    assert_eq!(verified["role"], "member");
    env.better_auth_user("u_member").await;
    let (_, bound) = env
        .post(
            "/internal/license/bind-member",
            json!({ "licenseKey": "MEMBER-KEY-001", "userId": "u_member", "email": "member@example.test", "role": "member", "firstOwner": false }),
        )
        .await;
    assert_eq!(bound, json!({ "bound": true }));
    assert_eq!(env.member_row("u_member").await, Some((0, None)));
    // Binding again does nothing.
    let (_, again) = env
        .post(
            "/internal/license/bind-member",
            json!({ "licenseKey": "MEMBER-KEY-001", "userId": "u_member", "email": "m", "role": "member", "firstOwner": false }),
        )
        .await;
    assert_eq!(again, json!({ "bound": false }));

    // Step 7: a key that isn't in the bundle.
    let (_, verified) = env
        .post(
            "/internal/license/signup-check",
            json!({ "licenseKey": "RANDOM-NOT-IN-BUNDLE", "email": "r@example.test" }),
        )
        .await;
    assert_eq!(
        verified,
        json!({ "ok": false, "error": "license_not_found" })
    );

    // The gate for each.
    let (_, gate) = env.get("/internal/license/gate?user=u_owner").await;
    assert_eq!(gate["state"], "ok");
    assert_eq!(gate["tenant"]["tenantId"], "airgap-sub_test_001");
    assert_eq!(gate["member"], json!({ "isOwner": true, "revoked": false }));
    let (_, members) = env.get("/internal/license/members").await;
    assert_eq!(members.as_array().unwrap().len(), 2);

    // Established install: an upload needs the owner.
    let (status, body) = env.upload(None, bundle(BundleOpts::default())).await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::UNAUTHORIZED, &json!("UNAUTHORIZED"))
    );
    assert_eq!(body["message"], "must be signed in");
    let (status, body) = env
        .upload(Some("u_member"), bundle(BundleOpts::default()))
        .await;
    assert_eq!(
        (status, &body["message"]),
        (StatusCode::FORBIDDEN, &json!("owner only"))
    );

    // Step 9: the owner deletes the bundle: online mode, rows kept.
    let (status, _) = env
        .send_from(
            Some(LOOPBACK),
            "POST",
            "/internal/license/airgap/clear?user=u_member",
            Vec::new(),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = env
        .send_from(
            Some(LOOPBACK),
            "POST",
            "/internal/license/airgap/clear",
            Vec::new(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = env
        .send_from(
            Some(LOOPBACK),
            "POST",
            "/internal/license/airgap/clear?user=u_owner",
            Vec::new(),
        )
        .await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
    let (_, st) = env
        .get("/internal/license/airgap/status?user=u_owner")
        .await;
    assert_eq!(st["mode"], "online");
    assert_eq!(st["bundle"], Value::Null);
    assert_eq!(st["member"], json!({ "isOwner": true, "revoked": false }));
    assert!(env.member_row("u_owner").await.is_some());
    assert!(env.member_row("u_member").await.is_some());

    // Step 10: an expired bundle uploads fine; with the cache past its
    // grace the gate says revalidate.
    let t = now();
    let (status, out) = env
        .upload(
            Some("u_owner"),
            bundle(BundleOpts {
                issued_at: t - 2000,
                not_after: t - 1000,
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(
        (status, out["status"].clone()),
        (StatusCode::OK, json!(200))
    );
    let (_, st) = env.get("/internal/license/airgap/status").await;
    assert_eq!(st["bundle"]["expired"], true);
    sqlx::query("UPDATE install_cache SET last_validated_at = ?, grace_until = ? WHERE id = 1")
        .bind(t - 10 * 86_400)
        .bind(t - 1)
        .execute(&mut env.node().await)
        .await
        .unwrap();
    let (_, gate) = env.get("/internal/license/gate?user=u_owner").await;
    assert_eq!(gate["state"], "revalidate");
    assert_eq!(gate["tenant"], Value::Null);

    // Step 11: a newer bundle revoking the member's key.
    let (status, out) = env
        .upload(
            Some("u_owner"),
            bundle(BundleOpts {
                issued_at: t + 10,
                not_after: t + 86_400,
                revoked: vec!["MEMBER-KEY-001"],
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["body"]["rowsRevoked"], 1);
    assert_eq!(out["body"]["revokedKeyCount"], 1);
    assert_eq!(out["revokedUserIds"], json!(["u_member"]));
    assert!(env.member_row("u_member").await.unwrap().1.is_some());
    assert_eq!(env.member_row("u_owner").await.unwrap().1, None);
    let (_, gate) = env.get("/internal/license/gate?user=u_member").await;
    assert_eq!(gate["member"], json!({ "isOwner": false, "revoked": true }));

    // A retry of the same upload (say Node's session purge failed after the
    // commit) answers `unchanged` and still lists the revoked user, so the
    // purge happens then.
    let (_, retry) = env
        .upload(
            Some("u_owner"),
            bundle(BundleOpts {
                issued_at: t + 10,
                not_after: t + 86_400,
                revoked: vec!["MEMBER-KEY-001"],
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(retry["body"]["unchanged"], true);
    assert_eq!(retry["body"]["rowsRevoked"], 0);
    assert_eq!(retry["revokedUserIds"], json!(["u_member"]));
    // So does a refusal after the auth check (an older bundle).
    let (_, older) = env
        .upload(
            Some("u_owner"),
            bundle(BundleOpts {
                issued_at: t - 5000,
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(older["status"], 409);
    assert_eq!(older["revokedUserIds"], json!(["u_member"]));
    // But not before it: an anonymous garbage upload learns nothing.
    let (_, garbage) = env.upload(None, b"not json".to_vec()).await;
    assert_eq!(garbage["revokedUserIds"], json!([]));
    // The revoked member drops out of the list; the owner stays.
    let (_, members) = env.get("/internal/license/members").await;
    assert_eq!(members.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn upload_refusals_keep_their_codes_and_statuses() {
    let env = env().await;
    let t = now();
    // Empty body.
    let (_, out) = env.upload(None, Vec::new()).await;
    assert_eq!(out["status"], 400);
    assert_eq!(
        serde_json::to_string(&out["body"]).unwrap(),
        r#"{"error":"malformed_envelope","ok":false}"#
    );
    // Untrusted signer.
    let evil = SigningKey::from_bytes(&[7; 32]);
    let mut env_json: Value = serde_json::from_slice(&bundle(BundleOpts::default())).unwrap();
    env_json["pubkey_fingerprint"] = json!(fingerprint_pubkey(&evil.verifying_key().to_bytes()));
    let (_, out) = env.upload(None, env_json.to_string().into_bytes()).await;
    assert_eq!(
        (out["status"].clone(), out["body"]["error"].clone()),
        (json!(400), json!("untrusted_signer"))
    );

    // First import, then: the same bytes again, an older one, another
    // subscription.
    let first = bundle(BundleOpts {
        issued_at: t,
        ..Default::default()
    });
    let (_, out) = env.upload(None, first.clone()).await;
    assert_eq!(out["body"]["unchanged"], false);
    let (_, out) = env.upload(None, first).await;
    assert_eq!(out["status"], 200);
    assert_eq!(out["body"]["unchanged"], true);
    assert_eq!(out["body"]["rowsRevoked"], 0);
    let (_, out) = env
        .upload(
            None,
            bundle(BundleOpts {
                issued_at: t - 1000,
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(
        (out["status"].clone(), out["body"]["error"].clone()),
        (json!(409), json!("bundle_older_than_current"))
    );
    let (_, out) = env
        .upload(
            None,
            bundle(BundleOpts {
                issued_at: t + 1000,
                subscription: "sub_B",
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(
        (out["status"].clone(), out["body"]["error"].clone()),
        (json!(409), json!("subscription_mismatch"))
    );
}

#[tokio::test]
async fn upload_bodies_keep_the_typescripts_key_order() {
    let env = env().await;
    let req = Request::builder()
        .method("POST")
        .uri("/internal/license/airgap/upload")
        .extension(ConnectInfo::<SocketAddr>(LOOPBACK.parse().unwrap()))
        .header("x-seaquel-internal", SECRET)
        .body(Body::from(bundle(BundleOpts::default())))
        .unwrap();
    let res = env.app.clone().oneshot(req).await.unwrap();
    let text =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let body_start = text.find(r#""body":{"#).unwrap();
    assert!(
        text[body_start..].starts_with(
            r#""body":{"ok":true,"unchanged":false,"tier":"business","seats":5,"notAfter":"#
        ),
        "{text}"
    );
}

// ── Online signup and team calls against a fake control plane ──

#[tokio::test]
async fn online_register_check_bind_members_and_unbind() {
    use axum::routing::{get, post};
    let app = axum::Router::new()
        .route(
            "/api/cloud/register-install",
            post(|| async {
                axum::Json(json!({
                    "tenantId": "t1", "slug": "acme", "status": "active", "publicUrl": "",
                    "anchorLicenseId": "", "subscriptionId": "s", "tier": "team",
                    "ownerEmail": "", "seatLimit": 5, "currentPeriodEnd": null,
                }))
            }),
        )
        .route(
            "/api/cloud/verify-membership-license",
            post(|| async {
                axum::Json(
                    json!({ "ok": true, "subscriptionId": "s", "tier": "team", "role": "owner" }),
                )
            }),
        )
        .route(
            "/api/cloud/bind-member",
            post(|| async { axum::Json(json!({ "tenantMemberId": "tm_up_1" })) }),
        )
        .route(
            "/api/cloud/unbind-member",
            post(|| async { axum::Json(json!({})) }),
        )
        .route(
            "/api/cloud/members",
            get(|| async {
                axum::Json(json!([{ "containerUserId": "u_owner", "role": "owner" }]))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let env = env_with(true, &format!("http://{addr}")).await;
    let (status, _) = env
        .post(
            "/internal/license/register-install",
            json!({ "licenseKey": "OWN" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, gate) = env.get("/internal/license/gate").await;
    assert_eq!(gate["state"], "ok");
    assert_eq!(gate["tenant"]["slug"], "acme");
    let (_, checked) = env
        .post(
            "/internal/license/signup-check",
            json!({ "licenseKey": "OWN", "email": "o@x" }),
        )
        .await;
    assert_eq!(checked["role"], "owner");
    env.better_auth_user("u_owner").await;
    let (_, bound) = env
        .post(
            "/internal/license/bind-member",
            json!({ "licenseKey": "OWN", "userId": "u_owner", "email": "o@x", "role": "owner", "firstOwner": true }),
        )
        .await;
    assert_eq!(bound["bound"], true);
    let control_member_id: String = sqlx::query_scalar(
        "SELECT control_member_id FROM member_license WHERE user_id = 'u_owner'",
    )
    .fetch_one(&mut env.node().await)
    .await
    .unwrap();
    assert_eq!(control_member_id, "tm_up_1");
    let (_, members) = env.get("/internal/license/members").await;
    assert_eq!(
        members,
        json!([{ "containerUserId": "u_owner", "role": "owner" }])
    );

    env.better_auth_user("u_gone").await;
    sqlx::query("INSERT INTO member_license (user_id, license_key, bound_at, control_member_id) VALUES ('u_gone', 'GONE', 1, 'tm')")
        .execute(&mut env.node().await)
        .await
        .unwrap();
    let (status, out) = env
        .post(
            "/internal/license/unbind-member",
            json!({ "userId": "u_gone" }),
        )
        .await;
    assert_eq!(
        (status, out),
        (StatusCode::OK, json!({ "localCleanupFailed": false }))
    );
    assert!(env.member_row("u_gone").await.is_none());
}

#[tokio::test]
async fn control_plane_errors_map_to_502() {
    let env = env().await;
    let mut node = env.node().await;
    sqlx::query(r#"INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt) VALUES ('u', 'u', 'u@x', 0, '', '')"#)
        .execute(&mut node)
        .await
        .unwrap();
    // No owner bound: team calls are refused before any request.
    let (status, body) = env.get("/internal/license/members").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::CONFLICT, &json!("NO_OWNER"))
    );
    sqlx::query("INSERT INTO member_license (user_id, license_key, bound_at, control_member_id, is_owner) VALUES ('u', 'K', 1, 'tm', 1)")
        .execute(&mut node)
        .await
        .unwrap();
    let (status, body) = env.get("/internal/license/members").await;
    assert_eq!(
        (status, &body["code"]),
        (StatusCode::BAD_GATEWAY, &json!("NETWORK_ERROR"))
    );
}
