//! What `main` does before it serves: a stderr logger, the bind-address
//! check and the file-descriptor limit.

use std::io::Write;
use std::net::SocketAddr;

/// A minimal stderr logger, so `log` lines (the `/rpc` error log, startup
/// messages) reach the container's output. Seaquel's own crates log at info
/// and above; dependencies at warn and above, except the targets in
/// [`DROPPED_TARGETS`], which never pass.
struct StderrLogger;

/// Log targets (and their submodules) that quote what a user's query sent,
/// dropped at every level:
/// - sqlx's statement log (`sqlx::query`, a statement's whole SQL; the
///   drivers also turn it off);
/// - Postgres notices (`sqlx::postgres::notice`, a `RAISE WARNING`'s text,
///   which the query chooses and can fill with values; probe M5);
/// - tiberius's token stream (`tiberius::tds::stream::token`): every SQL
///   Server error at ERROR and every `PRINT`/info message at INFO, with the
///   server's text, which quotes values ("Conversion failed … 'x'", a
///   duplicate key, `THROW`'s text). tiberius's TLS warnings stay.
pub const DROPPED_TARGETS: [&str; 3] = [
    "sqlx::query",
    "sqlx::postgres::notice",
    "tiberius::tds::stream::token",
];

/// Whether `target` is one of [`DROPPED_TARGETS`] or inside one.
fn dropped(target: &str) -> bool {
    DROPPED_TARGETS.iter().any(|t| {
        target
            .strip_prefix(t)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

/// Whether the server's logger writes a record with this metadata (see
/// [`init_logging`]). Public so tests can check what reaches the log.
pub fn logs(metadata: &log::Metadata) -> bool {
    if dropped(metadata.target()) {
        return false;
    }
    let floor = if metadata.target().starts_with("seaquel") {
        log::Level::Info
    } else {
        log::Level::Warn
    };
    metadata.level() <= floor
}

/// One log line: `[seaquel-server] LEVEL target: message key=value …`.
/// The key-values are the record's structured fields (`activity`, `code`,
/// ids, counts); nothing logged as one holds SQL or values. Some come from
/// the browser (a stream or connection id, before Core checks it), so
/// (phase 5b review, I1):
/// - each value is cut at [`MAX_LOG_VALUE_BYTES`] while it is written, never
///   formatted whole;
/// - a value holding a space, `=`, `"`, `\` or a control character is
///   quoted, logfmt style, with `"` and `\` escaped, so it can't forge a
///   field;
/// - control characters are escaped everywhere, so a record is one line.
///
/// Public so tests can check what reaches the log.
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
    let mut line = format!("[seaquel-server] {} {}: ", record.level(), record.target());
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

/// The longest key-value [`format_record`] writes, in bytes (before
/// escaping); longer ones end in `…`.
pub const MAX_LOG_VALUE_BYTES: usize = 128;

/// The longest message [`format_record`] writes, in bytes.
const MAX_LOG_MESSAGE_BYTES: usize = 1024;

/// `args` formatted into at most `max` bytes (cut on a char boundary), and
/// whether it was cut. Formatting stops at the cap: a long value is never
/// formatted whole.
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

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        logs(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(std::io::stderr().lock(), "{}", format_record(record));
        }
    }

    fn flush(&self) {}
}

/// Install [`StderrLogger`]. Does nothing if a logger is already set.
pub fn init_logging() {
    if log::set_logger(&StderrLogger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// Where the assistant's model calls may go (phase 6, Decision 9):
/// `public` (the default), `any` or `off`.
pub const AI_EGRESS_ENV: &str = "SEAQUEL_AI_EGRESS";

/// [`AI_EGRESS_ENV`]'s value as Core's `AiEgress`: unset or empty is
/// `Public`; anything `Egress::parse` doesn't read is an error naming the
/// variable, so `main` refuses to start on a typo rather than guess.
pub fn ai_egress_from(value: Option<&str>) -> Result<seaquel_core::ai::AiEgress, String> {
    use seaquel_core::ai::native::Egress;
    let value = value.unwrap_or("");
    if value.trim().is_empty() {
        return Ok(seaquel_core::ai::AiEgress::Public);
    }
    Egress::parse(value).map(Into::into).ok_or_else(|| {
        // The value isn't echoed: it's the operator's, but logs travel.
        format!("{AI_EGRESS_ENV} must be public, any or off")
    })
}

/// [`ai_egress_from`] over this process's environment.
pub fn ai_egress_from_env() -> Result<seaquel_core::ai::AiEgress, String> {
    ai_egress_from(std::env::var(AI_EGRESS_ENV).ok().as_deref())
}

/// The PEM bundle the model client trusts on top of the built-in roots
/// (a TLS-inspecting proxy's CA), as the license client does.
pub const EXTRA_CA_CERTS_ENV: &str = "NODE_EXTRA_CA_CERTS";

/// Set to `1` to let [`check_bind_addr`] accept a non-loopback address.
pub const ALLOW_NON_LOOPBACK_ENV: &str = "SEAQUEL_ALLOW_NON_LOOPBACK";

/// Below this many open files, `main` warns. At the LRU's cap of 1024
/// workspaces, each pool can hold 2 connections, and each SQLite connection
/// in WAL mode keeps the database, its `-wal` and its `-shm` open: about
/// 6,000 descriptors, plus sockets and the engines' connections.
pub const RECOMMENDED_NOFILE: u64 = 8192;

/// Refuse a bind address that isn't loopback unless `allow_non_loopback`.
/// The server trusts `X-Seaquel-User` (see `main.rs`), so anything that can
/// reach it can act as any user.
pub fn check_bind_addr(addr: &SocketAddr, allow_non_loopback: bool) -> Result<(), String> {
    if addr.ip().is_loopback() || allow_non_loopback {
        return Ok(());
    }
    Err(format!(
        "refusing to bind to {addr}: it isn't a loopback address, and this server trusts the \
         X-Seaquel-User header, so anyone who can reach it can act as any user. Bind to \
         127.0.0.1 (the default) and reach it through the Node server. For standalone local \
         development only, set {ALLOW_NON_LOOPBACK_ENV}=1."
    ))
}

/// Whether `SEAQUEL_ALLOW_NON_LOOPBACK` is `1`.
pub fn allow_non_loopback_from_env() -> bool {
    std::env::var(ALLOW_NON_LOOPBACK_ENV).is_ok_and(|v| v == "1")
}

/// The soft open-files limit before and after [`raise_nofile_limit`], and
/// the hard limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NofileLimit {
    pub before: u64,
    pub after: u64,
    pub hard: u64,
}

/// Raise the soft `RLIMIT_NOFILE` as far towards the hard limit as the OS
/// allows. Never lowers it. `None` where there is no such limit (not Unix)
/// or it can't be read.
#[cfg(unix)]
// `rlim_t` is u64 on the targets we ship, but not on every Unix.
#[allow(clippy::unnecessary_cast)]
pub fn raise_nofile_limit() -> Option<NofileLimit> {
    fn get() -> Option<libc::rlimit> {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: `lim` is a valid, writable rlimit.
        (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) } == 0).then_some(lim)
    }
    fn set(cur: libc::rlim_t, max: libc::rlim_t) -> bool {
        let lim = libc::rlimit {
            rlim_cur: cur,
            rlim_max: max,
        };
        // SAFETY: `lim` is a valid rlimit.
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) == 0 }
    }

    let lim = get()?;
    let before = lim.rlim_cur;
    let hard = lim.rlim_max;
    if before < hard {
        // Try the hard limit first. macOS refuses a soft limit above
        // OPEN_MAX (10240) even when the hard limit is unlimited, so fall
        // back to that.
        let targets = [
            Some(hard),
            cfg!(target_os = "macos").then(|| hard.min(10240)),
        ];
        for target in targets.into_iter().flatten() {
            if target > before && set(target, hard) {
                break;
            }
        }
    }
    let after = get().map_or(before, |l| l.rlim_cur);
    Some(NofileLimit {
        before: before as u64,
        after: after as u64,
        hard: hard as u64,
    })
}

#[cfg(not(unix))]
pub fn raise_nofile_limit() -> Option<NofileLimit> {
    None
}

/// The env var `server.js` passes the per-boot `/internal/*` secret in.
pub const INTERNAL_SECRET_ENV: &str = "SEAQUEL_INTERNAL_SECRET";

/// Read the `/internal/*` secret and remove it from this process's
/// environment, so nothing running in-process later (an engine's own
/// functions) can read it back. Call before serving.
pub fn take_internal_secret() -> Option<String> {
    let secret = std::env::var(INTERNAL_SECRET_ENV)
        .ok()
        .filter(|s| !s.is_empty());
    std::env::remove_var(INTERNAL_SECRET_ENV);
    secret
}

/// Where `HOME` points once [`scrub_database_client_env`] has run: a path
/// that doesn't exist and only root could create (as `server.js` sets it).
pub const NO_HOME: &str = "/nonexistent";

/// A variable a database client library takes defaults from: libpq's
/// (`PGPASSWORD`, `PGUSER`, `PGHOST`, `PGSSLKEY`, `PGPASSFILE`, …, which sqlx
/// reads) and MySQL's (`MYSQL_PWD`, `MYSQL_HOST`, …).
pub fn is_database_client_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("PG") || upper.starts_with("MYSQL")
}

/// Remove every [`is_database_client_var`] from this process's environment
/// and point `HOME` at [`NO_HOME`], returning the names removed.
///
/// A web user's Postgres URL without a password would otherwise get the
/// operator's `PGPASSWORD` or a `~/.pgpass` entry, and one without a host,
/// user or TLS files the operator's `PG*` defaults. `server.js` already
/// passes an allow-listed environment; this covers running the binary any
/// other way (`npm run rust:dev`). Call first in `main`, like
/// [`take_internal_secret`].
pub fn scrub_database_client_env() -> Vec<String> {
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| is_database_client_var(name))
        .collect();
    for name in &names {
        std::env::remove_var(name);
    }
    std::env::set_var("HOME", NO_HOME);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logs(target: &str, level: log::Level) -> bool {
        use log::Log;
        StderrLogger.enabled(&log::Metadata::builder().target(target).level(level).build())
    }

    /// sqlx's statement log holds the whole SQL: never, at any level.
    #[test]
    fn sqlx_statement_log_is_dropped() {
        for level in [log::Level::Error, log::Level::Warn, log::Level::Info] {
            assert!(!logs("sqlx::query", level), "{level}");
        }
        assert!(logs("sqlx::pool", log::Level::Warn));
        assert!(logs("seaquel_server::routes::rpc", log::Level::Info));
        // Postgres notices carry text the query chose (`RAISE WARNING`).
        for level in [log::Level::Error, log::Level::Warn, log::Level::Info] {
            assert!(!logs("sqlx::postgres::notice", level), "{level}");
            assert!(!logs("tiberius::tds::stream::token", level), "{level}");
        }
        assert!(logs("tiberius::client::tls_stream", log::Level::Warn));
        assert!(!logs("tiberius::client::tls_stream", log::Level::Info));
        assert!(logs("sqlx::query_builder", log::Level::Warn));
        assert!(!logs("hyper", log::Level::Info));
    }

    #[test]
    fn database_client_vars_are_recognised() {
        for name in [
            "PGPASSWORD",
            "PGPASSFILE",
            "PGUSER",
            "PGHOST",
            "PGSSLKEY",
            "pgpassword",
            "MYSQL_PWD",
            "MYSQL_HOST",
        ] {
            assert!(is_database_client_var(name), "{name}");
        }
        for name in ["PATH", "DATA_DIR", "SEAQUEL_CONTROL_URL", "HOME", "TZ"] {
            assert!(!is_database_client_var(name), "{name}");
        }
    }

    #[test]
    fn ai_egress_reads_public_any_and_off() {
        use seaquel_core::ai::AiEgress;
        assert_eq!(ai_egress_from(None), Ok(AiEgress::Public));
        assert_eq!(ai_egress_from(Some("")), Ok(AiEgress::Public));
        assert_eq!(ai_egress_from(Some("public")), Ok(AiEgress::Public));
        assert_eq!(ai_egress_from(Some(" Any ")), Ok(AiEgress::Any));
        assert_eq!(ai_egress_from(Some("OFF")), Ok(AiEgress::Off));
        let err = ai_egress_from(Some("pubilc")).unwrap_err();
        assert!(err.contains(AI_EGRESS_ENV), "{err}");
        assert!(err.contains("public, any or off"), "{err}");
    }

    #[test]
    fn loopback_binds_are_allowed() {
        for addr in ["127.0.0.1:8788", "[::1]:8788", "127.0.0.2:1"] {
            assert_eq!(check_bind_addr(&addr.parse().unwrap(), false), Ok(()));
        }
    }

    #[test]
    fn other_binds_need_the_opt_in() {
        for addr in [
            "0.0.0.0:8788",
            "[::]:8788",
            "10.0.0.5:8788",
            "192.168.1.2:80",
        ] {
            let addr: SocketAddr = addr.parse().unwrap();
            let err = check_bind_addr(&addr, false).unwrap_err();
            assert!(err.contains(ALLOW_NON_LOOPBACK_ENV), "{err}");
            assert_eq!(check_bind_addr(&addr, true), Ok(()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn raising_the_limit_never_lowers_it() {
        let lim = raise_nofile_limit().expect("RLIMIT_NOFILE is readable");
        assert!(lim.after >= lim.before, "{lim:?}");
        assert!(lim.after <= lim.hard, "{lim:?}");
        // A second call finds it already raised.
        let again = raise_nofile_limit().unwrap();
        assert_eq!(again.before, lim.after);
        assert_eq!(again.after, lim.after);
    }
}
