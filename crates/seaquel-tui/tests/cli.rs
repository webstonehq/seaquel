//! Runs the built `seaquel-tui` binary without a terminal.

use std::process::{Command, Output, Stdio};

const TERMS: &str = "https://seaquel.app/terms";

fn tui(args: &[&str]) -> Output {
    let data = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_seaquel-tui"))
        .args(args)
        .env("SEAQUEL_DATA_DIR", data.path())
        .stdin(Stdio::null())
        .output()
        .expect("run seaquel-tui")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8 output")
}

#[test]
fn version_prints_the_app_version_and_the_terms_line() {
    let out = tui(&["--version"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some(format!("seaquel-tui {}", seaquel_terminal::VERSION).as_str())
    );
    assert!(lines.next().unwrap_or_default().contains(TERMS), "{stdout}");
    assert!(!seaquel_terminal::VERSION.starts_with("0.1."));
}

#[test]
fn help_lists_the_options_and_ends_with_the_terms_line() {
    let out = tui(&["--help"]);
    assert!(out.status.success());
    let stdout = text(&out.stdout);
    for flag in [
        "--project",
        "--connection",
        "--theme",
        "--no-mouse",
        "--page-size",
        "--log-level",
    ] {
        assert!(stdout.contains(flag), "{flag}: {stdout}");
    }
    let last = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap();
    assert!(last.contains(TERMS), "{stdout}");
}

#[test]
fn without_a_terminal_it_says_so_and_fails() {
    let out = tui(&[]);
    assert!(!out.status.success());
    assert_eq!(text(&out.stdout), "");
    assert_eq!(
        text(&out.stderr).trim(),
        "seaquel-tui needs a terminal on stdin and stdout"
    );
}

#[test]
fn a_bad_option_fails_before_anything_starts() {
    let out = tui(&["--page-size", "0"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("--page-size"));
}
