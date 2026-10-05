#![allow(clippy::disallowed_methods, clippy::disallowed_types)] // the interrupted install runs on a task of its own; stale files are aged by `SystemTime`
//! `release_asset` against a release server on 127.0.0.1:
//! the metadata, the download's size, digest and
//! gzip checks, the install's folders and atomic rename, progress,
//! interruption and concurrency. Nothing here reaches a real host: every
//! source is the mock's loopback address. Installs go into temp folders.

use std::path::{Path, PathBuf};
use std::time::Duration;

use seaquel_http::release_asset::testing::{
    asset_path, digest, gzip, release_path, MockReleases, Route,
};
use seaquel_http::release_asset::{
    install_file, install_file_cancellable, installed, prepare_target, target_triple, Asset,
    Fetcher, InstallErrorKind, InstallTarget, Progress, ReleaseSource, STALE_PART_AGE,
};

const VERSION: &str = "2026.10.1";
const NAME: &str = "seaquel-duckdb-test.gz";
const LIMIT: Duration = Duration::from_secs(30);

/// A helper stand-in: 300 KiB that compress, so progress has steps.
fn payload() -> Vec<u8> {
    let mut v = b"#!/bin/sh\nexit 0\n".to_vec();
    let mut x: u32 = 7;
    while v.len() < 300 * 1024 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        v.extend_from_slice(format!("line {} {}\n", x % 97, x >> 20).as_bytes());
    }
    v
}

fn target(root: &Path) -> InstallTarget {
    InstallTarget {
        root: root.join("app.seaquel.test"),
        folders: vec!["bin".into(), "duckdb".into(), VERSION.into()],
        file_name: "seaquel-duckdb".into(),
    }
}

/// Every name in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
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

#[cfg(unix)]
fn chmod(path: &Path, m: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(m)).unwrap();
}

async fn fetch_and_install(
    mock: &MockReleases,
    t: &InstallTarget,
) -> Result<seaquel_http::release_asset::Installed, seaquel_http::release_asset::InstallError> {
    let fetcher = Fetcher::new(mock.source(), VERSION);
    let asset = tokio::time::timeout(LIMIT, fetcher.asset(VERSION, NAME))
        .await
        .expect("metadata in time")?;
    tokio::time::timeout(LIMIT, fetcher.install(&asset, t, &mut |_| {}))
        .await
        .expect("install in time")
}

/// Nothing at the target, and no file but `allowed` in its folder.
fn nothing_left(t: &InstallTarget, allowed: &[&str]) {
    assert!(
        std::fs::symlink_metadata(t.path()).is_err(),
        "the target exists"
    );
    let left: Vec<String> = names(&t.dir())
        .into_iter()
        .filter(|n| !allowed.contains(&n.as_str()))
        .collect();
    assert!(left.is_empty(), "left in the folder: {left:?}");
}

#[tokio::test]
async fn a_good_asset_installs_privately() {
    let mock = MockReleases::start().await;
    let bytes = payload();
    let gz = gzip(&bytes);
    mock.publish(VERSION, NAME, gz.clone());
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());

    let fetcher = Fetcher::new(mock.source(), VERSION);
    let asset = fetcher.asset(VERSION, NAME).await.unwrap();
    assert_eq!(
        asset,
        Asset {
            name: NAME.into(),
            version: VERSION.into(),
            size: gz.len() as u64,
            sha256: digest(&gz)["sha256:".len()..].to_string(),
        }
    );
    let mut seen: Vec<Progress> = Vec::new();
    let done = fetcher
        .install(&asset, &t, &mut |p| seen.push(p))
        .await
        .unwrap();
    assert_eq!(done.path, t.path());
    assert_eq!(done.size, bytes.len() as u64);
    assert_eq!(std::fs::read(t.path()).unwrap(), bytes);
    assert_eq!(installed(&t), Some(done.clone()));

    // Progress: compressed bytes, never backwards, ending at the total.
    assert!(seen.len() >= 2, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0].bytes <= w[1].bytes));
    assert!(seen.iter().all(|p| p.total == gz.len() as u64));
    assert_eq!(seen.last().unwrap().bytes, gz.len() as u64);
    assert!(seen
        .windows(2)
        .all(|w| w[1].bytes - w[0].bytes <= 128 * 1024));

    #[cfg(unix)]
    {
        assert_eq!(mode(&t.path()), 0o700);
        for f in ["bin", "bin/duckdb", &format!("bin/duckdb/{VERSION}")] {
            assert_eq!(mode(&t.root.join(f)), 0o700, "{f}");
        }
        assert_eq!(mode(&t.root) & 0o022, 0);
    }
    // The file and the install record, nothing else.
    assert_eq!(
        names(&t.dir()),
        ["seaquel-duckdb", "seaquel-duckdb.installed"]
    );

    // Requests: the metadata, then the asset; identified by the version only.
    let reqs = mock.requests();
    let paths: Vec<&str> = reqs.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [release_path(VERSION), asset_path(VERSION, NAME)]);
    for r in &reqs {
        assert_eq!(r.method, "GET");
        assert_eq!(r.header("user-agent"), Some(&*format!("Seaquel/{VERSION}")));
        for (name, value) in &r.headers {
            assert!(
                ["host", "user-agent", "accept"].contains(&name.as_str()),
                "unexpected header {name}"
            );
            assert!(!value.contains('@'), "{name}: {value}");
        }
    }
}

/// An asset as the metadata lists it: name, size, digest.
type Listed<'a> = (&'a str, u64, Option<&'a str>);

#[tokio::test]
async fn the_metadata_is_checked_before_anything_is_written() {
    let gz = gzip(&payload());
    let size = gz.len() as u64;
    let good = digest(&gz);
    let not_hex = format!("sha256:{}", "z".repeat(64));
    let cases: Vec<(&str, Vec<Listed>, InstallErrorKind)> = vec![
        (
            "no digest",
            vec![(NAME, size, None)],
            InstallErrorKind::DigestMissing,
        ),
        (
            "md5 digest",
            vec![(NAME, size, Some("md5:0123"))],
            InstallErrorKind::DigestMissing,
        ),
        (
            "short digest",
            vec![(NAME, size, Some("sha256:abc"))],
            InstallErrorKind::DigestInvalid,
        ),
        (
            "not hex",
            vec![(NAME, size, Some(&*not_hex))],
            InstallErrorKind::DigestInvalid,
        ),
        (
            "zero size",
            vec![(NAME, 0, Some(&*good))],
            InstallErrorKind::SizeInvalid,
        ),
        (
            "too large",
            vec![(NAME, 65 * 1024 * 1024, Some(&*good))],
            InstallErrorKind::SizeInvalid,
        ),
        (
            "another asset only",
            vec![("seaquel-duckdb-other.gz", size, Some(&*good))],
            InstallErrorKind::AssetNotFound,
        ),
    ];
    for (what, assets, kind) in cases {
        let mock = MockReleases::start().await;
        mock.metadata(VERSION, &assets);
        mock.route(&asset_path(VERSION, NAME), Route::ok(gz.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let t = target(tmp.path());
        let err = fetch_and_install(&mock, &t).await.unwrap_err();
        assert_eq!(err.kind, kind, "{what}: {err}");
        assert_eq!(mock.hits(&asset_path(VERSION, NAME)), 0, "{what}");
        nothing_left(&t, &[]);
    }

    // No such release.
    let mock = MockReleases::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let err = fetch_and_install(&mock, &target(tmp.path()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::ReleaseNotFound, "{err}");
    assert!(err.message.contains(VERSION), "{err}");

    // Metadata that isn't JSON, a rate limit.
    mock.route(&release_path(VERSION), Route::ok(b"<html>".to_vec()));
    let err = fetch_and_install(&mock, &target(tmp.path()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::MetadataInvalid, "{err}");
    mock.route(&release_path(VERSION), Route::status(403));
    let err = fetch_and_install(&mock, &target(tmp.path()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::Http, "{err}");
    assert!(err.message.contains("403"), "{err}");
}

/// A route serving `body` while the metadata says `size` and `digest`.
async fn refused(
    body_route: Route,
    size: u64,
    digest_of: &[u8],
) -> (
    seaquel_http::release_asset::InstallError,
    InstallTarget,
    tempfile::TempDir,
) {
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, size, Some(&digest(digest_of)))]);
    mock.route(&asset_path(VERSION, NAME), body_route);
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let err = fetch_and_install(&mock, &t).await.unwrap_err();
    // Checked before the rename: nothing at the target, no temp file.
    nothing_left(&t, &[]);
    (err, t, tmp)
}

#[tokio::test]
async fn a_download_that_does_not_match_is_refused() {
    let gz = gzip(&payload());
    let n = gz.len() as u64;

    // The digest of other bytes.
    let (err, ..) = refused(Route::ok(gz.clone()), n, b"other").await;
    assert_eq!(err.kind, InstallErrorKind::DigestMismatch, "{err}");
    // One byte flipped: the digest catches it before gzip's CRC is read.
    let mut flipped = gz.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x55;
    let (err, ..) = refused(Route::ok(flipped), n, &gz).await;
    assert_eq!(err.kind, InstallErrorKind::DigestMismatch, "{err}");

    // Shorter than the metadata says (its own length, and chunked).
    let short = gz[..gz.len() - 100].to_vec();
    let (err, ..) = refused(Route::ok(short.clone()), n, &gz).await;
    assert_eq!(err.kind, InstallErrorKind::SizeMismatch, "{err}");
    let (err, ..) = refused(Route::chunked(short), n, &gz).await;
    assert_eq!(err.kind, InstallErrorKind::SizeMismatch, "{err}");

    // Longer: announced, and sent chunked past the size.
    let mut long = gz.clone();
    long.extend_from_slice(&[0u8; 4096]);
    let (err, ..) = refused(Route::ok(long.clone()), n, &gz).await;
    assert_eq!(err.kind, InstallErrorKind::SizeMismatch, "{err}");
    let (err, ..) = refused(Route::chunked(long), n, &gz).await;
    assert_eq!(err.kind, InstallErrorKind::SizeMismatch, "{err}");

    // The asset's own URL missing: the asset isn't there,
    // as the metadata says of an asset it doesn't list, not an unusable
    // answer. Under a pin this is the only request, so it is the only way
    // to learn that.
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n, Some(&digest(&gz)))]);
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let err = fetch_and_install(&mock, &t).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::AssetNotFound, "{err}");
    assert_eq!(err.code(), "ASSET_NOT_FOUND");
    assert!(err.message.contains(NAME), "{err}");
    assert!(err.message.contains(VERSION), "{err}");
    nothing_left(&t, &[]);

    // Any other refusal of the download stays `HTTP_ERROR`.
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n, Some(&digest(&gz)))]);
    mock.route(&asset_path(VERSION, NAME), Route::status(503));
    let err = fetch_and_install(&mock, &t).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::Http, "{err}");
    assert!(err.message.contains("503"), "{err}");
    nothing_left(&t, &[]);
}

/// Without the metadata (a pinned install asks only for the download), a
/// 404 is `ASSET_NOT_FOUND` too.
#[tokio::test]
async fn a_missing_download_without_metadata_is_asset_not_found() {
    let gz = gzip(&payload());
    let mock = MockReleases::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let asset = Asset {
        name: NAME.into(),
        version: VERSION.into(),
        size: gz.len() as u64,
        sha256: digest(&gz)["sha256:".len()..].to_string(),
    };
    let fetcher = Fetcher::new(mock.source(), VERSION);
    let err = tokio::time::timeout(LIMIT, fetcher.install(&asset, &t, &mut |_| {}))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::AssetNotFound, "{err}");
    assert!(mock.requests().iter().all(|r| !r.path.starts_with("/api")));
    nothing_left(&t, &[]);
}

#[tokio::test]
async fn gzip_is_checked_to_its_end() {
    let gz = gzip(&payload());
    // Truncated: the release's own digest and size match the cut file,
    // so only gzip can tell.
    for cut in [gz.len() - 4, gz.len() / 2, 9] {
        let part = gz[..cut].to_vec();
        let (err, ..) = refused(Route::ok(part.clone()), part.len() as u64, &part).await;
        assert_eq!(err.kind, InstallErrorKind::NotGzip, "cut at {cut}: {err}");
    }
    // Not gzip at all.
    let plain = payload();
    let (err, ..) = refused(Route::ok(plain.clone()), plain.len() as u64, &plain).await;
    assert_eq!(err.kind, InstallErrorKind::NotGzip, "{err}");
    // A wrong CRC in the trailer.
    let mut crc = gz.clone();
    let at = crc.len() - 8;
    crc[at] ^= 0xff;
    let (err, ..) = refused(Route::ok(crc.clone()), crc.len() as u64, &crc).await;
    assert_eq!(err.kind, InstallErrorKind::NotGzip, "{err}");
}

#[tokio::test]
async fn decompression_is_capped() {
    let bomb = gzip(&vec![0u8; 3 * 1024 * 1024]);
    let mock = MockReleases::start().await;
    mock.publish(VERSION, NAME, bomb);
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let mut fetcher = Fetcher::new(mock.source(), VERSION);
    fetcher.max_installed_bytes = 1024 * 1024;
    let asset = fetcher.asset(VERSION, NAME).await.unwrap();
    let err = fetcher.install(&asset, &t, &mut |_| {}).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::TooLarge, "{err}");
    nothing_left(&t, &[]);
}

#[tokio::test]
async fn a_connection_lost_half_way_leaves_nothing() {
    let gz = gzip(&payload());
    let n = gz.len();
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n as u64, Some(&digest(&gz)))]);
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Body {
            status: 200,
            length: Some(n as u64),
            body: gz,
            stall_after: None,
            close_after: Some(n / 2),
            piece: 4096,
            gap: Duration::ZERO,
        },
    );
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let err = fetch_and_install(&mock, &t).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::Network, "{err}");
    nothing_left(&t, &[]);
}

/// Dropping the install half-way (Esc in the TUI, a killed task): while it
/// runs, the partial file is private and not executable and the target
/// isn't there; once dropped, nothing is left.
#[tokio::test]
async fn an_install_dropped_half_way_leaves_nothing_executable() {
    let gz = gzip(&payload());
    let n = gz.len();
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n as u64, Some(&digest(&gz)))]);
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Body {
            status: 200,
            length: Some(n as u64),
            body: gz,
            stall_after: Some(n / 2),
            close_after: None,
            piece: 4096,
            gap: Duration::ZERO,
        },
    );
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let task = {
        let source = mock.source();
        let t = t.clone();
        tokio::spawn(async move {
            let fetcher = Fetcher::new(source, VERSION);
            let asset = fetcher.asset(VERSION, NAME).await?;
            fetcher.install(&asset, &t, &mut |_| {}).await
        })
    };
    // Wait for the partial file to hold some bytes.
    let dir = t.dir();
    let partial = tokio::time::timeout(LIMIT, async {
        loop {
            for name in names(&dir) {
                let p = dir.join(&name);
                if std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false) {
                    return p;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a partial file appears");
    assert_ne!(
        partial,
        t.path(),
        "the download went straight to the target"
    );
    #[cfg(unix)]
    assert_eq!(mode(&partial) & 0o177, 0, "the partial file is private");
    assert!(std::fs::symlink_metadata(t.path()).is_err());
    assert!(!task.is_finished());

    task.abort();
    let _ = task.await;
    nothing_left(&t, &[]);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_folders_are_refused() {
    use std::os::unix::fs::symlink;
    let gz = gzip(&payload());
    for (level, link) in [
        (0, "app.seaquel.test"),
        (1, "app.seaquel.test/bin"),
        (2, "app.seaquel.test/bin/duckdb"),
        (3, &*format!("app.seaquel.test/bin/duckdb/{VERSION}")),
    ] {
        let mock = MockReleases::start().await;
        mock.publish(VERSION, NAME, gz.clone());
        let tmp = tempfile::tempdir().unwrap();
        let t = target(tmp.path());
        // The real folders up to the link's parent, the link pointing at a
        // folder elsewhere (as another user's would).
        let link = tmp.path().join(link);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        chmod(&elsewhere, 0o700);
        symlink(&elsewhere, &link).unwrap();
        for d in [&t.root, &t.root.join("bin"), &t.root.join("bin/duckdb")] {
            if std::fs::symlink_metadata(d)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                chmod(d, 0o700);
            }
        }
        let err = fetch_and_install(&mock, &t).await.unwrap_err();
        assert_eq!(
            err.kind,
            InstallErrorKind::UnsafeFolder,
            "level {level}: {err}"
        );
        assert!(
            names(&elsewhere).is_empty(),
            "level {level}: wrote through the link"
        );
        assert_eq!(mock.hits(&asset_path(VERSION, NAME)), 0, "level {level}");
        assert!(!err.message.contains(&*tmp.path().to_string_lossy()));
    }
    // The file's name a symlink: replaced by the file, not written through.
    let mock = MockReleases::start().await;
    mock.publish(VERSION, NAME, gz);
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir_all(t.dir()).unwrap();
    let victim = tmp.path().join("victim");
    std::fs::write(&victim, b"keep").unwrap();
    symlink(&victim, t.path()).unwrap();
    fetch_and_install(&mock, &t).await.unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    assert!(std::fs::symlink_metadata(t.path()).unwrap().is_file());
}

/// Folders this user owns but others can write (a umask of 002, or worse)
/// are tightened: `bin` and below to 0700, the app's folder to no group or
/// world write. Another user's folder is refused (the unit tests cover the
/// owner check; a test can't make a folder another user owns).
#[cfg(unix)]
#[tokio::test]
async fn writable_folders_of_this_user_are_tightened() {
    let mock = MockReleases::start().await;
    mock.publish(VERSION, NAME, gzip(&payload()));
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir_all(t.dir()).unwrap();
    chmod(&t.root, 0o777);
    for f in ["bin", "bin/duckdb", &format!("bin/duckdb/{VERSION}")] {
        chmod(&t.root.join(f), 0o777);
    }
    fetch_and_install(&mock, &t).await.unwrap();
    assert_eq!(mode(&t.root), 0o755);
    for f in ["bin", "bin/duckdb", &format!("bin/duckdb/{VERSION}")] {
        assert_eq!(mode(&t.root.join(f)), 0o700, "{f}");
    }
}

#[tokio::test]
async fn two_installs_at_once_both_succeed() {
    let mock = MockReleases::start().await;
    let bytes = payload();
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Body {
            status: 200,
            length: None,
            body: gzip(&bytes),
            stall_after: None,
            close_after: None,
            piece: 2048,
            gap: Duration::from_millis(1),
        },
    );
    let gz = gzip(&bytes);
    mock.metadata(VERSION, &[(NAME, gz.len() as u64, Some(&digest(&gz)))]);
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let (a, b) = tokio::join!(fetch_and_install(&mock, &t), fetch_and_install(&mock, &t));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert_eq!(mock.hits(&asset_path(VERSION, NAME)), 2);
    assert_eq!(std::fs::read(t.path()).unwrap(), bytes);
    assert_eq!(
        names(&t.dir()),
        ["seaquel-duckdb", "seaquel-duckdb.installed"]
    );
    assert!(installed(&t).is_some());
}

#[tokio::test]
async fn installing_the_same_version_again_replaces_it() {
    let mock = MockReleases::start().await;
    let bytes = payload();
    mock.publish(VERSION, NAME, gzip(&bytes));
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let first = fetch_and_install(&mock, &t).await.unwrap();
    let second = fetch_and_install(&mock, &t).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(installed(&t), Some(second));
    assert_eq!(
        names(&t.dir()),
        ["seaquel-duckdb", "seaquel-duckdb.installed"]
    );

    // A changed file no longer counts as installed.
    std::fs::write(t.path(), b"tampered").unwrap();
    assert_eq!(installed(&t), None);
    // Nor a missing record.
    fetch_and_install(&mock, &t).await.unwrap();
    std::fs::remove_file(t.dir().join("seaquel-duckdb.installed")).unwrap();
    assert_eq!(installed(&t), None);
}

#[tokio::test]
async fn redirects_are_followed_to_https_or_loopback_only() {
    let mock = MockReleases::start().await;
    let bytes = payload();
    let gz = gzip(&bytes);
    mock.metadata(VERSION, &[(NAME, gz.len() as u64, Some(&digest(&gz)))]);
    // GitHub sends the asset from another host after a 302.
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Redirect {
            status: 302,
            location: format!("{}/blob/1", mock.url()),
        },
    );
    mock.route("/blob/1", Route::ok(gz));
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    fetch_and_install(&mock, &t).await.unwrap();
    assert_eq!(std::fs::read(t.path()).unwrap(), bytes);

    // Plain HTTP to anything but loopback: refused before connecting.
    for location in [
        "http://203.0.113.9/asset".to_string(),
        "ftp://127.0.0.1/asset".to_string(),
    ] {
        mock.route(
            &asset_path(VERSION, NAME),
            Route::Redirect {
                status: 302,
                location,
            },
        );
        let tmp = tempfile::tempdir().unwrap();
        let t = target(tmp.path());
        let err = fetch_and_install(&mock, &t).await.unwrap_err();
        assert_eq!(err.kind, InstallErrorKind::RedirectRefused, "{err}");
        assert!(!err.message.contains("203.0.113.9"), "{err}");
        nothing_left(&t, &[]);
    }

    // A loop ends.
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Redirect {
            status: 302,
            location: asset_path(VERSION, NAME),
        },
    );
    let tmp = tempfile::tempdir().unwrap();
    let err = fetch_and_install(&mock, &target(tmp.path()))
        .await
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::RedirectRefused, "{err}");
}

#[test]
fn sources_are_https_or_loopback() {
    assert!(ReleaseSource::new("https://example.com/api", "https://example.com/dl").is_ok());
    assert!(ReleaseSource::new("http://127.0.0.1:9/api", "http://[::1]:9/dl").is_ok());
    for (api, download) in [
        ("http://example.com/api", "https://example.com/dl"),
        ("https://example.com/api", "http://10.0.0.1/dl"),
        ("http://localhost.example/api", "https://example.com/dl"),
        ("file:///tmp/api", "https://example.com/dl"),
        ("not a url", "https://example.com/dl"),
    ] {
        let err = ReleaseSource::new(api, download).unwrap_err();
        assert_eq!(
            err.kind,
            InstallErrorKind::InvalidArgument,
            "{api} {download}"
        );
    }
    let debug = format!("{:?}", ReleaseSource::github());
    assert!(debug.contains("api.github.com"), "{debug}");
}

#[tokio::test]
async fn versions_and_names_are_single_safe_components() {
    let mock = MockReleases::start().await;
    let fetcher = Fetcher::new(mock.source(), VERSION);
    for (version, name) in [
        ("../x", NAME),
        ("1/2", NAME),
        (VERSION, "../seaquel-duckdb.gz"),
        (VERSION, "a?b"),
        ("", NAME),
    ] {
        let err = fetcher.asset(version, name).await.unwrap_err();
        assert_eq!(
            err.kind,
            InstallErrorKind::InvalidArgument,
            "{version} {name}"
        );
    }
    assert!(mock.requests().is_empty());
    let tmp = tempfile::tempdir().unwrap();
    let mut t = target(tmp.path());
    t.folders[2] = "..".into();
    let asset = Asset {
        name: NAME.into(),
        version: VERSION.into(),
        size: 1,
        sha256: "a".repeat(64),
    };
    let err = fetcher.install(&asset, &t, &mut |_| {}).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::InvalidArgument);
}

#[test]
fn a_file_copied_over_installs_against_the_given_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let bytes = payload();
    let gz_path = tmp.path().join("seaquel-duckdb.gz");
    let gz = gzip(&bytes);
    std::fs::write(&gz_path, &gz).unwrap();
    let hash = digest(&gz)["sha256:".len()..].to_uppercase();

    let t = target(&tmp.path().join("a"));
    let err = install_file(&gz_path, &"0".repeat(64), &t).unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::DigestMismatch);
    nothing_left(&t, &[]);
    let err = install_file(&gz_path, "nothex", &t).unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::DigestInvalid);
    let err = install_file(&tmp.path().join("missing.gz"), &hash, &t).unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::File);

    let done = install_file(&gz_path, &hash, &t).unwrap();
    assert_eq!(std::fs::read(&done.path).unwrap(), bytes);
    assert_eq!(installed(&t), Some(done));

    // A file someone gunzipped first installs as it is.
    let bare = tmp.path().join("seaquel-duckdb");
    std::fs::write(&bare, &bytes).unwrap();
    let t = target(&tmp.path().join("b"));
    let done = install_file(&bare, &digest(&bytes)["sha256:".len()..], &t).unwrap();
    assert_eq!(std::fs::read(&done.path).unwrap(), bytes);
    #[cfg(unix)]
    assert_eq!(mode(&done.path), 0o700);
}

/// A `.part` file `age` old in `dir`.
fn part_aged(dir: &Path, name: &str, age: Duration) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_modified(std::time::SystemTime::now() - age)
        .unwrap();
    path
}

/// Review I2 (decision): a `.part` file a killed install left (SIGKILL,
/// or a signal the process didn't wait out) is swept by the next install
/// into that folder once it is older than [`STALE_PART_AGE`]; a fresh
/// one may be a live concurrent install and is kept, as is anything not
/// named like a download.
#[tokio::test]
async fn stale_downloads_are_swept_and_fresh_ones_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockReleases::start().await;
    mock.publish(VERSION, NAME, gzip(&payload()));
    let t = target(tmp.path());
    let dir = prepare_target(&t).unwrap();
    let old = STALE_PART_AGE + Duration::from_secs(60);
    let stale = part_aged(&dir, ".seaquel-download-old.part", old);
    let fresh = part_aged(&dir, ".seaquel-download-live.part", Duration::from_secs(5));
    let other = part_aged(&dir, "notes.part", old);
    fetch_and_install(&mock, &t).await.unwrap();
    assert!(!stale.exists(), "a stale download is swept");
    assert!(fresh.exists(), "a fresh one may be a live install");
    assert!(other.exists(), "other names are left alone");

    // `install_file` (`--from`) and an offline repair sweep too.
    let stale = part_aged(&dir, ".seaquel-download-old2.part", old);
    prepare_target(&t).unwrap();
    assert!(!stale.exists());
}

/// Review I2: `install_file_cancellable` stops between reads once its flag
/// is set, and leaves nothing behind.
#[test]
fn a_cancelled_file_install_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let gz = gzip(&payload());
    let file = tmp.path().join("seaquel-duckdb.gz");
    std::fs::write(&file, &gz).unwrap();
    let t = target(&tmp.path().join("a"));
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let err =
        install_file_cancellable(&file, &digest(&gz)["sha256:".len()..], &t, &cancel).unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::Cancelled);
    assert_eq!(err.code(), "CANCELLED");
    nothing_left(&t, &[]);
}

/// A FIFO (or any file that isn't a regular file) given
/// as the copied asset is refused at once, without blocking on its open or
/// its reads, and nothing is left.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let fifo = tmp.path().join("seaquel-duckdb.gz");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let t = target(&tmp.path().join("a"));
    let (tx, rx) = std::sync::mpsc::channel();
    let (f, t2) = (fifo.clone(), t.clone());
    std::thread::spawn(move || {
        let _ = tx.send(install_file(&f, &"0".repeat(64), &t2));
    });
    let err = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("refused at once, not blocked on the FIFO")
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::InvalidArgument, "{err}");
    assert!(!format!("{err}").contains(&*tmp.path().to_string_lossy()));
    nothing_left(&t, &[]);
    // A directory isn't a file either.
    let err = install_file(tmp.path(), &"0".repeat(64), &t).unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::InvalidArgument, "{err}");
}

#[test]
fn triples_match_the_release_targets() {
    assert_eq!(
        target_triple("macos", "aarch64"),
        Some("aarch64-apple-darwin")
    );
    assert_eq!(
        target_triple("macos", "x86_64"),
        Some("x86_64-apple-darwin")
    );
    assert_eq!(
        target_triple("linux", "x86_64"),
        Some("x86_64-unknown-linux-gnu")
    );
    assert_eq!(
        target_triple("linux", "aarch64"),
        Some("aarch64-unknown-linux-gnu")
    );
    assert_eq!(
        target_triple("windows", "x86_64"),
        Some("x86_64-pc-windows-msvc")
    );
    assert_eq!(
        target_triple("windows", "aarch64"),
        Some("aarch64-pc-windows-msvc")
    );
    assert_eq!(target_triple("freebsd", "x86_64"), None);
}

#[test]
fn debug_names_no_path() {
    let t = target(Path::new("/home/someone/secret-place"));
    assert!(!format!("{t:?}").contains("secret-place"));
    let i = seaquel_http::release_asset::Installed {
        path: PathBuf::from("/home/someone/secret-place/x"),
        sha256: "a".repeat(64),
        size: 3,
        asset_sha256: None,
    };
    assert!(!format!("{i:?}").contains("secret-place"));
}

/// A metadata redirect to another origin (here another port on
/// 127.0.0.1) is refused; one on the API's own origin is followed.
#[tokio::test]
async fn metadata_redirects_stay_on_the_api_origin() {
    let mock = MockReleases::start().await;
    let other = MockReleases::start().await;
    let gz = gzip(&payload());
    other.publish(VERSION, NAME, gz.clone());
    mock.route(
        &release_path(VERSION),
        Route::Redirect {
            status: 302,
            location: format!("{}{}", other.url(), release_path(VERSION)),
        },
    );
    let fetcher = Fetcher::new(mock.source(), VERSION);
    let err = fetcher.asset(VERSION, NAME).await.unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::RedirectRefused, "{err}");
    assert_eq!(other.hits(&release_path(VERSION)), 0);

    // GitHub answers a renamed repository with a redirect on its own host.
    mock.publish(VERSION, NAME, gz);
    mock.route(
        "/api/moved",
        Route::Redirect {
            status: 301,
            location: release_path(VERSION),
        },
    );
    mock.route(
        &release_path("9.9.9"),
        Route::Redirect {
            status: 301,
            location: "/api/moved".into(),
        },
    );
    let asset = fetcher.asset("9.9.9", NAME).await.unwrap();
    assert_eq!(asset.name, NAME);
}

/// The metadata has an overall limit, so a server that trickles
/// (or stalls) doesn't hold the dialog forever.
#[tokio::test]
async fn the_metadata_has_an_overall_limit() {
    let mock = MockReleases::start().await;
    let body = vec![b' '; 4096];
    mock.route(
        &release_path(VERSION),
        Route::Body {
            status: 200,
            length: Some(body.len() as u64),
            body,
            stall_after: Some(10),
            close_after: None,
            piece: 10,
            gap: Duration::ZERO,
        },
    );
    let mut fetcher = Fetcher::new(mock.source(), VERSION);
    fetcher.metadata_timeout = Duration::from_millis(300);
    let started = tokio::time::Instant::now();
    let err = tokio::time::timeout(LIMIT, fetcher.asset(VERSION, NAME))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, InstallErrorKind::Network, "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A failed install removes the folders it made, deepest
/// first, as long as they are empty; a folder that was already there, or
/// that holds anything, stays.
#[tokio::test]
async fn a_failed_install_removes_only_the_empty_folders_it_made() {
    let gz = gzip(&payload());
    let n = gz.len() as u64;
    // No route for the asset: the download is a 404.
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n, Some(&digest(&gz)))]);

    // Nothing there: every folder below the root goes; the root stays.
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    fetch_and_install(&mock, &t).await.unwrap_err();
    assert!(t.root.is_dir(), "the app's folder was removed");
    assert_eq!(names(&t.root), Vec::<String>::new());

    // `bin/duckdb` there with another version: only this version's folder
    // goes.
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let duckdb = t.dir().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(duckdb.join("2026.9.1")).unwrap();
    fetch_and_install(&mock, &t).await.unwrap_err();
    assert_eq!(names(&duckdb), ["2026.9.1"]);

    // This version's folder already there and empty: kept.
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    std::fs::create_dir_all(t.dir()).unwrap();
    fetch_and_install(&mock, &t).await.unwrap_err();
    assert!(t.dir().is_dir(), "a folder that was there was removed");

    // A folder the install made that holds something by the time it
    // fails (another process's file) stays, and so do those above it.
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let slow = MockReleases::start().await;
    slow.metadata(VERSION, &[(NAME, n, Some(&digest(&gz)))]);
    slow.route(
        &asset_path(VERSION, NAME),
        Route::Body {
            status: 200,
            length: Some(n),
            body: gz.clone(),
            stall_after: None,
            close_after: Some(gz.len() / 2),
            piece: 4096,
            gap: Duration::from_millis(5),
        },
    );
    let task = {
        let (source, t) = (slow.source(), t.clone());
        tokio::spawn(async move {
            let fetcher = Fetcher::new(source, VERSION);
            let asset = fetcher.asset(VERSION, NAME).await?;
            fetcher.install(&asset, &t, &mut |_| {}).await
        })
    };
    let dir = t.dir();
    tokio::time::timeout(LIMIT, async {
        while !dir.is_dir() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    std::fs::write(dir.join("someone-else"), b"x").unwrap();
    tokio::time::timeout(LIMIT, task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(names(&dir), ["someone-else"]);
}

/// The same when the install is dropped half-way or a copied file's
/// install fails or is cancelled.
#[tokio::test]
async fn a_dropped_or_refused_install_removes_the_folders_it_made() {
    let gz = gzip(&payload());
    let n = gz.len();
    let mock = MockReleases::start().await;
    mock.metadata(VERSION, &[(NAME, n as u64, Some(&digest(&gz)))]);
    mock.route(
        &asset_path(VERSION, NAME),
        Route::Body {
            status: 200,
            length: Some(n as u64),
            body: gz.clone(),
            stall_after: Some(n / 2),
            close_after: None,
            piece: 4096,
            gap: Duration::ZERO,
        },
    );
    let tmp = tempfile::tempdir().unwrap();
    let t = target(tmp.path());
    let task = {
        let (source, t) = (mock.source(), t.clone());
        tokio::spawn(async move {
            let fetcher = Fetcher::new(source, VERSION);
            let asset = fetcher.asset(VERSION, NAME).await?;
            fetcher.install(&asset, &t, &mut |_| {}).await
        })
    };
    let dir = t.dir();
    tokio::time::timeout(LIMIT, async {
        while names(&dir).is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(names(&t.root), Vec::<String>::new());

    // A copied file: refused (another file's hash), and cancelled.
    let file = tmp.path().join("copy.gz");
    std::fs::write(&file, &gz).unwrap();
    let other = &digest(b"other")["sha256:".len()..];
    let t = target(&tmp.path().join("a"));
    assert_eq!(
        install_file(&file, other, &t).unwrap_err().kind,
        InstallErrorKind::DigestMismatch
    );
    assert_eq!(names(&t.root), Vec::<String>::new());
    let cancel = std::sync::atomic::AtomicBool::new(true);
    let good = &digest(&gz)["sha256:".len()..];
    install_file_cancellable(&file, good, &t, &cancel).unwrap_err();
    assert_eq!(names(&t.root), Vec::<String>::new());
    // And a success keeps them.
    install_file(&file, good, &t).unwrap();
    assert!(t.path().is_file());
}
