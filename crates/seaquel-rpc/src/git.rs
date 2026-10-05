//! The `git` group of the workspace RPC: shared projects' repositories.
//!
//! The desktop serves it with [`dispatch_git`]. The web server has no
//! shared projects, so `dispatch_workspace` answers `NOT_SUPPORTED` for it.
//!
//! **The repo lock (phase 5e).** Pull, push, commit and
//! conflict resolution change the working tree or the branch, so they run
//! under Core's per-repo lock, which the shared projection's syncs and
//! publishes take too: a checkout never races a file write. With the
//! desktop's storage workspace (and `LocalFiles`) they go through
//! `Workspace::shared_git_*`, which also record a successful pull's or
//! push's `lastSyncAt`; before storage opens they take the lock here and
//! record nothing. The other calls don't touch the tree's files.
//!
//! Paths are absolute paths to a repository's working tree, and `filePath` is
//! relative to it. Errors keep `seaquel-git`'s codes (`CLONE_ERROR`,
//! `PULL_ERROR`, `PUSH_ERROR`, …).

pub use seaquel_types::git::{GitConflictContent, GitCredentials, GitRepoStatus, GitSyncResult};
use serde::{Deserialize, Serialize};

#[cfg(feature = "git")]
use crate::RpcError;

/// A git call. Fields are camelCase; `credentials` may be left out.
/// `Debug` never shows a password or passphrase (see [`GitCredentials`]).
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum GitRequest {
    Clone {
        url: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        credentials: Option<GitCredentials>,
    },
    Init {
        path: String,
    },
    Pull {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        credentials: Option<GitCredentials>,
    },
    Push {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        credentials: Option<GitCredentials>,
    },
    Status {
        path: String,
    },
    /// Stage everything and commit; the result is the commit id.
    Commit {
        path: String,
        message: String,
    },
    ResolveConflict {
        path: String,
        file_path: String,
        resolution: String,
        /// Keep the side that deleted the file:
        /// the file is deleted and the deletion staged; `resolution` is
        /// ignored.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        #[cfg_attr(feature = "ts", ts(optional, as = "Option<bool>"))]
        delete: bool,
    },
    ConflictContent {
        path: String,
        file_path: String,
    },
    SetRemote {
        path: String,
        url: String,
    },
    RemoteUrl {
        path: String,
    },
}

impl GitRequest {
    /// Whether the call runs under the repo lock: pull, push, commit and
    /// conflict resolution. The desktop gives those its storage workspace.
    pub fn takes_repo_lock(&self) -> bool {
        matches!(
            self,
            Self::Pull { .. }
                | Self::Push { .. }
                | Self::Commit { .. }
                | Self::ResolveConflict { .. }
        )
    }

    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Clone { .. } => "clone",
            Self::Init { .. } => "init",
            Self::Pull { .. } => "pull",
            Self::Push { .. } => "push",
            Self::Status { .. } => "status",
            Self::Commit { .. } => "commit",
            Self::ResolveConflict { .. } => "resolveConflict",
            Self::ConflictContent { .. } => "conflictContent",
            Self::SetRemote { .. } => "setRemote",
            Self::RemoteUrl { .. } => "remoteUrl",
        }
    }
}

/// A git call's result, as `{"method": …, "result": …}`. Calls that return
/// nothing have `"result": null`, and `remoteUrl` is `null` without an
/// `origin`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum GitResponse {
    Clone(()),
    Init(()),
    Pull(GitSyncResult),
    Push(GitSyncResult),
    Status(GitRepoStatus),
    Commit(String),
    ResolveConflict(()),
    ConflictContent(GitConflictContent),
    SetRemote(()),
    RemoteUrl(Option<String>),
}

/// Run one git call. The desktop routes the `git` group here, with its
/// storage workspace for the calls that take the repo lock
/// ([`GitRequest::takes_repo_lock`]) once it's open, and `None` before (or
/// when it can't open). Logs like `dispatch_workspace`: the method, never
/// the params.
#[cfg(feature = "git")]
pub async fn dispatch_git(
    core: &seaquel_core::Core,
    ws: Option<&seaquel_core::Workspace>,
    git: &seaquel_core::git::Git,
    req: GitRequest,
    origin: &seaquel_core::WriteOrigin,
) -> Result<GitResponse, RpcError> {
    let method = req.method();
    crate::workspace::logged("git", method, async move {
        // Through the workspace: the lock, and `lastSyncAt` after a pull or
        // push.
        #[cfg(feature = "storage")]
        if let Some(ws) = ws.filter(|_| core.local_files().is_some()) {
            match req {
                GitRequest::Pull { path, credentials } => {
                    return Ok(GitResponse::Pull(
                        ws.shared_git_pull(core, origin, git, &path, credentials)
                            .await?,
                    ))
                }
                GitRequest::Push { path, credentials } => {
                    return Ok(GitResponse::Push(
                        ws.shared_git_push(core, origin, git, &path, credentials)
                            .await?,
                    ))
                }
                GitRequest::Commit { path, message } => {
                    return Ok(GitResponse::Commit(
                        ws.shared_git_commit(core, git, &path, &message).await?,
                    ))
                }
                GitRequest::ResolveConflict {
                    path,
                    file_path,
                    resolution,
                    delete,
                } => {
                    let resolution = (!delete).then_some(resolution.as_str());
                    return Ok(GitResponse::ResolveConflict(
                        ws.shared_git_resolve(core, git, &path, &file_path, resolution)
                            .await?,
                    ));
                }
                other => return plain(core, git, other).await,
            }
        }
        #[cfg(not(feature = "storage"))]
        let _ = (ws, origin);
        plain(core, git, req).await
    })
    .await
}

/// One git call with no workspace: the calls that change the tree take the
/// repo lock and record nothing.
#[cfg(feature = "git")]
async fn plain(
    core: &seaquel_core::Core,
    git: &seaquel_core::git::Git,
    req: GitRequest,
) -> Result<GitResponse, RpcError> {
    let _lock = match &req {
        GitRequest::Pull { path, .. }
        | GitRequest::Push { path, .. }
        | GitRequest::Commit { path, .. }
        | GitRequest::ResolveConflict { path, .. } => {
            // Keyed as `Workspace::shared_git_*` key it, so both routes
            // share one lock for one repo.
            let key = seaquel_core::git::repo_path_key(path);
            Some(core.repo_lock(std::path::Path::new(&key)).await)
        }
        _ => None,
    };
    {
        let to_rpc = |e: seaquel_core::git::GitError| RpcError::new(e.code, e.message);
        Ok(match req {
            GitRequest::Clone {
                url,
                path,
                credentials,
            } => GitResponse::Clone(
                git.clone_repo(&url, &path, credentials)
                    .await
                    .map_err(to_rpc)?,
            ),
            GitRequest::Init { path } => {
                GitResponse::Init(git.init_repo(&path).await.map_err(to_rpc)?)
            }
            GitRequest::Pull { path, credentials } => {
                GitResponse::Pull(git.pull_repo(&path, credentials).await.map_err(to_rpc)?)
            }
            GitRequest::Push { path, credentials } => {
                GitResponse::Push(git.push_repo(&path, credentials).await.map_err(to_rpc)?)
            }
            GitRequest::Status { path } => {
                GitResponse::Status(git.repo_status(&path).await.map_err(to_rpc)?)
            }
            GitRequest::Commit { path, message } => {
                GitResponse::Commit(git.commit_changes(&path, &message).await.map_err(to_rpc)?)
            }
            GitRequest::ResolveConflict {
                path,
                file_path,
                resolution,
                delete,
            } => GitResponse::ResolveConflict(
                if delete {
                    git.resolve_conflict_deleted(&path, &file_path).await
                } else {
                    git.resolve_conflict(&path, &file_path, &resolution).await
                }
                .map_err(to_rpc)?,
            ),
            GitRequest::ConflictContent { path, file_path } => GitResponse::ConflictContent(
                git.conflict_content(&path, &file_path)
                    .await
                    .map_err(to_rpc)?,
            ),
            GitRequest::SetRemote { path, url } => {
                GitResponse::SetRemote(git.set_remote(&path, &url).await.map_err(to_rpc)?)
            }
            GitRequest::RemoteUrl { path } => {
                GitResponse::RemoteUrl(git.remote_url(&path).await.map_err(to_rpc)?)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_passwords_and_passphrases() {
        let req = GitRequest::Pull {
            path: "/repos/team".into(),
            credentials: Some(GitCredentials {
                username: Some("alice".into()),
                password: Some("ghp_SECRET_TOKEN".into()),
                ssh_key_path: Some("/k/id".into()),
                ssh_passphrase: Some("PASSPHRASE-9".into()),
            }),
        };
        let shown = format!("{req:?} {req:#?}");
        assert!(!shown.contains("ghp_SECRET_TOKEN"), "{shown}");
        assert!(!shown.contains("PASSPHRASE-9"), "{shown}");
        assert!(shown.contains("/repos/team"));
    }

    #[test]
    fn wire_shape() {
        let req: GitRequest = serde_json::from_str(
            r#"{"method":"resolveConflict","params":{"path":"/r","filePath":"q.sql","resolution":"x"}}"#,
        )
        .unwrap();
        assert_eq!(req.method(), "resolveConflict");

        // `credentials` may be missing or null.
        for body in [
            r#"{"method":"pull","params":{"path":"/r"}}"#,
            r#"{"method":"pull","params":{"path":"/r","credentials":null}}"#,
        ] {
            let req: GitRequest = serde_json::from_str(body).unwrap();
            assert!(matches!(
                req,
                GitRequest::Pull {
                    credentials: None,
                    ..
                }
            ));
        }
        let req: GitRequest = serde_json::from_str(
            r#"{"method":"clone","params":{"url":"u","path":"/r","credentials":{"username":"a","password":"p","ssh_key_path":null,"ssh_passphrase":null}}}"#,
        )
        .unwrap();
        let GitRequest::Clone { credentials, .. } = req else {
            panic!("not a clone")
        };
        assert_eq!(credentials.unwrap().password.as_deref(), Some("p"));

        assert_eq!(
            serde_json::to_string(&GitResponse::RemoteUrl(None)).unwrap(),
            r#"{"method":"remoteUrl","result":null}"#
        );
        assert_eq!(
            serde_json::to_string(&GitResponse::Init(())).unwrap(),
            r#"{"method":"init","result":null}"#
        );
    }
}
