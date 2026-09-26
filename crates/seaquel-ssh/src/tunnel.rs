//! Opening a tunnel and forwarding its connections.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use log::{error, info, warn};
use russh::keys::ssh_key::PrivateKey;
use russh::{client, ChannelMsg, Disconnect};
use seaquel_types::ssh::TunnelConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::host_key::{ClientHandler, RejectionSlot};
use crate::{TunnelError, TunnelOptions};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

type Session = client::Handle<ClientHandler>;

/// A running tunnel: `127.0.0.1:<local_port>` forwards to the remote host.
///
/// [`Tunnel::close`] or dropping it stops the listener, aborts every forward
/// running through it and ends the SSH session.
pub struct Tunnel {
    local_port: u16,
    session: Arc<Session>,
    /// The accept loop. It owns the forwards' own `JoinSet`, so aborting it
    /// (or dropping this set) aborts every forward too.
    tasks: JoinSet<()>,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel")
            .field("local_port", &self.local_port)
            .finish_non_exhaustive()
    }
}

impl Tunnel {
    /// The local port to connect the database client to.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// Close the tunnel: stop accepting, abort every forward (their local
    /// sockets close), and disconnect the SSH session. When this returns the
    /// local port is free.
    pub async fn close(mut self) {
        self.tasks.shutdown().await;
        // Best effort: the session may already be gone.
        let _ = self
            .session
            .disconnect(Disconnect::ByApplication, "tunnel closed", "en")
            .await;
        info!(activity = "ssh.tunnel.close", local_port = self.local_port; "SSH tunnel closed");
    }
}

// Dropping `tasks` aborts the accept loop and, through the `JoinSet` it
// owns, every forward. The session ends once the last handle to it (held by
// those tasks and by `session`) is gone.

fn load_private_key(key_path: &str, passphrase: Option<&str>) -> Result<PrivateKey, TunnelError> {
    let path = Path::new(key_path);
    if !path.exists() {
        return Err(TunnelError::new(
            "KEY_NOT_FOUND",
            format!("SSH key file not found: {key_path}"),
        ));
    }
    russh_keys::load_secret_key(path, passphrase)
        .map_err(|e| TunnelError::new("KEY_LOAD_ERROR", format!("Failed to load SSH key: {e}")))
}

/// Connect to the SSH server, check its host key, authenticate, and start
/// forwarding a fresh local port to `remote_host:remote_port`.
pub async fn open(config: &TunnelConfig, options: &TunnelOptions) -> Result<Tunnel, TunnelError> {
    info!(activity = "ssh.tunnel.create", auth_method = config.auth_method.as_str(); "Creating SSH tunnel");

    let addr = (config.ssh_host.as_str(), config.ssh_port);
    let rejection = RejectionSlot::default();
    let handler = ClientHandler {
        host: config.ssh_host.clone(),
        port: config.ssh_port,
        trust_host_key: config.trust_host_key.clone(),
        known_hosts: options.known_hosts.clone(),
        rejection: Arc::clone(&rejection),
    };
    let ssh_config = Arc::new(client::Config::default());
    let mut session = tokio::time::timeout(CONNECT_TIMEOUT, client::connect(ssh_config, addr, handler))
        .await
        .map_err(|_| {
            error!(activity = "ssh.tunnel.create", error_code = "TIMEOUT"; "SSH tunnel connection timed out");
            TunnelError::new("TIMEOUT", "Connection timed out")
        })?
        .map_err(|e| {
            // A rejected host key surfaces here as a generic connection
            // error, so replace it with the reason the handler recorded.
            if let Some(rejected) = take(&rejection) {
                return rejected.into_error(&config.ssh_host, config.ssh_port);
            }
            error!(activity = "ssh.tunnel.create", error_code = "CONNECTION_ERROR"; "SSH tunnel connection failed");
            TunnelError::new(
                "CONNECTION_ERROR",
                format!("Failed to connect to SSH server: {e}"),
            )
        })?;

    authenticate(&mut session, config).await?;

    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
        error!(activity = "ssh.tunnel.create", error_code = "BIND_ERROR"; "Failed to bind local port");
        TunnelError::new("BIND_ERROR", format!("Failed to bind local port: {e}"))
    })?;
    let local_port = listener
        .local_addr()
        .map_err(|e| TunnelError::new("BIND_ERROR", format!("Failed to get local address: {e}")))?
        .port();

    let session = Arc::new(session);
    let mut tasks = JoinSet::new();
    tasks.spawn(accept_loop(
        listener,
        Arc::clone(&session),
        config.remote_host.clone(),
        config.remote_port,
    ));

    info!(activity = "ssh.tunnel.create", local_port = local_port; "SSH tunnel established");
    Ok(Tunnel {
        local_port,
        session,
        tasks,
    })
}

fn take(slot: &RejectionSlot) -> Option<crate::host_key::HostKeyRejection> {
    slot.lock().ok().and_then(|mut s| s.take())
}

async fn authenticate(session: &mut Session, config: &TunnelConfig) -> Result<(), TunnelError> {
    let authenticated = match config.auth_method.as_str() {
        "password" => {
            let password = config.password.as_ref().ok_or_else(|| {
                TunnelError::new(
                    "AUTH_ERROR",
                    "Password required for password authentication",
                )
            })?;
            session
                .authenticate_password(&config.ssh_username, password)
                .await
                .map_err(|e| {
                    TunnelError::new(
                        "AUTH_FAILED",
                        format!("Password authentication failed: {e}"),
                    )
                })?
        }
        "key" => {
            let key_path = config.key_path.as_ref().ok_or_else(|| {
                TunnelError::new("AUTH_ERROR", "Key path required for key authentication")
            })?;
            let private_key = load_private_key(key_path, config.key_passphrase.as_deref())?;
            session
                .authenticate_publickey(&config.ssh_username, Arc::new(private_key))
                .await
                .map_err(|e| {
                    TunnelError::new("AUTH_FAILED", format!("Key authentication failed: {e}"))
                })?
        }
        other => {
            return Err(TunnelError::new(
                "INVALID_AUTH_METHOD",
                format!("Unknown auth method: {other}"),
            ));
        }
    };

    if !authenticated {
        error!(activity = "ssh.tunnel.create", error_code = "AUTH_FAILED"; "SSH authentication failed");
        return Err(TunnelError::new("AUTH_FAILED", "Authentication failed"));
    }
    Ok(())
}

/// Accept local connections and forward each through its own channel. The
/// forwards live in a `JoinSet` owned by this future, so aborting it aborts
/// them.
async fn accept_loop(
    listener: TcpListener,
    session: Arc<Session>,
    remote_host: String,
    remote_port: u16,
) {
    let mut forwards = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let session = Arc::clone(&session);
                    let remote_host = remote_host.clone();
                    forwards.spawn(async move {
                        if let Err(e) = forward(stream, &session, &remote_host, remote_port).await {
                            warn!(activity = "ssh.tunnel.forward"; "SSH tunnel forwarding error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    warn!(activity = "ssh.tunnel.forward"; "SSH tunnel accept error: {}", e);
                    // Out of file descriptors (EMFILE) and the like fail
                    // again at once; don't spin on them.
                    tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                }
            },
            // Reap finished forwards so the set doesn't grow.
            Some(_) = forwards.join_next(), if !forwards.is_empty() => {}
        }
    }
}

/// Copy bytes both ways between a local connection and a `direct-tcpip`
/// channel to the remote host, until either side ends.
async fn forward(
    mut local: TcpStream,
    session: &Session,
    remote_host: &str,
    remote_port: u16,
) -> Result<(), russh::Error> {
    let mut channel = session
        .channel_open_direct_tcpip(remote_host, u32::from(remote_port), "127.0.0.1", 0)
        .await?;
    let (mut local_read, mut local_write) = local.split();
    let mut buf = vec![0u8; 32768];

    loop {
        tokio::select! {
            read = local_read.read(&mut buf) => match read {
                Ok(0) => break,
                Ok(n) => channel.data(&buf[..n]).await?,
                Err(e) => {
                    warn!(activity = "ssh.tunnel.forward"; "Local read error: {}", e);
                    break;
                }
            },
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    if let Err(e) = local_write.write_all(&data).await {
                        warn!(activity = "ssh.tunnel.forward"; "Local write error: {}", e);
                        break;
                    }
                }
                Some(ChannelMsg::Eof) | None => break,
                _ => {}
            },
        }
    }
    Ok(())
}
