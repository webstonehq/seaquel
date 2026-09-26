//! The `ssh` group of the workspace RPC: open and close the tunnels Core
//! owns.
//!
//! ```json
//! {"method":"ssh","params":{"method":"open","params":{"config":{"sshHost":"…",…}}}}
//! {"method":"ssh","result":{"method":"open","result":{"tunnelId":"tunnel-1","localPort":54321}}}
//! {"method":"ssh","params":{"method":"close","params":{"tunnelId":"tunnel-1"}}}
//! ```
//!
//! Only the desktop serves this group, with [`dispatch_ssh`], before any
//! storage opens. `dispatch_workspace` (the web server's `/rpc`) always
//! answers `NOT_SUPPORTED`, whatever features Cargo unified into the build,
//! and so does `dispatch_ssh` in a build without Core's `ssh` feature. Errors keep `seaquel-ssh`'s
//! codes (`UNKNOWN_HOST_KEY`, `HOST_KEY_MISMATCH`, `AUTH_FAILED`, …) and
//! `TUNNEL_NOT_FOUND`, which the desktop's host-key prompt matches on.

use seaquel_core::Core;
pub use seaquel_types::ssh::{TunnelConfig, TunnelInfo};
use serde::{Deserialize, Serialize};

use crate::RpcError;

/// An SSH call. Fields are camelCase. `Debug` redacts the password and
/// passphrase (see [`TunnelConfig`]).
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SshRequest {
    Open { config: TunnelConfig },
    Close { tunnel_id: String },
}

impl SshRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Close { .. } => "close",
        }
    }
}

/// An SSH call's result. `close` gives `null`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SshResponse {
    Open(TunnelInfo),
    Close(()),
}

/// Run one SSH call on Core's tunnels. Logs like `dispatch_workspace`: the
/// method, never the params.
pub async fn dispatch_ssh(core: &Core, req: SshRequest) -> Result<SshResponse, RpcError> {
    let method = req.method();
    crate::workspace::logged("ssh", method, run(core, req)).await
}

#[cfg(feature = "ssh")]
async fn run(core: &Core, req: SshRequest) -> Result<SshResponse, RpcError> {
    Ok(match req {
        SshRequest::Open { config } => SshResponse::Open(core.ssh_open(&config).await?),
        SshRequest::Close { tunnel_id } => SshResponse::Close(core.ssh_close(&tunnel_id).await?),
    })
}

#[cfg(not(feature = "ssh"))]
async fn run(_: &Core, _: SshRequest) -> Result<SshResponse, RpcError> {
    Err(RpcError::not_supported("SSH tunnels"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_password_and_passphrase() {
        let req: SshRequest = serde_json::from_str(
            r#"{"method":"open","params":{"config":{"sshHost":"h","sshPort":22,
            "sshUsername":"u","authMethod":"key","password":"pw-SECRET-1",
            "keyPath":"/k","keyPassphrase":"pp-SECRET-2","remoteHost":"db","remotePort":5432}}}"#,
        )
        .unwrap();
        let shown = format!("{req:?} {req:#?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }
}
