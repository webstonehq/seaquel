//! SSH tunnels (`seaquel-ssh`). Core owns the running tunnels: the desktop
//! opens one per connection that goes through a bastion, closes it on
//! disconnect, and every tunnel still open closes when Core is dropped.
//!
//! The tunnels' forwarding tasks run on the tokio runtime of the caller of
//! [`Core::ssh_open`]; `seaquel-ssh` spawns them, Core only keeps the handles.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use log::{debug, info};
pub use seaquel_ssh::{Tunnel, TunnelError, TunnelOptions};
pub use seaquel_types::ssh::{TunnelConfig, TunnelInfo};

use crate::{Core, CoreBuilder, CoreError};

/// `Ssh::Close` on an id that isn't open (never was, or already closed).
pub const TUNNEL_NOT_FOUND: &str = "TUNNEL_NOT_FOUND";

/// The open tunnels, by id (`tunnel-1`, `tunnel-2`, …).
#[derive(Default)]
pub(crate) struct TunnelManager {
    options: TunnelOptions,
    tunnels: Mutex<HashMap<String, Tunnel>>,
    next_id: AtomicU64,
    /// Tunnel ids by the id of the connection that owns them
    /// (`Workspace::connect_saved`), so `disconnect` closes the tunnel too.
    owned: Mutex<HashMap<String, String>>,
}

impl TunnelManager {
    pub(crate) fn new(options: TunnelOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }
}

impl From<TunnelError> for CoreError {
    fn from(e: TunnelError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl CoreBuilder {
    /// Check and record SSH host keys in `path` instead of the user's
    /// `~/.ssh/known_hosts`. Tests use a temp file.
    #[must_use]
    pub fn ssh_known_hosts(mut self, path: impl Into<PathBuf>) -> Self {
        self.ssh = self.ssh.with_known_hosts(path);
        self
    }
}

impl Core {
    /// Open a tunnel: connect, check the host key, authenticate and start
    /// forwarding a local port. Fails with the `seaquel-ssh` codes
    /// (`UNKNOWN_HOST_KEY`, `AUTH_FAILED`, …).
    pub async fn ssh_open(&self, config: &TunnelConfig) -> Result<TunnelInfo, CoreError> {
        let tunnels = &self.tunnels;
        let tunnel = seaquel_ssh::open(config, &tunnels.options).await?;
        let tunnel_id = format!(
            "tunnel-{}",
            tunnels.next_id.fetch_add(1, Ordering::Relaxed) + 1
        );
        let local_port = tunnel.local_port();
        tunnels
            .tunnels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tunnel_id.clone(), tunnel);
        info!(activity = "ssh.tunnel.create", tunnel_id = tunnel_id.as_str(), local_port = local_port; "SSH tunnel open");
        Ok(TunnelInfo {
            tunnel_id,
            local_port,
        })
    }

    /// Close a tunnel: its local port stops accepting, every connection
    /// through it is cut, and the SSH session ends.
    pub async fn ssh_close(&self, tunnel_id: &str) -> Result<(), CoreError> {
        let tunnel = self
            .tunnels
            .tunnels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(tunnel_id);
        match tunnel {
            Some(tunnel) => {
                debug!(activity = "ssh.tunnel.close", tunnel_id = tunnel_id; "Closing SSH tunnel");
                tunnel.close().await;
                Ok(())
            }
            None => Err(CoreError::new(
                TUNNEL_NOT_FOUND,
                format!("Tunnel not found: {tunnel_id}"),
            )),
        }
    }

    /// Tie a tunnel to a connection: [`Core::disconnect`] of the connection
    /// closes it. Dropping Core closes it anyway.
    #[cfg_attr(
        not(all(feature = "storage", feature = "secrets", feature = "workspace")),
        allow(dead_code)
    )]
    pub(crate) fn own_tunnel(&self, connection_id: &str, tunnel_id: &str) {
        self.tunnels
            .owned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(connection_id.to_string(), tunnel_id.to_string());
    }

    /// Take the tunnel a connection owns, if any, out of the ownership map,
    /// guarded: [`TunnelGuard::close`] closes it, and so does dropping the
    /// guard, so a `disconnect` whose future is dropped can't leak it.
    pub(crate) fn take_tunnel_of(&self, connection_id: &str) -> Option<TunnelGuard<'_>> {
        let tunnel_id = self
            .tunnels
            .owned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id)?;
        Some(self.tunnel_guard(&tunnel_id))
    }

    /// Closes the tunnel when dropped unless [`TunnelGuard::keep`] was
    /// called: a caller whose future is dropped between opening a tunnel and
    /// handing it to a connection doesn't leak it.
    pub(crate) fn tunnel_guard(&self, tunnel_id: &str) -> TunnelGuard<'_> {
        TunnelGuard {
            tunnels: &self.tunnels.tunnels,
            tunnel_id: Some(tunnel_id.to_string()),
        }
    }

    /// The number of open tunnels.
    pub fn ssh_tunnel_count(&self) -> usize {
        self.tunnels
            .tunnels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// See [`Core::tunnel_guard`].
pub(crate) struct TunnelGuard<'a> {
    tunnels: &'a Mutex<HashMap<String, Tunnel>>,
    tunnel_id: Option<String>,
}

impl TunnelGuard<'_> {
    /// Don't close the tunnel on drop.
    #[cfg_attr(
        not(all(feature = "storage", feature = "secrets", feature = "workspace")),
        allow(dead_code)
    )]
    pub(crate) fn keep(mut self) {
        self.tunnel_id = None;
    }

    /// Close the tunnel now, waiting for its SSH session to end. The tunnel
    /// leaves the map before anything is awaited; if this future is dropped
    /// part-way, dropping the `Tunnel` closes it. A tunnel already closed by
    /// id is nothing to do.
    pub(crate) async fn close(mut self) {
        let Some(id) = self.tunnel_id.take() else {
            return;
        };
        let tunnel = self
            .tunnels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        if let Some(tunnel) = tunnel {
            debug!(activity = "ssh.tunnel.close", tunnel_id = id.as_str(); "Closing SSH tunnel");
            tunnel.close().await;
        }
    }
}

impl Drop for TunnelGuard<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.tunnel_id.take() {
            // Dropping a `Tunnel` stops its listener and aborts its forwards.
            let tunnel = self
                .tunnels
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            drop(tunnel);
        }
    }
}
