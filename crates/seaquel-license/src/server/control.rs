//! The control-plane client (`/api/cloud/*`) and the six calls that route to
//! it or, while a bundle is stored, to their air-gap equivalents, ported
//! from `licensing.ts`.
//!
//! Every request carries `X-Install-Id` (this install) and `X-License-Key`
//! (the key whose authority the call asserts): the presented key for
//! `register-install` and `verify-membership-license`, the owner's key (or
//! a first-owner override) for the admin calls. Bodies are JSON with the
//! TypeScript's key order; GETs have none and no content type.
//!
//! The mode is read on every call, never cached.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use super::cache::TenantFields;
use super::{LicenseServer, Result, ServerError, ServerErrorCode};

/// A member's role in the tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Member,
}

/// `bindMember`'s arguments. `Debug` hides both keys.
#[derive(Clone)]
pub struct BindArgs {
    /// The member's key; goes in the body.
    pub license_key: String,
    pub container_user_id: String,
    pub email: String,
    pub role: Role,
    /// Overrides the `X-License-Key` header: the first owner's own key,
    /// before any owner is bound.
    pub auth_key: Option<String>,
}

impl fmt::Debug for BindArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BindArgs")
            .field("license_key", &"<redacted>")
            .field("container_user_id", &self.container_user_id)
            .field("role", &self.role)
            .field("auth_key", &self.auth_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody<'a> {
    install_id: &'a str,
    license_key: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyBody<'a> {
    license_key: &'a str,
    signup_email: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindBody<'a> {
    license_key: &'a str,
    container_user_id: &'a str,
    email: &'a str,
    role: Role,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnbindBody<'a> {
    container_user_id: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindResult {
    tenant_member_id: String,
}

pub(crate) struct ControlClient {
    base: String,
    http: crate::http::LazyClient,
}

/// A control-plane answer: its status and body.
struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl Reply {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json<T: serde::de::DeserializeOwned>(&self, what: &str) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| {
            ServerError::new(
                ServerErrorCode::ControlPlaneError,
                format!("{what}: the control plane's answer isn't valid JSON: {e}"),
            )
        })
    }
}

impl ControlClient {
    pub(crate) fn new(base: &str, extra_ca_file: Option<&std::path::Path>) -> Self {
        // Node's fetch (undici): a 10 s connect timeout, 300 s for headers
        // and body. Never panics: see `crate::http`.
        let http = crate::http::LazyClient::new(
            "control-plane client",
            crate::http::ClientOptions {
                connect_timeout: Some(Duration::from_secs(10)),
                timeout: Some(Duration::from_secs(300)),
                extra_roots: extra_ca_file
                    .map(crate::http::load_extra_roots)
                    .unwrap_or_default(),
            },
        );
        Self {
            base: base.to_string(),
            http,
        }
    }

    /// One request. A failure to reach the control plane (or to read its
    /// answer) is `NETWORK_ERROR`; reqwest's message names the URL, never
    /// a header or the body.
    async fn fetch(
        &self,
        post: bool,
        path: &str,
        install_id: &str,
        license_key: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Reply> {
        let url = format!("{}{path}", self.base);
        let http = self.http.get().map_err(|e| {
            ServerError::new(
                ServerErrorCode::NetworkError,
                format!("control plane unreachable: {e}"),
            )
        })?;
        let mut req = if post {
            http.post(&url)
        } else {
            http.get(&url)
        };
        req = req
            .header("X-Install-Id", install_id)
            .header("X-License-Key", license_key);
        if let Some(body) = body {
            req = req.header("Content-Type", "application/json").body(body);
        }
        let network = |e: reqwest::Error| {
            ServerError::new(
                ServerErrorCode::NetworkError,
                format!("control plane unreachable: {e}"),
            )
        };
        let res = req.send().await.map_err(network)?;
        let status = res.status().as_u16();
        let body = res.bytes().await.map_err(network)?.to_vec();
        Ok(Reply { status, body })
    }
}

fn control_error(message: String) -> ServerError {
    ServerError::new(ServerErrorCode::ControlPlaneError, message)
}

fn json_body(value: &impl Serialize) -> Vec<u8> {
    // Plain structs of strings: serialization can't fail.
    serde_json::to_vec(value).unwrap_or_default()
}

impl LicenseServer {
    async fn control_fetch(
        &self,
        post: bool,
        path: &str,
        license_key: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Reply> {
        let install_id = self.install_id().await?;
        self.control
            .fetch(post, path, &install_id, license_key, body)
            .await
    }

    async fn owner_key_or_fail(&self) -> Result<String> {
        self.owner_license_key()
            .await?
            .ok_or_else(ServerError::no_owner)
    }

    /// `registerInstall`: create the tenant for this install. Doesn't write
    /// the cache itself (the air-gap path does, as before).
    pub async fn register_install_dispatch(&self, license_key: &str) -> Result<TenantFields> {
        if self.is_bundle_driven().await? {
            return Ok(TenantFields::from(
                &self.register_install_local(license_key).await?,
            ));
        }
        let install_id = self.install_id().await?;
        let body = json_body(&RegisterBody {
            install_id: &install_id,
            license_key,
        });
        let reply = self
            .control_fetch(true, "/api/cloud/register-install", license_key, Some(body))
            .await?;
        if !reply.ok() {
            return Err(control_error(format!(
                "register-install failed: {} {}",
                reply.status,
                reply.text()
            )));
        }
        reply.json("register-install")
    }

    /// `tenantInfo`: the tenant as the control plane (or the bundle) has it
    /// now. `None` when there is none (an expired bundle, or a JSON null).
    pub async fn tenant_info(&self) -> Result<Option<TenantFields>> {
        if self.is_bundle_driven().await? {
            return Ok(self
                .local_tenant_info()
                .await?
                .map(|t| TenantFields::from(&t)));
        }
        let owner = self.owner_key_or_fail().await?;
        let reply = self
            .control_fetch(false, "/api/cloud/tenant-info", &owner, None)
            .await?;
        if !reply.ok() {
            if reply.status == 401 {
                return Err(control_error(
                    "control plane rejected license key (revoked or wrong install?)".into(),
                ));
            }
            return Err(control_error(format!(
                "tenant-info failed: {}",
                reply.status
            )));
        }
        reply.json("tenant-info")
    }

    /// `verifyMembershipLicense`: the `VerifyResult` as the control plane
    /// (or the bundle) gives it.
    pub async fn verify_membership(
        &self,
        license_key: &str,
        signup_email: &str,
    ) -> Result<Box<RawValue>> {
        if self.is_bundle_driven().await? {
            return self
                .verify_local_membership_license(license_key, signup_email)
                .await;
        }
        let body = json_body(&VerifyBody {
            license_key,
            signup_email,
        });
        let reply = self
            .control_fetch(
                true,
                "/api/cloud/verify-membership-license",
                license_key,
                Some(body),
            )
            .await?;
        if !reply.ok() {
            return Err(control_error(format!(
                "verify-membership-license failed: {}",
                reply.status
            )));
        }
        reply.json("verify-membership-license")
    }

    /// `bindMember`: the upstream member id for the new row.
    pub async fn bind_member_dispatch(&self, args: &BindArgs) -> Result<String> {
        if self.is_bundle_driven().await? {
            return Ok(super::airgap::local_control::synthetic_tenant_member_id(
                &args.license_key,
            ));
        }
        let auth = match &args.auth_key {
            Some(k) => k.clone(),
            None => self.owner_key_or_fail().await?,
        };
        let body = json_body(&BindBody {
            license_key: &args.license_key,
            container_user_id: &args.container_user_id,
            email: &args.email,
            role: args.role,
        });
        let reply = self
            .control_fetch(true, "/api/cloud/bind-member", &auth, Some(body))
            .await?;
        if !reply.ok() {
            return Err(control_error(format!(
                "bind-member failed: {} {}",
                reply.status,
                reply.text()
            )));
        }
        Ok(reply.json::<BindResult>("bind-member")?.tenant_member_id)
    }

    /// `unbindMember`. A no-op in air-gap mode.
    pub async fn unbind_member_dispatch(&self, container_user_id: &str) -> Result<()> {
        if self.is_bundle_driven().await? {
            return Ok(());
        }
        let owner = self.owner_key_or_fail().await?;
        let body = json_body(&UnbindBody { container_user_id });
        let reply = self
            .control_fetch(true, "/api/cloud/unbind-member", &owner, Some(body))
            .await?;
        if !reply.ok() {
            return Err(control_error(format!(
                "unbind-member failed: {} {}",
                reply.status,
                reply.text()
            )));
        }
        Ok(())
    }

    /// `listMembers`: the `MemberView[]` as the control plane (or the local
    /// rows) give it.
    pub async fn list_members(&self) -> Result<Box<RawValue>> {
        if self.is_bundle_driven().await? {
            return self.list_local_members().await;
        }
        let owner = self.owner_key_or_fail().await?;
        let reply = self
            .control_fetch(false, "/api/cloud/members", &owner, None)
            .await?;
        if !reply.ok() {
            return Err(control_error(format!("members failed: {}", reply.status)));
        }
        reply.json("members")
    }

    /// This install's id: read from `install`, or minted (a v4 UUID) and
    /// stored on first use. Kept in memory afterwards.
    pub async fn install_id(&self) -> Result<String> {
        let mut cached = self.install_id.lock().await;
        if let Some(id) = cached.as_ref() {
            return Ok(id.clone());
        }
        let pool = self.pool().await?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT install_id FROM install WHERE id = 1")
                .fetch_optional(pool)
                .await?;
        let id = match existing {
            Some(id) => id,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                sqlx::query("INSERT INTO install (id, install_id, created_at) VALUES (1, ?, ?)")
                    .bind(&id)
                    .bind(self.now())
                    .execute(pool)
                    .await?;
                id
            }
        };
        *cached = Some(id.clone());
        Ok(id)
    }
}
