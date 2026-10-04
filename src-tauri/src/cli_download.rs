//! Download the version-matched standalone CLI release asset on demand.
//! GitHub's release metadata supplies the asset size and SHA-256 digest; the
//! executable is installed only after both have been checked.
//!
//! **The CLI's DuckDB helper** (the DuckDB helper plan, Q4 C, Decisions 9
//! and 10). The CLI runs DuckDB in the `seaquel-duckdb` helper, so the
//! install also fetches it ([`install_duckdb_helper`]). That goes through
//! Core's own install (`Core::duckdb_helper_install`, the TUI's and
//! `seaquel-cli duckdb install`'s): the gzipped asset, size and SHA-256
//! checked, folders and file 0700, pruning, into
//! `seaquel_core::storage::data_local_dir(identifier)` + `bin/duckdb/<version>/`,
//! which follows `SEAQUEL_DATA_DIR` as the terminal binaries do. The app
//! builds a small Core of its own for it, with only the remote DuckDB
//! engine's locator (the app's main Core keeps the native driver, Q6 A).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use seaquel_core::{
    Core, CoreError, DuckdbHelper, DuckdbHelperInstalled, DuckdbHelperReleases, DuckdbHelperStatus,
};

use crate::cli_install::CLI_NAME;

const MAX_CLI_BYTES: u64 = 200 * 1024 * 1024;
const RELEASES: &str = "https://api.github.com/repos/webstonehq/seaquel/releases/tags";
/// Cargo's version keeps the full release tag on Windows, where the Tauri
/// bundle version is shortened for MSI compatibility.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    digest: Option<String>,
}

pub fn installed_path(identifier: &str) -> Result<PathBuf, String> {
    let data = dirs::data_local_dir().ok_or("Couldn't find your data directory.")?;
    Ok(data
        .join(identifier)
        .join("bin")
        .join(format!("{CLI_NAME}{}", std::env::consts::EXE_SUFFIX)))
}

fn asset_name() -> Result<String, String> {
    asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The release targets (`release.yml`'s matrix). The same table as
/// `seaquel-http`'s `target_triple`, which names the helper's asset; a test
/// keeps them equal.
fn triple_for(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

fn asset_name_for(os: &str, arch: &str) -> Result<String, String> {
    let triple =
        triple_for(os, arch).ok_or("No command line tool is published for this platform.")?;
    let suffix = if os == "windows" { ".exe" } else { "" };
    Ok(format!("{CLI_NAME}-{triple}{suffix}"))
}

/// The helper's release asset, `seaquel-duckdb-<triple>[.exe].gz`, as Core
/// asks for it.
#[cfg(test)]
fn helper_asset_name_for(os: &str, arch: &str) -> Result<String, String> {
    let triple = triple_for(os, arch).ok_or("No DuckDB helper is published for this platform.")?;
    let suffix = if os == "windows" { ".exe" } else { "" };
    Ok(format!("seaquel-duckdb-{triple}{suffix}.gz"))
}

fn expected_hash(digest: Option<&str>) -> Result<String, String> {
    let hash = digest
        .and_then(|value| value.strip_prefix("sha256:"))
        .ok_or("The CLI release asset has no SHA-256 digest. Installation was stopped.")?;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The CLI release asset has an invalid SHA-256 digest.".into());
    }
    Ok(hash.to_ascii_lowercase())
}

fn save_verified(
    mut source: impl Read,
    target: &Path,
    size: u64,
    expected: &str,
) -> Result<(), String> {
    if size == 0 || size > MAX_CLI_BYTES {
        return Err("The CLI release asset has an invalid size.".into());
    }
    let parent = target
        .parent()
        .ok_or("The CLI install path has no parent directory.")?;
    fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Couldn't create a temporary CLI file: {e}"))?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let count = source
            .read(&mut chunk)
            .map_err(|e| format!("Couldn't download the CLI: {e}"))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > size || total > MAX_CLI_BYTES {
            return Err("The CLI download is larger than its release metadata says.".into());
        }
        hasher.update(&chunk[..count]);
        temp.write_all(&chunk[..count])
            .map_err(|e| format!("Couldn't write the CLI download: {e}"))?;
    }
    if total != size {
        return Err(format!(
            "The CLI download is incomplete ({total} of {size} bytes)."
        ));
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err("The CLI download failed its SHA-256 integrity check.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Couldn't make the CLI executable: {e}"))?;
    }
    temp.persist(target)
        .map_err(|e| format!("Couldn't install {}: {}", target.display(), e.error))?;
    Ok(())
}

/// Uses a nearby debug build when available, so unreleased source checkouts
/// can exercise the install button. Production builds always download.
#[cfg(debug_assertions)]
fn debug_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cli = exe
        .parent()?
        .join(format!("{CLI_NAME}{}", std::env::consts::EXE_SUFFIX));
    cli.is_file().then_some(cli)
}

pub fn install(version: &str, identifier: &str) -> Result<PathBuf, String> {
    let target = installed_path(identifier)?;
    #[cfg(debug_assertions)]
    if let Some(local) = debug_cli() {
        let len = fs::metadata(&local)
            .map_err(|e| format!("Couldn't read local CLI: {e}"))?
            .len();
        let hash = {
            let mut file =
                fs::File::open(&local).map_err(|e| format!("Couldn't read local CLI: {e}"))?;
            let mut hasher = Sha256::new();
            std::io::copy(&mut file, &mut hasher)
                .map_err(|e| format!("Couldn't hash local CLI: {e}"))?;
            format!("{:x}", hasher.finalize())
        };
        let file = fs::File::open(&local).map_err(|e| format!("Couldn't read local CLI: {e}"))?;
        save_verified(file, &target, len, &hash)?;
        write_version(&target, version)?;
        return Ok(target);
    }

    let name = asset_name()?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Seaquel/{version}"))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Couldn't prepare the CLI download: {e}"))?;
    let tag = format!("v{version}");
    let release: Release = client
        .get(format!("{RELEASES}/{tag}"))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("Couldn't find the Seaquel {version} CLI release: {e}"))?
        .json()
        .map_err(|e| format!("Couldn't read the CLI release metadata: {e}"))?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| {
            format!("The Seaquel {version} release doesn't have a CLI for this platform yet.")
        })?;
    let hash = expected_hash(asset.digest.as_deref())?;
    let download = format!("https://github.com/webstonehq/seaquel/releases/download/{tag}/{name}");
    let response = client
        .get(download)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("Couldn't download the CLI: {e}"))?;
    save_verified(response, &target, asset.size, &hash)?;
    write_version(&target, version)?;
    Ok(target)
}

fn version_path(target: &Path) -> PathBuf {
    target.with_extension("version")
}

fn write_version(target: &Path, version: &str) -> Result<(), String> {
    fs::write(version_path(target), version)
        .map_err(|e| format!("Couldn't record the installed CLI version: {e}"))
}

pub fn is_current(target: &Path, version: &str) -> bool {
    target.is_file()
        && fs::read_to_string(version_path(target))
            .map(|installed| installed == version)
            .unwrap_or(false)
}

// --- the DuckDB helper ------------------------------------------------------------

/// The helper's folder, `<data_local_dir>/<identifier>/bin/duckdb`
/// (`$SEAQUEL_DATA_DIR/bin/duckdb` when that is set): where the terminal
/// binaries of this identifier look (`seaquel_terminal::duckdb_helper_dir`).
pub fn duckdb_helper_dir(identifier: &str) -> Result<PathBuf, String> {
    let root = seaquel_core::storage::data_local_dir(identifier)
        .map_err(|_| "Couldn't find your data directory.".to_string())?;
    Ok(root.join("bin").join("duckdb"))
}

/// A Core that only knows where this version's helper goes. `releases`
/// (tests, debug builds) replaces GitHub as the source.
fn helper_core(dir: PathBuf, releases: Option<DuckdbHelperReleases>) -> Core {
    let builder = seaquel_core::with_plugins(|_| false).duckdb_helper(DuckdbHelper {
        dir,
        version: VERSION.to_string(),
    });
    #[cfg(debug_assertions)]
    let builder = match releases {
        Some(source) => builder.duckdb_helper_releases(source),
        None => builder,
    };
    #[cfg(not(debug_assertions))]
    let _ = releases;
    builder.build()
}

/// Whether this version's helper is installed for the CLI (the settings
/// panel). Reads metadata only.
pub fn duckdb_helper_status(identifier: &str) -> Option<DuckdbHelperStatus> {
    let dir = duckdb_helper_dir(identifier).ok()?;
    helper_core(dir, None).duckdb_helper_status().ok()
}

/// A `seaquel-duckdb` built beside a debug app, as [`debug_cli`] for the
/// CLI. Production builds always download.
#[cfg(debug_assertions)]
fn debug_helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let helper = exe
        .parent()?
        .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
    helper.is_file().then_some(helper)
}

fn hash_file(path: &Path) -> Result<String, CoreError> {
    let unreadable = |e: std::io::Error| CoreError::new("FILE_ERROR", format!("{:?}", e.kind()));
    let mut file = fs::File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(unreadable)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Core's install of this version's helper: from `local` (a debug build's
/// own helper, checked against its own hash) or downloaded. Nothing is
/// fetched when it is already there and intact.
async fn install_helper(
    core: &Core,
    local: Option<&Path>,
) -> Result<DuckdbHelperInstalled, CoreError> {
    match local {
        Some(file) => {
            let sha256 = hash_file(file)?;
            core.duckdb_helper_install_from_file(file, &sha256).await
        }
        None => core.duckdb_helper_install(&mut |_| {}).await,
    }
}

/// Installs the CLI's DuckDB helper for `identifier`. Blocks (its own
/// runtime), so call it off the main thread, as the CLI's install is.
pub fn install_duckdb_helper(identifier: &str) -> Result<DuckdbHelperInstalled, CoreError> {
    let dir = duckdb_helper_dir(identifier).map_err(|m| CoreError::new("NO_DATA_DIR", m))?;
    let core = helper_core(dir, None);
    #[cfg(debug_assertions)]
    let local = debug_helper();
    #[cfg(not(debug_assertions))]
    let local: Option<PathBuf> = None;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CoreError::new("UNKNOWN", format!("can't start the install: {e}")))?;
    runtime.block_on(install_helper(&core, local.as_deref()))
}

/// What the install dialog adds about the helper: nothing when it was
/// already installed, a line when it was downloaded, and on a failure why,
/// how to retry and the code (Core's messages hold no path or URL).
pub fn helper_report(result: &Result<DuckdbHelperInstalled, CoreError>) -> Option<String> {
    let e = match result {
        Ok(done) if !done.downloaded => return None,
        Ok(_) => return Some("DuckDB support for the command line tool was installed too.".into()),
        Err(e) => e,
    };
    let why = match e.code.as_str() {
        "NETWORK_ERROR" => "the download server couldn't be reached",
        "RELEASE_NOT_FOUND" | "ASSET_NOT_FOUND" => {
            "it isn't published for this version and platform yet"
        }
        "DIGEST_MISMATCH" | "SIZE_MISMATCH" | "GZIP_ERROR" => "the download was damaged",
        "FILE_ERROR" => "it couldn't be saved",
        "UNSAFE_FOLDER" => "a folder on the way to it belongs to another user or is a link",
        "NOT_SUPPORTED" => "this platform has no DuckDB download",
        _ => "the install failed",
    };
    Some(format!(
        "DuckDB support for the command line tool couldn't be installed: {why}. The tool works \
         without it except for DuckDB connections. Run \"seaquel-cli duckdb install\" to try \
         again.\n({}: {})",
        e.code, e.message
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_missing_or_invalid_digest() {
        assert!(expected_hash(None).is_err());
        assert!(expected_hash(Some("sha256:abc")).is_err());
        assert_eq!(
            expected_hash(Some(&format!("sha256:{}", "A".repeat(64)))).unwrap(),
            "a".repeat(64)
        );
    }

    #[test]
    fn release_asset_names_match_ci_targets() {
        assert_eq!(
            asset_name_for("macos", "aarch64").unwrap(),
            "seaquel-cli-aarch64-apple-darwin"
        );
        assert_eq!(
            asset_name_for("linux", "x86_64").unwrap(),
            "seaquel-cli-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            asset_name_for("windows", "aarch64").unwrap(),
            "seaquel-cli-aarch64-pc-windows-msvc.exe"
        );
        assert!(asset_name_for("macos", "i686").is_err());
    }

    #[test]
    fn installs_only_a_complete_matching_download() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(CLI_NAME);
        let bytes = b"cli";
        let hash = format!("{:x}", Sha256::digest(bytes));
        save_verified(Cursor::new(bytes), &target, bytes.len() as u64, &hash).unwrap();
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(save_verified(Cursor::new(b"bad"), &target, 3, &hash).is_err());
        assert_eq!(fs::read(&target).unwrap(), bytes);
        assert!(save_verified(Cursor::new(bytes), &target, 4, &hash).is_err());
        assert_eq!(fs::read(&target).unwrap(), bytes);

        let updated = b"cli-v2";
        let updated_hash = format!("{:x}", Sha256::digest(updated));
        save_verified(
            Cursor::new(updated),
            &target,
            updated.len() as u64,
            &updated_hash,
        )
        .unwrap();
        assert_eq!(fs::read(&target).unwrap(), updated);
    }

    /// The app installs the DuckDB helper's asset under the names
    /// `release.yml` uploads and Core asks for: the CLI's triple table and
    /// `seaquel-http`'s agree on every platform.
    #[test]
    fn helper_asset_names_match_ci_targets() {
        assert_eq!(
            helper_asset_name_for("macos", "aarch64").unwrap(),
            "seaquel-duckdb-aarch64-apple-darwin.gz"
        );
        assert_eq!(
            helper_asset_name_for("linux", "x86_64").unwrap(),
            "seaquel-duckdb-x86_64-unknown-linux-gnu.gz"
        );
        assert_eq!(
            helper_asset_name_for("windows", "x86_64").unwrap(),
            "seaquel-duckdb-x86_64-pc-windows-msvc.exe.gz"
        );
        assert!(helper_asset_name_for("macos", "i686").is_err());
        for os in ["macos", "linux", "windows", "freebsd"] {
            for arch in ["aarch64", "x86_64", "i686"] {
                assert_eq!(
                    triple_for(os, arch),
                    seaquel_http::release_asset::target_triple(os, arch),
                    "{os} {arch}"
                );
            }
        }
    }

    /// Task 6's review: the helper's folder is `data_local_dir` +
    /// `bin/duckdb`, so with `SEAQUEL_DATA_DIR` set it is
    /// `$SEAQUEL_DATA_DIR/bin/duckdb`, where the terminal binaries look.
    /// The variable is process-wide, so the check runs in a child process
    /// of this test binary.
    #[test]
    fn the_helper_folder_follows_seaquel_data_dir() {
        const CHILD: &str = "SEAQUEL_TEST_HELPER_DIR_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let data = std::env::var_os("SEAQUEL_DATA_DIR").unwrap();
            assert_eq!(
                duckdb_helper_dir("app.seaquel.desktop").unwrap(),
                PathBuf::from(data).join("bin").join("duckdb")
            );
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli_download::tests::the_helper_folder_follows_seaquel_data_dir",
                "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("SEAQUEL_DATA_DIR", tmp.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}");
        assert!(
            stdout.contains("1 passed"),
            "the child ran the check: {stdout}"
        );
    }

    fn helper_name() -> String {
        helper_asset_name_for(std::env::consts::OS, std::env::consts::ARCH).unwrap()
    }

    /// The app's install puts the helper where the terminal binaries look,
    /// through Core's install (size, digest, gzip, 0700), from a release
    /// server on 127.0.0.1. A second install fetches nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn installs_the_helper_where_the_terminal_binaries_look() {
        use seaquel_http::release_asset::testing::{asset_path, gzip, MockReleases};
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app").join("bin").join("duckdb");
        let helper = b"#!/bin/sh\nexit 0\n";
        let mock = MockReleases::start().await;
        mock.publish(VERSION, &helper_name(), gzip(helper));
        let core = helper_core(dir.clone(), Some(mock.source()));
        assert_eq!(
            core.duckdb_helper_status().unwrap(),
            seaquel_core::DuckdbHelperStatus::Missing
        );

        let done = install_helper(&core, None).await.unwrap();
        let expected = dir
            .join(VERSION)
            .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(done.path, expected);
        assert!(done.downloaded);
        assert_eq!(fs::read(&expected).unwrap(), helper);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&expected), 0o700);
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&dir.join(VERSION)), 0o700);
        }
        assert_eq!(
            core.duckdb_helper_status().unwrap(),
            seaquel_core::DuckdbHelperStatus::Installed { path: expected }
        );
        let again = install_helper(&core, None).await.unwrap();
        assert!(!again.downloaded);
        assert_eq!(mock.hits(&asset_path(VERSION, &helper_name())), 1);
    }

    /// A debug build installs a helper built beside it from its file,
    /// checked against its own hash, without a network.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_local_helper_is_installed_from_its_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("app").join("bin").join("duckdb");
        let local = tmp.path().join("seaquel-duckdb");
        fs::write(&local, b"local helper").unwrap();
        let nowhere = seaquel_core::DuckdbHelperReleases::new(
            "http://127.0.0.1:9/api",
            "http://127.0.0.1:9/download",
        )
        .unwrap();
        let core = helper_core(dir.clone(), Some(nowhere));
        let done = install_helper(&core, Some(&local)).await.unwrap();
        assert_eq!(fs::read(&done.path).unwrap(), b"local helper");
        assert!(matches!(
            core.duckdb_helper_status().unwrap(),
            seaquel_core::DuckdbHelperStatus::Installed { .. }
        ));
    }

    /// What the install dialog adds about the helper: nothing when it was
    /// already there, a line when it was downloaded, and on a failure the
    /// reason, the code and the command that retries it.
    #[test]
    fn the_dialog_says_how_the_helper_install_went() {
        let installed = |downloaded| seaquel_core::DuckdbHelperInstalled {
            path: PathBuf::from("/x"),
            downloaded,
            pruned: 0,
        };
        assert_eq!(helper_report(&Ok(installed(false))), None);
        assert!(helper_report(&Ok(installed(true)))
            .unwrap()
            .contains("DuckDB support"));
        let failed = helper_report(&Err(seaquel_core::CoreError::new(
            "NETWORK_ERROR",
            "the connection failed",
        )))
        .unwrap();
        assert!(failed.contains("couldn't be installed"), "{failed}");
        assert!(failed.contains("seaquel-cli duckdb install"), "{failed}");
        assert!(failed.contains("NETWORK_ERROR"), "{failed}");
    }

    #[test]
    fn installed_version_marks_an_outdated_cli() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(CLI_NAME);
        fs::write(&target, b"cli").unwrap();
        assert!(!is_current(&target, "2026.9.2"));
        write_version(&target, "2026.9.2").unwrap();
        assert!(is_current(&target, "2026.9.2"));
        assert!(!is_current(&target, "2026.9.3"));
    }
}
