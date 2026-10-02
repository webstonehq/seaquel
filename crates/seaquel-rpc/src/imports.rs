//! The `imports` group of the workspace RPC (phase 5e, Decision 47):
//! connections from TablePlus and DBeaver, read and imported by Core
//! (`Workspace::import_candidates`, `import_create`).
//!
//! ```json
//! {"method":"imports","params":{"method":"candidates","params":{"source":"dbeaver","projectId":"p"}}}
//! {"method":"imports","result":{"method":"candidates","result":{"found":true,"candidates":[…]}}}
//! ```
//!
//! - **Desktop only**, like the `shared` group: refused with
//!   `NOT_SUPPORTED` on a Core without `LocalFiles::Allowed`, and without
//!   the `imports` feature.
//! - `path` is optional: without it Core reads the tool's default location
//!   under the home it was built with (`ImportPaths`).
//! - `candidates` answers `{found: false}`, `{found: true, unreadable}` or
//!   the candidates; `create` reads the file again and imports the keys
//!   given, answering one outcome per key (`IMPORT_SOURCE_UNREADABLE` when
//!   the file is gone, `INVALID_ARGUMENT` for a key whose candidate has a
//!   problem).
//!
//! `Debug` shows the method and source only: never a path or a key.

use std::fmt;

use seaquel_core::domain::imports::{ImportCandidates, ImportSource};
use seaquel_core::domain::shared_api::ImportOutcome;
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use serde::{Deserialize, Serialize};

use crate::workspace::RpcError;

/// An `imports` call.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ImportsRequest {
    /// The import dialog's list. Read-only.
    Candidates {
        source: ImportSource,
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        path: Option<String>,
    },
    /// Import the candidates `keys` name into the project.
    Create {
        source: ImportSource,
        project_id: String,
        keys: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        path: Option<String>,
    },
}

impl ImportsRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::Candidates { .. } => "candidates",
            Self::Create { .. } => "create",
        }
    }
}

/// The method, the source and how many keys: a path names the user, a key
/// can be a connection's id in the other tool.
impl fmt::Debug for ImportsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Candidates { source, path, .. } => f
                .debug_struct("Candidates")
                .field("source", source)
                .field("path", &path.is_some())
                .finish_non_exhaustive(),
            Self::Create {
                source, keys, path, ..
            } => f
                .debug_struct("Create")
                .field("source", source)
                .field("keys", &keys.len())
                .field("path", &path.is_some())
                .finish_non_exhaustive(),
        }
    }
}

/// An `imports` call's result.
// Not `Deserialize`: results only go out.
#[derive(Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ImportsResponse {
    Candidates(ImportCandidates),
    Create(Seqd<ImportOutcome>),
}

/// `ImportCandidates`' and `ImportOutcome`'s own `Debug`s show no name,
/// host or key.
impl fmt::Debug for ImportsResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Candidates(c) => f.debug_tuple("Candidates").field(c).finish(),
            Self::Create(r) => f
                .debug_struct("Create")
                .field("value", &r.value)
                .field("seq", &r.seq)
                .finish(),
        }
    }
}

#[cfg(not(feature = "imports"))]
pub(crate) async fn imports(
    _: &Core,
    _: &Workspace,
    _: ImportsRequest,
    _: &WriteOrigin,
) -> Result<ImportsResponse, RpcError> {
    Err(RpcError::not_supported("Imports"))
}

/// Serve an `imports` call on `ws`. The caller checked `LocalFiles`; Core
/// checks it again.
#[cfg(feature = "imports")]
pub(crate) async fn imports(
    core: &Core,
    ws: &Workspace,
    req: ImportsRequest,
    origin: &WriteOrigin,
) -> Result<ImportsResponse, RpcError> {
    Ok(match req {
        ImportsRequest::Candidates {
            source,
            project_id,
            path,
        } => ImportsResponse::Candidates(
            ws.import_candidates(core, source, &project_id, path.as_deref())
                .await?,
        ),
        ImportsRequest::Create {
            source,
            project_id,
            keys,
            path,
        } => ImportsResponse::Create(
            ws.import_create(core, origin, source, &project_id, &keys, path.as_deref())
                .await?,
        ),
    })
}
