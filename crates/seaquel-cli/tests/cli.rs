//! Runs the built `seaquel-cli` binary.

use std::process::{Command, Output};

const TERMS: &str = "https://seaquel.app/terms";

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_seaquel-cli"))
        .args(args)
        .output()
        .expect("run seaquel-cli")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8 output")
}

#[test]
fn version_prints_the_app_version_and_the_terms_line() {
    let out = cli(&["--version"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some(format!("seaquel-cli {}", seaquel_cli::VERSION).as_str())
    );
    assert!(lines.next().unwrap_or_default().contains(TERMS), "{stdout}");
    assert!(
        !seaquel_cli::VERSION.starts_with("0.1."),
        "the app's version, not the crate's"
    );
}

#[test]
fn help_points_to_the_terms() {
    let out = cli(&["--help"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    assert!(stdout.contains(TERMS), "{stdout}");
    assert!(stdout.contains("mcp"), "{stdout}");
}

#[test]
fn mcp_help_lists_the_exposure_and_log_flags() {
    let out = cli(&["mcp", "--help"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    for flag in ["--connection", "--project", "--log-level"] {
        assert!(stdout.contains(flag), "{stdout}");
    }
}
