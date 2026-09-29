//! Download the version-matched standalone CLI release asset on demand.
//! GitHub's release metadata supplies the asset size and SHA-256 digest; the
//! executable is installed only after both have been checked.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

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

fn asset_name_for(os: &str, arch: &str) -> Result<String, String> {
    let triple = match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return Err("No command line tool is published for this platform.".into()),
    };
    let suffix = if os == "windows" { ".exe" } else { "" };
    Ok(format!("{CLI_NAME}-{triple}{suffix}"))
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
