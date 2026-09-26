//! The `ssh` group: parsing, the wire shape, and `dispatch_ssh` and
//! `dispatch_workspace` keeping Core's error codes. Live tunnels are tested
//! in `seaquel-ssh` and Core.

use seaquel_core::{Core, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_ssh, dispatch_workspace, parse_request, Request, Response, SshRequest, SshResponse,
    TunnelInfo,
};

const OPEN: &str = r#"{"method":"ssh","params":{"method":"open","params":{"config":{"sshHost":"127.0.0.1","sshPort":1,"sshUsername":"me","authMethod":"password","password":"pw-SECRET-9","remoteHost":"db","remotePort":5432}}}}"#;

fn core(dir: &tempfile::TempDir) -> Core {
    Core::builder()
        .ssh_known_hosts(dir.path().join("known_hosts"))
        .build()
}

#[test]
fn an_open_request_parses_from_bytes_and_hides_the_password() {
    let req = parse_request(OPEN.as_bytes()).unwrap();
    assert_eq!((req.group(), req.method()), ("ssh", "open"));
    let Request::Ssh(SshRequest::Open { config }) = &req else {
        panic!("{req:?}");
    };
    assert_eq!(config.ssh_host, "127.0.0.1");
    assert_eq!(config.password.as_deref(), Some("pw-SECRET-9"));
    assert_eq!(config.trust_host_key, None);
    assert!(!format!("{req:?}").contains("SECRET"));
}

#[test]
fn close_takes_a_camel_case_tunnel_id() {
    let req = parse_request(
        br#"{"method":"ssh","params":{"method":"close","params":{"tunnelId":"tunnel-3"}}}"#,
    )
    .unwrap();
    let Request::Ssh(SshRequest::Close { tunnel_id }) = req else {
        panic!("{req:?}");
    };
    assert_eq!(tunnel_id, "tunnel-3");
}

#[test]
fn params_before_method_is_refused_here_too() {
    let err = parse_request(
        br#"{"method":"ssh","params":{"params":{"tunnelId":"tunnel-1"},"method":"close"}}"#,
    )
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

#[test]
fn responses_serialize_as_the_ts_client_reads_them() {
    let open = Response::Ssh(SshResponse::Open(TunnelInfo {
        tunnel_id: "tunnel-1".into(),
        local_port: 54321,
    }));
    assert_eq!(
        serde_json::to_string(&open).unwrap(),
        r#"{"method":"ssh","result":{"method":"open","result":{"tunnelId":"tunnel-1","localPort":54321}}}"#
    );
    let close = Response::Ssh(SshResponse::Close(()));
    assert_eq!(
        serde_json::to_string(&close).unwrap(),
        r#"{"method":"ssh","result":{"method":"close","result":null}}"#
    );
}

#[tokio::test]
async fn closing_an_unknown_tunnel_is_tunnel_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = dispatch_ssh(
        &core(&dir),
        SshRequest::Close {
            tunnel_id: "tunnel-9".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "TUNNEL_NOT_FOUND");
    assert_eq!(err.message, "Tunnel not found: tunnel-9");
}

#[tokio::test]
async fn open_keeps_the_tunnel_error_code_and_never_echoes_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let Request::Ssh(req) = parse_request(OPEN.as_bytes()).unwrap() else {
        unreachable!()
    };
    // Port 1 on loopback refuses at once.
    let err = dispatch_ssh(&core(&dir), req).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_ERROR");
    assert!(!err.message.contains("SECRET"), "{err}");
}

/// The web server's `/rpc` goes through `dispatch_workspace`, which never
/// opens tunnels, even in this build where the `ssh` feature is on.
#[tokio::test]
async fn dispatch_workspace_refuses_the_ssh_group() {
    let dir = tempfile::tempdir().unwrap();
    let core = core(&dir);
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let Request::Ssh(open) = parse_request(OPEN.as_bytes()).unwrap() else {
        unreachable!()
    };
    let close = SshRequest::Close {
        tunnel_id: "tunnel-1".into(),
    };
    for req in [open, close] {
        let err = dispatch_workspace(&core, &ws, Request::Ssh(req))
            .await
            .unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
    }
}
