//! What `main` does before it serves: a stderr logger, the bind-address
//! check and the file-descriptor limit.

use std::io::Write;
use std::net::SocketAddr;

/// A minimal stderr logger, so `log` lines (the `/rpc` error log, startup
/// messages) reach the container's output. Seaquel's own crates log at info
/// and above; dependencies at warn and above.
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
