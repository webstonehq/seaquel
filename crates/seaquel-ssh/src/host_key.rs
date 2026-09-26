//! The host-key check: known_hosts with trust on first use.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::{error, info, warn};
use russh::client;
use russh::keys::ssh_key::{HashAlg, PublicKey};

use crate::TunnelError;

/// Why a host key was rejected, recorded so `open` can turn the connection
/// failure that follows into an actionable error for the UI.
#[derive(Debug)]
pub(crate) struct HostKeyRejection {
    code: &'static str,
    fingerprint: String,
}

impl HostKeyRejection {
    pub(crate) fn into_error(self, host: &str, port: u16) -> TunnelError {
        let Self { code, fingerprint } = self;
        let message = match code {
            "UNKNOWN_HOST_KEY" => format!(
                "The host key for {host}:{port} is not in known_hosts.\nFingerprint: {fingerprint}"
            ),
            "HOST_KEY_MISMATCH" => format!(
                "The host key for {host}:{port} does not match the one recorded in known_hosts. \
                 This can mean the server was rebuilt — or that the connection is being intercepted.\nFingerprint: {fingerprint}"
            ),
            "HOST_KEY_STORE_ERROR" => "Could not record the host key in known_hosts.".to_string(),
            _ => "Could not read known_hosts to verify the host key.".to_string(),
        };
        TunnelError::new(code, message)
    }
}

pub(crate) type RejectionSlot = Arc<Mutex<Option<HostKeyRejection>>>;

/// Verifies the server's host key against known_hosts.
///
/// Unknown hosts are rejected with `UNKNOWN_HOST_KEY` and the fingerprint,
/// so the frontend can show a trust-on-first-use prompt and retry with
/// `trust_host_key` set to the fingerprint the user approved. The retry
/// records the key only if it has exactly that fingerprint: a different key
/// on the second connection is refused with `UNKNOWN_HOST_KEY` and its own
/// fingerprint, and nothing is written. A key that no longer matches the recorded one is
/// rejected with `HOST_KEY_MISMATCH` and is never auto-accepted: that is the
/// man-in-the-middle case.
pub(crate) struct ClientHandler {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// The fingerprint the user approved, if any.
    pub(crate) trust_host_key: Option<String>,
    /// `None`: the user's own known_hosts.
    pub(crate) known_hosts: Option<PathBuf>,
    pub(crate) rejection: RejectionSlot,
}

impl ClientHandler {
    fn reject(&self, code: &'static str, fingerprint: String) -> Result<bool, russh::Error> {
        if let Ok(mut slot) = self.rejection.lock() {
            *slot = Some(HostKeyRejection { code, fingerprint });
        }
        Ok(false)
    }

    fn check(&self, key: &PublicKey) -> Result<bool, russh_keys::Error> {
        match &self.known_hosts {
            Some(path) => russh_keys::check_known_hosts_path(&self.host, self.port, key, path),
            None => russh_keys::check_known_hosts(&self.host, self.port, key),
        }
    }

    fn learn(&self, key: &PublicKey) -> Result<(), russh_keys::Error> {
        match &self.known_hosts {
            Some(path) => {
                russh_keys::known_hosts::learn_known_hosts_path(&self.host, self.port, key, path)
            }
            None => russh_keys::known_hosts::learn_known_hosts(&self.host, self.port, key),
        }
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();

        match self.check(key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                match self.trust_host_key.as_deref() {
                    Some(approved) if approved == fingerprint => {}
                    Some(_) => {
                        warn!(activity = "ssh.tunnel.hostkey", error_code = "UNKNOWN_HOST_KEY"; "SSH host key is not the one the user approved");
                        return self.reject("UNKNOWN_HOST_KEY", fingerprint);
                    }
                    None => {
                        warn!(activity = "ssh.tunnel.hostkey", error_code = "UNKNOWN_HOST_KEY"; "SSH host key is not in known_hosts");
                        return self.reject("UNKNOWN_HOST_KEY", fingerprint);
                    }
                }
                if let Err(e) = self.learn(key) {
                    error!(activity = "ssh.tunnel.hostkey", error_code = "HOST_KEY_STORE_ERROR"; "Failed to record SSH host key: {}", e);
                    return self.reject("HOST_KEY_STORE_ERROR", fingerprint);
                }
                info!(activity = "ssh.tunnel.hostkey"; "Recorded new SSH host key in known_hosts");
                Ok(true)
            }
            Err(russh_keys::Error::KeyChanged { line }) => {
                error!(activity = "ssh.tunnel.hostkey", error_code = "HOST_KEY_MISMATCH", line = line; "SSH host key does not match known_hosts");
                self.reject("HOST_KEY_MISMATCH", fingerprint)
            }
            Err(e) => {
                error!(activity = "ssh.tunnel.hostkey", error_code = "HOST_KEY_ERROR"; "Failed to read known_hosts: {}", e);
                self.reject("HOST_KEY_ERROR", fingerprint)
            }
        }
    }
}
