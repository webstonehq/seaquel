//! `seaquel-duckdb`, the DuckDB helper (the DuckDB helper plan): DuckDB in a
//! process of its own, so the terminal binaries don't link it. The client
//! (`seaquel-engine-duckdb`'s remote driver) starts one per open DuckDB
//! connection and speaks to it in frames over its stdin and stdout; the
//! loop is `seaquel_engine_duckdb::helper::serve`.
//!
//! - stdout carries frames only. The helper keeps its own copy of it for
//!   them and points the process's stdout at stderr (fd 1 on Unix; the
//!   standard handle and the C runtime's fd 1 on Windows), so anything else
//!   in the process that prints (DuckDB's progress bar draws on stdout)
//!   can't break the wire.
//! - stderr is the only log channel, and the helper writes to it only when
//!   it refuses to start or a thread panics (the location, never the
//!   message, which can quote DuckDB's text). The client discards it.
//! - It exits when stdin ends or a write to stdout fails (`EPIPE`, the
//!   client gone), whatever DuckDB is doing. Nothing else watches the
//!   parent: a parent that dies closes both pipes.

use std::io::IsTerminal;
use std::process::ExitCode;

/// The desktop app's version (`build.rs`), which `--version` prints and
/// `helloOk` reports; the client refuses a helper of another version.
const VERSION: &str = env!("SEAQUEL_APP_VERSION");

const BY_HAND: &str = "seaquel-duckdb is started by Seaquel; it isn't run by hand";

/// A wrong command line or a terminal on stdin.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    match (args.next(), args.next()) {
        (None, _) => {}
        (Some(arg), None) if arg == "--version" || arg == "-V" => {
            println!("seaquel-duckdb {VERSION}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("usage: seaquel-duckdb [--version]\n{BY_HAND}.");
            return ExitCode::from(EXIT_USAGE);
        }
    }
    if std::io::stdin().is_terminal() {
        eprintln!("{BY_HAND}.");
        return ExitCode::from(EXIT_USAGE);
    }

    quiet_panics();
    let frames = match frames_out() {
        Ok(frames) => frames,
        Err(_) => return ExitCode::FAILURE,
    };
    seaquel_engine_duckdb::helper::serve(std::io::stdin(), frames, VERSION)
}

/// A panic says where, never what: its message can quote DuckDB's text,
/// which can quote SQL. The call it happened in still gets its error frame
/// (the helper catches panics per call).
fn quiet_panics() {
    std::panic::set_hook(Box::new(|info| match info.location() {
        Some(at) => eprintln!(
            "seaquel-duckdb: a thread panicked at {}:{}",
            at.file(),
            at.line()
        ),
        None => eprintln!("seaquel-duckdb: a thread panicked"),
    }));
}

/// The output for frames: a copy of stdout, after which fd 1 is stderr.
#[cfg(unix)]
fn frames_out() -> std::io::Result<std::fs::File> {
    use std::os::fd::AsFd;
    // Close-on-exec, like every descriptor std makes.
    let frames = std::io::stdout().as_fd().try_clone_to_owned()?;
    // SAFETY: `dup2` on the process's own standard descriptors, which stay
    // open; nothing has written to stdout yet.
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(std::fs::File::from(frames))
}

/// The output for frames: a copy of the stdout handle, after which the
/// standard output handle (what Rust's `print!` writes to) and the C
/// runtime's fd 1 (what DuckDB's `printf`s write to) are stderr.
#[cfg(windows)]
fn frames_out() -> std::io::Result<std::fs::File> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Console::{SetStdHandle, STD_OUTPUT_HANDLE};
    // Not inheritable, like every handle std duplicates.
    let frames = std::io::stdout().as_handle().try_clone_to_owned()?;
    let stderr = std::io::stderr().as_raw_handle();
    // SAFETY: `SetStdHandle` with the process's own stderr handle, which
    // stays open; `_dup2` on the C runtime's own standard descriptors.
    unsafe {
        if SetStdHandle(STD_OUTPUT_HANDLE, stderr) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if libc::dup2(2, 1) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(std::fs::File::from(frames))
}
