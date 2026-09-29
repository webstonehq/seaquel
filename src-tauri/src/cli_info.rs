//! What the MCP settings panel needs to know about the installed `seaquel-cli`,
//! and its install button. Two Tauri commands rather than a `core_call`
//! group: the answers come from the local CLI install and the install shows
//! native dialogs through the `AppHandle`, none
//! of which `seaquel-rpc` can reach, and a group would also need wire types,
//! a web refusal and desktop routing for a desktop-only question.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli_download;
use crate::cli_install;

/// Whether `seaquel-cli`, typed in a terminal, runs this app's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathStatus {
    /// It resolves to this version's installed CLI.
    Installed,
    /// It resolves to a CLI installed for another app version.
    Outdated,
    /// It resolves to some other file.
    Other,
    /// Not found.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInfo {
    /// The stable CLI install path in the user's app data directory.
    pub binary_path: String,
    pub binary_exists: bool,
    pub binary_current: bool,
    /// The path an MCP host should run.
    pub command_path: String,
    pub path_status: PathStatus,
    /// What `seaquel-cli` resolves to, when found.
    pub found_path: Option<String>,
    /// Whether the install button is offered on this desktop platform.
    pub can_install: bool,
    pub app_image: bool,
}

fn exe_name() -> String {
    format!("{}{}", cli_install::CLI_NAME, std::env::consts::EXE_SUFFIX)
}

/// The first `name` in `dirs` that is a file.
fn find_in(dirs: impl IntoIterator<Item = PathBuf>, name: &str) -> Option<PathBuf> {
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Where a terminal would look: `PATH`, then the places the install links into.
/// A GUI app on macOS starts with launchd's short `PATH`, which lacks
/// `/usr/local/bin` though every terminal has it (`/etc/paths`).
fn search_dirs(path_var: Option<&OsStr>, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path_var
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for d in extra {
        if !dirs.contains(d) {
            dirs.push(d.clone());
        }
    }
    dirs
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The status of `found` against this version's installed CLI.
pub fn path_status(found: Option<&Path>, binary: &Path, current: bool) -> PathStatus {
    let Some(found) = found else {
        return PathStatus::Missing;
    };
    if same_file(found, binary) {
        if current {
            PathStatus::Installed
        } else {
            PathStatus::Outdated
        }
    } else {
        PathStatus::Other
    }
}

#[tauri::command]
pub fn cli_info(app: tauri::AppHandle) -> Result<CliInfo, String> {
    let binary = cli_download::installed_path(&app.config().identifier)?;
    let app_image = cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some();

    let mut extra = Vec::new();
    if cfg!(target_os = "macos") {
        extra.push(PathBuf::from("/usr/local/bin"));
    }
    if cfg!(target_os = "linux") {
        if let Some(home) = dirs::home_dir() {
            extra.push(home.join(".local").join("bin"));
        }
    }
    let found = find_in(
        search_dirs(std::env::var_os("PATH").as_deref(), &extra),
        &exe_name(),
    );
    let current = cli_download::is_current(&binary, cli_download::VERSION);

    Ok(CliInfo {
        binary_path: binary.display().to_string(),
        binary_exists: binary.is_file(),
        binary_current: current,
        command_path: binary.display().to_string(),
        path_status: path_status(found.as_deref(), &binary, current),
        found_path: found.map(|p| p.display().to_string()),
        can_install: cli_install::available(),
        app_image,
    })
}

/// The settings panel's install button: the menu item's install, dialogs and
/// all. Resolves once the user has seen the result, so the panel can refresh.
#[tauri::command]
pub async fn install_cli(app: tauri::AppHandle) -> Result<(), String> {
    if !cli_install::available() {
        return Err("Installing the command line tool isn't supported here.".into());
    }
    tauri::async_runtime::spawn_blocking(move || cli_install::install_and_report(&app))
        .await
        .map_err(|e| format!("The install stopped unexpectedly: {e}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_on_path_then_in_extra_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("c"),
        );
        for d in [&a, &b, &c] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(b.join("tool"), b"x").unwrap();
        fs::write(c.join("tool"), b"x").unwrap();
        let path = std::env::join_paths([&a, &b]).unwrap();
        let dirs = search_dirs(Some(&path), &[c.clone(), b.clone()]);
        assert_eq!(dirs, vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(find_in(dirs, "tool"), Some(b.join("tool")));
        let only_extra = search_dirs(None, std::slice::from_ref(&c));
        assert_eq!(find_in(only_extra, "tool"), Some(c.join("tool")));
        assert_eq!(find_in(search_dirs(None, &[a]), "tool"), None);
    }

    #[test]
    fn status_follows_the_installed_binary_and_version() {
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("data/bin/seaquel-cli");
        let link = tmp.path().join("bin/seaquel-cli");
        let other = tmp.path().join("other/seaquel-cli");
        for p in [&binary, &link, &other] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
        }
        fs::write(&binary, b"v2").unwrap();
        fs::write(&other, b"v2").unwrap();

        assert_eq!(path_status(None, &binary, true), PathStatus::Missing);
        std::os::unix::fs::symlink(&binary, &link).unwrap();
        assert_eq!(
            path_status(Some(&link), &binary, true),
            PathStatus::Installed
        );
        assert_eq!(path_status(Some(&other), &binary, true), PathStatus::Other);
        assert_eq!(
            path_status(Some(&link), &binary, false),
            PathStatus::Outdated
        );
    }
}
