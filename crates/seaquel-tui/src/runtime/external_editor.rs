//! `$EDITOR` (Decision 14, spike S5): Ctrl+O hands the tab's text to
//! `$VISUAL`, else `$EDITOR`, else `vi`, and reads it back. The text goes to
//! a file of its own under `<data_dir>/tui/` (created 0600, never through a
//! symlink, removed afterwards whatever happened); the command runs through
//! `sh -c` so `EDITOR="code -w"` works. The loop gives the editor the
//! terminal ([`super::terminal::outside`]) and takes it back without
//! `Terminal::clear()`.
//!
//! An editor that can't start or exits with an error leaves the text as it
//! was; the screen says why.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The command the editor runs as: `$VISUAL`, else `$EDITOR`, else `vi`
/// (an empty variable doesn't count).
pub fn editor_command(visual: Option<String>, editor: Option<String>) -> String {
    [visual, editor]
        .into_iter()
        .flatten()
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// [`editor_command`] from the process's environment.
pub fn editor_from_env() -> String {
    editor_command(std::env::var("VISUAL").ok(), std::env::var("EDITOR").ok())
}

/// Runs `command` on a file holding `text` and answers what the file holds
/// afterwards. A trailing newline the editor added is dropped when `text`
/// had none. `Err` says why the text is unchanged.
///
/// With `job_control` (the real terminal), the editor runs in a process
/// group of its own that owns the terminal (review I4): Ctrl+Z there
/// stops the editor alone, and the TUI then takes the terminal back and
/// stops itself, so the shell sees one stopped job; `fg` continues the TUI,
/// which gives the terminal back to the editor and continues it. Tests
/// without a terminal pass `false`.
pub fn edit(text: &str, data_dir: &Path, command: &str) -> Result<String, String> {
    edit_with(text, data_dir, command, false)
}

/// [`edit`], with job control when `job_control`.
pub fn edit_with(
    text: &str,
    data_dir: &Path,
    command: &str,
    job_control: bool,
) -> Result<String, String> {
    if !data_dir.is_dir() {
        return Err("the data dir doesn't exist".to_string());
    }
    let folder = data_dir.join("tui");
    super::state_file::make_private_dir(&folder)
        .map_err(|e| format!("can't make the folder for the file: {}", e.kind()))?;
    // Removed on every way out, an unwind included (review M2).
    let file = TempFile(temp_path(&folder));
    let mut out =
        new_private_file(&file.0).map_err(|e| format!("can't write the file: {}", e.kind()))?;
    out.write_all(text.as_bytes())
        .and_then(|()| out.sync_all())
        .map_err(|e| format!("can't write the file: {}", e.kind()))?;
    drop(out);
    run(shell(command, &file.0), job_control)?;
    let mut edited = std::fs::read_to_string(&file.0)
        .map_err(|e| format!("can't read the file back: {}", e.kind()))?;
    if !text.ends_with('\n') && edited.ends_with('\n') {
        edited.pop();
        if edited.ends_with('\r') {
            edited.pop();
        }
    }
    Ok(edited)
}

/// The edit's file, removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs the editor to its end; `Err` says how it failed.
fn run(mut command: Command, job_control: bool) -> Result<(), String> {
    #[cfg(unix)]
    if job_control {
        return job::run(command);
    }
    let _ = job_control;
    let status = command
        .status()
        .map_err(|e| format!("can't start the editor: {}", e.kind()))?;
    if status.success() {
        return Ok(());
    }
    Err(match status.code() {
        Some(code) => format!("the editor exited with status {code}"),
        None => "the editor was stopped by a signal".to_string(),
    })
}

/// The editor as a job of its own on the terminal (review I4).
#[cfg(unix)]
mod job {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    /// Makes process group `group` the terminal's foreground (stdin is the
    /// terminal). SIGTTOU is ignored around it: a background group may not
    /// set it otherwise.
    fn give_terminal(group: libc::pid_t) {
        // SAFETY: plain libc calls; the old SIGTTOU disposition is put back.
        unsafe {
            let old = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(libc::STDIN_FILENO, group);
            libc::signal(libc::SIGTTOU, old);
        }
    }

    pub fn run(mut command: Command) -> Result<(), String> {
        command.process_group(0);
        // SAFETY: the closure runs in the child between fork and exec and
        // calls only async-signal-safe functions (signal, tcsetpgrp,
        // getpid).
        unsafe {
            command.pre_exec(|| {
                let old = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpid());
                libc::signal(libc::SIGTTOU, old);
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|e| format!("can't start the editor: {}", e.kind()))?;
        let pid = child.id() as libc::pid_t;
        // Also from here: whichever runs first, the editor has it.
        give_terminal(pid);
        // SAFETY: `getpgrp` has no preconditions.
        let ours = unsafe { libc::getpgrp() };
        let result = loop {
            let mut status: libc::c_int = 0;
            // SAFETY: `waitpid` on our own child, writing to a local.
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
            if r < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break Err("lost track of the editor".to_string());
            }
            if libc::WIFSTOPPED(status) {
                // Ctrl+Z in the editor: the job stops as a whole.
                give_terminal(ours);
                // SAFETY: stops this process until SIGCONT (as Ctrl+Z
                // does, `terminal::suspend`), then returns.
                unsafe {
                    libc::raise(libc::SIGSTOP);
                }
                give_terminal(pid);
                // SAFETY: continues the editor's own process group.
                unsafe {
                    libc::kill(-pid, libc::SIGCONT);
                }
                continue;
            }
            if libc::WIFEXITED(status) {
                break match libc::WEXITSTATUS(status) {
                    0 => Ok(()),
                    code => Err(format!("the editor exited with status {code}")),
                };
            }
            break Err("the editor was stopped by a signal".to_string());
        };
        give_terminal(ours);
        // Reaped above; nothing for `Child` to wait for.
        drop(child);
        result
    }
}

/// A name of its own for this edit.
fn temp_path(folder: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    folder.join(format!("edit-{}-{n}.sql", std::process::id()))
}

/// A new file, 0600, never through a symlink or over an existing file.
fn new_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// The editor command with the file as its last argument, through the
/// shell (so a command with arguments works). `exec`, so the editor is the
/// process the TUI waits on (and sees stop).
#[cfg(unix)]
fn shell(command: &str, path: &Path) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(format!("exec {command} \"$1\""))
        .arg("seaquel-tui-editor")
        .arg(path);
    c
}

#[cfg(not(unix))]
fn shell(command: &str, path: &Path) -> Command {
    let mut c = Command::new("cmd");
    c.arg("/C").arg(command).arg(path);
    c
}

/// Removes `tui/edit-*` files and editors' swap files for them
/// (`tui/.edit-*.sw?`) older than `max_age`, left by a run that crashed or
/// was killed (review M2). A file whose process (the pid in its name) is
/// still running is kept, whatever its age.
pub fn sweep(data_dir: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(data_dir.join("tui")) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = leftover_pid(&name) else {
            continue;
        };
        if alive(pid) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= max_age);
        if meta.is_file() && old && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The pid in `edit-<pid>-<n>.sql`, or in a swap file for one
/// (`.edit-<pid>-<n>.sql.sw?`); `None` for any other name.
fn leftover_pid(name: &str) -> Option<u32> {
    let rest = match name.strip_prefix(".edit-") {
        Some(rest) => {
            let (base, swap) = rest.rsplit_once('.')?;
            let swap_ok = swap.len() == 3 && swap.starts_with("sw");
            (swap_ok && base.ends_with(".sql")).then_some(base)?
        }
        None => name.strip_prefix("edit-").filter(|r| r.ends_with(".sql"))?,
    };
    let (pid, n) = rest.trim_end_matches(".sql").split_once('-')?;
    n.parse::<u64>().ok()?;
    pid.parse().ok()
}

/// Whether a process with this pid exists.
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A shell script used as the editor.
    fn script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir.join("tui"))
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("edit-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    // Review M2: old leftovers go, a live process's and new ones stay.
    #[test]
    fn the_startup_sweep_removes_old_leftovers_only() {
        let dir = tempfile::tempdir().unwrap();
        let tui = dir.path().join("tui");
        std::fs::create_dir(&tui).unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        // A pid that can't be running.
        let gone = 999_999_999u32;
        let me = std::process::id();
        let files = [
            (format!("edit-{gone}-0.sql"), true, true),
            (format!(".edit-{gone}-0.sql.swp"), true, true),
            (format!(".edit-{gone}-1.sql.swo"), true, true),
            (format!("edit-{me}-0.sql"), true, false),
            (format!("edit-{gone}-2.sql"), false, false),
            ("state.json".to_string(), true, false),
            (format!("edit-{gone}-3.txt"), true, false),
        ];
        for (name, aged, _) in &files {
            let path = tui.join(name);
            let f = std::fs::File::create(&path).unwrap();
            if *aged {
                f.set_modified(old).unwrap();
            }
        }
        let removed = sweep(dir.path(), std::time::Duration::from_secs(24 * 3600));
        assert_eq!(removed, 3);
        for (name, _, gone) in &files {
            assert_eq!(!tui.join(name).exists(), *gone, "{name}");
        }
        // No `tui/` folder: nothing to do.
        assert_eq!(
            sweep(&dir.path().join("missing"), std::time::Duration::ZERO),
            0
        );
    }

    // Review M2: the temp file goes even when reading it back fails.
    #[test]
    fn the_temp_file_goes_on_every_path() {
        let dir = tempfile::tempdir().unwrap();
        let removes = script(dir.path(), "rm.sh", "chmod 000 \"$1\"");
        assert!(edit("x", dir.path(), &removes).is_err());
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn visual_then_editor_then_vi() {
        assert_eq!(
            editor_command(Some("code -w".into()), Some("nano".into())),
            "code -w"
        );
        assert_eq!(
            editor_command(Some(String::new()), Some("nano".into())),
            "nano"
        );
        assert_eq!(editor_command(None, Some("  ".into())), "vi");
        assert_eq!(editor_command(None, None), "vi");
    }

    #[test]
    fn the_text_round_trips_through_a_private_file() {
        let dir = tempfile::tempdir().unwrap();
        let editor = script(
            dir.path(),
            "ed.sh",
            "ls -l \"$1\" | cut -c1-10 > \"$(dirname \"$1\")/../mode\"\nprintf ' -- edited\\n' >> \"$1\"",
        );
        let out = edit("SELECT 1", dir.path(), &editor).unwrap();
        assert_eq!(out, "SELECT 1 -- edited", "the editor's newline is dropped");
        let mode = std::fs::read_to_string(dir.path().join("mode")).unwrap();
        assert_eq!(mode.trim(), "-rw-------");
        assert!(leftovers(dir.path()).is_empty(), "the file is removed");
        // A text that ended with a newline keeps it.
        let keep = script(dir.path(), "keep.sh", "true");
        assert_eq!(edit("SELECT 2\n", dir.path(), &keep).unwrap(), "SELECT 2\n");
        // Arguments in the command work (`code -w`).
        let args = script(
            dir.path(),
            "args.sh",
            "[ \"$1\" = \"-w\" ] && printf X >> \"$2\"",
        );
        assert_eq!(edit("a", dir.path(), &format!("{args} -w")).unwrap(), "aX");
    }

    #[test]
    fn a_missing_or_failing_editor_leaves_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let e = edit("SELECT 1", dir.path(), "/nonexistent/editor").unwrap_err();
        // `exec` of a missing file: 126 or 127, by shell.
        assert!(e.contains("exited with status 12"), "{e}");
        let fails = script(dir.path(), "fails.sh", "printf X >> \"$1\"; exit 3");
        let e = edit("SELECT 1", dir.path(), &fails).unwrap_err();
        assert!(e.contains('3'), "{e}");
        assert!(leftovers(dir.path()).is_empty());
        // No data dir: nothing is created and it says so.
        let gone = dir.path().join("missing");
        assert!(edit("x", &gone, "true").is_err());
        assert!(!gone.exists());
    }
}
