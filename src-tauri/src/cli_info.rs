//! What the MCP settings panel needs to know about the bundled `seaquel-cli`,
//! and its install button. Two Tauri commands rather than a `core_call`
//! group: the answers come from the app bundle (`current_exe`, the AppImage
//! mount) and the install shows native dialogs through the `AppHandle`, none
//! of which `seaquel-rpc` can reach, and a group would also need wire types,
//! a web refusal and desktop routing for a desktop-only question.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli_install;

/// Whether `seaquel-cli`, typed in a terminal, runs this app's tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PathStatus {
    /// It resolves to this app's sidecar (or, for an AppImage, to the copy of it).
    Installed,
    /// AppImage: it resolves to the copy, but the copy is from another version.
    Outdated,
    /// It resolves to some other file.
    Other,
    /// Not found.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInfo {
    /// The sidecar next to the app's executable.
    pub sidecar_path: String,
    pub sidecar_exists: bool,
    /// The path an MCP host should run: the sidecar, or for an AppImage the
    /// stable copy the install makes (the image's mount path changes on every
    /// run).
    pub command_path: String,
    pub path_status: PathStatus,
    /// What `seaquel-cli` resolves to, when found.
    pub found_path: Option<String>,
    /// Whether the install button is offered (macOS, Linux AppImage).
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

fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The status of `found` against the sidecar and, for an AppImage, its copy.
pub fn path_status(found: Option<&Path>, sidecar: &Path, copy: Option<&Path>) -> PathStatus {
    let Some(found) = found else {
        return PathStatus::Missing;
    };
    if same_file(found, sidecar) {
        return PathStatus::Installed;
    }
    match copy {
        Some(copy) if same_file(found, copy) => {
            if same_contents(copy, sidecar) {
                PathStatus::Installed
            } else {
                PathStatus::Outdated
            }
        }
        _ => PathStatus::Other,
    }
}

#[tauri::command]
pub fn cli_info(app: tauri::AppHandle) -> Result<CliInfo, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Couldn't find the app: {e}"))?;
    let sidecar = exe
        .parent()
        .ok_or("The app's executable has no parent directory.")?
        .join(exe_name());
    let app_image = cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some();
    let copy = if app_image {
        Some(cli_install::appimage_paths(&app)?.copy)
    } else {
        None
    };

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

    Ok(CliInfo {
        sidecar_path: sidecar.display().to_string(),
        sidecar_exists: sidecar.is_file(),
        command_path: copy.as_deref().unwrap_or(&sidecar).display().to_string(),
        path_status: path_status(found.as_deref(), &sidecar, copy.as_deref()),
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
    fn status_follows_links_and_compares_the_appimage_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let sidecar = tmp.path().join("app dir/seaquel-cli");
        let copy = tmp.path().join("data/bin/seaquel-cli");
        let link = tmp.path().join("bin/seaquel-cli");
        let other = tmp.path().join("other/seaquel-cli");
        for p in [&sidecar, &copy, &link, &other] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
        }
        fs::write(&sidecar, b"v2").unwrap();
        fs::write(&other, b"v2").unwrap();

        assert_eq!(path_status(None, &sidecar, None), PathStatus::Missing);
        std::os::unix::fs::symlink(&sidecar, &link).unwrap();
        assert_eq!(
            path_status(Some(&link), &sidecar, None),
            PathStatus::Installed
        );
        assert_eq!(path_status(Some(&other), &sidecar, None), PathStatus::Other);

        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&copy, &link).unwrap();
        fs::write(&copy, b"v1").unwrap();
        assert_eq!(
            path_status(Some(&link), &sidecar, Some(&copy)),
            PathStatus::Outdated
        );
        fs::write(&copy, b"v2").unwrap();
        assert_eq!(
            path_status(Some(&link), &sidecar, Some(&copy)),
            PathStatus::Installed
        );
    }
}
