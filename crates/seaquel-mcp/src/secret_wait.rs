//! Time spent reading secrets, which the per-call timeout leaves out.
//!
//! On macOS the first read of a keychain item from `seaquel-cli` can show a
//! prompt ("seaquel-cli wants to use your confidential information…") that
//! waits for the user. That wait isn't the database's, so it shouldn't use up
//! the call's 60 s: [`SecretWait::watch`] wraps the workspace's secret store,
//! and [`crate::ServerOptions::with_secret_wait`] hands the same
//! `SecretWait` to the server, whose timeout then adds the time any read was
//! pending. A read pending longer than [`SecretWait::limit`] (5 minutes by
//! default: a prompt nobody answers) ends the call with `TIMEOUT` and a
//! message about the prompt.
//!
//! The wait is counted per server, not per call: while one call waits on a
//! prompt, every other running call gets the same extension. That only ever
//! makes a timeout later, never earlier.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use seaquel_core::secrets::{SecretError, SecretStore};

/// How long one secret read may stay pending by default.
pub const DEFAULT_SECRET_WAIT_LIMIT: Duration = Duration::from_secs(300);

/// Counts the time secret reads were pending. Share one between the wrapped
/// store ([`SecretWait::watch`]) and the server's options.
#[derive(Debug)]
pub struct SecretWait {
    limit: Duration,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Reads in progress.
    pending: usize,
    /// When `pending` last went from 0 to 1.
    since: Option<Instant>,
    /// The time some read was pending, up to `since`.
    total: Duration,
}

impl SecretWait {
    /// A `SecretWait` with the default limit.
    pub fn new() -> Arc<Self> {
        Self::with_limit(DEFAULT_SECRET_WAIT_LIMIT)
    }

    /// A `SecretWait` whose reads may stay pending for `limit`.
    pub fn with_limit(limit: Duration) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::default(),
        })
    }

    /// How long one read may stay pending before its call gives up.
    pub fn limit(&self) -> Duration {
        self.limit
    }

    /// `store`, with every read counted here.
    pub fn watch(self: &Arc<Self>, store: Arc<dyn SecretStore>) -> Arc<dyn SecretStore> {
        Arc::new(Watched {
            inner: store,
            wait: self.clone(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(self: &Arc<Self>) -> Pending {
        let mut state = self.state();
        if state.pending == 0 {
            state.since = Some(Instant::now());
        }
        state.pending += 1;
        Pending(self.clone())
    }

    fn end(&self) {
        let mut state = self.state();
        state.pending = state.pending.saturating_sub(1);
        if state.pending == 0 {
            if let Some(since) = state.since.take() {
                state.total += since.elapsed();
            }
        }
    }

    /// The total time some read was pending, the current one included.
    pub(crate) fn waited(&self) -> Duration {
        let state = self.state();
        state.total + state.since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// How long the reads in progress have been pending, if any are.
    pub(crate) fn pending_for(&self) -> Option<Duration> {
        self.state().since.map(|s| s.elapsed())
    }
}

/// Ends a read's count when the read finishes or its future is dropped.
struct Pending(Arc<SecretWait>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.end();
    }
}

struct Watched {
    inner: Arc<dyn SecretStore>,
    wait: Arc<SecretWait>,
}

#[seaquel_runtime::async_trait]
impl SecretStore for Watched {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let _pending = self.wait.begin();
        self.inner.get(key).await
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.inner.set(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seaquel_core::secrets::MemoryStore;

    #[tokio::test]
    async fn reads_are_counted_and_pass_through() {
        let wait = SecretWait::new();
        let inner = MemoryStore::new();
        inner.set("db:x", "pw").await.unwrap();
        let store = wait.watch(Arc::new(inner));
        assert_eq!(store.get("db:x").await.unwrap().as_deref(), Some("pw"));
        assert_eq!(wait.pending_for(), None);
        let first = wait.waited();

        let pending = wait.begin();
        assert!(wait.pending_for().is_some());
        std::thread::sleep(Duration::from_millis(20));
        assert!(wait.waited() >= first + Duration::from_millis(20));
        drop(pending);
        assert_eq!(wait.pending_for(), None);
        assert!(wait.waited() >= first + Duration::from_millis(20));
    }
}
