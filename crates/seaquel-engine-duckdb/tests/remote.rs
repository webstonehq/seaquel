//! The remote driver (the DuckDB helper plan, Task 3): a `Driver` over a
//! `seaquel-duckdb` child process.
//!
//! `SEAQUEL_TEST_DUCKDB_HELPER` names a built helper (`cargo build -p
//! seaquel-duckdb`); without it the tests that need one are skipped, and
//! with `SEAQUEL_TEST_REQUIRE_ENGINES` set they fail. Each test installs the
//! helper (a hard link, else a copy) into a folder of its own under
//! `CARGO_TARGET_TMPDIR`, laid out as the install does
//! (`bin/duckdb/<version>/seaquel-duckdb`), so its processes can be found by
//! that path. Every test runs under [`LIMIT`], and a runtime that ends kills
//! whatever helper it still holds (`kill_on_drop`).
#![cfg(feature = "remote")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use seaquel_engine::{
    BatchStatement, CancellationToken, ConnectConfig, DbError, Driver, ExpectRows, ReadOnlyOptions,
    Value,
};
use seaquel_engine_duckdb::{remote_engine, HelperLocator};
use tokio::time::{timeout, Instant};

#[path = "common/cast.rs"]
mod cast;

/// How long any one test may take.
const LIMIT: Duration = Duration::from_secs(60);

/// The helper's file name.
fn file_name() -> String {
    format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX)
}

/// The built helper, or `None` (skipped) when `SEAQUEL_TEST_DUCKDB_HELPER`
/// isn't set.
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

/// The app version the built helper reports (`--version`).
fn helper_version(bin: &Path) -> String {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let out = std::process::Command::new(bin)
                .arg("--version")
                .output()
                .expect("run the helper's --version");
            let text = String::from_utf8(out.stdout).unwrap();
            text.trim()
                .strip_prefix("seaquel-duckdb ")
                .expect("seaquel-duckdb <version>")
                .to_string()
        })
        .clone()
}

/// A helper installed in a folder of the test's own.
struct Install {
    dir: tempfile::TempDir,
    locator: HelperLocator,
}

impl Install {
    /// The folders (`bin/duckdb/<version>`, each 0700), no helper yet.
    fn empty(version: &str) -> Install {
        let dir = tempfile::Builder::new()
            .prefix("remote-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let bin = dir.path().join("bin");
        let root = bin.join("duckdb");
        let folder = root.join(version);
        std::fs::create_dir_all(&folder).unwrap();
        for d in [&bin, &root, &folder] {
            private(d);
        }
        Install {
            dir,
            locator: HelperLocator {
                dir: root,
                version: version.to_string(),
            },
        }
    }

    /// The built helper, installed under `version` (another version than
    /// the helper's own makes a stale install).
    fn helper(bin: &Path, version: &str) -> Install {
        let install = Install::empty(version);
        let to = install.path();
        if std::fs::hard_link(bin, &to).is_err() {
            std::fs::copy(bin, &to).unwrap();
        }
        install
    }

    /// The built helper under its own version.
    fn current(bin: &Path) -> Install {
        Install::helper(bin, &helper_version(bin))
    }

    /// A shell script in the helper's place.
    #[cfg(unix)]
    fn script(version: &str, body: &str) -> Install {
        use std::os::unix::fs::PermissionsExt;
        let install = Install::empty(version);
        std::fs::write(install.path(), format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(install.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        install
    }

    fn path(&self) -> PathBuf {
        self.locator
            .dir
            .join(&self.locator.version)
            .join(file_name())
    }

    async fn open(&self) -> Result<Arc<dyn Driver>, DbError> {
        remote_engine(self.locator.clone()).open(&memory()).await
    }

    async fn open_config(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        remote_engine(self.locator.clone()).open(config).await
    }

    async fn driver(&self) -> Arc<dyn Driver> {
        match self.open().await {
            Ok(driver) => driver,
            Err(e) => panic!("open: {e}"),
        }
    }

    /// The helper processes running from this install.
    #[cfg(unix)]
    fn pids(&self) -> Vec<u32> {
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(self.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| l.trim().parse().unwrap())
            .collect()
    }

    /// Waits up to `within` for this install's helpers to be gone.
    #[cfg(unix)]
    async fn gone_within(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            if self.pids().is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Shell functions speaking the helper's frames, for fake helpers: `send
/// CALL JSON` writes a control frame, `recv` reads one into `$CALL` and
/// `$BODY` (and exits at the input's end), `greet VERSION` answers `hello`
/// and `open`.
#[cfg(unix)]
const FRAMES_SH: &str = r#"
u32() { printf "$(printf '\\%03o\\%03o\\%03o\\%03o' $(($1 & 255)) $((($1 >> 8) & 255)) $((($1 >> 16) & 255)) $((($1 >> 24) & 255)))"; }
send() { u32 $(( ${#2} + 5 )); printf '\000'; u32 "$1"; printf '%s' "$2"; }
num() { dd bs=1 count=4 2>/dev/null | od -An -tu4 | tr -d ' \n'; }
recv() { LEN=$(num); [ -n "$LEN" ] || exit 0; dd bs=1 count=1 2>/dev/null >/dev/null; CALL=$(num); BODY=$(dd bs=1 count=$((LEN - 5)) 2>/dev/null); }
greet() { recv; send 0 "{\"type\":\"helloOk\",\"protocol\":2,\"version\":\"$1\",\"duckdb\":\"fake\"}"; recv; send 1 '{"type":"opened"}'; }
"#;

#[cfg(unix)]
fn private(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[cfg(not(unix))]
fn private(_: &Path) {}

#[cfg(unix)]
fn kill(signal: &str, pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .status()
        .unwrap()
        .success()
}

/// An in-memory database on one thread, so a result's rows come in order.
fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": ":memory:",
        "duckdb_config": { "threads": "1" }
    }))
    .unwrap()
}

fn open_error(outcome: Result<Arc<dyn Driver>, DbError>) -> DbError {
    match outcome {
        Ok(_) => panic!("the open succeeded"),
        Err(e) => e,
    }
}

/// Runs a test body under [`LIMIT`].
async fn limit<T>(body: impl std::future::Future<Output = T>) -> T {
    timeout(LIMIT, body).await.expect("the test took too long")
}

/// 50M rows hashing a kilobyte each: minutes in full, a first batch at once.
const LONG_STREAM: &str =
    "SELECT i, md5(i::VARCHAR || repeat('x', 1000)) AS h FROM range(50000000) t(i)";

/// Seconds of work with no rows until the end.
const LONG_SUM: &str = "SELECT sum(i % 7) AS s FROM range(3000000000) t(i)";

async fn assert_usable(driver: &dyn Driver, within: Duration) {
    let started = Instant::now();
    let r = timeout(within, driver.query("SELECT 42 AS n", vec![]))
        .await
        .unwrap_or_else(|_| panic!("the connection is still busy after {within:?}"))
        .unwrap();
    assert_eq!(
        r.rows,
        vec![vec![Value::Int(42)]],
        "{:?}",
        started.elapsed()
    );
}

// ── Installs that aren't there or can't be trusted ───────────────────────

#[tokio::test]
async fn a_missing_helper_is_not_installed() {
    limit(async {
        let install = Install::empty("2026.1.1");
        let started = Instant::now();
        let e = open_error(install.open().await);
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");
        assert!(started.elapsed() < Duration::from_millis(100));
        let dir = install.dir.path().to_str().unwrap();
        assert!(!e.message.contains(dir), "a path in {e}");
    })
    .await;
}

#[tokio::test]
async fn another_version_is_not_installed() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::helper(&bin, "2000.1.1");
        let started = Instant::now();
        let e = open_error(install.open().await);
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");
        assert!(
            e.message.contains("2000.1.1"),
            "names the version wanted: {e}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(1000),
            "{:?}",
            started.elapsed()
        );
        #[cfg(unix)]
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}

/// A script that leaves a mark when it runs.
#[cfg(unix)]
fn marking_script(version: &str) -> (Install, PathBuf) {
    let marker = std::env::temp_dir().join(format!(
        "seaquel-remote-marker-{}-{}",
        std::process::id(),
        uuid_like()
    ));
    let install = Install::script(version, &format!("touch '{}'", marker.display()));
    (install, marker)
}

fn uuid_like() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

#[cfg(unix)]
async fn refused_without_spawning(install: &Install, marker: &Path) {
    let started = Instant::now();
    let e = open_error(install.open().await);
    assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");
    assert!(e.message.contains("unsafe permissions"), "{e}");
    assert!(started.elapsed() < Duration::from_millis(100));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!marker.exists(), "the helper was started");
}

#[cfg(unix)]
#[tokio::test]
async fn a_folder_others_can_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    limit(async {
        let (install, marker) = marking_script("2026.1.1");
        // The trusted script runs: the mark proves the check below is what
        // stops the spawn.
        let _ = install.open().await;
        assert!(marker.exists(), "the script didn't run");
        std::fs::remove_file(&marker).unwrap();
        for folder in [
            install.locator.dir.join("2026.1.1"),
            install.locator.dir.clone(),
            install.locator.dir.parent().unwrap().to_path_buf(),
            // `<identifier>`, the folder above `bin` (the review's M1).
            install.dir.path().to_path_buf(),
        ] {
            for mode in [0o777, 0o720, 0o702] {
                std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(mode)).unwrap();
                refused_without_spawning(&install, &marker).await;
            }
            private(&folder);
        }
        // So is the file itself.
        std::fs::set_permissions(install.path(), std::fs::Permissions::from_mode(0o722)).unwrap();
        refused_without_spawning(&install, &marker).await;
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_is_refused() {
    limit(async {
        let (install, marker) = marking_script("2026.1.1");
        // The helper is a link to the script.
        let real = install.dir.path().join("real");
        std::fs::rename(install.path(), &real).unwrap();
        std::os::unix::fs::symlink(&real, install.path()).unwrap();
        refused_without_spawning(&install, &marker).await;
        // The version folder is a link to a folder holding the script.
        std::fs::remove_file(install.path()).unwrap();
        let elsewhere = install.dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        private(&elsewhere);
        std::fs::rename(&real, elsewhere.join(file_name())).unwrap();
        let folder = install.locator.dir.join("2026.1.1");
        std::fs::remove_dir(&folder).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &folder).unwrap();
        refused_without_spawning(&install, &marker).await;
    })
    .await;
}

// The handshake's bounds (5 s, and one 20 s retry for a file this process
// hasn't started yet) are tested in `remote/process.rs` with short bounds.

// ── Lifecycle ─────────────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test]
async fn dropping_the_driver_ends_the_helper() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        assert_usable(&*driver, Duration::from_secs(5)).await;
        assert_eq!(install.pids().len(), 1, "one helper per connection");
        drop(driver);
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn close_lets_the_helper_exit() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        assert_usable(&*driver, Duration::from_secs(5)).await;
        let started = Instant::now();
        driver.close().await.unwrap();
        // It exited on its own: the kill would come after 2 s.
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(install.gone_within(Duration::from_millis(100)).await);
        let e = driver.query("SELECT 1", vec![]).await.unwrap_err();
        assert_eq!(e.code, "CONNECTION_CLOSED", "{e}");
    })
    .await;
}

/// `close` right after a stream was dropped (Core's disconnect cancels
/// its streams, then closes): the stream's last frame arriving after the
/// calls were failed doesn't count as a broken wire, so the helper closes
/// the database and exits on its own. A clean exit checkpoints, which
/// removes the WAL; a kill leaves it.
#[tokio::test]
async fn close_during_a_stream_closes_the_database_cleanly() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let file = install.dir.path().join("closing.duckdb");
        let wal = install.dir.path().join("closing.duckdb.wal");
        let config: ConnectConfig = serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": file.to_str().unwrap(),
            "create_if_missing": true,
            "duckdb_config": { "threads": "1" }
        }))
        .unwrap();
        let driver = match remote_engine(install.locator.clone()).open(&config).await {
            Ok(driver) => driver,
            Err(e) => panic!("open: {e}"),
        };
        driver
            .execute(
                "CREATE TABLE t (a INT, b VARCHAR); \
                 INSERT INTO t SELECT i, md5(i::VARCHAR) FROM range(1000000) r(i)",
                vec![],
            )
            .await
            .unwrap();
        assert!(
            wal.exists(),
            "nothing to checkpoint: the test proves nothing"
        );
        let mut stream =
            driver.query_stream(LONG_STREAM.to_string(), vec![], CancellationToken::new());
        stream.next().await.unwrap().unwrap();
        // As Core's disconnect does: the stream goes (its cancel is
        // posted), then the driver closes at once. The helper's answer to
        // the cancel arrives after the calls were failed.
        drop(stream);
        driver.close().await.unwrap();
        assert!(!wal.exists(), "the helper was killed, not closed");
    })
    .await;
}

/// `close` refuses new calls only: a call in flight gets its real answer
/// (the review's I1). The fake helper answers the `execute` after it has
/// read `close`, then exits.
#[cfg(unix)]
#[tokio::test]
async fn close_lets_a_call_in_flight_finish() {
    limit(async {
        let install = Install::script(
            "2026.1.1",
            &format!(
                "{FRAMES_SH}\ngreet 2026.1.1\nrecv\nEXEC=$CALL\nrecv\n\
                 send $EXEC '{{\"type\":\"executed\",\"rowsAffected\":7}}'\nexit 0"
            ),
        );
        let driver = install.driver().await;
        let (executed, ()) = tokio::join!(driver.execute("UPDATE t SET a = 1", vec![]), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            driver.close().await.unwrap();
        });
        assert_eq!(executed.unwrap().rows_affected, 7);
        let e = driver.query("SELECT 1", vec![]).await.unwrap_err();
        assert_eq!(e.code, "CONNECTION_CLOSED", "{e}");
        assert!(e.message.contains("closed"), "{e}");
    })
    .await;
}

/// Probe F3: a helper that took `close` (its output ended) but is still
/// checkpointing after 2 s is left to finish, not killed: `close` stops
/// waiting and the helper exits on its own. The fake helper closes its
/// output on `close`, then takes 4 s before it exits and leaves a marker.
#[cfg(unix)]
#[tokio::test]
async fn close_leaves_a_closing_helper_to_finish() {
    limit(async {
        let marker = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "closed-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let _ = std::fs::remove_file(&marker);
        let install = Install::script(
            "2026.1.1",
            &format!(
                "{FRAMES_SH}\ngreet 2026.1.1\nrecv\nexec 1>&-\nsleep 4\ntouch '{}'\nexit 0",
                marker.display()
            ),
        );
        let driver = install.driver().await;
        let started = Instant::now();
        driver.close().await.unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert!(!install.pids().is_empty(), "the helper was killed");
        let e = driver.query("SELECT 1", vec![]).await.unwrap_err();
        assert_eq!(e.code, "CONNECTION_CLOSED", "{e}");
        // Dropping the driver now doesn't kill it either.
        drop(driver);
        assert!(install.gone_within(Duration::from_secs(8)).await);
        assert!(marker.exists(), "the helper didn't finish on its own");
        let _ = std::fs::remove_file(&marker);
    })
    .await;
}

/// A helper that doesn't act on `close` (its output stays open) is still
/// killed after 2 s.
#[cfg(unix)]
#[tokio::test]
async fn close_kills_a_helper_that_does_not_close() {
    limit(async {
        let install = Install::script(
            "2026.1.1",
            &format!("{FRAMES_SH}\ngreet 2026.1.1\nrecv\nsleep 30"),
        );
        let driver = install.driver().await;
        let started = Instant::now();
        driver.close().await.unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(6), "{took:?}");
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}

/// A driver dropped without `close` kills its helper, whatever it is doing.
#[cfg(unix)]
#[tokio::test]
async fn dropping_the_driver_kills_a_helper_that_ignores_its_input() {
    limit(async {
        let install = Install::script(
            "2026.1.1",
            &format!("{FRAMES_SH}\ngreet 2026.1.1\nsleep 30"),
        );
        let driver = install.driver().await;
        assert_eq!(install.pids().len(), 1);
        drop(driver);
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}

/// A file database's config, created if missing, on one thread.
fn file_config(path: &Path) -> ConnectConfig {
    serde_json::from_value(serde_json::json!({
        "driver": "duckdb",
        "path": path.to_str().unwrap(),
        "create_if_missing": true,
        "duckdb_config": { "threads": "1" }
    }))
    .unwrap()
}

/// A Perl proxy in the helper's place, in front of a copy of the real
/// helper of its own (`real`): each start appends a line to `starts`. On
/// `close` it ends its output at once, as a helper does once it took
/// `close`, but keeps the real helper (and its lock on the file) for
/// `hold` seconds, as a long close checkpoint would; then it passes
/// `close` on, waits for the helper and exits.
#[cfg(unix)]
fn holding_proxy(version: &str, real: &Path, starts: &Path, hold: u32) -> Install {
    use std::os::unix::fs::PermissionsExt;
    let install = Install::empty(version);
    let q = |p: &Path| p.to_string_lossy().replace('\'', "\\'");
    let script = format!(
        r#"#!/usr/bin/perl
use strict;
use warnings;
use IPC::Open2;
$SIG{{PIPE}} = 'IGNORE';
open(my $log, '>>', '{starts}'); print $log "start\n"; close $log;
my $pid = open2(my $from, my $to, '{real}', @ARGV);
binmode STDIN; binmode STDOUT; binmode $from; binmode $to;
sub put {{ my ($fh, $d) = @_; while (length $d) {{ my $w = syswrite($fh, $d); exit 1 unless defined $w; substr($d, 0, $w, ''); }} }}
my ($buf, $in, $out) = ('', 1, 1);
while ($out) {{
  my $rin = '';
  vec($rin, fileno(STDIN), 1) = 1 if $in;
  vec($rin, fileno($from), 1) = 1;
  next unless select(my $rout = $rin, undef, undef, undef) > 0;
  if ($in && vec($rout, fileno(STDIN), 1)) {{
    my $n = sysread(STDIN, my $chunk, 65536);
    if (!$n) {{ $in = 0; close $to; }}
    else {{
      $buf .= $chunk;
      while (length($buf) >= 4) {{
        my $len = unpack('V', substr($buf, 0, 4));
        last if length($buf) < 4 + $len;
        my $frame = substr($buf, 0, 4 + $len, '');
        my $body = substr($frame, 4, 1) eq "\0" ? substr($frame, 9) : '';
        if ($body =~ /^\x7b"type":"close"/) {{
          close STDOUT; sleep {hold}; put($to, $frame); close $to;
          while (sysread($from, my $c, 65536)) {{}}
          waitpid($pid, 0); exit 0;
        }}
        put($to, $frame);
      }}
    }}
  }}
  if (vec($rout, fileno($from), 1)) {{
    my $n = sysread($from, my $chunk, 65536);
    if (!$n) {{ $out = 0; }} else {{ put(\*STDOUT, $chunk); }}
  }}
}}
waitpid($pid, 0);
exit($? >> 8);
"#,
        starts = q(starts),
        real = q(real),
    );
    std::fs::write(install.path(), script).unwrap();
    std::fs::set_permissions(install.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    install
}

/// A copy of the built helper in `dir` (a hard link, else a copy), so a
/// test's process checks see only its own helpers.
fn own_copy(bin: &Path, dir: &Path) -> PathBuf {
    let to = dir.join(format!("real-{}", file_name()));
    if std::fs::hard_link(bin, &to).is_err() {
        std::fs::copy(bin, &to).unwrap();
    }
    to
}

/// Review I1 of the probe fixes: a reconnect while this process's helper
/// for the same file is still closing it (left to finish after `close`)
/// waits for that helper to exit, then starts once, instead of meeting
/// DuckDB's lock on the file.
#[cfg(unix)]
#[tokio::test]
async fn a_reconnect_waits_for_the_closing_helper_of_the_same_file() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let dir = tempfile::Builder::new()
            .prefix("closing-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let real = own_copy(&bin, dir.path());
        let starts = dir.path().join("starts");
        let install = holding_proxy(&helper_version(&bin), &real, &starts, 4);
        let config = file_config(&dir.path().join("held.duckdb"));
        let engine = remote_engine(install.locator.clone());
        let driver = engine.open(&config).await.unwrap();
        driver
            .execute("CREATE TABLE t AS SELECT 1 AS a", vec![])
            .await
            .unwrap();
        driver.close().await.unwrap();
        assert!(!install.pids().is_empty(), "the closing helper was let go");
        let started = Instant::now();
        let again = match engine.open(&config).await {
            Ok(driver) => driver,
            Err(e) => panic!("the reconnect failed: {e}"),
        };
        assert!(
            started.elapsed() >= Duration::from_millis(500),
            "it didn't wait: {:?}",
            started.elapsed()
        );
        let r = again.query("SELECT a FROM t", vec![]).await.unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
        let lines = std::fs::read_to_string(&starts).unwrap();
        assert_eq!(lines.lines().count(), 2, "one start per open");
        drop(again);
        assert!(install.gone_within(Duration::from_secs(2)).await);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pids(&real).is_empty() {
            assert!(Instant::now() < deadline, "the real helper outlived it");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
}

/// The pids of processes started from `path`.
#[cfg(unix)]
fn pids(path: &Path) -> Vec<u32> {
    let out = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| l.trim().parse().unwrap())
        .collect()
}

/// A helper of another process holding `file` open: `hello` and `open`
/// written by hand, the replies read. Dropping its stdin ends it.
fn holder(real: &Path, version: &str, file: &Path) -> std::process::Child {
    use std::io::{Read, Write};
    let mut child = std::process::Command::new(real)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.as_mut().unwrap();
    let frame = |call: u32, json: serde_json::Value| {
        let body = json.to_string().into_bytes();
        let mut f = ((body.len() + 5) as u32).to_le_bytes().to_vec();
        f.push(0);
        f.extend(call.to_le_bytes());
        f.extend(body);
        f
    };
    stdin
        .write_all(&frame(
            0,
            serde_json::json!({"type": "hello", "protocol": 2, "version": version}),
        ))
        .unwrap();
    stdin
        .write_all(&frame(
            1,
            serde_json::json!({"type": "open", "path": file.to_str().unwrap(), "createIfMissing": true}),
        ))
        .unwrap();
    let stdout = child.stdout.as_mut().unwrap();
    for _ in 0..2 {
        let mut len = [0u8; 4];
        stdout.read_exact(&mut len).unwrap();
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        stdout.read_exact(&mut body).unwrap();
        assert!(
            !String::from_utf8_lossy(&body).contains("\"error\""),
            "the holder didn't open the file"
        );
    }
    child
}

/// Review I1 of the probe fixes, across processes: another process's
/// helper holds the file. The open retries DuckDB's lock conflict for up
/// to 10 s and succeeds once the file is free.
#[cfg(unix)]
#[tokio::test]
async fn an_open_retries_a_file_another_process_is_closing() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let real = own_copy(&bin, install.dir.path());
        let file = install.dir.path().join("held.duckdb");
        let mut held = holder(&real, &helper_version(&bin), &file);
        let release = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_secs(2));
            drop(held.stdin.take());
            held.wait().unwrap();
        });
        let started = Instant::now();
        let driver = match install.open_config(&file_config(&file)).await {
            Ok(driver) => driver,
            Err(e) => panic!("the open wasn't retried: {e}"),
        };
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_usable(&*driver, Duration::from_secs(5)).await;
        release.await.unwrap();
    })
    .await;
}

/// …and past 10 s it fails, worded without the other process's id or the
/// file's path.
#[cfg(unix)]
#[tokio::test]
async fn a_file_held_past_the_retry_is_refused_in_plain_words() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let real = own_copy(&bin, install.dir.path());
        let file = install.dir.path().join("held.duckdb");
        let mut held = holder(&real, &helper_version(&bin), &file);
        let started = Instant::now();
        let e = open_error(install.open_config(&file_config(&file)).await);
        let took = started.elapsed();
        assert!(
            took >= Duration::from_secs(10) && took < Duration::from_secs(15),
            "{took:?}"
        );
        assert_eq!(
            e.message,
            "DuckDB is still closing this file in another Seaquel process. Try again in a few \
             seconds."
        );
        assert_eq!(e.code, "CONNECTION_ERROR");
        drop(held.stdin.take());
        held.wait().unwrap();
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}

/// The helper inherits the environment, minus Seaquel's test hooks
/// (`SEAQUEL_*_TEST_*`, the review's M3). This process has
/// `SEAQUEL_TEST_DUCKDB_HELPER`; a script records what the helper gets,
/// then runs the real one.
#[cfg(unix)]
#[tokio::test]
async fn the_helper_gets_no_test_variables() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let version = helper_version(&bin);
        let env = std::env::temp_dir().join(format!(
            "seaquel-remote-env-{}-{}",
            std::process::id(),
            uuid_like()
        ));
        let install = Install::script(
            &version,
            &format!("env > '{}'\nexec '{}'", env.display(), bin.display()),
        );
        let driver = install.driver().await;
        assert_usable(&*driver, Duration::from_secs(5)).await;
        let seen = std::fs::read_to_string(&env).unwrap();
        let _ = std::fs::remove_file(&env);
        assert!(seen.lines().any(|l| l.starts_with("PATH=")), "PATH is kept");
        let hooks: Vec<_> = seen
            .lines()
            .filter_map(|l| l.split_once('=').map(|(k, _)| k))
            .filter(|k| k.starts_with("SEAQUEL_") && k.contains("_TEST_"))
            .collect();
        assert!(hooks.is_empty(), "{hooks:?}");
    })
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_killed_helper_fails_the_stream_and_every_later_call() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        let mut stream =
            driver.query_stream(LONG_STREAM.to_string(), vec![], CancellationToken::new());
        let first = stream.next().await.unwrap().unwrap();
        assert!(!first.rows.is_empty());
        let pids = install.pids();
        assert_eq!(pids.len(), 1);
        assert!(kill("-9", pids[0]));
        let e = loop {
            match stream.next().await {
                Some(Ok(_)) => continue,
                Some(Err(e)) => break e,
                None => panic!("the stream ended without an error"),
            }
        };
        assert_eq!(e.code, "CONNECTION_CLOSED", "{e}");
        assert!(e.message.contains("signal 9"), "{e}");
        drop(stream);
        let started = Instant::now();
        let e = driver.query("SELECT 1", vec![]).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(e.code, "CONNECTION_CLOSED", "{e}");
        assert!(e.message.contains("signal 9"), "{e}");
    })
    .await;
}

// ── Cancel and drop ───────────────────────────────────────────────────────

#[tokio::test]
async fn a_dropped_stream_stops_its_query() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        assert_usable(&*driver, Duration::from_secs(5)).await;
        let mut stream =
            driver.query_stream(LONG_STREAM.to_string(), vec![], CancellationToken::new());
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.columns, Some(vec!["i".to_string(), "h".to_string()]));
        drop(stream);
        assert_usable(&*driver, Duration::from_millis(100)).await;
    })
    .await;
}

/// A cancel token stops the stream as dropping it does.
#[tokio::test]
async fn a_cancelled_stream_ends_and_stops_its_query() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        let cancel = CancellationToken::new();
        let mut stream = driver.query_stream(LONG_STREAM.to_string(), vec![], cancel.clone());
        stream.next().await.unwrap().unwrap();
        cancel.cancel();
        let rest = timeout(Duration::from_secs(1), async {
            let mut n = 0;
            while let Some(item) = stream.next().await {
                assert!(item.is_ok(), "{:?}", item.err());
                n += 1;
            }
            n
        })
        .await
        .expect("the stream didn't end");
        assert!(rest <= 3, "{rest} batches after the cancel");
        drop(stream);
        assert_usable(&*driver, Duration::from_millis(100)).await;
    })
    .await;
}

#[tokio::test]
async fn a_dropped_query_stops_it() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        let started = timeout(Duration::from_millis(300), driver.query(LONG_SUM, vec![])).await;
        assert!(started.is_err(), "the query finished");
        assert_usable(&*driver, Duration::from_millis(200)).await;
    })
    .await;
}

#[tokio::test]
async fn a_dropped_transaction_rolls_back() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let driver = install.driver().await;
        driver
            .execute("CREATE TABLE t (a BIGINT)", vec![])
            .await
            .unwrap();
        let batch = vec![
            BatchStatement {
                sql: "INSERT INTO t VALUES (1)".into(),
                params: vec![],
                expect_rows: None,
            },
            BatchStatement {
                sql: format!("INSERT INTO t {LONG_SUM}"),
                params: vec![],
                expect_rows: None,
            },
        ];
        let outcome = timeout(Duration::from_millis(300), driver.transaction(batch)).await;
        assert!(outcome.is_err(), "the transaction finished");
        let r = driver
            .query("SELECT count(*) FROM t", vec![])
            .await
            .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(0)]]);
    })
    .await;
}

// ── Every call ────────────────────────────────────────────────────────────

#[tokio::test]
async fn every_call_answers() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;

        // query, with a bound value, and an empty result's columns
        let r = d
            .query(
                "SELECT 42 AS n, ?::VARCHAR AS s",
                vec![Value::Text("x".into())],
            )
            .await
            .unwrap();
        assert_eq!(r.columns, vec!["n", "s"]);
        assert_eq!(r.rows, vec![vec![Value::Int(42), Value::Text("x".into())]]);
        let r = d.query("SELECT 1 AS a WHERE false", vec![]).await.unwrap();
        assert_eq!(r.columns, vec!["a"]);
        assert!(r.rows.is_empty());
        let e = d.query("SELECT nope", vec![]).await.unwrap_err();
        assert_eq!(e.code, "QUERY_ERROR", "{e}");

        // execute
        d.execute("CREATE TABLE t (a INTEGER PRIMARY KEY, b VARCHAR)", vec![])
            .await
            .unwrap();
        let x = d
            .execute(
                "INSERT INTO t VALUES (?, 'one'), (2, 'two')",
                vec![Value::Int(1)],
            )
            .await
            .unwrap();
        assert_eq!(x.rows_affected, 2);
        assert_eq!(x.last_insert_id, None);

        // transaction: a failure names its statement and rolls back
        let stmt = |sql: &str, min: Option<u64>| BatchStatement {
            sql: sql.into(),
            params: vec![],
            expect_rows: min.map(|min| ExpectRows { min }),
        };
        let e = d
            .transaction(vec![
                stmt("UPDATE t SET b = 'changed'", Some(1)),
                stmt("INSERT INTO t VALUES (1, 'dup')", None),
            ])
            .await
            .unwrap_err();
        assert_eq!(e.index, Some(1), "{e}");
        let e = d
            .transaction(vec![stmt("DELETE FROM t WHERE a = 99", Some(1))])
            .await
            .unwrap_err();
        assert_eq!(e.error.code, "NO_ROWS_AFFECTED", "{e}");
        let affected = d
            .transaction(vec![
                stmt("UPDATE t SET b = b || '!'", Some(1)),
                stmt("INSERT INTO t VALUES (3, 'three')", None),
            ])
            .await
            .unwrap();
        assert_eq!(affected, vec![2, 1]);
        let r = d.query("SELECT b FROM t ORDER BY a", vec![]).await.unwrap();
        assert_eq!(
            r.rows,
            vec![
                vec![Value::Text("one!".into())],
                vec![Value::Text("two!".into())],
                vec![Value::Text("three".into())],
            ]
        );

        // query_stream: 5,000-row batches, the columns on the first, an
        // empty result's on its final batch
        let batches: Vec<_> = d
            .query_stream(
                "SELECT range AS r FROM range(12345)".into(),
                vec![],
                CancellationToken::new(),
            )
            .map(Result::unwrap)
            .collect()
            .await;
        let sizes: Vec<_> = batches.iter().map(|b| b.rows.len()).collect();
        assert_eq!(sizes, vec![5000, 5000, 2345]);
        assert_eq!(batches[0].columns, Some(vec!["r".to_string()]));
        assert!(batches[1].columns.is_none());
        assert!(batches[2].is_final && !batches[0].is_final);
        assert_eq!(batches[2].rows[2344], vec![Value::Int(12344)]);
        let batches: Vec<_> = d
            .query_stream(
                "SELECT 1 AS a WHERE false".into(),
                vec![],
                CancellationToken::new(),
            )
            .map(Result::unwrap)
            .collect()
            .await;
        assert_eq!(batches.len(), 1);
        assert!(batches[0].is_final && batches[0].rows.is_empty());
        assert_eq!(batches[0].columns, Some(vec!["a".to_string()]));
        let items: Vec<_> = d
            .query_stream("SELECT nope".into(), vec![], CancellationToken::new())
            .collect()
            .await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].as_ref().unwrap_err().code, "QUERY_ERROR");

        // the read-only path: capped, truncated, refusing writes and binds
        let r = d
            .query_read_only_with(
                "SELECT * FROM range(10)",
                vec![],
                ReadOnlyOptions::default().with_max_rows(Some(3)),
            )
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 3);
        assert!(r.truncated);
        let r = d
            .query_read_only_with(
                "SELECT repeat('x', 100) FROM range(10)",
                vec![],
                ReadOnlyOptions::default().with_max_bytes(Some(250)),
            )
            .await
            .unwrap();
        // The row that crosses the budget is kept.
        let kept = r.rows.len();
        assert!((2..10).contains(&kept), "{kept}");
        assert!(r.truncated);
        let r = d
            .query_read_only("SELECT a FROM t ORDER BY a", vec![], None)
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 3);
        assert!(!r.truncated);
        let e = d
            .query_read_only("INSERT INTO t VALUES (9, 'x')", vec![], None)
            .await
            .unwrap_err();
        assert_eq!(e.code, "READ_ONLY", "{e}");
        let e = d
            .query_read_only("SELECT ?", vec![Value::Int(1)], None)
            .await
            .unwrap_err();
        assert_eq!(e.code, "READ_ONLY", "{e}");

        // EXPLAIN: one statement, read-only
        let e = d
            .explain_read_only("SELECT 1; DROP TABLE t", vec![], None)
            .await
            .unwrap_err();
        assert_eq!(e.code, "READ_ONLY", "{e}");
        let plan = d
            .explain_read_only("SELECT * FROM t WHERE a = ?", vec![Value::Int(1)], None)
            .await
            .unwrap();
        assert!(!plan.is_analyze);
        let r = d.query("SELECT count(*) FROM t", vec![]).await.unwrap();
        assert_eq!(
            r.rows,
            vec![vec![Value::Int(3)]],
            "the table is still there"
        );

        // introspection, through `query`
        let schemas = d.list_schemas().await.unwrap();
        assert!(schemas.contains(&"main".to_string()), "{schemas:?}");
        let tables = d.schema_tables().await.unwrap();
        assert!(tables.iter().any(|t| t.name == "t"));
        let (columns, _) = d.table_metadata("main", "t").await.unwrap();
        assert_eq!(columns.len(), 2);
        assert!(d.explain("SELECT * FROM t", vec![], false).await.is_ok());
    })
    .await;
}

/// Results the helper sends in pieces: an ENUM's dictionary ahead of its
/// batch, chunks sliced under 8 MiB, one row alone past it, and a row too
/// large for any frame.
#[tokio::test]
async fn results_in_pieces_arrive_whole() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;
        d.execute("CREATE TYPE mood AS ENUM ('sad', 'ok')", vec![])
            .await
            .unwrap();
        let r = d
            .query(
                "SELECT (CASE WHEN i % 2 = 0 THEN 'ok' ELSE 'sad' END)::mood AS m FROM range(5000) t(i)",
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 5000);
        assert_eq!(r.rows[0], vec![Value::Text("ok".into())]);
        assert_eq!(r.rows[1], vec![Value::Text("sad".into())]);

        // 2,048 rows of 64 KiB: 128 MiB in one chunk, sliced by the helper.
        let r = d
            .query("SELECT repeat('x', 65536) AS c FROM range(2048)", vec![])
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 2048);
        assert!(r
            .rows
            .iter()
            .all(|row| matches!(&row[0], Value::Text(t) if t.len() == 65536)));
        // One 12 MiB row goes alone.
        let r = d
            .query("SELECT repeat('y', 12 * 1024 * 1024) AS c", vec![])
            .await
            .unwrap();
        assert!(matches!(&r.rows[0][0], Value::Text(t) if t.len() == 12 * 1024 * 1024));
        // A 20 MiB row can't be sent; the connection goes on.
        let e = d
            .query("SELECT repeat('z', 20 * 1024 * 1024) AS c", vec![])
            .await
            .unwrap_err();
        assert_eq!(e.code, "RESULT_TOO_LARGE", "{e}");
        assert_usable(&*d, Duration::from_secs(1)).await;
    })
    .await;
}

/// Cells decode as the native driver decodes them whatever the session did
/// to `arrow_lossless_conversion` (Checkpoint H-1): with it reset, DuckDB
/// sends UHUGEINT as a bare `Decimal128(38, 0)` and BIT as its internal
/// bytes, which the Arrow field alone can't tell from DECIMAL(38, 0) and
/// BLOB. The helper sends the columns' kinds from DuckDB's logical types,
/// at the top level and nested, on every path that returns rows.
#[tokio::test]
async fn cells_decode_as_natively_whatever_the_session_s_arrow_settings() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        for lossy in [false, true] {
            let d = install.driver().await;
            cast::compare_with_cast(&*d, lossy).await;
        }

        let d = install.driver().await;
        d.execute("RESET arrow_lossless_conversion", vec![])
            .await
            .unwrap();
        let sql = "SELECT v::BIT AS b, [v::BIT] AS l, {'u': u::UHUGEINT} AS s, u::UHUGEINT AS u \
                   FROM (VALUES ('101', '340282366920938463463374607431768211455')) t(v, u)";
        let max = "340282366920938463463374607431768211455";
        let expected = vec![
            Value::Text("101".into()),
            Value::Array(vec![Value::Text("101".into())]),
            Value::Json(serde_json::json!({ "u": max })),
            Value::Decimal(max.into()),
        ];
        assert_eq!(
            d.query(sql, vec![]).await.unwrap().rows,
            vec![expected.clone()]
        );
        let mut stream = d.query_stream(sql.into(), vec![], CancellationToken::new());
        let batch = stream.next().await.unwrap().unwrap();
        assert_eq!(batch.rows, vec![expected]);
    })
    .await;
}

/// Past `max_query_rows`, `RESULT_TOO_LARGE`; the rest of the result is
/// cancelled and discarded, and the next call answers.
#[tokio::test]
async fn a_query_past_the_cap_fails_and_the_connection_goes_on() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;
        let cap = seaquel_engine::max_query_rows();
        let e = d
            .query(&format!("SELECT * FROM range({})", cap + 1), vec![])
            .await
            .unwrap_err();
        assert_eq!(e.code, "RESULT_TOO_LARGE", "{e}");
        assert_usable(&*d, Duration::from_millis(200)).await;
    })
    .await;
}

/// A stream nobody reads holds the main session (as natively); a read-only
/// call runs beside it on a clone.
#[tokio::test]
async fn a_read_only_call_answers_beside_a_paused_stream() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;
        let mut stream = d.query_stream(LONG_STREAM.to_string(), vec![], CancellationToken::new());
        stream.next().await.unwrap().unwrap();
        // Long enough for the helper to fill the credit window and wait.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let r = timeout(
            Duration::from_secs(2),
            d.query_read_only("SELECT 7 AS n", vec![], None),
        )
        .await
        .expect("the read-only call waited for the stream")
        .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(7)]]);
        // The stream still reads on.
        assert!(!stream.next().await.unwrap().unwrap().rows.is_empty());
        drop(stream);
        assert_usable(&*d, Duration::from_millis(200)).await;
    })
    .await;
}

/// Past 16 read-only calls at once, `TOO_MANY_REQUESTS`.
#[tokio::test]
async fn read_only_calls_are_capped() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;
        let mut calls: futures::stream::FuturesUnordered<_> = (0..17)
            .map(|_| d.query_read_only(LONG_SUM, vec![], None))
            .collect();
        let first = timeout(Duration::from_secs(5), calls.next())
            .await
            .expect("no call answered")
            .unwrap();
        assert_eq!(first.unwrap_err().code, "TOO_MANY_REQUESTS");
        drop(calls);
        assert_usable(&*d, Duration::from_millis(500)).await;
    })
    .await;
}

/// SQL larger than a frame is refused before it is sent; the connection
/// goes on.
#[tokio::test]
async fn a_request_too_large_for_a_frame_is_refused() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let d = install.driver().await;
        let sql = format!("SELECT '{}'", "x".repeat(17 * 1024 * 1024));
        let e = d.query(&sql, vec![]).await.unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT", "{e}");
        assert!(!e.message.contains("xxxx"), "the SQL in the message");
        assert_usable(&*d, Duration::from_millis(200)).await;
    })
    .await;
}

/// Two connections are two helpers.
#[cfg(unix)]
#[tokio::test]
async fn each_connection_has_its_own_helper() {
    let Some(bin) = built_helper() else { return };
    limit(async {
        let install = Install::current(&bin);
        let a = install.driver().await;
        let b = install.driver().await;
        assert_eq!(install.pids().len(), 2);
        a.execute("CREATE TABLE only_a (x INT)", vec![])
            .await
            .unwrap();
        assert!(b.query("SELECT * FROM only_a", vec![]).await.is_err());
        drop(a);
        drop(b);
        assert!(install.gone_within(Duration::from_secs(1)).await);
    })
    .await;
}
