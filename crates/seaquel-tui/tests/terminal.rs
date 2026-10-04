//! The terminal is always restored (Decision 18): the built binary runs on
//! a pty under `script`, is ended by SIGTERM, SIGHUP or a panic (on the
//! main thread and in a spawned task), and afterwards the pty is back in
//! cooked mode (`stty -a`, run by the same shell), the alternate screen was
//! left, the cursor shown, mouse capture and the kitty keyboard flags
//! turned off, in that order after they were turned on.
//!
//! The test answers the kitty protocol query itself (as kitty would), so
//! the flags are pushed and their pop can be checked. Everything runs in a
//! temp data dir, seeded with a current `seaquel.db` (the TUI opens it as a
//! second process and refuses a data dir without one).

#![cfg(unix)]

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(20);

/// crossterm's query for the kitty flags and the primary device attributes.
const QUERY: &[u8] = b"\x1b[?u\x1b[c";
/// A kitty-capable terminal's answers: flags 0, then DA1.
const ANSWER: &[u8] = b"\x1b[?0u\x1b[?62c";

/// One `script` run. Dropped, it kills and reaps everything it started
/// (`script`, its shell, the TUI), so a failing or timed-out test leaves no
/// TUI behind: `script` gives its child a session of its own on the pty, so
/// no process group reaches it from here, and the tree is walked instead.
struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Arc<Mutex<Vec<u8>>>,
    data: Option<tempfile::TempDir>,
}

impl Drop for Session {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            // Ended on its own: closing the pty hung its session up.
            return;
        }
        // The deepest first, so nothing is respawned or reparented
        // half-way; SIGKILL, since a stuck TUI may not answer SIGTERM.
        let mut tree = descendants(self.child.id());
        tree.reverse();
        for pid in &tree {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every process below `pid`, parents before children (`pgrep -P`).
fn descendants(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut queue = vec![pid];
    while let Some(parent) = queue.pop() {
        let Ok(found) = Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .output()
        else {
            continue;
        };
        for child in String::from_utf8_lossy(&found.stdout)
            .split_whitespace()
            .filter_map(|p| p.parse::<u32>().ok())
        {
            out.push(child);
            queue.push(child);
        }
    }
    out
}

/// A current `seaquel.db`, made as the app makes it.
fn seed(dir: &std::path::Path) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let core = seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default()).build();
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(dir))
            .await
            .unwrap();
        ws.close().await;
    });
}

impl Session {
    /// Runs `shell` under `script`, with `$TUI` the binary, on a seeded
    /// data dir.
    fn start(shell: &str, envs: &[(&str, &str)]) -> Session {
        let data = tempfile::tempdir().unwrap();
        seed(data.path());
        Session::start_in(data, shell, envs)
    }

    /// Runs `shell` under `script` on `data` as it is.
    fn start_in(data: tempfile::TempDir, shell: &str, envs: &[(&str, &str)]) -> Session {
        let inner = r#"sh -c "$SEAQUEL_TEST_SH""#;
        let mut cmd = Command::new("script");
        if cfg!(target_os = "macos") {
            cmd.args(["-q", "/dev/null", "sh", "-c", inner]);
        } else {
            cmd.args(["-q", "-e", "-c", inner, "/dev/null"]);
        }
        // Never the real keychain, known_hosts or home: an empty secrets
        // file, an empty known_hosts and HOME, all in the temp dir.
        let secrets = data.path().join("test-secrets.json");
        let known_hosts = data.path().join("test-known_hosts");
        std::fs::write(&secrets, "{}").unwrap();
        std::fs::write(&known_hosts, "").unwrap();
        cmd.env("SEAQUEL_TEST_SH", shell)
            .env("TUI", env!("CARGO_BIN_EXE_seaquel-tui"))
            .env("SEAQUEL_DATA_DIR", data.path())
            .env("SEAQUEL_TUI_TEST_SECRETS", &secrets)
            .env("SEAQUEL_TUI_TEST_KNOWN_HOSTS", &known_hosts)
            .env("HOME", data.path())
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("run script");
        let stdin = child.stdin.take();
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut stdout = child.stdout.take().unwrap();
        let sink = out.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        });
        Session {
            child,
            stdin,
            out,
            data: Some(data),
        }
    }

    fn output(&self) -> Vec<u8> {
        self.out.lock().unwrap().clone()
    }

    fn wait_for(&self, pattern: &[u8]) {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if find(&self.output(), pattern).is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "never saw {:?} in:\n{}",
            String::from_utf8_lossy(pattern),
            String::from_utf8_lossy(&self.output())
        );
    }

    fn type_bytes(&mut self, bytes: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes).unwrap();
        stdin.flush().unwrap();
    }

    /// The `PID:` line the shell printed for the background TUI.
    fn tui_pid(&self) -> String {
        let out = String::from_utf8_lossy(&self.output()).to_string();
        let at = out.find("PID:").expect("the shell printed the pid") + 4;
        out[at..].chars().take_while(char::is_ascii_digit).collect()
    }

    fn finish(mut self) -> (Vec<u8>, tempfile::TempDir) {
        self.wait_for(b"DONE");
        drop(self.stdin.take());
        let start = Instant::now();
        while self.child.try_wait().unwrap().is_none() {
            assert!(start.elapsed() < WAIT, "script didn't end");
            std::thread::sleep(Duration::from_millis(20));
        }
        (self.output(), self.data.take().unwrap())
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Turned on, then turned off after it.
fn on_then_off(out: &[u8], on: &[u8], off: &[u8], what: &str) {
    let on_at = rfind(out, on).unwrap_or_else(|| panic!("{what} was never turned on"));
    let off_at = rfind(out, off).unwrap_or_else(|| panic!("{what} was never turned off"));
    assert!(off_at > on_at, "{what} was left on");
}

/// Checks everything the TUI turned on was turned off, and that the pty
/// is cooked again.
fn assert_restored(out: &[u8]) {
    on_then_off(out, b"\x1b[?1049h", b"\x1b[?1049l", "the alternate screen");
    on_then_off(out, b"\x1b[?1000h", b"\x1b[?1000l", "mouse capture");
    on_then_off(out, b"\x1b[?2004h", b"\x1b[?2004l", "bracketed paste");
    on_then_off(out, b"\x1b[>1u", b"\x1b[<1u", "the kitty keyboard flags");
    on_then_off(out, b"\x1b[?25l", b"\x1b[?25h", "the hidden cursor");
    let text = String::from_utf8_lossy(out);
    let stty = &text[text.rfind("EXIT:").expect("the shell went on")..];
    let words: Vec<&str> = stty.split_whitespace().collect();
    for flag in ["icanon", "echo", "isig"] {
        assert!(words.contains(&flag), "{flag} is off:\n{stty}");
        assert!(
            !words.contains(&format!("-{flag}").as_str()),
            "{flag} is off:\n{stty}"
        );
    }
}

fn exit_code(out: &[u8]) -> i32 {
    let text = String::from_utf8_lossy(out);
    let at = text.rfind("EXIT:").unwrap() + 5;
    text[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

#[cfg(unix)]
fn assert_private_log(data: &tempfile::TempDir) {
    use std::os::unix::fs::PermissionsExt;
    let log = data.path().join("logs/tui.log");
    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

// A background job's stdin would be /dev/null, so it gets the pty through
// fd 3 (not by reopening /dev/tty, which macOS's kqueue can't poll).
const BACKGROUND: &str = r#"stty rows 30 cols 100
exec 3<&0
"$TUI" <&3 &
pid=$!
echo "PID:$pid"
wait $pid
echo "EXIT:$?"
stty -a
echo DONE"#;

fn signal_case(signal: &str) {
    let mut s = Session::start(BACKGROUND, &[]);
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    s.wait_for(b"Command Log");
    let pid = s.tui_pid();
    let status = Command::new("kill").args([signal, &pid]).status().unwrap();
    assert!(status.success());
    let (out, data) = s.finish();
    assert_restored(&out);
    assert_eq!(exit_code(&out), 0, "ended by {signal} without restoring");
    assert_private_log(&data);
}

#[test]
fn sigterm_restores_the_terminal() {
    signal_case("-TERM");
}

#[test]
fn sighup_restores_the_terminal() {
    signal_case("-HUP");
}

const FOREGROUND: &str = r#"stty rows 30 cols 100
"$TUI"
echo "EXIT:$?"
stty -a
echo DONE"#;

fn panic_case(which: &str) {
    let mut s = Session::start(FOREGROUND, &[("SEAQUEL_TUI_TEST_PANIC", which)]);
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    let (out, data) = s.finish();
    assert_restored(&out);
    assert_ne!(exit_code(&out), 0);
    // The panic is printed after the terminal is back.
    let left = rfind(&out, b"\x1b[?1049l").unwrap();
    let panicked = find(&out, b"panicked at").expect("the panic is printed");
    assert!(panicked > left, "printed before the terminal was restored");
    let crashed = find(&out, b"seaquel-tui crashed; the log is at").expect("names the log");
    assert!(crashed > left);
    assert_private_log(&data);
}

#[test]
fn a_panic_on_the_main_thread_restores_the_terminal() {
    panic_case("main");
}

#[test]
fn a_panic_in_a_spawned_task_restores_the_terminal() {
    panic_case("task");
}

/// A data dir the app never opened: refused before the screen is taken,
/// worded on stderr, non-zero (Decision 3).
#[test]
fn a_refused_data_dir_is_said_without_taking_the_screen() {
    let data = tempfile::tempdir().unwrap();
    let s = Session::start_in(data, FOREGROUND, &[]);
    let (out, data) = s.finish();
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("seaquel-tui: "), "{text}");
    assert!(text.contains("Open the Seaquel app once"), "{text}");
    assert_ne!(exit_code(&out), 0);
    assert!(
        find(&out, b"\x1b[?1049h").is_none(),
        "the screen was never taken"
    );
    assert!(
        !data.path().join("seaquel.db").exists(),
        "nothing was created"
    );
}

#[test]
fn stdin_must_be_the_terminal_too() {
    let s = Session::start(
        r#"stty rows 30 cols 100
"$TUI" </dev/null
echo "EXIT:$?"
stty -a
echo DONE"#,
        &[],
    );
    let (out, _) = s.finish();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("seaquel-tui needs a terminal on stdin and stdout"),
        "{text}"
    );
    assert_ne!(exit_code(&out), 0);
    assert!(
        find(&out, b"\x1b[?1049h").is_none(),
        "the screen was never taken"
    );
}

/// `kill -TSTP` (a job-control stop from outside) gives the terminal back
/// before the process stops, and SIGCONT takes it again and repaints.
#[test]
fn sigtstp_restores_and_sigcont_repaints() {
    let mut s = Session::start(BACKGROUND, &[]);
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    s.wait_for(b"Command Log");
    let pid = s.tui_pid();
    let entered = count(&s.output(), b"\x1b[?1049h");
    let kill = |signal: &str| {
        let status = Command::new("kill").args([signal, &pid]).status().unwrap();
        assert!(status.success());
    };
    kill("-TSTP");
    // Stopped, with the screen given back first.
    wait_until(|| process_state(&pid).starts_with('T'));
    let out = s.output();
    on_then_off(&out, b"\x1b[?1049h", b"\x1b[?1049l", "the alternate screen");
    on_then_off(&out, b"\x1b[>1u", b"\x1b[<1u", "the kitty keyboard flags");
    kill("-CONT");
    wait_until(|| count(&s.output(), b"\x1b[?1049h") > entered);
    let repainted = rfind(&s.output(), b"\x1b[?1049h").unwrap();
    wait_until(|| {
        let out = s.output();
        find(&out[repainted..], b"Command Log").is_some()
    });
    kill("-TERM");
    let (out, _) = s.finish();
    assert_restored(&out);
    assert_eq!(exit_code(&out), 0);
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

fn process_state(pid: &str) -> String {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < WAIT, "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` is a live process (a zombie counts as gone).
fn alive(pid: &str) -> bool {
    let state = process_state(pid);
    !state.is_empty() && !state.starts_with('Z')
}

/// A session dropped mid-test (a failed assertion, a `WAIT` running out)
/// kills and reaps everything it started: `script`, its shell and the TUI.
#[test]
fn a_dropped_session_leaves_no_tui_running() {
    let mut s = Session::start(BACKGROUND, &[]);
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    s.wait_for(b"Command Log");
    let tui = s.tui_pid();
    let script = s.child.id().to_string();
    assert!(alive(&tui) && alive(&script));
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _session = s;
        panic!("a test failing while its TUI runs");
    }));
    assert!(failed.is_err());
    assert!(!alive(&script), "script is still running");
    wait_until(|| !alive(&tui));
}

/// Ctrl+O (Decision 14, spike S5): the editor gets a cooked terminal and
/// every key, the TUI takes the terminal back and draws again (no
/// `clear()`), and an editor that can't start leaves the TUI drawn and
/// saying why. The terminal is restored at the end.
fn editor_case(editor: &str) -> (Vec<u8>, usize) {
    let mut s = Session::start(FOREGROUND, &[("EDITOR", editor), ("VISUAL", "")]);
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    s.wait_for(b"Command Log");
    // The picker opens first (no projects here): Esc closes it.
    s.type_bytes(b"\x1b");
    std::thread::sleep(Duration::from_millis(300));
    s.type_bytes(b"Q");
    s.wait_for(b"untitled-1");
    s.type_bytes(b"SELECT 1");
    s.wait_for(b"SELECT");
    let before = s.output().len();
    // Ctrl+O; entering again asks the kitty query again.
    s.type_bytes(b"\x0f");
    let start = Instant::now();
    while count(&s.output()[before..], QUERY) == 0 {
        assert!(start.elapsed() < WAIT, "never entered again");
        std::thread::sleep(Duration::from_millis(20));
    }
    s.type_bytes(ANSWER);
    // Wait for the screen to be drawn again, then quit from Normal mode.
    let drawn = |s: &Session| {
        let out = s.output();
        find(&out[before..], b"Results").is_some()
    };
    let start = Instant::now();
    while !drawn(&s) {
        assert!(start.elapsed() < WAIT, "never drawn again");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    s.type_bytes(b"\r");
    std::thread::sleep(Duration::from_millis(100));
    s.type_bytes(b"\x1b");
    std::thread::sleep(Duration::from_millis(300));
    s.type_bytes(b"q");
    let (out, _) = s.finish();
    (out, before)
}

#[test]
fn the_external_editor_gets_the_terminal_and_gives_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let editor = dir.path().join("editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nstty -a | grep -q -- ' icanon' && echo EDITOR_SAW_COOKED\nprintf ' -- edited' >> \"$1\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let (out, before) = editor_case(editor.to_str().unwrap());
    let after = &out[before..];
    assert!(
        find(after, b"EDITOR_SAW_COOKED").is_some(),
        "{}",
        String::from_utf8_lossy(after)
    );
    assert!(find(after, b"-- edited").is_some(), "the text came back");
    assert!(
        find(after, b"\x1b[?1049l").unwrap() < find(after, b"EDITOR_SAW_COOKED").unwrap(),
        "the screen was given back before the editor ran"
    );
    assert!(
        rfind(after, b"\x1b[?1049h").unwrap() > find(after, b"EDITOR_SAW_COOKED").unwrap(),
        "and taken again after"
    );
    // Never `Terminal::clear()`: no cursor-position query after the editor.
    assert!(find(after, b"\x1b[6n").is_none(), "no clear()");
    assert_restored(&out);
    assert_eq!(exit_code(&out), 0);
}

#[test]
fn an_editor_that_cannot_start_leaves_the_tui_drawn() {
    let (out, before) = editor_case("/nonexistent/editor");
    let after = &out[before..];
    assert!(
        find(after, b"didn't run").is_some(),
        "{}",
        String::from_utf8_lossy(after)
    );
    assert_restored(&out);
    assert_eq!(exit_code(&out), 0);
}

/// Review I4: Ctrl+Z inside `$EDITOR` (here the editor stops itself with
/// SIGTSTP, as vim does on Ctrl+Z). The editor runs in its own foreground
/// process group: the TUI takes the terminal back and stops too, so the
/// shell sees one stopped job; continued, it hands the terminal back to the
/// editor and continues it, and the edit comes back.
#[test]
fn ctrl_z_in_the_external_editor_stops_and_resumes_the_job() {
    let dir = tempfile::tempdir().unwrap();
    let editor = dir.path().join("editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nkill -TSTP $$\nprintf ' -- resumed' >> \"$1\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut s = Session::start(
        BACKGROUND,
        &[("EDITOR", editor.to_str().unwrap()), ("VISUAL", "")],
    );
    s.wait_for(QUERY);
    s.type_bytes(ANSWER);
    s.wait_for(b"Command Log");
    let pid = s.tui_pid();
    s.type_bytes(b"\x1b");
    std::thread::sleep(Duration::from_millis(300));
    s.type_bytes(b"Q");
    s.wait_for(b"untitled-1");
    s.type_bytes(b"SELECT 1");
    s.wait_for(b"SELECT");
    let before = s.output().len();
    s.type_bytes(b"\x0f");
    // The editor stopped itself; the TUI stops with it.
    wait_until(|| process_state(&pid).starts_with('T'));
    let status = Command::new("kill").args(["-CONT", &pid]).status().unwrap();
    assert!(status.success());
    let start = Instant::now();
    while count(&s.output()[before..], QUERY) == 0 {
        assert!(start.elapsed() < WAIT, "never entered again");
        std::thread::sleep(Duration::from_millis(20));
    }
    s.type_bytes(ANSWER);
    let start = Instant::now();
    while find(&s.output()[before..], b"resumed").is_none() {
        assert!(
            start.elapsed() < WAIT,
            "the edit never came back:\n{}",
            String::from_utf8_lossy(&s.output()[before..])
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    s.type_bytes(b"\x1b");
    std::thread::sleep(Duration::from_millis(300));
    s.type_bytes(b"q");
    let (out, _) = s.finish();
    assert_restored(&out);
    assert_eq!(exit_code(&out), 0);
}
