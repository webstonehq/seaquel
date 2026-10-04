//! The TUI's log (Decision 17): never stdout or stderr while the screen is
//! up, but `<data_dir>/logs/tui.log` at `--log-level` (default `warn`),
//! created 0600 in a 0700 directory, rotated at 5 MB with two old files
//! kept (`tui.log.1`, `tui.log.2`). Only `logs/` is ever created; a missing
//! data dir means no log. A symlinked `logs/` or `tui.log` is refused
//! (`O_NOFOLLOW` on Unix).
//!
//! The filter is `seaquel-terminal`'s, with `rmcp`, `ratatui` and
//! `crossterm` also held at WARN. As everywhere: no SQL, values, secrets,
//! names, hosts or paths in a line; the TUI's own lines carry activity
//! names, ids, counts, codes and durations.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use seaquel_terminal::LogLevel;
use tracing_subscriber::filter::Targets;

/// Where the log goes under the data dir.
pub const LOG_DIR: &str = "logs";
pub const LOG_FILE: &str = "tui.log";
/// Rotate past this many bytes.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Old files kept.
pub const KEEP: usize = 2;

/// Targets held at WARN on top of the shared filter.
pub const HELD_AT_WARN: &[&str] = &["rmcp", "ratatui", "crossterm"];

/// The TUI's filter.
pub fn filter(level: LogLevel) -> Targets {
    seaquel_terminal::log_filter_holding(level, HELD_AT_WARN)
}

/// `<data_dir>/logs/tui.log`.
pub fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR).join(LOG_FILE)
}

/// An append-only file that moves itself to `.1` (and `.1` to `.2`, …)
/// once a write would take it past `max_bytes`.
#[derive(Debug)]
pub struct RotatingFile {
    path: PathBuf,
    file: File,
    len: u64,
    max_bytes: u64,
    keep: usize,
}

impl RotatingFile {
    /// Opens (or creates) `path` for appending, 0600, its directory 0700.
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<RotatingFile> {
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        refuse_symlink(path)?;
        let file = open_private(path)?;
        let len = file.metadata()?.len();
        Ok(RotatingFile {
            path: path.to_path_buf(),
            file,
            len,
            max_bytes,
            keep,
        })
    }

    /// `tui.log.(keep-1)` → `.keep`, …, `tui.log` → `.1`, then a new file.
    fn rotate(&mut self) -> io::Result<()> {
        let numbered = |n: usize| PathBuf::from(format!("{}.{n}", self.path.display()));
        if self.keep == 0 {
            std::fs::remove_file(&self.path)?;
        } else {
            for n in (1..self.keep).rev() {
                let from = numbered(n);
                if from.exists() {
                    std::fs::rename(&from, numbered(n + 1))?;
                }
            }
            std::fs::rename(&self.path, numbered(1))?;
        }
        self.file = open_private(&self.path)?;
        self.len = 0;
        Ok(())
    }
}

/// Makes the `logs` folder (never anything above it: the data dir must
/// exist) and refuses one that is a symlink.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(io::Error::other("the log folder isn't a plain folder"));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir)?;
        }
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A symlink where the log goes is refused, not followed (the open below
/// also uses `O_NOFOLLOW` on Unix, so a link made in between fails too).
fn refuse_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(io::Error::other("the log file is a symlink"))
        }
        _ => Ok(()),
    }
}

fn open_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        // A file an older run (or anyone) left wider is narrowed.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        options.open(path)
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// The longest key-value [`format_record`] writes, in bytes (before
/// escaping); longer ones end in `…`. The server's rule.
pub const MAX_LOG_VALUE_BYTES: usize = 128;

/// The longest message [`format_record`] writes, in bytes.
pub const MAX_LOG_MESSAGE_BYTES: usize = 1024;

/// One `log` record as `LEVEL target: message key=value …`, the server's
/// `format_record` (`seaquel-server/src/startup.rs`) without its prefix.
/// The level is padded to five columns, as `tracing`'s lines in the same
/// file are; [`FileLogger`] puts the time in front. The key-values are the
/// record's structured fields (`activity`, `code`, ids, counts):
/// - each value is cut at [`MAX_LOG_VALUE_BYTES`] while it is written,
///   never formatted whole, and the message at [`MAX_LOG_MESSAGE_BYTES`];
/// - a value holding a space, `=`, `"`, `\` or a control character (or
///   none at all) is quoted, logfmt style, with `"` and `\` escaped, so it
///   can't forge a field;
/// - control characters are escaped everywhere, so a record is one line.
pub fn format_record(record: &log::Record) -> String {
    struct Pairs(String);
    impl<'kvs> log::kv::VisitSource<'kvs> for Pairs {
        fn visit_pair(
            &mut self,
            key: log::kv::Key<'kvs>,
            value: log::kv::Value<'kvs>,
        ) -> Result<(), log::kv::Error> {
            self.0.push(' ');
            push_value(&mut self.0, format_args!("{}", key.as_str()));
            self.0.push('=');
            push_value(&mut self.0, format_args!("{value}"));
            Ok(())
        }
    }
    let mut line = format!("{:>5} {}: ", record.level(), record.target());
    let (message, cut) = capped(*record.args(), MAX_LOG_MESSAGE_BYTES);
    push_escaped(&mut line, &message, false);
    if cut {
        line.push('…');
    }
    let mut pairs = Pairs(String::new());
    let _ = record.key_values().visit(&mut pairs);
    line.push_str(&pairs.0);
    line
}

/// `args` formatted into at most `max` bytes (cut on a char boundary), and
/// whether it was cut. Formatting stops at the cap.
fn capped(args: std::fmt::Arguments<'_>, max: usize) -> (String, bool) {
    struct Capped {
        out: String,
        max: usize,
        cut: bool,
    }
    impl std::fmt::Write for Capped {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if self.out.len() + c.len_utf8() > self.max {
                    self.cut = true;
                    // Stops the formatting.
                    return Err(std::fmt::Error);
                }
                self.out.push(c);
            }
            Ok(())
        }
    }
    let mut w = Capped {
        out: String::new(),
        max,
        cut: false,
    };
    let _ = std::fmt::Write::write_fmt(&mut w, args);
    (w.out, w.cut)
}

/// One key or value, cut at [`MAX_LOG_VALUE_BYTES`], bare or quoted.
fn push_value(out: &mut String, args: std::fmt::Arguments<'_>) {
    let (text, cut) = capped(args, MAX_LOG_VALUE_BYTES);
    let quote = text.is_empty()
        || text
            .chars()
            .any(|c| matches!(c, ' ' | '=' | '"' | '\\') || c.is_control());
    if quote {
        out.push('"');
    }
    push_escaped(out, &text, quote);
    if cut {
        out.push('…');
    }
    if quote {
        out.push('"');
    }
}

/// `text` with control characters escaped (`\n`, `\u{1b}`), and in a
/// quoted value `"` and `\` too.
fn push_escaped(out: &mut String, text: &str, quoted: bool) {
    for c in text.chars() {
        if c.is_control() || (quoted && matches!(c, '"' | '\\')) {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
}

/// `log`'s records (Core's, sqlx's, the TUI's own), written to the log
/// file with their key-values, through the same filter as `tracing`'s
/// events. `tracing-log`'s bridge would drop the key-values, so the TUI
/// installs this logger instead of it.
pub struct FileLogger {
    out: SharedFile,
    filter: Targets,
}

impl FileLogger {
    pub fn new(out: SharedFile, filter: Targets) -> FileLogger {
        FileLogger { out, filter }
    }
}

/// `log`'s level as `tracing`'s, for [`Targets`].
fn tracing_level(level: log::Level) -> tracing::Level {
    match level {
        log::Level::Error => tracing::Level::ERROR,
        log::Level::Warn => tracing::Level::WARN,
        log::Level::Info => tracing::Level::INFO,
        log::Level::Debug => tracing::Level::DEBUG,
        log::Level::Trace => tracing::Level::TRACE,
    }
}

/// The most `log` may pass at `level` (the held and dropped targets are
/// [`FileLogger::enabled`]'s).
fn max_level(level: LogLevel) -> log::LevelFilter {
    match level {
        LogLevel::Off => log::LevelFilter::Off,
        LogLevel::Error => log::LevelFilter::Error,
        LogLevel::Warn => log::LevelFilter::Warn,
        LogLevel::Info => log::LevelFilter::Info,
        LogLevel::Debug => log::LevelFilter::Debug,
        LogLevel::Trace => log::LevelFilter::Trace,
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.filter
            .would_enable(metadata.target(), &tracing_level(metadata.level()))
    }

    fn log(&self, record: &log::Record) {
        use tracing_subscriber::fmt::time::FormatTime;
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = String::new();
        let _ = tracing_subscriber::fmt::time::SystemTime
            .format_time(&mut tracing_subscriber::fmt::format::Writer::new(&mut line));
        line.push(' ');
        line.push_str(&format_record(record));
        line.push('\n');
        let mut file = self.out.lock();
        let _ = file.write_all(line.as_bytes());
    }

    fn flush(&self) {
        let _ = self.out.lock().flush();
    }
}

/// The log file, shared by `log`'s records ([`FileLogger`]) and
/// `tracing`'s events (the `fmt` layer), so their lines never interleave.
#[derive(Clone)]
pub struct SharedFile(std::sync::Arc<std::sync::Mutex<RotatingFile>>);

impl SharedFile {
    pub fn new(file: RotatingFile) -> SharedFile {
        SharedFile(std::sync::Arc::new(std::sync::Mutex::new(file)))
    }

    /// The file; a panic while it was held doesn't stop the log.
    fn lock(&self) -> std::sync::MutexGuard<'_, RotatingFile> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One `tracing` event's write.
pub struct SharedWriter<'a>(std::sync::MutexGuard<'a, RotatingFile>);

impl Write for SharedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedFile {
    type Writer = SharedWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        SharedWriter(self.lock())
    }
}

/// Starts logging to `<data_dir>/logs/tui.log`; returns the file's path.
/// `tracing`'s events go through the `fmt` layer, and `log`'s records
/// (Core's, the TUI's) through [`FileLogger`], with their key-values.
///
/// **No data dir, no log:** when `data_dir` doesn't exist nothing is
/// created (the TUI never makes the app's data dir) and `None` comes back;
/// the TUI then runs without a log, and the open refuses a missing
/// `seaquel.db` anyway (`STORAGE_NOT_FOUND`).
pub fn init(data_dir: &Path, level: LogLevel) -> io::Result<Option<PathBuf>> {
    use tracing_subscriber::layer::SubscriberExt;

    if !data_dir.is_dir() {
        return Ok(None);
    }
    let path = log_path(data_dir);
    let out = SharedFile::new(RotatingFile::open(&path, MAX_BYTES, KEEP)?);
    // Not `try_init`: that would install `tracing-log`'s bridge, which drops
    // the records' key-values. A second call (tests) keeps the first
    // subscriber and logger.
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(out.clone())
                .with_ansi(false),
        )
        .with(filter(level));
    if tracing::subscriber::set_global_default(subscriber).is_ok()
        && log::set_boxed_logger(Box::new(FileLogger::new(out, filter(level)))).is_ok()
    {
        log::set_max_level(max_level(level));
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::filter::LevelFilter;

    fn enables(t: &Targets, target: &str, level: LevelFilter) -> bool {
        t.would_enable(target, &level.into_level().unwrap())
    }

    #[test]
    fn the_filter_drops_sql_and_holds_the_noisy_crates_at_warn() {
        for level in [
            LogLevel::Off,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let t = filter(level);
            for l in [
                LevelFilter::ERROR,
                LevelFilter::WARN,
                LevelFilter::INFO,
                LevelFilter::DEBUG,
                LevelFilter::TRACE,
            ] {
                for dropped in [
                    "sqlx::query",
                    "sqlx::postgres::notice",
                    "tiberius::tds::stream::token",
                    "sqlparser",
                ] {
                    assert!(!enables(&t, dropped, l), "{level:?} {dropped} {l:?}");
                }
                for held in [
                    "rmcp",
                    "tiberius",
                    "ratatui",
                    "crossterm",
                    "crossterm::event",
                ] {
                    if l > LevelFilter::WARN {
                        assert!(!enables(&t, held, l), "{level:?} {held} {l:?}");
                    }
                }
            }
        }
        assert!(enables(
            &filter(LogLevel::Warn),
            "crossterm",
            LevelFilter::WARN
        ));
        assert!(enables(
            &filter(LogLevel::Debug),
            "seaquel_tui",
            LevelFilter::DEBUG
        ));
        assert!(!enables(
            &filter(LogLevel::Warn),
            "seaquel_tui",
            LevelFilter::INFO
        ));
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        let mut file = RotatingFile::open(&path, MAX_BYTES, KEEP).unwrap();
        file.write_all(b"line\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line\n");
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_file_is_made_private_and_appended_to() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut file = RotatingFile::open(&path, MAX_BYTES, KEEP).unwrap();
        file.write_all(b"new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old\nnew\n");
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn rotates_past_the_limit_and_keeps_two() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        let mut file = RotatingFile::open(&path, 10, 2).unwrap();
        for line in ["aaaaaaa\n", "bbbbbbb\n", "ccccccc\n", "ddddddd\n"] {
            file.write_all(line.as_bytes()).unwrap();
        }
        let read =
            |suffix: &str| std::fs::read_to_string(format!("{}{suffix}", path.display())).ok();
        assert_eq!(read("").as_deref(), Some("ddddddd\n"));
        assert_eq!(read(".1").as_deref(), Some("ccccccc\n"));
        assert_eq!(read(".2").as_deref(), Some("bbbbbbb\n"));
        assert_eq!(read(".3"), None);
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(Path::new(&format!("{}.1", path.display()))), 0o600);
        }
    }

    #[test]
    fn no_data_dir_means_no_log() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-there");
        assert_eq!(init(&missing, LogLevel::Warn).unwrap(), None);
        assert!(!missing.exists(), "nothing was created");
    }

    #[test]
    fn only_logs_is_created_under_the_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        // `open` makes the `logs` folder, never the data dir above it.
        assert!(RotatingFile::open(&log_path(&data), MAX_BYTES, KEEP).is_err());
        assert!(!data.exists());
        std::fs::create_dir(&data).unwrap();
        RotatingFile::open(&log_path(&data), MAX_BYTES, KEEP).unwrap();
        assert!(log_path(&data).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_log_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(RotatingFile::open(&path, MAX_BYTES, KEEP).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_logs_folder_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        std::os::unix::fs::symlink(&elsewhere, data.join(LOG_DIR)).unwrap();
        assert!(RotatingFile::open(&log_path(&data), MAX_BYTES, KEEP).is_err());
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    }

    fn record_line(level: log::Level, target: &str, kvs: &[(&str, &str)], message: &str) -> String {
        format_record(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{message}"))
                .key_values(&kvs)
                .build(),
        )
    }

    #[test]
    fn a_record_s_key_values_follow_its_message() {
        let line = record_line(
            log::Level::Info,
            "seaquel_core::ai",
            &[("activity", "ai.generate"), ("code", "SECRET_UNREADABLE")],
            "Generating SQL",
        );
        assert_eq!(
            line,
            " INFO seaquel_core::ai: Generating SQL activity=ai.generate code=SECRET_UNREADABLE"
        );
    }

    #[test]
    fn a_value_is_quoted_escaped_and_cut_at_128_bytes() {
        let long = format!("marker \"x\"\nforged=1 {}", "y".repeat(400));
        let line = record_line(
            log::Level::Warn,
            "seaquel_tui",
            &[
                ("activity", "tui.state_file"),
                ("error", long.as_str()),
                ("empty", ""),
            ],
            "Ignoring the state file",
        );
        assert!(!line.contains('\n'), "one line: {line}");
        assert!(
            line.contains(r#"error="marker \"x\"\nforged=1 yyy"#),
            "quoted and escaped: {line}"
        );
        assert!(!line.contains(" forged=1"), "no forged field: {line}");
        let value = line.split("error=").nth(1).unwrap();
        let value = &value[..value.find("…\"").expect("cut with …")];
        // 128 bytes of the value, before escaping: `"` ×2 and `\n` gain one each.
        assert_eq!(value.len(), 1 + 128 + 3, "{value}");
        assert!(
            line.ends_with(r#" empty="""#),
            "an empty value is quoted: {line}"
        );
    }

    #[test]
    fn the_message_is_one_line_and_cut_at_1_kib() {
        let message = format!("first\r\nsecond \u{1b}[31m {}", "z".repeat(2000));
        let line = record_line(log::Level::Error, "seaquel_tui", &[("code", "X")], &message);
        assert!(!line.contains('\n') && !line.contains('\r') && !line.contains('\u{1b}'));
        assert!(line.contains(r"first\r\nsecond \u{1b}[31m"), "{line}");
        let (message, kvs) = line.split_once("…").expect("cut");
        assert_eq!(kvs, " code=X");
        let written = message.split_once(": ").unwrap().1;
        // 1 KiB before escaping; `\r` and `\n` gain a byte each, ESC five.
        assert_eq!(written.len(), 1024 + 1 + 1 + 5, "{written}");
    }

    #[test]
    fn the_file_logger_writes_key_values_through_the_filter() {
        use log::Log;
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(dir.path());
        let file = RotatingFile::open(&path, MAX_BYTES, KEEP).unwrap();
        let logger = FileLogger::new(SharedFile::new(file), filter(LogLevel::Info));
        let emit = |level, target: &str, message: &str| {
            let kvs: &[(&str, &str)] = &[("activity", "tui.ask"), ("code", "PROVIDER_ERROR")];
            logger.log(
                &log::Record::builder()
                    .level(level)
                    .target(target)
                    .args(format_args!("{message}"))
                    .key_values(&kvs)
                    .build(),
            );
        };
        emit(log::Level::Info, "seaquel_tui::runtime", "Ask AI failed");
        emit(log::Level::Debug, "seaquel_core", "debug-marker");
        emit(log::Level::Error, "sqlx::query", "SELECT 'sql-marker'");
        emit(log::Level::Info, "russh_keys::known_hosts", "host-marker");
        emit(log::Level::Warn, "russh", "russh warns");
        logger.flush();
        assert!(!logger.enabled(
            &log::Metadata::builder()
                .level(log::Level::Error)
                .target("sqlx::query")
                .build()
        ));
        let logged = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = logged.lines().collect();
        assert_eq!(lines.len(), 2, "{logged}");
        assert!(
            lines[0].ends_with(
                " INFO seaquel_tui::runtime: Ask AI failed activity=tui.ask code=PROVIDER_ERROR"
            ),
            "{logged}"
        );
        assert!(
            lines[1].contains(" WARN russh: russh warns activity=tui.ask"),
            "{logged}"
        );
        // A timestamp first, as the `tracing` lines have.
        assert!(lines[0].as_bytes()[0].is_ascii_digit(), "{logged}");
        for marker in ["debug-marker", "sql-marker", "host-marker"] {
            assert!(!logged.contains(marker), "{marker}: {logged}");
        }
    }
}
