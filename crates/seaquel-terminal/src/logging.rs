//! The terminal binaries' log filter (moved from `seaquel-cli`'s `mcp.rs`,
//! Decision 17). sqlx's `sqlx::query` target (any statement slower than 1 s
//! at WARN, with its full SQL) and `sqlx::postgres::notice` (a `RAISE`'s
//! text) are dropped at every level, and so are tiberius's token stream
//! (SQL Server errors and `PRINT` text) and `sqlparser`; the rest of
//! tiberius passes at WARN at most, as do `russh` and `russh_keys` (probe
//! F6: the SSH bastion's host and host keys at DEBUG). [`log_filter_holding`]
//! holds more targets at WARN on top (the TUI's `rmcp`, `ratatui`,
//! `crossterm`).

use clap::ValueEnum;
use tracing_subscriber::filter::{LevelFilter, Targets};

/// `--log-level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn filter(self) -> LevelFilter {
        match self {
            LogLevel::Off => LevelFilter::OFF,
            LogLevel::Error => LevelFilter::ERROR,
            LogLevel::Warn => LevelFilter::WARN,
            LogLevel::Info => LevelFilter::INFO,
            LogLevel::Debug => LevelFilter::DEBUG,
            LogLevel::Trace => LevelFilter::TRACE,
        }
    }
}

/// sqlx's statement log target. It logs every statement at DEBUG, and each
/// one slower than a second at WARN, with its whole SQL, which may quote
/// values. The drivers turn it off; this drops it too, at every level.
const SQLX_QUERY_TARGET: &str = "sqlx::query";

/// sqlx's Postgres notice target: a `RAISE WARNING`'s text, which the query
/// chooses and can fill with values (probe M5). Dropped at every level.
const SQLX_NOTICE_TARGET: &str = "sqlx::postgres::notice";

/// tiberius's token stream: every SQL Server error (ERROR) and `PRINT`
/// (INFO) with the server's text, which can quote values. Dropped at every
/// level; the rest of tiberius passes at WARN at most (its TLS warnings).
const TIBERIUS_TOKEN_TARGET: &str = "tiberius::tds::stream::token";

/// What `--log-level` lets through: everything at `level`, except
/// `sqlx::query`, `sqlx::postgres::notice`, tiberius's token stream and
/// `sqlparser`, which log SQL or query text and never pass, and the rest of
/// tiberius, `russh` and `russh_keys` (the SSH bastion's host and host
/// keys), held at WARN.
pub fn log_filter(level: LogLevel) -> Targets {
    let filter = level.filter();
    Targets::new()
        .with_default(filter)
        .with_target(SQLX_QUERY_TARGET, LevelFilter::OFF)
        .with_target(SQLX_NOTICE_TARGET, LevelFilter::OFF)
        .with_target(TIBERIUS_TOKEN_TARGET, LevelFilter::OFF)
        .with_target("tiberius", filter.min(LevelFilter::WARN))
        .with_target("sqlparser", LevelFilter::OFF)
        // russh names the SSH bastion's host and port and prints its host
        // keys at DEBUG and TRACE (probe F6).
        .with_target("russh", filter.min(LevelFilter::WARN))
        .with_target("russh_keys", filter.min(LevelFilter::WARN))
}

/// [`log_filter`], with each of `held` (and its submodules) also held at
/// WARN at most.
pub fn log_filter_holding(level: LogLevel, held: &[&str]) -> Targets {
    let filter = level.filter();
    held.iter().fold(log_filter(level), |targets, target| {
        targets.with_target(*target, filter.min(LevelFilter::WARN))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The levels, as `LevelFilter`s (the CLI doesn't depend on `tracing`).
    struct Level;
    impl Level {
        const ERROR: LevelFilter = LevelFilter::ERROR;
        const WARN: LevelFilter = LevelFilter::WARN;
        const INFO: LevelFilter = LevelFilter::INFO;
        const DEBUG: LevelFilter = LevelFilter::DEBUG;
        const TRACE: LevelFilter = LevelFilter::TRACE;
    }

    fn enables(t: &Targets, target: &str, level: LevelFilter) -> bool {
        t.would_enable(target, &level.into_level().unwrap())
    }

    const LEVELS: [LogLevel; 6] = [
        LogLevel::Off,
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Debug,
        LogLevel::Trace,
    ];

    // Moved unchanged from `seaquel-cli`'s `mcp.rs`.
    #[test]
    fn sqlx_statement_logs_never_pass() {
        for level in LEVELS {
            let t = log_filter(level);
            for l in [
                Level::ERROR,
                Level::WARN,
                Level::INFO,
                Level::DEBUG,
                Level::TRACE,
            ] {
                assert!(!enables(&t, "sqlx::query", l), "{level:?} {l:?}");
                assert!(!enables(&t, "sqlx::postgres::notice", l), "{level:?} {l:?}");
                assert!(
                    !enables(&t, "tiberius::tds::stream::token", l),
                    "{level:?} {l:?}"
                );
                if l > Level::WARN {
                    assert!(!enables(&t, "tiberius::client", l), "{level:?} {l:?}");
                }
            }
        }
        assert!(enables(
            &log_filter(LogLevel::Warn),
            "seaquel_mcp::transport",
            Level::WARN
        ));
        assert!(enables(
            &log_filter(LogLevel::Warn),
            "sqlx::pool",
            Level::WARN
        ));
        assert!(enables(
            &log_filter(LogLevel::Error),
            "seaquel_mcp",
            Level::ERROR
        ));
    }

    #[test]
    fn sqlparser_never_passes() {
        for level in LEVELS {
            let t = log_filter(level);
            assert!(!enables(&t, "sqlparser::parser", Level::ERROR), "{level:?}");
        }
    }

    /// Probe F6: at DEBUG and TRACE russh names the SSH bastion's host and
    /// port (`russh_keys::known_hosts`) and prints its host keys
    /// (`russh::client`). Both are held at WARN in the shared filter, so
    /// both binaries get it.
    #[test]
    fn russh_is_held_at_warn() {
        for level in LEVELS {
            for t in [log_filter(level), log_filter_holding(level, &["rmcp"])] {
                for target in [
                    "russh",
                    "russh::client",
                    "russh::client::encrypted",
                    "russh_keys",
                    "russh_keys::known_hosts",
                ] {
                    for l in [Level::INFO, Level::DEBUG, Level::TRACE] {
                        assert!(!enables(&t, target, l), "{level:?} {target} {l:?}");
                    }
                    assert_eq!(
                        enables(&t, target, Level::WARN),
                        level != LogLevel::Off && level != LogLevel::Error,
                        "{level:?} {target}"
                    );
                }
            }
        }
    }

    /// The same, end to end: russh's lines at trace with a marker host
    /// never reach the writer, while Seaquel's own debug lines do.
    #[test]
    fn a_russh_host_marker_never_reaches_the_log() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(move || writer.clone())
                    .with_ansi(false),
            )
            .with(log_filter(LogLevel::Trace));
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(target: "russh_keys::known_hosts", host_port = "[marker-bastion-f6]:2222");
            tracing::trace!(target: "russh::client", "server_public_Key marker-bastion-f6");
            tracing::info!(target: "russh::client", "marker-bastion-f6");
            tracing::debug!(target: "seaquel_core::ssh", "activity marker-own-line");
        });
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(!out.contains("marker-bastion-f6"), "{out}");
        assert!(out.contains("marker-own-line"), "{out}");
    }

    #[test]
    fn held_targets_pass_at_warn_at_most_and_the_rest_still_drop() {
        let held = ["rmcp", "ratatui", "crossterm"];
        for level in LEVELS {
            let t = log_filter_holding(level, &held);
            for target in held.iter().chain(&["tiberius"]) {
                let sub = format!("{target}::inner");
                for l in [Level::INFO, Level::DEBUG, Level::TRACE] {
                    assert!(!enables(&t, target, l), "{level:?} {target} {l:?}");
                    assert!(!enables(&t, &sub, l), "{level:?} {sub} {l:?}");
                }
                assert_eq!(
                    enables(&t, target, Level::WARN),
                    level != LogLevel::Off && level != LogLevel::Error,
                    "{level:?} {target}"
                );
            }
            for l in [Level::ERROR, Level::WARN, Level::TRACE] {
                assert!(!enables(&t, "sqlx::query", l), "{level:?} {l:?}");
                assert!(!enables(&t, "sqlx::postgres::notice", l), "{level:?}");
                assert!(!enables(&t, "tiberius::tds::stream::token", l), "{level:?}");
                assert!(!enables(&t, "sqlparser", l), "{level:?}");
            }
        }
        // Everything else keeps the level.
        let t = log_filter_holding(LogLevel::Debug, &held);
        assert!(enables(&t, "seaquel_tui::runtime", Level::DEBUG));
        // The plain filter doesn't hold them.
        assert!(enables(&log_filter(LogLevel::Debug), "rmcp", Level::DEBUG));
    }
}
