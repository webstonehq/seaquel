//! Seaquel's SSH tunnels. It opens a local port that forwards through an SSH
//! server to a database host, authenticating with a password or a key file,
//! checks the server's host key against known_hosts with trust on first use,
//! and ends every forward when a tunnel is closed. Core owns the running
//! tunnels behind its `ssh` feature.
//!
//! [`open`] returns a [`Tunnel`]. [`Tunnel::close`] (or dropping it) stops
//! the listener, aborts every forward running through it and ends the SSH
//! session, so a database client can't keep using a closed tunnel.
//!
//! Errors are [`TunnelError`]s whose codes the desktop's host-key prompt
//! matches on; they are the codes the Tauri command had:
//!
//! - `UNKNOWN_HOST_KEY` (the message holds `Fingerprint: SHA256:…`; retry
//!   with `trust_host_key` set to that fingerprint to record it; a key with
//!   any other fingerprint is refused again with its own), `HOST_KEY_MISMATCH`
//!   (never accepted), `HOST_KEY_STORE_ERROR`, `HOST_KEY_ERROR`;
//! - `TIMEOUT` (30 s to connect), `CONNECTION_ERROR`;
//! - `AUTH_ERROR` (no password or key path given), `AUTH_FAILED`,
//!   `KEY_NOT_FOUND`, `KEY_LOAD_ERROR`, `INVALID_AUTH_METHOD`;
//! - `BIND_ERROR` for the local port.
//!
//! Native only: it spawns its forwarding tasks on the tokio runtime it is
//! called from.

mod host_key;
mod tunnel;

use std::fmt;
use std::path::PathBuf;

pub use tunnel::{open, Tunnel};

/// How [`open`] checks host keys. Build it from `TunnelOptions::default()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TunnelOptions {
    /// The known_hosts file to check and record host keys in. `None` is the
    /// user's own: `~/.ssh/known_hosts` (russh's `~/ssh/known_hosts` on
    /// Windows, as before). Tests always set a temp file.
    pub known_hosts: Option<PathBuf>,
}

impl TunnelOptions {
    #[must_use]
    pub fn with_known_hosts(mut self, path: impl Into<PathBuf>) -> Self {
        self.known_hosts = Some(path.into());
        self
    }
}

/// Why a tunnel couldn't be opened. `code` is one of the codes in the crate
/// docs. Messages never include a password or passphrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelError {
    pub code: &'static str,
    pub message: String,
}

impl TunnelError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for TunnelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TunnelError {}
