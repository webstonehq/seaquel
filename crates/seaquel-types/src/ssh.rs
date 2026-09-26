//! SSH tunnel wire types: what the desktop app sends to open a tunnel
//! (`seaquel-rpc`'s `Ssh::Open`) and what it gets back.
//!
//! [`TunnelConfig`] carries a password or a key passphrase, so its `Debug`
//! redacts both.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How to reach the database through an SSH server. camelCase on the wire.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TunnelConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_username: String,
    /// `"password"` or `"key"`. Anything else fails with
    /// `INVALID_AUTH_METHOD`.
    pub auth_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub password: Option<String>,
    /// A private key file on the machine running Core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub key_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub key_passphrase: Option<String>,
    /// The database host and port, as the SSH server sees them.
    pub remote_host: String,
    pub remote_port: u16,
    /// The SHA256 fingerprint (`SHA256:…`) the user approved in the trust
    /// prompt, set only by the retry after it. An unknown host key is
    /// recorded in known_hosts only when its fingerprint is exactly this;
    /// any other key fails. A key that differs from a recorded one is never
    /// accepted this way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub trust_host_key: Option<String>,
}

impl fmt::Debug for TunnelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |v: &Option<String>| v.as_ref().map(|_| "<redacted>");
        f.debug_struct("TunnelConfig")
            .field("ssh_host", &self.ssh_host)
            .field("ssh_port", &self.ssh_port)
            .field("ssh_username", &self.ssh_username)
            .field("auth_method", &self.auth_method)
            .field("password", &redacted(&self.password))
            .field("key_path", &self.key_path)
            .field("key_passphrase", &redacted(&self.key_passphrase))
            .field("remote_host", &self.remote_host)
            .field("remote_port", &self.remote_port)
            .field("trust_host_key", &self.trust_host_key)
            .finish()
    }
}

/// An open tunnel: connect to `127.0.0.1:<localPort>` to reach the remote
/// host, and pass `tunnelId` to `Ssh::Close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub local_port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TunnelConfig {
        TunnelConfig {
            ssh_host: "bastion".into(),
            ssh_port: 22,
            ssh_username: "me".into(),
            auth_method: "password".into(),
            password: Some("hunter2-password".into()),
            key_path: Some("/k".into()),
            key_passphrase: Some("hunter2-passphrase".into()),
            remote_host: "db".into(),
            remote_port: 5432,
            trust_host_key: None,
        }
    }

    #[test]
    fn debug_redacts_the_password_and_passphrase() {
        let debug = format!("{:?} {:#?}", config(), config());
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("bastion"));
    }

    #[test]
    fn wire_is_camel_case_and_trust_defaults_to_none() {
        let json = r#"{"sshHost":"h","sshPort":22,"sshUsername":"u","authMethod":"key",
            "keyPath":"/k","remoteHost":"db","remotePort":5432}"#;
        let c: TunnelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.trust_host_key, None);
        let trusted: TunnelConfig = serde_json::from_str(
            r#"{"sshHost":"h","sshPort":22,"sshUsername":"u","authMethod":"key",
            "remoteHost":"db","remotePort":5432,"trustHostKey":"SHA256:abc"}"#,
        )
        .unwrap();
        assert_eq!(trusted.trust_host_key.as_deref(), Some("SHA256:abc"));
        assert_eq!(c.password, None);
        assert_eq!(c.key_path.as_deref(), Some("/k"));
        let info = TunnelInfo {
            tunnel_id: "tunnel-1".into(),
            local_port: 5000,
        };
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"tunnelId":"tunnel-1","localPort":5000}"#
        );
    }
}
