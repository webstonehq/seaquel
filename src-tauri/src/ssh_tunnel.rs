use async_trait::async_trait;
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
    local_port: u16,
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

    pub async fn close_all(&self) {
        let mut tunnels = self.tunnels.lock().await;
        for (_, handle) in tunnels.drain() {
            if let Some(tx) = handle.shutdown_tx {
                let _ = tx.send(());
            }
        }
    }
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

struct ClientHandler;

#[async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all server keys (similar to StrictHostKeyChecking=no)
        // In production, you might want to implement proper host key verification
        Ok(true)
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
    // Create SSH config
    let ssh_config = Arc::new(client::Config::default());

    // Connect to SSH server
    let addr = format!("{}:{}", config.ssh_host, config.ssh_port);
    let mut session = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client::connect(ssh_config, &addr, ClientHandler),
    )
    .await
    .map_err(|_| TunnelError {
        message: "Connection timed out".to_string(),
        code: "TIMEOUT".to_string(),
    })?
    .map_err(|e| TunnelError {
        message: format!("Failed to connect to SSH server: {}", e),
        code: "CONNECTION_ERROR".to_string(),
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
        return Err(TunnelError {
            message: "Authentication failed".to_string(),
            code: "AUTH_FAILED".to_string(),
        });
    }

    // Bind to a random local port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| TunnelError {
            message: format!("Failed to bind local port: {}", e),
            code: "BIND_ERROR".to_string(),
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
                local_port,
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
                                if let Err(e) = handle_connection(
                                    local_stream,
                                    session,
                                    &remote_host,
                                    remote_port,
                                ).await {
                                    eprintln!("Tunnel connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            eprintln!("Failed to accept connection: {}", e);
                        }
                    }
                }
            }
        }
    });

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
    let mut tunnels = tunnel_manager.tunnels.lock().await;

    if let Some(mut handle) = tunnels.remove(&tunnel_id) {
        if let Some(tx) = handle.shutdown_tx.take() {
            let _ = tx.send(());
        }
        Ok(())
    } else {
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
    let tunnels = tunnel_manager.tunnels.lock().await;
    Ok(tunnels.contains_key(&tunnel_id))
}

#[tauri::command]
pub async fn list_active_tunnels(
    tunnel_manager: State<'_, TunnelManager>,
) -> Result<Vec<String>, TunnelError> {
    let tunnels = tunnel_manager.tunnels.lock().await;
    Ok(tunnels.keys().cloned().collect())
}
