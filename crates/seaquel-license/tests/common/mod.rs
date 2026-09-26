//! Shared test support: signing, a temp `auth.db` built from Node's
//! migrations 006–012, a fake control plane, and a fixed clock.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use seaquel_license::server::airgap::canonical::{
    canonicalize, fingerprint_pubkey, CanonicalValue,
};
use seaquel_license::server::airgap::TrustSet;
use seaquel_license::server::{Clock, LicenseServer, ServerConfig};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqliteConnection};

// ── Bytes ──

pub fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn b64url_decode(text: &str) -> Vec<u8> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ── Signing ──

/// The seed the TypeScript tests use (bytes 0..=31), which is also the
/// golden-vector seed.
pub fn test_seed() -> SigningKey {
    let seed: [u8; 32] = std::array::from_fn(|i| i as u8);
    SigningKey::from_bytes(&seed)
}

pub fn sign_raw(key: &SigningKey, message: &[u8]) -> Vec<u8> {
    key.sign(message).to_bytes().to_vec()
}

pub struct Signed {
    pub envelope: Vec<u8>,
    pub fingerprint: String,
    pub pubkey: Vec<u8>,
    pub payload_bytes: Vec<u8>,
}

impl Signed {
    pub fn trust(&self) -> TrustSet {
        trust_set(&self.fingerprint, &self.pubkey)
    }

    /// `sha256(canonical payload)`, lowercase hex.
    pub fn payload_sha256(&self) -> String {
        use sha2::Digest;
        hex(&sha2::Sha256::digest(&self.payload_bytes))
    }
}

/// Sign `payload_bytes` as they are and wrap them in an envelope, the way
/// the TypeScript tests' `signLocally` did.
pub fn sign_bytes(payload_bytes: &[u8], key: &SigningKey) -> Signed {
    let sig = sign_raw(key, payload_bytes);
    let pubkey = key.verifying_key().to_bytes().to_vec();
    let fingerprint = fingerprint_pubkey(&pubkey);
    let envelope = json!({
        "payload": b64url(payload_bytes),
        "sig": b64url(&sig),
        "pubkey_fingerprint": fingerprint,
    });
    Signed {
        // serde_json sorts keys; the verifier doesn't care about order.
        envelope: envelope.to_string().into_bytes(),
        fingerprint,
        pubkey,
        payload_bytes: payload_bytes.to_vec(),
    }
}

pub fn sign_canonical(payload: &Value, key: &SigningKey) -> Signed {
    let bytes = canonicalize(&CanonicalValue::from(payload)).unwrap();
    sign_bytes(&bytes, key)
}

pub fn trust_set(fingerprint: &str, pubkey: &[u8]) -> TrustSet {
    let mut t = TrustSet::new();
    t.insert(fingerprint.to_string(), pubkey.to_vec());
    t
}

/// The payload the bundle-store and local-control tests use: issued at
/// 1,700,000,000, with an owner and a member seat.
pub fn bundle_payload(not_after: i64) -> Value {
    json!({
        "version": 1,
        "issued_at": 1_700_000_000,
        "not_before": 1_700_000_000 - 60,
        "not_after": not_after,
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

// ── auth.db ──

pub fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/lib/server/migrations")
}

/// Create `auth.db` in `dir` the way Node's `openAuthDb` does: WAL, then
/// migrations 006–012 in order, each recorded in `_seaquel_auth_migrations`.
pub async fn create_auth_db(dir: &Path) -> PathBuf {
    let path = dir.join("auth.db");
    let mut conn = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .connect()
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _seaquel_auth_migrations (
           name TEXT PRIMARY KEY,
           applied_at INTEGER NOT NULL DEFAULT (unixepoch()))",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let mut files: Vec<_> = std::fs::read_dir(migrations_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    let names: Vec<String> = files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "006_better_auth.sql",
            "007_member_license.sql",
            "008_install_and_validation_cache.sql",
            "009_member_license_is_owner.sql",
            "010_airgap_bundle.sql",
            "011_member_license_revoked_at.sql",
            "012_install_cache_mode.sql",
        ],
        "a new auth migration: check the license server still matches it"
    );
    for (file, name) in files.iter().zip(&names) {
        let sql = std::fs::read_to_string(file).unwrap();
        let mut tx = conn.begin().await.unwrap();
        sqlx::raw_sql(&sql).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO _seaquel_auth_migrations (name) VALUES (?)")
            .bind(name)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    conn.close().await.unwrap();
    path
}

/// A second connection to the same file, standing in for Node's
/// better-sqlite3 handle.
pub async fn node_conn(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5))
        .connect()
        .await
        .unwrap()
}

/// Like [`node_conn`], creating the file.
pub async fn node_conn_create(path: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .connect()
        .await
        .unwrap()
}

pub async fn exec(conn: &mut SqliteConnection, sql: &str) {
    sqlx::raw_sql(sql).execute(conn).await.unwrap();
}

pub async fn seed_user(conn: &mut SqliteConnection, id: &str, email: &str) {
    sqlx::query(
        r#"INSERT INTO "user" (id, name, email, emailVerified, createdAt, updatedAt)
           VALUES (?, ?, ?, 0, datetime('now'), datetime('now'))"#,
    )
    .bind(id)
    .bind(email.split('@').next().unwrap())
    .bind(email)
    .execute(conn)
    .await
    .unwrap();
}

// ── Clock ──

/// A clock the test moves by hand.
#[derive(Clone)]
pub struct TestClock(Arc<AtomicI64>);

impl TestClock {
    pub fn at(now: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now)))
    }
    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
    pub fn advance(&self, secs: i64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
    pub fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    pub fn clock(&self) -> Clock {
        let inner = self.0.clone();
        Arc::new(move || inner.load(Ordering::SeqCst))
    }
}

// ── Fake control plane ──

/// One request the fake saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Clone, Default)]
pub struct FakeControl {
    pub seen: Arc<Mutex<Vec<Seen>>>,
    /// Path → (status, body). A path with no entry answers 404.
    pub replies: Arc<Mutex<HashMap<String, (u16, String)>>>,
}

impl FakeControl {
    pub fn reply(&self, path: &str, status: u16, body: Value) {
        self.replies
            .lock()
            .unwrap()
            .insert(path.to_string(), (status, body.to_string()));
    }
    pub fn reply_text(&self, path: &str, status: u16, body: &str) {
        self.replies
            .lock()
            .unwrap()
            .insert(path.to_string(), (status, body.to_string()));
    }
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }
}

async fn fake_handler(
    axum::extract::State(fake): axum::extract::State<FakeControl>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    body: String,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = uri.path().to_string();
    fake.seen.lock().unwrap().push(Seen {
        method: method.to_string(),
        path: path.clone(),
        headers: headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        body,
    });
    let reply = fake.replies.lock().unwrap().get(&path).cloned();
    match reply {
        Some((status, body)) => (
            axum::http::StatusCode::from_u16(status).unwrap(),
            [("content-type", "application/json")],
            body,
        )
            .into_response(),
        None => (axum::http::StatusCode::NOT_FOUND, "no reply set").into_response(),
    }
}

/// Start a fake control plane; returns its base URL.
pub async fn start_fake_control() -> (String, FakeControl) {
    let fake = FakeControl::default();
    let app = axum::Router::new()
        .fallback(fake_handler)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // The fake runs beside the test; the no-spawn rule is for library code.
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), fake)
}

/// A URL nothing listens on.
pub fn closed_url() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

// ── A whole environment ──

pub struct Env {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
    pub clock: TestClock,
    pub fake: FakeControl,
    pub control_url: String,
    pub signer: Signed,
    pub server: LicenseServer,
}

impl Env {
    /// A fresh auth.db, a fake control plane, the test signer trusted, and
    /// the clock at `now`.
    pub async fn new(now: i64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = create_auth_db(dir.path()).await;
        let (control_url, fake) = start_fake_control().await;
        let clock = TestClock::at(now);
        let signer = sign_canonical(&bundle_payload(now + 86_400), &test_seed());
        let server = LicenseServer::new(Self::config_for(&db, &control_url, &clock, &signer));
        Self {
            dir,
            db,
            clock,
            fake,
            control_url,
            signer,
            server,
        }
    }

    pub fn config_for(
        db: &Path,
        control_url: &str,
        clock: &TestClock,
        signer: &Signed,
    ) -> ServerConfig {
        ServerConfig::new(db)
            .with_control_url(control_url)
            .with_trusted(signer.trust())
            .with_clock(clock.clock())
    }

    /// Another server on the same file (a restart, or a different config).
    pub fn server_with(&self, config: impl FnOnce(ServerConfig) -> ServerConfig) -> LicenseServer {
        LicenseServer::new(config(Self::config_for(
            &self.db,
            &self.control_url,
            &self.clock,
            &self.signer,
        )))
    }

    pub async fn node(&self) -> SqliteConnection {
        node_conn(&self.db).await
    }

    /// Sign `payload` with the test key.
    pub fn sign(&self, payload: &Value) -> Signed {
        sign_canonical(payload, &test_seed())
    }
}
