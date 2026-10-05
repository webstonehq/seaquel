//! `seaquel-cli duckdb status` and `seaquel-cli duckdb install` in the built
//! binary.
//!
//! Every run sets `SEAQUEL_DATA_DIR` to a temp folder under the target dir
//! and `SEAQUEL_CLI_TEST_DUCKDB_RELEASES` to a release server on 127.0.0.1
//! (`MockReleases`), or to a closed loopback port when nothing should be
//! fetched. Never the real data dir or the network.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use seaquel_http::release_asset::target_triple;
use seaquel_http::release_asset::testing::{asset_path, digest, gzip, MockReleases};
use tokio::process::Command;

const VERSION: &str = seaquel_cli::VERSION;
/// Nothing listens there: a command that tried to download would fail
/// with `NETWORK_ERROR` instead of reaching GitHub.
const NO_RELEASES: &str = "http://127.0.0.1:9";
/// What the stand-in helper holds.
const HELPER: &[u8] = b"#!/bin/sh\nexit 0\n";

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("cli-duckdb-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        Self { dir }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn helper_path(&self) -> PathBuf {
        self.data()
            .join("bin")
            .join("duckdb")
            .join(VERSION)
            .join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX))
    }

    /// `seaquel-cli <args>` against the sandbox and `releases`, bounded at
    /// 60 s.
    async fn run(&self, releases: &str, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_seaquel-cli"));
        cmd.args(args)
            .env("SEAQUEL_DATA_DIR", self.data())
            .env("SEAQUEL_CLI_TEST_DUCKDB_RELEASES", releases)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        tokio::time::timeout(Duration::from_secs(60), cmd.output())
            .await
            .expect("seaquel-cli answers in time")
            .unwrap()
    }
}

fn asset_name() -> String {
    let triple = target_triple(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    format!("seaquel-duckdb-{triple}{}.gz", std::env::consts::EXE_SUFFIX)
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// A release server with this version's helper (the stand-in) published.
async fn published() -> MockReleases {
    let mock = MockReleases::start().await;
    mock.publish(VERSION, &asset_name(), gzip(HELPER));
    mock
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread")]
async fn status_says_missing_and_exits_1() {
    let sb = Sandbox::new();
    let out = sb.run(NO_RELEASES, &["duckdb", "status"]).await;
    assert_eq!(stdout(&out), "missing\n", "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("seaquel-cli duckdb install"),
        "{}",
        stderr(&out)
    );
    assert!(!sb.data().join("bin").exists(), "status writes nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn status_says_outdated_when_only_another_version_is_there() {
    let sb = Sandbox::new();
    let old = sb.data().join("bin").join("duckdb").join("2000.1.1");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(
        old.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX)),
        HELPER,
    )
    .unwrap();
    let out = sb.run(NO_RELEASES, &["duckdb", "status"]).await;
    assert_eq!(stdout(&out), "outdated\n", "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(1));
    // The pruning rule, in words.
    assert!(stderr(&out).contains("two newest"), "{}", stderr(&out));
}

/// `install` downloads, prints only the path on stdout and its progress
/// on stderr; `status` then says installed with that path. A second
/// install fetches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn install_prints_the_path_and_status_says_installed() {
    let sb = Sandbox::new();
    let mock = published().await;
    let out = sb.run(mock.url(), &["duckdb", "install"]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let path = sb.helper_path();
    assert_eq!(stdout(&out), format!("{}\n", path.display()));
    let err = stderr(&out);
    assert!(err.contains("100%"), "progress on stderr: {err}");
    assert!(!err.contains(&path.display().to_string()), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), HELPER);
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o700);
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 1);

    let out = sb.run(mock.url(), &["duckdb", "status"]).await;
    assert_eq!(stdout(&out), format!("installed {}\n", path.display()));
    assert_eq!(out.status.code(), Some(0));

    let out = sb.run(mock.url(), &["duckdb", "install"]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", path.display()));
    assert!(
        stderr(&out).contains("already installed"),
        "{}",
        stderr(&out)
    );
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 1);
}

/// A helper folder another process loosened reads as `unsafe`; an install
/// makes it private again without downloading.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn status_says_unsafe_for_a_loose_folder() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let mock = published().await;
    assert!(sb
        .run(mock.url(), &["duckdb", "install"])
        .await
        .status
        .success());
    let bin = sb.data().join("bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o777)).unwrap();
    let out = sb.run(NO_RELEASES, &["duckdb", "status"]).await;
    assert_eq!(stdout(&out), "unsafe\n", "{}", stderr(&out));
    assert_eq!(out.status.code(), Some(1));

    let out = sb.run(NO_RELEASES, &["duckdb", "install"]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(mode(&bin), 0o700);
}

/// A failed install says why on stderr, split as the TUI's dialog words
/// it, with the code; stdout stays empty and nothing is installed.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_install_is_worded_on_stderr() {
    let sb = Sandbox::new();
    let mock = MockReleases::start().await; // nothing published
    let out = sb.run(mock.url(), &["duckdb", "install"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(err.contains("isn't published for this version"), "{err}");
    assert!(err.contains("RELEASE_NOT_FOUND"), "{err}");
    assert!(!sb.helper_path().exists());

    // No network: the network's wording.
    let out = sb.run(NO_RELEASES, &["duckdb", "install"]).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Couldn't reach the download server"),
        "{}",
        stderr(&out)
    );
}

/// `--from FILE --sha256 HEX`: the copied asset is checked against the
/// hash given; a wrong one installs nothing. Nothing is fetched.
#[tokio::test(flavor = "multi_thread")]
async fn install_from_a_file_checks_the_hash() {
    let sb = Sandbox::new();
    let file = sb.dir.path().join(asset_name());
    let gz = gzip(HELPER);
    std::fs::write(&file, &gz).unwrap();
    let right = digest(&gz);
    let right = right.trim_start_matches("sha256:");
    let wrong = "0".repeat(64);
    let from = file.to_str().unwrap();

    let out = sb
        .run(
            NO_RELEASES,
            &["duckdb", "install", "--from", from, "--sha256", &wrong],
        )
        .await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("DIGEST_MISMATCH"), "{}", stderr(&out));
    assert!(!sb.helper_path().exists());

    let out = sb
        .run(
            NO_RELEASES,
            &["duckdb", "install", "--from", from, "--sha256", right],
        )
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), format!("{}\n", sb.helper_path().display()));
    assert_eq!(std::fs::read(sb.helper_path()).unwrap(), HELPER);
}

/// `--from` and `--sha256` only together.
#[tokio::test(flavor = "multi_thread")]
async fn from_needs_sha256_and_back() {
    let sb = Sandbox::new();
    let out = sb
        .run(NO_RELEASES, &["duckdb", "install", "--from", "x.gz"])
        .await;
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = sb
        .run(
            NO_RELEASES,
            &["duckdb", "install", "--sha256", &"0".repeat(64)],
        )
        .await;
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(!sb.data().join("bin").exists());
}

/// The `.part` files in the version folder.
fn parts(sb: &Sandbox) -> Vec<String> {
    let dir = sb.helper_path().parent().unwrap().to_path_buf();
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".part"))
                .collect()
        })
        .unwrap_or_default()
}

/// SIGINT during `install --from` stops the blocking copy
/// between reads and waits for it, so no `.part` file is left, and the CLI
/// exits 130. The file is a regular file of 8 MiB (128 reads of 64 KiB),
/// copied with a 50 ms pause before each read through the debug-only
/// `SEAQUEL_CLI_TEST_SLOW_READ_MS` hook: about 6.4 s in all, well past the
/// CLI's 3 s wait for the copy to stop, so a copy that ignored the cancel
/// would leave its `.part` behind when the process exits. (It used a FIFO until Task 4's review I1 made
/// `--from` refuse anything but a regular file.)
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn sigint_during_install_from_leaves_no_partial_file() {
    let sb = Sandbox::new();
    let file = sb.dir.path().join("asset.gz");
    std::fs::write(&file, vec![0u8; 8 * 1024 * 1024]).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_seaquel-cli"));
    let zeros = "0".repeat(64);
    cmd.args([
        "duckdb",
        "install",
        "--from",
        file.to_str().unwrap(),
        "--sha256",
        &zeros,
    ])
    .env("SEAQUEL_DATA_DIR", sb.data())
    .env("SEAQUEL_CLI_TEST_DUCKDB_RELEASES", NO_RELEASES)
    .env("SEAQUEL_CLI_TEST_SLOW_READ_MS", "50")
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);
    let child = cmd.spawn().unwrap();
    // Mid-copy: the partial file is there.
    let mut waited = 0;
    while parts(&sb).is_empty() {
        assert!(waited < 400, "the install never started copying");
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += 1;
    }
    let pid = child.id().unwrap().to_string();
    assert!(std::process::Command::new("kill")
        .args(["-INT", pid.as_str()])
        .status()
        .unwrap()
        .success());
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("exits after SIGINT")
        .unwrap();
    assert_eq!(out.status.code(), Some(130), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    assert_eq!(parts(&sb), Vec::<String>::new(), "a partial file was left");
    assert!(!sb.helper_path().exists());
}

/// A FIFO given to `--from` (nothing writing to it, so
/// opening it would block) is refused at once with a plain error, and
/// nothing is made.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_fifo_given_to_from_is_refused_at_once() {
    let sb = Sandbox::new();
    let fifo = sb.dir.path().join("asset.gz");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let zeros = "0".repeat(64);
    let out = tokio::time::timeout(
        Duration::from_secs(10),
        sb.run(
            NO_RELEASES,
            &[
                "duckdb",
                "install",
                "--from",
                fifo.to_str().unwrap(),
                "--sha256",
                &zeros,
            ],
        ),
    )
    .await
    .expect("refused at once, not blocked on the FIFO");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let err = stderr(&out);
    assert!(err.contains("isn't a regular file"), "{err}");
    assert!(!err.contains(&*sb.dir.path().to_string_lossy()), "{err}");
    assert_eq!(parts(&sb), Vec::<String>::new());
    assert!(!sb.helper_path().exists());
}
