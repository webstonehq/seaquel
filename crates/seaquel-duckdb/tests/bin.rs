//! The `seaquel-duckdb` binary as its client starts it: stdin and stdout
//! piped, frames both ways (`seaquel-engine-duckdb`'s `wire.rs`, written by
//! hand here since the wire is the crate's own).
//!
//! Every child runs under a [`Helper`] guard that kills it on drop, and
//! every wait has a limit, so a failing test leaves no helper behind.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

include!("../../seaquel-terminal/src/package_version.rs");

const BIN: &str = env!("CARGO_BIN_EXE_seaquel-duckdb");

/// How long any one wait in these tests may take.
const LIMIT: Duration = Duration::from_secs(10);

/// The desktop app's version, which the helper reports.
fn app_version() -> String {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/Cargo.toml");
    let text = std::fs::read_to_string(manifest).unwrap();
    package_version(&text).unwrap().to_string()
}

/// A running helper, killed when dropped.
struct Helper {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Option<(u8, u32, Vec<u8>)>>,
    stderr: mpsc::Receiver<Vec<u8>>,
}

impl Helper {
    fn start() -> Self {
        let mut child = Command::new(BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        thread::spawn(move || loop {
            let frame = read_frame(&mut stdout);
            let end = frame.is_none();
            if tx.send(frame).is_err() || end {
                return;
            }
        });
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (tx, stderr) = mpsc::channel();
        thread::spawn(move || {
            let mut all = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut all);
            let _ = tx.send(all);
        });
        Helper {
            child,
            stdin,
            frames,
            stderr,
        }
    }

    fn send(&mut self, call: u32, json: &str) {
        let stdin = self.stdin.as_mut().expect("stdin closed");
        let len = (5 + json.len()) as u32;
        let mut frame = len.to_le_bytes().to_vec();
        frame.push(0);
        frame.extend(call.to_le_bytes());
        frame.extend(json.as_bytes());
        stdin.write_all(&frame).unwrap();
        stdin.flush().unwrap();
    }

    /// The next frame: kind, call and payload.
    fn next(&self) -> (u8, u32, Vec<u8>) {
        self.frames
            .recv_timeout(LIMIT)
            .expect("no frame in time")
            .expect("stdout ended")
    }

    /// The next frame as a control message's JSON text.
    fn control(&self, call: u32) -> String {
        let (kind, got, payload) = self.next();
        assert_eq!((kind, got), (0, call));
        String::from_utf8(payload).unwrap()
    }

    fn hello(&mut self) -> String {
        self.send(
            1,
            &format!(
                r#"{{"type":"hello","protocol":2,"version":"{}"}}"#,
                app_version()
            ),
        );
        self.control(1)
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    fn wait(&mut self) -> ExitStatus {
        wait_within(&mut self.child, LIMIT)
    }

    /// Everything the helper wrote to stderr, once it has exited.
    fn stderr(&self) -> Vec<u8> {
        self.stderr.recv_timeout(LIMIT).expect("stderr didn't end")
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_frame(input: &mut impl Read) -> Option<(u8, u32, Vec<u8>)> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len).ok()?;
    let len = u32::from_le_bytes(len) as usize;
    let mut head = [0u8; 5];
    input.read_exact(&mut head).ok()?;
    let mut payload = vec![0u8; len - 5];
    input.read_exact(&mut payload).ok()?;
    Some((
        head[0],
        u32::from_le_bytes([head[1], head[2], head[3], head[4]]),
        payload,
    ))
}

/// Waits for `child` to exit; kills it and fails past `limit`.
fn wait_within(child: &mut Child, limit: Duration) -> ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the helper didn't exit within {limit:?}");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Runs the binary to its end with nothing on stdin: its exit status,
/// stdout and stderr.
fn run(args: &[&str]) -> (ExitStatus, String, String) {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_within(&mut child, LIMIT);
    let mut out = String::new();
    let mut err = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    (status, out, err)
}

#[test]
fn version_prints_the_app_version() {
    let (status, out, err) = run(&["--version"]);
    assert!(status.success(), "{status:?} {err}");
    assert_eq!(out, format!("seaquel-duckdb {}\n", app_version()));
    assert!(!app_version().starts_with("0.1."));
}

#[test]
fn an_unknown_argument_is_refused_on_stderr() {
    let (status, out, err) = run(&["--frobnicate"]);
    assert_eq!(status.code(), Some(2));
    assert_eq!(out, "");
    assert!(err.contains("seaquel-duckdb"), "{err}");
}

/// Started by a client: nothing on stdout before `hello`, then the
/// handshake, a query, and exit 0 with nothing on stderr once stdin ends.
#[test]
fn speaks_frames_on_its_pipes_and_nothing_else() {
    let mut h = Helper::start();
    thread::sleep(Duration::from_millis(300));
    assert!(
        h.frames.try_recv().is_err(),
        "the helper wrote before hello"
    );

    let hello = h.hello();
    assert!(
        hello.starts_with(&format!(
            r#"{{"type":"helloOk","protocol":2,"version":"{}","duckdb":"v1."#,
            app_version()
        )),
        "{hello}"
    );
    h.send(2, r#"{"type":"open","path":":memory:"}"#);
    assert_eq!(h.control(2), r#"{"type":"opened"}"#);
    h.send(3, r#"{"type":"query","sql":"SELECT 42 AS n","params":[]}"#);
    let kinds: Vec<u8> = (0..3)
        .map(|_| h.next())
        .map(|(k, c, _)| {
            assert_eq!(c, 3);
            k
        })
        .collect();
    assert_eq!(kinds, vec![1, 2, 0], "schema, batch, done");

    h.close_stdin();
    let status = h.wait();
    assert_eq!(status.code(), Some(0));
    assert!(matches!(h.frames.recv_timeout(LIMIT), Ok(None)));
    let stderr = h.stderr();
    assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
}

/// A wrong protocol: one refusal frame, exit 3.
#[test]
fn refuses_another_protocol_with_exit_3() {
    let mut h = Helper::start();
    h.send(1, r#"{"type":"hello","protocol":99,"version":"x"}"#);
    let refusal = h.control(1);
    assert!(refusal.contains("HELPER_PROTOCOL"), "{refusal}");
    assert_eq!(h.wait().code(), Some(3));
    assert!(matches!(h.frames.recv_timeout(LIMIT), Ok(None)));
}

/// stdin ending while DuckDB runs a long query: the helper exits at once.
#[test]
fn exits_when_stdin_ends_mid_query() {
    let mut h = Helper::start();
    h.hello();
    h.send(
        2,
        r#"{"type":"open","path":":memory:","duckdbConfig":{"threads":"1"}}"#,
    );
    assert_eq!(h.control(2), r#"{"type":"opened"}"#);
    h.send(
        3,
        r#"{"type":"query","sql":"SELECT max(md5(i::VARCHAR)) FROM range(3000000000) t(i)","params":[]}"#,
    );
    thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    h.close_stdin();
    let status = wait_within(&mut h.child, Duration::from_secs(2));
    assert_eq!(status.code(), Some(0));
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// Something in the process printing to fd 1 (DuckDB's progress bar
/// writes there) doesn't reach the client's frames: fd 1 is stderr once
/// the helper runs, and the bar shows up there. The helper turns the bar
/// off when it opens; a statement turning it back on, with
/// `progress_bar_time = 0`, makes DuckDB draw it at once.
#[test]
fn fd_1_is_not_the_frame_channel() {
    let mut h = Helper::start();
    h.hello();
    h.send(
        2,
        r#"{"type":"open","path":":memory:","duckdbConfig":{"threads":"1"}}"#,
    );
    assert_eq!(h.control(2), r#"{"type":"opened"}"#);
    h.send(
        3,
        r#"{"type":"execute","sql":"SET enable_progress_bar = true; SET enable_progress_bar_print = true; SET progress_bar_time = 0","params":[]}"#,
    );
    assert!(h.control(3).starts_with(r#"{"type":"executed""#));
    h.send(
        4,
        r#"{"type":"query","sql":"SELECT count(DISTINCT md5(i::VARCHAR)) AS n FROM range(3000000) t(i)","params":[]}"#,
    );
    // Schema, batch and done, each a well-formed frame of call 4.
    let kinds: Vec<u8> = (0..3)
        .map(|_| {
            let (kind, call, _) = h.next();
            assert_eq!(call, 4);
            kind
        })
        .collect();
    assert_eq!(kinds, vec![1, 2, 0]);
    h.close_stdin();
    assert_eq!(h.wait().code(), Some(0));
    let stderr = String::from_utf8_lossy(&h.stderr()).into_owned();
    assert!(
        stderr.contains('%'),
        "no progress bar on stderr: {stderr:?}"
    );
}

/// The reviewer's reproduction, through the binary: a stream with all the
/// credit it wants that nobody reads, a second `open` behind it, then
/// stdin closed. The helper exits within a bound.
#[test]
fn exits_when_stdin_ends_while_its_output_is_unread() {
    let child = Command::new(BIN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Killed on drop, whatever happens below.
    let mut h = Helper {
        child,
        stdin: None,
        frames: mpsc::channel().1,
        stderr: mpsc::channel().1,
    };
    h.stdin = h.child.stdin.take();
    let mut stdout = h.child.stdout.take().unwrap();
    h.send(
        1,
        &format!(
            r#"{{"type":"hello","protocol":2,"version":"{}"}}"#,
            app_version()
        ),
    );
    read_frame(&mut stdout).unwrap();
    h.send(2, r#"{"type":"open","path":":memory:"}"#);
    read_frame(&mut stdout).unwrap();
    // From here on nobody reads: frames pile up in the pipe.
    h.send(
        3,
        r#"{"type":"stream","sql":"SELECT i, md5(i::VARCHAR) AS h FROM range(10000000) t(i)","params":[]}"#,
    );
    h.send(3, r#"{"type":"credit","frames":4294967295}"#);
    thread::sleep(Duration::from_millis(500));
    h.send(4, r#"{"type":"open","path":":memory:"}"#);
    thread::sleep(Duration::from_millis(100));
    h.close_stdin();
    let status = wait_within(&mut h.child, Duration::from_secs(4));
    assert_eq!(status.code(), Some(0));
    drop(stdout);
}

/// Started with a terminal on stdin (by hand), it says so on stderr and
/// exits 2. `script` gives it a terminal.
#[cfg(unix)]
#[test]
fn refuses_a_terminal() {
    let mut cmd = Command::new("script");
    if cfg!(target_os = "macos") {
        cmd.args(["-q", "/dev/null", BIN]);
    } else {
        cmd.args(["-qec", BIN, "/dev/null"]);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("script");
    let status = wait_within(&mut child, LIMIT);
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert!(
        out.contains("seaquel-duckdb is started by Seaquel; it isn't run by hand"),
        "{status:?}: {out:?}"
    );
    assert_eq!(status.code(), Some(2), "{out:?}");
}
