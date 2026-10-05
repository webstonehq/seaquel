//! The DuckDB helper's install dialog against a real Core:
//! a DuckDB connection with no helper opens the
//! dialog, which downloads from `seaquel-http`'s `MockReleases` on
//! 127.0.0.1 into the temp data dir's `bin/duckdb/<version>/` and then
//! connects again. Nothing reaches GitHub.
//!
//! The last test installs the real helper (`SEAQUEL_TEST_DUCKDB_HELPER`,
//! gzipped and served), connects through it, kills it and reconnects; it is
//! skipped without the variable and fails under
//! `SEAQUEL_TEST_REQUIRE_ENGINES=1`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::KeyCode;
use seaquel_http::release_asset::target_triple;
use seaquel_http::release_asset::testing::{asset_path, digest, gzip, MockReleases, Route};

use crate::state::app::{Conn, Modal, Model, Panel};
use crate::state::install::Stage;
use crate::state::text;
use crate::testing::core::{connection, memory_store, project, Seed};
use crate::testing::harness::{Harness, HarnessOptions};

const VERSION: &str = seaquel_terminal::VERSION;

/// What `release.yml` names this platform's helper asset.
fn asset_name() -> String {
    let triple = target_triple(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    format!("seaquel-duckdb-{triple}{}.gz", std::env::consts::EXE_SUFFIX)
}

/// A "helper" that exits at once: it installs like the real one, and its
/// start fails (`ENGINE_NOT_INSTALLED`: no handshake).
fn fake_helper(extra: usize) -> Vec<u8> {
    let mut v = b"#!/bin/sh\nexit 0\n".to_vec();
    // Bytes gzip can't shrink, so the download takes several reads.
    let mut x: u32 = 0x9e37_79b9;
    v.extend((0..extra).map(|_| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    }));
    v
}

/// A data dir (under the target folder) with a DuckDB connection.
async fn duckdb_seed() -> (Seed, String) {
    let seed = Seed::in_target().await;
    let conn = seed
        .with(|core, ws| async move {
            let project_id = project(&core, &ws, "Warehouse").await;
            connection(
                &core,
                &ws,
                serde_json::json!({"projectId": project_id, "name": "warehouse",
                                   "type": "duckdb", "databaseName": ":memory:"}),
            )
            .await
        })
        .await;
    (seed, conn)
}

/// The TUI on `seed`, connecting `conn` at once, downloading from `mock`.
async fn open(seed: &Seed, conn: &str, mock: &MockReleases) -> Harness {
    Harness::open(HarnessOptions {
        connection: Some(conn),
        duckdb_releases: Some(mock.url().to_string()),
        ..HarnessOptions::new(seed.path(), memory_store())
    })
    .await
}

fn stage(m: &Model) -> Option<&Stage> {
    match &m.modal {
        Some(Modal::InstallDuckdb(d)) => Some(&d.stage),
        _ => None,
    }
}

fn version_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin").join("duckdb").join(VERSION)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dialog_downloads_into_the_data_dir_and_connects_again() {
    let (seed, conn) = duckdb_seed().await;
    let mock = MockReleases::start().await;
    let helper = fake_helper(300_000);
    let gz = gzip(&helper);
    mock.publish(VERSION, &asset_name(), gz.clone());
    let mut h = open(&seed, &conn, &mock).await;

    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    let Some(Stage::Ask(offer)) = stage(&h.model) else {
        unreachable!()
    };
    assert_eq!(offer.size, gz.len() as u64, "the asset's size, asked about");
    assert!(!offer.repair);
    // Nothing downloaded before Enter: only the metadata was read.
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 0);

    h.press(KeyCode::Enter);
    // The connect goes on by itself once installed. This "helper" exits at
    // once, so the new connect fails, and says so instead of offering
    // another download.
    h.until("the connect after the install", |m| {
        matches!(&m.modal, Some(Modal::Problem(p)) if p.title == text::PROBLEM_TITLE_NOT_INSTALLED)
    })
    .await;
    assert_eq!(mock.hits(&asset_path(VERSION, &asset_name())), 1);
    let file = version_dir(seed.path()).join(seaquel_core::DuckdbHelper::file_name());
    assert_eq!(std::fs::read(&file).unwrap(), helper);
    assert_eq!(
        h.session.core.duckdb_helper_status().unwrap(),
        seaquel_core::DuckdbHelperStatus::Installed { path: file }
    );
    let log: Vec<String> = h.model.log.last(10).map(|l| l.text.clone()).collect();
    assert!(log.iter().any(|l| l == text::INSTALLED_LINE), "{log:?}");
    // The command log names no path.
    let root = seed.path().to_string_lossy().into_owned();
    assert!(log.iter().all(|l| !l.contains(&root)), "{log:?}");
    h.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn esc_mid_download_leaves_nothing_behind() {
    let (seed, conn) = duckdb_seed().await;
    let mock = MockReleases::start().await;
    let gz = gzip(&fake_helper(1_000_000));
    let name = asset_name();
    mock.metadata(VERSION, &[(&name, gz.len() as u64, Some(&digest(&gz)))]);
    // The body stops after 128 KiB and the connection stays open.
    mock.route(
        &asset_path(VERSION, &name),
        Route::Body {
            status: 200,
            length: Some(gz.len() as u64),
            body: gz,
            stall_after: Some(128 * 1024),
            close_after: None,
            piece: 16 * 1024,
            gap: Duration::ZERO,
        },
    );
    let mut h = open(&seed, &conn, &mock).await;
    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    h.press(KeyCode::Enter);
    h.until(
        "part of the download",
        |m| matches!(stage(m), Some(Stage::Downloading { bytes, .. }) if *bytes > 0),
    )
    .await;
    h.press(KeyCode::Esc);
    assert!(
        matches!(h.model.modal, Some(Modal::Picker(_))),
        "{:?}",
        h.model.modal
    );
    // The dropped download takes its partial file with it.
    let dir = version_dir(seed.path());
    let mut left = Vec::new();
    for _ in 0..100 {
        left = std::fs::read_dir(&dir)
            .map(|d| d.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        if left.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(left.is_empty(), "left behind: {left:?}");
    assert_eq!(
        h.session.core.duckdb_helper_status().unwrap(),
        seaquel_core::DuckdbHelperStatus::Missing
    );
    h.close().await;
}

/// The release has no helper for this version yet: the dialog says so,
/// and Retry looks again (and finds it once it's published).
#[tokio::test(flavor = "multi_thread")]
async fn an_unpublished_release_says_so_and_retry_looks_again() {
    let (seed, conn) = duckdb_seed().await;
    let mock = MockReleases::start().await;
    let mut h = open(&seed, &conn, &mock).await;
    h.until("the refusal", |m| {
        matches!(stage(m), Some(Stage::Failed(_)))
    })
    .await;
    let Some(Stage::Failed(f)) = stage(&h.model) else {
        unreachable!()
    };
    assert_eq!(f.code, "RELEASE_NOT_FOUND");
    assert_eq!(f.title, text::install_failure("RELEASE_NOT_FOUND").0);
    mock.publish(VERSION, &asset_name(), gzip(&fake_helper(10)));
    h.keys("r");
    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    h.close().await;
}

/// `SEAQUEL_TEST_DUCKDB_HELPER`, or `None` to skip (a failure under
/// `SEAQUEL_TEST_REQUIRE_ENGINES=1`).
fn built_helper() -> Option<PathBuf> {
    match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
        Some(path) => Some(PathBuf::from(path)),
        None if std::env::var("SEAQUEL_TEST_REQUIRE_ENGINES").as_deref() == Ok("1") => {
            panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        None => {
            eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
            None
        }
    }
}

/// The pids of processes started from `path`.
#[cfg(unix)]
fn pids_of(path: &Path) -> Vec<i32> {
    let out = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// The real helper: installed by the dialog, connected through, killed
/// (the next call in panel 2 says the connection closed and offers to
/// connect again), and connected again.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_installed_helper_connects_and_a_stopped_one_offers_reconnect() {
    let Some(bin) = built_helper() else { return };
    let (seed, conn) = duckdb_seed().await;
    let mock = MockReleases::start().await;
    mock.publish(VERSION, &asset_name(), gzip(&std::fs::read(&bin).unwrap()));
    let mut h = open(&seed, &conn, &mock).await;
    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    h.press(KeyCode::Enter);
    let connected = |m: &Model| matches!(m.conn, Conn::Connected { .. });
    h.until_within(
        "connected through the helper",
        Duration::from_secs(90),
        connected,
    )
    .await;
    let file = version_dir(seed.path()).join(seaquel_core::DuckdbHelper::file_name());
    let pids = pids_of(&file);
    assert_eq!(pids.len(), 1, "one helper for the connection: {pids:?}");
    // SAFETY: a plain `kill(2)` of the helper this test started.
    assert_eq!(unsafe { libc::kill(pids[0], libc::SIGKILL) }, 0);

    // Panel 2's `r` reads the tables again, through the dead helper.
    h.keys("2");
    assert_eq!(h.model.focus, Panel::Tables);
    h.keys("r");
    h.until(
        "the reconnect offer",
        |m| matches!(&m.modal, Some(Modal::Problem(p)) if p.reconnect.is_some()),
    )
    .await;
    let Some(Modal::Problem(p)) = &h.model.modal else {
        unreachable!()
    };
    assert_eq!(p.code, "CONNECTION_CLOSED");
    assert!(matches!(h.model.conn, Conn::Closed { .. }));
    h.keys("r");
    h.until_within("connected again", Duration::from_secs(60), connected)
        .await;
    assert_eq!(pids_of(&file).len(), 1, "a new helper, the old one gone");
    h.close().await;
}

/// A copy of the built helper of the test's own under `dir` (a hard
/// link, else a copy), so its process checks see only this test's helpers.
#[cfg(unix)]
fn own_helper(bin: &Path, dir: &Path) -> PathBuf {
    let to = dir.join("real-seaquel-duckdb");
    if std::fs::hard_link(bin, &to).is_err() {
        std::fs::copy(bin, &to).unwrap();
    }
    to
}

/// Whether process `pid` still runs (a zombie doesn't count).
#[cfg(unix)]
fn running(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// Waits up to `within` until every proxy and helper the test started is
/// gone: by the pids the proxies logged (each proxy and its child), and
/// by the paths of the proxy and of the test's own helper.
#[cfg(unix)]
async fn all_gone(pid_log: &Path, paths: &[&Path], within: Duration) {
    let deadline = std::time::Instant::now() + within;
    loop {
        let log = std::fs::read_to_string(pid_log).unwrap_or_default();
        assert!(!log.is_empty(), "no proxy logged its pids");
        let mut left: Vec<String> = log
            .split_whitespace()
            .filter(|pid| running(pid))
            .map(str::to_string)
            .collect();
        left.extend(paths.iter().flat_map(|p| pids_of(p)).map(|p| p.to_string()));
        if left.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "still running: {left:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What the stand-in helper does.
#[cfg(unix)]
enum Proxy<'a> {
    /// On the client's `transaction` request, kill the helper and
    /// itself before the helper sees it.
    DieOnCommit,
    /// On `close`, pass it on, end its output, wait for the
    /// helper, then take `secs` more (a slow checkpoint) and leave `marker`.
    LingerOnClose { secs: u32, marker: &'a Path },
}

/// A stand-in helper: a Perl proxy in front of the real helper that passes
/// every frame through, and acts as `mode` says.
#[cfg(unix)]
fn proxy(real: &Path, mode: Proxy, pid_log: &Path) -> Vec<u8> {
    let quote = |p: &Path| p.to_string_lossy().replace('\'', "\\'");
    let real = quote(real);
    let pid_log = quote(pid_log);
    let on_frame = match mode {
        Proxy::DieOnCommit => {
            r#"if ($body =~ /^\x7b"type":"transaction"/) { kill 'KILL', $pid; waitpid($pid, 0); kill 'KILL', $$; }"#.to_string()
        }
        Proxy::LingerOnClose { secs, marker } => format!(
            r#"if ($body =~ /^\x7b"type":"close"/) {{ put($to, $frame); close $to; while (sysread($from, my $c, 65536)) {{}} close STDOUT; waitpid($pid, 0); sleep {secs}; open(my $m, '>', '{}'); close $m; exit 0; }}"#,
            quote(marker)
        ),
    };
    format!(
        r#"#!/usr/bin/perl
use strict;
use warnings;
use IPC::Open2;
$SIG{{PIPE}} = 'IGNORE';
my $pid = open2(my $from, my $to, '{real}', @ARGV);
open(my $pl, '>>', '{pid_log}'); print $pl "$$ $pid\n"; close $pl;
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
        {on_frame}
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
"#
    )
    .into_bytes()
}

/// Probe F1 against a real Core: the helper dies with a two-change commit
/// in flight (atomic, so a transaction). The TUI marks the connection
/// closed, offers to connect again, keeps the queue marked "may be partly
/// applied", and committing it again asks first.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_helper_that_dies_mid_commit_loses_the_connection_and_marks_the_queue() {
    use super::browse_tests::{loaded, open_table};
    use crate::state::app::Load;
    use crate::state::pending::Plan;

    let Some(bin) = built_helper() else { return };
    let (seed, conn) = duckdb_seed().await;
    let real = own_helper(&bin, seed.path());
    let pid_log = seed.path().join("proxy-pids");
    let mock = MockReleases::start().await;
    mock.publish(
        VERSION,
        &asset_name(),
        gzip(&proxy(&real, Proxy::DieOnCommit, &pid_log)),
    );
    let mut h = open(&seed, &conn, &mock).await;
    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    h.press(KeyCode::Enter);
    let connected = |m: &Model| matches!(m.conn, Conn::Connected { .. });
    h.until_within(
        "connected through the proxy",
        Duration::from_secs(90),
        connected,
    )
    .await;
    let core_id = h.model.conn.core_id().unwrap().to_string();
    for sql in [
        "CREATE TABLE invoices (id INTEGER PRIMARY KEY, customer VARCHAR NOT NULL)",
        "INSERT INTO invoices VALUES (1, 'c1'), (2, 'c2'), (3, 'c3')",
    ] {
        h.session
            .ws
            .execute(&h.session.core, &core_id, sql, Vec::new())
            .await
            .unwrap();
    }
    h.keys("2");
    h.keys("r");
    h.until("listed", |m| {
        m.schema_load == Load::Loaded && m.schema.iter().any(|t| t.name == "invoices")
    })
    .await;
    open_table(&mut h, "invoices");
    h.until("the first page", loaded).await;
    for row in [0, 1] {
        h.model.browse.row = row;
        h.model.browse.col = 1;
        h.keys("eX");
        h.press(KeyCode::Enter);
    }
    h.until("planned", |m| {
        m.queue.entries().len() == 2
            && m.queue
                .entries()
                .iter()
                .all(|e| matches!(e.plan, Plan::Planned(_)))
    })
    .await;
    h.keys("c");
    h.press(KeyCode::Enter);
    // Core's `ConnectionClosed`
    // can open the offer before the apply's own answer ends
    // the commit, so both are waited for.
    h.until_within(
        "the reconnect offer and the commit's end",
        Duration::from_secs(20),
        |m| {
            matches!(&m.modal, Some(Modal::Problem(p)) if p.reconnect.is_some())
                && m.committing.is_none()
        },
    )
    .await;
    let Some(Modal::Problem(p)) = &h.model.modal else {
        unreachable!()
    };
    assert_eq!(p.code, "CONNECTION_CLOSED");
    assert!(matches!(h.model.conn, Conn::Closed { .. }));
    assert!(h.model.committing.is_none());
    assert_eq!(h.model.queue.entries().len(), 2, "the queue is kept");
    assert!(h.model.queue.interrupted());
    let log: Vec<String> = h.model.log.last(10).map(|l| l.text.clone()).collect();
    assert!(log.iter().any(|l| l == text::COMMIT_INTERRUPTED), "{log:?}");

    // Connected again (a new proxy and helper), `c` asks first.
    h.keys("r");
    h.until_within("connected again", Duration::from_secs(60), connected)
        .await;
    h.keys("4");
    h.keys("c");
    assert_eq!(h.model.modal, Some(Modal::ConfirmRecommit));
    h.press(KeyCode::Esc);
    h.close().await;
    let file = version_dir(seed.path()).join(seaquel_core::DuckdbHelper::file_name());
    all_gone(&pid_log, &[&real, &file], Duration::from_secs(5)).await;
}

/// Quitting while the helper is still closing its database (a
/// checkpoint that takes seconds) stays within the TUI's exit bound
/// (`SETTLE_WITHIN`): the driver stops waiting after 2 s and lets the
/// helper go, and the helper finishes and exits on its own.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn quitting_leaves_a_closing_helper_to_finish_within_the_exit_bound() {
    let Some(bin) = built_helper() else { return };
    let (seed, conn) = duckdb_seed().await;
    let marker = seed.path().join("closed-on-its-own");
    let real = own_helper(&bin, seed.path());
    let pid_log = seed.path().join("proxy-pids");
    let mock = MockReleases::start().await;
    mock.publish(
        VERSION,
        &asset_name(),
        gzip(&proxy(
            &real,
            Proxy::LingerOnClose {
                secs: 4,
                marker: &marker,
            },
            &pid_log,
        )),
    );
    let mut h = open(&seed, &conn, &mock).await;
    h.until("the question", |m| matches!(stage(m), Some(Stage::Ask(_))))
        .await;
    h.press(KeyCode::Enter);
    h.until_within(
        "connected through the proxy",
        Duration::from_secs(90),
        |m: &Model| matches!(m.conn, Conn::Connected { .. }),
    )
    .await;
    let file = version_dir(seed.path()).join(seaquel_core::DuckdbHelper::file_name());
    let started = std::time::Instant::now();
    tokio::time::timeout(super::SETTLE_WITHIN, h.close())
        .await
        .expect("closing took past the exit bound");
    let took = started.elapsed();
    assert!(took < Duration::from_secs(4), "{took:?}");
    assert!(!marker.exists(), "the close didn't wait for the checkpoint");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !pids_of(&file).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the helper didn't exit on its own"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(marker.exists(), "the helper was killed, not left to finish");
    all_gone(&pid_log, &[&real, &file], Duration::from_secs(1)).await;
}
