use async_trait::async_trait;
use log::{debug, error, info, warn};
use russh::{client, ChannelMsg};
use russh_keys::ssh_key::PrivateKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_username: String,
    pub auth_method: String,
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub key_passphrase: Option<String>,
    pub remote_host: String,
    pub remote_port: u16,
    /// Set after the user accepted an unknown host key in the trust prompt.
    /// The key is then written to `~/.ssh/known_hosts` on a successful check.
    #[serde(default)]
    pub trust_new_host_key: bool,
}

#[derive(Debug, Serialize)]
pub struct TunnelResult {
    pub tunnel_id: String,
    pub local_port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TunnelError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TunnelError {}

struct TunnelHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

pub struct TunnelManager {
    tunnels: Arc<Mutex<HashMap<String, TunnelHandle>>>,
    next_id: Arc<Mutex<u64>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a host key was rejected, recorded so `establish_tunnel` can turn the
/// resulting connection failure into an actionable error for the UI.
struct HostKeyRejection {
    code: &'static str,
    fingerprint: String,
}

/// Verifies the server's host key against `~/.ssh/known_hosts`.
///
/// Unknown hosts are rejected with `UNKNOWN_HOST_KEY` and the fingerprint, so
/// the frontend can show a trust-on-first-use prompt and retry with
/// `trust_new_host_key`. A key that no longer matches the recorded one is
/// rejected with `HOST_KEY_MISMATCH` and is never auto-accepted — that is the
/// man-in-the-middle case.
struct ClientHandler {
    host: String,
    port: u16,
    trust_new_host_key: bool,
    rejection: Arc<std::sync::Mutex<Option<HostKeyRejection>>>,
}

impl ClientHandler {
    fn reject(&self, code: &'static str, fingerprint: String) -> Result<bool, russh::Error> {
        if let Ok(mut slot) = self.rejection.lock() {
            *slot = Some(HostKeyRejection { code, fingerprint });
        }
        Ok(false)
    }
}

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key
            .fingerprint(russh::keys::ssh_key::HashAlg::Sha256)
            .to_string();

        match russh_keys::check_known_hosts(&self.host, self.port, server_public_key) {
            Ok(true) => Ok(true),
            Ok(false) => {
                if !self.trust_new_host_key {
                    warn!(activity = "ssh.tunnel.hostkey", error_code = "UNKNOWN_HOST_KEY"; "SSH host key is not in known_hosts");
                    return self.reject("UNKNOWN_HOST_KEY", fingerprint);
                }
                if let Err(e) =
                    russh_keys::known_hosts::learn_known_hosts(&self.host, self.port, server_public_key)
                {
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

fn load_private_key(key_path: &str, passphrase: Option<&str>) -> Result<PrivateKey, TunnelError> {
    let path = Path::new(key_path);

    if !path.exists() {
        return Err(TunnelError {
            message: format!("SSH key file not found: {}", key_path),
            code: "KEY_NOT_FOUND".to_string(),
        });
    }

    russh_keys::load_secret_key(path, passphrase).map_err(|e| TunnelError {
        message: format!("Failed to load SSH key: {}", e),
        code: "KEY_LOAD_ERROR".to_string(),
    })
}

async fn establish_tunnel(
    config: &TunnelConfig,
    tunnel_manager: &TunnelManager,
) -> Result<TunnelResult, TunnelError> {
    info!(activity = "ssh.tunnel.create", auth_method = config.auth_method.as_str(); "Creating SSH tunnel");

    // Create SSH config
    let ssh_config = Arc::new(client::Config::default());

    // Connect to SSH server
    let addr = format!("{}:{}", config.ssh_host, config.ssh_port);
    let rejection = Arc::new(std::sync::Mutex::new(None::<HostKeyRejection>));
    let handler = ClientHandler {
        host: config.ssh_host.clone(),
        port: config.ssh_port,
        trust_new_host_key: config.trust_new_host_key,
        rejection: Arc::clone(&rejection),
    };
    let mut session = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client::connect(ssh_config, &addr, handler),
    )
    .await
    .map_err(|_| {
        error!(activity = "ssh.tunnel.create", error_code = "TIMEOUT"; "SSH tunnel connection timed out");
        TunnelError {
            message: "Connection timed out".to_string(),
            code: "TIMEOUT".to_string(),
        }
    })?
    .map_err(|e| {
        // A rejected host key surfaces here as a generic connection error, so
        // replace it with the specific reason the handler recorded.
        if let Some(HostKeyRejection { code, fingerprint }) =
            rejection.lock().ok().and_then(|mut slot| slot.take())
        {
            let message = match code {
                "UNKNOWN_HOST_KEY" => format!(
                    "The host key for {}:{} is not in known_hosts.\nFingerprint: {}",
                    config.ssh_host, config.ssh_port, fingerprint
                ),
                "HOST_KEY_MISMATCH" => format!(
                    "The host key for {}:{} does not match the one recorded in known_hosts. \
                     This can mean the server was rebuilt — or that the connection is being intercepted.\nFingerprint: {}",
                    config.ssh_host, config.ssh_port, fingerprint
                ),
                "HOST_KEY_STORE_ERROR" => {
                    "Could not record the host key in ~/.ssh/known_hosts.".to_string()
                }
                _ => "Could not read ~/.ssh/known_hosts to verify the host key.".to_string(),
            };
            return TunnelError {
                message,
                code: code.to_string(),
            };
        }
        error!(activity = "ssh.tunnel.create", error_code = "CONNECTION_ERROR"; "SSH tunnel connection failed");
        TunnelError {
            message: format!("Failed to connect to SSH server: {}", e),
            code: "CONNECTION_ERROR".to_string(),
        }
    })?;

    // Authenticate
    let authenticated = match config.auth_method.as_str() {
        "password" => {
            let password = config.password.as_ref().ok_or(TunnelError {
                message: "Password required for password authentication".to_string(),
                code: "AUTH_ERROR".to_string(),
            })?;

            session
                .authenticate_password(&config.ssh_username, password)
                .await
                .map_err(|e| TunnelError {
                    message: format!("Password authentication failed: {}", e),
                    code: "AUTH_FAILED".to_string(),
                })?
        }
        "key" => {
            let key_path = config.key_path.as_ref().ok_or(TunnelError {
                message: "Key path required for key authentication".to_string(),
                code: "AUTH_ERROR".to_string(),
            })?;

            let private_key = load_private_key(key_path, config.key_passphrase.as_deref())?;

            session
                .authenticate_publickey(&config.ssh_username, Arc::new(private_key))
                .await
                .map_err(|e| TunnelError {
                    message: format!("Key authentication failed: {}", e),
                    code: "AUTH_FAILED".to_string(),
                })?
        }
        _ => {
            return Err(TunnelError {
                message: format!("Unknown auth method: {}", config.auth_method),
                code: "INVALID_AUTH_METHOD".to_string(),
            });
        }
    };

    if !authenticated {
        error!(activity = "ssh.tunnel.create", error_code = "AUTH_FAILED"; "SSH authentication failed");
        return Err(TunnelError {
            message: "Authentication failed".to_string(),
            code: "AUTH_FAILED".to_string(),
        });
    }

    // Bind to a random local port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| {
            error!(activity = "ssh.tunnel.create", error_code = "BIND_ERROR"; "Failed to bind local port");
            TunnelError {
                message: format!("Failed to bind local port: {}", e),
                code: "BIND_ERROR".to_string(),
            }
        })?;

    let local_port = listener
        .local_addr()
        .map_err(|e| TunnelError {
            message: format!("Failed to get local address: {}", e),
            code: "BIND_ERROR".to_string(),
        })?
        .port();

    // Generate tunnel ID
    let tunnel_id = {
        let mut next_id = tunnel_manager.next_id.lock().await;
        let id = format!("tunnel-{}", *next_id);
        *next_id += 1;
        id
    };

    // Create shutdown channel
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    // Store tunnel handle
    {
        let mut tunnels = tunnel_manager.tunnels.lock().await;
        tunnels.insert(
            tunnel_id.clone(),
            TunnelHandle {
                shutdown_tx: Some(shutdown_tx),
            },
        );
    }

    let remote_host = config.remote_host.clone();
    let remote_port = config.remote_port;
    let session = Arc::new(session);

    // Spawn forwarding task
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((local_stream, _)) => {
                            let session = Arc::clone(&session);
                            let remote_host = remote_host.clone();

                            tokio::spawn(async move {
                                if let Err(_e) = handle_connection(
                                    local_stream,
                                    session,
                                    &remote_host,
                                    remote_port,
                                ).await {
                                    warn!(activity = "ssh.tunnel.forward"; "SSH tunnel forwarding error");
                                }
                            });
                        }
                        Err(_e) => {
                            warn!(activity = "ssh.tunnel.forward"; "SSH tunnel accept error");
                        }
                    }
                }
            }
        }
    });

    info!(activity = "ssh.tunnel.create", tunnel_id = tunnel_id.as_str(), local_port = local_port; "SSH tunnel established");
    Ok(TunnelResult {
        tunnel_id,
        local_port,
    })
}

async fn handle_connection(
    mut local_stream: tokio::net::TcpStream,
    session: Arc<client::Handle<ClientHandler>>,
    remote_host: &str,
    remote_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Open a direct-tcpip channel to the remote host
    let mut channel = session
        .channel_open_direct_tcpip(remote_host, remote_port as u32, "127.0.0.1", 0)
        .await?;

    let (mut local_read, mut local_write) = local_stream.split();

    // Buffer for reading from local
    let mut local_buf = vec![0u8; 32768];

    loop {
        tokio::select! {
            // Read from local, write to SSH channel
            read_result = local_read.read(&mut local_buf) => {
                match read_result {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        channel.data(&local_buf[..n]).await?;
                    }
                    Err(e) => {
                        eprintln!("Local read error: {}", e);
                        break;
                    }
                }
            }
            // Read from SSH channel, write to local
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        local_write.write_all(&data).await?;
                    }
                    Some(ChannelMsg::Eof) | None => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn create_ssh_tunnel(
    config: TunnelConfig,
    tunnel_manager: State<'_, TunnelManager>,
) -> Result<TunnelResult, TunnelError> {
    establish_tunnel(&config, &tunnel_manager).await
}

#[tauri::command]
pub async fn close_ssh_tunnel(
    tunnel_id: String,
    tunnel_manager: State<'_, TunnelManager>,
) -> Result<(), TunnelError> {
    info!(activity = "ssh.tunnel.close", tunnel_id = tunnel_id.as_str(); "Closing SSH tunnel");
    let mut tunnels = tunnel_manager.tunnels.lock().await;

    if let Some(mut handle) = tunnels.remove(&tunnel_id) {
        if let Some(tx) = handle.shutdown_tx.take() {
            let _ = tx.send(());
        }
        Ok(())
    } else {
        warn!(activity = "ssh.tunnel.close", tunnel_id = tunnel_id.as_str(), error_code = "TUNNEL_NOT_FOUND"; "SSH tunnel not found");
        Err(TunnelError {
            message: format!("Tunnel not found: {}", tunnel_id),
            code: "TUNNEL_NOT_FOUND".to_string(),
        })
    }
}

#[tauri::command]
pub async fn check_tunnel_status(
    tunnel_id: String,
    tunnel_manager: State<'_, TunnelManager>,
) -> Result<bool, TunnelError> {
    debug!(activity = "ssh.tunnel.status", tunnel_id = tunnel_id.as_str(); "Checking tunnel status");
    let tunnels = tunnel_manager.tunnels.lock().await;
    Ok(tunnels.contains_key(&tunnel_id))
}

#[tauri::command]
pub async fn list_active_tunnels(
    tunnel_manager: State<'_, TunnelManager>,
) -> Result<Vec<String>, TunnelError> {
    debug!(activity = "ssh.tunnel.status"; "Listing active tunnels");
    let tunnels = tunnel_manager.tunnels.lock().await;
    Ok(tunnels.keys().cloned().collect())
}
