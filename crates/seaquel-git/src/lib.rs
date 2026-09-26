//! Seaquel's git operations for shared projects: clone, init, pull with
//! conflicts, push, status with ahead/behind counts, commit, conflict
//! resolution and remotes, over libgit2. libgit2 blocks, so every call runs on
//! a blocking thread. Core exposes it behind its `git` feature.
//!
//! [`Git`] carries the home dir the credential chain looks for default SSH
//! keys in (see the `credentials` module), so tests never touch `~/.ssh`.
//! Commits are signed with the repo's git config (repo, then global), or as
//! `Seaquel User <seaquel@local>` when none has a user.
//!
//! Errors are [`GitError`]s with the codes the Tauri commands had:
//! `CLONE_ERROR`, `INIT_ERROR`, `REPO_OPEN_ERROR`, `REPO_ERROR`,
//! `REMOTE_ERROR`, `PULL_ERROR`, `MERGE_ERROR`, `INDEX_ERROR`,
//! `CONFLICT_ERROR`, `STAGE_ERROR`, `COMMIT_ERROR` and `PUSH_ERROR`, plus
//! `GIT_TASK_ERROR` when the blocking task itself fails. A push that isn't a
//! fast-forward says [`PUSH_REJECTED_NON_FAST_FORWARD`]; commit refuses with
//! `CONFLICT_ERROR` while files are conflicted.

use std::fmt;
use std::path::{Path, PathBuf};

mod credentials;
mod ops;

pub use ops::PUSH_REJECTED_NON_FAST_FORWARD;
pub use seaquel_types::git::{GitConflictContent, GitCredentials, GitRepoStatus, GitSyncResult};

/// A failed git call: a code from the crate docs and a message that includes
/// libgit2's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub code: String,
    pub message: String,
}

impl GitError {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GitError {}

/// The git client. Cheap to clone; holds no repository open between calls.
#[derive(Debug, Clone)]
pub struct Git {
    home: Option<PathBuf>,
}

impl Git {
    /// A client whose credential chain looks for `.ssh/id_ed25519` and
    /// `.ssh/id_rsa` under `home_dir`, or skips them with `None`.
    pub fn new(home_dir: Option<PathBuf>) -> Self {
        Self { home: home_dir }
    }

    /// A client for the current user's home dir.
    pub fn from_env() -> Self {
        Self::new(dirs::home_dir())
    }

    pub fn home_dir(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Clone `url` into `path`.
    pub async fn clone_repo(
        &self,
        url: &str,
        path: impl AsRef<Path>,
        credentials: Option<GitCredentials>,
    ) -> Result<(), GitError> {
        let (url, path, home) = (url.to_string(), owned(path), self.home.clone());
        blocking(move || ops::clone_repo(&url, &path, credentials, home)).await
    }

    /// Create a repository at `path`.
    pub async fn init_repo(&self, path: impl AsRef<Path>) -> Result<(), GitError> {
        let path = owned(path);
        blocking(move || ops::init_repo(&path)).await
    }

    /// Fetch the current branch from `origin` and merge it: fast-forward,
    /// a merge commit, or `success: false` with the conflicting paths.
    pub async fn pull_repo(
        &self,
        path: impl AsRef<Path>,
        credentials: Option<GitCredentials>,
    ) -> Result<GitSyncResult, GitError> {
        let (path, home) = (owned(path), self.home.clone());
        blocking(move || ops::pull_repo(&path, credentials, home)).await
    }

    /// Push the current branch to the same name on `origin`.
    pub async fn push_repo(
        &self,
        path: impl AsRef<Path>,
        credentials: Option<GitCredentials>,
    ) -> Result<GitSyncResult, GitError> {
        let (path, home) = (owned(path), self.home.clone());
        blocking(move || ops::push_repo(&path, credentials, home)).await
    }

    pub async fn repo_status(&self, path: impl AsRef<Path>) -> Result<GitRepoStatus, GitError> {
        let path = owned(path);
        blocking(move || ops::repo_status(&path)).await
    }

    /// Stage everything and commit it. Returns the commit id. During a merge
    /// the commit gets the merged commits as parents and ends the merge.
    pub async fn commit_changes(
        &self,
        path: impl AsRef<Path>,
        message: &str,
    ) -> Result<String, GitError> {
        let (path, message) = (owned(path), message.to_string());
        blocking(move || ops::commit_changes(&path, &message)).await
    }

    /// Write `resolution` to the conflicted `file_path` (relative to the
    /// repo) and stage it.
    pub async fn resolve_conflict(
        &self,
        path: impl AsRef<Path>,
        file_path: &str,
        resolution: &str,
    ) -> Result<(), GitError> {
        let (path, file_path, resolution) =
            (owned(path), file_path.to_string(), resolution.to_string());
        blocking(move || ops::resolve_conflict(&path, &file_path, &resolution)).await
    }

    /// The base, ours and theirs of a conflicted file; empty strings for a
    /// file that isn't conflicted.
    pub async fn conflict_content(
        &self,
        path: impl AsRef<Path>,
        file_path: &str,
    ) -> Result<GitConflictContent, GitError> {
        let (path, file_path) = (owned(path), file_path.to_string());
        blocking(move || ops::conflict_content(&path, &file_path)).await
    }

    /// Point `origin` at `url`, replacing it if it exists.
    pub async fn set_remote(&self, path: impl AsRef<Path>, url: &str) -> Result<(), GitError> {
        let (path, url) = (owned(path), url.to_string());
        blocking(move || ops::set_remote(&path, &url)).await
    }

    /// `origin`'s URL, or `None` without one.
    pub async fn remote_url(&self, path: impl AsRef<Path>) -> Result<Option<String>, GitError> {
        let path = owned(path);
        blocking(move || ops::remote_url(&path)).await
    }
}

fn owned(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().to_path_buf()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, GitError> + Send + 'static,
) -> Result<T, GitError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| GitError::new("GIT_TASK_ERROR", format!("Git task failed: {e}")))?
}
