//! The shared-project git calls' types: what `seaquel-git` takes and returns.
//!
//! The JSON of [`GitRepoStatus`] and [`GitSyncResult`] is the snake_case
//! shape `src-tauri`'s git commands sent before phase 3, which
//! `src/lib/services/git.ts` maps to the app's camelCase types.
//! [`GitCredentials`] keeps the snake_case keys it had as a command argument.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How to authenticate against a remote. Every field is optional; see
/// `seaquel-git`'s credential chain for the order they're tried in.
///
/// `Debug` never shows the password or the passphrase.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GitCredentials {
    /// For HTTPS.
    pub username: Option<String>,
    /// A password or token, for HTTPS.
    pub password: Option<String>,
    /// A private key file, for SSH.
    pub ssh_key_path: Option<String>,
    /// The key file's passphrase.
    pub ssh_passphrase: Option<String>,
}

impl fmt::Debug for GitCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn redacted(v: &Option<String>) -> Option<&'static str> {
            v.as_ref().map(|_| "<redacted>")
        }
        f.debug_struct("GitCredentials")
            .field("username", &self.username)
            .field("password", &redacted(&self.password))
            .field("ssh_key_path", &self.ssh_key_path)
            .field("ssh_passphrase", &redacted(&self.ssh_passphrase))
            .finish()
    }
}

/// A repository's working tree and its standing against `origin`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GitRepoStatus {
    pub is_clean: bool,
    pub pending_changes: u32,
    pub ahead_by: u32,
    pub behind_by: u32,
    pub has_conflicts: bool,
    pub current_branch: String,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
    /// Files with unresolved conflicts (not in `modified_files`). Added in
    /// phase 3, after the fields the Tauri command sent.
    pub conflict_files: Vec<String>,
}

/// What a pull or push did. A pull that stops on conflicts is
/// `success: false` with the conflicting paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GitSyncResult {
    pub success: bool,
    pub message: String,
    pub conflicts: Vec<String>,
    pub files_changed: Vec<String>,
}

/// The three sides of a conflicted file. A side that doesn't exist (a file
/// added on both sides has no base) is an empty string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GitConflictContent {
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_password_and_passphrase() {
        let creds = GitCredentials {
            username: Some("alice".into()),
            password: Some("hunter2-token".into()),
            ssh_key_path: Some("/k/id_ed25519".into()),
            ssh_passphrase: Some("open-sesame".into()),
        };
        let shown = format!("{creds:?} {creds:#?}");
        assert!(!shown.contains("hunter2-token"), "{shown}");
        assert!(!shown.contains("open-sesame"), "{shown}");
        assert!(shown.contains("alice") && shown.contains("/k/id_ed25519"));
        assert!(shown.contains("<redacted>"));
    }

    /// The JSON `src-tauri`'s commands sent, which `services/git.ts` read as
    /// `RustRepoStatus` and `RustSyncResult`: same keys, same order, numbers
    /// as plain JSON numbers. `conflict_files` is new, added last on purpose.
    #[test]
    fn repo_status_and_sync_result_json_is_unchanged() {
        let status = GitRepoStatus {
            is_clean: false,
            pending_changes: 2,
            ahead_by: 1,
            behind_by: 3,
            has_conflicts: true,
            current_branch: "main".into(),
            modified_files: vec!["a.sql".into()],
            untracked_files: vec!["b.sql".into()],
            conflict_files: vec!["c.sql".into()],
        };
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            r#"{"is_clean":false,"pending_changes":2,"ahead_by":1,"behind_by":3,"has_conflicts":true,"current_branch":"main","modified_files":["a.sql"],"untracked_files":["b.sql"],"conflict_files":["c.sql"]}"#
        );

        let sync = GitSyncResult {
            success: false,
            message: "Merge conflicts detected".into(),
            conflicts: vec!["q.sql".into()],
            files_changed: vec![],
        };
        assert_eq!(
            serde_json::to_string(&sync).unwrap(),
            r#"{"success":false,"message":"Merge conflicts detected","conflicts":["q.sql"],"files_changed":[]}"#
        );

        let conflict = GitConflictContent {
            base: "b".into(),
            ours: "o".into(),
            theirs: "t".into(),
        };
        assert_eq!(
            serde_json::to_string(&conflict).unwrap(),
            r#"{"base":"b","ours":"o","theirs":"t"}"#
        );
    }

    #[test]
    fn credentials_keep_their_snake_case_keys() {
        let creds: GitCredentials = serde_json::from_str(
            r#"{"username":"u","password":"p","ssh_key_path":"k","ssh_passphrase":null}"#,
        )
        .unwrap();
        assert_eq!(creds.ssh_key_path.as_deref(), Some("k"));
        assert_eq!(creds.ssh_passphrase, None);
    }
}
