//! The air-gap answers to the control-plane calls, ported from
//! `local-control.ts`. The bundle is the only ground truth; the one side
//! effect is `register_install_local` writing the install cache.

use serde::Serialize;
use serde_json::value::RawValue;

use super::canonical::sha256_hex;
use super::verify::{BundlePayload, SeatRole};
use crate::server::cache::{Mode, TenantFields};
use crate::server::js::{iso_string, utf16_len, utf16_tail};
use crate::server::{LicenseServer, Result, ServerError, ServerErrorCode, TenantContext};

/// The control plane's default air-gap grace (seaquel-app's
/// `DEFAULT_AIRGAP_GRACE_SECONDS`): it bakes `not_after = currentPeriodEnd
/// + grace` into each bundle, so subtracting it recovers the period end.
pub const AIRGAP_GRACE_SECONDS: i64 = 2_592_000;

/// The bundle as a tenant: `airgap-<subscription>`, active, the bundle's
/// tier and seats, and the recovered period end (null at or before the
/// epoch).
pub fn project_tenant_context(payload: &BundlePayload) -> TenantContext {
    let period_end = payload.not_after - AIRGAP_GRACE_SECONDS as f64;
    let current_period_end = if period_end > 0.0 {
        iso_string(period_end * 1000.0)
    } else {
        None
    };
    TenantContext {
        tenant_id: format!("airgap-{}", payload.subscription_id),
        slug: payload.tenant_slug.clone(),
        status: "active".into(),
        public_url: String::new(),
        anchor_license_id: String::new(),
        subscription_id: payload.subscription_id.clone(),
        tier: payload.tier.clone(),
        owner_email: String::new(),
        seat_limit: payload.seats as i64,
        current_period_end,
    }
}

/// `local-` and the first 16 hex characters of `sha256(key)`: the same key
/// always gets the same id.
pub fn synthetic_tenant_member_id(license_key: &str) -> String {
    format!("local-{}", &sha256_hex(license_key.as_bytes())[..16])
}

/// `••••-` and the last four UTF-16 units; keys of four or fewer are shown
/// as they are, and an empty key stays empty.
pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    if utf16_len(key) <= 4 {
        return key.to_string();
    }
    format!("••••-{}", utf16_tail(key, 4))
}

/// `VerifyResult`, in the TypeScript's key order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Verified<'a> {
    ok: bool,
    subscription_id: &'a str,
    tier: &'a str,
    role: SeatRole,
}

#[derive(Serialize)]
struct Refused<'a> {
    ok: bool,
    error: &'a str,
}

/// `MemberView`, in the TypeScript's key order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    tenant_member_id: String,
    container_user_id: String,
    email: String,
    role: &'static str,
    bound_at: Option<String>,
    masked_license_key: String,
}

pub(crate) fn raw(value: &impl Serialize) -> Result<Box<RawValue>> {
    serde_json::value::to_raw_value(value).map_err(|e| {
        ServerError::new(
            ServerErrorCode::DbError,
            format!("couldn't encode an answer: {e}"),
        )
    })
}

fn refused(error: &str) -> Result<Box<RawValue>> {
    raw(&Refused { ok: false, error })
}

impl LicenseServer {
    /// Air-gap signup: the key must be the bundle's owner seat. Writes the
    /// install cache in air-gap mode.
    pub async fn register_install_local(&self, license_key: &str) -> Result<TenantContext> {
        let Some(bundle) = self.read_active_bundle().await? else {
            return Err(ServerError::new(
                ServerErrorCode::NoAirgapBundle,
                "no_airgap_bundle",
            ));
        };
        let owner = bundle
            .payload
            .seat_tokens
            .iter()
            .find(|t| t.role == SeatRole::Owner);
        if owner.is_none_or(|o| o.key != license_key) {
            return Err(ServerError::new(
                ServerErrorCode::LicenseNotFound,
                "license_not_found",
            ));
        }
        let tenant = project_tenant_context(&bundle.payload);
        self.write_install_cache(&TenantFields::from(&tenant), Mode::Airgap)
            .await?;
        Ok(tenant)
    }

    /// The bundle as a tenant, or `None` without one or past its
    /// `not_after`. No side effects.
    pub async fn local_tenant_info(&self) -> Result<Option<TenantContext>> {
        let Some(bundle) = self.read_active_bundle().await? else {
            return Ok(None);
        };
        if self.now() as f64 > bundle.payload.not_after {
            return Ok(None);
        }
        Ok(Some(project_tenant_context(&bundle.payload)))
    }

    /// `verifyMembershipLicense` from the bundle. The email is unused
    /// offline.
    pub async fn verify_local_membership_license(
        &self,
        license_key: &str,
        _signup_email: &str,
    ) -> Result<Box<RawValue>> {
        let Some(bundle) = self.read_active_bundle().await? else {
            return refused("license_not_found");
        };
        if bundle.payload.revoked_keys.iter().any(|k| k == license_key) {
            return refused("license_inactive");
        }
        // Placeholders that pad a bundle to its seat count aren't seats.
        if license_key.starts_with("airgap-vacant-") {
            return refused("license_not_found");
        }
        let Some(seat) = bundle
            .payload
            .seat_tokens
            .iter()
            .find(|t| t.key == license_key)
        else {
            return refused("license_not_found");
        };
        if self
            .find_member_by_license_key(license_key)
            .await?
            .is_some()
        {
            return refused("license_already_in_other_tenant");
        }
        raw(&Verified {
            ok: true,
            subscription_id: &bundle.payload.subscription_id,
            tier: &bundle.payload.tier,
            role: seat.role,
        })
    }

    /// The local members for `/settings/team`: bound, not revoked, oldest
    /// first, with masked keys.
    pub async fn list_local_members(&self) -> Result<Box<RawValue>> {
        let rows = self.active_members().await?;
        let views: Vec<MemberView> = rows
            .into_iter()
            .map(|(user_id, key, bound_at, is_owner, email)| MemberView {
                tenant_member_id: synthetic_tenant_member_id(&key),
                container_user_id: user_id,
                email,
                role: if is_owner == 1 { "owner" } else { "member" },
                bound_at: iso_string(bound_at as f64 * 1000.0),
                masked_license_key: mask_key(&key),
            })
            .collect();
        raw(&views)
    }
}
