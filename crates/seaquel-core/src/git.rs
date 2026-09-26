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
