//! What `main` does before it serves: a stderr logger, the bind-address
//! check and the file-descriptor limit.

use std::io::Write;
use std::net::SocketAddr;

/// A minimal stderr logger, so `log` lines (the `/rpc` error log, startup
/// messages) reach the container's output. Seaquel's own crates log at info
/// and above; dependencies at warn and above, except sqlx's statement log
/// (`sqlx::query`), which holds a statement's whole SQL and never passes
/// (the drivers also turn it off).
struct StderrLogger;

/// sqlx's statement log target.
const SQLX_QUERY_TARGET: &str = "sqlx::query";

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if metadata.target() == SQLX_QUERY_TARGET {
            return false;
        }
        let floor = if metadata.target().starts_with("seaquel") {
            log::Level::Info
        } else {
            log::Level::Warn
        };
        metadata.level() <= floor
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "[seaquel-server] {} {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
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
        let mut targets = vec![hard];
        #[cfg(target_os = "macos")]
        targets.push(hard.min(10240));
        for target in targets {
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
