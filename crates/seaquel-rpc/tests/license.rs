//! The `license` group: parsing, the desktop's `dispatch_license`, and the
//! web workspace's `NOT_SUPPORTED`. The activation client itself is tested
//! against a fake server in `seaquel-license`.

use seaquel_core::license::desktop::DesktopClient;
use seaquel_core::{Core, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_license, dispatch_workspace, parse_request, DesktopLicenseRequest, Request,
};

const KEY: &str = "SQ-RPC-TEST-KEY-0001";

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn a_license_request_parses_from_bytes() {
    let req = parse_request(
        format!(
            r#"{{"method":"license","params":{{"method":"validate","params":{{"key":"{KEY}","instanceId":"i1"}}}}}}"#
        )
        .as_bytes(),
    )
    .unwrap();
    assert_eq!((req.group(), req.method()), ("license", "validate"));
    let Request::License(DesktopLicenseRequest::Validate { key, instance_id }) = &req else {
        panic!("{req:?}");
    };
    assert_eq!((key.as_str(), instance_id.as_str()), (KEY, "i1"));
    assert!(!format!("{req:?}").contains(KEY));
}

#[test]
fn params_before_method_is_refused_here_too() {
    let err = parse_request(
        br#"{"method":"license","params":{"params":{"key":"k","instanceName":"n"},"method":"activate"}}"#,
    )
    .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn dispatch_license_keeps_the_clients_error_code() {
    let client = DesktopClient::new(format!("http://127.0.0.1:{}", closed_port()));
    let err = dispatch_license(
        &client,
        DesktopLicenseRequest::Activate {
            key: KEY.into(),
            instance_name: "host__me".into(),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "NETWORK_ERROR");
    assert!(err
        .message
        .starts_with("Failed to connect to license server: "));
    assert!(!err.message.contains(KEY));
}

#[tokio::test]
async fn a_workspace_has_no_activation_client() {
    let core = Core::builder().build();
    let dir = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let req = parse_request(
        br#"{"method":"license","params":{"method":"deactivate","params":{"key":"k","instanceId":"i"}}}"#,
    )
    .unwrap();
    let err = dispatch_workspace(&core, &ws, req, seaquel_rpc::WriteOrigin::none())
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
}
