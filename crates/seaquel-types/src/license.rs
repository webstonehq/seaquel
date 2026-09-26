//! Licensing wire types.
//!
//! [`LicenseResponse`] is what the license server (`/api/licenses/*`) answers
//! the desktop's activate, validate and deactivate calls, passed through to
//! the TypeScript license store unchanged. Its fields are snake_case, as the
//! server sends them.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A license as the license server describes it. Unknown fields in the
/// server's answer are ignored.
///
/// `Debug` never shows `key`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LicenseResponse {
    pub id: String,
    pub status: String,
    pub key: String,
    pub tier: String,
    pub activation: u32,
    pub activation_limit: u32,
    pub expires_at: Option<String>,
    pub instance_id: Option<String>,
}

impl fmt::Debug for LicenseResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LicenseResponse")
            .field("id", &self.id)
            .field("status", &self.status)
            .field("key", &"<redacted>")
            .field("tier", &self.tier)
            .field("activation", &self.activation)
            .field("activation_limit", &self.activation_limit)
            .field("expires_at", &self.expires_at)
            .field("instance_id", &self.instance_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LicenseResponse {
        LicenseResponse {
            id: "lic_1".into(),
            status: "active".into(),
            key: "SQ-SECRET-KEY-1234".into(),
            tier: "business".into(),
            activation: 1,
            activation_limit: 3,
            expires_at: None,
            instance_id: Some("inst_1".into()),
        }
    }

    #[test]
    fn debug_redacts_the_key() {
        let shown = format!("{:?} {:#?}", sample(), sample());
        assert!(!shown.contains("SQ-SECRET-KEY-1234"), "{shown}");
        assert!(shown.contains("<redacted>"));
        assert!(shown.contains("inst_1"));
    }

    #[test]
    fn json_is_the_servers_snake_case() {
        let json = serde_json::to_value(sample()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "lic_1", "status": "active", "key": "SQ-SECRET-KEY-1234",
                "tier": "business", "activation": 1, "activation_limit": 3,
                "expires_at": null, "instance_id": "inst_1"
            })
        );
    }
}
