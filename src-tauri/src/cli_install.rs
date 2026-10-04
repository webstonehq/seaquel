//! "Install Command Line Tool…": downloads the version-matched CLI on demand.
//!
//! - **macOS:** `/usr/local/bin/seaquel-cli` becomes a symlink to
//!   the CLI in the app's data directory. When the user can't write there
//!   (the usual case), the same two commands run through `osascript … with
//!   administrator privileges`, built from fixed, quoted paths only.
//! - **Linux:** `~/.local/bin/seaquel-cli` links to the downloaded CLI in the
//!   app's data directory, including for AppImage, deb, and rpm installs.
//! - **Windows:** the CLI is installed in the app's data directory. MCP
//!   snippets use that absolute path; this doesn't edit the user's `PATH`.
//!
//! Every platform then installs the CLI's DuckDB helper beside it
//! (`cli_download::install_duckdb_helper`, the DuckDB helper plan's Q4 C),
//! also when the CLI itself was already current, so an earlier install
//! gains it. A helper that can't be installed doesn't fail the CLI's
//! install: the dialog says why and how to retry.
//!
//! The file-system steps are plain functions over paths so they can be tested
//! against a temp directory; only [`install_from_menu`] touches the real system.
//! Each platform uses part of them and the tests use all of them, hence the
//! `dead_code` allowance.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cli_download;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

pub const MENU_ID: &str = "install_cli";
pub const MENU_LABEL: &str = "Install Command Line Tool…";
/// The command users type, on every platform (phase 4, open question 1).
pub const CLI_NAME: &str = "seaquel-cli";
const DIALOG_TITLE: &str = "Install Command Line Tool";

/// Whether the menu shows the item on this platform and install.
pub fn available() -> bool {
    cfg!(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    ))
}

/// What is at the link path now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// Nothing there.
    Missing,
    /// A symlink that already resolves to the target.
    Ours,
    /// A symlink to something else (possibly dangling).
    OtherLink(PathBuf),
    /// A regular file (another install of some `seaquel-cli`).
    File,
    /// A directory: never touched.
    Directory,
}

pub fn link_state(link: &Path, target: &Path) -> io::Result<LinkState> {
    let meta = match fs::symlink_metadata(link) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LinkState::Missing),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        let points_to = fs::read_link(link)?;
        if points_to == target {
            return Ok(LinkState::Ours);
        }
        // A relative link, or one through another symlink, that still ends at
        // the same file.
        if let (Ok(a), Ok(b)) = (fs::canonicalize(link), fs::canonicalize(target)) {
            if a == b {
                return Ok(LinkState::Ours);
            }
        }
        return Ok(LinkState::OtherLink(points_to));
    }
    if meta.is_dir() {
        Ok(LinkState::Directory)
    } else {
        Ok(LinkState::File)
    }
}

/// How an install ended, for the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    AlreadyInstalled,
    Replaced,
    /// The link was ours but the installed CLI was from another version.
    Updated,
    Cancelled,
}

/// Creates `link` -> `target`, replacing a file or symlink already there when
/// `replace` is set. Creates the link's directory if needed. Errors (notably
/// `PermissionDenied`) are returned untouched so the caller can escalate.
#[cfg(unix)]
pub fn place_symlink(link: &Path, target: &Path, replace: bool) -> io::Result<()> {
    if let Some(dir) = link.parent() {
        fs::create_dir_all(dir)?;
    }
    if replace {
        match fs::remove_file(link) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::os::unix::fs::symlink(target, link)
}

// --- quoting -------------------------------------------------------------------

/// A path as UTF-8 text fit to quote: control characters (a newline above all)
/// are refused rather than quoted, so the command is always one line.
fn quotable(path: &Path) -> Result<&str, String> {
    let s = path
        .to_str()
        .ok_or_else(|| format!("the path {} isn't valid UTF-8", path.display()))?;
    if s.chars().any(char::is_control) {
        return Err(format!("the path {s:?} contains a control character"));
    }
    Ok(s)
}

/// POSIX shell single quotes: everything literal, `'` written as `'\''`.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// An AppleScript string literal: backslash and double quote escaped.
pub fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// The shell command the admin prompt runs: make the directory, then link
/// (`-f` replaces a file or link already there, `-n` doesn't follow a link to a
/// directory). Only the two paths vary, and both are quoted.
pub fn admin_shell_command(link: &Path, target: &Path) -> Result<String, String> {
    let dir = link
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", link.display()))?;
    Ok(format!(
        "/bin/mkdir -p {} && /bin/ln -sfn {} {}",
        sh_quote(quotable(dir)?),
        sh_quote(quotable(target)?),
        sh_quote(quotable(link)?),
    ))
}

/// The AppleScript `osascript -e` runs for the admin prompt.
pub fn admin_osascript(link: &Path, target: &Path) -> Result<String, String> {
    Ok(format!(
        "do shell script {} with administrator privileges",
        applescript_string(&admin_shell_command(link, target)?)
    ))
}

// --- macOS -----------------------------------------------------------------------

#[cfg(target_os = "macos")]
const MACOS_LINK: &str = "/usr/local/bin/seaquel-cli";

/// `osascript` exit: the user pressed Cancel in the password prompt.
#[cfg(target_os = "macos")]
fn osascript_cancelled(stderr: &str) -> bool {
    stderr.contains("(-128)")
}

#[cfg(target_os = "macos")]
fn install_macos(app: &tauri::AppHandle) -> Result<(Outcome, String), String> {
    let target = cli_download::installed_path(&app.config().identifier)?;
    let version = cli_download::VERSION;
    let current = cli_download::is_current(&target, version);
    let link = PathBuf::from(MACOS_LINK);
    let state =
        link_state(&link, &target).map_err(|e| format!("Couldn't check {MACOS_LINK}: {e}"))?;
    let replace = match &state {
        LinkState::Ours if current => {
            return Ok((Outcome::AlreadyInstalled, link_message(&link, &target)))
        }
        LinkState::Ours => false,
        LinkState::Directory => {
            return Err(format!(
                "{MACOS_LINK} is a directory. Remove it, then try again."
            ))
        }
        LinkState::Missing => false,
        LinkState::OtherLink(_) | LinkState::File => {
            if !confirm_replace(app, &link, &state) {
                return Ok((Outcome::Cancelled, String::new()));
            }
            true
        }
    };

    if !current {
        cli_download::install(version, &app.config().identifier)?;
    }

    if state == LinkState::Ours {
        return Ok((Outcome::Updated, link_message(&link, &target)));
    }

    match place_symlink(&link, &target, replace) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let script = admin_osascript(&link, &target)?;
            let out = std::process::Command::new("/usr/bin/osascript")
                .arg("-e")
                .arg(script)
                .output()
                .map_err(|e| format!("Couldn't run osascript: {e}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if osascript_cancelled(&stderr) {
                    return Ok((Outcome::Cancelled, String::new()));
                }
                return Err(format!("Couldn't create {MACOS_LINK}: {}", stderr.trim()));
            }
        }
        Err(e) => return Err(format!("Couldn't create {MACOS_LINK}: {e}")),
    }

    match link_state(&link, &target) {
        Ok(LinkState::Ours) => {}
        Ok(other) => {
            return Err(format!(
                "{MACOS_LINK} still isn't a link to {} ({other:?}).",
                target.display()
            ))
        }
        Err(e) => return Err(format!("Couldn't check {MACOS_LINK}: {e}")),
    }
    let outcome = if replace {
        Outcome::Replaced
    } else {
        Outcome::Installed
    };
    Ok((outcome, link_message(&link, &target)))
}

// --- Linux -----------------------------------------------------------------------

/// Whether `dir` is one of `PATH`'s entries.
pub fn on_path(dir: &Path, path_var: Option<&std::ffi::OsStr>) -> bool {
    path_var
        .map(|p| std::env::split_paths(p).any(|entry| entry == dir))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn install_linux(app: &tauri::AppHandle) -> Result<(Outcome, String), String> {
    let home = dirs::home_dir().ok_or("Couldn't find your home directory.")?;
    let target = cli_download::installed_path(&app.config().identifier)?;
    let version = cli_download::VERSION;
    let current = cli_download::is_current(&target, version);
    let link = home.join(".local").join("bin").join(CLI_NAME);
    let state = link_state(&link, &target)
        .map_err(|e| format!("Couldn't check {}: {e}", link.display()))?;
    let replace = match &state {
        LinkState::Ours if current => {
            return Ok((Outcome::AlreadyInstalled, link_message(&link, &target)))
        }
        LinkState::Ours => false,
        LinkState::Directory => {
            return Err(format!(
                "{} is a directory. Remove it, then try again.",
                link.display()
            ))
        }
        LinkState::Missing => false,
        LinkState::OtherLink(_) | LinkState::File => {
            if !confirm_replace(app, &link, &state) {
                return Ok((Outcome::Cancelled, String::new()));
            }
            true
        }
    };
    if !current {
        cli_download::install(version, &app.config().identifier)?;
    }
    let outcome = if state == LinkState::Ours {
        Outcome::Updated
    } else {
        place_symlink(&link, &target, replace)
            .map_err(|e| format!("Couldn't create {}: {e}", link.display()))?;
        if replace {
            Outcome::Replaced
        } else {
            Outcome::Installed
        }
    };
    let mut message = link_message(&link, &target);
    let bin_dir = home.join(".local").join("bin");
    if !on_path(&bin_dir, std::env::var_os("PATH").as_deref()) {
        message.push_str(&format!(
            "\n\n{} isn't on your PATH. Add it in your shell's profile to run {CLI_NAME} by name.",
            bin_dir.display()
        ));
    }
    Ok((outcome, message))
}

#[cfg(target_os = "windows")]
fn install_windows(app: &tauri::AppHandle) -> Result<(Outcome, String), String> {
    let version = cli_download::VERSION;
    let target = cli_download::installed_path(&app.config().identifier)?;
    if cli_download::is_current(&target, version) {
        return Ok((Outcome::AlreadyInstalled, target.display().to_string()));
    }
    let outcome = if target.is_file() {
        Outcome::Updated
    } else {
        Outcome::Installed
    };
    cli_download::install(version, &app.config().identifier)?;
    Ok((outcome, target.display().to_string()))
}

// --- dialogs ---------------------------------------------------------------------

fn link_message(link: &Path, target: &Path) -> String {
    format!("{} → {}", link.display(), target.display())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn confirm_replace(app: &tauri::AppHandle, link: &Path, state: &LinkState) -> bool {
    let what = match state {
        LinkState::OtherLink(to) => format!(
            "{} already exists and points to {}.",
            link.display(),
            to.display()
        ),
        _ => format!("{} already exists.", link.display()),
    };
    app.dialog()
        .message(format!(
            "{what}\n\nReplace it with Seaquel's command line tool?"
        ))
        .title(DIALOG_TITLE)
        .kind(MessageDialogKind::Warning)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
            "Replace".into(),
            "Cancel".into(),
        ))
        .blocking_show()
}

fn show(app: &tauri::AppHandle, kind: MessageDialogKind, message: String) {
    app.dialog()
        .message(message)
        .title(DIALOG_TITLE)
        .kind(kind)
        .blocking_show();
}

/// Runs the install for the menu item on its own thread (the dialogs and the
/// admin prompt block) and reports the result in a native dialog.
pub fn install_from_menu(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || install_and_report(&app));
}

/// The install behind the menu item and the settings panel's button: runs it
/// and reports the result in a native dialog. Blocks (dialogs, the admin
/// prompt), so call it off the main thread.
pub fn install_and_report(app: &tauri::AppHandle) {
    let app = app.clone();
    {
        #[cfg(target_os = "macos")]
        let result = install_macos(&app);
        #[cfg(target_os = "linux")]
        let result = install_linux(&app);
        #[cfg(target_os = "windows")]
        let result = install_windows(&app);
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let result: Result<(Outcome, String), String> =
            Err("Installing the command line tool isn't supported on this platform.".into());

        match result {
            Ok((Outcome::Cancelled, _)) => {}
            Ok((outcome, detail)) => {
                let head = match outcome {
                    Outcome::Installed => {
                        if cfg!(target_os = "windows") {
                            "Installed. Use the path below in your MCP configuration.".to_string()
                        } else {
                            format!("Installed. You can now run {CLI_NAME} in a terminal.")
                        }
                    }
                    Outcome::AlreadyInstalled => {
                        "The command line tool is already installed.".to_string()
                    }
                    Outcome::Replaced => {
                        format!("Replaced. You can now run {CLI_NAME} in a terminal.")
                    }
                    Outcome::Updated => {
                        "Updated the command line tool to this version of Seaquel.".to_string()
                    }
                    Outcome::Cancelled => unreachable!(),
                };
                let name = format!("{outcome:?}");
                log::info!(activity = "app.cli_install", outcome = name.as_str(); "{detail}");
                let helper = cli_download::install_duckdb_helper(&app.config().identifier);
                match &helper {
                    Ok(done) => {
                        log::info!(activity = "app.cli_install", event = "duckdb_helper", downloaded = done.downloaded, pruned = done.pruned; "The CLI's DuckDB helper is installed");
                    }
                    Err(e) => {
                        log::warn!(activity = "app.cli_install", event = "duckdb_helper", code = e.code.as_str(); "The CLI's DuckDB helper couldn't be installed");
                    }
                }
                let mut message = format!("{head}\n\n{detail}");
                if let Some(line) = cli_download::helper_report(&helper) {
                    message.push_str("\n\n");
                    message.push_str(&line);
                }
                show(&app, MessageDialogKind::Info, message);
            }
            Err(message) => {
                log::error!(activity = "app.cli_install"; "{message}");
                show(&app, MessageDialogKind::Error, message);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    const AWKWARD: &str = "Seaquel's \"beta\" $HOME `id` \\ app";

    fn sidecar_in(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let p = dir.join(name).join(CLI_NAME);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, contents).unwrap();
        p
    }

    #[test]
    fn sh_quote_keeps_everything_literal() {
        assert_eq!(sh_quote("/a b/c"), "'/a b/c'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        // The shell hands the quoted text back unchanged.
        for s in [
            "/Applications/Seaquel.app",
            AWKWARD,
            "a'b'c",
            "'",
            "$(rm -rf /)",
            "",
        ] {
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("printf %s {}", sh_quote(s)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), s);
        }
    }

    #[test]
    fn applescript_string_escapes_quotes_and_backslashes() {
        assert_eq!(applescript_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_string_round_trips_through_osascript() {
        let script = format!("return {}", applescript_string(&sh_quote(AWKWARD)));
        let out = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8(out.stdout)
                .unwrap()
                .trim_end_matches('\n'),
            sh_quote(AWKWARD)
        );
    }

    #[test]
    fn admin_command_quotes_both_paths() {
        let cmd = admin_shell_command(
            Path::new("/usr/local/bin/seaquel-cli"),
            Path::new("/Applications/My Apps/Seaquel's.app/Contents/MacOS/seaquel-cli"),
        )
        .unwrap();
        assert_eq!(
            cmd,
            r"/bin/mkdir -p '/usr/local/bin' && /bin/ln -sfn '/Applications/My Apps/Seaquel'\''s.app/Contents/MacOS/seaquel-cli' '/usr/local/bin/seaquel-cli'"
        );
        let script = admin_osascript(
            Path::new("/usr/local/bin/seaquel-cli"),
            Path::new("/A \"b\"/x"),
        )
        .unwrap();
        assert_eq!(
            script,
            r#"do shell script "/bin/mkdir -p '/usr/local/bin' && /bin/ln -sfn '/A \"b\"/x' '/usr/local/bin/seaquel-cli'" with administrator privileges"#
        );
    }

    #[test]
    fn admin_command_refuses_control_characters() {
        let err = admin_shell_command(
            Path::new("/usr/local/bin/seaquel-cli"),
            Path::new("/a\nb/seaquel-cli"),
        );
        assert!(err.is_err());
        let err = admin_shell_command(Path::new("/usr/local/bin/seaquel-cli"), Path::new("/a\tb"));
        assert!(err.is_err());
    }

    /// The admin command, run by a plain shell against a temp prefix (no
    /// prompt): it makes the directory, links, and replaces a stale link.
    #[test]
    fn admin_command_links_under_a_temp_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let target = sidecar_in(tmp.path(), AWKWARD, b"cli");
        let link = tmp.path().join("prefix dir/bin").join(CLI_NAME);
        for _ in 0..2 {
            let status = Command::new("/bin/sh")
                .arg("-c")
                .arg(admin_shell_command(&link, &target).unwrap())
                .status()
                .unwrap();
            assert!(status.success());
            assert_eq!(link_state(&link, &target).unwrap(), LinkState::Ours);
        }
        let other = sidecar_in(tmp.path(), "other", b"old");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        let status = Command::new("/bin/sh")
            .arg("-c")
            .arg(admin_shell_command(&link, &target).unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Ours);
    }

    #[test]
    fn link_state_and_place_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = sidecar_in(tmp.path(), "My App.app/Contents/MacOS", b"cli");
        let link = tmp.path().join("usr/local/bin").join(CLI_NAME);

        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Missing);
        place_symlink(&link, &target, false).unwrap();
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Ours);

        // Pointing elsewhere, then a dangling link, then a plain file.
        let other = sidecar_in(tmp.path(), "Old.app", b"old");
        place_symlink(&link, &other, true).unwrap();
        assert_eq!(
            link_state(&link, &target).unwrap(),
            LinkState::OtherLink(other.clone())
        );
        fs::remove_file(&other).unwrap();
        assert_eq!(
            link_state(&link, &target).unwrap(),
            LinkState::OtherLink(other)
        );
        fs::remove_file(&link).unwrap();
        fs::write(&link, b"someone else's").unwrap();
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::File);
        // Without `replace` nothing is overwritten.
        assert_eq!(
            place_symlink(&link, &target, false).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        place_symlink(&link, &target, true).unwrap();
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Ours);

        // A relative link to the same file counts as ours.
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink("../../../My App.app/Contents/MacOS/seaquel-cli", &link)
            .unwrap();
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Ours);

        fs::remove_file(&link).unwrap();
        fs::create_dir(&link).unwrap();
        assert_eq!(link_state(&link, &target).unwrap(), LinkState::Directory);
    }

    #[test]
    fn place_symlink_reports_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let target = sidecar_in(tmp.path(), "app", b"cli");
        let locked = tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let err = place_symlink(&locked.join(CLI_NAME), &target, false).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        // root ignores the mode bits; the escalation path is for everyone else.
        if std::env::var("USER").as_deref() != Ok("root") {
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn on_path_matches_whole_entries() {
        let path = std::env::join_paths(["/usr/bin", "/home/me/.local/bin"]).unwrap();
        assert!(on_path(Path::new("/home/me/.local/bin"), Some(&path)));
        assert!(!on_path(Path::new("/home/me/.local"), Some(&path)));
        assert!(!on_path(Path::new("/usr/bin"), None));
    }
}
