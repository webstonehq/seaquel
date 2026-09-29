//! The `library` group of the workspace RPC (phase 5d-1): saved
//! connections, projects, custom labels, saved queries and their versions,
//! written through Core (`Workspace::create_connection`, …).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"library","params":{"method":"connectionRemove","params":{"id":"conn-…"}}}
//! {"method":"library","result":{"method":"connectionRemove","result":{"value":null,"seq":{"epoch":"…","n":7}}}}
//! ```
//!
//! Every result is a `Seqd` (`{value, seq}`, Decision 17): a write's `seq`
//! is its own number, a list's the published number it's at least as new
//! as. Request fields are camelCase; drafts and patches are
//! `seaquel_core::domain::library`'s (a patch field left out is kept, a
//! `Clearable` one sent as `null` is cleared). `connectionCreate` and
//! `connectionUpdate` take `secrets` on the desktop only (the web answers
//! `NOT_SUPPORTED`, its vault stays in the browser). `Debug` never shows a
//! secret, a name, a host, a string or query text.
//!
//! Each write takes the caller's [`WriteOrigin`] (the desktop's webview
//! label, the web's `X-Seaquel-Origin`), which its `StorageChanged` event
//! carries so the writer's own window can ignore it.
//!
//! Without the `storage` feature every method answers `NOT_SUPPORTED`.

use seaquel_core::domain::library::{
    ConnectionDraft, ConnectionPatch, LabelDraft, LabelPatch, LabelRemoved, ProjectDraft,
    ProjectPatch, ProjectRemoved, SavedQueryDraft, SavedQueryPatch, SavedQueryUpdated,
    SecretChanges,
};
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use seaquel_types::storage::{
    ConnectionLabel, PersistedConnection, PersistedProject, PersistedQueryVersion,
    PersistedSavedQuery,
};
use serde::{Deserialize, Serialize};

use crate::workspace::RpcError;

/// A `library` call.
#[allow(clippy::large_enum_variant)] // See `Request`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LibraryRequest {
    ConnectionsList,
    ProjectsList,
    SavedQueriesList {
        project_id: String,
    },
    /// Every version of the project's saved queries.
    QueryVersionsList {
        project_id: String,
    },
    ConnectionCreate {
        connection: ConnectionDraft,
        /// Desktop only. Absent: none.
        #[serde(default, skip_serializing_if = "SecretChanges::is_empty")]
        #[cfg_attr(feature = "ts", ts(as = "Option<SecretChanges>", optional))]
        secrets: SecretChanges,
    },
    ConnectionUpdate {
        id: String,
        patch: ConnectionPatch,
        /// Desktop only. Absent: none.
        #[serde(default, skip_serializing_if = "SecretChanges::is_empty")]
        #[cfg_attr(feature = "ts", ts(as = "Option<SecretChanges>", optional))]
        secrets: SecretChanges,
    },
    ConnectionRemove {
        id: String,
    },
    ProjectCreate {
        project: ProjectDraft,
    },
    /// Creates the default project when there is none; the result is the
    /// projects inserted (none or the default one).
    ProjectEnsureDefault,
    ProjectUpdate {
        id: String,
        patch: ProjectPatch,
    },
    ProjectRemove {
        id: String,
    },
    LabelCreate {
        project_id: String,
        label: LabelDraft,
    },
    LabelUpdate {
        project_id: String,
        label_id: String,
        patch: LabelPatch,
    },
    LabelRemove {
        project_id: String,
        label_id: String,
    },
    SavedQueryCreate {
        query: SavedQueryDraft,
    },
    SavedQueryUpdate {
        id: String,
        patch: SavedQueryPatch,
    },
    SavedQueryRemove {
        id: String,
    },
}

impl LibraryRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            LibraryRequest::ConnectionsList => "connectionsList",
            LibraryRequest::ProjectsList => "projectsList",
            LibraryRequest::SavedQueriesList { .. } => "savedQueriesList",
            LibraryRequest::QueryVersionsList { .. } => "queryVersionsList",
            LibraryRequest::ConnectionCreate { .. } => "connectionCreate",
            LibraryRequest::ConnectionUpdate { .. } => "connectionUpdate",
            LibraryRequest::ConnectionRemove { .. } => "connectionRemove",
            LibraryRequest::ProjectCreate { .. } => "projectCreate",
            LibraryRequest::ProjectEnsureDefault => "projectEnsureDefault",
            LibraryRequest::ProjectUpdate { .. } => "projectUpdate",
            LibraryRequest::ProjectRemove { .. } => "projectRemove",
            LibraryRequest::LabelCreate { .. } => "labelCreate",
            LibraryRequest::LabelUpdate { .. } => "labelUpdate",
            LibraryRequest::LabelRemove { .. } => "labelRemove",
            LibraryRequest::SavedQueryCreate { .. } => "savedQueryCreate",
            LibraryRequest::SavedQueryUpdate { .. } => "savedQueryUpdate",
            LibraryRequest::SavedQueryRemove { .. } => "savedQueryRemove",
        }
    }
}

/// A `library` call's result, as `{"method": …, "result": {value, seq}}`.
// Not `Deserialize`: results only go out.
#[derive(Debug, Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum LibraryResponse {
    ConnectionsList(Seqd<Vec<PersistedConnection>>),
    ProjectsList(Seqd<Vec<PersistedProject>>),
    SavedQueriesList(Seqd<Vec<PersistedSavedQuery>>),
    QueryVersionsList(Seqd<Vec<PersistedQueryVersion>>),
    ConnectionCreate(Seqd<PersistedConnection>),
    ConnectionUpdate(Seqd<PersistedConnection>),
    ConnectionRemove(Seqd<()>),
    ProjectCreate(Seqd<PersistedProject>),
    ProjectEnsureDefault(Seqd<Vec<PersistedProject>>),
    ProjectUpdate(Seqd<PersistedProject>),
    ProjectRemove(Seqd<ProjectRemoved>),
    LabelCreate(Seqd<ConnectionLabel>),
    LabelUpdate(Seqd<ConnectionLabel>),
    LabelRemove(Seqd<LabelRemoved>),
    SavedQueryCreate(Seqd<PersistedSavedQuery>),
    SavedQueryUpdate(Seqd<SavedQueryUpdated>),
    SavedQueryRemove(Seqd<()>),
}

#[cfg(not(feature = "storage"))]
pub(crate) async fn library(
    _: &Core,
    _: &Workspace,
    _: LibraryRequest,
    _: &WriteOrigin,
) -> Result<LibraryResponse, RpcError> {
    Err(RpcError::not_supported("The library"))
}

/// Serve a `library` call on `ws`, its writes tagged with `origin`.
#[cfg(feature = "storage")]
pub(crate) async fn library(
    core: &Core,
    ws: &Workspace,
    req: LibraryRequest,
    origin: &WriteOrigin,
) -> Result<LibraryResponse, RpcError> {
    use LibraryRequest as Q;
    use LibraryResponse as R;

    Ok(match req {
        Q::ConnectionsList => R::ConnectionsList(ws.list_connections().await?),
        Q::ProjectsList => R::ProjectsList(ws.list_projects().await?),
        Q::SavedQueriesList { project_id } => {
            R::SavedQueriesList(ws.list_saved_queries(core, &project_id).await?)
        }
        Q::QueryVersionsList { project_id } => {
            R::QueryVersionsList(ws.list_query_versions(core, &project_id).await?)
        }
        Q::ConnectionCreate {
            connection,
            secrets,
        } => R::ConnectionCreate(
            ws.create_connection(core, origin, connection, secrets)
                .await?,
        ),
        Q::ConnectionUpdate { id, patch, secrets } => R::ConnectionUpdate(
            ws.update_connection(core, origin, &id, patch, secrets)
                .await?,
        ),
        Q::ConnectionRemove { id } => {
            R::ConnectionRemove(ws.remove_connection(core, origin, &id).await?)
        }
        Q::ProjectCreate { project } => {
            R::ProjectCreate(ws.create_project(core, origin, project).await?)
        }
        Q::ProjectEnsureDefault => {
            R::ProjectEnsureDefault(ws.ensure_default_project(core, origin).await?)
        }
        Q::ProjectUpdate { id, patch } => {
            R::ProjectUpdate(ws.update_project(core, origin, &id, patch).await?)
        }
        Q::ProjectRemove { id } => R::ProjectRemove(ws.remove_project(core, origin, &id).await?),
        Q::LabelCreate { project_id, label } => {
            R::LabelCreate(ws.create_label(core, origin, &project_id, label).await?)
        }
        Q::LabelUpdate {
            project_id,
            label_id,
            patch,
        } => R::LabelUpdate(
            ws.update_label(core, origin, &project_id, &label_id, patch)
                .await?,
        ),
        Q::LabelRemove {
            project_id,
            label_id,
        } => R::LabelRemove(
            ws.remove_label(core, origin, &project_id, &label_id)
                .await?,
        ),
        Q::SavedQueryCreate { query } => {
            R::SavedQueryCreate(ws.create_saved_query(core, origin, query).await?)
        }
        Q::SavedQueryUpdate { id, patch } => {
            R::SavedQueryUpdate(ws.update_saved_query(core, origin, &id, patch).await?)
        }
        Q::SavedQueryRemove { id } => {
            R::SavedQueryRemove(ws.remove_saved_query(core, origin, &id).await?)
        }
    })
}
