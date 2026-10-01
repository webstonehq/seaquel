//! State, settings, dashboards and chats (phase 5d-2), written through
//! Core: the `library` additions (dashboards and their versions, saved
//! workflows, AI chats and messages, a project's connection order), the
//! `settings` group (app-state settings, the AI settings record and its API
//! keys, themes, onboarding, tutorial progress, import state) and the `ui`
//! group (each window's view state).
//!
//! Every write follows 5d-1's pattern (`library.rs`):
//! 1. the input is checked against its rules and the interface's
//!    [`StateLimits`] and [`LibraryLimits`] (`seaquel_workspace::state`),
//!    before anything is read;
//! 2. on the desktop an AI provider's API key is written to the keychain
//!    first (Decision 8, Q19), outside any write transaction, after the
//!    same checks ran on a pool read, and taken back if the record write
//!    then fails;
//! 3. it reads, checks and writes in one [`WriteTx`], taking its change
//!    number while it holds the lock, and commits;
//! 4. it emits one `StorageChanged` event, then deletes what the call gave
//!    up in the keychain (best effort, logged by code).
//!
//! A write that writes nothing (a stale view-state save, a first load with
//! nothing to copy) emits nothing and answers the published sequence.
//! Inside a write every read goes through the transaction. Nothing here
//! logs or puts in an error a name, text, JSON, a setting's value, a window
//! id (it is the caller's origin) or a secret.

use std::time::Duration;

use log::debug;
#[cfg(feature = "secrets")]
use log::warn;
use seaquel_storage::{
    ai_chats, app_state, connections, dashboard_versions, dashboards, import_state, onboarding,
    project_state, projects, saved_canvases, themes, tutorial, user_credentials, window_state,
    windows, Reader, WriteTx,
};
use seaquel_types::storage::{
    ImportState, PersistedAIChat, PersistedDashboard, PersistedDashboardVersion,
    PersistedDashboardVersionMeta, PersistedWorkflowMeta, ThemePreferences, TutorialProgress,
};
use seaquel_workspace::library::{
    self as lib, check_count, check_id, name_key, Clearable, LibraryError, VersionMeta,
};
use seaquel_workspace::run::iso_timestamp;
use seaquel_workspace::state::{
    self as st, AiProviderCreated, AiProviderDraft, AiProviderPatch, AiSettings, AiSettingsPatch,
    ChatDraft, ChatMessageDraft, ChatMessages, ChatPatch, CopiedFrom, DashboardDraft,
    DashboardPatch, DashboardUpdated, SettingKey, ThemeCreated, Themes, WindowActive, WindowFrom,
    WindowStateLoaded, WindowStateSaved, WorkflowDraft, AI_API_KEY_VAULT_SCOPE, AI_SETTINGS_KEY,
    CHAT_NOT_FOUND, DASHBOARD_ID_PREFIX, DASHBOARD_NOT_FOUND, DASHBOARD_VERSION_ID_PREFIX,
    DASHBOARD_VERSION_LIMIT_KEY, DASHBOARD_VERSION_NOT_FOUND, DEFAULT_DARK_THEME,
    DEFAULT_LIGHT_THEME, LAST_ACTIVE_PROJECT_KEY, MAIN_WINDOW, THEME_ID_PREFIX, THEME_NOT_FOUND,
    WINDOW_UNUSED_DAYS, WORKFLOW_ID_PREFIX, WORKFLOW_NOT_FOUND,
};
use serde_json::value::RawValue;

use crate::changes::WriteOrigin;
use crate::library::{connection_not_found, new_id, project_not_found};
use crate::{Core, CoreError, Seqd, StoredKind, Workspace};

type Result<T> = std::result::Result<T, CoreError>;

/// An id for a log line: a call's id may be logged before its size check,
/// so one past 128 bytes is left out.
fn log_id(id: &str) -> &str {
    if id.len() <= 128 {
        id
    } else {
        "<long>"
    }
}

fn dashboard_not_found() -> CoreError {
    CoreError::new(DASHBOARD_NOT_FOUND, "Dashboard not found.")
}

fn dashboard_version_not_found() -> CoreError {
    CoreError::new(DASHBOARD_VERSION_NOT_FOUND, "Dashboard version not found.")
}

fn workflow_not_found() -> CoreError {
    CoreError::new(WORKFLOW_NOT_FOUND, "Saved workflow not found.")
}

fn chat_not_found() -> CoreError {
    CoreError::new(CHAT_NOT_FOUND, "Chat not found.")
}

fn theme_not_found() -> CoreError {
    CoreError::new(THEME_NOT_FOUND, "Theme not found.")
}

/// The executor's time, and that time as Core stores it.
fn clock(core: &Core) -> Result<(Duration, String)> {
    let executor = core.executor.as_ref().ok_or_else(|| {
        CoreError::new(
            "NOT_SUPPORTED",
            "Saving isn't enabled here (no executor is set)",
        )
    })?;
    let t = executor.unix_time();
    Ok((t, iso_timestamp(t)))
}

/// A raw JSON value from text Core built or read (`null` if it isn't
/// JSON, which Core's own text always is).
fn raw(text: String) -> Box<RawValue> {
    st::to_raw(text).unwrap_or_default()
}

async fn project_exists(r: impl Into<Reader<'_>>, project_id: &str) -> Result<()> {
    match projects::get(r, project_id).await? {
        Some(_) => Ok(()),
        None => Err(project_not_found()),
    }
}

// ── Dashboards (Decision 21) ──

impl Workspace {
    /// A project's dashboards. A NULL `starred` reads as false; a
    /// beta-era dashboard with no project is in no project's list.
    pub async fn list_dashboards(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<PersistedDashboard>>> {
        check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = dashboards::list(self.storage(), project_id).await?;
        Ok(Seqd { value, seq })
    }

    /// The versions of a project's dashboards without their snapshots
    /// (5d-2 Task 7: a project's could be hundreds of MiB), by dashboard,
    /// oldest first: what the version history shows.
    /// [`Workspace::get_dashboard_version`] answers one whole.
    pub async fn list_dashboard_versions(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<PersistedDashboardVersionMeta>>> {
        check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = dashboard_versions::list_meta_by_project(self.storage(), project_id).await?;
        Ok(Seqd { value, seq })
    }

    /// One version of a dashboard, with its snapshot
    /// (`dashboardVersionGet`): what the history compares and restores. A
    /// missing dashboard is `DASHBOARD_NOT_FOUND`; a version it doesn't
    /// have (another dashboard's included) `DASHBOARD_VERSION_NOT_FOUND`.
    pub async fn get_dashboard_version(
        &self,
        core: &Core,
        dashboard_id: &str,
        version_id: &str,
    ) -> Result<Seqd<PersistedDashboardVersion>> {
        let libl = core.library_limits();
        check_id(dashboard_id, "dashboard id", &libl)?;
        check_id(version_id, "version id", &libl)?;
        let seq = self.change_seq();
        // Removing the dashboard between the two reads takes its versions,
        // so the second then answers `DASHBOARD_VERSION_NOT_FOUND`.
        dashboards::get(self.storage(), dashboard_id)
            .await?
            .ok_or_else(dashboard_not_found)?;
        let value = dashboard_versions::get(self.storage(), dashboard_id, version_id)
            .await?
            .ok_or_else(dashboard_version_not_found)?;
        Ok(Seqd { value, seq })
    }

    /// Save a new dashboard (`dashboardCreate`) with a Core id.
    ///
    /// Errors: `INVALID_ARGUMENT`, `PROJECT_NOT_FOUND`, `NAME_TAKEN`, the
    /// storage codes.
    pub async fn create_dashboard(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: DashboardDraft,
    ) -> Result<Seqd<PersistedDashboard>> {
        debug!(activity = "library.dashboardCreate", project_id = log_id(&draft.project_id); "Create a dashboard");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        st::check_dashboard_draft(&draft, &libl, &stl)?;
        let (_, now) = clock(core)?;
        let id = new_id(DASHBOARD_ID_PREFIX);
        let row = st::dashboard_from_draft(id.clone(), &draft, &now);
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        check_count(
            dashboards::count(&mut tx).await?,
            stl.max_dashboards,
            "max_dashboards",
        )?;
        project_exists(&mut tx, &row.project_id).await?;
        let mut row = row;
        if draft.rename_if_taken {
            row.name = dashboard_free_name(&mut tx, &row.project_id, &row.name).await?;
        } else {
            dashboard_name_free(&mut tx, &row.project_id, &row.name, None).await?;
        }
        dashboards::insert(&mut tx, &row).await?;
        let stored = dashboards::get(&mut tx, &id)
            .await?
            .ok_or_else(dashboard_not_found)?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Dashboard,
            Some(stored.project_id.clone()),
            Some(vec![id]),
            origin,
        );
        Ok(Seqd { value: stored, seq })
    }

    /// Change a dashboard (`dashboardUpdate`, Decision 21): only the
    /// patch's fields. With `captureVersion`, a version of the stored
    /// dashboard (before the change) is appended, numbered inside the
    /// transaction, then the versions are pruned to
    /// `dashboard_version_limit` (0 keeps all) and, on the web, to
    /// `max_dashboard_version_bytes`. A removed dashboard is
    /// `DASHBOARD_NOT_FOUND`, never re-inserted.
    pub async fn update_dashboard(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: DashboardPatch,
    ) -> Result<Seqd<DashboardUpdated>> {
        debug!(activity = "library.dashboardUpdate", dashboard_id = log_id(id); "Update a dashboard");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        check_id(id, "dashboard id", &libl)?;
        st::check_dashboard_patch(&patch, &libl, &stl)?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut row = dashboards::get(&mut tx, id)
            .await?
            .ok_or_else(dashboard_not_found)?;
        let snapshot = patch.capture_version.then(|| st::dashboard_snapshot(&row));
        let before = st::dashboard_size(&row);
        let renamed = st::apply_dashboard_patch(&mut row, &patch, &now);
        // Past `max_dashboard_bytes` only when it grew: a dashboard stored
        // before the limit stays editable.
        st::check_dashboard_size(&row, Some(before), &stl)?;
        if renamed {
            dashboard_name_free(&mut tx, &row.project_id, &row.name, Some(id)).await?;
        }
        if !dashboards::update(&mut tx, &row).await? {
            return Err(dashboard_not_found());
        }
        let mut version = None;
        let mut pruned_version_ids = Vec::new();
        if let Some(snapshot) = snapshot {
            let v = dashboard_versions::append(
                &mut tx,
                &new_id(DASHBOARD_VERSION_ID_PREFIX),
                id,
                &snapshot,
                &now,
            )
            .await?;
            let limit = app_state::get(&mut tx, DASHBOARD_VERSION_LIMIT_KEY).await?;
            let keep = lib::parse_version_limit(limit.as_deref());
            let metas: Vec<VersionMeta> = dashboard_versions::list_meta(&mut tx, id)
                .await?
                .into_iter()
                .map(|m| VersionMeta {
                    id: m.id,
                    version: m.version,
                    keyframe: m.keyframe,
                    bytes: m.bytes,
                })
                .collect();
            pruned_version_ids = lib::version_prune(&metas, keep, stl.max_dashboard_version_bytes);
            dashboard_versions::delete_ids(&mut tx, id, &pruned_version_ids).await?;
            version = Some(v);
        }
        let stored = dashboards::get(&mut tx, id)
            .await?
            .ok_or_else(dashboard_not_found)?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Dashboard,
            Some(stored.project_id.clone()),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: DashboardUpdated {
                dashboard: stored,
                version,
                pruned_version_ids,
            },
            seq,
        })
    }

    /// Remove a dashboard (`dashboardRemove`) and its versions (on every
    /// file shape, the beta era's included).
    pub async fn remove_dashboard(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "library.dashboardRemove", dashboard_id = log_id(id); "Remove a dashboard");
        check_id(id, "dashboard id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = dashboards::get(&mut tx, id)
            .await?
            .ok_or_else(dashboard_not_found)?;
        if !dashboards::delete(&mut tx, id).await? {
            return Err(dashboard_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Dashboard,
            Some(row.project_id),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: (), seq })
    }
}

/// `NAME_TAKEN` naming the first other dashboard of the project whose name
/// has `name`'s key (an index search of the stored `name_key`).
async fn dashboard_name_free(
    tx: &mut WriteTx,
    project_id: &str,
    name: &str,
    except: Option<&str>,
) -> Result<()> {
    let holder = dashboards::with_name_key(tx, project_id, &name_key(name))
        .await?
        .into_iter()
        .find(|r| Some(r.id.as_str()) != except);
    match holder {
        Some(r) => Err(LibraryError::name_taken("dashboard", r.id).into()),
        None => Ok(()),
    }
}

/// `name`, or the first free `"<name> (n)"` among the project's
/// dashboards (Decision 13's rule, as the library's imports), one indexed
/// lookup per candidate.
async fn dashboard_free_name(tx: &mut WriteTx, project_id: &str, name: &str) -> Result<String> {
    if dashboards::with_name_key(&mut *tx, project_id, &name_key(name))
        .await?
        .is_empty()
    {
        return Ok(name.to_string());
    }
    // Each taken candidate is a distinct row, so this ends by the time it
    // has tried one more than the project's dashboards.
    for n in 2u64.. {
        let candidate = format!("{name} ({n})");
        if dashboards::with_name_key(&mut *tx, project_id, &name_key(&candidate))
            .await?
            .is_empty()
        {
            return Ok(candidate);
        }
    }
    unreachable!("a free name exists")
}

// ── Saved workflows (Decision 23) ──

impl Workspace {
    /// A project's saved workflows without their bodies (5d-2 Task 7: a
    /// project's could be hundreds of MiB): id, project, name, times and
    /// size, in rowid order, without the rows that don't read (nothing
    /// here deletes them). [`Workspace::get_workflow`] answers one whole.
    pub async fn list_workflows(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<PersistedWorkflowMeta>>> {
        check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = saved_canvases::list_meta(self.storage(), project_id).await?;
        Ok(Seqd { value, seq })
    }

    /// One saved workflow as its stored JSON (`workflowGet`), byte for
    /// byte: what opening it shows. A missing one, or one whose stored
    /// JSON doesn't read (which no list shows), is `WORKFLOW_NOT_FOUND`.
    pub async fn get_workflow(&self, core: &Core, id: &str) -> Result<Seqd<Box<RawValue>>> {
        check_id(id, "workflow id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = saved_canvases::get(self.storage(), id)
            .await?
            .and_then(|row| row.data)
            .ok_or_else(workflow_not_found)?;
        Ok(Seqd { value, seq })
    }

    /// Save a new workflow (`workflowCreate`). Core sets its `id`,
    /// `projectId`, `createdAt` and `updatedAt` in the JSON and keeps the
    /// rest byte for byte. On the web a workflow past
    /// `max_workflow_bytes` is refused.
    pub async fn create_workflow(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: WorkflowDraft,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "library.workflowCreate", project_id = log_id(&draft.project_id); "Create a saved workflow");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        check_id(&draft.project_id, "project id", &libl)?;
        st::check_workflow_body(&draft.workflow, &libl, &stl)?;
        let (_, now) = clock(core)?;
        let id = new_id(WORKFLOW_ID_PREFIX);
        let json = st::workflow_json(&draft.workflow, &id, &draft.project_id, &now, &now);
        st::check_workflow_size(&json, None, &stl)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        check_count(
            saved_canvases::count(&mut tx).await?,
            stl.max_workflows,
            "max_workflows",
        )?;
        project_exists(&mut tx, &draft.project_id).await?;
        saved_canvases::insert(&mut tx, &id, &draft.project_id, &json).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Workflow,
            Some(draft.project_id),
            Some(vec![id]),
            origin,
        );
        Ok(Seqd {
            value: raw(json),
            seq,
        })
    }

    /// Replace a saved workflow (`workflowUpdate`): everything but its
    /// `id`, `projectId` and `createdAt`; `updatedAt` becomes now. On the
    /// web a workflow past `max_workflow_bytes` is refused only when it is
    /// larger than the one stored.
    pub async fn update_workflow(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        workflow: Box<RawValue>,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "library.workflowUpdate", workflow_id = log_id(id); "Update a saved workflow");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        check_id(id, "workflow id", &libl)?;
        // Its size is checked against the stored workflow, inside the write.
        st::check_workflow_shape(&workflow, &libl)?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = saved_canvases::get(&mut tx, id)
            .await?
            .ok_or_else(workflow_not_found)?;
        let created = st::created_at_of(row.data.as_deref()).unwrap_or_else(|| now.clone());
        let json = st::workflow_json(&workflow, id, &row.project_id, &created, &now);
        // Past `max_workflow_bytes` only when it grew: a workflow stored
        // before the limit stays saveable and can shrink.
        let stored = row.data.as_ref().map(|d| d.get().len());
        st::check_workflow_size(&json, stored, &stl)?;
        if !saved_canvases::update(&mut tx, id, &json).await? {
            return Err(workflow_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Workflow,
            Some(row.project_id),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: raw(json),
            seq,
        })
    }

    /// Rename a saved workflow (`workflowRename`, 5d-2 Task 7 review): only
    /// the stored JSON's `name` and `updatedAt` change, on the row read
    /// inside the write, so a save another window made just before stays
    /// (the GUI no longer reads the body and writes it back whole). A
    /// missing workflow, or one whose stored JSON doesn't read, is
    /// `WORKFLOW_NOT_FOUND`. Answers the renamed workflow without its body.
    pub async fn rename_workflow(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        name: &str,
    ) -> Result<Seqd<PersistedWorkflowMeta>> {
        debug!(activity = "library.workflowRename", workflow_id = log_id(id); "Rename a saved workflow");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        check_id(id, "workflow id", &libl)?;
        st::check_workflow_rename(name, &libl)?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = saved_canvases::get(&mut tx, id)
            .await?
            .ok_or_else(workflow_not_found)?;
        let stored = row.data.as_deref().ok_or_else(workflow_not_found)?;
        let json = st::rename_workflow_json(stored, name, &now).ok_or_else(workflow_not_found)?;
        // Past `max_workflow_bytes` only when it grew (a longer name).
        st::check_workflow_size(&json, Some(stored.get().len()), &stl)?;
        let created_at = st::created_at_of(Some(stored));
        if !saved_canvases::update(&mut tx, id, &json).await? {
            return Err(workflow_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Workflow,
            Some(row.project_id.clone()),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: PersistedWorkflowMeta {
                id: id.to_string(),
                project_id: row.project_id,
                name: name.to_string(),
                created_at,
                updated_at: Some(now),
                bytes: json.len() as u64,
            },
            seq,
        })
    }

    /// Remove a saved workflow (`workflowRemove`).
    pub async fn remove_workflow(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "library.workflowRemove", workflow_id = log_id(id); "Remove a saved workflow");
        check_id(id, "workflow id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = saved_canvases::get(&mut tx, id)
            .await?
            .ok_or_else(workflow_not_found)?;
        saved_canvases::delete(&mut tx, id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Workflow,
            Some(row.project_id),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: (), seq })
    }
}

// ── A project's connection order (Decision 22) ──

impl Workspace {
    /// The project's connection order (`projectSidebarGet`), shared by its
    /// windows: the refetch a `project` event asks for, since
    /// `projectsList` rows don't hold it. A project without a
    /// `project_state` row, or an order that isn't a list, reads as none.
    pub async fn project_sidebar(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<String>>> {
        check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let stored = project_state::sidebar(self.storage(), project_id).await?;
        Ok(Seqd {
            value: st::read_connection_order(stored.as_deref()),
            seq,
        })
    }

    /// Set the project's connection order (`projectSidebarSet`): an upsert
    /// of its `project_state` row, the other columns taking the schema's
    /// defaults when it had none. Emits a `project` event.
    pub async fn set_project_sidebar(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        connection_order: Vec<String>,
    ) -> Result<Seqd<Vec<String>>> {
        debug!(activity = "library.projectSidebarSet", project_id = log_id(project_id), connections = connection_order.len(); "Set a project's connection order");
        let libl = core.library_limits();
        check_id(project_id, "project id", &libl)?;
        st::check_connection_order(&connection_order, &libl)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        project_exists(&mut tx, project_id).await?;
        project_state::set_connection_order(&mut tx, project_id, &connection_order).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Project,
            None,
            Some(vec![project_id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: connection_order,
            seq,
        })
    }
}

// ── AI chats (Decision 24) ──

impl Workspace {
    /// A connection's chats, most recently updated first.
    pub async fn list_chats(
        &self,
        core: &Core,
        connection_id: &str,
    ) -> Result<Seqd<Vec<PersistedAIChat>>> {
        check_id(connection_id, "connection id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = ai_chats::list(self.storage(), connection_id).await?;
        Ok(Seqd { value, seq })
    }

    /// A chat's messages, by `timestamp` then the order they were first
    /// stored, and the chat's stored content bytes (so a full chat opens
    /// disabled on the web). A chat with no messages, or none at all,
    /// answers an empty list.
    pub async fn list_chat_messages(
        &self,
        core: &Core,
        chat_id: &str,
    ) -> Result<Seqd<ChatMessages>> {
        check_id(chat_id, "chat id", &core.library_limits())?;
        let seq = self.change_seq();
        let messages = ai_chats::load_messages(self.storage(), chat_id).await?;
        let stored_bytes = ai_chats::content_bytes(self.storage(), chat_id).await?;
        let full = st::chat_is_full(stored_bytes, messages.len() as u64, &core.state_limits());
        Ok(Seqd {
            value: ChatMessages {
                messages,
                stored_bytes,
                full,
            },
            seq,
        })
    }

    /// Save a new chat (`chatCreate`) with a Core id (a uuid).
    pub async fn create_chat(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: ChatDraft,
    ) -> Result<Seqd<PersistedAIChat>> {
        debug!(activity = "library.chatCreate", connection_id = log_id(&draft.connection_id); "Create a chat");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        st::check_chat_draft(&draft, &libl)?;
        let (_, now) = clock(core)?;
        let id = uuid::Uuid::new_v4().to_string();
        let row = st::chat_from_draft(id.clone(), &draft, &now);
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        check_count(ai_chats::count(&mut tx).await?, stl.max_chats, "max_chats")?;
        if connections::get(&mut tx, &row.connection_id)
            .await?
            .is_none()
        {
            return Err(connection_not_found());
        }
        ai_chats::insert(&mut tx, &row).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Chat,
            Some(row.connection_id.clone()),
            Some(vec![id]),
            origin,
        );
        Ok(Seqd { value: row, seq })
    }

    /// Change a chat (`chatUpdate`): its title, and `touched` sets
    /// `updatedAt` to now.
    pub async fn update_chat(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: ChatPatch,
    ) -> Result<Seqd<PersistedAIChat>> {
        debug!(activity = "library.chatUpdate", chat_id = log_id(id); "Update a chat");
        let libl = core.library_limits();
        check_id(id, "chat id", &libl)?;
        st::check_chat_patch(&patch, &libl)?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut row = ai_chats::get(&mut tx, id)
            .await?
            .ok_or_else(chat_not_found)?;
        st::apply_chat_patch(&mut row, &patch, &now);
        if !ai_chats::update(&mut tx, &row).await? {
            return Err(chat_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Chat,
            Some(row.connection_id.clone()),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: row, seq })
    }

    /// Remove a chat and its messages (`chatRemove`).
    pub async fn remove_chat(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "library.chatRemove", chat_id = log_id(id); "Remove a chat");
        check_id(id, "chat id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = ai_chats::get(&mut tx, id)
            .await?
            .ok_or_else(chat_not_found)?;
        ai_chats::delete(&mut tx, id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Chat,
            Some(row.connection_id),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: (), seq })
    }

    /// Upsert messages into a chat by their (GUI-made) ids
    /// (`chatMessagesPut`, Decision 24): a stored message keeps its place
    /// and gets the new fields, new ones are added in list order, and
    /// messages not listed stay. An id of another chat is refused. On the
    /// web the chat's content, less the messages replaced, plus the put's,
    /// must stay within `max_chat_bytes`; past it nothing is stored.
    ///
    /// Answers the messages stored (not the chat's whole list) and the
    /// chat's stored bytes after the put.
    pub async fn put_chat_messages(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        chat_id: &str,
        messages: Vec<ChatMessageDraft>,
    ) -> Result<Seqd<ChatMessages>> {
        debug!(activity = "library.chatMessagesPut", chat_id = log_id(chat_id), messages = messages.len(); "Put a chat's messages");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        check_id(chat_id, "chat id", &libl)?;
        st::check_messages(&messages, &libl, &stl)?;
        let rows: Vec<_> = messages
            .iter()
            .map(|m| st::message_row(chat_id, m))
            .collect();
        let ids: Vec<String> = rows.iter().map(|m| m.id.clone()).collect();
        let added: u64 = rows.iter().map(|m| m.content.len() as u64).sum();
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        if ai_chats::get(&mut tx, chat_id).await?.is_none() {
            return Err(chat_not_found());
        }
        let existing = ai_chats::message_chat_ids(&mut tx, &ids).await?;
        if existing.iter().any(|(_, owner)| owner != chat_id) {
            return Err(LibraryError::invalid("A message id belongs to another chat.").into());
        }
        if let Some(max) = stl.max_messages_per_chat {
            let now_held = ai_chats::message_count(&mut tx, chat_id).await?;
            let new = (ids.len() - existing.len()) as u64;
            if now_held.saturating_add(new) > max as u64 {
                return Err(LibraryError::invalid(format!(
                    "This chat has more messages than allowed here (max_messages_per_chat: {max})."
                ))
                .into());
            }
        }
        if stl.max_chat_bytes.is_some() {
            let stored = ai_chats::content_bytes(&mut tx, chat_id).await?;
            let replaced = ai_chats::content_bytes_of(&mut tx, chat_id, &ids).await?;
            st::check_chat_budget(stored, replaced, added, &stl)?;
        }
        match ai_chats::put_messages(&mut tx, chat_id, &rows).await? {
            ai_chats::PutMessages::Done => {}
            ai_chats::PutMessages::OtherChat { .. } => {
                return Err(LibraryError::invalid("A message id belongs to another chat.").into())
            }
        }
        let stored_bytes = ai_chats::content_bytes(&mut tx, chat_id).await?;
        let held = ai_chats::message_count(&mut tx, chat_id).await?;
        let full = st::chat_is_full(stored_bytes, held, &stl);
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::ChatMessages,
            Some(chat_id.to_string()),
            Some(ids),
            origin,
        );
        Ok(Seqd {
            value: ChatMessages {
                messages: rows,
                stored_bytes,
                full,
            },
            seq,
        })
    }

    /// Delete messages of a chat (`chatMessagesRemove`); ids that aren't
    /// this chat's are left alone. Answers how many went.
    pub async fn remove_chat_messages(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        chat_id: &str,
        ids: Vec<String>,
    ) -> Result<Seqd<u64>> {
        debug!(activity = "library.chatMessagesRemove", chat_id = log_id(chat_id), messages = ids.len(); "Remove a chat's messages");
        check_id(chat_id, "chat id", &core.library_limits())?;
        st::check_message_ids(&ids, &core.state_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        if ai_chats::get(&mut tx, chat_id).await?.is_none() {
            return Err(chat_not_found());
        }
        let removed = ai_chats::delete_messages(&mut tx, chat_id, &ids).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::ChatMessages,
            Some(chat_id.to_string()),
            Some(ids),
            origin,
        );
        Ok(Seqd {
            value: removed,
            seq,
        })
    }
}

// ── Settings (Decision 20) ──

impl Workspace {
    /// A setting's stored value (`settingGet`): `None` for no row or a NULL
    /// one. Only the closed set's keys are read; any other is
    /// `INVALID_ARGUMENT` naming it.
    pub async fn get_setting(&self, key: &str) -> Result<Seqd<Option<String>>> {
        let k = SettingKey::parse(key)?;
        let seq = self.change_seq();
        let value = app_state::get(self.storage(), k.as_str()).await?;
        Ok(Seqd { value, seq })
    }

    /// Set a setting (`settingSet`): the value is checked for the key, and
    /// `None` deletes the row. `lastActiveProjectId` (written by
    /// `windowActivate`), Core's own keys and the storage group's are
    /// refused, and `connectionStringSecretsNotice` can only be cleared.
    pub async fn set_setting(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        key: &str,
        value: Option<String>,
    ) -> Result<Seqd<Option<String>>> {
        let k = st::check_setting_set(
            key,
            value.as_deref(),
            &core.library_limits(),
            &core.state_limits(),
        )?;
        debug!(activity = "settings.settingSet", key = k.as_str(), clear = value.is_none(); "Set a setting");
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        match &value {
            Some(v) => app_state::set_in(&mut tx, k.as_str(), Some(v)).await?,
            None => {
                app_state::delete_in(&mut tx, k.as_str()).await?;
            }
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Setting,
            None,
            Some(vec![k.as_str().to_string()]),
            origin,
        );
        Ok(Seqd { value, seq })
    }
}

// ── AI settings and their API keys (Decision 20, Q19) ──

impl Workspace {
    async fn read_ai_settings(&self, r: impl Into<Reader<'_>>) -> Result<AiSettings> {
        let raw = app_state::get(r, AI_SETTINGS_KEY).await?;
        Ok(st::read_ai_settings(raw.as_deref()))
    }

    /// The AI settings record as the GUI reads it (`aiSettingsGet`): the
    /// legacy provider fields cleaned, the defaults for absent known fields
    /// and a record that doesn't read, every other field as stored.
    pub async fn get_ai_settings(&self) -> Result<Seqd<Box<RawValue>>> {
        let seq = self.change_seq();
        let value = self.read_ai_settings(self.storage()).await?.to_raw();
        Ok(Seqd { value, seq })
    }

    /// Rewrite the stored record inside `tx` with `change` applied: read
    /// from the stored copy (never the GUI's), fields Core doesn't know
    /// kept byte for byte.
    async fn rewrite_ai_settings(
        &self,
        core: &Core,
        tx: &mut WriteTx,
        change: impl FnOnce(&mut AiSettings) -> Result<()>,
    ) -> Result<AiSettings> {
        let mut settings = self.read_ai_settings(&mut *tx).await?;
        change(&mut settings)?;
        st::check_ai_settings_size(&settings, &core.state_limits())?;
        app_state::set_in(tx, AI_SETTINGS_KEY, Some(&settings.to_json())).await?;
        Ok(settings)
    }

    /// Change the AI settings' flags (`aiSettingsPatch`).
    pub async fn patch_ai_settings(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        patch: AiSettingsPatch,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "settings.aiSettingsPatch"; "Change the AI settings");
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let settings = self
            .rewrite_ai_settings(core, &mut tx, |s| {
                s.apply_patch(&patch);
                Ok(())
            })
            .await?;
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::AiSettings, None, None, origin);
        Ok(Seqd {
            value: settings.to_raw(),
            seq,
        })
    }

    /// `NOT_SUPPORTED` for an API key (set or cleared) on a workspace
    /// without a secret store: the web's vault stays in the browser.
    fn check_api_key_support(&self, api_key: &Clearable<String>) -> Result<()> {
        if api_key.is_none() || self.has_secret_store() {
            Ok(())
        } else {
            Err(CoreError::new(
                "NOT_SUPPORTED",
                "Saving an API key with the provider isn't available here.",
            ))
        }
    }

    /// Add an AI provider (`aiProviderCreate`) with a Core id (a uuid). On
    /// the desktop its API key goes to the keychain first; if the record
    /// write then fails, the key is deleted.
    pub async fn create_ai_provider(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: AiProviderDraft,
        api_key: Clearable<String>,
    ) -> Result<Seqd<AiProviderCreated>> {
        debug!(activity = "settings.aiProviderCreate", key = api_key.is_some(); "Add an AI provider");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        st::check_ai_provider_draft(&draft, &libl)?;
        st::check_api_key(&api_key, &libl)?;
        self.check_api_key_support(&api_key)?;
        let id = uuid::Uuid::new_v4().to_string();
        let add = |s: &mut AiSettings| -> Result<()> {
            check_count(
                s.provider_count() as u64,
                stl.max_ai_providers,
                "max_ai_providers",
            )?;
            s.add_provider(&id, &draft);
            Ok(())
        };
        let key = api_key.as_ref().and_then(Option::as_deref);
        if let Some(key) = key {
            // The checks that read, on the pool, so a refused call never
            // touches the keychain.
            let mut probe = self.read_ai_settings(self.storage()).await?;
            add(&mut probe)?;
            st::check_ai_settings_size(&probe, &stl)?;
            self.set_api_key(&id, key).await?;
        }
        let written = async {
            let mut tx = self.storage().write().await?;
            let ticket = self.take_seq();
            let settings = self.rewrite_ai_settings(core, &mut tx, add).await?;
            tx.commit().await?;
            Ok::<_, CoreError>((settings, ticket))
        }
        .await;
        match written {
            Ok((settings, ticket)) => {
                let seq = self.announce(
                    ticket,
                    StoredKind::AiSettings,
                    None,
                    Some(vec![id.clone()]),
                    origin,
                );
                Ok(Seqd {
                    value: AiProviderCreated {
                        id,
                        settings: settings.to_raw(),
                    },
                    seq,
                })
            }
            Err(e) => {
                if let Some(key) = key {
                    self.restore_api_key(&id, key, None).await;
                }
                Err(e)
            }
        }
    }

    /// Change an AI provider (`aiProviderUpdate`): only the patch's fields,
    /// in the stored record. `apiKey` (desktop): absent keeps the key,
    /// `null` deletes it after the commit, a string sets it first; if the
    /// record write then fails, the key it replaced is put back (or, for a
    /// provider removed meanwhile, the key is deleted).
    pub async fn update_ai_provider(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: AiProviderPatch,
        api_key: Clearable<String>,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "settings.aiProviderUpdate", key = api_key.is_some(); "Change an AI provider");
        let libl = core.library_limits();
        check_id(id, "AI provider id", &libl)?;
        st::check_ai_provider_patch(&patch, &libl)?;
        st::check_api_key(&api_key, &libl)?;
        self.check_api_key_support(&api_key)?;
        let update = |s: &mut AiSettings| -> Result<()> { Ok(s.update_provider(id, &patch)?) };
        let key = api_key.as_ref().and_then(Option::as_deref);
        let mut before = None;
        if let Some(key) = key {
            let mut probe = self.read_ai_settings(self.storage()).await?;
            update(&mut probe)?;
            st::check_ai_settings_size(&probe, &core.state_limits())?;
            before = Some(self.set_api_key(id, key).await?);
        }
        let written = async {
            let mut tx = self.storage().write().await?;
            let ticket = self.take_seq();
            let settings = self.rewrite_ai_settings(core, &mut tx, update).await?;
            tx.commit().await?;
            Ok::<_, CoreError>((settings, ticket))
        }
        .await;
        let (settings, ticket) = match written {
            Ok(done) => done,
            Err(e) => {
                if let (Some(before), Some(key)) = (before, key) {
                    let gone = e.code == st::AI_PROVIDER_NOT_FOUND;
                    self.restore_api_key(id, key, if gone { None } else { before })
                        .await;
                }
                return Err(e);
            }
        };
        let seq = self.announce(
            ticket,
            StoredKind::AiSettings,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        if matches!(api_key, Some(None)) {
            self.delete_api_key(id).await;
        }
        Ok(Seqd {
            value: settings.to_raw(),
            seq,
        })
    }

    /// Remove an AI provider (`aiProviderRemove`). Its web vault rows go in
    /// the same transaction; its keychain entry is deleted after the
    /// commit, best effort.
    pub async fn remove_ai_provider(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "settings.aiProviderRemove"; "Remove an AI provider");
        check_id(id, "AI provider id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let settings = self
            .rewrite_ai_settings(core, &mut tx, |s| Ok(s.remove_provider(id)?))
            .await?;
        user_credentials::remove_in(&mut tx, AI_API_KEY_VAULT_SCOPE, id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::AiSettings,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        self.delete_api_key(id).await;
        Ok(Seqd {
            value: settings.to_raw(),
            seq,
        })
    }

    /// Sets the provider's keychain entry and answers what it held before
    /// (for [`Workspace::restore_api_key`]). Never inside a `WriteTx`.
    #[cfg(feature = "secrets")]
    async fn set_api_key(&self, id: &str, key: &str) -> Result<Option<String>> {
        let Some(store) = self.secrets() else {
            return Ok(None);
        };
        let name = format!("{}{id}", st::AI_API_KEY_PREFIX);
        let before = store.get(&name).await.map_err(|e| {
            warn!(activity = "settings.apiKey", code = e.code(); "Reading an AI API key failed");
            CoreError::from(e)
        })?;
        store.set(&name, key).await.map_err(|e| {
            warn!(activity = "settings.apiKey", code = e.code(); "Saving an AI API key failed");
            CoreError::from(e)
        })?;
        Ok(before)
    }

    #[cfg(not(feature = "secrets"))]
    async fn set_api_key(&self, _id: &str, _key: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Puts back what the entry held before a failed write (`None`:
    /// deletes it), best effort, and only while the entry still holds
    /// `ours`, this call's key: a key another call set meanwhile is kept.
    #[cfg(feature = "secrets")]
    async fn restore_api_key(&self, id: &str, ours: &str, before: Option<String>) {
        let Some(store) = self.secrets() else {
            return;
        };
        let name = format!("{}{id}", st::AI_API_KEY_PREFIX);
        match store.get(&name).await {
            Ok(Some(now)) if now == ours => {}
            Ok(_) => return,
            Err(e) => {
                warn!(activity = "settings.apiKey", code = e.code(); "Reading an AI API key failed");
                return;
            }
        }
        let undone = match before {
            Some(old) => store.set(&name, &old).await,
            None => store.delete(&name).await,
        };
        if let Err(e) = undone {
            warn!(activity = "settings.apiKey", code = e.code(); "Undoing an AI API key failed");
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn restore_api_key(&self, _id: &str, _ours: &str, _before: Option<String>) {}

    /// Deletes the provider's keychain entry, best effort.
    #[cfg(feature = "secrets")]
    async fn delete_api_key(&self, id: &str) {
        let Some(store) = self.secrets() else {
            return;
        };
        if let Err(e) = store
            .delete(&format!("{}{id}", st::AI_API_KEY_PREFIX))
            .await
        {
            warn!(activity = "settings.apiKey", code = e.code(); "Deleting an AI API key failed");
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn delete_api_key(&self, _id: &str) {}
}

// ── Themes (Decision 20) ──

/// A read inside the write transaction, or on the pool.
enum Src<'a> {
    Pool(&'a seaquel_storage::Storage),
    Tx(&'a mut WriteTx),
}

impl Src<'_> {
    fn r(&mut self) -> Reader<'_> {
        match self {
            Src::Pool(st) => Reader::Pool(st),
            Src::Tx(tx) => Reader::Tx(tx),
        }
    }
}

async fn read_themes(mut src: Src<'_>) -> Result<Themes> {
    let preferences = themes::preferences(src.r())
        .await?
        .unwrap_or_else(st::default_preferences);
    let user_themes = themes::list(src.r()).await?;
    Ok(Themes {
        preferences,
        user_themes,
    })
}

impl Workspace {
    /// The theme preferences (the defaults with no row) and every user
    /// theme that reads (`themesGet`).
    pub async fn get_themes(&self) -> Result<Seqd<Themes>> {
        let seq = self.change_seq();
        let value = read_themes(Src::Pool(self.storage())).await?;
        Ok(Seqd { value, seq })
    }

    /// Set the light and dark theme ids (`themePreferencesSet`).
    pub async fn set_theme_preferences(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        preferences: ThemePreferences,
    ) -> Result<Seqd<Themes>> {
        debug!(activity = "settings.themePreferencesSet"; "Set the theme preferences");
        let libl = core.library_limits();
        st::check_theme_id(&preferences.light_theme_id, &libl)?;
        st::check_theme_id(&preferences.dark_theme_id, &libl)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        themes::set_preferences(
            &mut tx,
            &preferences.light_theme_id,
            &preferences.dark_theme_id,
        )
        .await?;
        let value = read_themes(Src::Tx(&mut tx)).await?;
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::Theme, None, None, origin);
        Ok(Seqd { value, seq })
    }

    /// Add a user theme (`userThemeCreate`) with a Core id (`theme-<uuid>`),
    /// set in its JSON too, with `createdAt` and `updatedAt`. Only its
    /// `user_themes` row is written.
    pub async fn create_user_theme(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        theme: Box<RawValue>,
    ) -> Result<Seqd<ThemeCreated>> {
        debug!(activity = "settings.userThemeCreate"; "Add a user theme");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        st::check_user_theme(&theme, &libl, &stl)?;
        let (_, now) = clock(core)?;
        let id = new_id(THEME_ID_PREFIX);
        let json = st::user_theme_json(&theme, &id, &now, &now);
        check_theme_size(&json, &stl)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        check_count(
            themes::count(&mut tx).await?,
            stl.max_user_themes,
            "max_user_themes",
        )?;
        themes::insert(&mut tx, &id, &json).await?;
        let value = read_themes(Src::Tx(&mut tx)).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Theme,
            None,
            Some(vec![id.clone()]),
            origin,
        );
        Ok(Seqd {
            value: ThemeCreated { id, themes: value },
            seq,
        })
    }

    /// Replace a user theme (`userThemeUpdate`): its id and `createdAt`
    /// stay, `updatedAt` becomes now.
    pub async fn update_user_theme(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        theme: Box<RawValue>,
    ) -> Result<Seqd<Themes>> {
        debug!(activity = "settings.userThemeUpdate", theme_id = log_id(id); "Change a user theme");
        let (libl, stl) = (core.library_limits(), core.state_limits());
        st::check_theme_id(id, &libl)?;
        st::check_user_theme(&theme, &libl, &stl)?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = themes::get(&mut tx, id)
            .await?
            .ok_or_else(theme_not_found)?;
        let created = st::created_at_of(row.data.as_deref()).unwrap_or_else(|| now.clone());
        let json = st::user_theme_json(&theme, id, &created, &now);
        check_theme_size(&json, &stl)?;
        if !themes::update(&mut tx, id, &json).await? {
            return Err(theme_not_found());
        }
        let value = read_themes(Src::Tx(&mut tx)).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Theme,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value, seq })
    }

    /// Remove a user theme (`userThemeRemove`). A preference that named it
    /// goes back to its default in the same transaction; otherwise the
    /// preferences aren't written.
    pub async fn remove_user_theme(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<Themes>> {
        debug!(activity = "settings.userThemeRemove", theme_id = log_id(id); "Remove a user theme");
        st::check_theme_id(id, &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        if !themes::delete(&mut tx, id).await? {
            return Err(theme_not_found());
        }
        if let Some(p) = themes::preferences(&mut tx).await? {
            if p.light_theme_id == id || p.dark_theme_id == id {
                let light = if p.light_theme_id == id {
                    DEFAULT_LIGHT_THEME
                } else {
                    &p.light_theme_id
                };
                let dark = if p.dark_theme_id == id {
                    DEFAULT_DARK_THEME
                } else {
                    &p.dark_theme_id
                };
                themes::set_preferences(&mut tx, light, dark).await?;
            }
        }
        let value = read_themes(Src::Tx(&mut tx)).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Theme,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value, seq })
    }
}

fn check_theme_size(json: &str, limits: &st::StateLimits) -> Result<()> {
    match limits.max_setting_bytes {
        Some(max) if json.len() > max => Err(LibraryError::invalid(format!(
            "The theme is larger than allowed here (max_setting_bytes: {max} bytes)."
        ))
        .into()),
        _ => Ok(()),
    }
}

// ── Onboarding, tutorial progress, import state (Decision 20) ──

impl Workspace {
    /// The onboarding record (`onboardingGet`): the store's six defaults
    /// with the stored fields over them.
    pub async fn get_onboarding(&self) -> Result<Seqd<Box<RawValue>>> {
        let seq = self.change_seq();
        let stored = onboarding::get(self.storage()).await?;
        Ok(Seqd {
            value: st::read_onboarding(stored.as_deref()),
            seq,
        })
    }

    /// Merge the patch's top-level fields into the stored record
    /// (`onboardingPatch`), read over the defaults, so a write keeps every
    /// field.
    pub async fn patch_onboarding(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        patch: Box<RawValue>,
    ) -> Result<Seqd<Box<RawValue>>> {
        debug!(activity = "settings.onboardingPatch"; "Change the onboarding state");
        let stl = core.state_limits();
        st::check_onboarding_patch(&patch, &core.library_limits(), &stl)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let stored = onboarding::get(&mut tx).await?;
        let merged = st::merge_onboarding(stored.as_deref(), &patch);
        if let Some(max) = stl.max_setting_bytes {
            if merged.get().len() > max {
                return Err(LibraryError::invalid(format!(
                    "The onboarding record is larger than allowed here (max_setting_bytes: {max} bytes)."
                ))
                .into());
            }
        }
        onboarding::set(&mut tx, &merged).await?;
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::Onboarding, None, None, origin);
        Ok(Seqd { value: merged, seq })
    }

    /// Every tutorial progress row (`tutorialList`); `state` stays text.
    pub async fn list_tutorial(&self) -> Result<Seqd<Vec<TutorialProgress>>> {
        let seq = self.change_seq();
        let value = tutorial::list(self.storage()).await?;
        Ok(Seqd { value, seq })
    }

    /// Save one challenge's progress (`tutorialSave`), and answer every row.
    pub async fn save_tutorial(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        progress: TutorialProgress,
    ) -> Result<Seqd<Vec<TutorialProgress>>> {
        debug!(activity = "settings.tutorialSave", state = progress.state.is_some(); "Save tutorial progress");
        st::check_tutorial_ids(
            &progress.lesson_id,
            Some(&progress.challenge_id),
            &core.library_limits(),
        )?;
        st::check_tutorial_state(progress.state.as_deref(), &core.state_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        tutorial::save_in(
            &mut tx,
            &progress.lesson_id,
            &progress.challenge_id,
            progress.state.as_deref(),
        )
        .await?;
        let value = tutorial::list(&mut tx).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Tutorial,
            None,
            Some(vec![progress.lesson_id]),
            origin,
        );
        Ok(Seqd { value, seq })
    }

    /// Delete a lesson's progress (`tutorialRemoveLesson`).
    pub async fn remove_tutorial_lesson(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        lesson_id: &str,
    ) -> Result<Seqd<Vec<TutorialProgress>>> {
        debug!(activity = "settings.tutorialRemoveLesson"; "Reset a tutorial lesson");
        st::check_tutorial_ids(lesson_id, None, &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        tutorial::remove_lesson_in(&mut tx, lesson_id).await?;
        let value = tutorial::list(&mut tx).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Tutorial,
            None,
            Some(vec![lesson_id.to_string()]),
            origin,
        );
        Ok(Seqd { value, seq })
    }

    /// Delete every tutorial progress row (`tutorialReset`).
    pub async fn reset_tutorial(
        &self,
        origin: &WriteOrigin,
    ) -> Result<Seqd<Vec<TutorialProgress>>> {
        debug!(activity = "settings.tutorialReset"; "Reset the tutorial");
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        tutorial::remove_all_in(&mut tx).await?;
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::Tutorial, None, None, origin);
        Ok(Seqd {
            value: Vec::new(),
            seq,
        })
    }

    /// One import source's state (`importStateGet`), `None` before it's
    /// first saved.
    pub async fn get_import_state(&self, source: &str) -> Result<Seqd<Option<ImportState>>> {
        st::check_import_source(source)?;
        let seq = self.change_seq();
        let value = import_state::get(self.storage(), source).await?;
        Ok(Seqd { value, seq })
    }

    /// Save one import source's state (`importStateSave`).
    pub async fn save_import_state(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        source: &str,
        state: ImportState,
    ) -> Result<Seqd<ImportState>> {
        st::check_import_source(source)?;
        st::check_import_time(
            state.last_check_timestamp.as_deref(),
            &core.library_limits(),
        )?;
        debug!(activity = "settings.importStateSave", source = source; "Save an import's state");
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        import_state::save_in(
            &mut tx,
            source,
            state.has_offered_import,
            state.last_check_timestamp.as_deref(),
        )
        .await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::ImportState,
            None,
            Some(vec![source.to_string()]),
            origin,
        );
        Ok(Seqd { value: state, seq })
    }
}

// ── Window view state (Decision 22) ──

/// A `ui` call names the calling window: its id must be the call's origin
/// (Decision 18), so one tab can't read or overwrite another's view.
fn check_window(origin: &WriteOrigin, window_id: &str) -> Result<()> {
    st::check_window_id(window_id)?;
    if origin.as_deref() == Some(window_id) {
        Ok(())
    } else {
        Err(LibraryError::invalid("A window can only use its own view state.").into())
    }
}

impl Workspace {
    /// The window's active project (`windowGet`): its own; for a window
    /// with none, the most recently used window's that has one; else
    /// `lastActiveProjectId`; else none. Writes nothing; the GUI checks the
    /// id against the projects it listed.
    pub async fn get_window(
        &self,
        origin: &WriteOrigin,
        window_id: &str,
    ) -> Result<Seqd<WindowActive>> {
        check_window(origin, window_id)?;
        let seq = self.change_seq();
        let st = self.storage();
        let own = windows::get(st, window_id)
            .await?
            .and_then(|w| w.active_project_id);
        let value = if let Some(p) = own {
            WindowActive {
                active_project_id: Some(p),
                from: Some(WindowFrom::Window),
            }
        } else if let Some(p) = windows::most_recent_active(st)
            .await?
            .and_then(|w| w.active_project_id)
        {
            WindowActive {
                active_project_id: Some(p),
                from: Some(WindowFrom::Recent),
            }
        } else if let Some(p) = app_state::get(st, LAST_ACTIVE_PROJECT_KEY).await? {
            WindowActive {
                active_project_id: Some(p),
                from: Some(WindowFrom::LastActive),
            }
        } else {
            WindowActive {
                active_project_id: None,
                from: None,
            }
        };
        Ok(Seqd { value, seq })
    }

    /// Make `project_id` the window's active project (`windowActivate`),
    /// marking the window used, and write `lastActiveProjectId` in the same
    /// transaction so older releases and a new window still find it.
    pub async fn activate_window(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        window_id: &str,
        project_id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "ui.windowActivate", project_id = log_id(project_id); "Activate a project in a window");
        check_window(origin, window_id)?;
        check_id(project_id, "project id", &core.library_limits())?;
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        project_exists(&mut tx, project_id).await?;
        windows::set_active_project(&mut tx, window_id, project_id, &now).await?;
        app_state::set_in(&mut tx, LAST_ACTIVE_PROJECT_KEY, Some(project_id)).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::ProjectState,
            Some(project_id.to_string()),
            Some(vec![window_id.to_string()]),
            origin,
        );
        Ok(Seqd { value: (), seq })
    }

    /// The window's view state of a project (`windowStateLoad`). Its own
    /// row when it has one (nothing is written). Otherwise, in order: a
    /// copy of the project's most recently used window's row; today's
    /// `project_state` and `tabs` rows (the first window after the
    /// upgrade); nothing (`empty`, the GUI adds the starter tabs). A copy
    /// is written as the window's row at once (rev 0, or one past a stored
    /// row that doesn't read), so it doesn't change under the window
    /// before its first save.
    pub async fn load_window_state(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        window_id: &str,
        project_id: &str,
    ) -> Result<Seqd<WindowStateLoaded>> {
        check_window(origin, window_id)?;
        check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        if let Some(row) = window_state::get(self.storage(), window_id, project_id).await? {
            if let Some(state) = row.state {
                return Ok(Seqd {
                    value: WindowStateLoaded {
                        state: Some(state),
                        rev: row.rev,
                        copied_from: None,
                    },
                    seq,
                });
            }
        }
        debug!(activity = "ui.windowStateLoad", project_id = log_id(project_id); "Load a window's first view of a project");
        let (_, now) = clock(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        project_exists(&mut tx, project_id).await?;
        // Again inside the write: another call of this window may have
        // saved since.
        let own = window_state::get(&mut tx, window_id, project_id).await?;
        if let Some(row) = &own {
            if let Some(state) = &row.state {
                drop(tx);
                drop(ticket);
                return Ok(Seqd {
                    value: WindowStateLoaded {
                        state: Some(state.clone()),
                        rev: row.rev,
                        copied_from: None,
                    },
                    seq: self.change_seq(),
                });
            }
        }
        let recent = window_state::most_recent(&mut tx, project_id)
            .await?
            .filter(|r| r.window_id != window_id)
            .and_then(|r| r.state);
        let (state, copied_from) = match recent {
            Some(state) => (state, CopiedFrom::Window),
            None => match project_state::load(&mut tx, project_id).await? {
                Some(legacy) => (st::legacy_view(&legacy), CopiedFrom::Legacy),
                None => {
                    drop(tx);
                    drop(ticket);
                    // A stored row that doesn't read keeps its rev: the
                    // page counts up from it, so its first save lands.
                    return Ok(Seqd {
                        value: WindowStateLoaded {
                            state: None,
                            rev: own.map_or(0, |r| r.rev),
                            copied_from: Some(CopiedFrom::Empty),
                        },
                        seq: self.change_seq(),
                    });
                }
            },
        };
        let rev = own.map_or(0, |r| r.rev.saturating_add(1));
        windows::touch(&mut tx, window_id, &now).await?;
        let put =
            window_state::put_if_newer(&mut tx, window_id, project_id, rev, state.get(), &now)
                .await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::ProjectState,
            Some(project_id.to_string()),
            Some(vec![window_id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: WindowStateLoaded {
                state: Some(state),
                rev: put.rev,
                copied_from: Some(copied_from),
            },
            seq,
        })
    }

    /// Save the window's view state of a project (`windowStateSave`),
    /// replacing its row when `rev` is higher than the stored one, and in
    /// the same transaction the legacy mirror (today's `project_state` and
    /// `tabs` rows, keeping the stored connection order) and the bounded
    /// prunes (windows unused for 30 days, past `max_windows`, and the
    /// project's view states past `max_window_states_per_project`, never
    /// this window or, on the desktop, `main`).
    ///
    /// On the web a state past a size limit (its bytes, its tabs, a tab's
    /// text) is still accepted when it is no larger than the window's
    /// stored row, item by item, so a state stored before the limits stays
    /// saveable and can shrink; only growth past a limit is refused.
    ///
    /// A stale save (a `rev` not higher than the stored one) writes nothing
    /// at all, emits nothing, and answers `stale` with the stored `rev`.
    pub async fn save_window_state(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        window_id: &str,
        project_id: &str,
        rev: u64,
        state: Box<RawValue>,
    ) -> Result<Seqd<WindowStateSaved>> {
        check_window(origin, window_id)?;
        let stl = core.state_limits();
        check_id(project_id, "project id", &core.library_limits())?;
        // At most 2^53 - 1, which the page's counter holds exactly.
        st::check_rev(rev)?;
        // A body past `max_view_state_bytes` and larger than the window's
        // stored state is refused before it is parsed (one indexed read of
        // the stored length); one no larger than the stored state goes on
        // to the item-by-item check below.
        if stl
            .max_view_state_bytes
            .is_some_and(|max| state.get().len() > max)
        {
            let stored = window_state::state_bytes(self.storage(), window_id, project_id)
                .await?
                .unwrap_or(0);
            if state.get().len() as u64 > stored {
                st::check_view_state_size(&state, &stl)?;
            }
        }
        let mirror = st::parse_view_state(&state, project_id)?;
        let size = st::view_state_size(&state, &mirror);
        // Within the limits: nothing to read. Past one, the save is still
        // accepted when it is no larger than the window's stored row, item
        // by item, which is read inside the write below.
        let over = st::check_view_state_limits(&size, None, &stl).is_err();
        debug!(activity = "ui.windowStateSave", project_id = log_id(project_id), bytes = state.get().len(); "Save a window's view state");
        let (t, now) = clock(core)?;
        let unused_before =
            iso_timestamp(t.saturating_sub(Duration::from_secs(WINDOW_UNUSED_DAYS * 24 * 60 * 60)));
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        project_exists(&mut tx, project_id).await?;
        if over {
            let stored = window_state::get(&mut tx, window_id, project_id)
                .await?
                .and_then(|row| row.state)
                .and_then(|raw| {
                    st::parse_view_state(&raw, project_id)
                        .ok()
                        .map(|s| st::view_state_size(&raw, &s))
                });
            st::check_view_state_limits(&size, stored.as_ref(), &stl)?;
        }
        windows::touch(&mut tx, window_id, &now).await?;
        let put =
            window_state::put_if_newer(&mut tx, window_id, project_id, rev, state.get(), &now)
                .await?;
        if !put.written {
            // Nothing else is written: the touch goes with the rollback.
            tx.rollback().await?;
            drop(ticket);
            return Ok(Seqd {
                value: WindowStateSaved {
                    stale: true,
                    rev: put.rev,
                },
                seq: self.change_seq(),
            });
        }
        let skipped = project_state::write_legacy_mirror(&mut tx, &mirror).await?;
        let mut spare = vec![window_id];
        if stl.spare_main_window && window_id != MAIN_WINDOW {
            spare.push(MAIN_WINDOW);
        }
        let pruned_windows =
            windows::prune(&mut tx, &unused_before, Some(stl.max_windows), &spare).await?;
        let pruned_states = window_state::prune_for_project(
            &mut tx,
            project_id,
            stl.max_window_states_per_project,
            &spare,
        )
        .await?;
        tx.commit().await?;
        if skipped > 0 || pruned_windows > 0 || pruned_states > 0 {
            debug!(activity = "ui.windowStateSave", skipped_tabs = skipped, pruned_windows = pruned_windows, pruned_states = pruned_states; "Saved a view state");
        }
        let seq = self.announce(
            ticket,
            StoredKind::ProjectState,
            Some(project_id.to_string()),
            Some(vec![window_id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: WindowStateSaved {
                stale: false,
                rev: put.rev,
            },
            seq,
        })
    }
}
