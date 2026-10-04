//! The signals that end a terminal binary (moved from `seaquel-cli`'s
//! `mcp.rs`). The MCP server ends on SIGINT and SIGTERM; the TUI on
//! SIGTERM, SIGHUP (a closed terminal or SSH session) and SIGINT from
//! outside, since Ctrl+C reaches it as a key in raw mode.
//!
//! [`ShutdownSignals::install`] takes the handlers at once, so a signal
//! that arrives before anything waits (the TUI's startup) is kept, not
//! fatal; [`shutdown_signal`] installs and waits in one call.

/// A signal [`shutdown_signal`] can wait for. Off Unix every set waits for
/// Ctrl+C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    Interrupt,
    Terminate,
    Hangup,
}

/// Installed handlers for a set of signals. Needs a tokio runtime.
#[derive(Debug)]
pub struct ShutdownSignals {
    #[cfg(unix)]
    streams: Vec<(tokio::signal::unix::Signal, &'static str)>,
}

impl ShutdownSignals {
    /// Installs a handler for each of `signals`; `None` (with a warning) if
    /// one can't be installed, in which case the default actions (exit)
    /// stay.
    pub fn install(signals: &[ShutdownSignal]) -> Option<ShutdownSignals> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut streams = Vec::with_capacity(signals.len());
            for which in signals {
                let (kind, name) = match which {
                    ShutdownSignal::Interrupt => (SignalKind::interrupt(), "SIGINT"),
                    ShutdownSignal::Terminate => (SignalKind::terminate(), "SIGTERM"),
                    ShutdownSignal::Hangup => (SignalKind::hangup(), "SIGHUP"),
                };
                match signal(kind) {
                    Ok(stream) => streams.push((stream, name)),
                    Err(e) => {
                        log::warn!("Can't handle {name}: {e}");
                        return None;
                    }
                }
            }
            Some(ShutdownSignals { streams })
        }
        // Windows has no signals to install: whatever the set, `recv`
        // waits for Ctrl+C (and console close ends the process). tokio
        // registers that handler lazily, on `recv`'s first poll, so a
        // Ctrl+C before the first wait isn't caught; the TUI reads Ctrl+C
        // as a key in raw mode anyway.
        #[cfg(not(unix))]
        {
            let _ = signals;
            Some(ShutdownSignals {})
        }
    }

    /// Waits for one of the signals (one that arrived since
    /// [`ShutdownSignals::install`] counts) and names it.
    pub async fn recv(&mut self) -> &'static str {
        #[cfg(unix)]
        {
            if self.streams.is_empty() {
                return std::future::pending().await;
            }
            std::future::poll_fn(|cx| {
                for (stream, name) in &mut self.streams {
                    if stream.poll_recv(cx).is_ready() {
                        return std::task::Poll::Ready(*name);
                    }
                }
                std::task::Poll::Pending
            })
            .await
        }
        #[cfg(not(unix))]
        {
            match tokio::signal::ctrl_c().await {
                Ok(()) => "Ctrl+C",
                Err(e) => {
                    log::warn!("Can't handle Ctrl+C: {e}");
                    std::future::pending().await
                }
            }
        }
    }
}

/// Waits for one of `signals` and names it; `None` if a handler can't be
/// installed, in which case the default action (exit) stays.
pub async fn shutdown_signal(signals: &[ShutdownSignal]) -> Option<&'static str> {
    let mut installed = ShutdownSignals::install(signals)?;
    Some(installed.recv().await)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    fn send(signal: &str) {
        let status = std::process::Command::new("kill")
            .args([signal, &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }

    // One test, so the signals are never in flight at once.
    #[tokio::test]
    async fn names_the_signal_it_got() {
        for (which, flag, name) in [
            (ShutdownSignal::Hangup, "-HUP", "SIGHUP"),
            (ShutdownSignal::Terminate, "-TERM", "SIGTERM"),
        ] {
            let waiting = tokio::spawn(async move { shutdown_signal(&[which]).await });
            // Let the handler install before the signal goes out.
            tokio::time::sleep(Duration::from_millis(100)).await;
            send(flag);
            let got = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("the signal arrives")
                .unwrap();
            assert_eq!(got, Some(name));
        }

        // Installed first, waited on later: the signal is kept.
        let mut installed =
            ShutdownSignals::install(&[ShutdownSignal::Terminate, ShutdownSignal::Hangup]).unwrap();
        send("-HUP");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let got = tokio::time::timeout(Duration::from_secs(5), installed.recv())
            .await
            .expect("the early signal was kept");
        assert_eq!(got, "SIGHUP");
    }
}
