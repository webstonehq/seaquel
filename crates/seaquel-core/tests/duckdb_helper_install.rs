//! Core's DuckDB helper status and install:
//! `duckdb_helper_status`, `duckdb_helper_asset`,
//! `duckdb_helper_install` and `duckdb_helper_install_from_file`, against
//! `seaquel-http`'s release server on 127.0.0.1. Installs go into temp
//! folders under `CARGO_TARGET_TMPDIR` (so a helper started from one is
//! found by `pgrep -f` on the target dir); nothing reaches GitHub.
//!
//! The last test installs the real helper (`SEAQUEL_TEST_DUCKDB_HELPER`,
//! gzipped and served) and connects through it; it is skipped without the
//! variable and fails with `SEAQUEL_TEST_REQUIRE_ENGINES`.
#![cfg(all(
    feature = "workspace",
    feature = "storage",
    feature = "duckdb-helper-install"
))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use seaquel_core::{
    Core, DuckdbHelper, DuckdbHelperInstalled, DuckdbHelperProgress, DuckdbHelperStatus,
};
use seaquel_http::release_asset::target_triple;
use seaquel_http::release_asset::testing::{asset_path, digest, gzip, release_path, MockReleases};

const VERSION: &str = "2026.10.4";
const LIMIT: Duration = Duration::from_secs(60);

/// What `release.yml` names this platform's helper asset.
fn asset_name() -> String {
    let triple = target_triple(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    format!("seaquel-duckdb-{triple}{}.gz", std::env::consts::EXE_SUFFIX)
}

fn fake_helper() -> Vec<u8> {
    let mut v = b"#!/bin/sh\nexit 0\n".to_vec();
    v.extend((0..200_000u32).map(|i| (i % 251) as u8));
    v
}

/// `<tmp>/app.seaquel.test/bin/duckdb`, and the temp dir holding it.
fn layout() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("duckdb-install-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let dir = tmp
        .path()
        .join("app.seaquel.test")
        .join("bin")
        .join("duckdb");
    (tmp, dir)
}

fn core_with(dir: &Path, version: &str, mock: Option<&MockReleases>) -> Core {
    let mut b = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(DuckdbHelper {
            dir: dir.to_path_buf(),
            version: version.to_string(),
        })
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor));
    if let Some(m) = mock {
        b = b.duckdb_helper_releases(m.source());
    }
    b.build()
}

async fn install(core: &Core) -> Result<DuckdbHelperInstalled, seaquel_core::CoreError> {
    tokio::time::timeout(LIMIT, core.duckdb_helper_install(&mut |_| {}))
        .await
        .expect("the install ended in time")
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[tokio::test]
async fn status_and_install_without_a_helper_are_not_supported() {
    let core = seaquel_core::with_plugins(|id| id != "duckdb")
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    assert_eq!(
        core.duckdb_helper_status().unwrap_err().code,
        "NOT_SUPPORTED"
    );
    assert_eq!(install(&core).await.unwrap_err().code, "NOT_SUPPORTED");
    assert_eq!(
        core.duckdb_helper_asset().await.unwrap_err().code,
        "NOT_SUPPORTED"
    );
}

/// The install lands where `HelperLocator`'s check looks, and the check
/// passes on the folders it made: the status says installed.
#[tokio::test]
async fn an_install_is_where_the_check_looks() {
    let mock = MockReleases::start().await;
    let bytes = fake_helper();
    let gz = gzip(&bytes);
    mock.publish(VERSION, &asset_name(), gz.clone());
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );

    let asset = core.duckdb_helper_asset().await.unwrap();
    assert_eq!(asset.name, asset_name());
    assert_eq!(asset.size, gz.len() as u64);

    let mut seen: Vec<DuckdbHelperProgress> = Vec::new();
    let done = tokio::time::timeout(LIMIT, core.duckdb_helper_install(&mut |p| seen.push(p)))
        .await
        .unwrap()
        .unwrap();
    assert!(done.downloaded);
    let helper = core.duckdb_helper().unwrap();
    assert_eq!(done.path, helper.path());
    assert_eq!(helper.check().unwrap(), done.path);
    assert_eq!(std::fs::read(&done.path).unwrap(), bytes);
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed {
            path: done.path.clone()
        }
    );
    assert_eq!(seen.last().unwrap().bytes, gz.len() as u64);
    #[cfg(unix)]
    {
        assert_eq!(mode(&done.path), 0o700);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(dir.parent().unwrap()), 0o700);
        assert_eq!(mode(&dir.join(VERSION)), 0o700);
    }
    // The User-Agent names the version, nothing else.
    for r in mock.requests() {
        assert_eq!(r.header("user-agent"), Some(&*format!("Seaquel/{VERSION}")));
    }
}

/// Installing the version that is already there and intact downloads
/// nothing, and needs no network.
#[tokio::test]
async fn installing_again_is_idempotent() {
    let mock = MockReleases::start().await;
    mock.publish(VERSION, &asset_name(), gzip(&fake_helper()));
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    let first = install(&core).await.unwrap();
    let requests = mock.requests().len();
    let again = install(&core).await.unwrap();
    assert!(!again.downloaded);
    assert_eq!(again.path, first.path);
    assert_eq!(
        mock.requests().len(),
        requests,
        "the second install fetched"
    );

    // A file changed since (not the one installed) is downloaded again.
    std::fs::write(&first.path, b"something else").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&first.path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let repaired = install(&core).await.unwrap();
    assert!(repaired.downloaded);
    assert_eq!(std::fs::read(&repaired.path).unwrap(), fake_helper());
}

/// Two installs at once in one Core: the second waits for the first and
/// then finds its file, so the asset is fetched once.
#[tokio::test]
async fn two_installs_at_once_download_once() {
    let mock = MockReleases::start().await;
    let name = asset_name();
    mock.publish(VERSION, &name, gzip(&fake_helper()));
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    let (a, b) = tokio::join!(install(&core), install(&core));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.path, b.path);
    assert_eq!(
        [a.downloaded, b.downloaded].iter().filter(|d| **d).count(),
        1
    );
    assert_eq!(mock.hits(&asset_path(VERSION, &name)), 1);
    assert_eq!(std::fs::read(&a.path).unwrap(), fake_helper());

    // Two Cores (two processes' worth): both download, both succeed.
    let (_tmp2, dir2) = layout();
    let c1 = core_with(&dir2, VERSION, Some(&mock));
    let c2 = core_with(&dir2, VERSION, Some(&mock));
    let (a, b) = tokio::join!(install(&c1), install(&c2));
    assert_eq!(a.unwrap().path, b.unwrap().path);
    assert!(matches!(
        c1.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

#[tokio::test]
async fn refusals_carry_their_codes_and_leave_nothing() {
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper());
    // The metadata's digest is another file's.
    mock.metadata(
        VERSION,
        &[(&asset_name(), gz.len() as u64, Some(&digest(b"other")))],
    );
    mock.route(
        &asset_path(VERSION, &asset_name()),
        seaquel_http::release_asset::testing::Route::ok(gz),
    );
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "DIGEST_MISMATCH", "{e}");
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    // Task 10's O3: the version folder the install made is gone again.
    assert!(!dir.join(VERSION).exists());

    // No release for this version.
    let (_tmp, dir) = layout();
    let core = core_with(&dir, "2026.10.99", Some(&mock));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "RELEASE_NOT_FOUND", "{e}");
    assert_eq!(mock.hits(&release_path("2026.10.99")), 1);

    // A version that isn't one plain folder name never reaches the disk
    // or the network.
    let (_tmp, dir) = layout();
    let core = core_with(&dir, "../x", Some(&mock));
    let before = mock.requests().len();
    assert_eq!(install(&core).await.unwrap_err().code, "INVALID_ARGUMENT");
    assert_eq!(mock.requests().len(), before);
}

#[cfg(unix)]
#[tokio::test]
async fn status_tells_missing_outdated_unsafe_and_installed_apart() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, None);
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );

    // Another version's helper: outdated.
    let old = dir.join("2026.9.1");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join(DuckdbHelper::file_name()), b"old").unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Outdated
    );
    // An empty version folder doesn't count.
    std::fs::remove_file(old.join(DuckdbHelper::file_name())).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );

    // This version's file, in a folder others can write: unsafe.
    let here = dir.join(VERSION);
    std::fs::create_dir_all(&here).unwrap();
    let file = here.join(DuckdbHelper::file_name());
    std::fs::write(&file, b"helper").unwrap();
    for d in [
        dir.parent().unwrap().parent().unwrap(),
        dir.parent().unwrap(),
        &dir,
        &here,
    ] {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { path: file.clone() }
    );
    std::fs::set_permissions(&here, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Unsafe
    );
    assert!(!format!("{:?}", DuckdbHelperStatus::Installed { path: file }).contains("app.seaquel"));
}

/// An install fixes folders this user owns that others can write (a
/// `bin` made 0775 by a umask of 002 refuses every start), and refuses a
/// symlinked one.
#[cfg(unix)]
#[tokio::test]
async fn an_install_fixes_loose_folders_and_refuses_links() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let mock = MockReleases::start().await;
    mock.publish(VERSION, &asset_name(), gzip(&fake_helper()));

    let (_tmp, dir) = layout();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(
        dir.parent().unwrap(),
        std::fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    let core = core_with(&dir, VERSION, Some(&mock));
    install(&core).await.unwrap();
    assert!(core.duckdb_helper().unwrap().check().is_ok());

    let (tmp, dir) = layout();
    let elsewhere = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    symlink(&elsewhere, &dir).unwrap();
    let core = core_with(&dir, VERSION, Some(&mock));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "UNSAFE_FOLDER", "{e}");
    assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    assert!(!e.message.contains(&*tmp.path().to_string_lossy()));
}

#[tokio::test]
async fn a_copied_file_installs_against_its_hash() {
    let (tmp, dir) = layout();
    let core = core_with(&dir, VERSION, None);
    let gz = gzip(&fake_helper());
    let file = tmp.path().join("seaquel-duckdb.gz");
    std::fs::write(&file, &gz).unwrap();
    let wrong = "0".repeat(64);
    let e = core
        .duckdb_helper_install_from_file(&file, Some(&wrong))
        .await
        .unwrap_err();
    assert_eq!(e.code, "DIGEST_MISMATCH");
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );

    let hash = &digest(&gz)["sha256:".len()..];
    let done = core
        .duckdb_helper_install_from_file(&file, Some(hash))
        .await
        .unwrap();
    assert!(done.downloaded);
    assert_eq!(std::fs::read(&done.path).unwrap(), fake_helper());
    assert!(matches!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

/// After an install, version folders older than the two newest go; the
/// running version stays whatever its age, and names that aren't versions
/// are left alone.
#[tokio::test]
async fn an_install_prunes_old_versions() {
    let (tmp, dir) = layout();
    let running = "2026.1.5";
    for v in ["2025.12.0", "2026.3.0", "2026.9.0", "2026.10.0", "notes"] {
        let d = dir.join(v);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(DuckdbHelper::file_name()), b"x").unwrap();
    }
    let core = core_with(&dir, running, None);
    let gz = gzip(&fake_helper());
    let file = tmp.path().join("seaquel-duckdb.gz");
    std::fs::write(&file, &gz).unwrap();
    let done = core
        .duckdb_helper_install_from_file(&file, Some(&digest(&gz)["sha256:".len()..]))
        .await
        .unwrap();
    assert_eq!(done.pruned, 2);
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    // 2026.10 is newer than 2026.9 (numbers, not text).
    assert_eq!(left, ["2026.1.5", "2026.10.0", "2026.9.0", "notes"]);
}

fn built_helper() -> Option<PathBuf> {
    match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            None
        }
    }
}

/// The real helper, gzipped and served, installed by Core and then started
/// by a connect: the install's layout and permissions pass the start's
/// check, and the helper answers.
#[tokio::test]
async fn the_installed_helper_runs() {
    let Some(bin) = built_helper() else { return };
    let out = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap();
    let version = String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("seaquel-duckdb ")
        .unwrap()
        .to_string();
    let mock = MockReleases::start().await;
    mock.publish(&version, &asset_name(), gzip(&std::fs::read(&bin).unwrap()));
    let (_tmp, dir) = layout();
    let core = core_with(&dir, &version, Some(&mock));
    let done = install(&core).await.unwrap();
    assert!(done.downloaded);

    let data = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(seaquel_core::WorkspaceSpec::new(data.path()))
        .await
        .unwrap();
    let form: seaquel_core::ConnectionForm = serde_json::from_value(
        serde_json::json!({"name": "installed", "type": "duckdb", "databaseName": ":memory:"}),
    )
    .unwrap();
    let req = seaquel_core::ConnectRequest::form(form)
        .with_secrets(seaquel_core::SuppliedSecrets::none())
        .with_create_if_missing(true);
    let id = tokio::time::timeout(LIMIT, ws.connect(&core, req))
        .await
        .unwrap()
        .expect("connect through the installed helper");
    let r = ws
        .query(&core, &id, "SELECT 40 + 2 AS answer", vec![])
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&r.rows).unwrap(),
        serde_json::json!([[42]])
    );
    ws.disconnect(&core, &id).await.unwrap();
}

/// The interfaces run the install on a task of its own (the TUI's
/// `Effect::InstallDuckdb`), so its future must be `Send`.
#[allow(dead_code)]
fn the_install_futures_are_send(core: &Core, file: &Path) {
    fn send<T: Send>(_: T) {}
    send(core.duckdb_helper_install(&mut |_| {}));
    send(core.duckdb_helper_asset());
    send(core.duckdb_helper_install_from_file(file, None));
}

/// An intact install in a folder that has gone loose (a umask,
/// another tool) is tightened and kept, offline: no download.
#[cfg(unix)]
#[tokio::test]
async fn an_intact_install_in_a_loose_folder_is_fixed_offline() {
    use std::os::unix::fs::PermissionsExt;
    let mock = MockReleases::start().await;
    mock.publish(VERSION, &asset_name(), gzip(&fake_helper()));
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    install(&core).await.unwrap();
    let bin = dir.parent().unwrap();
    std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o775)).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Unsafe
    );

    // The network is gone: the release server stops.
    let source = mock.source();
    drop(mock);
    let offline = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(DuckdbHelper {
            dir: dir.clone(),
            version: VERSION.to_string(),
        })
        .duckdb_helper_releases(source)
        .build();
    let done = install(&offline).await.unwrap();
    assert!(!done.downloaded);
    assert_eq!(mode(bin), 0o700);
    assert!(matches!(
        offline.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

/// An intact install whose file lost its owner's execute
/// bit (a restore, a copy tool) reads `Unsafe`, and the install doesn't
/// keep it: it downloads over it, and the file is 0700 again.
#[cfg(unix)]
#[tokio::test]
async fn a_helper_its_owner_cant_execute_is_unsafe_and_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let mock = MockReleases::start().await;
    let name = asset_name();
    mock.publish(VERSION, &name, gzip(&fake_helper()));
    let (_tmp, dir) = layout();
    let core = core_with(&dir, VERSION, Some(&mock));
    let first = install(&core).await.unwrap();
    std::fs::set_permissions(&first.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Unsafe
    );
    let e = core.duckdb_helper().unwrap().check().unwrap_err();
    assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");

    let repaired = install(&core).await.unwrap();
    assert!(repaired.downloaded, "the install kept the file");
    assert_eq!(mock.hits(&asset_path(VERSION, &name)), 2);
    assert_eq!(mode(&repaired.path), 0o700);
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed {
            path: repaired.path.clone()
        }
    );

    // The same from a copied file.
    let file = _tmp.path().join("copy.gz");
    let gz = gzip(&fake_helper());
    std::fs::write(&file, &gz).unwrap();
    std::fs::set_permissions(&repaired.path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Unsafe
    );
    let copied = tokio::time::timeout(
        LIMIT,
        core.duckdb_helper_install_from_file(&file, Some(&hex(&gz))),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(mode(&copied.path), 0o700);
    assert!(matches!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

// --- The pinned asset.

/// [`core_with`] with the asset's size and SHA-256 built in, as the app's
/// release build has them.
fn pinned_core(dir: &Path, mock: Option<&MockReleases>, size: u64, sha256: &str) -> Core {
    let mut b = seaquel_core::with_plugins(|id| id != "duckdb")
        .duckdb_helper(DuckdbHelper {
            dir: dir.to_path_buf(),
            version: VERSION.to_string(),
        })
        .duckdb_helper_pinned(size, sha256)
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor));
    if let Some(m) = mock {
        b = b.duckdb_helper_releases(m.source());
    }
    b.build()
}

fn hex(bytes: &[u8]) -> String {
    digest(bytes)["sha256:".len()..].to_string()
}

/// With a pin, the asset is answered from it: no request at all, so the
/// dialog knows the size offline and GitHub's API limit never applies.
#[tokio::test]
async fn a_pinned_asset_is_answered_without_a_request() {
    let mock = MockReleases::start().await;
    let (_tmp, dir) = layout();
    let core = pinned_core(&dir, Some(&mock), 12_345, &"ab".repeat(32));
    let pin = core.duckdb_helper_pin().expect("the pin");
    assert_eq!(pin.size, 12_345);
    assert_eq!(pin.sha256, "ab".repeat(32));

    let asset = core.duckdb_helper_asset().await.unwrap();
    assert_eq!(asset.name, asset_name());
    assert_eq!(asset.size, 12_345);
    assert!(mock.requests().is_empty(), "{:?}", mock.requests());
}

/// The pinned install fetches only the download path, never the release
/// metadata (the API), and installs what it got.
#[tokio::test]
async fn a_pinned_install_fetches_only_the_download() {
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper());
    let name = asset_name();
    // No metadata route: a metadata request would be a 404.
    mock.route(
        &asset_path(VERSION, &name),
        seaquel_http::release_asset::testing::Route::ok(gz.clone()),
    );
    let (_tmp, dir) = layout();
    let core = pinned_core(&dir, Some(&mock), gz.len() as u64, &hex(&gz));
    let mut seen: Vec<DuckdbHelperProgress> = Vec::new();
    let done = tokio::time::timeout(LIMIT, core.duckdb_helper_install(&mut |p| seen.push(p)))
        .await
        .unwrap()
        .unwrap();
    assert!(done.downloaded);
    assert_eq!(std::fs::read(&done.path).unwrap(), fake_helper());
    assert_eq!(seen.last().unwrap().bytes, gz.len() as u64);
    let paths: Vec<String> = mock.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, [asset_path(VERSION, &name)]);
    assert_eq!(mock.hits(&release_path(VERSION)), 0);
    assert!(matches!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

/// Under the pin the download is the only request, and a
/// release without this platform's asset answers it 404. That is
/// `ASSET_NOT_FOUND`, as the metadata path says it, so the dialog words it
/// as unpublished and offers the file. The version folder the install made
/// is removed again (O3).
#[tokio::test]
async fn a_pinned_install_of_a_missing_asset_is_asset_not_found() {
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper());
    let (_tmp, dir) = layout();
    let core = pinned_core(&dir, Some(&mock), gz.len() as u64, &hex(&gz));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "ASSET_NOT_FOUND", "{e}");
    assert!(e.message.contains(&asset_name()), "{e}");
    assert_eq!(mock.hits(&release_path(VERSION)), 0);
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    assert!(!dir.join(VERSION).exists(), "the version folder was left");
}

/// A served file whose digest isn't the pinned one is refused, even when
/// the release's own metadata would vouch for it, and nothing is left.
#[tokio::test]
async fn a_pinned_install_refuses_another_file() {
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper());
    // The release (metadata and asset) agrees with itself; the pin doesn't.
    mock.publish(VERSION, &asset_name(), gz.clone());
    let (_tmp, dir) = layout();
    let core = pinned_core(&dir, Some(&mock), gz.len() as u64, &hex(b"another file"));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "DIGEST_MISMATCH", "{e}");
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    assert!(!dir.join(VERSION).exists());
    assert_eq!(mock.hits(&release_path(VERSION)), 0);

    // A file of another size than the pinned one: refused by its length.
    let (_tmp, dir) = layout();
    let core = pinned_core(&dir, Some(&mock), gz.len() as u64 + 1, &hex(&gz));
    let e = install(&core).await.unwrap_err();
    assert_eq!(e.code, "SIZE_MISMATCH", "{e}");
    // The pinned size isn't the release's: the message doesn't say it is.
    assert!(!e.message.contains("release says"), "{e}");
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
}

/// "Install from a file…" passes no hash: the file is checked against the
/// pin. Without a pin, no hash is refused before anything is read.
#[tokio::test]
async fn a_copied_file_with_no_hash_is_checked_against_the_pin() {
    let (tmp, dir) = layout();
    let gz = gzip(&fake_helper());
    let file = tmp.path().join("seaquel-duckdb.gz");
    std::fs::write(&file, &gz).unwrap();
    // The same length and still gzip, but not the same bytes (a byte of
    // the trailer's CRC changed): only the digest tells it apart.
    let other = tmp.path().join("other.gz");
    let mut changed = gz.clone();
    let at = changed.len() - 6;
    changed[at] ^= 0xff;
    std::fs::write(&other, changed).unwrap();

    // No pin and no hash.
    let unpinned = core_with(&dir, VERSION, None);
    let e = unpinned
        .duckdb_helper_install_from_file(&file, None)
        .await
        .unwrap_err();
    assert_eq!(e.code, "INVALID_ARGUMENT", "{e}");
    assert!(!dir.exists(), "nothing is made without a hash to check");

    let core = pinned_core(&dir, None, gz.len() as u64, &hex(&gz));
    let e = core
        .duckdb_helper_install_from_file(&other, None)
        .await
        .unwrap_err();
    assert_eq!(e.code, "DIGEST_MISMATCH", "{e}");
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );

    let done = core
        .duckdb_helper_install_from_file(&file, None)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&done.path).unwrap(), fake_helper());
    assert!(matches!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Installed { .. }
    ));
}

/// Without a pin there is none to read.
#[test]
fn no_pin_by_default() {
    let (_tmp, dir) = layout();
    assert!(core_with(&dir, VERSION, None).duckdb_helper_pin().is_none());
}

/// A pin that can't be one is a programming error (the app's build script
/// refuses such values before they are compiled in).
#[test]
#[should_panic(expected = "pinned")]
fn a_malformed_pin_is_refused() {
    let _ = seaquel_core::with_plugins(|id| id != "duckdb").duckdb_helper_pinned(10, "not hex");
}

#[test]
#[should_panic(expected = "pinned")]
fn a_pinned_size_of_zero_is_refused() {
    let _ =
        seaquel_core::with_plugins(|id| id != "duckdb").duckdb_helper_pinned(0, &"a".repeat(64));
}

/// A pinned Core keeps an intact install of the pinned asset that
/// came the metadata way (a terminal binary's, or an earlier unpinned run):
/// nothing is requested.
#[tokio::test]
async fn a_pinned_core_keeps_an_intact_install_of_the_same_asset() {
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper());
    mock.publish(VERSION, &asset_name(), gz.clone());
    let (_tmp, dir) = layout();
    install(&core_with(&dir, VERSION, Some(&mock)))
        .await
        .unwrap();
    let before = mock.requests().len();

    let pinned = pinned_core(&dir, Some(&mock), gz.len() as u64, &hex(&gz));
    let done = install(&pinned).await.unwrap();
    assert!(!done.downloaded);
    assert_eq!(mock.requests().len(), before, "{:?}", mock.requests());
}

/// Review 1 (decision): an intact install whose record names another asset
/// digest than the pin is replaced with the pinned asset.
#[tokio::test]
async fn a_pinned_core_replaces_an_install_of_another_asset() {
    let mock = MockReleases::start().await;
    let first = gzip(&fake_helper());
    mock.publish(VERSION, &asset_name(), first);
    let (_tmp, dir) = layout();
    install(&core_with(&dir, VERSION, Some(&mock)))
        .await
        .unwrap();

    let mut bytes = fake_helper();
    bytes.extend_from_slice(b"another build");
    let second = gzip(&bytes);
    mock.route(
        &asset_path(VERSION, &asset_name()),
        seaquel_http::release_asset::testing::Route::ok(second.clone()),
    );
    let pinned = pinned_core(&dir, Some(&mock), second.len() as u64, &hex(&second));
    let done = install(&pinned).await.unwrap();
    assert!(done.downloaded);
    assert_eq!(std::fs::read(&done.path).unwrap(), bytes);
}

/// A FIFO picked as the file (no writer, so opening it
/// would block forever) is `WRONG_FILE` at once under the pin, with nothing
/// made and no thread left waiting.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_fifo_under_the_pin_is_the_wrong_file_at_once() {
    let (tmp, dir) = layout();
    let gz = gzip(&fake_helper());
    let core = pinned_core(&dir, None, gz.len() as u64, &hex(&gz));
    let fifo = tmp.path().join("seaquel-duckdb.gz");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let e = tokio::time::timeout(
        Duration::from_secs(5),
        core.duckdb_helper_install_from_file(&fifo, None),
    )
    .await
    .expect("refused at once, not blocked on the FIFO")
    .unwrap_err();
    assert_eq!(e.code, "WRONG_FILE", "{e}");
    assert!(!e.message.contains(&*tmp.path().to_string_lossy()), "{e}");
    assert!(!dir.join(VERSION).exists(), "nothing is made");
}

/// Under the pin, a copied file of another length, or one that
/// isn't gzip, is `WRONG_FILE`, naming the asset to pick; nothing is made.
#[tokio::test]
async fn a_wrong_file_under_the_pin_names_the_asset_to_pick() {
    let (tmp, dir) = layout();
    let gz = gzip(&fake_helper());
    let core = pinned_core(&dir, None, gz.len() as u64, &hex(&gz));
    let want = format!("{} from the v{VERSION} release", asset_name());

    // Another length (an older release's asset, say).
    let longer = tmp.path().join("longer.gz");
    let mut l = gz.clone();
    l.push(0);
    std::fs::write(&longer, l).unwrap();
    // The right length, but not gzip (the helper unpacked, then cut).
    let plain = tmp.path().join("plain");
    std::fs::write(&plain, &fake_helper()[..gz.len()]).unwrap();

    for file in [&longer, &plain] {
        let e = core
            .duckdb_helper_install_from_file(file, None)
            .await
            .unwrap_err();
        assert_eq!(e.code, "WRONG_FILE", "{e}");
        assert!(e.message.contains(&want), "{e}");
        assert!(!e.message.contains(&*tmp.path().to_string_lossy()), "{e}");
    }
    assert_eq!(
        core.duckdb_helper_status().unwrap(),
        DuckdbHelperStatus::Missing
    );
    assert!(
        !dir.join(VERSION).exists(),
        "nothing is made for a wrong file"
    );
}
