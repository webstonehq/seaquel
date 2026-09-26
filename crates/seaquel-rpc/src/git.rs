//! The `git` group of the workspace RPC: shared projects' repositories.
//!
//! The desktop serves it with [`dispatch_git`], which needs no storage. The
//! web server has no shared projects, so `dispatch_workspace` answers
//! `NOT_SUPPORTED` for it.
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
    rename_all_fields = "camelCase"
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

/// Run one git call. The desktop routes the `git` group here, before any
/// storage opens. Logs like `dispatch_workspace`: the method, never the
/// params.
#[cfg(feature = "git")]
pub async fn dispatch_git(
    git: &seaquel_core::git::Git,
    req: GitRequest,
) -> Result<GitResponse, RpcError> {
    let method = req.method();
    crate::workspace::logged("git", method, async move {
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
            } => GitResponse::ResolveConflict(
                git.resolve_conflict(&path, &file_path, &resolution)
                    .await
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
    })
    .await
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
