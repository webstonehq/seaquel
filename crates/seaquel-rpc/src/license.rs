//! The `license` group of the workspace RPC: the desktop's activation calls.
//!
//! The desktop serves it with [`dispatch_license`], which needs no storage.
//! A web workspace has no activation client, so `dispatch_workspace`
//! answers `NOT_SUPPORTED` for it.

use std::fmt;

pub use seaquel_types::license::LicenseResponse;
use serde::{Deserialize, Serialize};

#[cfg(feature = "license-desktop")]
use crate::RpcError;

/// A desktop license call. Fields are camelCase. `Debug` never shows a key.
///
/// Errors keep the activation client's codes: `NETWORK_ERROR`,
/// `ACTIVATION_ERROR`, `VALIDATION_ERROR`, `DEACTIVATION_ERROR` and
/// `PARSE_ERROR`.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DesktopLicenseRequest {
    Activate { key: String, instance_name: String },
    Validate { key: String, instance_id: String },
    Deactivate { key: String, instance_id: String },
}

impl DesktopLicenseRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Activate { .. } => "activate",
            Self::Validate { .. } => "validate",
            Self::Deactivate { .. } => "deactivate",
        }
    }
}

impl fmt::Debug for DesktopLicenseRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const KEY: &str = "<redacted>";
        match self {
            Self::Activate { instance_name, .. } => f
                .debug_struct("Activate")
                .field("key", &KEY)
                .field("instance_name", instance_name)
                .finish(),
            Self::Validate { instance_id, .. } => f
                .debug_struct("Validate")
                .field("key", &KEY)
                .field("instance_id", instance_id)
                .finish(),
            Self::Deactivate { instance_id, .. } => f
                .debug_struct("Deactivate")
                .field("key", &KEY)
                .field("instance_id", instance_id)
                .finish(),
        }
    }
}

/// A desktop license call's result: the license as the server describes it.
/// Its `Debug` hides the key.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DesktopLicenseResponse {
    Activate(LicenseResponse),
    Validate(LicenseResponse),
    Deactivate(LicenseResponse),
}

/// Run one license call on the activation client. The desktop routes the
/// `license` group here, before any storage opens. Logs like
/// `dispatch_workspace`: the method, never the params.
#[cfg(feature = "license-desktop")]
pub async fn dispatch_license(
    client: &seaquel_core::license::desktop::DesktopClient,
    req: DesktopLicenseRequest,
) -> Result<DesktopLicenseResponse, RpcError> {
    let method = req.method();
    crate::workspace::logged("license", method, async move {
        let to_rpc =
            |e: seaquel_core::license::desktop::LicenseError| RpcError::new(e.code, e.message);
        Ok(match req {
            DesktopLicenseRequest::Activate { key, instance_name } => {
                DesktopLicenseResponse::Activate(
                    client
                        .activate(&key, &instance_name)
                        .await
                        .map_err(to_rpc)?,
                )
            }
            DesktopLicenseRequest::Validate { key, instance_id } => {
                DesktopLicenseResponse::Validate(
                    client.validate(&key, &instance_id).await.map_err(to_rpc)?,
                )
            }
            DesktopLicenseRequest::Deactivate { key, instance_id } => {
                DesktopLicenseResponse::Deactivate(
                    client
                        .deactivate(&key, &instance_id)
                        .await
                        .map_err(to_rpc)?,
                )
            }
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "SQ-RPC-SECRET-77";

    #[test]
    fn debug_hides_the_key() {
        for req in [
            DesktopLicenseRequest::Activate {
                key: KEY.into(),
                instance_name: "host__me".into(),
            },
            DesktopLicenseRequest::Validate {
                key: KEY.into(),
                instance_id: "i1".into(),
            },
            DesktopLicenseRequest::Deactivate {
                key: KEY.into(),
                instance_id: "i1".into(),
            },
        ] {
            let shown = format!("{req:?} {req:#?}");
            assert!(!shown.contains(KEY), "{shown}");
            assert!(shown.contains("<redacted>"));
        }
        let res = DesktopLicenseResponse::Validate(LicenseResponse {
            id: "l".into(),
            status: "active".into(),
            key: KEY.into(),
            tier: "business".into(),
            activation: 1,
            activation_limit: 1,
            expires_at: None,
            instance_id: None,
        });
        assert!(!format!("{res:?}").contains(KEY));
    }

    #[test]
    fn params_are_camel_case() {
        let req: DesktopLicenseRequest = serde_json::from_str(
            r#"{"method":"activate","params":{"key":"k","instanceName":"n"}}"#,
        )
        .unwrap();
        assert_eq!(req.method(), "activate");
        assert_eq!(
            serde_json::to_string(&DesktopLicenseRequest::Validate {
                key: "k".into(),
                instance_id: "i".into()
            })
            .unwrap(),
            r#"{"method":"validate","params":{"key":"k","instanceId":"i"}}"#
        );
    }
}
