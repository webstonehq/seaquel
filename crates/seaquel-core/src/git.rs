//! Git for shared projects (`seaquel-git`), for interfaces and `seaquel-rpc`,
//! which may not depend on it directly. [`Git`] runs each call on a blocking
//! thread; build it with [`Git::from_env`] (the user's home, for the default
//! SSH keys) or [`Git::new`] with a home dir of your own (tests).

pub use seaquel_git::{
    Git, GitConflictContent, GitCredentials, GitError, GitRepoStatus, GitSyncResult,
};

impl From<GitError> for crate::CoreError {
    fn from(e: GitError) -> Self {
        Self::new(e.code, e.message)
    }
}

use std::path::Path;
use std::sync::{Arc, PoisonError};

/// A repo path as stored and compared: without trailing separators. Every
/// caller of [`crate::Core::repo_lock`] for a git call or the projection
/// keys the lock with it, so both routes share a key by construction.
pub fn repo_path_key(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        path.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Held while a repo's working tree is read or written (phase 5e, Decision
/// 38). Dropping it lets the next caller in.
pub struct RepoLock {
    _guard: futures::lock::OwnedMutexGuard<()>,
}

impl std::fmt::Debug for RepoLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepoLock")
    }
}

impl crate::Core {
    /// Waits for and takes the lock of the repo at `path`. The shared
    /// projection's syncs and publishes take it, and so must every git call
    /// that changes the working tree (pull, commit, conflict resolution),
    /// so a checkout never races a file write. Always take it before the
    /// storage's write lock, never while holding it.
    pub async fn repo_lock(&self, path: &Path) -> RepoLock {
        // Canonicalized off the async thread, before the map's lock.
        let key = seaquel_git::tree::lock_key(path).await;
        let mutex = {
            let mut locks = self
                .repo_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            Arc::clone(locks.entry(key).or_default())
        };
        RepoLock {
            _guard: mutex.lock_owned().await,
        }
    }

    /// The test hook ([`crate::CoreBuilder::file_write_hook`]).
    pub(crate) fn file_hook(&self) -> Option<seaquel_git::tree::WriteHook> {
        self.file_hook.clone()
    }
}
