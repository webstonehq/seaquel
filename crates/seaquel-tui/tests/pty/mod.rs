//! A pty the test holds both ends of: the TUI runs on its slave end as a
//! session leader with the pty as its controlling terminal (as a terminal
//! emulator starts a shell), stdin, stdout and stderr all on it; the test
//! reads and writes the master end, and can close it. Every wait has a
//! hard limit, and a dropped `Pty` kills the TUI.

#![allow(dead_code)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const WAIT: Duration = Duration::from_secs(20);
/// How soon the TUI must be gone once the terminal is.
pub const EXIT_WITHIN: Duration = Duration::from_secs(10);

/// crossterm's query for the kitty flags and the primary device attributes,
/// and a kitty-capable terminal's answers.
pub const QUERY: &[u8] = b"\x1b[?u\x1b[c";
pub const ANSWER: &[u8] = b"\x1b[?0u\x1b[?62c";

/// The TUI on a pty whose master end this test holds (non-blocking, read by
/// a thread; closing it closes the only master descriptor).
pub struct Pty {
    master: Arc<Mutex<Option<OwnedFd>>>,
    out: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    child: Child,
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            // Close the master before waiting: with nobody reading it, a
            // killed TUI can stay in exit on macOS while the tty waits for
            // its unread output to drain (seen once, 13 minutes, after the
            // mouse test's 200 redraws).
            self.master.lock().unwrap().take();
            let _ = self.child.wait();
        }
        self.master.lock().unwrap().take();
    }
}

impl Pty {
    pub fn start(data: &std::path::Path, args: &[&str]) -> Pty {
        Pty::start_sized(data, args, (100, 30))
    }

    /// [`Pty::start`] on a pty `size` columns × rows.
    pub fn start_sized(data: &std::path::Path, args: &[&str], size: (u16, u16)) -> Pty {
        let (mut master, mut slave) = (-1, -1);
        let mut size = libc::winsize {
            ws_row: size.1,
            ws_col: size.0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty fills in two new descriptors, which are owned
        // below and nowhere else.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(rc, 0, "openpty");
        // SAFETY: both were just opened and are owned once each.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // SAFETY: fcntl on descriptors this function owns. The master is
        // close-on-exec, or the TUI would hold it open itself.
        unsafe {
            let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(slave.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let secrets = data.join("test-secrets.json");
        let known_hosts = data.join("test-known_hosts");
        if !secrets.exists() {
            std::fs::write(&secrets, "{}").unwrap();
        }
        std::fs::write(&known_hosts, "").unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_seaquel-tui"));
        cmd.args(args)
            .env("SEAQUEL_DATA_DIR", data)
            .env("SEAQUEL_TUI_TEST_SECRETS", &secrets)
            .env("SEAQUEL_TUI_TEST_KNOWN_HOSTS", &known_hosts)
            .env("HOME", data)
            .env("TERM", "xterm-256color")
            .env("EDITOR", "/nonexistent/editor")
            .env("VISUAL", "/nonexistent/editor")
            .env_remove("NO_COLOR")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave.try_clone().unwrap()));
        // SAFETY: only async-signal-safe calls between fork and exec: a new
        // session, with the pty (stdin) as its controlling terminal, as a
        // terminal emulator starts its shell.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("start seaquel-tui");
        // Only the child holds the slave now, so closing the master hangs
        // it up.
        drop(slave);
        let master = Arc::new(Mutex::new(Some(master)));
        let out = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (m, o, s) = (master.clone(), out.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while !s.load(Ordering::SeqCst) {
                let n = {
                    let guard = m.lock().unwrap();
                    let Some(fd) = guard.as_ref() else { break };
                    // SAFETY: reads into a buffer of the length given.
                    unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) }
                };
                if n > 0 {
                    o.lock().unwrap().extend_from_slice(&buf[..n as usize]);
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
        Pty {
            master,
            out,
            stop,
            child,
        }
    }

    pub fn output(&self) -> Vec<u8> {
        self.out.lock().unwrap().clone()
    }

    pub fn wait_for(&self, pattern: &[u8]) {
        let start = Instant::now();
        while find(&self.output(), pattern).is_none() {
            assert!(
                start.elapsed() < WAIT,
                "never saw {:?} in:\n{}",
                String::from_utf8_lossy(pattern),
                String::from_utf8_lossy(&self.output())
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn send(&self, bytes: &[u8]) {
        let guard = self.master.lock().unwrap();
        let fd = guard.as_ref().expect("the terminal is open");
        let mut rest = bytes;
        let start = Instant::now();
        while !rest.is_empty() {
            // SAFETY: writes from a slice of the length given.
            let n = unsafe { libc::write(fd.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
            if n > 0 {
                rest = &rest[n as usize..];
            } else {
                assert!(start.elapsed() < WAIT, "the pty won't take input");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Typed slowly enough that no two keys arrive in one read.
    pub fn type_text(&self, text: &str) {
        for b in text.bytes() {
            self.send(&[b]);
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    /// Closes the terminal: the only master descriptor goes.
    pub fn close_terminal(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.master.lock().unwrap().take();
    }

    /// The exit status once the TUI has ended, within [`EXIT_WITHIN`].
    pub fn wait_exit(&mut self) -> std::process::ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if start.elapsed() > EXIT_WITHIN {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("seaquel-tui was still running {EXIT_WITHIN:?} after the terminal closed");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
