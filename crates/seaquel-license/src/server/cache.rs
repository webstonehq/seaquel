//! The persisted validation cache (`install_cache`, one row) and the gate's
//! TTL ladder, ported from `license-cache.ts` and `resolveLicenseState`.
//!
//! - Soft TTL (default 24 h): once it lapses, the gate refreshes from the
//!   control plane (or the bundle).
//! - Grace (default 14 d): how long the last good answer stands while the
//!   refresh keeps failing. Past it, the gate says `revalidate`.

use serde::{Deserialize, Deserializer, Serialize};

use super::{GateState, LicenseServer, LicenseState, Result, TenantContext};
use crate::server::js::parse_int;

/// `parseEnvSeconds`: `parseInt(raw, 10)` when it's positive, else the
/// default.
pub fn parse_env_seconds(raw: Option<&str>, default: i64) -> i64 {
    match raw.filter(|r| !r.is_empty()).and_then(|r| parse_int(r, 10)) {
        Some(n) if n > 0 => n,
        _ => default,
    }
}

/// Whether the install talks to the control plane or runs from a bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Online,
    Airgap,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Online => "online",
            Mode::Airgap => "airgap",
        }
    }
}

/// The `install_cache` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCache {
    pub tenant_id: Option<String>,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tier: Option<String>,
    pub seat_limit: Option<i64>,
    pub current_period_end: Option<String>,
    pub last_validated_at: i64,
    pub grace_until: i64,
    /// Anything but `airgap` reads as `online`, so a bad row can't lock the
    /// install into air-gap mode.
    pub mode: Mode,
}

/// The tenant fields the cache keeps, as the control plane (or a bundle)
/// reports them. Read leniently: a missing field is null, other fields are
/// ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TenantFields {
    pub tenant_id: Option<String>,
    pub slug: Option<String>,
    pub status: Option<String>,
    pub tier: Option<String>,
    #[serde(deserialize_with = "integer_or_null")]
    pub seat_limit: Option<i64>,
    pub current_period_end: Option<String>,
}

/// A JSON number (JavaScript's, so `5.0` is 5) or null.
fn integer_or_null<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let n = Option::<f64>::deserialize(d)?;
    Ok(n.map(|n| n as i64))
}

impl From<&TenantContext> for TenantFields {
    fn from(t: &TenantContext) -> Self {
        Self {
            tenant_id: Some(t.tenant_id.clone()),
            slug: Some(t.slug.clone()),
            status: Some(t.status.clone()),
            tier: Some(t.tier.clone()),
            seat_limit: Some(t.seat_limit),
            current_period_end: t.current_period_end.clone(),
        }
    }
}

type CacheRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    i64,
    i64,
    Option<String>,
);

/// `toState`: `ok` or `suspended` with the cached tenant, or `unregistered`
/// when a field it needs is empty.
fn to_state(cache: &InstallCache) -> LicenseState {
    let non_empty = |v: &Option<String>| v.as_deref().filter(|s| !s.is_empty()).map(str::to_string);
    let (Some(tenant_id), Some(slug), Some(status), Some(tier)) = (
        non_empty(&cache.tenant_id),
        non_empty(&cache.slug),
        non_empty(&cache.status),
        non_empty(&cache.tier),
    ) else {
        return LicenseState {
            kind: GateState::Unregistered,
            tenant: None,
        };
    };
    let kind = if status == "suspended" {
        GateState::Suspended
    } else {
        GateState::Ok
    };
    LicenseState {
        kind,
        tenant: Some(TenantContext {
            tenant_id,
            slug,
            status,
            // Not cached; never used by the gate.
            public_url: String::new(),
            anchor_license_id: String::new(),
            subscription_id: String::new(),
            tier,
            owner_email: String::new(),
            seat_limit: cache.seat_limit.unwrap_or(0),
            current_period_end: cache.current_period_end.clone(),
        }),
    }
}

impl LicenseServer {
    pub async fn read_install_cache(&self) -> Result<Option<InstallCache>> {
        let row: Option<CacheRow> = sqlx::query_as(
            "SELECT tenant_id, slug, status, tier, seat_limit, current_period_end,
                    last_validated_at, grace_until, mode
               FROM install_cache WHERE id = 1",
        )
        .fetch_optional(self.pool().await?)
        .await?;
        Ok(row.map(|r| InstallCache {
            tenant_id: r.0,
            slug: r.1,
            status: r.2,
            tier: r.3,
            seat_limit: r.4,
            current_period_end: r.5,
            last_validated_at: r.6,
            grace_until: r.7,
            mode: if r.8.as_deref() == Some("airgap") {
                Mode::Airgap
            } else {
                Mode::Online
            },
        }))
    }

    /// Record a successful validation now: the tenant, `last_validated_at =
    /// now` and `grace_until = now + grace`.
    pub async fn write_install_cache(&self, t: &TenantFields, mode: Mode) -> Result<()> {
        let now = self.now();
        sqlx::query(
            "INSERT INTO install_cache
               (id, tenant_id, slug, status, tier, seat_limit,
                current_period_end, last_validated_at, grace_until, mode)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               tenant_id          = excluded.tenant_id,
               slug               = excluded.slug,
               status             = excluded.status,
               tier               = excluded.tier,
               seat_limit         = excluded.seat_limit,
               current_period_end = excluded.current_period_end,
               last_validated_at  = excluded.last_validated_at,
               grace_until        = excluded.grace_until,
               mode               = excluded.mode",
        )
        .bind(&t.tenant_id)
        .bind(&t.slug)
        .bind(&t.status)
        .bind(&t.tier)
        .bind(t.seat_limit)
        .bind(&t.current_period_end)
        .bind(now)
        .bind(now.saturating_add(self.config.grace_ttl))
        .bind(mode.as_str())
        .execute(self.pool().await?)
        .await?;
        Ok(())
    }

    /// Switch the mode. Before signup there is no row, so one is made with
    /// no tenant and the current timestamps (a later validation fills it).
    pub async fn set_mode(&self, mode: Mode) -> Result<()> {
        let pool = self.pool().await?;
        let changed = sqlx::query("UPDATE install_cache SET mode = ? WHERE id = 1")
            .bind(mode.as_str())
            .execute(pool)
            .await?
            .rows_affected();
        if changed > 0 {
            return Ok(());
        }
        let now = self.now();
        sqlx::query(
            "INSERT INTO install_cache
               (id, tenant_id, slug, status, tier, seat_limit,
                current_period_end, last_validated_at, grace_until, mode)
             VALUES (1, NULL, NULL, NULL, NULL, NULL, NULL, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET mode = excluded.mode",
        )
        .bind(now)
        .bind(now.saturating_add(self.config.grace_ttl))
        .bind(mode.as_str())
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Stamp the validation-cache columns on `member_license` rows that
    /// predate them, so an upgrade locks no one out. Only rows with a NULL
    /// `last_validated_at` change.
    pub(crate) async fn backfill_existing_members(&self, pool: &sqlx::SqlitePool) -> Result<()> {
        let now = self.now();
        sqlx::query(
            "UPDATE member_license
                SET last_validated_at = COALESCE(last_validated_at, ?),
                    grace_until       = COALESCE(grace_until, ?),
                    cached_status     = COALESCE(cached_status, 'active')
              WHERE last_validated_at IS NULL",
        )
        .bind(now)
        .bind(now.saturating_add(self.config.grace_ttl))
        .execute(pool)
        .await?;
        Ok(())
    }

    /// The gate's ladder:
    ///
    /// 1. no row, or no tenant: `unregistered`;
    /// 2. validated less than the soft TTL ago: the cached state, no call;
    /// 3. otherwise refresh (`tenantInfo`); on success write and answer the
    ///    new state;
    /// 4. on any failure (or no tenant), the cached state until
    ///    `grace_until`, then `revalidate`.
    pub async fn resolve_license_state(&self) -> Result<LicenseState> {
        let Some(cache) = self.read_install_cache().await? else {
            return Ok(LicenseState {
                kind: GateState::Unregistered,
                tenant: None,
            });
        };
        if cache.tenant_id.is_none() {
            return Ok(LicenseState {
                kind: GateState::Unregistered,
                tenant: None,
            });
        }

        let now = self.now();
        if now.saturating_sub(cache.last_validated_at) < self.config.soft_ttl {
            return Ok(to_state(&cache));
        }

        match self.refresh().await {
            Ok(Some(refreshed)) => return Ok(to_state(&refreshed)),
            Ok(None) => {}
            Err(e) => {
                log::warn!(activity = "license.refresh", code = e.code.as_str(); "License refresh failed: {}", e.message);
            }
        }

        if now < cache.grace_until {
            return Ok(to_state(&cache));
        }
        Ok(LicenseState {
            kind: GateState::Revalidate,
            tenant: None,
        })
    }

    /// Fetch the tenant, write it, and read the row back. `None` when there
    /// was no tenant to write.
    async fn refresh(&self) -> Result<Option<InstallCache>> {
        let Some(fresh) = self.tenant_info().await? else {
            return Ok(None);
        };
        // An answer without a tenant id is a failed refresh, not a reason to
        // wipe the cached tenant (the TS wrote it, turning the install
        // `unregistered`). The grace rung then decides.
        if fresh.tenant_id.as_deref().is_none_or(str::is_empty) {
            return Err(super::ServerError::new(
                super::ServerErrorCode::ControlPlaneError,
                "tenant-info returned no tenantId",
            ));
        }
        let mode = if self.is_bundle_driven().await? {
            Mode::Airgap
        } else {
            Mode::Online
        };
        self.write_install_cache(&fresh, mode).await?;
        self.read_install_cache().await
    }
}
