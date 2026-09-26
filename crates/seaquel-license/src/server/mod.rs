//! The self-hosted web build's licensing, ported from the Node server's
//! `licensing.ts`, `license-cache.ts`, `member-license.ts`, `install.ts` and
//! `airgap/*`.
//!
//! [`LicenseServer`] owns every read and write of the license tables in
//! `auth.db` (`member_license`, `install`, `install_cache`,
//! `airgap_bundle`). Node keeps Better Auth and applies migrations 006–012;
//! until it has, every call answers `NOT_READY`. Rust never creates the
//! file. Both processes keep it open at once (WAL, busy timeout).
//!
//! What it does:
//!
//! - **The install id** ([`LicenseServer::install_id`]), minted once.
//! - **The control-plane client**: `/api/cloud/*` with `X-Install-Id` and
//!   `X-License-Key`, the same requests as before. Every call routes to the
//!   air-gap equivalent instead while a bundle is stored.
//! - **The gate** ([`LicenseServer::gate`]): the soft/hard TTL ladder (24 h
//!   and 14 d, `SEAQUEL_LICENSE_SOFT_TTL` / `SEAQUEL_LICENSE_GRACE_TTL`),
//!   the user's membership row, and the install facts the layout redirects
//!   read.
//! - **Signup, team and air-gap steps**, one method each, called by Node in
//!   the order its routes run them.
//!
//! License keys never appear in a log line, an error message or `Debug`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;

pub mod airgap;
mod cache;
mod control;
mod db;
pub(crate) mod js;
mod member;

pub use airgap::bundle_store::ActiveBundle;
pub use cache::{parse_env_seconds, InstallCache, Mode, TenantFields};
pub use control::{BindArgs, Role};
pub use member::MemberLicense;

use airgap::verify::{verify_bundle, BundleVerifyError};
use airgap::TrustSet;

// ── Configuration ──

/// Unix seconds now. Tests pass a clock they move by hand.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The real clock.
pub fn system_clock() -> Clock {
    // The one place this crate reads the wall clock (the clippy rule is for
    // wasm32 Core crates; this one is native only).
    #[allow(clippy::disallowed_types, clippy::disallowed_methods)]
    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
    Arc::new(now)
}

pub const DEFAULT_CONTROL_URL: &str = "https://seaquel.app";
pub const DEFAULT_SOFT_TTL_SECONDS: i64 = 24 * 60 * 60;
pub const DEFAULT_GRACE_TTL_SECONDS: i64 = 14 * 24 * 60 * 60;

/// The env vars the Node server read, which `server.js` passes through.
pub const CONTROL_URL_ENV: &str = "SEAQUEL_CONTROL_URL";
pub const SOFT_TTL_ENV: &str = "SEAQUEL_LICENSE_SOFT_TTL";
pub const GRACE_TTL_ENV: &str = "SEAQUEL_LICENSE_GRACE_TTL";
pub const TRUSTED_PUBKEY_ENV: &str = "SEAQUEL_BUNDLE_TRUSTED_PUBKEY";
/// Node's variable for extra CA certificates, which its fetch honoured for
/// the control-plane calls; the Rust client honours it too.
pub const EXTRA_CA_CERTS_ENV: &str = "NODE_EXTRA_CA_CERTS";

/// How a [`LicenseServer`] is set up. Build it with [`ServerConfig::new`]
/// or [`ServerConfig::from_env`] and the `with_*` methods.
#[derive(Clone)]
pub struct ServerConfig {
    /// `DATA_DIR/auth.db`.
    pub auth_db: PathBuf,
    /// The control plane, without a trailing `/`.
    pub control_url: String,
    pub soft_ttl: i64,
    pub grace_ttl: i64,
    /// The keys air-gap bundles may be signed with.
    pub trusted: TrustSet,
    pub clock: Clock,
    /// A PEM file of CA certificates to trust on top of the built-in roots
    /// (`NODE_EXTRA_CA_CERTS`): a TLS-inspecting proxy's CA. Unreadable or
    /// rejected certificates are skipped with a warning.
    pub extra_ca_file: Option<PathBuf>,
}

impl ServerConfig {
    /// The defaults: seaquel.app, 24 h and 14 d, the production trust set
    /// and the system clock.
    pub fn new(auth_db: impl Into<PathBuf>) -> Self {
        Self {
            auth_db: auth_db.into(),
            control_url: DEFAULT_CONTROL_URL.to_string(),
            soft_ttl: DEFAULT_SOFT_TTL_SECONDS,
            grace_ttl: DEFAULT_GRACE_TTL_SECONDS,
            trusted: airgap::bundle_store::load_trusted_pubkeys(None),
            clock: system_clock(),
            extra_ca_file: None,
        }
    }

    /// As the Node server read its environment: `SEAQUEL_CONTROL_URL`
    /// (one trailing `/` dropped), the two TTLs (`parseInt`, used when
    /// positive) and `SEAQUEL_BUNDLE_TRUSTED_PUBKEY` on top of the
    /// production keys. Read once: the server's environment doesn't change
    /// while it runs.
    pub fn from_env(auth_db: impl Into<PathBuf>) -> Self {
        let var = |k: &str| std::env::var(k).ok();
        let mut c = Self::new(auth_db);
        if let Some(url) = var(CONTROL_URL_ENV) {
            c = c.with_control_url(&url);
        }
        c.soft_ttl = parse_env_seconds(var(SOFT_TTL_ENV).as_deref(), DEFAULT_SOFT_TTL_SECONDS);
        c.grace_ttl = parse_env_seconds(var(GRACE_TTL_ENV).as_deref(), DEFAULT_GRACE_TTL_SECONDS);
        c.trusted = airgap::bundle_store::load_trusted_pubkeys(var(TRUSTED_PUBKEY_ENV).as_deref());
        c.extra_ca_file = var(EXTRA_CA_CERTS_ENV)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from);
        c
    }

    /// `url` with one trailing `/` dropped, as `controlUrl()` did.
    #[must_use]
    pub fn with_control_url(mut self, url: &str) -> Self {
        self.control_url = url.strip_suffix('/').unwrap_or(url).to_string();
        self
    }

    #[must_use]
    pub fn with_ttls(mut self, soft: i64, grace: i64) -> Self {
        self.soft_ttl = soft;
        self.grace_ttl = grace;
        self
    }

    #[must_use]
    pub fn with_trusted(mut self, trusted: TrustSet) -> Self {
        self.trusted = trusted;
        self
    }

    #[must_use]
    pub fn with_extra_ca_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.extra_ca_file = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("auth_db", &self.auth_db)
            .field("control_url", &self.control_url)
            .field("soft_ttl", &self.soft_ttl)
            .field("grace_ttl", &self.grace_ttl)
            .field("trusted", &self.trusted.keys().collect::<Vec<_>>())
            .field("extra_ca_file", &self.extra_ca_file)
            .finish()
    }
}

// ── Errors ──

/// What went wrong, as a code Node can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerErrorCode {
    /// auth.db or its license tables don't exist yet: Node hasn't opened it.
    NotReady,
    /// The control plane couldn't be reached (the fetch itself failed).
    NetworkError,
    /// The control plane answered with an error or something unreadable.
    ControlPlaneError,
    /// A call that authenticates as the owner, with no owner bound.
    NoOwner,
    /// Air-gap signup without a bundle.
    NoAirgapBundle,
    /// Air-gap signup with a key that isn't the bundle's owner seat.
    LicenseNotFound,
    /// A step that needs a signed-in user got none.
    Unauthorized,
    /// A step that needs the owner got someone else.
    Forbidden,
    InvalidArgument,
    /// Anything else from auth.db.
    DbError,
}

impl ServerErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotReady => "NOT_READY",
            Self::NetworkError => "NETWORK_ERROR",
            Self::ControlPlaneError => "CONTROL_PLANE_ERROR",
            Self::NoOwner => "NO_OWNER",
            Self::NoAirgapBundle => "NO_AIRGAP_BUNDLE",
            Self::LicenseNotFound => "LICENSE_NOT_FOUND",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::DbError => "LICENSE_DB_ERROR",
        }
    }

    /// The HTTP status `/internal/license/*` answers with.
    pub fn http_status(self) -> u16 {
        match self {
            Self::NotReady => 503,
            Self::NetworkError | Self::ControlPlaneError => 502,
            Self::NoOwner | Self::NoAirgapBundle => 409,
            Self::LicenseNotFound | Self::InvalidArgument => 400,
            Self::Unauthorized => 401,
            Self::Forbidden => 403,
            Self::DbError => 500,
        }
    }
}

/// A failed license call. The message never holds a license key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct ServerError {
    pub code: ServerErrorCode,
    pub message: String,
}

impl ServerError {
    pub fn new(code: ServerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// `isNetworkFailure`: the control plane was never reached.
    pub fn is_network_failure(&self) -> bool {
        self.code == ServerErrorCode::NetworkError
    }

    fn no_owner() -> Self {
        Self::new(
            ServerErrorCode::NoOwner,
            "no owner license bound — call registerInstall first",
        )
    }
}

impl From<sqlx::Error> for ServerError {
    fn from(e: sqlx::Error) -> Self {
        // sqlx's messages name SQL and SQLite errors, never bound values.
        let message = e.to_string();
        if message.contains("no such table") || message.contains("no such column") {
            Self::new(
                ServerErrorCode::NotReady,
                format!("auth.db isn't ready: {message}"),
            )
        } else {
            Self::new(ServerErrorCode::DbError, message)
        }
    }
}

pub type Result<T, E = ServerError> = std::result::Result<T, E>;

// ── Answers ──

/// A tenant as the gate reports it (`TenantContext` in TypeScript).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantContext {
    pub tenant_id: String,
    pub slug: String,
    pub status: String,
    pub public_url: String,
    pub anchor_license_id: String,
    pub subscription_id: String,
    pub tier: String,
    pub owner_email: String,
    pub seat_limit: i64,
    pub current_period_end: Option<String>,
}

/// The gate's rungs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateState {
    Ok,
    Suspended,
    /// Past the grace window with the control plane unreachable.
    Revalidate,
    /// No owner has registered the install yet.
    Unregistered,
}

/// `resolveLicenseState`'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseState {
    pub kind: GateState,
    /// For `ok` and `suspended`.
    pub tenant: Option<TenantContext>,
}

/// A user's membership row, without the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberState {
    pub is_owner: bool,
    /// Revoked by a bundle import: counts as not bound for the API gate.
    pub revoked: bool,
}

/// Everything the Node hooks and the `(app)` layout read per request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateAnswer {
    pub state: GateState,
    pub tenant: Option<TenantContext>,
    /// The user's row, when a user was named and has one.
    pub member: Option<MemberState>,
    /// `install_cache.tenant_id` is set (non-empty).
    pub has_tenant: bool,
    /// An air-gap bundle row exists (not re-verified).
    pub bundle_present: bool,
}

/// The two install facts the signup route reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStatus {
    pub has_tenant: bool,
    pub bundle_present: bool,
}

/// The signup route's bind step. `Debug` hides the key.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindMemberRequest {
    pub license_key: String,
    pub user_id: String,
    pub email: String,
    pub role: Role,
    /// This signup registered the install: the key authenticates the bind
    /// and the row is the owner's.
    pub first_owner: bool,
}

impl fmt::Debug for BindMemberRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindMemberRequest")
            .field("license_key", &"<redacted>")
            .field("user_id", &self.user_id)
            .field("role", &self.role)
            .field("first_owner", &self.first_owner)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindOutcome {
    /// False when the user already had a row (nothing was done).
    pub bound: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnbindOutcome {
    /// The upstream unbind worked but deleting the local row didn't (logged).
    pub local_cleanup_failed: bool,
}

/// The bundle fields the status views show.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleStatus {
    pub tier: String,
    pub seats: Value,
    pub not_after: Value,
    pub issued_at: Value,
    pub imported_at: i64,
    pub pubkey_fingerprint: String,
    pub payload_sha256: String,
    pub revoked_key_count: usize,
    pub expired: bool,
}

/// What `/revalidate`, `/settings/airgap` and `GET /api/airgap/bundle`
/// read.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AirgapStatus {
    /// `online` when there's no cache row.
    pub mode: Mode,
    pub last_validated_at: Option<i64>,
    pub grace_until: Option<i64>,
    pub member: Option<MemberState>,
    /// The active (re-verified) bundle.
    pub bundle: Option<BundleStatus>,
}

/// The result of a bundle upload, for Node to send on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadOutcome {
    /// 200, 400 or 409.
    pub status: u16,
    /// The JSON body `/api/airgap/bundle` answers with, in the TypeScript's
    /// key order.
    pub body: Box<RawValue>,
    /// Every user whose row is revoked (after the auth check; empty
    /// before it): Node purges their sessions. All of them, not only this
    /// import's, so a purge that failed last time happens on a retry.
    pub revoked_user_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadAccepted<'a> {
    ok: bool,
    unchanged: bool,
    tier: &'a str,
    seats: Value,
    not_after: Value,
    issued_at: Value,
    pubkey_fingerprint: &'a str,
    revoked_key_count: usize,
    rows_revoked: usize,
}

#[derive(Serialize)]
struct UploadRefused<'a> {
    ok: bool,
    error: &'a str,
}

// ── The server ──

/// The license gate and everything behind it. One per process; cheap to
/// share behind an `Arc`.
pub struct LicenseServer {
    config: ServerConfig,
    db: db::AuthDb,
    control: control::ControlClient,
    install_id: tokio::sync::Mutex<Option<String>>,
    bundle_cache: std::sync::Mutex<Option<Arc<ActiveBundle>>>,
}

impl fmt::Debug for LicenseServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LicenseServer")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl LicenseServer {
    /// Nothing is opened until the first call.
    pub fn new(config: ServerConfig) -> Self {
        Self {
            db: db::AuthDb::new(&config.auth_db),
            control: control::ControlClient::new(
                &config.control_url,
                config.extra_ca_file.as_deref(),
            ),
            config,
            install_id: tokio::sync::Mutex::new(None),
            bundle_cache: std::sync::Mutex::new(None),
        }
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub fn auth_db_path(&self) -> &Path {
        &self.config.auth_db
    }

    pub(crate) fn now(&self) -> i64 {
        (self.config.clock)()
    }

    /// The pool, once auth.db has the license tables. The first time it
    /// does, `backfillExistingMembers` runs (moved here from Node's
    /// `openAuthDb`).
    pub(crate) async fn pool(&self) -> Result<&sqlx::SqlitePool> {
        self.db
            .ready(|pool| async move { self.backfill_existing_members(pool).await })
            .await
    }

    // ── The gate ──

    /// The answer the Node hooks cache for 5 seconds per user: the TTL
    /// ladder's state, the user's membership row and the install facts the
    /// `(app)` layout redirects on, in the order the hooks read them.
    pub async fn gate(&self, user_id: Option<&str>) -> Result<GateAnswer> {
        let state = self.resolve_license_state().await?;
        let member = match user_id {
            Some(id) => self.find_member(id).await?.map(|m| m.state()),
            None => None,
        };
        let has_tenant = self.has_tenant().await?;
        let bundle_present = self.is_bundle_driven().await?;
        Ok(GateAnswer {
            state: state.kind,
            tenant: state.tenant,
            member,
            has_tenant,
            bundle_present,
        })
    }

    /// Whether the install has a tenant and whether a bundle is stored,
    /// read now (the signup route's `isFirstOwner` and `isBundleDriven`).
    pub async fn install_status(&self) -> Result<InstallStatus> {
        Ok(InstallStatus {
            has_tenant: self.has_tenant().await?,
            bundle_present: self.is_bundle_driven().await?,
        })
    }

    async fn has_tenant(&self) -> Result<bool> {
        Ok(self
            .read_install_cache()
            .await?
            .is_some_and(|c| c.tenant_id.is_some_and(|t| !t.is_empty())))
    }

    // ── Signup ──

    /// First-owner signup: register the install (online, or from the
    /// bundle) and write the cache in the mode that's current then.
    pub async fn register_install(&self, license_key: &str) -> Result<()> {
        let registered = self.register_install_dispatch(license_key).await?;
        let mode = if self.is_bundle_driven().await? {
            Mode::Airgap
        } else {
            Mode::Online
        };
        self.write_install_cache(&registered, mode).await
    }

    /// Check a signup's key (`verifyMembershipLicense`); the answer is the
    /// control plane's (or the bundle's) `VerifyResult` as is.
    pub async fn signup_check(&self, license_key: &str, email: &str) -> Result<Box<RawValue>> {
        self.verify_membership(license_key, email).await
    }

    /// Bind a new user: nothing if they already have a row, otherwise
    /// `bindMember` upstream (authenticated by their key on the first-owner
    /// signup) and then the local row.
    pub async fn bind_member(&self, req: &BindMemberRequest) -> Result<BindOutcome> {
        if self.find_member(&req.user_id).await?.is_some() {
            return Ok(BindOutcome { bound: false });
        }
        let control_member_id = self
            .bind_member_dispatch(&BindArgs {
                license_key: req.license_key.clone(),
                container_user_id: req.user_id.clone(),
                email: req.email.clone(),
                role: req.role,
                auth_key: req.first_owner.then(|| req.license_key.clone()),
            })
            .await?;
        self.insert_member(
            &req.user_id,
            &req.license_key,
            self.now(),
            &control_member_id,
            req.first_owner,
        )
        .await?;
        Ok(BindOutcome { bound: true })
    }

    // ── Team ──

    /// Remove a member upstream, then their local row. A failure of the
    /// local delete is logged and reported, not an error: upstream already
    /// removed them.
    pub async fn unbind_member(&self, user_id: &str) -> Result<UnbindOutcome> {
        self.unbind_member_dispatch(user_id).await?;
        let local_cleanup_failed = match self.delete_member(user_id).await {
            Ok(()) => false,
            Err(e) => {
                log::error!(activity = "license.unbind", code = e.code.as_str(); "Local member cleanup failed (upstream still removed): {}", e.message);
                true
            }
        };
        Ok(UnbindOutcome {
            local_cleanup_failed,
        })
    }

    // ── Air gap ──

    /// The mode, TTL timestamps, the user's row and the active bundle.
    pub async fn airgap_status(&self, user_id: Option<&str>) -> Result<AirgapStatus> {
        let cache = self.read_install_cache().await?;
        let member = match user_id {
            Some(id) => self.find_member(id).await?.map(|m| m.state()),
            None => None,
        };
        let now = self.now();
        let bundle = self.read_active_bundle().await?.map(|b| BundleStatus {
            tier: b.payload.tier.clone(),
            seats: js::number_value(b.payload.seats),
            not_after: js::number_value(b.payload.not_after),
            issued_at: js::number_value(b.payload.issued_at),
            imported_at: b.imported_at,
            pubkey_fingerprint: b.pubkey_fingerprint.clone(),
            payload_sha256: b.payload_sha256.clone(),
            revoked_key_count: b.payload.revoked_keys.len(),
            expired: now as f64 > b.payload.not_after,
        });
        Ok(AirgapStatus {
            mode: cache.as_ref().map_or(Mode::Online, |c| c.mode),
            last_validated_at: cache.as_ref().map(|c| c.last_validated_at),
            grace_until: cache.as_ref().map(|c| c.grace_until),
            member,
            bundle,
        })
    }

    /// `POST /api/airgap/bundle` after its origin and rate-limit checks:
    /// verify, then the auth rule (anyone on a fresh install, the owner once
    /// a tenant exists), the replay and subscription checks, then the bundle
    /// row and the revocations in one transaction, then air-gap mode.
    pub async fn airgap_upload(
        &self,
        envelope: &[u8],
        user_id: Option<&str>,
    ) -> Result<UploadOutcome> {
        use airgap::local_control::raw;
        let fail = |status: u16, error: &str| -> Result<UploadOutcome> {
            Ok(UploadOutcome {
                status,
                body: raw(&UploadRefused { ok: false, error })?,
                revoked_user_ids: Vec::new(),
            })
        };
        if envelope.is_empty() {
            return fail(400, BundleVerifyError::MalformedEnvelope.as_str());
        }
        let verified = match verify_bundle(envelope, &self.config.trusted, self.now()) {
            Ok(v) => v,
            Err(e) => return fail(400, e.as_str()),
        };

        if self.has_tenant().await? {
            self.require_owner(user_id).await?;
        }

        // From here on every answer lists every revoked user, not only this
        // import's: if Node's session purge failed after an earlier import
        // committed, the retry (which answers `unchanged`) purges them then.
        let refuse = |status: u16, error: &str, revoked_user_ids: Vec<String>| {
            Ok(UploadOutcome {
                status,
                body: raw(&UploadRefused { ok: false, error })?,
                revoked_user_ids,
            })
        };

        let current = self.read_active_bundle().await?;
        if let Some(current) = &current {
            if verified.payload.issued_at < current.payload.issued_at {
                return refuse(
                    409,
                    "bundle_older_than_current",
                    self.revoked_user_ids().await?,
                );
            }
        }
        let payload_sha256 = verified.payload.sha256_hex();
        let p = &verified.payload;
        let body = |unchanged: bool, rows_revoked: usize| {
            raw(&UploadAccepted {
                ok: true,
                unchanged,
                tier: &p.tier,
                seats: js::number_value(p.seats),
                not_after: js::number_value(p.not_after),
                issued_at: js::number_value(p.issued_at),
                pubkey_fingerprint: &verified.pubkey_fingerprint,
                revoked_key_count: p.revoked_keys.len(),
                rows_revoked,
            })
        };
        if let Some(current) = &current {
            if current.payload_sha256 == payload_sha256 {
                return Ok(UploadOutcome {
                    status: 200,
                    body: body(true, 0)?,
                    revoked_user_ids: self.revoked_user_ids().await?,
                });
            }
            if current.payload.subscription_id != p.subscription_id {
                return refuse(409, "subscription_mismatch", self.revoked_user_ids().await?);
            }
        }

        let newly_revoked = self.import_bundle(&verified, &payload_sha256).await?;
        self.set_mode(Mode::Airgap).await?;
        Ok(UploadOutcome {
            status: 200,
            body: body(false, newly_revoked.len())?,
            revoked_user_ids: self.revoked_user_ids().await?,
        })
    }

    /// `DELETE /api/airgap/bundle`: the owner only; back to online mode.
    /// Member rows stay.
    pub async fn airgap_clear(&self, user_id: Option<&str>) -> Result<()> {
        self.require_owner(user_id).await?;
        self.clear_bundle().await?;
        self.set_mode(Mode::Online).await
    }

    /// Signed in, with a live owner row: else 401 / 403, with the route's
    /// messages.
    async fn require_owner(&self, user_id: Option<&str>) -> Result<()> {
        let user_id = user_id
            .ok_or_else(|| ServerError::new(ServerErrorCode::Unauthorized, "must be signed in"))?;
        match self.find_member(user_id).await? {
            Some(m) if m.is_owner && m.revoked_at.is_none() => Ok(()),
            _ => Err(ServerError::new(ServerErrorCode::Forbidden, "owner only")),
        }
    }
}
