//! The TUI's Core: built by `seaquel-terminal`
//! with the AI client the desktop's has (Ask AI), and one workspace
//! on the app's data dir opened as a **second process**: writable, no
//! schema work (anything pending is `STORAGE_NEEDS_UPGRADE`), no
//! maintenance writes, and external changes polled every second. Secrets
//! go through `SecretWait`, so the screen can say it's waiting for a
//! keychain dialog.
//!
//! Every Core call the TUI makes is a method of [`Session`], answering in
//! the model's own types; the runtime (`effects.rs`) runs them as tasks and
//! sends the answers to `update`. Writes carry the TUI's origin
//! (`tui-<8 hex>`, random per process), so its own `StorageChanged` events
//! are told apart from another writer's.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use seaquel_core::domain::library::{ConnectionPatch, SecretChanges};
use seaquel_core::secrets::{SecretStore, SecretWait};
use seaquel_core::{
    ConnectRequest, Core, CoreError, DuckdbHelperProgress, DuckdbHelperStatus, HostKeyPolicy,
    Workspace, WorkspaceSpec, WriteOrigin,
};
use seaquel_types::storage::{
    PersistedConnection, PersistedQueryHistoryItem, PersistedSavedQuery, SshTunnelConfig,
};
use seaquel_types::{
    CreateTableColumn, CreateTableDefinition, CreateTableForeignKey, CreateTableIndex,
    SchemaColumn, SchemaIndex, SchemaTable, TableKind as CoreTableKind,
};

use seaquel_core::domain::edits::{PlannedChange, TableTarget};

use super::clock;
use crate::state::app::{ConnectCall, SaveCall};
use crate::state::ask::GenerateCall;
use crate::state::browse::{PageCall, PlanCall, TableMeta};
use crate::state::commit::HistoryLabel;
use crate::state::commit::{Applied, ApplyCall, Destructive, Failure};
use crate::state::dialogs::CallError;
use crate::state::grid::Page;
use crate::state::install::Offer;
use crate::state::panels::{
    ConnAi, ConnItem, HistoryItem, LabelItem, Library, ProjectItem, SavedItem, TableItem,
    TableKind, Tunnel, TunnelAuth,
};
use crate::state::query::{ExplainCall, PageRunCall, RunCall, RunMsg, SaveKind, SaveQueryCall};
use crate::state::secrets::SecretKind;
use seaquel_types::ExplainResult;

/// How often the workspace looks for another process's writes.
pub const EXTERNAL_POLL: Duration = Duration::from_secs(1);

/// What [`open`] needs.
pub struct OpenOptions {
    pub data_dir: PathBuf,
    /// The keychain, or the test hook's `MemoryStore`.
    pub store: Arc<dyn SecretStore>,
    /// What `seaquel-terminal`'s Core takes: the test hooks' known_hosts
    /// and DuckDB helper and release server (`CoreOptions::with_hooks`),
    /// and the DuckDB helper's folder (tests give one under their data dir).
    pub core: seaquel_terminal::CoreOptions,
    pub poll: Duration,
    /// The test hook's origin; else a random one.
    pub origin: Option<String>,
}

/// The open Core and workspace.
pub struct Session {
    pub core: Arc<Core>,
    pub ws: Arc<Workspace>,
    pub origin: WriteOrigin,
    pub wait: Arc<SecretWait>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session")
    }
}

/// The TUI's Core: `seaquel-terminal`'s, plus the native AI client with
/// `AiEgress::Any`, as `src-tauri`'s `desktop_core` (Task 7 uses it).
/// DuckDB runs in the helper process, which the
/// TUI can download (`seaquel-terminal`'s `duckdb-helper-install`).
pub fn build_core(options: seaquel_terminal::CoreOptions) -> Core {
    use seaquel_core::ai::native::{NativeHttp, NativeHttpOptions};
    use seaquel_core::ai::AiEgress;

    let egress = AiEgress::Any;
    let http = NativeHttp::new(NativeHttpOptions::new(egress.into()));
    // Every test build calls models through a client that panics on any
    // host but loopback: no test can reach a real provider.
    #[cfg(test)]
    let http = seaquel_ai::testing::LoopbackOnly(http);
    seaquel_terminal::core_builder(seaquel_terminal::CoreOptions {
        local_files: None,
        ..options
    })
    .ai_http(Arc::new(http))
    .ai_egress(egress)
    .build()
}

/// A write origin: the test hook's, else `tui-<8 hex>`.
pub fn new_origin(hook: Option<&str>) -> WriteOrigin {
    if let Some(hook) = hook {
        return WriteOrigin::new(Some(hook));
    }
    use std::hash::{BuildHasher, Hasher};
    // `RandomState` is seeded randomly per process and moves on with each
    // `new()`; the time and pid only add to it.
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    hasher.write_u32(std::process::id());
    WriteOrigin::new(Some(&format!("tui-{:08x}", hasher.finish() as u32)))
}

/// Opens Core on the data dir as a second process.
pub async fn open(options: OpenOptions) -> Result<Session, CoreError> {
    let core = Arc::new(build_core(options.core));
    let wait = SecretWait::new();
    let spec = WorkspaceSpec::new(&options.data_dir)
        .with_secrets(wait.watch(options.store))
        .second_process()
        .with_external_changes(options.poll);
    let ws = core.open_workspace(spec).await?;
    Ok(Session {
        core,
        ws,
        origin: new_origin(options.origin.as_deref()),
        wait,
    })
}

/// A startup refusal as `seaquel-tui` prints it (after the terminal is
/// restored, or before it was ever taken). Core's message says what's
/// wrong; a file that needs an upgrade also says which app version (the TUI may be newer than the app).
pub fn startup_message(error: &CoreError) -> String {
    match error.code.as_str() {
        "STORAGE_NEEDS_UPGRADE" => format!(
            "{}\nThis file needs Seaquel {version} or later. Open the app (version {version} or \
             later) once, then start seaquel-tui again.",
            error.message,
            version = seaquel_terminal::VERSION
        ),
        _ => error.message.clone(),
    }
}

fn call_error(e: CoreError) -> CallError {
    CallError::new(e.code, e.message)
}

fn db_error(e: seaquel_types::DbError) -> CallError {
    CallError::new(e.code, e.message)
}

/// A stored port as a TCP port; `None` when it isn't one.
fn port(n: f64) -> Option<u16> {
    (n.fract() == 0.0 && (0.0..=65535.0).contains(&n)).then_some(n as u16)
}

/// A connection as the model keeps it; `global` is the app's AI sharing
/// default, which Core's rule applies under the row's own flags.
fn conn_item(row: PersistedConnection, global: seaquel_core::ai::Sharing) -> ConnItem {
    let sharing = seaquel_core::ai::sharing::sharing(&row, global);
    let ai = ConnAi {
        schema: sharing.schema,
        data: sharing.data,
        model: row.active_ai_model.clone().filter(|m| !m.is_empty()),
    };
    let tunnel = row
        .ssh_tunnel
        .as_deref()
        .and_then(|raw| serde_json::from_str::<SshTunnelConfig>(raw.get()).ok())
        .filter(|t| t.enabled)
        .map(|t| Tunnel {
            port: port(t.port).filter(|p| *p != 0).unwrap_or(22),
            auth: if t.auth_method == "key" {
                TunnelAuth::Key
            } else {
                TunnelAuth::Password
            },
            host: t.host,
        });
    ConnItem {
        port: port(row.port),
        id: row.id,
        project_id: row.project_id,
        name: row.name,
        engine: row.ty,
        host: row.host,
        database: row.database_name,
        save_password: row.save_password,
        save_ssh_password: row.save_ssh_password,
        save_ssh_key_passphrase: row.save_ssh_key_passphrase,
        tunnel,
        label_ids: row.label_ids,
        ai,
    }
}

fn saved_item(row: PersistedSavedQuery) -> SavedItem {
    SavedItem {
        id: row.id,
        name: row.name,
        folder: row.folder,
        shared: row.shared_path.is_some(),
        sql: row.query,
    }
}

fn history_item(row: PersistedQueryHistoryItem) -> HistoryItem {
    HistoryItem {
        when: clock::local_when(&row.timestamp),
        id: row.id,
        sql: row.query,
        elapsed_ms: row.execution_time,
        rows: row.row_count,
    }
}

fn table_item(t: SchemaTable) -> TableItem {
    TableItem {
        kind: match t.kind {
            CoreTableKind::Table => TableKind::Table,
            CoreTableKind::View => TableKind::View,
            CoreTableKind::MaterializedView => TableKind::MaterializedView,
        },
        row_count: t.row_count,
        columns: t.columns.into_iter().map(|c| (c.name, c.ty)).collect(),
        schema: t.schema,
        name: t.name,
    }
}

impl Session {
    /// The projects and connections.
    pub async fn library(&self) -> Result<Library, CallError> {
        let projects = self.ws.list_projects().await.map_err(call_error)?.value;
        let connections = self.ws.list_connections().await.map_err(call_error)?.value;
        // Ask AI's title: the app's AI settings, read as Core
        // reads them for a model call.
        let settings = self.ws.get_ai_settings().await.map_err(call_error)?.value;
        let raw = settings.get();
        let global = seaquel_core::ai::sharing::global_sharing_from(Some(raw));
        let ai_off = !seaquel_core::domain::state::read_ai_settings(Some(raw)).enabled();
        let labels = projects
            .iter()
            .flat_map(|p| {
                p.custom_labels.iter().map(|l| LabelItem {
                    project_id: p.id.clone(),
                    id: l.id.clone(),
                    name: l.name.clone(),
                    color: l.color.clone(),
                })
            })
            .collect();
        Ok(Library {
            projects: projects
                .into_iter()
                .map(|p| ProjectItem {
                    id: p.id,
                    name: p.name,
                })
                .collect(),
            connections: connections
                .into_iter()
                .map(|row| conn_item(row, global))
                .collect(),
            labels,
            ai_off,
        })
    }

    /// A project's saved queries.
    pub async fn saved(&self, project_id: &str) -> Result<Vec<SavedItem>, CallError> {
        let rows = self
            .ws
            .list_saved_queries(&self.core, project_id)
            .await
            .map_err(call_error)?
            .value;
        Ok(rows.into_iter().map(saved_item).collect())
    }

    /// A saved connection's history, newest first.
    pub async fn history(&self, connection_id: &str) -> Result<Vec<HistoryItem>, CallError> {
        let rows = seaquel_core::storage::query_history::load_by_connection(
            self.ws.storage(),
            connection_id,
        )
        .await
        .map_err(|e| CallError::new(e.code(), e.to_string()))?;
        Ok(rows.into_iter().map(history_item).collect())
    }

    /// The connected database's tables, views and materialized views.
    pub async fn schema(&self, core_id: &str) -> Result<Vec<TableItem>, CallError> {
        let tables = self
            .ws
            .engine(&self.core, core_id)
            .map_err(db_error)?
            .schema_tables()
            .await
            .map_err(db_error)?;
        Ok(tables.into_iter().map(table_item).collect())
    }

    /// Connects a saved connection; Core's connection id.
    pub async fn connect(&self, call: &ConnectCall) -> Result<String, CallError> {
        let mut req = ConnectRequest::saved(&call.connection_id)
            .with_secrets(call.secrets.supplied())
            .with_origin(self.origin.clone());
        if let Some(fingerprint) = &call.trust {
            req = req.with_host_key(HostKeyPolicy::Trust(fingerprint.clone()));
        }
        self.ws.connect(&self.core, req).await.map_err(call_error)
    }

    /// DuckDB support's download: its
    /// size from the release metadata (one request), and whether the
    /// helper already there sits in a folder that isn't private.
    pub async fn duckdb_offer(&self) -> Result<Offer, CallError> {
        let status = self.core.duckdb_helper_status().map_err(call_error)?;
        let asset = self.core.duckdb_helper_asset().await.map_err(call_error)?;
        Ok(Offer {
            size: asset.size,
            repair: status == DuckdbHelperStatus::Unsafe,
        })
    }

    /// Downloads and installs DuckDB support, `progress` getting the
    /// compressed bytes received and the total. Dropping the future stops
    /// the download and leaves nothing behind.
    pub async fn install_duckdb(
        &self,
        mut progress: impl FnMut(u64, u64) + Send,
    ) -> Result<(), CallError> {
        let mut report = |p: DuckdbHelperProgress| progress(p.bytes, p.total);
        self.core
            .duckdb_helper_install(&mut report)
            .await
            .map(drop)
            .map_err(call_error)
    }

    pub async fn disconnect(&self, core_id: &str) {
        if let Err(e) = self.ws.disconnect(&self.core, core_id).await {
            log::warn!(activity = "tui.disconnect", code = e.code.as_str(); "Disconnect failed");
        }
    }

    /// "Save password": `connectionUpdate` with the save
    /// flags and the secrets together, so Core writes the keychain before
    /// the row and refuses a secret whose flag would end up off.
    pub async fn save_password(&self, call: &SaveCall) -> Result<(), CallError> {
        let (patch, secrets) = save_changes(call)?;
        self.ws
            .update_connection(
                &self.core,
                &self.origin,
                &call.connection_id,
                patch,
                secrets,
            )
            .await
            .map(|_| ())
            .map_err(call_error)
    }

    /// One page of a table (`table_page`), read whole: Core's SELECT, the
    /// rows and the counts. Dropping the future drops the stream, which
    /// cancels the page on the server.
    pub async fn table_page(&self, call: &PageCall) -> Result<Page, CallError> {
        use futures::StreamExt;
        use seaquel_core::domain::edits::TablePageParams;
        use seaquel_core::domain::run::RunEvent;

        let mut events = self.ws.table_page(
            &self.core,
            TablePageParams {
                connection_id: call.core_id.clone(),
                stream_id: format!("tui-page-{}", call.op),
                query: call.query.clone(),
                page: call.page,
                page_size: call.page_size,
            },
        );
        let mut page = Page {
            sql: String::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            page: call.page,
            page_size: call.page_size,
            total_rows: 0,
            total_pages: 1,
            count_estimated: false,
            elapsed_ms: 0.0,
        };
        while let Some(event) = events.next().await {
            match event {
                RunEvent::StatementStart { sql, .. } => page.sql = sql,
                RunEvent::Batch(batch) => {
                    if let Some(columns) = batch.columns {
                        page.columns = columns;
                    }
                    page.rows.extend(batch.rows);
                }
                RunEvent::StatementDone {
                    elapsed_ms,
                    total_rows,
                    total_pages,
                    count_estimated,
                    ..
                } => {
                    page.elapsed_ms = elapsed_ms;
                    page.total_rows = total_rows;
                    page.total_pages = total_pages;
                    page.count_estimated = count_estimated;
                }
                RunEvent::StatementError { code, message, .. }
                | RunEvent::Error { code, message, .. } => {
                    return Err(CallError::new(code, message))
                }
                RunEvent::Done { .. } => return Ok(page),
                RunEvent::StatementDeferred { .. } => {}
            }
        }
        // Ended with no `done`: cancelled (the connection or workspace went).
        Err(CallError::new("CANCELLED", "The page was cancelled."))
    }

    /// A table's columns, names and types (`table_metadata`), for
    /// completion (`schema_tables` lists none).
    pub async fn table_columns(
        &self,
        core_id: &str,
        target: &TableTarget,
    ) -> Result<Vec<(String, String)>, CallError> {
        let handle = self.ws.engine(&self.core, core_id).map_err(db_error)?;
        let (columns, _) = handle
            .table_metadata(&target.schema, &target.table)
            .await
            .map_err(db_error)?;
        Ok(columns.into_iter().map(|c| (c.name, c.ty)).collect())
    }

    /// A table's metadata, and the approximate DDL Core's dialect builds
    /// from it (the DDL tab is headed "approximate").
    pub async fn table_meta(
        &self,
        core_id: &str,
        target: &TableTarget,
    ) -> Result<TableMeta, CallError> {
        let handle = self.ws.engine(&self.core, core_id).map_err(db_error)?;
        let (columns, indexes) = handle
            .table_metadata(&target.schema, &target.table)
            .await
            .map_err(db_error)?;
        let definition = definition(target, &columns, &indexes);
        let ddl = handle
            .with_dialect(|d| d.create_table(&definition))
            .map_err(db_error);
        Ok(TableMeta {
            columns,
            indexes,
            ddl,
        })
    }

    /// Core's plan for one staged entry (`plan_edits`).
    pub async fn plan_edit(&self, call: &PlanCall) -> Result<PlannedChange, CallError> {
        use seaquel_core::domain::edits::PlanEditsParams;
        let mut planned = self
            .ws
            .plan_edits(
                &self.core,
                PlanEditsParams {
                    connection_id: call.core_id.clone(),
                    edits: vec![call.request.edit.clone()],
                },
            )
            .await
            .map_err(call_error)?;
        planned
            .pop()
            .ok_or_else(|| CallError::new("INVALID_ARGUMENT", "Core planned nothing."))
    }

    /// Commits the staged changes (`apply_changes`) with a history
    /// context, so Core records each applied change under the saved
    /// connection; the history rows come back in the answer. Carries the
    /// TUI's origin, so its own `StorageChanged` event is skipped.
    pub async fn apply(&self, call: &ApplyCall) -> Result<Applied, CallError> {
        use seaquel_core::domain::edits::{ApplyChangesParams, ApplyOutcome};

        let history = history_context(&call.connection_id, &call.connection_name, &call.labels)?;
        let outcome = self
            .ws
            .apply_changes_from(
                &self.core,
                ApplyChangesParams {
                    connection_id: call.core_id.clone(),
                    changes: call.changes.clone(),
                    confirmed: call.confirmed,
                    history: Some(history),
                },
                &self.origin,
            )
            .await
            .map_err(call_error)?;
        Ok(match outcome {
            ApplyOutcome::Applied {
                mode,
                applied,
                results,
                failed,
                ddl,
                history,
            } => Applied::Applied {
                mode,
                applied,
                results: results.into_iter().map(|r| r.id).collect(),
                failed: failed.map(|f| Failure {
                    id: f.id,
                    error: CallError::new(f.code, f.message),
                }),
                ddl,
                history: history.into_iter().map(history_item).collect(),
            },
            ApplyOutcome::ConfirmRequired {
                destructive,
                destructive_total,
            } => Applied::ConfirmRequired {
                destructive: destructive
                    .into_iter()
                    .map(|d| Destructive {
                        sql: d.sql,
                        reason: seaquel_terminal::destructive_reason(d.reason).to_string(),
                    })
                    .collect(),
                total: destructive_total,
            },
        })
    }

    /// A query tab's run (`db.run`, with the TUI's origin and its history
    /// context): each event goes to `send` as it comes, in the model's
    /// types, and a stream that ends with neither `done` nor `error` ends
    /// with [`RunMsg::Ended`]. Dropping the future drops the stream, which
    /// cancels the run in Core.
    pub async fn run(&self, call: &RunCall, send: impl FnMut(RunMsg)) {
        use seaquel_core::domain::run::{ParamValue, RunParams};
        let history = match &call.history {
            Some(h) => match history_context(&h.connection_id, &h.connection_name, &h.labels) {
                Ok(context) => Some(context),
                Err(error) => {
                    let mut send = send;
                    send(RunMsg::Refused {
                        error,
                        destructive: None,
                    });
                    return;
                }
            },
            None => None,
        };
        let params = RunParams {
            connection_id: call.core_id.clone(),
            stream_id: call.stream_id.clone(),
            text: call.text.clone(),
            target: call.target,
            params: call.params.as_ref().map(|values| {
                values
                    .iter()
                    .map(|(name, value)| ParamValue {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect()
            }),
            page_size: call.page_size,
            confirmed: call.confirmed,
            defer_writes: false,
            history,
        };
        let events = self.ws.run_from(&self.core, params, self.origin.clone());
        forward(events, send).await;
    }

    /// `db.page` for one statement of a tab's last run, as [`Session::run`].
    pub async fn page(&self, call: &PageRunCall, send: impl FnMut(RunMsg)) {
        use seaquel_core::domain::run::PageParams;
        let events = self.ws.page(
            &self.core,
            PageParams {
                connection_id: call.core_id.clone(),
                stream_id: call.stream_id.clone(),
                source: call.source.clone(),
                page: call.page,
                page_size: call.page_size,
            },
        );
        forward(events, send).await;
    }

    /// Core's `explain` of the statement as typed (no parameters).
    pub async fn explain(&self, call: &ExplainCall) -> Result<Box<ExplainResult>, CallError> {
        self.ws
            .engine(&self.core, &call.core_id)
            .map_err(db_error)?
            .explain(&call.sql, Vec::new(), call.analyze)
            .await
            .map(Box::new)
            .map_err(db_error)
    }

    /// `savedQueryCreate` or `savedQueryUpdate` of a tab's text, with the
    /// TUI's origin. A `NAME_TAKEN` answer carries the id of the row that has
    /// the name.
    pub async fn save_query(
        &self,
        call: &SaveQueryCall,
    ) -> Result<SavedItem, (CallError, Option<String>)> {
        use seaquel_core::domain::library::{SavedQueryDraft, SavedQueryPatch};
        let refused = |e: CoreError| {
            let taken_by = e.taken_by.clone();
            (CallError::new(e.code, e.message), taken_by)
        };
        let row = match &call.save {
            SaveKind::Create { project_id, name } => {
                let draft: SavedQueryDraft = serde_json::from_value(serde_json::json!({
                    "projectId": project_id,
                    "name": name,
                    "query": call.text,
                }))
                .map_err(|_| {
                    (
                        CallError::new("INVALID_ARGUMENT", "The saved query couldn't be sent."),
                        None,
                    )
                })?;
                self.ws
                    .create_saved_query(&self.core, &self.origin, draft)
                    .await
                    .map_err(refused)?
                    .value
            }
            SaveKind::Update { id } => {
                let patch = SavedQueryPatch {
                    query: Some(call.text.clone()),
                    ..SavedQueryPatch::default()
                };
                self.ws
                    .update_saved_query(&self.core, &self.origin, id, patch)
                    .await
                    .map_err(refused)?
                    .value
                    .query
            }
        };
        Ok(saved_item(row))
    }

    /// Ask AI: Core's `ai_generate` for the saved connection, the
    /// request as typed and the existing query. Core reads the provider,
    /// model, sharing and key (the keychain through `SecretWait`); dropping
    /// the future drops the HTTP request.
    pub async fn generate(&self, call: &GenerateCall) -> Result<String, CallError> {
        use seaquel_core::ai::GenerateParams;
        self.ws
            .ai_generate(
                &self.core,
                GenerateParams {
                    connection_id: call.connection_id.clone(),
                    request: call.request.clone(),
                    existing_query: call.existing.clone(),
                    api_key: None,
                    provider_id: None,
                },
            )
            .await
            .map_err(call_error)
    }

    /// A project's dashboards' names, for Ask AI's `@` list.
    pub async fn dashboard_names(&self, project_id: &str) -> Result<Vec<String>, CallError> {
        let rows = self
            .ws
            .list_dashboards(&self.core, project_id)
            .await
            .map_err(call_error)?
            .value;
        Ok(rows.into_iter().map(|d| d.name).collect())
    }

    /// Cancels a stream in Core (a run whose task was dropped, or one whose
    /// start this cancel overtook).
    pub fn cancel(&self, stream_id: &str) {
        self.ws.cancel(&self.core, stream_id);
    }

    /// Closes every connection and tunnel, then the storage.
    pub async fn close(&self) {
        self.ws.close_all(&self.core).await;
        self.ws.close().await;
    }
}

/// The history context `db.run` and `apply_changes` take: the saved
/// connection and its labels as the GUI snapshots them.
fn history_context(
    connection_id: &str,
    connection_name: &str,
    labels: &[HistoryLabel],
) -> Result<seaquel_core::domain::run::HistoryContext, CallError> {
    use seaquel_types::storage::ConnectionLabel;
    let labels: Vec<ConnectionLabel> = labels
        .iter()
        .map(|l| ConnectionLabel {
            id: l.id.clone(),
            name: l.name.clone(),
            is_predefined: l.predefined,
            color: l.color.clone(),
        })
        .collect();
    let connection_labels = serde_json::value::to_raw_value(&labels)
        .map_err(|_| CallError::new("INVALID_ARGUMENT", "The labels couldn't be sent."))?;
    Ok(seaquel_core::domain::run::HistoryContext {
        connection_id: connection_id.to_string(),
        connection_name: connection_name.to_string(),
        connection_labels,
    })
}

/// A run's (or page's) events to `send` in the model's types, until `done`
/// or `error`; a stream that ends with neither ends with
/// [`RunMsg::Ended`].
async fn forward(
    mut events: futures::stream::BoxStream<'_, seaquel_core::domain::run::RunEvent>,
    mut send: impl FnMut(RunMsg),
) {
    use futures::StreamExt;
    while let Some(event) = events.next().await {
        let terminal = event.is_terminal();
        send(run_msg(event));
        if terminal {
            return;
        }
    }
    send(RunMsg::Ended);
}

/// A `RunEvent` as `update` takes it.
fn run_msg(event: seaquel_core::domain::run::RunEvent) -> RunMsg {
    use seaquel_core::domain::run::RunEvent;
    match event {
        RunEvent::StatementStart {
            index,
            sql,
            source,
            kind,
            page,
            page_size,
            ..
        } => RunMsg::Start {
            index,
            sql,
            source,
            kind,
            page,
            page_size,
        },
        RunEvent::Batch(batch) => RunMsg::Batch {
            columns: batch.columns,
            rows: batch.rows,
        },
        RunEvent::StatementDone {
            index,
            elapsed_ms,
            total_rows,
            total_pages,
            count_estimated,
            rows_affected,
            ..
        } => RunMsg::Done {
            index,
            elapsed_ms,
            total_rows,
            total_pages,
            count_estimated,
            rows_affected,
        },
        RunEvent::StatementError {
            index,
            code,
            message,
            elapsed_ms,
            sql,
        } => RunMsg::Failed {
            index,
            error: CallError::new(code, message),
            elapsed_ms,
            sql,
        },
        // Pending changes aren't on in the TUI (edits are always staged):
        // `deferWrites` is never sent, so this can't come.
        RunEvent::StatementDeferred { index, sql, .. } => RunMsg::Failed {
            index,
            error: CallError::new("DEFERRED", "The statement was deferred and didn't run."),
            elapsed_ms: 0.0,
            sql: Some(sql),
        },
        RunEvent::Done {
            statements,
            succeeded,
            history,
        } => RunMsg::Finished {
            statements,
            succeeded,
            history: history.map(history_item),
        },
        RunEvent::Error {
            code,
            message,
            destructive,
            destructive_total,
        } => RunMsg::Refused {
            destructive: destructive.map(|list| {
                let total = destructive_total.unwrap_or(list.len() as u32);
                (
                    list.into_iter()
                        .map(|d| Destructive {
                            sql: d.sql,
                            reason: seaquel_terminal::destructive_reason(d.reason).to_string(),
                        })
                        .collect(),
                    total,
                )
            }),
            error: CallError::new(code, message),
        },
    }
}

/// What "Save password" sends for `call` (M1): the ticked kinds' flags and
/// their secrets, each a value to set (never a delete); a kind with no
/// typed secret is skipped, an empty one refused, and nothing left to save
/// is refused too.
pub fn save_changes(call: &SaveCall) -> Result<(ConnectionPatch, SecretChanges), CallError> {
    let mut patch = ConnectionPatch::default();
    let mut secrets = SecretChanges::default();
    for kind in &call.kinds {
        let Some(secret) = call.secrets.get(*kind) else {
            continue;
        };
        if secret.is_empty() {
            return Err(CallError::new(
                "INVALID_ARGUMENT",
                "An empty password isn't saved.",
            ));
        }
        let set = Some(Some(secret.expose().to_string()));
        match kind {
            SecretKind::Db => {
                patch.save_password = Some(true);
                secrets.db = set;
            }
            SecretKind::Ssh => {
                patch.save_ssh_password = Some(true);
                secrets.ssh = set;
            }
            SecretKind::SshKey => {
                patch.save_ssh_key_passphrase = Some(true);
                secrets.ssh_key = set;
            }
        }
    }
    if secrets.db.is_none() && secrets.ssh.is_none() && secrets.ssh_key.is_none() {
        return Err(CallError::new(
            "INVALID_ARGUMENT",
            "There is no password to save.",
        ));
    }
    Ok((patch, secrets))
}

/// A table's metadata as the table editor's definition (what
/// `create_table` takes), as the GUI builds it: each column's type as Core
/// reported it, its default, key and unique flags; the indexes but the
/// primary key's; the foreign keys.
fn definition(
    target: &TableTarget,
    columns: &[SchemaColumn],
    indexes: &[SchemaIndex],
) -> CreateTableDefinition {
    let mut primary: Vec<&str> = columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    primary.sort_unstable();
    CreateTableDefinition {
        table_name: target.table.clone(),
        schema_name: target.schema.clone(),
        columns: columns
            .iter()
            .map(|c| CreateTableColumn {
                id: c.name.clone(),
                name: c.name.clone(),
                ty: c.ty.clone(),
                length: None,
                precision: None,
                nullable: c.nullable,
                default_value: c.default_value.clone().unwrap_or_default(),
                is_primary_key: c.is_primary_key,
                is_unique: c.is_unique,
                collation: c.collation.clone(),
                in_unique_constraint: c.in_unique_constraint,
            })
            .collect(),
        indexes: indexes
            .iter()
            .filter(|i| {
                let mut cols: Vec<&str> = i.columns.iter().map(String::as_str).collect();
                cols.sort_unstable();
                !(i.unique && cols == primary)
            })
            .map(|i| CreateTableIndex {
                id: i.name.clone(),
                name: i.name.clone(),
                columns: i.columns.clone(),
                unique: i.unique,
                ty: i.ty.clone(),
            })
            .collect(),
        foreign_keys: columns
            .iter()
            .filter_map(|c| {
                let fk = c.foreign_key_ref.as_ref()?;
                Some(CreateTableForeignKey {
                    id: c.name.clone(),
                    column: c.name.clone(),
                    referenced_schema: fk.referenced_schema.clone(),
                    referenced_table: fk.referenced_table.clone(),
                    referenced_column: fk.referenced_column.clone(),
                })
            })
            .collect(),
    }
}

/// Where the data dir's file is.
pub fn storage_file(data_dir: &Path) -> PathBuf {
    data_dir.join(seaquel_core::DESKTOP_STORAGE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::core::{memory_store, Seed};

    fn options(dir: &Path) -> OpenOptions {
        OpenOptions {
            data_dir: dir.to_path_buf(),
            store: memory_store(),
            core: seaquel_terminal::CoreOptions {
                duckdb_helper_dir: Some(dir.join("bin").join("duckdb")),
                ..seaquel_terminal::CoreOptions::default()
            },
            poll: Duration::from_millis(50),
            origin: Some("tui-test0001".into()),
        }
    }

    /// The TUI's Core runs DuckDB in the helper,
    /// looked for in the folder it's given; with none there a
    /// connect is `ENGINE_NOT_INSTALLED` (what Task 7's dialog opens on),
    /// and the download calls are compiled in.
    #[tokio::test]
    async fn duckdb_runs_in_the_helper_under_the_given_folder() {
        use seaquel_core::{ConnectRequest, ConnectionForm, DuckdbHelperStatus};

        let seed = Seed::new().await;
        let session = open(options(seed.dir.path())).await.unwrap();
        let helper = session.core.duckdb_helper().expect("the remote engine");
        assert_eq!(helper.dir, seed.dir.path().join("bin").join("duckdb"));
        assert_eq!(
            session.core.duckdb_helper_status().unwrap(),
            DuckdbHelperStatus::Missing
        );
        let form: ConnectionForm = serde_json::from_value(
            serde_json::json!({"name": "d", "type": "duckdb", "databaseName": ":memory:"}),
        )
        .unwrap();
        let e = session
            .ws
            .connect(&session.core, ConnectRequest::form(form))
            .await
            .unwrap_err();
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e:?}");
        // Built, never awaited: no request leaves the test.
        drop(session.core.duckdb_helper_asset());
        session.close().await;
    }

    async fn refused(dir: &Path) -> CoreError {
        match open(options(dir)).await {
            Ok(_) => panic!("opened"),
            Err(e) => e,
        }
    }

    use crate::state::secrets::Secret;

    fn save_call(kinds: &[SecretKind], typed: &[(SecretKind, &str)]) -> SaveCall {
        let mut secrets = crate::state::secrets::Typed::default();
        for (kind, value) in typed {
            secrets.set(*kind, Secret::new(*value));
        }
        SaveCall {
            connection_id: "conn-1".into(),
            kinds: kinds.to_vec(),
            secrets,
        }
    }

    #[test]
    fn a_save_only_ever_sets_and_skips_what_wasnt_typed() {
        let (patch, secrets) = save_changes(&save_call(
            &[SecretKind::Db, SecretKind::Ssh],
            &[(SecretKind::Db, "pw")],
        ))
        .unwrap();
        assert_eq!(patch.save_password, Some(true));
        assert_eq!(patch.save_ssh_password, None, "no SSH secret typed");
        assert_eq!(secrets.db, Some(Some("pw".to_string())));
        assert_eq!(secrets.ssh, None, "never a delete");
        assert_eq!(secrets.ssh_key, None);
        let (patch, secrets) = save_changes(&save_call(
            &[SecretKind::SshKey],
            &[(SecretKind::SshKey, "phrase")],
        ))
        .unwrap();
        assert_eq!(patch.save_ssh_key_passphrase, Some(true));
        assert_eq!(secrets.ssh_key, Some(Some("phrase".to_string())));
    }

    #[test]
    fn an_empty_password_or_nothing_to_save_is_refused() {
        let e = save_changes(&save_call(&[SecretKind::Db], &[(SecretKind::Db, "")])).unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT");
        let e = save_changes(&save_call(&[SecretKind::Db], &[])).unwrap_err();
        assert_eq!(e.code, "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn a_missing_file_is_refused_and_nothing_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let e = refused(dir.path()).await;
        assert_eq!(e.code, "STORAGE_NOT_FOUND");
        let text = startup_message(&e);
        assert!(text.contains("Open the Seaquel app once"), "{text}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_file_needing_an_upgrade_names_the_tui_s_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(storage_file(dir.path()), b"").unwrap();
        let e = refused(dir.path()).await;
        assert_eq!(e.code, "STORAGE_NEEDS_UPGRADE");
        let text = startup_message(&e);
        assert!(
            text.contains(&format!(
                "needs Seaquel {} or later",
                seaquel_terminal::VERSION
            )),
            "{text}"
        );
        assert!(text.contains("Open the app"), "{text}");
        assert_eq!(std::fs::read(storage_file(dir.path())).unwrap(), b"");
    }

    #[tokio::test]
    async fn legacy_and_corrupt_files_are_refused_with_core_s_words() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("projects.json"), b"[]").unwrap();
        let e = refused(dir.path()).await;
        assert_eq!(e.code, "LEGACY_STORAGE");
        assert!(startup_message(&e).contains("older than 2026.4.5"));

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(storage_file(dir.path()), b"this is not a database at all").unwrap();
        let e = refused(dir.path()).await;
        assert_eq!(e.code, "STORAGE_CORRUPT");
        assert!(startup_message(&e).contains("isn't a readable Seaquel database"));
    }

    #[tokio::test]
    async fn a_current_file_opens_as_a_second_process_that_polls() {
        let seed = Seed::new().await;
        let session = open(options(seed.path())).await.unwrap();
        assert!(session.ws.polls_external_changes());
        // With the AI client, a model call gets past `NOT_SUPPORTED` to the
        // provider lookup (no provider is configured; nothing is sent).
        let e = session
            .ws
            .ai_models(&session.core, "no-such-provider", None)
            .await
            .unwrap_err();
        assert_ne!(e.code, "NOT_SUPPORTED", "{}", e.message);
        assert_eq!(session.origin.as_deref(), Some("tui-test0001"));
        session.close().await;
    }

    #[test]
    fn origins_are_tui_and_eight_hex_digits_unless_the_hook_says() {
        let a = new_origin(None);
        let b = new_origin(None);
        for o in [&a, &b] {
            let text = o.as_deref().unwrap();
            assert_eq!(text.len(), 12, "{text}");
            assert!(text.starts_with("tui-"));
            assert!(text[4..].chars().all(|c| c.is_ascii_hexdigit()), "{text}");
        }
        assert_ne!(a, b, "random per process");
        assert_eq!(
            new_origin(Some("tui-fixed01")).as_deref(),
            Some("tui-fixed01")
        );
    }

    #[tokio::test]
    async fn library_saved_and_history_come_back_as_the_model_s_types() {
        let seed = Seed::new().await;
        let (project, conn) = seed
            .with(|core, ws| async move {
                let project = crate::testing::core::project(&core, &ws, "Analytics").await;
                let draft: seaquel_core::domain::library::LabelDraft = serde_json::from_value(
                    serde_json::json!({"name": "Billing", "color": "#123456"}),
                )
                .unwrap();
                ws.create_label(
                    &core,
                    &seaquel_core::WriteOrigin::new(Some("app-window")),
                    &project,
                    draft,
                )
                .await
                .unwrap();
                let conn = crate::testing::core::connection(
                    &core,
                    &ws,
                    serde_json::json!({
                        "projectId": project, "name": "prod", "type": "postgres",
                        "host": "db.internal", "port": 6543, "databaseName": "app",
                        "username": "me", "savePassword": false,
                        "sshTunnel": {"enabled": true, "host": "bastion", "port": 22,
                                      "username": "me", "authMethod": "key", "keyPath": "/k"},
                        "labelIds": ["prod"]
                    }),
                )
                .await;
                crate::testing::core::saved_query(
                    &core,
                    &ws,
                    &project,
                    "b",
                    "SELECT 2",
                    Some("reports"),
                )
                .await;
                crate::testing::core::saved_query(&core, &ws, &project, "a", "SELECT 1", None)
                    .await;
                (project, conn)
            })
            .await;
        let session = open(options(seed.path())).await.unwrap();
        let lib = session.library().await.unwrap();
        assert_eq!(lib.projects.len(), 1);
        let c = lib.connection(&conn).unwrap();
        assert_eq!(
            (
                c.engine.as_str(),
                c.port,
                c.save_password,
                c.label_ids.as_slice()
            ),
            (
                "postgres",
                Some(6543),
                false,
                ["prod".to_string()].as_slice()
            )
        );
        let t = c.tunnel.as_ref().unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.auth),
            ("bastion", 22, crate::state::panels::TunnelAuth::Key)
        );
        // The project's own labels, for the history snapshot.
        assert_eq!(lib.labels.len(), 1);
        let label = &lib.labels[0];
        assert_eq!(
            (
                label.project_id.as_str(),
                label.name.as_str(),
                label.color.as_str()
            ),
            (project.as_str(), "Billing", "#123456")
        );
        assert!(label.id.starts_with("label-"), "{}", label.id);
        let saved = session.saved(&project).await.unwrap();
        let names: Vec<_> = saved
            .iter()
            .map(|s| (s.name.as_str(), s.folder.as_deref()))
            .collect();
        assert!(names.contains(&("b", Some("reports"))) && names.contains(&("a", None)));
        assert!(session.history(&conn).await.unwrap().is_empty());
        let e = session.saved("project-nope").await.unwrap();
        assert!(e.is_empty());
        session.close().await;
    }
}
