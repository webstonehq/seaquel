//! The `library` group of the workspace RPC: saved connections, projects,
//! custom labels, saved queries and their versions (phase 5d-1), and
//! dashboards and their versions, saved workflows, AI chats and their
//! messages and a project's connection order (phase 5d-2), written through
//! Core (`Workspace::create_connection`, `Workspace::update_dashboard`, …).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"library","params":{"method":"connectionRemove","params":{"id":"conn-…"}}}
//! {"method":"library","result":{"method":"connectionRemove","result":{"value":null,"seq":{"epoch":"…","n":7}}}}
//! ```
//!
//! Every result is a `Seqd` (`{value, seq}`): a write's `seq`
//! is its own number, a list's the published number it's at least as new
//! as. Request fields are camelCase; drafts and patches are
//! `seaquel_core::domain::library`'s (a patch field left out is kept, a
//! `Clearable` one sent as `null` is cleared). `connectionCreate` and
//! `connectionUpdate` take `secrets` on the desktop only (the web answers
//! `NOT_SUPPORTED`, its vault stays in the browser). JSON bodies (a
//! dashboard's widgets, viewport and date filter, a workflow) cross as the
//! text that came in (`RawValue`), and Core keeps them byte for byte.
//! `Debug` shows the method only: never a secret, a name, a host, a
//! string, query text or JSON.
//!
//! Each write takes the caller's [`WriteOrigin`] (the desktop's webview
//! label, the web's `X-Seaquel-Origin`), which its `StorageChanged` event
//! carries so the writer's own window can ignore it.
//!
//! Without the `storage` feature every method answers `NOT_SUPPORTED`.

use std::fmt;

use seaquel_core::domain::library::{
    ConnectionDraft, ConnectionPatch, LabelDraft, LabelPatch, LabelRemoved, ProjectDraft,
    ProjectPatch, ProjectRemoved, SavedQueryDraft, SavedQueryPatch, SavedQueryUpdated,
    SecretChanges,
};
use seaquel_core::domain::state::{
    ChatDraft, ChatMessageDraft, ChatMessages, ChatPatch, DashboardDraft, DashboardPatch,
    DashboardUpdated, WorkflowDraft,
};
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use seaquel_types::storage::{
    ConnectionLabel, PersistedAIChat, PersistedConnection, PersistedDashboard,
    PersistedDashboardVersion, PersistedDashboardVersionMeta, PersistedProject,
    PersistedQueryVersion, PersistedSavedQuery, PersistedWorkflowMeta,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::workspace::RpcError;

/// A `library` call.
#[allow(clippy::large_enum_variant)] // See `Request`.
#[derive(Serialize, Deserialize)]
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

    // ── Phase 5d-2 ──
    /// A project's dashboards.
    DashboardsList {
        project_id: String,
    },
    /// Every version of the project's dashboards, without their
    /// snapshots.
    DashboardVersionsList {
        project_id: String,
    },
    /// One version of a dashboard, with its snapshot.
    DashboardVersionGet {
        dashboard_id: String,
        version_id: String,
    },
    DashboardCreate {
        dashboard: DashboardDraft,
    },
    /// Records a version of the previous state only when the patch says
    /// `captureVersion: true`.
    DashboardUpdate {
        id: String,
        patch: DashboardPatch,
    },
    DashboardRemove {
        id: String,
    },
    /// A project's saved workflows, without their bodies.
    WorkflowsList {
        project_id: String,
    },
    /// One saved workflow, today's `SavedWorkflow` JSON.
    WorkflowGet {
        workflow_id: String,
    },
    WorkflowCreate {
        workflow: WorkflowDraft,
    },
    /// Replaces the workflow but its `id`, `projectId` and times.
    WorkflowUpdate {
        id: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        workflow: Box<RawValue>,
    },
    WorkflowRemove {
        id: String,
    },
    /// Changes only the stored workflow's name (and `updatedAt`); answers
    /// it without its body.
    WorkflowRename {
        workflow_id: String,
        name: String,
    },
    ChatsList {
        connection_id: String,
    },
    /// The chat's messages and its stored content bytes.
    ChatMessagesList {
        chat_id: String,
    },
    ChatCreate {
        chat: ChatDraft,
    },
    ChatUpdate {
        id: String,
        patch: ChatPatch,
    },
    ChatRemove {
        id: String,
    },
    /// Upserts the listed messages by id; messages not listed stay.
    ChatMessagesPut {
        chat_id: String,
        messages: Vec<ChatMessageDraft>,
    },
    ChatMessagesRemove {
        chat_id: String,
        ids: Vec<String>,
    },
    /// The project's connection order (shared by its windows; the active
    /// connection is per window, in the `ui` group's view state).
    ProjectSidebarGet {
        project_id: String,
    },
    ProjectSidebarSet {
        project_id: String,
        connection_order: Vec<String>,
    },
}

/// The method only: params can hold names, hosts, strings, query text,
/// JSON and secrets.
impl fmt::Debug for LibraryRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryRequest")
            .field("method", &self.method())
            .finish_non_exhaustive()
    }
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
            LibraryRequest::DashboardsList { .. } => "dashboardsList",
            LibraryRequest::DashboardVersionsList { .. } => "dashboardVersionsList",
            LibraryRequest::DashboardVersionGet { .. } => "dashboardVersionGet",
            LibraryRequest::DashboardCreate { .. } => "dashboardCreate",
            LibraryRequest::DashboardUpdate { .. } => "dashboardUpdate",
            LibraryRequest::DashboardRemove { .. } => "dashboardRemove",
            LibraryRequest::WorkflowsList { .. } => "workflowsList",
            LibraryRequest::WorkflowGet { .. } => "workflowGet",
            LibraryRequest::WorkflowCreate { .. } => "workflowCreate",
            LibraryRequest::WorkflowUpdate { .. } => "workflowUpdate",
            LibraryRequest::WorkflowRemove { .. } => "workflowRemove",
            LibraryRequest::WorkflowRename { .. } => "workflowRename",
            LibraryRequest::ChatsList { .. } => "chatsList",
            LibraryRequest::ChatMessagesList { .. } => "chatMessagesList",
            LibraryRequest::ChatCreate { .. } => "chatCreate",
            LibraryRequest::ChatUpdate { .. } => "chatUpdate",
            LibraryRequest::ChatRemove { .. } => "chatRemove",
            LibraryRequest::ChatMessagesPut { .. } => "chatMessagesPut",
            LibraryRequest::ChatMessagesRemove { .. } => "chatMessagesRemove",
            LibraryRequest::ProjectSidebarGet { .. } => "projectSidebarGet",
            LibraryRequest::ProjectSidebarSet { .. } => "projectSidebarSet",
        }
    }
}

/// A `library` call's result, as `{"method": …, "result": {value, seq}}`.
/// `Debug` shows the method and sequence only.
// Not `Deserialize`: results only go out.
#[derive(Serialize)]
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
    DashboardsList(Seqd<Vec<PersistedDashboard>>),
    DashboardVersionsList(Seqd<Vec<PersistedDashboardVersionMeta>>),
    DashboardVersionGet(Seqd<PersistedDashboardVersion>),
    DashboardCreate(Seqd<PersistedDashboard>),
    DashboardUpdate(Seqd<DashboardUpdated>),
    DashboardRemove(Seqd<()>),
    WorkflowsList(Seqd<Vec<PersistedWorkflowMeta>>),
    WorkflowGet(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    WorkflowCreate(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    WorkflowUpdate(#[cfg_attr(feature = "ts", ts(as = "crate::SeqdJson"))] Seqd<Box<RawValue>>),
    WorkflowRemove(Seqd<()>),
    WorkflowRename(Seqd<PersistedWorkflowMeta>),
    ChatsList(Seqd<Vec<PersistedAIChat>>),
    ChatMessagesList(Seqd<ChatMessages>),
    ChatCreate(Seqd<PersistedAIChat>),
    ChatUpdate(Seqd<PersistedAIChat>),
    ChatRemove(Seqd<()>),
    ChatMessagesPut(Seqd<ChatMessages>),
    /// How many messages went.
    ChatMessagesRemove(#[cfg_attr(feature = "ts", ts(type = "Seqd<number>"))] Seqd<u64>),
    ProjectSidebarGet(Seqd<Vec<String>>),
    ProjectSidebarSet(Seqd<Vec<String>>),
}

impl fmt::Debug for LibraryResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The method's name is the variant's, as serde writes it; the
        // serialised form is needed for nothing else.
        let (method, seq) = match self {
            LibraryResponse::ConnectionsList(r) => ("connectionsList", &r.seq),
            LibraryResponse::ProjectsList(r) => ("projectsList", &r.seq),
            LibraryResponse::SavedQueriesList(r) => ("savedQueriesList", &r.seq),
            LibraryResponse::QueryVersionsList(r) => ("queryVersionsList", &r.seq),
            LibraryResponse::ConnectionCreate(r) => ("connectionCreate", &r.seq),
            LibraryResponse::ConnectionUpdate(r) => ("connectionUpdate", &r.seq),
            LibraryResponse::ConnectionRemove(r) => ("connectionRemove", &r.seq),
            LibraryResponse::ProjectCreate(r) => ("projectCreate", &r.seq),
            LibraryResponse::ProjectEnsureDefault(r) => ("projectEnsureDefault", &r.seq),
            LibraryResponse::ProjectUpdate(r) => ("projectUpdate", &r.seq),
            LibraryResponse::ProjectRemove(r) => ("projectRemove", &r.seq),
            LibraryResponse::LabelCreate(r) => ("labelCreate", &r.seq),
            LibraryResponse::LabelUpdate(r) => ("labelUpdate", &r.seq),
            LibraryResponse::LabelRemove(r) => ("labelRemove", &r.seq),
            LibraryResponse::SavedQueryCreate(r) => ("savedQueryCreate", &r.seq),
            LibraryResponse::SavedQueryUpdate(r) => ("savedQueryUpdate", &r.seq),
            LibraryResponse::SavedQueryRemove(r) => ("savedQueryRemove", &r.seq),
            LibraryResponse::DashboardsList(r) => ("dashboardsList", &r.seq),
            LibraryResponse::DashboardVersionsList(r) => ("dashboardVersionsList", &r.seq),
            LibraryResponse::DashboardVersionGet(r) => ("dashboardVersionGet", &r.seq),
            LibraryResponse::DashboardCreate(r) => ("dashboardCreate", &r.seq),
            LibraryResponse::DashboardUpdate(r) => ("dashboardUpdate", &r.seq),
            LibraryResponse::DashboardRemove(r) => ("dashboardRemove", &r.seq),
            LibraryResponse::WorkflowsList(r) => ("workflowsList", &r.seq),
            LibraryResponse::WorkflowGet(r) => ("workflowGet", &r.seq),
            LibraryResponse::WorkflowCreate(r) => ("workflowCreate", &r.seq),
            LibraryResponse::WorkflowUpdate(r) => ("workflowUpdate", &r.seq),
            LibraryResponse::WorkflowRemove(r) => ("workflowRemove", &r.seq),
            LibraryResponse::WorkflowRename(r) => ("workflowRename", &r.seq),
            LibraryResponse::ChatsList(r) => ("chatsList", &r.seq),
            LibraryResponse::ChatMessagesList(r) => ("chatMessagesList", &r.seq),
            LibraryResponse::ChatCreate(r) => ("chatCreate", &r.seq),
            LibraryResponse::ChatUpdate(r) => ("chatUpdate", &r.seq),
            LibraryResponse::ChatRemove(r) => ("chatRemove", &r.seq),
            LibraryResponse::ChatMessagesPut(r) => ("chatMessagesPut", &r.seq),
            LibraryResponse::ChatMessagesRemove(r) => ("chatMessagesRemove", &r.seq),
            LibraryResponse::ProjectSidebarGet(r) => ("projectSidebarGet", &r.seq),
            LibraryResponse::ProjectSidebarSet(r) => ("projectSidebarSet", &r.seq),
        };
        f.debug_struct("LibraryResponse")
            .field("method", &method)
            .field("seq", seq)
            .finish_non_exhaustive()
    }
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

        Q::DashboardsList { project_id } => {
            R::DashboardsList(ws.list_dashboards(core, &project_id).await?)
        }
        Q::DashboardVersionsList { project_id } => {
            R::DashboardVersionsList(ws.list_dashboard_versions(core, &project_id).await?)
        }
        Q::DashboardVersionGet {
            dashboard_id,
            version_id,
        } => R::DashboardVersionGet(
            ws.get_dashboard_version(core, &dashboard_id, &version_id)
                .await?,
        ),
        Q::DashboardCreate { dashboard } => {
            R::DashboardCreate(ws.create_dashboard(core, origin, dashboard).await?)
        }
        Q::DashboardUpdate { id, patch } => {
            R::DashboardUpdate(ws.update_dashboard(core, origin, &id, patch).await?)
        }
        Q::DashboardRemove { id } => {
            R::DashboardRemove(ws.remove_dashboard(core, origin, &id).await?)
        }
        Q::WorkflowsList { project_id } => {
            R::WorkflowsList(ws.list_workflows(core, &project_id).await?)
        }
        Q::WorkflowGet { workflow_id } => {
            R::WorkflowGet(ws.get_workflow(core, &workflow_id).await?)
        }
        Q::WorkflowCreate { workflow } => {
            R::WorkflowCreate(ws.create_workflow(core, origin, workflow).await?)
        }
        Q::WorkflowUpdate { id, workflow } => {
            R::WorkflowUpdate(ws.update_workflow(core, origin, &id, workflow).await?)
        }
        Q::WorkflowRemove { id } => R::WorkflowRemove(ws.remove_workflow(core, origin, &id).await?),
        Q::WorkflowRename { workflow_id, name } => R::WorkflowRename(
            ws.rename_workflow(core, origin, &workflow_id, &name)
                .await?,
        ),
        Q::ChatsList { connection_id } => R::ChatsList(ws.list_chats(core, &connection_id).await?),
        Q::ChatMessagesList { chat_id } => {
            R::ChatMessagesList(ws.list_chat_messages(core, &chat_id).await?)
        }
        Q::ChatCreate { chat } => R::ChatCreate(ws.create_chat(core, origin, chat).await?),
        Q::ChatUpdate { id, patch } => {
            R::ChatUpdate(ws.update_chat(core, origin, &id, patch).await?)
        }
        Q::ChatRemove { id } => R::ChatRemove(ws.remove_chat(core, origin, &id).await?),
        Q::ChatMessagesPut { chat_id, messages } => R::ChatMessagesPut(
            ws.put_chat_messages(core, origin, &chat_id, messages)
                .await?,
        ),
        Q::ChatMessagesRemove { chat_id, ids } => {
            R::ChatMessagesRemove(ws.remove_chat_messages(core, origin, &chat_id, ids).await?)
        }
        Q::ProjectSidebarGet { project_id } => {
            R::ProjectSidebarGet(ws.project_sidebar(core, &project_id).await?)
        }
        Q::ProjectSidebarSet {
            project_id,
            connection_order,
        } => R::ProjectSidebarSet(
            ws.set_project_sidebar(core, origin, &project_id, connection_order)
                .await?,
        ),
    })
}
