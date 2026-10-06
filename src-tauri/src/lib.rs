use arboard::Clipboard;
use futures::stream::{BoxStream, StreamExt};
use image::ImageReader;
use log::{debug, error, info};
use seaquel_core::git::Git;
use seaquel_core::license::desktop::DesktopClient;
use seaquel_core::secrets::{KeychainStore, SecretStore};
use seaquel_core::storage::{LEGACY_STORAGE, STORAGE_CORRUPT};
use seaquel_core::{ConnectPolicy, Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    ConnectTargetParams, CoreEvent, DbRequest, Request, Response, RpcError, StreamKind, WriteOrigin,
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tauri::ipc::{Channel, InvokeBody};
use tauri::menu::{AboutMetadata, IsMenuItem, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, State};
use tauri_plugin_log::{Target, TargetKind, TimezoneStrategy};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::OnceCell;

mod cli_download;
mod cli_info;
mod cli_install;
mod duckdb_helper;
mod helper_pin;
mod logging;
mod update_channel;

use update_channel::{PendingDownload, UpdateChannel};

/// How often the desktop's workspace polls for commits another process made
/// to `seaquel.db` (about 0.6 s p50, 0.9 s max, 13 µs a poll).
const EXTERNAL_CHANGES_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a reloaded or closed webview's connections may take to close:
/// a backstop past which the closes are dropped, which
/// drops their drivers. The remote DuckDB driver's own close lets a
/// checkpointing helper go after about 2 s, so it normally never applies.
const CLOSE_WINDOW_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, serde::Serialize)]
struct UpdateInfo {
    version: String,
    date: Option<String>,
    size: Option<u64>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CommandError {
    message: String,
    code: String,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// The desktop's Core: every engine, file and tunnel (the desktop connects
/// wherever its user asks, so no config check), and the user's own files
/// (phase 5e): shared repos, and TablePlus's and DBeaver's
/// files under `import_paths` (the user's home; `None` when there is none,
/// and then the imports find nothing at the default locations).
///
/// The assistant (phase 6) calls models with the native client, anywhere
/// the user's provider is (`AiEgress::Any`: a local Ollama included), with
/// the OS proxy settings (`ai-system-proxy`).
///
/// **DuckDB runs in the helper**:
/// each connection is a `seaquel-duckdb` process of this app version,
/// found under `<data_local_dir>/<identifier>/bin/duckdb` (the folder the
/// terminal binaries and the CLI's install use; `SEAQUEL_DATA_DIR/bin/duckdb`
/// when that is set). The app links no DuckDB. A connect with no helper
/// installed is `ENGINE_NOT_INSTALLED` at once, and the GUI downloads it
/// between two connects. With no data-local dir DuckDB isn't registered
/// (`ENGINE_NOT_AVAILABLE`) and a WARN says so, logged by `setup`.
///
/// It runs before `tauri-plugin-log` attaches its logger (in the plugin's
/// setup), so it logs nothing itself: it answers the WARN for [`run`]'s
/// `setup` to log.
fn desktop_core(
    identifier: &str,
    import_paths: Option<seaquel_core::ImportPaths>,
) -> (Core, Option<&'static str>) {
    let dir = cli_download::duckdb_helper_dir(identifier);
    let warning = helper_folder_warning(&dir);
    (desktop_core_with(dir.ok(), import_paths), warning)
}

/// The WARN when there is no folder for the helper (no data-local dir):
/// DuckDB isn't registered, and a connect is `ENGINE_NOT_AVAILABLE`.
const NO_HELPER_FOLDER: &str = "No folder for the DuckDB helper; DuckDB is unavailable";

/// What `setup` logs about the helper's folder: [`NO_HELPER_FOLDER`] when
/// there is none (the error names no path; it isn't logged).
fn helper_folder_warning(dir: &Result<PathBuf, String>) -> Option<&'static str> {
    dir.is_err().then_some(NO_HELPER_FOLDER)
}

/// [`desktop_core`] with the DuckDB helper's `bin/duckdb` folder given
/// (tests pass one under their temp dir); `None` leaves DuckDB out. The
/// helper asset's pin is the one this build compiled in, if any.
fn desktop_core_with(
    helper_dir: Option<PathBuf>,
    import_paths: Option<seaquel_core::ImportPaths>,
) -> Core {
    let pin = helper_pin::compiled_pin(option_env!("SEAQUEL_DUCKDB_HELPER_PIN"));
    desktop_core_pinned(helper_dir, pin, import_paths)
}

/// [`desktop_core_with`] with the helper asset's pin given (tests pass
/// fake values through the build script's parser).
fn desktop_core_pinned(
    helper_dir: Option<PathBuf>,
    pin: Option<helper_pin::HelperPin>,
    import_paths: Option<seaquel_core::ImportPaths>,
) -> Core {
    use seaquel_core::ai::native::{NativeHttp, NativeHttpOptions};
    use seaquel_core::ai::AiEgress;

    let egress = AiEgress::Any;
    // Every engine but DuckDB natively, so a build where Cargo unified a
    // native DuckDB driver in still registers `duckdb` once, remotely.
    let mut builder = seaquel_core::with_plugins(|id| id != "duckdb");
    if let Some(dir) = helper_dir {
        builder = builder.duckdb_helper(seaquel_core::DuckdbHelper {
            dir,
            version: cli_download::VERSION.to_string(),
        });
    }
    // The release build's size and digest of the helper's `.gz`:
    // the install then skips the release metadata and accepts only that
    // file, and "Install from a file…" is checked against it.
    if let Some(pin) = pin {
        builder = builder.duckdb_helper_pinned(pin.size, &pin.sha256);
    }
    let builder = builder
        .connect_policy(ConnectPolicy::Unrestricted)
        .executor(std::sync::Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(seaquel_core::LocalFiles::Allowed)
        .ai_http(Arc::new(NativeHttp::new(NativeHttpOptions::new(
            egress.into(),
        ))))
        .ai_egress(egress);
    match import_paths {
        Some(paths) => builder.import_paths(paths),
        None => builder,
    }
    .build()
}

/// The license server: `LICENSE_API_URL` at compile time, else the dev
/// server in debug builds and seaquel.app in release builds.
fn license_base_url() -> &'static str {
    option_env!("LICENSE_API_URL").unwrap_or(if cfg!(debug_assertions) {
        "http://localhost:5173"
    } else {
        "https://seaquel.app"
    })
}

/// The update downloaded in the background, waiting for `install_update`.
struct PendingUpdate {
    download: Mutex<Option<PendingDownload>>,
}

/// The desktop's workspace. Storage opens on the first call that needs it:
/// a `core_call`, or the startup update check reading `updateChannel`
/// ([`update_channel()`]). Either way:
///
/// - a failure that retrying can't fix (`LEGACY_STORAGE`, `STORAGE_CORRUPT`,
///   or `NO_DATA_DIR`) is kept, and every storage call answers with it so the
///   UI can show its blocking error screen;
/// - any other failure is returned, and the next storage call tries again.
///
/// Secret calls use the keychain alone, so they work whatever storage does.
///
/// `db` calls (connect, queries, streams) go to one workspace for the life
/// of the app, so every connection has the same owner ([`DesktopWorkspace::db`]):
/// the storage workspace when it opens, or a storage-less stand-in when
/// storage failed for good, where form connects still work and saved ones
/// answer with the storage error.
pub(crate) struct DesktopWorkspace {
    /// `seaquel_storage::data_dir(identifier)`, or why there is none.
    data_dir: Result<PathBuf, RpcError>,
    secrets: Arc<dyn SecretStore>,
    /// The license server's activation client ([`license_base_url`]).
    license: DesktopClient,
    /// Shared projects' git, with the user's home for the default SSH keys.
    git: Git,
    /// Set once storage opens, or once it fails for good.
    workspace: OnceCell<Result<Arc<Workspace>, RpcError>>,
    /// The workspace `db` calls use, fixed by the first one that needs it.
    db: OnceCell<DbWorkspace>,
    /// Each webview's `core_events` sink and running `core_stream`s.
    webviews: Arc<Mutex<Webviews>>,
}

/// Takes one event; `false` means the receiver is gone.
type EventSink = Box<dyn Fn(CoreEvent) -> bool + Send>;

/// What each webview (by label: `main`, the theme editor, …) has open.
#[derive(Default)]
struct Webviews {
    /// Where [`CoreEvent::ConnectionClosed`] and [`CoreEvent::StorageChanged`]
    /// events go: every live sink gets every event.
    sinks: HashMap<String, EventSink>,
    /// The stream ids of each webview's running `core_stream`s.
    streams: HashMap<String, HashSet<String>>,
}

impl Webviews {
    fn lock(webviews: &Mutex<Webviews>) -> MutexGuard<'_, Webviews> {
        webviews.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Send `event` to every sink, dropping those whose receiver is gone.
    fn send(&mut self, event: &CoreEvent) {
        self.sinks.retain(|_, send| send(event.clone()));
    }

    /// Take `label`'s running stream ids, to cancel them.
    fn take_streams(&mut self, label: &str) -> HashSet<String> {
        self.streams.remove(label).unwrap_or_default()
    }
}

/// Removes a `core_stream`'s id from [`Webviews::streams`] when it ends,
/// however it ends.
struct StreamTracking {
    webviews: Arc<Mutex<Webviews>>,
    label: String,
    stream_id: String,
}

impl Drop for StreamTracking {
    fn drop(&mut self) {
        let mut webviews = Webviews::lock(&self.webviews);
        if let Some(ids) = webviews.streams.get_mut(&self.label) {
            ids.remove(&self.stream_id);
            if ids.is_empty() {
                webviews.streams.remove(&self.label);
            }
        }
    }
}

/// See [`DesktopWorkspace::db`].
pub(crate) struct DbWorkspace {
    pub(crate) ws: Arc<Workspace>,
    /// Why storage can't open, when `ws` is the stand-in. Saved connects and
    /// tests answer with it, since the stand-in has no saved rows.
    storage_error: Option<RpcError>,
    /// The stand-in's own (empty) storage, in the OS's temp dir. Tauri never
    /// drops managed state, so it outlives the app and is left to the OS's
    /// temp cleanup; it's only made when storage failed for good.
    _scratch: Option<tempfile::TempDir>,
}

impl DesktopWorkspace {
    fn new(data_dir: Result<PathBuf, RpcError>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            data_dir,
            secrets,
            license: DesktopClient::new(license_base_url()),
            git: Git::from_env(),
            workspace: OnceCell::new(),
            db: OnceCell::new(),
            webviews: Arc::default(),
        }
    }

    /// Whether storage can never open in this run: no data dir, or a kept
    /// `LEGACY_STORAGE`/`STORAGE_CORRUPT`.
    fn storage_failed_for_good(&self) -> bool {
        self.data_dir.is_err() || matches!(self.workspace.get(), Some(Err(_)))
    }

    /// The workspace `db` calls use. The first call picks it, and it stays:
    ///
    /// - storage opens: the storage workspace, so saved connects read their
    ///   rows from it;
    /// - storage failed for good: a stand-in workspace on an empty scratch
    ///   storage and no secret store. Form connects work on it; saved
    ///   connects and tests answer with the storage error (see
    ///   [`DesktopWorkspace::call`]). Storage can't open later in this run,
    ///   so no connection is ever split across two owners;
    /// - a failure worth retrying: that error, and the next `db` call tries
    ///   again, like a storage call.
    ///
    /// The storage workspace's events already reach `core_events` (see
    /// [`DesktopWorkspace::workspace`]); picking the stand-in subscribes to
    /// its events once.
    pub(crate) async fn db(&self, core: &Core) -> Result<&DbWorkspace, RpcError> {
        self.db
            .get_or_try_init(|| async {
                match self.workspace(core).await {
                    Ok(ws) => Ok(DbWorkspace {
                        ws,
                        storage_error: None,
                        _scratch: None,
                    }),
                    Err(e) if self.storage_failed_for_good() => {
                        let db = Self::stand_in(core, e).await?;
                        self.pump(&db.ws);
                        Ok(db)
                    }
                    Err(e) => Err(e),
                }
            })
            .await
    }

    /// Send `ws`'s events to every webview's `core_events` sink from now on.
    /// Called once per workspace, when it opens, so a storage write made
    /// before any `db` call is announced too.
    fn pump(&self, ws: &Workspace) {
        tauri::async_runtime::spawn(pump_events(
            seaquel_rpc::workspace_events(ws),
            self.webviews.clone(),
        ));
    }

    async fn stand_in(core: &Core, storage_error: RpcError) -> Result<DbWorkspace, RpcError> {
        let scratch = tempfile::Builder::new()
            .prefix("seaquel-no-storage-")
            .tempdir()
            .map_err(|e| {
                RpcError::new(
                    "STORAGE_ERROR",
                    format!("Couldn't create a scratch dir for connections: {e}"),
                )
            })?;
        let ws = core
            .open_workspace(WorkspaceSpec::new(scratch.path()))
            .await?;
        log::warn!(activity = "workspace.open", code = storage_error.code.as_str(); "Storage can't open; connections use a stand-in workspace, and saved connections fail");
        Ok(DbWorkspace {
            ws,
            storage_error: Some(storage_error),
            _scratch: Some(scratch),
        })
    }

    /// Send [`CoreEvent::ConnectionClosed`] and [`CoreEvent::StorageChanged`]
    /// events to `sink` for webview `label` from now on. Every webview's
    /// sink gets every event, the writer's too (it skips its own by
    /// `origin`, its label).
    ///
    /// A second call for the same label is a reload: its sink replaces the
    /// old one (whose channel may never report that it's gone), the label's
    /// running streams, which the reloaded page can no longer read, are
    /// cancelled, and the connections it opened, which the reloaded page
    /// can't name (it starts with every connection disconnected), are
    /// closed and announced as `WINDOW_CLOSED` to every sink, the new one
    /// included (a DuckDB connection holds its file, so a reconnect would meet it).
    /// Returns
    /// how many it closed.
    async fn set_event_sink(&self, core: &Core, label: &str, sink: EventSink) -> usize {
        let (reload, stale) = {
            let mut webviews = Webviews::lock(&self.webviews);
            match webviews.sinks.insert(label.to_string(), sink) {
                Some(_) => (true, webviews.take_streams(label)),
                None => (false, HashSet::new()),
            }
        };
        self.cancel_streams(core, label, stale);
        if !reload {
            return 0;
        }
        self.close_window(core, label).await
    }

    /// Webview `label` is gone: drop its sink, cancel its streams and close
    /// the connections it opened. Returns how many it closed.
    async fn forget_webview(&self, core: &Core, label: &str) -> usize {
        let stale = {
            let mut webviews = Webviews::lock(&self.webviews);
            webviews.sinks.remove(label);
            webviews.take_streams(label)
        };
        self.cancel_streams(core, label, stale);
        self.close_window(core, label).await
    }

    /// Close the connections webview `label` opened (`close_owned_by`, by
    /// the write origin each connect carried); other webviews' stay.
    async fn close_window(&self, core: &Core, label: &str) -> usize {
        // No `db` workspace yet means no connection was ever opened.
        let Some(db) = self.db.get() else { return 0 };
        // The connections are out of Core and announced before any driver's
        // close is awaited, so stopping the wait here still leaves them
        // closed (dropping the closes drops the drivers).
        match tokio::time::timeout(CLOSE_WINDOW_WAIT, db.ws.close_owned_by(core, label)).await {
            Ok(closed) => {
                if closed > 0 {
                    info!(activity = "db.close_window", webview = label, connections = closed; "Closed a reloaded or closed page's connections");
                }
                closed
            }
            Err(_) => {
                log::warn!(activity = "db.close_window", webview = label, code = "TIMEOUT"; "A reloaded or closed page's connections didn't close in time; their closes were dropped");
                0
            }
        }
    }

    fn cancel_streams(&self, core: &Core, label: &str, stream_ids: HashSet<String>) {
        // No `db` workspace yet means no stream ever started.
        let Some(db) = self.db.get() else { return };
        if !stream_ids.is_empty() {
            info!(activity = "db.cancel_stream", webview = label, streams = stream_ids.len(); "Cancelling a gone page's streams");
        }
        for id in stream_ids {
            db.ws.cancel(core, &id);
        }
    }

    /// The open workspace, opening it if this is the first call to need it
    /// or the last attempt failed in a way worth retrying.
    async fn workspace(&self, core: &Core) -> Result<Arc<Workspace>, RpcError> {
        let data_dir = self.data_dir.as_ref().map_err(Clone::clone)?;
        let opened = self
            .workspace
            .get_or_try_init(|| async {
                // Hear what another process (the TUI)
                // commits to the file. The stand-in workspace doesn't poll:
                // nothing else writes its scratch file.
                let spec = WorkspaceSpec::new(data_dir)
                    .with_secrets(self.secrets.clone())
                    .with_external_changes(EXTERNAL_CHANGES_POLL);
                match core.open_workspace(spec).await {
                    Ok(ws) => {
                        // No path: the data dir names the user's home
                        // (Task 10's O1).
                        info!(activity = "workspace.open"; "Workspace open");
                        self.pump(&ws);
                        Ok(Ok(ws))
                    }
                    Err(e) if e.code == LEGACY_STORAGE || e.code == STORAGE_CORRUPT => {
                        error!(activity = "workspace.open", code = e.code.as_str(); "Workspace can't be opened: {}", e.message);
                        Ok(Err(RpcError::from(e)))
                    }
                    Err(e) => {
                        error!(activity = "workspace.open", code = e.code.as_str(); "Workspace failed to open, will retry: {}", e.message);
                        Err(RpcError::from(e))
                    }
                }
            })
            .await?;
        opened.clone()
    }

    /// Serve `req` from webview `origin` (its label, which the
    /// `StorageChanged` event of a write carries).
    async fn call(
        &self,
        core: &Core,
        origin: WriteOrigin,
        req: Request,
    ) -> Result<Response, RpcError> {
        match req {
            Request::Secret(r) => seaquel_rpc::dispatch_secret(Some(&*self.secrets), r)
                .await
                .map(Response::Secret),
            Request::License(r) => seaquel_rpc::dispatch_license(&self.license, r)
                .await
                .map(Response::License),
            // Pull, push, commit and conflict resolution run under the repo
            // lock through the storage workspace, which also
            // records a pull's or push's `lastSyncAt` (best effort). Getting
            // the workspace may open storage, or retry a failure worth
            // retrying, like any storage call; any error is dropped here, so
            // storage never blocks git: without it these calls still take
            // the lock and only skip `lastSyncAt`. The other git calls need
            // no storage.
            Request::Git(r) => {
                let ws = if r.takes_repo_lock() {
                    self.workspace(core).await.ok()
                } else {
                    None
                };
                seaquel_rpc::dispatch_git(core, ws.as_deref(), &self.git, r, &origin)
                    .await
                    .map(Response::Git)
            }
            // Core owns the tunnels; they need no storage.
            Request::Ssh(r) => seaquel_rpc::dispatch_ssh(core, r).await.map(Response::Ssh),
            Request::Db(r) => {
                let db = self.db(core).await?;
                if let Some(e) = &db.storage_error {
                    if reads_saved_row(&r) {
                        return Err(e.clone());
                    }
                }
                seaquel_rpc::dispatch_workspace(core, &db.ws, Request::Db(r), origin).await
            }
            // Storage, the library, settings, ui, and (phase 5e) the
            // `shared` and `imports` groups, which `dispatch_workspace`
            // serves only because this Core has `LocalFiles`.
            req => {
                let ws = self.workspace(core).await?;
                seaquel_rpc::dispatch_workspace(core, &ws, req, origin).await
            }
        }
    }
}

/// A saved connect or test, which reads its row from storage.
fn reads_saved_row(req: &DbRequest) -> bool {
    matches!(
        req,
        DbRequest::Connect(p) | DbRequest::Test(p)
            if matches!(p.target, ConnectTargetParams::Saved { .. })
    )
}

/// Hand each workspace event to every webview's sink. With none set, it's
/// dropped (the desktop only closes connections when asked).
///
/// Unlike the web's per-socket queue (`LISTENER_EVENT_BOUND` in
/// `seaquel-server`), nothing here is bounded: Core's subscriber channel
/// and each Tauri `Channel` are unbounded. That's acceptable because it's
/// all in one process: a sink never waits on a network peer, and a webview
/// that stops reading is one the app itself has hung.
async fn pump_events(mut events: BoxStream<'static, CoreEvent>, webviews: Arc<Mutex<Webviews>>) {
    while let Some(event) = events.next().await {
        Webviews::lock(&webviews).send(&event);
    }
}

/// The request's JSON bytes. The frontend sends them as a `Uint8Array`
/// (`invoke("core_call", bytes)`), which reaches Rust untouched as
/// `InvokeBody::Raw` over the `ipc://` protocol. When the webview blocks that
/// protocol, Tauri falls back to `postMessage`, where the same `Uint8Array`
/// arrives as a JSON array of numbers; that is accepted too. A plain object
/// is refused: Tauri would already have parsed it into a
/// `serde_json::Value`, which sorts the keys inside stored JSON.
fn core_call_body(body: &InvokeBody) -> Result<Cow<'_, [u8]>, RpcError> {
    match body {
        InvokeBody::Raw(bytes) => Ok(Cow::Borrowed(bytes)),
        InvokeBody::Json(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
            .collect::<Option<Vec<u8>>>()
            .map(Cow::Owned)
            .ok_or_else(|| {
                RpcError::invalid_argument("core_call: the body array must hold bytes (0-255)")
            }),
        InvokeBody::Json(_) => Err(RpcError::invalid_argument(
            "core_call takes the request's UTF-8 JSON as bytes (a Uint8Array), not as an \
             object: parsing an object would reorder the keys inside stored JSON",
        )),
    }
}

/// The workspace RPC (`seaquel_rpc::Request`). The body is the request's
/// JSON as bytes; see [`core_call_body`].
///
/// The write origin is the calling webview's label, read here from the
/// webview itself and never from the payload (phase 5d): a
/// window's writes come back to it as events it can recognise and skip.
#[tauri::command]
async fn core_call(
    request: tauri::ipc::Request<'_>,
    webview: tauri::Webview,
    core: State<'_, Core>,
    workspace: State<'_, DesktopWorkspace>,
) -> Result<Response, RpcError> {
    handle_core_call(&core, &workspace, webview.label(), request.body()).await
}

async fn handle_core_call(
    core: &Core,
    workspace: &DesktopWorkspace,
    label: &str,
    body: &InvokeBody,
) -> Result<Response, RpcError> {
    let req = seaquel_rpc::parse_request(&core_call_body(body)?)?;
    workspace
        .call(core, WriteOrigin::new(Some(label)), req)
        .await
}

/// A `db.queryStream`, `db.run`, `db.page`, `db.tablePage` or `ai.chat`
/// request, its events pushed to `channel` as [`CoreEvent::Stream`]s (a
/// query stream's batches), [`CoreEvent::Run`]s (a run's, page's or table
/// page's statements and batches) or [`CoreEvent::Ai`]s (a turn's), then one
/// `done` or `error`, or nothing more after a `db.cancel` with its
/// `streamId` (which may arrive before it starts).
/// `request` is the request's JSON as a string: the invoke carries the
/// channel too, so the body can't be raw bytes. Resolves when the stream
/// ends or the webview drops the channel, with the number of events it sent:
/// the reply can overtake channel messages, so the GUI waits until it has
/// that many before it treats a stream with no `done`/`error` as cancelled.
/// Rejects (`RpcError`) only for a request that isn't a stream or when the
/// `db` workspace can't be had.
///
/// The stream belongs to the calling webview: reloading it (a new
/// `core_events` from the same label) or closing it cancels the stream. A
/// turn is cancelled through Core and polled to its end, so it stores its
/// reply (phase 6); so is one whose channel is gone.
#[tauri::command]
async fn core_stream(
    request: String,
    channel: Channel<CoreEvent>,
    webview: tauri::Webview,
    core: State<'_, Core>,
    workspace: State<'_, DesktopWorkspace>,
) -> Result<u64, RpcError> {
    run_core_stream(
        &core,
        &workspace,
        webview.label(),
        request.as_bytes(),
        |event| channel.send(event).is_ok(),
    )
    .await
}

/// [`core_stream`]'s body: `send` gets each event and returns `false` when
/// the receiver is gone, which stops the stream (dropping it stops the
/// driver's fetch and releases its connection). Returns how many events
/// were sent.
async fn run_core_stream(
    core: &Core,
    workspace: &DesktopWorkspace,
    label: &str,
    body: &[u8],
    mut send: impl FnMut(CoreEvent) -> bool,
) -> Result<u64, RpcError> {
    let req = seaquel_rpc::parse_request(body)?;
    // A query stream's, a run's, a page's, a table page's or a turn's id.
    let stream_id = req.stream_id().map(str::to_string);
    let turn = req.stream_kind() == Some(StreamKind::Ai);
    let db = workspace.db(core).await?;
    // The run's history event carries the webview's label, as `core_call`'s
    // writes do.
    let origin = WriteOrigin::new(Some(label));
    let mut events = seaquel_rpc::dispatch_stream(core, &db.ws, req, origin)?;
    // Tracked once registered, so a reload's cancel always finds it.
    let _tracking = stream_id.clone().map(|stream_id| {
        Webviews::lock(&workspace.webviews)
            .streams
            .entry(label.to_string())
            .or_default()
            .insert(stream_id.clone());
        StreamTracking {
            webviews: workspace.webviews.clone(),
            label: label.to_string(),
            stream_id,
        }
    });
    let mut sent = 0;
    while let Some(event) = events.next().await {
        if !send(event) {
            if turn {
                // The webview went away under a turn: stop it through Core and
                // let it store its reply (phase 6, Task 4's contract).
                if let Some(id) = &stream_id {
                    stop_turn(core, &db.ws, id, events).await;
                }
            }
            break;
        }
        sent += 1;
    }
    Ok(sent)
}

/// Stops a turn whose receiver is gone: `cancel` through Core, then polls
/// the turn to its end so the reply is stored with what streamed (dropping
/// it would drop the reply's write). The events are dropped. Bounded by
/// [`seaquel_rpc::TURN_STOP_WAIT`]: a turn still running then is dropped.
async fn stop_turn(
    core: &Core,
    ws: &Workspace,
    stream_id: &str,
    mut events: BoxStream<'_, CoreEvent>,
) {
    ws.cancel(core, stream_id);
    let drained = tokio::time::timeout(seaquel_rpc::TURN_STOP_WAIT, async {
        while events.next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        log::warn!(activity = "ai.stop"; "A stopped turn didn't end in time; dropping it");
    }
}

/// Push the workspace's [`CoreEvent::ConnectionClosed`] and
/// [`CoreEvent::StorageChanged`] events to `channel`. Each webview calls it
/// once when its page loads; every webview gets every event (events from
/// before it registered are gone, so a page reloads what it shows). A second call from the same webview is a reload: the new channel
/// replaces the old one and the webview's running streams are cancelled
/// ([`DesktopWorkspace::set_event_sink`]); it also closes the connections
/// the old page opened (side by side, the wait bounded by
/// [`CLOSE_WINDOW_WAIT`]) and resolves once they are closed. Those
/// connections are out of Core before it resolves; a reconnect to a DuckDB
/// file one of them held is protected by the engine's wait for a closing
/// helper, not by this close.
/// Closing the window drops the sink and closes its connections too.
#[tauri::command]
async fn core_events(
    channel: Channel<CoreEvent>,
    webview: tauri::Webview,
    core: State<'_, Core>,
    workspace: State<'_, DesktopWorkspace>,
) -> Result<(), RpcError> {
    workspace
        .set_event_sink(
            &core,
            webview.label(),
            Box::new(move |event| channel.send(event).is_ok()),
        )
        .await;
    Ok(())
}

#[tauri::command]
fn copy_image_to_clipboard(path: String) -> Result<(), CommandError> {
    debug!(activity = "app.clipboard"; "Copying image to clipboard");
    let img = ImageReader::open(&path)
        .map_err(|e| CommandError {
            message: format!("Failed to open image: {}", e),
            code: "IMAGE_ERROR".to_string(),
        })?
        .decode()
        .map_err(|e| CommandError {
            message: format!("Failed to decode image: {}", e),
            code: "IMAGE_ERROR".to_string(),
        })?;

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    let img_data = arboard::ImageData {
        width: width as usize,
        height: height as usize,
        bytes: rgba.into_raw().into(),
    };

    let mut clipboard = Clipboard::new().map_err(|e| CommandError {
        message: format!("Failed to access clipboard: {}", e),
        code: "CLIPBOARD_ERROR".to_string(),
    })?;

    clipboard.set_image(img_data).map_err(|e| CommandError {
        message: format!("Failed to copy image: {}", e),
        code: "CLIPBOARD_ERROR".to_string(),
    })?;

    Ok(())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), CommandError> {
    debug!(activity = "app.open"; "Opening external path");
    opener::open(&path).map_err(|e| CommandError {
        message: format!("Failed to open path: {}", e),
        code: "OPEN_ERROR".to_string(),
    })
}

#[tauri::command]
fn get_username() -> String {
    whoami::username()
}

#[tauri::command]
fn read_log_file(app: tauri::AppHandle) -> Result<String, CommandError> {
    use std::io::{Read, Seek, SeekFrom};

    let log_dir = app.path().app_log_dir().map_err(|e| CommandError {
        message: format!("Failed to get log dir: {}", e),
        code: "DIR_ERROR".to_string(),
    })?;
    let log_path = log_dir.join("seaquel.log");

    let mut file = std::fs::File::open(&log_path).map_err(|e| CommandError {
        message: format!("Failed to read log file: {}", e),
        code: "READ_ERROR".to_string(),
    })?;

    let metadata = file.metadata().map_err(|e| CommandError {
        message: format!("Failed to read log file metadata: {}", e),
        code: "READ_ERROR".to_string(),
    })?;

    // Read only the last 512KB to avoid memory/rendering issues with large logs
    const MAX_BYTES: u64 = 512 * 1024;
    let file_size = metadata.len();
    let truncated = file_size > MAX_BYTES;

    if truncated {
        file.seek(SeekFrom::End(-(MAX_BYTES as i64)))
            .map_err(|e| CommandError {
                message: format!("Failed to seek log file: {}", e),
                code: "READ_ERROR".to_string(),
            })?;
    }

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|e| CommandError {
            message: format!("Failed to read log file: {}", e),
            code: "READ_ERROR".to_string(),
        })?;

    if truncated {
        // Skip to the first complete line after the seek point
        if let Some(newline_pos) = content.find('\n') {
            content = format!(
                "… (showing last 512KB of log)\n{}",
                &content[newline_pos + 1..]
            );
        }
    }

    Ok(content)
}

#[tauri::command]
fn clear_log_file(app: tauri::AppHandle) -> Result<(), CommandError> {
    let log_dir = app.path().app_log_dir().map_err(|e| CommandError {
        message: format!("Failed to get log dir: {}", e),
        code: "DIR_ERROR".to_string(),
    })?;
    let log_path = log_dir.join("seaquel.log");

    std::fs::write(&log_path, "").map_err(|e| CommandError {
        message: format!("Failed to clear log file: {}", e),
        code: "WRITE_ERROR".to_string(),
    })?;

    Ok(())
}

/// The data dir the UI shows: `SEAQUEL_DATA_DIR`, or the platform's data dir
/// plus this build's identifier (`seaquel_storage::data_dir`). Created if
/// it's missing.
#[tauri::command]
fn get_data_dir(app: tauri::AppHandle) -> Result<String, CommandError> {
    let path =
        seaquel_core::storage::data_dir(&app.config().identifier).map_err(|e| CommandError {
            message: e.to_string(),
            code: e.code().to_string(),
        })?;
    if !path.exists() {
        std::fs::create_dir_all(&path).map_err(|e| CommandError {
            message: format!("Failed to create data dir: {}", e),
            code: "DIR_ERROR".to_string(),
        })?;
    }
    Ok(path.to_string_lossy().to_string())
}

/// `install_update`'s answer when there is no downloaded update it can
/// install on the channel the user is on now (none pending, or one from
/// another channel).
fn update_stale() -> CommandError {
    log::warn!(activity = "app.update", error_code = "UPDATE_STALE"; "Downloaded update no longer matches the update channel");
    CommandError {
        message:
            "The downloaded update no longer matches the update channel. Check for updates again."
                .to_string(),
        code: "UPDATE_STALE".to_string(),
    }
}

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<(), CommandError> {
    info!(activity = "app.update"; "Installing update");
    let download = pending
        .download
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    // Nothing pending: another window's check on a new channel dropped the
    // download this window still shows, or it was never there.
    let Some(download) = download else {
        return Err(update_stale());
    };
    // Re-check for update to get the Update object needed for install,
    // on the channel the user is on now.
    let channel = update_channel(&app).await;
    let checked = match channel_updater(&app, channel) {
        Ok(updater) => updater.check().await.map_err(|e| {
            error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to check for update");
            CommandError {
                message: format!("Failed to check for update: {}", e),
                code: "UPDATE_ERROR".to_string(),
            }
        }),
        Err(e) => {
            error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to get updater");
            Err(CommandError {
                message: format!("Failed to get updater: {}", e),
                code: "UPDATE_ERROR".to_string(),
            })
        }
    };
    // A check that failed says nothing about the download: keep it, so the
    // next click retries without downloading again. Don't overwrite one a
    // newer background download stored meanwhile.
    let update = match checked {
        Ok(update) => update,
        Err(e) => {
            let mut d = pending
                .download
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if d.is_none() {
                *d = Some(download);
            }
            return Err(e);
        }
    };
    // The bytes are installed only as the version they were downloaded as,
    // and only on the channel that downloaded them: nothing newer on this
    // channel now (a beta download, then a switch to stable) is stale too.
    let Some(update) = update else {
        return Err(update_stale());
    };
    if !download.matches(channel, &update.version) {
        return Err(update_stale());
    }
    update.install(&download.bytes).map_err(|e| {
        error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to install update");
        CommandError {
            message: format!("Failed to install update: {}", e),
            code: "UPDATE_ERROR".to_string(),
        }
    })?;
    info!(activity = "app.update"; "Update installed, restarting");
    app.restart();
}

#[tauri::command]
async fn check_for_update_command(
    app: tauri::AppHandle,
) -> Result<Option<UpdateInfo>, CommandError> {
    debug!(activity = "app.update"; "Checking for updates");
    let channel = update_channel(&app).await;
    // Drop a download made on the other channel; `install_update` refuses
    // one anyway, and `check_for_update` drops one that finishes after a
    // switch.
    {
        let pending = app.state::<PendingUpdate>();
        let mut d = pending
            .download
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if d.as_ref().is_some_and(|p| p.channel != channel) {
            *d = None;
        }
    }
    let update = channel_updater(&app, channel)
        .map_err(|e| CommandError {
            message: format!("Failed to get updater: {}", e),
            code: "UPDATE_ERROR".to_string(),
        })?
        .check()
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to check for update: {}", e),
            code: "UPDATE_ERROR".to_string(),
        })?;

    match update {
        Some(u) => {
            info!(activity = "app.update", version = u.version.as_str(); "Update available");
            let info = UpdateInfo {
                version: u.version.clone(),
                date: u.date.map(|d| {
                    d.format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| d.to_string())
                }),
                size: None,
            };

            // Spawn background download so badge appears immediately
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = check_for_update(handle).await;
            });

            Ok(Some(info))
        }
        None => {
            debug!(activity = "app.update"; "No update available");
            Ok(None)
        }
    }
}

fn create_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // Load and decode the app icon for the About dialog
    let icon = {
        let icon_bytes = include_bytes!("../icons/128x128@2x.png");
        image::load_from_memory(icon_bytes).ok().map(|img| {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            tauri::image::Image::new_owned(rgba.into_raw(), width, height)
        })
    };

    // About metadata with custom icon
    let about_metadata = AboutMetadata {
        icon,
        ..Default::default()
    };

    // Settings menu item with Cmd+, accelerator
    let settings = MenuItemBuilder::new("Settings...")
        .id("settings")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    // "Install Command Line Tool…" downloads the CLI; macOS and Linux also
    // put it on PATH (cli_install.rs).
    let install_cli = if cli_install::available() {
        Some(
            MenuItemBuilder::new(cli_install::MENU_LABEL)
                .id(cli_install::MENU_ID)
                .build(app)?,
        )
    } else {
        None
    };

    // App menu (macOS)
    let about = PredefinedMenuItem::about(app, Some("About Seaquel"), Some(about_metadata))?;
    let separators = (0..4)
        .map(|_| PredefinedMenuItem::separator(app))
        .collect::<tauri::Result<Vec<_>>>()?;
    let services = PredefinedMenuItem::services(app, None)?;
    let hide = PredefinedMenuItem::hide(app, None)?;
    let hide_others = PredefinedMenuItem::hide_others(app, None)?;
    let show_all = PredefinedMenuItem::show_all(app, None)?;
    let quit = PredefinedMenuItem::quit(app, None)?;
    let mut app_items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&about, &separators[0], &settings];
    if let Some(item) = &install_cli {
        app_items.push(item);
    }
    app_items.extend([
        &separators[1] as &dyn IsMenuItem<tauri::Wry>,
        &services,
        &separators[2],
        &hide,
        &hide_others,
        &show_all,
        &separators[3],
        &quit,
    ]);
    let app_menu = Submenu::with_items(app, "Seaquel", true, &app_items)?;

    // File menu - Custom "Close Tab" instead of "Close Window" for Cmd+W
    let close_tab = MenuItemBuilder::new("Close Tab")
        .id("close_tab")
        .accelerator("CmdOrCtrl+W")
        .build(app)?;

    let file_menu = Submenu::with_items(app, "File", true, &[&close_tab])?;

    // Edit menu with standard items
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    // Window menu
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &window_menu])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    info!(activity = "app.startup"; "Seaquel starting");
    let logger = tauri_plugin_log::Builder::new()
        .format(logging::make_logfmt_formatter(TimezoneStrategy::UseLocal))
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some("seaquel".into()),
            }),
            Target::new(TargetKind::Webview)
                .filter(|metadata| metadata.level() <= log::Level::Info),
        ])
        .level(log::LevelFilter::Info)
        .level_for("seaquel_lib", log::LevelFilter::Trace)
        // DB activity (queries, streams, cancels) is logged by the core crate.
        .level_for("seaquel_core", log::LevelFilter::Trace)
        // sqlx's statement log holds each statement's whole SQL (at WARN when
        // it's slower than a second). The drivers turn it off; this drops it
        // too.
        .level_for("sqlx::query", log::LevelFilter::Off)
        // Postgres notices: a `RAISE WARNING`'s text, which the query
        // chooses and can fill with values, at WARN.
        .level_for("sqlx::postgres::notice", log::LevelFilter::Off)
        // tiberius logs every SQL Server error (ERROR) and PRINT (INFO) with
        // the server's text, which can quote values; the rest of it (TLS
        // warnings) passes at WARN.
        .level_for("tiberius::tds::stream::token", log::LevelFilter::Off)
        .level_for("tiberius", log::LevelFilter::Warn)
        // sqlparser (Core's table and column refs for a run) logs the
        // tokens it parses, literals included, at DEBUG.
        .level_for("sqlparser", log::LevelFilter::Off)
        // russh names the SSH bastion's host and port and prints its host
        // keys below WARN.
        .level_for("russh", log::LevelFilter::Warn)
        .level_for("russh_keys", log::LevelFilter::Warn)
        .max_file_size(5_000_000)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll);
    // Workspace calls log their method names at debug; dev builds show them.
    #[cfg(debug_assertions)]
    let logger = logger.level_for("seaquel_rpc", log::LevelFilter::Debug);
    let context = tauri::generate_context!();
    let (core, helper_warning) = desktop_core(
        &context.config().identifier,
        seaquel_core::ImportPaths::from_env(),
    );
    tauri::Builder::default()
        .plugin(logger.build())
        .plugin(tauri_plugin_os::init())
        // Every engine, file and tunnel: the desktop connects wherever its
        // user asks, so no config check.
        .manage(core)
        .manage(PendingUpdate {
            download: Mutex::new(None),
        })
        .manage(duckdb_helper::HelperInstalls::default())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_deep_link::init())
        .invoke_handler(tauri::generate_handler![
            core_call,
            core_stream,
            core_events,
            cli_info::cli_info,
            cli_info::install_cli,
            duckdb_helper::duckdb_helper_offer,
            duckdb_helper::duckdb_helper_install,
            duckdb_helper::duckdb_helper_cancel,
            duckdb_helper::duckdb_helper_install_file,
            copy_image_to_clipboard,
            open_path,
            get_data_dir,
            read_log_file,
            clear_log_file,
            get_username,
            install_update,
            check_for_update_command,
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Destroyed => {
                // Closing its connections awaits their drivers, so it runs
                // off this (main) thread.
                let app = window.app_handle().clone();
                let label = window.label().to_string();
                tauri::async_runtime::spawn(async move {
                    if let (Some(core), Some(ws)) =
                        (app.try_state::<Core>(), app.try_state::<DesktopWorkspace>())
                    {
                        ws.forget_webview(&core, &label).await;
                    }
                });
                if window.label() == "main" {
                    for (label, w) in window.app_handle().webview_windows() {
                        if label != "main" {
                            let _ = w.destroy();
                        }
                    }
                }
            }
            tauri::WindowEvent::DragDrop(drag_drop_event) => match drag_drop_event {
                tauri::DragDropEvent::Drop { paths, .. } => {
                    let supported_extensions =
                        ["parquet", "csv", "json", "duckdb", "db", "xlsx", "xls"];
                    let supported_paths: Vec<String> = paths
                        .iter()
                        .filter(|p| {
                            p.extension()
                                .and_then(|ext| ext.to_str())
                                .map(|ext| {
                                    supported_extensions.contains(&ext.to_lowercase().as_str())
                                })
                                .unwrap_or(false)
                        })
                        .filter_map(|p| p.to_str().map(String::from))
                        .collect();
                    if !supported_paths.is_empty() {
                        let _ = window.emit("file-drop", supported_paths);
                    }
                }
                tauri::DragDropEvent::Enter { paths, .. } => {
                    let paths: Vec<String> = paths
                        .iter()
                        .filter_map(|p| p.to_str().map(String::from))
                        .collect();
                    let _ = window.emit("file-drop-hover", paths);
                }
                tauri::DragDropEvent::Over { .. } => {}
                tauri::DragDropEvent::Leave => {
                    let _ = window.emit("file-drop-leave", ());
                }
                _ => {}
            },
            _ => {}
        })
        .setup(move |app| {
            // Core was built before the log plugin attached its logger
            // (plugins set up before this), so its WARN is logged here.
            if let Some(warning) = helper_warning {
                log::warn!(activity = "duckdb.helper"; "{warning}");
            }
            // Set up custom menu
            let menu = create_menu(app.handle())?;
            app.set_menu(menu)?;

            // Listen for menu clicks and emit to frontend
            app.on_menu_event(|app, event| {
                if event.id().as_ref() == "close_tab" {
                    // If a child window is focused, close it instead of closing a tab
                    let focused_child = app
                        .webview_windows()
                        .into_iter()
                        .find(|(label, w)| label != "main" && w.is_focused().unwrap_or(false));
                    if let Some((_, child)) = focused_child {
                        let _ = child.destroy();
                    } else {
                        let _ = app.emit("menu-close-tab", ());
                    }
                } else if event.id().as_ref() == "settings" {
                    let _ = app.emit("menu-settings", ());
                } else if event.id().as_ref() == cli_install::MENU_ID {
                    cli_install::install_from_menu(app);
                }
            });

            // Before any command can run, so `core_call` always finds it.
            // Storage itself opens on the first call that needs it (the
            // update check below reads `updateChannel` from it).
            let data_dir = seaquel_core::storage::data_dir(&app.config().identifier)
                .map_err(|e| RpcError::from(CoreError::from(e)));
            let keychain = KeychainStore::new(seaquel_core::secrets::DESKTOP_SERVICE);
            app.manage(DesktopWorkspace::new(data_dir, Arc::new(keychain)));

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = check_for_update(handle).await;
            });
            info!(activity = "app.startup"; "App setup complete");
            Ok(())
        })
        .run(context)
        .expect("error while running tauri application");
}

/// The channel the user chose (`updateChannel`), or this build's default
/// when it's unset or storage can't be read: an update check never fails
/// on storage.
async fn update_channel(app: &tauri::AppHandle) -> UpdateChannel {
    let core = app.state::<Core>();
    let desktop = app.state::<DesktopWorkspace>();
    let stored = match desktop.workspace(&core).await {
        Ok(ws) => match ws
            .get_setting(seaquel_core::domain::state::SettingKey::UpdateChannel.as_str())
            .await
        {
            Ok(s) => s.value,
            Err(e) => {
                log::warn!(activity = "app.update", code = e.code.as_str(); "Can't read the update channel; using this build's default");
                None
            }
        },
        // `workspace` has logged why.
        Err(e) => {
            debug!(activity = "app.update", code = e.code.as_str(); "Storage isn't open; using this build's default update channel");
            None
        }
    };
    UpdateChannel::resolve(stored.as_deref(), &update_channel::app_version())
}

/// The updater for `channel`'s feed.
fn channel_updater(
    app: &tauri::AppHandle,
    channel: UpdateChannel,
) -> tauri_plugin_updater::Result<tauri_plugin_updater::Updater> {
    // The plugin compares against `package_info().version`, which Windows
    // builds rewrite for MSI (`update_channel::app_version`): every feed
    // entry would count as newer there, older stables included.
    let current = update_channel::app_version();
    app.updater_builder()
        .endpoints(vec![channel.endpoint()])?
        .version_comparator(move |_rewritten, release| {
            update_channel::is_newer(&release.version, &current)
        })
        .build()
}

async fn check_for_update(app: tauri::AppHandle) -> tauri_plugin_updater::Result<()> {
    let channel = update_channel(&app).await;
    if let Some(update) = channel_updater(&app, channel)?.check().await? {
        let mut downloaded = 0;
        let mut total_size: Option<u64> = None;

        info!(activity = "app.update", version = update.version.as_str(); "Downloading update");
        let bytes = update
            .download(
                |chunk_length, content_length| {
                    downloaded += chunk_length;
                    if total_size.is_none() {
                        total_size = content_length;
                    }
                },
                || {
                    info!(activity = "app.update"; "Update download complete");
                },
            )
            .await?;

        let info = UpdateInfo {
            version: update.version.clone(),
            date: update.date.map(|d| {
                d.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| d.to_string())
            }),
            size: total_size,
        };

        // The user may have switched channels while it downloaded: keep it
        // only for the channel it came from.
        if update_channel(&app).await != channel {
            info!(activity = "app.update"; "Update channel changed during the download; dropping it");
            return Ok(());
        }

        info!(activity = "app.update"; "Update downloaded, notifying frontend");

        // Store the bytes for later installation
        let pending = app.state::<PendingUpdate>();
        *pending
            .download
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(PendingDownload {
            channel,
            version: update.version.clone(),
            bytes,
        });

        let _ = app.emit("update-downloaded", info);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"method":"storage","params":{"method":"licenseSave","params":{"data":{"z":1e+21,"a":1}}}}"#;

    #[test]
    fn core_call_takes_raw_bytes_as_they_are() {
        let body = InvokeBody::Raw(BODY.as_bytes().to_vec());
        assert_eq!(&*core_call_body(&body).unwrap(), BODY.as_bytes());
    }

    #[test]
    fn core_call_takes_the_post_message_byte_array() {
        let numbers = BODY.bytes().map(serde_json::Value::from).collect();
        let body = InvokeBody::Json(serde_json::Value::Array(numbers));
        assert_eq!(&*core_call_body(&body).unwrap(), BODY.as_bytes());

        let body = InvokeBody::Json(serde_json::json!([1, 256]));
        assert_eq!(core_call_body(&body).unwrap_err().code, "INVALID_ARGUMENT");
    }

    #[test]
    fn core_call_refuses_a_parsed_object() {
        let body = InvokeBody::Json(serde_json::from_str(BODY).unwrap());
        let err = core_call_body(&body).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        assert!(err.message.contains("Uint8Array"), "{}", err.message);
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;
    use seaquel_core::secrets::MemoryStore;

    fn body(json: &str) -> InvokeBody {
        InvokeBody::Raw(json.as_bytes().to_vec())
    }

    const LOAD: &str = r#"{"method":"storage","params":{"method":"queryHistoryLoadByConnection","params":{"connectionId":"c1"}}}"#;
    const SET: &str =
        r#"{"method":"secret","params":{"method":"set","params":{"key":"db:c1","value":"pw"}}}"#;
    const GET: &str = r#"{"method":"secret","params":{"method":"get","params":{"key":"db:c1"}}}"#;

    fn call(core: &Core, ws: &DesktopWorkspace, json: &str) -> Result<serde_json::Value, RpcError> {
        tauri::async_runtime::block_on(handle_core_call(core, ws, "main", &body(json)))
            .map(|res| serde_json::to_value(res).unwrap())
    }

    fn desktop(dir: PathBuf) -> DesktopWorkspace {
        DesktopWorkspace::new(Ok(dir), Arc::new(MemoryStore::new()))
    }

    fn secrets_still_work(core: &Core, ws: &DesktopWorkspace) {
        call(core, ws, SET).unwrap();
        assert_eq!(call(core, ws, GET).unwrap()["result"]["result"], "pw");
    }

    #[test]
    fn storage_opens_on_the_first_call() {
        let core = Core::builder().build();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        assert!(!tmp.path().join("data").exists());
        let res = call(&core, &ws, LOAD).unwrap();
        assert_eq!(res["result"]["result"], serde_json::json!([]));
        assert!(tmp.path().join("data/seaquel.db").is_file());
        secrets_still_work(&core, &ws);
    }

    /// Records every log line, message and key-values, that holds one of
    /// [`MARKERS`]. Installed once for the test binary; other tests' lines
    /// never hold a marker, so they only cost a format.
    struct MarkerLog;

    static MARKERS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    static MARKED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    impl log::Log for MarkerLog {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            struct Kvs<'a>(&'a mut String);
            impl<'kvs> log::kv::VisitSource<'kvs> for Kvs<'_> {
                fn visit_pair(
                    &mut self,
                    key: log::kv::Key<'kvs>,
                    value: log::kv::Value<'kvs>,
                ) -> Result<(), log::kv::Error> {
                    self.0.push_str(&format!(" {key}={value}"));
                    Ok(())
                }
            }
            let mut line = format!("{} {}", record.target(), record.args());
            let _ = record.key_values().visit(&mut Kvs(&mut line));
            let markers = MARKERS.lock().unwrap();
            if markers.iter().any(|m| line.contains(m.as_str())) {
                MARKED.lock().unwrap().push(line);
            }
        }

        fn flush(&self) {}
    }

    /// Task 10's O1: opening storage logs no path (the data dir names the
    /// user's home).
    #[test]
    fn opening_storage_logs_no_path() {
        static LOGGER: MarkerLog = MarkerLog;
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Trace);
        }
        let marker = "datadirmarker7f3a";
        MARKERS.lock().unwrap().push(marker.to_string());
        let core = Core::builder().build();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join(marker));
        call(&core, &ws, LOAD).unwrap();
        assert!(tmp.path().join(marker).join("seaquel.db").is_file());
        let marked = MARKED.lock().unwrap().clone();
        assert!(marked.is_empty(), "the log names the data dir: {marked:?}");
    }

    #[test]
    fn legacy_storage_is_remembered_and_secrets_still_work() {
        let core = Core::builder().build();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("projects.json"), "{}").unwrap();
        let ws = desktop(tmp.path().to_path_buf());

        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "LEGACY_STORAGE");
        secrets_still_work(&core, &ws);
        // Fixing the dir doesn't help until restart: the answer is kept.
        std::fs::remove_file(tmp.path().join("projects.json")).unwrap();
        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "LEGACY_STORAGE");
        assert!(!tmp.path().join("seaquel.db").exists());
    }

    #[test]
    fn corrupt_storage_is_remembered() {
        let core = Core::builder().build();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("seaquel.db"), "not sqlite").unwrap();
        let ws = desktop(tmp.path().to_path_buf());
        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "STORAGE_CORRUPT");
        secrets_still_work(&core, &ws);
        std::fs::remove_file(tmp.path().join("seaquel.db")).unwrap();
        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "STORAGE_CORRUPT");
    }

    /// SSH tunnels need no storage: they reach Core even when it can't open.
    #[test]
    fn ssh_calls_skip_failed_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let core = Core::builder()
            .ssh_known_hosts(tmp.path().join("known_hosts"))
            .build();
        std::fs::write(tmp.path().join("projects.json"), "{}").unwrap();
        let ws = desktop(tmp.path().to_path_buf());
        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "LEGACY_STORAGE");
        let close =
            r#"{"method":"ssh","params":{"method":"close","params":{"tunnelId":"tunnel-1"}}}"#;
        assert_eq!(
            call(&core, &ws, close).unwrap_err().code,
            "TUNNEL_NOT_FOUND"
        );
    }

    #[test]
    fn a_transient_failure_is_retried() {
        let core = Core::builder().build();
        let tmp = tempfile::tempdir().unwrap();
        // A file where the data dir's parent should be: the open fails with
        // an I/O error.
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "").unwrap();
        let ws = desktop(blocker.join("data"));

        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "STORAGE_ERROR");
        secrets_still_work(&core, &ws);
        std::fs::remove_file(&blocker).unwrap();
        let res = call(&core, &ws, LOAD).unwrap();
        assert_eq!(res["result"]["result"], serde_json::json!([]));
    }

    #[test]
    fn no_data_dir_fails_storage_calls_only() {
        let core = Core::builder().build();
        let ws = DesktopWorkspace::new(
            Err(RpcError::from(CoreError::from(
                seaquel_core::storage::StorageError::NoDataDir,
            ))),
            Arc::new(MemoryStore::new()),
        );
        let err = call(&core, &ws, LOAD).unwrap_err();
        assert_eq!(err.code, "NO_DATA_DIR");
        assert!(err.message.contains("SEAQUEL_DATA_DIR"), "{}", err.message);
        secrets_still_work(&core, &ws);
    }

    // ── The `db` group, `core_stream` and `core_events` ──

    use serde_json::{json, Value as Json};
    use std::sync::mpsc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(30);

    /// The desktop's Core, with SQLite only.
    fn sqlite_core() -> Core {
        seaquel_core::with_plugins(|id| id == "sqlite")
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(seaquel_runtime::TokioExecutor))
            .build()
    }

    fn db_call(
        core: &Core,
        ws: &DesktopWorkspace,
        method: &str,
        params: Json,
    ) -> Result<Json, RpcError> {
        let req = json!({"method": "db", "params": {"method": method, "params": params}});
        let res = call(core, ws, &req.to_string())?;
        assert_eq!(res["result"]["method"], method, "{res}");
        Ok(res["result"]["result"].clone())
    }

    fn form(file: &std::path::Path) -> Json {
        json!({
            "target": {"type": "form", "form": {
                "name": "Lite", "type": "sqlite", "databaseName": file.display().to_string(),
            }},
            "createIfMissing": true,
        })
    }

    /// A form connect to a new SQLite file with a table `t` of `rows` rows.
    fn sqlite(core: &Core, ws: &DesktopWorkspace, file: &std::path::Path, rows: u32) -> String {
        sqlite_by(core, ws, "main", file, rows)
    }

    /// [`sqlite`], connected from webview `label`, which owns it (a reload
    /// of that webview closes it).
    fn sqlite_by(
        core: &Core,
        ws: &DesktopWorkspace,
        label: &str,
        file: &std::path::Path,
        rows: u32,
    ) -> String {
        let id = sqlite_from(core, ws, label, file);
        let sql = format!(
            "CREATE TABLE t AS WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL \
             SELECT x + 1 FROM c WHERE x < {rows}) SELECT x FROM c"
        );
        db_call(core, ws, "execute", json!({"connectionId": id, "sql": sql})).unwrap();
        id
    }

    fn stream_body(connection_id: &str, stream_id: &str, sql: &str) -> String {
        json!({"method": "db", "params": {"method": "queryStream", "params": {
            "connectionId": connection_id, "streamId": stream_id, "sql": sql,
        }}})
        .to_string()
    }

    /// A `db.run` of every statement in `text` at `page_size`.
    fn run_body(connection_id: &str, stream_id: &str, text: &str, page_size: u32) -> String {
        json!({"method": "db", "params": {"method": "run", "params": {
            "connectionId": connection_id, "streamId": stream_id, "text": text,
            "target": {"type": "all"}, "pageSize": page_size,
        }}})
        .to_string()
    }

    /// Run a stream to its end through `run_core_stream`; its events as JSON.
    /// The count it returns is the number of events sent.
    fn stream(core: &Core, ws: &DesktopWorkspace, body: &str) -> Vec<Json> {
        let mut events = Vec::new();
        let sent = tauri::async_runtime::block_on(run_core_stream(
            core,
            ws,
            "main",
            body.as_bytes(),
            |event| {
                events.push(serde_json::to_value(event).unwrap());
                true
            },
        ))
        .unwrap();
        assert_eq!(sent, events.len() as u64);
        events
    }

    fn event_types(events: &[Json]) -> Vec<&str> {
        events
            .iter()
            .map(|e| e["event"]["type"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn db_group_connect_query_disconnect_through_core_call() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));

        let id = sqlite(&core, &ws, &tmp.path().join("a.db"), 3);
        let res = db_call(
            &core,
            &ws,
            "query",
            json!({"connectionId": id, "sql": "SELECT sum(x) AS s FROM t WHERE x > ?", "params": [1]}),
        )
        .unwrap();
        assert_eq!(res["rows"], json!([[5]]), "{res}");
        let res = db_call(
            &core,
            &ws,
            "engine",
            json!({"connectionId": id, "request": {"method": "listSchemas"}}),
        )
        .unwrap();
        assert!(res.is_object(), "{res}");

        // The db workspace is the storage one: saved connects read its rows.
        let err = db_call(
            &core,
            &ws,
            "connect",
            json!({"target": {"type": "saved", "id": "nope"}}),
        )
        .unwrap_err();
        assert_eq!(err.code, "CONNECTION_NOT_FOUND", "{err}");
        assert!(err.message.contains("Saved connection"), "{err}");

        db_call(&core, &ws, "disconnect", json!({"connectionId": id})).unwrap();
        let err = db_call(
            &core,
            &ws,
            "query",
            json!({"connectionId": id, "sql": "SELECT 1"}),
        )
        .unwrap_err();
        assert_eq!(err.code, "CONNECTION_NOT_FOUND");
        // A connection another interface opened on Core isn't reachable.
        let err = db_call(
            &core,
            &ws,
            "disconnect",
            json!({"connectionId": "sqlite-not-ours"}),
        )
        .unwrap_err();
        assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    }

    #[test]
    fn core_stream_sends_batches_then_done() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let id = sqlite(&core, &ws, &tmp.path().join("s.db"), 1000);

        let events = stream(&core, &ws, &stream_body(&id, "s1", "SELECT x FROM t"));
        let types = event_types(&events);
        assert_eq!(types.last(), Some(&"done"), "{types:?}");
        assert!(types[..types.len() - 1].iter().all(|t| *t == "batch"));
        let rows: usize = events
            .iter()
            .filter_map(|e| e["event"]["rows"].as_array())
            .map(Vec::len)
            .sum();
        assert_eq!(rows, 1000);
        assert!(events
            .iter()
            .all(|e| e["type"] == "stream" && e["streamId"] == "s1"));

        // Not a stream: the invoke rejects.
        let cancel = r#"{"method":"db","params":{"method":"cancel","params":{"streamId":"s1"}}}"#;
        let err = tauri::async_runtime::block_on(run_core_stream(
            &core,
            &ws,
            "main",
            cancel.as_bytes(),
            |_| true,
        ))
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        // A connection it doesn't own: one error event.
        let events = stream(&core, &ws, &stream_body("sqlite-x", "s2", "SELECT 1"));
        assert_eq!(event_types(&events), ["error"]);
        assert_eq!(events[0]["event"]["code"], "CONNECTION_NOT_FOUND");
    }

    /// A send that fails (the webview dropped the channel) stops the stream.
    #[test]
    fn core_stream_stops_when_the_channel_is_gone() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let id = sqlite(&core, &ws, &tmp.path().join("g.db"), 1000);
        let body = stream_body(&id, "s", "SELECT a.x, b.x FROM t a, t b");
        let mut tried = 0;
        let sent = tauri::async_runtime::block_on(run_core_stream(
            &core,
            &ws,
            "main",
            body.as_bytes(),
            |_| {
                tried += 1;
                false
            },
        ))
        .unwrap();
        assert_eq!(tried, 1);
        // The failed send isn't counted.
        assert_eq!(sent, 0);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert_eq!(db.ws.stream_count(&core), 0);
    }

    #[test]
    fn cancel_stops_a_running_stream() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        let id = sqlite(&core, &ws, &tmp.path().join("c.db"), 2000);

        let (tx, rx) = mpsc::channel();
        let task = {
            let (core, ws) = (core.clone(), ws.clone());
            let body = stream_body(&id, "big", "SELECT a.x, b.x FROM t a, t b");
            tauri::async_runtime::spawn(async move {
                run_core_stream(&core, &ws, "main", body.as_bytes(), |event| {
                    tx.send(serde_json::to_value(event).unwrap()).is_ok()
                })
                .await
            })
        };
        let first = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(first["event"]["type"], "batch", "{first}");

        db_call(&core, &ws, "cancel", json!({"streamId": "big"})).unwrap();
        // The rest are batches already on their way; no done after a cancel.
        loop {
            match rx.recv_timeout(WAIT) {
                Ok(event) => assert_eq!(event["event"]["type"], "batch", "{event}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("the cancelled stream didn't end: {e}"),
            }
        }
        tauri::async_runtime::block_on(task).unwrap().unwrap();
    }

    /// A cancel that overtakes its stream's start (separate IPC calls)
    /// still stops it: the stream ends at once with no events.
    #[test]
    fn a_cancel_before_the_stream_starts_still_stops_it() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let id = sqlite(&core, &ws, &tmp.path().join("e.db"), 10);

        db_call(&core, &ws, "cancel", json!({"streamId": "early"})).unwrap();
        let events = stream(&core, &ws, &stream_body(&id, "early", "SELECT x FROM t"));
        assert!(events.is_empty(), "{events:?}");

        // Other ids aren't touched, and the early cancel is used up.
        let events = stream(&core, &ws, &stream_body(&id, "other", "SELECT x FROM t"));
        assert_eq!(event_types(&events).last(), Some(&"done"));
        let events = stream(&core, &ws, &stream_body(&id, "early", "SELECT x FROM t"));
        assert_eq!(event_types(&events).last(), Some(&"done"));
    }

    /// `core_stream` serves `db.run` and `db.page` like a query stream: `run`
    /// events with their `streamId`, one terminal event, and the count of
    /// events sent as the reply.
    #[test]
    fn core_stream_serves_a_run_and_returns_its_event_count() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let id = sqlite(&core, &ws, &tmp.path().join("run.db"), 1000);

        // `stream` checks the returned count against the events received.
        let events = stream(
            &core,
            &ws,
            &run_body(
                &id,
                "r1",
                "SELECT x FROM t; SELECT nope; SELECT 1 AS one",
                100,
            ),
        );
        assert!(events
            .iter()
            .all(|e| e["type"] == "run" && e["streamId"] == "r1"));
        assert_eq!(
            event_types(&events),
            [
                "statementStart",
                "batch",
                "statementDone",
                "statementStart",
                "statementError",
                "statementStart",
                "batch",
                "statementDone",
                "done"
            ]
        );
        assert_eq!(events[2]["event"]["totalRows"], 1000);
        assert_eq!(events[2]["event"]["totalPages"], 10);

        let page = json!({"method": "db", "params": {"method": "page", "params": {
            "connectionId": id, "streamId": "p1", "source": events[0]["event"]["source"],
            "page": 10, "pageSize": 100,
        }}})
        .to_string();
        let events = stream(&core, &ws, &page);
        assert_eq!(
            event_types(&events),
            ["statementStart", "batch", "statementDone", "done"]
        );
        assert_eq!(events[1]["event"]["rows"][99], json!([1000]));

        // A destructive run without confirmation: one terminal error.
        let events = stream(&core, &ws, &run_body(&id, "r2", "DROP TABLE t", 100));
        assert_eq!(event_types(&events), ["error"]);
        assert_eq!(events[0]["event"]["code"], "CONFIRM_REQUIRED");
        // Tracking ended with the runs.
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
    }

    /// A reload cancels the webview's running run: the statement in flight
    /// stops, the later ones never run, and no terminal event comes.
    #[test]
    fn a_reload_cancels_a_running_run() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        // Opened by another webview, so the reload (which closes main's
        // connections) leaves it, and the cancel alone is what's seen.
        let id = sqlite_by(&core, &ws, "theme-editor", &tmp.path().join("rr.db"), 2000);
        register(&core, &ws, "main", Box::new(|_| true));

        let (rx, task) = background_stream(
            &core,
            &ws,
            "main",
            run_body(
                &id,
                "run",
                "SELECT a.x, b.x FROM t a, t b; CREATE TABLE after_run (a int)",
                0,
            ),
        );
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap()["event"]["type"],
            "statementStart"
        );
        assert_eq!(rx.recv_timeout(WAIT).unwrap()["event"]["type"], "batch");
        assert!(Webviews::lock(&ws.webviews).streams["main"].contains("run"));

        register(&core, &ws, "main", Box::new(|_| true));
        ends_cancelled(&rx);
        let sent = tauri::async_runtime::block_on(task).unwrap().unwrap();
        assert!(sent >= 2);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert_eq!(db.ws.stream_count(&core), 0);
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
        let res = db_call(
            &core,
            &ws,
            "query",
            json!({"connectionId": id, "sql": "SELECT count(*) FROM sqlite_master WHERE name = 'after_run'"}),
        )
        .unwrap();
        assert_eq!(res["rows"], json!([[0]]));
    }

    /// A reload cancels the webview's running table page (phase 5c): its
    /// query stops, no terminal event comes, and its tracking ends.
    #[test]
    fn a_reload_cancels_a_table_page() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        // Opened by another webview, so main's reload cancels the page
        // without closing its connection.
        let id = sqlite_by(&core, &ws, "theme-editor", &tmp.path().join("tp.db"), 20000);
        // Every row of the view is a scan of a 20,000 × 20,000 join.
        db_call(
            &core,
            &ws,
            "execute",
            json!({"connectionId": id, "sql": "CREATE VIEW slow AS SELECT a.x AS x, b.x AS y FROM t a, t b"}),
        )
        .unwrap();
        register(&core, &ws, "main", Box::new(|_| true));

        let body = json!({"method": "db", "params": {"method": "tablePage", "params": {
            "connectionId": id, "streamId": "tp", "page": 1, "pageSize": 10,
            "query": {"target": {"schema": "main", "table": "slow"},
                      "filters": [{"column": "y", "op": "=", "value": "-1"}]},
        }}})
        .to_string();
        let (rx, task) = background_stream(&core, &ws, "main", body);
        let start = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(start["type"], "run", "{start}");
        assert_eq!(start["streamId"], "tp", "{start}");
        assert_eq!(start["event"]["type"], "statementStart", "{start}");
        assert!(Webviews::lock(&ws.webviews).streams["main"].contains("tp"));

        register(&core, &ws, "main", Box::new(|_| true));
        ends_cancelled(&rx);
        let sent = tauri::async_runtime::block_on(task).unwrap().unwrap();
        assert_eq!(sent, 1);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert_eq!(db.ws.stream_count(&core), 0);
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
    }

    /// `core_call` serves the edits service's unary calls; a table page
    /// isn't one of them.
    #[test]
    fn core_call_serves_edits() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let id = sqlite(&core, &ws, &tmp.path().join("e.db"), 1);
        db_call(
            &core,
            &ws,
            "execute",
            json!({"connectionId": id, "sql": "CREATE TABLE p (id INTEGER PRIMARY KEY, name TEXT)"}),
        )
        .unwrap();
        let target = json!({"schema": "main", "table": "p"});
        let outcome = db_call(
            &core,
            &ws,
            "applyChanges",
            json!({"connectionId": id, "changes": [
                {"type": "edit", "id": "a", "edit": {"type": "insertRow", "target": target, "values": [["id", 1], ["name", "x"]]}},
                {"type": "edit", "id": "b", "edit": {"type": "updateCell", "target": target, "key": [["id", 1]], "column": "name", "value": "y"}},
            ]}),
        )
        .unwrap();
        assert_eq!(outcome["mode"], "atomic", "{outcome}");
        assert_eq!(outcome["applied"], 2, "{outcome}");
        let err = db_call(
            &core,
            &ws,
            "tablePage",
            json!({"connectionId": id, "streamId": "t", "page": 1, "pageSize": 10, "query": {"target": target}}),
        )
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
    }

    fn sink(tx: mpsc::Sender<Json>) -> EventSink {
        Box::new(move |event| tx.send(serde_json::to_value(event).unwrap()).is_ok())
    }

    /// Every webview's sink gets every event; a second `core_events` from
    /// one webview (a reload) replaces only that webview's sink.
    #[test]
    fn core_events_reach_every_webview() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (old_tx, old_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(old_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));
        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));

        let id = sqlite(&core, &ws, &tmp.path().join("v.db"), 1);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        tauri::async_runtime::block_on(db.ws.close_all(&core));

        for rx in [&rx, &editor_rx] {
            let event = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(event["type"], "connectionClosed", "{event}");
            assert_eq!(event["code"], "WORKSPACE_EVICTED", "{event}");
            assert_eq!(event["connectionId"], id.as_str());
        }
        // The replaced sink was dropped: its channel is closed, and empty.
        assert_eq!(old_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    /// Phase 5d: `core_call` serves the library, and a write's event carries
    /// the calling webview's label as its origin; every webview's sink gets
    /// it, including one registered before storage opened.
    #[test]
    fn core_call_serves_library_with_the_webview_origin() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (main_tx, main_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(main_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));

        let lib = |label: &str, method: &str, params: Json| {
            let body = json!({"method": "library", "params": {"method": method, "params": params}});
            let res = tauri::async_runtime::block_on(handle_core_call(
                &core,
                &ws,
                label,
                &InvokeBody::Raw(body.to_string().into_bytes()),
            ))
            .unwrap();
            let res = serde_json::to_value(res).unwrap();
            assert_eq!(res["result"]["method"], method, "{res}");
            res["result"]["result"].clone()
        };
        let body = json!({"method": "library", "params": {"method": "projectEnsureDefault"}});
        tauri::async_runtime::block_on(handle_core_call(
            &core,
            &ws,
            "main",
            &InvokeBody::Raw(body.to_string().into_bytes()),
        ))
        .unwrap();
        let created = lib(
            "theme-editor",
            "savedQueryCreate",
            json!({"query": {"projectId": "default-seaquel", "name": "Q", "query": "SELECT 1"}}),
        );
        let id = created["value"]["id"].as_str().unwrap();

        for rx in [&main_rx, &editor_rx] {
            // The ensure-default's event first, from `main`.
            let first = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(first["type"], "storageChanged", "{first}");
            assert_eq!(first["kind"], "project", "{first}");
            assert_eq!(first["origin"], "main", "{first}");
            let event = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(
                event,
                json!({"type": "storageChanged", "kind": "savedQuery", "scope": "default-seaquel",
                       "ids": [id], "origin": "theme-editor", "seq": created["seq"]})
            );
        }

        // A label that isn't a valid origin writes with none.
        lib(
            "odd label/with:chars",
            "savedQueryUpdate",
            json!({"id": id, "patch": {"starred": true}}),
        );
        assert_eq!(main_rx.recv_timeout(WAIT).unwrap()["origin"], Json::Null);
    }

    /// Phase 5d-2: `core_call` serves the `settings` and `ui` groups, a
    /// write's event carries the calling webview's label, and a `ui` call
    /// works only for that webview's own window id.
    #[test]
    fn core_call_serves_settings_and_ui_with_the_webview_origin() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (main_tx, main_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(main_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));

        let group = |label: &str, group: &str, method: &str, params: Json| {
            let inner = if params.is_null() {
                json!({"method": method})
            } else {
                json!({"method": method, "params": params})
            };
            let body = json!({"method": group, "params": inner});
            tauri::async_runtime::block_on(handle_core_call(
                &core,
                &ws,
                label,
                &InvokeBody::Raw(body.to_string().into_bytes()),
            ))
            .map(|res| {
                let res = serde_json::to_value(res).unwrap();
                assert_eq!(res["result"]["method"], method, "{res}");
                res["result"]["result"].clone()
            })
        };
        group("main", "library", "projectEnsureDefault", Json::Null).unwrap();
        for rx in [&main_rx, &editor_rx] {
            assert_eq!(rx.recv_timeout(WAIT).unwrap()["kind"], "project");
        }

        // settings: a write from the theme editor carries its label.
        let theme = group(
            "theme-editor",
            "settings",
            "userThemeCreate",
            json!({"theme": {"name": "Mine"}}),
        )
        .unwrap();
        for rx in [&main_rx, &editor_rx] {
            let event = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(
                event,
                json!({"type": "storageChanged", "kind": "theme", "scope": null,
                       "ids": [theme["value"]["id"]], "origin": "theme-editor",
                       "seq": theme["seq"]})
            );
        }
        // The desktop has a keychain: an AI provider's key is written.
        let provider = group(
            "main",
            "settings",
            "aiProviderCreate",
            json!({"provider": {"name": "P", "type": "anthropic"}, "apiKey": "k-1"}),
        )
        .unwrap();
        let id = provider["value"]["id"].as_str().unwrap();
        // Read from the keychain itself: the `secret` group refuses AI keys
        // (phase 6).
        assert_eq!(
            tauri::async_runtime::block_on(ws.secrets.get(&format!("ai-api-key:{id}")))
                .unwrap()
                .as_deref(),
            Some("k-1")
        );
        assert_eq!(main_rx.recv_timeout(WAIT).unwrap()["kind"], "aiSettings");
        assert_eq!(editor_rx.recv_timeout(WAIT).unwrap()["kind"], "aiSettings");

        // ui: the main window's own view state; the theme editor can't
        // name it.
        let state = json!({"projectId": "default-seaquel", "queryTabs": [], "schemaTabs": [],
            "explainTabs": [], "erdTabs": [], "tabOrder": [], "activeView": "query"});
        let saved = group(
            "main",
            "ui",
            "windowStateSave",
            json!({"windowId": "main", "projectId": "default-seaquel", "rev": 1,
                "state": state}),
        )
        .unwrap();
        assert_eq!(saved["value"], json!({"stale": false, "rev": 1}));
        for rx in [&main_rx, &editor_rx] {
            let event = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(event["kind"], "projectState", "{event}");
            assert_eq!(event["ids"], json!(["main"]), "{event}");
            assert_eq!(event["origin"], "main", "{event}");
        }
        let err = group(
            "theme-editor",
            "ui",
            "windowStateLoad",
            json!({"windowId": "main", "projectId": "default-seaquel"}),
        )
        .unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT");
        let loaded = group(
            "main",
            "ui",
            "windowStateLoad",
            json!({"windowId": "main", "projectId": "default-seaquel"}),
        )
        .unwrap();
        assert_eq!(loaded["value"]["rev"], 1);
        assert!(loaded["value"]["copiedFrom"].is_null(), "{loaded}");
    }

    /// A run's history event carries the label of the
    /// webview that started it through `core_stream`.
    #[test]
    fn a_runs_history_event_carries_the_webview_label() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));
        let file = tmp.path().join("h.db");
        call(
            &core,
            &ws,
            r#"{"method":"library","params":{"method":"projectEnsureDefault"}}"#,
        )
        .unwrap();
        let saved = call(
            &core,
            &ws,
            &json!({"method": "library", "params": {"method": "connectionCreate", "params": {
                "connection": {"projectId": "default-seaquel", "name": "Lite", "type": "sqlite",
                    "host": "", "port": 0, "databaseName": file.display().to_string(),
                    "username": ""}}}})
            .to_string(),
        )
        .unwrap()["result"]["result"]["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let id = sqlite(&core, &ws, &file, 1);
        let body = json!({"method": "db", "params": {"method": "run", "params": {
            "connectionId": id, "streamId": "r-h", "text": "SELECT 1", "target": {"type": "all"},
            "pageSize": 10, "history": {"connectionId": saved, "connectionName": "Lite",
            "connectionLabels": []}}}})
        .to_string();
        let mut events = Vec::new();
        tauri::async_runtime::block_on(run_core_stream(
            &core,
            &ws,
            "log-viewer",
            body.as_bytes(),
            |event| {
                events.push(serde_json::to_value(event).unwrap());
                true
            },
        ))
        .unwrap();
        assert_eq!(
            events.last().unwrap()["event"]["type"],
            "done",
            "{events:?}"
        );
        let history = loop {
            let event = rx.recv_timeout(WAIT).unwrap();
            if event["kind"] == "history" {
                break event;
            }
        };
        assert_eq!(history["scope"], json!(saved));
        assert_eq!(history["origin"], "log-viewer");
    }

    /// A storage-group write reaches every webview too, with its origin.
    #[test]
    fn a_write_reaches_every_webview_sink() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (main_tx, main_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(main_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));
        let body = r#"{"method":"storage","params":{"method":"userCredentialsSave","params":{"credential":{"scope":"db","key":"k","nonce":"n","ciphertext":"c","updatedAt":"t"}}}}"#;
        tauri::async_runtime::block_on(handle_core_call(
            &core,
            &ws,
            "main",
            &InvokeBody::Raw(body.as_bytes().to_vec()),
        ))
        .unwrap();
        for rx in [&main_rx, &editor_rx] {
            let event = rx.recv_timeout(WAIT).unwrap();
            assert_eq!(event["type"], "storageChanged", "{event}");
            assert_eq!(event["kind"], "storage", "{event}");
            assert_eq!(event["ids"], json!(["k"]), "{event}");
            assert_eq!(event["origin"], "main", "{event}");
        }
        // One pump per workspace: no second copy once `db` picks it too.
        tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        tauri::async_runtime::block_on(handle_core_call(
            &core,
            &ws,
            "main",
            &InvokeBody::Raw(body.as_bytes().to_vec()),
        ))
        .unwrap();
        assert_eq!(main_rx.recv_timeout(WAIT).unwrap()["kind"], "storage");
        assert!(main_rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    /// A stream on webview `label`, run in the background; its events as
    /// JSON, and its task.
    fn background_stream(
        core: &Arc<Core>,
        ws: &Arc<DesktopWorkspace>,
        label: &'static str,
        body: String,
    ) -> (
        mpsc::Receiver<Json>,
        tauri::async_runtime::JoinHandle<Result<u64, RpcError>>,
    ) {
        let (tx, rx) = mpsc::channel();
        let (core, ws) = (core.clone(), ws.clone());
        let task = tauri::async_runtime::spawn(async move {
            run_core_stream(&core, &ws, label, body.as_bytes(), |event| {
                tx.send(serde_json::to_value(event).unwrap()).is_ok()
            })
            .await
        });
        (rx, task)
    }

    /// Only batches until the stream's task ends: it was cancelled.
    fn ends_cancelled(rx: &mpsc::Receiver<Json>) {
        loop {
            match rx.recv_timeout(WAIT) {
                Ok(event) => assert_eq!(event["event"]["type"], "batch", "{event}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("the stream didn't end: {e}"),
            }
        }
    }

    /// Only batches until the stream's task ends, cancelled, or ended by its
    /// connection's close (`CONNECTION_CLOSED`): what a closed webview's
    /// stream may see, since its connections close right after its streams
    /// are cancelled. Its channel is gone either way.
    fn ends_cancelled_or_closed(rx: &mpsc::Receiver<Json>) {
        loop {
            match rx.recv_timeout(WAIT) {
                Ok(event) if event["event"]["type"] == "error" => {
                    assert_eq!(event["event"]["code"], "CONNECTION_CLOSED", "{event}");
                }
                Ok(event) => assert_eq!(event["event"]["type"], "batch", "{event}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("the stream didn't end: {e}"),
            }
        }
    }

    /// A reload (`core_events` again from the same webview) cancels that
    /// webview's streams, whose channels the old page can't read; another
    /// webview's streams go on.
    #[test]
    fn a_reload_cancels_the_webviews_streams() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        // Opened by the editor, so main's reload (which closes main's
        // connections) cancels main's stream without closing it.
        let id = sqlite_by(&core, &ws, "theme-editor", &tmp.path().join("r.db"), 2000);
        let big = "SELECT a.x, b.x FROM t a, t b";
        register(&core, &ws, "main", Box::new(|_| true));

        let (main_rx, main_task) =
            background_stream(&core, &ws, "main", stream_body(&id, "m", big));
        let (editor_rx, editor_task) =
            background_stream(&core, &ws, "theme-editor", stream_body(&id, "e", big));
        main_rx.recv_timeout(WAIT).unwrap();
        editor_rx.recv_timeout(WAIT).unwrap();

        register(&core, &ws, "main", Box::new(|_| true));
        ends_cancelled(&main_rx);
        tauri::async_runtime::block_on(main_task).unwrap().unwrap();

        // The editor's stream still runs, until its window is destroyed.
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert_eq!(db.ws.stream_count(&core), 1);
        assert!(Webviews::lock(&ws.webviews)
            .streams
            .contains_key("theme-editor"));
        assert!(!Webviews::lock(&ws.webviews).streams.contains_key("main"));
        forget(&core, &ws, "theme-editor");
        ends_cancelled_or_closed(&editor_rx);
        tauri::async_runtime::block_on(editor_task)
            .unwrap()
            .unwrap();
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
    }

    /// `core_events` from webview `label` ([`DesktopWorkspace::set_event_sink`]).
    fn register(core: &Core, ws: &DesktopWorkspace, label: &str, sink: EventSink) {
        tauri::async_runtime::block_on(ws.set_event_sink(core, label, sink));
    }

    /// Webview `label` destroyed ([`DesktopWorkspace::forget_webview`]).
    fn forget(core: &Core, ws: &DesktopWorkspace, label: &str) {
        tauri::async_runtime::block_on(ws.forget_webview(core, label));
    }

    /// A form connect to a new SQLite file from webview `label`.
    fn sqlite_from(
        core: &Core,
        ws: &DesktopWorkspace,
        label: &str,
        file: &std::path::Path,
    ) -> String {
        let req = json!({"method": "db", "params": {"method": "connect", "params": form(file)}});
        let res = tauri::async_runtime::block_on(handle_core_call(
            core,
            ws,
            label,
            &body(&req.to_string()),
        ))
        .map(|res| serde_json::to_value(res).unwrap())
        .unwrap();
        res["result"]["result"]["connectionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn alive(core: &Core, ws: &DesktopWorkspace, ids: &[&str]) -> Json {
        db_call(core, ws, "alive", json!({ "connectionIds": ids })).unwrap()
    }

    /// The next `connectionClosed` on `rx`.
    fn closed_event(rx: &mpsc::Receiver<Json>) -> Json {
        loop {
            let event = rx.recv_timeout(WAIT).expect("a connectionClosed event");
            if event["type"] == "connectionClosed" {
                return event;
            }
        }
    }

    /// A reload (`core_events`
    /// again from one webview) closes the connections that webview opened,
    /// which the reloaded page can't name any more, announced as
    /// `WINDOW_CLOSED` to the new sink; another webview's connections stay.
    #[test]
    fn a_reload_closes_the_webviews_connections() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (old_tx, _old_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(old_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));
        let main_id = sqlite_from(&core, &ws, "main", &tmp.path().join("m.db"));
        let editor_id = sqlite_from(&core, &ws, "theme-editor", &tmp.path().join("e.db"));

        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));
        for rx in [&rx, &editor_rx] {
            let event = closed_event(rx);
            assert_eq!(event["code"], "WINDOW_CLOSED", "{event}");
            assert_eq!(event["connectionId"], main_id.as_str(), "{event}");
        }
        assert_eq!(
            alive(&core, &ws, &[&main_id, &editor_id]),
            json!([editor_id])
        );
        // Only the reloaded webview's connection was announced.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        // The first `core_events` of a webview closes nothing.
        let (other_tx, _other_rx) = mpsc::channel();
        register(&core, &ws, "log-viewer", sink(other_tx));
        assert_eq!(alive(&core, &ws, &[&editor_id]), json!([editor_id]));
    }

    /// A reload with a stream running on the reloaded
    /// webview's own connection: the stream ends (cancelled, or ended by its
    /// connection's close; the two race) and the new sink gets
    /// `WINDOW_CLOSED` for the connection.
    #[test]
    fn a_reload_ends_a_stream_on_its_own_connection() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        let id = sqlite(&core, &ws, &tmp.path().join("own.db"), 2000);
        register(&core, &ws, "main", Box::new(|_| true));
        let (stream_rx, task) = background_stream(
            &core,
            &ws,
            "main",
            stream_body(&id, "own", "SELECT a.x, b.x FROM t a, t b"),
        );
        stream_rx.recv_timeout(WAIT).unwrap();

        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));
        ends_cancelled_or_closed(&stream_rx);
        tauri::async_runtime::block_on(task).unwrap().unwrap();
        let event = closed_event(&rx);
        assert_eq!(event["code"], "WINDOW_CLOSED", "{event}");
        assert_eq!(event["connectionId"], id.as_str(), "{event}");
        assert_eq!(alive(&core, &ws, &[&id]), json!([]));
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
    }

    /// A destroyed webview's connections close too, and only
    /// its own.
    #[test]
    fn a_destroyed_webview_closes_its_connections() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));
        let (editor_tx, _editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));
        let main_id = sqlite_from(&core, &ws, "main", &tmp.path().join("m.db"));
        let editor_id = sqlite_from(&core, &ws, "theme-editor", &tmp.path().join("e.db"));

        forget(&core, &ws, "theme-editor");
        let event = closed_event(&rx);
        assert_eq!(event["code"], "WINDOW_CLOSED", "{event}");
        assert_eq!(event["connectionId"], editor_id.as_str(), "{event}");
        assert_eq!(alive(&core, &ws, &[&main_id, &editor_id]), json!([main_id]));
    }

    /// A destroyed window's sink is dropped; the others still get events.
    #[test]
    fn a_destroyed_webview_gets_no_more_events() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (tx, rx) = mpsc::channel();
        register(&core, &ws, "main", sink(tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));

        forget(&core, &ws, "theme-editor");
        assert_eq!(editor_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        // Forgetting a label with nothing registered is fine.
        forget(&core, &ws, "never-seen");

        sqlite(&core, &ws, &tmp.path().join("d.db"), 1);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        tauri::async_runtime::block_on(db.ws.close_all(&core));
        assert_eq!(rx.recv_timeout(WAIT).unwrap()["type"], "connectionClosed");
        assert_eq!(
            Webviews::lock(&ws.webviews)
                .sinks
                .keys()
                .collect::<Vec<_>>(),
            ["main"]
        );
    }

    /// Storage failed for good: form connects work on the stand-in
    /// workspace, saved ones answer with the storage error, and storage
    /// calls still do.
    #[test]
    fn failed_storage_still_serves_a_form_connect() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("projects.json"), "{}").unwrap();
        let ws = desktop(tmp.path().to_path_buf());

        let id = sqlite(&core, &ws, &tmp.path().join("f.db"), 2);
        let res = db_call(
            &core,
            &ws,
            "query",
            json!({"connectionId": id, "sql": "SELECT count(*) AS n FROM t"}),
        )
        .unwrap();
        assert_eq!(res["rows"], json!([[2]]));
        let events = stream(&core, &ws, &stream_body(&id, "s", "SELECT x FROM t"));
        assert_eq!(event_types(&events).last(), Some(&"done"));

        for method in ["connect", "test"] {
            let saved = json!({"target": {"type": "saved", "id": "c1"}});
            let err = db_call(&core, &ws, method, saved).unwrap_err();
            assert_eq!(err.code, "LEGACY_STORAGE", "{method}: {err}");
        }
        assert_eq!(call(&core, &ws, LOAD).unwrap_err().code, "LEGACY_STORAGE");
        assert!(!tmp.path().join("seaquel.db").exists());
    }

    #[test]
    fn no_data_dir_still_serves_a_form_connect() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = DesktopWorkspace::new(
            Err(RpcError::from(CoreError::from(
                seaquel_core::storage::StorageError::NoDataDir,
            ))),
            Arc::new(MemoryStore::new()),
        );
        let id = sqlite(&core, &ws, &tmp.path().join("n.db"), 1);
        db_call(&core, &ws, "disconnect", json!({"connectionId": id})).unwrap();
        let saved = json!({"target": {"type": "saved", "id": "c1"}});
        let err = db_call(&core, &ws, "connect", saved).unwrap_err();
        assert_eq!(err.code, "NO_DATA_DIR");
    }

    /// A failure worth retrying fails `db` calls too, without fixing the
    /// stand-in: once storage opens, `db` calls use it.
    #[test]
    fn a_transient_failure_is_retried_for_db_calls() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "").unwrap();
        let ws = desktop(blocker.join("data"));

        let err = db_call(&core, &ws, "connect", form(&tmp.path().join("r.db"))).unwrap_err();
        assert_eq!(err.code, "STORAGE_ERROR");
        std::fs::remove_file(&blocker).unwrap();
        sqlite(&core, &ws, &tmp.path().join("r.db"), 1);
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert!(db.storage_error.is_none());
        assert!(blocker.join("data/seaquel.db").is_file());
    }

    #[test]
    fn git_calls_need_no_storage() {
        let core = Core::builder().build();
        let ws = DesktopWorkspace::new(
            Err(RpcError::from(CoreError::from(
                seaquel_core::storage::StorageError::NoDataDir,
            ))),
            Arc::new(MemoryStore::new()),
        );
        // Not a repository: git's own error, not the storage one.
        let tmp = tempfile::tempdir().unwrap();
        let req = serde_json::json!({"method": "git", "params": {"method": "remoteUrl", "params": {"path": tmp.path()}}});
        let err = call(&core, &ws, &req.to_string()).unwrap_err();
        assert_eq!(err.code, "REPO_OPEN_ERROR", "{}", err.message);
    }

    // ── Phase 5e: the `shared` and `imports` groups, the repo lock ──

    /// The desktop's Core ([`desktop_core`]) with the imports reading
    /// `home`, never the user's.
    fn files_core(home: &std::path::Path) -> Core {
        desktop_core_with(
            Some(home.join("app").join("bin").join("duckdb")),
            Some(seaquel_core::ImportPaths::new(home)),
        )
    }

    /// `core_call` serves both groups through the storage workspace, on a
    /// Core built as the app builds it (`LocalFiles`, the import paths),
    /// and each write's event carries the calling webview's label.
    #[test]
    fn core_call_serves_shared_and_imports_with_the_webview_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let core = files_core(&home);
        assert_eq!(core.local_files(), Some(seaquel_core::LocalFiles::Allowed));
        let ws = desktop(tmp.path().join("data"));
        let (main_tx, main_rx) = mpsc::channel();
        register(&core, &ws, "main", sink(main_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        register(&core, &ws, "theme-editor", sink(editor_tx));
        let group = |label: &str, group: &str, method: &str, params: Json| {
            let inner = if params.is_null() {
                json!({"method": method})
            } else {
                json!({"method": method, "params": params})
            };
            let body = json!({"method": group, "params": inner});
            tauri::async_runtime::block_on(handle_core_call(
                &core,
                &ws,
                label,
                &InvokeBody::Raw(body.to_string().into_bytes()),
            ))
            .map(|res| {
                let res = serde_json::to_value(res).unwrap();
                assert_eq!(res["method"], group, "{res}");
                assert_eq!(res["result"]["method"], method, "{res}");
                res["result"]["result"].clone()
            })
        };
        group("main", "library", "projectEnsureDefault", Json::Null).unwrap();
        for rx in [&main_rx, &editor_rx] {
            assert_eq!(rx.recv_timeout(WAIT).unwrap()["kind"], "project");
        }

        // shared: a repo registered from the theme editor carries its label.
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let registered = group(
            "theme-editor",
            "shared",
            "repoRegister",
            json!({"path": repo.display().to_string()}),
        )
        .unwrap();
        for rx in [&main_rx, &editor_rx] {
            assert_eq!(
                rx.recv_timeout(WAIT).unwrap(),
                json!({"type": "storageChanged", "kind": "sharedRepo", "scope": null,
                    "ids": [registered["value"]["id"]], "origin": "theme-editor",
                    "seq": registered["seq"]})
            );
        }
        let list = group("main", "shared", "reposList", Json::Null).unwrap();
        assert_eq!(list["value"][0]["id"], registered["value"]["id"]);
        let preview = group(
            "main",
            "shared",
            "scan",
            json!({"path": repo.display().to_string()}),
        )
        .unwrap();
        assert_eq!(preview, json!({"conflicted": false, "projects": []}));

        // imports: the default location is under the injected home.
        let none = group(
            "main",
            "imports",
            "candidates",
            json!({"source": "dbeaver", "projectId": "default-seaquel"}),
        )
        .unwrap();
        assert_eq!(none, json!({"found": false}));
        let Some(rel) = seaquel_core::domain::imports::default_path(
            seaquel_core::domain::imports::ImportSource::Dbeaver,
            std::env::consts::OS,
        ) else {
            return;
        };
        let file = home.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            r#"{"connections": {"pg-1": {"provider": "postgresql", "name": "Shop",
                "configuration": {"host": "db.example.com", "port": "5432", "database": "shop",
                "user": "app"}}}}"#,
        )
        .unwrap();
        let found = group(
            "main",
            "imports",
            "candidates",
            json!({"source": "dbeaver", "projectId": "default-seaquel"}),
        )
        .unwrap();
        assert_eq!(found["candidates"][0]["key"], "pg-1", "{found}");
        let created = group(
            "log-viewer",
            "imports",
            "create",
            json!({"source": "dbeaver", "projectId": "default-seaquel", "keys": ["pg-1"]}),
        )
        .unwrap();
        assert_eq!(created["value"]["results"][0]["status"], "imported");
        let id = &created["value"]["results"][0]["id"];
        let mut kinds = Vec::new();
        for _ in 0..2 {
            let event = main_rx.recv_timeout(WAIT).unwrap();
            assert_eq!(event["origin"], "log-viewer", "{event}");
            if event["kind"] == "connection" {
                assert_eq!(event["ids"], json!([id]), "{event}");
            }
            kinds.push(event["kind"].as_str().unwrap().to_string());
        }
        kinds.sort();
        assert_eq!(kinds, ["connection", "project"]);
    }

    /// A pull through `core_call` waits for the repo's lock,
    /// which the shared projection's syncs and publishes hold, with storage
    /// open and with no storage at all.
    #[test]
    fn pull_takes_the_repo_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let core = Arc::new(files_core(&home));
        let with_storage = desktop(tmp.path().join("data"));
        let no_storage = DesktopWorkspace::new(
            Err(RpcError::from(CoreError::from(
                seaquel_core::storage::StorageError::NoDataDir,
            ))),
            Arc::new(MemoryStore::new()),
        );
        // Not a repository, so git answers at once (without reading any
        // git config) once it runs.
        let repo = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&repo).unwrap();
        for mut ws in [with_storage, no_storage] {
            ws.git = Git::new(Some(home.clone()));
            let ws = Arc::new(ws);
            let lock = tauri::async_runtime::block_on(core.repo_lock(&repo));
            let (tx, rx) = mpsc::channel();
            let (c, w, path) = (core.clone(), ws.clone(), repo.display().to_string());
            let task = std::thread::spawn(move || {
                let body = json!({"method": "git", "params": {"method": "pull",
                    "params": {"path": path}}});
                let res = tauri::async_runtime::block_on(handle_core_call(
                    &c,
                    &w,
                    "main",
                    &InvokeBody::Raw(body.to_string().into_bytes()),
                ));
                tx.send(res.map(|_| ()).map_err(|e| e.code)).unwrap();
            });
            assert!(
                rx.recv_timeout(Duration::from_millis(500)).is_err(),
                "the pull ran while the repo was locked"
            );
            drop(lock);
            let answer = rx.recv_timeout(WAIT).unwrap();
            assert!(answer.is_err(), "not a repo: {answer:?}");
            task.join().unwrap();
        }
    }

    // ── DuckDB through the helper ──

    /// `<tmp>/app.seaquel.desktop/bin/duckdb`, the folders 0700 as an
    /// install makes them: where [`desktop_core_with`] looks in these
    /// tests, never the user's data-local dir.
    fn helper_dir(tmp: &std::path::Path) -> PathBuf {
        let ident = tmp.join("app.seaquel.desktop");
        let dir = ident.join("bin").join("duckdb");
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        for d in [ident.as_path(), &ident.join("bin"), &dir] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    /// The app's Core registers `duckdb` once, through the helper's locator
    /// (`with_plugins(|id| id != "duckdb")`, so a build where Cargo unified
    /// a native driver in still has one `duckdb`), for this app version.
    #[test]
    fn desktop_core_registers_duckdb_once_through_the_helper() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = helper_dir(tmp.path());
        let core = desktop_core_with(Some(dir.clone()), None);
        let ids = core.engine_ids();
        assert_eq!(
            ids.iter().filter(|id| **id == "duckdb").count(),
            1,
            "{ids:?}"
        );
        for id in ["postgres", "mysql", "sqlite", "mssql"] {
            assert!(ids.contains(&id), "{id}: {ids:?}");
        }
        let helper = core.duckdb_helper().expect("the helper's locator");
        assert_eq!(helper.dir, dir);
        assert_eq!(helper.version, cli_download::VERSION);

        // No folder for it (no data-local dir): no DuckDB at all.
        let core = desktop_core_with(None, None);
        assert!(!core.engine_ids().contains(&"duckdb"));
        assert!(core.duckdb_helper().is_none());
    }

    /// The helper asset's pin reaches Core as the build script's
    /// parser read it, and only a pin compiled in by `build.rs` is used: a
    /// build without one has none.
    #[test]
    fn desktop_core_carries_the_pin_built_in() {
        const HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let tmp = tempfile::tempdir().unwrap();
        let dir = helper_dir(tmp.path());
        let pin = helper_pin::pin_from_inputs(Some("4096"), Some(&HEX.to_uppercase()))
            .unwrap()
            .unwrap();
        let pin = helper_pin::compiled_pin(Some(&helper_pin::pin_text(&pin)));
        let core = desktop_core_pinned(Some(dir.clone()), pin, None);
        let got = core.duckdb_helper_pin().expect("the pin");
        assert_eq!((got.size, got.sha256.as_str()), (4096, HEX));

        let core = desktop_core_pinned(Some(dir.clone()), None, None);
        assert!(core.duckdb_helper_pin().is_none());

        // The app's own Core: what this build compiled in, nothing else.
        let core = desktop_core_with(Some(dir), None);
        let built = helper_pin::compiled_pin(option_env!("SEAQUEL_DUCKDB_HELPER_PIN"));
        assert_eq!(
            core.duckdb_helper_pin().map(|p| (p.size, p.sha256.clone())),
            built.map(|p| (p.size, p.sha256))
        );
    }

    /// Runs `test` again in a child process of this test binary with
    /// `CHILD` set and `envs` changed (`None` removes one), since the
    /// variables `desktop_core` reads are process-wide.
    fn in_child(test: &str, envs: &[(&str, Option<&std::path::Path>)]) {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            &format!("workspace_tests::{test}"),
            "--test-threads=1",
        ])
        .env(DESKTOP_CORE_CHILD, "1");
        for (key, value) in envs {
            match value {
                Some(v) => cmd.env(key, v),
                None => cmd.env_remove(key),
            };
        }
        let out = cmd.output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}");
        assert!(
            stdout.contains("1 passed"),
            "the child ran the check: {stdout}"
        );
    }

    const DESKTOP_CORE_CHILD: &str = "SEAQUEL_TEST_DESKTOP_CORE_CHILD";

    fn is_child() -> bool {
        std::env::var_os(DESKTOP_CORE_CHILD).is_some()
    }

    /// `desktop_core(identifier, …)` takes the helper's folder from the
    /// identifier, under `SEAQUEL_DATA_DIR` when that is set, where the
    /// terminal binaries and the CLI's install look; with a folder there
    /// is nothing to warn about.
    #[test]
    fn desktop_core_finds_the_helper_under_seaquel_data_dir() {
        if is_child() {
            let data = PathBuf::from(std::env::var_os("SEAQUEL_DATA_DIR").unwrap());
            let (core, warning) = desktop_core("app.seaquel.desktop", None);
            assert_eq!(warning, None);
            let helper = core.duckdb_helper().expect("the helper's locator");
            assert_eq!(helper.dir, data.join("bin").join("duckdb"));
            assert_eq!(
                core.engine_ids()
                    .iter()
                    .filter(|id| **id == "duckdb")
                    .count(),
                1
            );
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        in_child(
            "desktop_core_finds_the_helper_under_seaquel_data_dir",
            &[("SEAQUEL_DATA_DIR", Some(tmp.path()))],
        );
    }

    /// Without `SEAQUEL_DATA_DIR` the folder is the platform's data-local
    /// dir (under `HOME`, here a temp folder) plus the identifier the app
    /// passed, so a hard-coded identifier fails.
    #[test]
    fn desktop_core_names_the_helper_folder_after_the_identifier() {
        const IDENT: &str = "app.seaquel.test-ident";
        if is_child() {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap());
            let (core, warning) = desktop_core(IDENT, None);
            assert_eq!(warning, None);
            let dir = &core.duckdb_helper().expect("the helper's locator").dir;
            assert!(
                dir.ends_with(std::path::Path::new(IDENT).join("bin").join("duckdb")),
                "{dir:?}"
            );
            assert!(dir.starts_with(&home), "{dir:?}");
            assert!(!dir.exists(), "nothing is made before an install");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        in_child(
            "desktop_core_names_the_helper_folder_after_the_identifier",
            &[
                ("SEAQUEL_DATA_DIR", None),
                ("HOME", Some(home.as_path())),
                ("XDG_DATA_HOME", Some(home.join(".local/share").as_path())),
            ],
        );
    }

    /// I1 of Task 1's review: `desktop_core` runs before `tauri-plugin-log`
    /// attaches its logger, so the missing folder's WARN is kept and
    /// `setup` logs it ([`run`]).
    #[test]
    fn a_missing_helper_folder_is_kept_for_setup_to_log() {
        assert_eq!(
            helper_folder_warning(&Err("Couldn't find your data directory.".into())),
            Some(NO_HELPER_FOLDER)
        );
        assert_eq!(
            helper_folder_warning(&Ok(PathBuf::from("/x/bin/duckdb"))),
            None
        );
        assert!(!NO_HELPER_FOLDER.contains('/'));
    }

    fn duckdb_form(file: &std::path::Path) -> Json {
        json!({
            "target": {"type": "form", "form": {
                "name": "Duck", "type": "duckdb", "databaseName": file.display().to_string(),
            }},
            "createIfMissing": true,
        })
    }

    /// With no helper installed, a DuckDB connect through `core_call` is
    /// refused at once with `ENGINE_NOT_INSTALLED` (the GUI's dialog then
    /// downloads between two connects, never inside one): nothing spawned,
    /// no file made.
    #[test]
    fn a_duckdb_connect_without_the_helper_is_refused_at_once() {
        let tmp = tempfile::tempdir().unwrap();
        let core = desktop_core_with(Some(helper_dir(tmp.path())), None);
        let ws = desktop(tmp.path().join("data"));
        // Storage opens on the first call; time only the connect.
        call(&core, &ws, LOAD).unwrap();
        let dir = tmp
            .path()
            .join("app.seaquel.desktop")
            .join("bin")
            .join("duckdb");
        let file = tmp.path().join("missing-helper.duckdb");
        let started = std::time::Instant::now();
        let err = db_call(&core, &ws, "connect", duckdb_form(&file)).unwrap_err();
        let took = started.elapsed();
        assert_eq!(err.code, "ENGINE_NOT_INSTALLED", "{err}");
        // No download and no connect timeout: refused before any spawn.
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing installed"
        );
        #[cfg(unix)]
        {
            let children = std::process::Command::new("pgrep")
                .args(["-P", &std::process::id().to_string(), "seaquel-duckdb"])
                .output()
                .unwrap();
            assert!(children.stdout.is_empty(), "a helper was spawned");
        }
        assert!(!err.message.contains(&tmp.path().display().to_string()));
    }

    /// `SEAQUEL_TEST_DUCKDB_HELPER`, or `None` to skip (a failure under
    /// `SEAQUEL_TEST_REQUIRE_ENGINES`).
    fn built_helper() -> Option<PathBuf> {
        match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
            Some(path) => Some(PathBuf::from(path)),
            None if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
                panic!("SEAQUEL_TEST_DUCKDB_HELPER is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
            }
            None => {
                eprintln!("skipping: SEAQUEL_TEST_DUCKDB_HELPER is not set");
                None
            }
        }
    }

    /// The built helper linked (else copied) into `dir/<version>/`, laid
    /// out as an install.
    fn install_helper(built: &std::path::Path, dir: &std::path::Path) {
        let folder = dir.join(cli_download::VERSION);
        std::fs::create_dir_all(&folder).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let to = folder.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX));
        if std::fs::hard_link(built, &to).is_err() {
            std::fs::copy(built, &to).unwrap();
        }
    }

    /// The GUI's DuckDB paths through the app's own commands, on the
    /// helper: a form connect through `core_call`, a run and its pages
    /// through `core_stream`, the data tab's table page, and a grid edit
    /// through `applyChanges`, on a DuckDB file.
    #[test]
    fn duckdb_runs_through_the_helper_on_the_app_s_core() {
        let Some(built) = built_helper() else { return };
        let tmp = tempfile::tempdir().unwrap();
        let dir = helper_dir(tmp.path());
        install_helper(&built, &dir);
        let core = desktop_core_with(Some(dir), None);
        let ws = desktop(tmp.path().join("data"));
        let file = tmp.path().join("app.duckdb");

        // DuckDB opens only a file that exists: make it from `:memory:`.
        let seed = db_call(
            &core,
            &ws,
            "connect",
            duckdb_form(std::path::Path::new(":memory:")),
        )
        .unwrap()["connectionId"]
            .as_str()
            .unwrap()
            .to_string();
        let sql = format!(
            "ATTACH '{}' AS seed; CREATE TABLE seed.t AS SELECT range::INTEGER AS x \
             FROM range(1, 1001); DETACH seed",
            file.display()
        );
        for statement in sql.split("; ") {
            db_call(
                &core,
                &ws,
                "execute",
                json!({"connectionId": seed, "sql": statement}),
            )
            .unwrap();
        }
        db_call(&core, &ws, "disconnect", json!({"connectionId": seed})).unwrap();
        assert!(file.is_file());

        let res = db_call(&core, &ws, "connect", duckdb_form(&file)).unwrap();
        let id = res["connectionId"].as_str().unwrap().to_string();

        let events = stream(
            &core,
            &ws,
            &run_body(&id, "r1", "SELECT x FROM t ORDER BY x", 100),
        );
        assert_eq!(
            event_types(&events),
            ["statementStart", "batch", "statementDone", "done"],
            "{events:?}"
        );
        assert_eq!(events[1]["event"]["rows"][0], json!([1]));
        assert_eq!(events[2]["event"]["totalRows"], 1000);
        assert_eq!(events[2]["event"]["totalPages"], 10);
        let page = json!({"method": "db", "params": {"method": "page", "params": {
            "connectionId": id, "streamId": "p1", "source": events[0]["event"]["source"],
            "page": 10, "pageSize": 100,
        }}})
        .to_string();
        let events = stream(&core, &ws, &page);
        assert_eq!(
            event_types(&events),
            ["statementStart", "batch", "statementDone", "done"]
        );
        assert_eq!(events[1]["event"]["rows"][99], json!([1000]));

        db_call(
            &core,
            &ws,
            "execute",
            json!({"connectionId": id, "sql": "CREATE TABLE p (id INTEGER PRIMARY KEY, name VARCHAR)"}),
        )
        .unwrap();
        let target = json!({"schema": "main", "table": "p"});
        let outcome = db_call(
            &core,
            &ws,
            "applyChanges",
            json!({"connectionId": id, "changes": [
                {"type": "edit", "id": "a", "edit": {"type": "insertRow", "target": target, "values": [["id", 1], ["name", "x"]]}},
                {"type": "edit", "id": "b", "edit": {"type": "insertRow", "target": target, "values": [["id", 2], ["name", "y"]]}},
                {"type": "edit", "id": "c", "edit": {"type": "updateCell", "target": target, "key": [["id", 1]], "column": "name", "value": "z"}},
            ]}),
        )
        .unwrap();
        assert_eq!(outcome["mode"], "atomic", "{outcome}");
        assert_eq!(outcome["applied"], 3, "{outcome}");

        let body = json!({"method": "db", "params": {"method": "tablePage", "params": {
            "connectionId": id, "streamId": "tp", "page": 1, "pageSize": 10,
            "query": {"target": target,
                      "filters": [{"column": "name", "op": "=", "value": "z"}]},
        }}})
        .to_string();
        let events = stream(&core, &ws, &body);
        assert!(events
            .iter()
            .all(|e| e["type"] == "run" && e["streamId"] == "tp"));
        assert_eq!(
            event_types(&events),
            ["statementStart", "batch", "statementDone", "done"],
            "{events:?}"
        );
        assert_eq!(events[1]["event"]["rows"], json!([[1, "z"]]));

        db_call(&core, &ws, "disconnect", json!({"connectionId": id})).unwrap();
    }
}

/// The assistant over the desktop's transports: `ai.chat`
/// through `core_stream`, a turn stopped by a reload or a gone channel
/// still storing its reply, and the `secret` group refusing AI keys. Model
/// calls go to a local mock through a loopback-only client.
#[cfg(test)]
mod ai_tests {
    use super::*;
    use seaquel_ai::testing::scripts::{openai_chunk, openai_text};
    use seaquel_ai::testing::{LoopbackOnly, MockProvider, Reply, SseEnd};
    use seaquel_core::ai::native::{Egress, NativeHttp, NativeHttpOptions};
    use seaquel_core::secrets::MemoryStore;
    use serde_json::{json, Value as Json};
    use std::sync::mpsc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(30);

    /// The desktop's Core with SQLite only, calling models through a
    /// client that refuses any host but loopback.
    fn ai_core() -> Core {
        seaquel_core::with_plugins(|id| id == "sqlite")
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(seaquel_runtime::TokioExecutor))
            .ai_http(Arc::new(LoopbackOnly(NativeHttp::new(
                NativeHttpOptions::new(Egress::Any),
            ))))
            .ai_egress(seaquel_core::ai::AiEgress::Any)
            .build()
    }

    fn group(
        core: &Core,
        ws: &DesktopWorkspace,
        group: &str,
        method: &str,
        params: Json,
    ) -> Result<Json, RpcError> {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let body = json!({"method": group, "params": inner});
        let res = tauri::async_runtime::block_on(handle_core_call(
            core,
            ws,
            "main",
            &InvokeBody::Raw(body.to_string().into_bytes()),
        ))?;
        let res = serde_json::to_value(res).unwrap();
        assert_eq!(res["result"]["method"], method, "{res}");
        Ok(res["result"]["result"].clone())
    }

    struct World {
        core: Arc<Core>,
        ws: Arc<DesktopWorkspace>,
        mock: MockProvider,
        chat: String,
        connection: String,
        _tmp: tempfile::TempDir,
    }

    /// A provider at the mock (OpenAI-compatible, keyless), a saved SQLite
    /// connection on it, a chat, and the connection open.
    fn world() -> World {
        let mock = tauri::async_runtime::block_on(MockProvider::start());
        let core = Arc::new(ai_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(DesktopWorkspace::new(
            Ok(tmp.path().join("data")),
            Arc::new(MemoryStore::new()),
        ));
        group(&core, &ws, "library", "projectEnsureDefault", Json::Null).unwrap();
        let provider = group(
            &core,
            &ws,
            "settings",
            "aiProviderCreate",
            json!({"provider": {"name": "Mock", "type": "openai-compatible",
                "baseUrl": format!("{}/v1", mock.url())}}),
        )
        .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let file = tmp.path().join("app.db");
        let saved = group(
            &core,
            &ws,
            "library",
            "connectionCreate",
            json!({"connection": {"projectId": "default-seaquel", "name": "Lite",
                "type": "sqlite", "host": "", "port": 0, "username": "",
                "databaseName": file.display().to_string(),
                "activeAIProviderId": provider, "activeAIModel": "model-1"}}),
        )
        .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let chat = group(
            &core,
            &ws,
            "library",
            "chatCreate",
            json!({"chat": {"connectionId": saved, "title": "T"}}),
        )
        .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let connection = group(
            &core,
            &ws,
            "db",
            "connect",
            json!({"target": {"type": "saved", "id": saved}, "createIfMissing": true}),
        )
        .unwrap()["connectionId"]
            .as_str()
            .unwrap()
            .to_string();
        World {
            core,
            ws,
            mock,
            chat,
            connection,
            _tmp: tmp,
        }
    }

    impl World {
        fn chat_body(&self, stream: &str) -> String {
            json!({"method": "ai", "params": {"method": "chat", "params": {
                "streamId": stream, "chatId": self.chat, "connectionId": self.connection,
                "userMessage": {"id": format!("{stream}-u"), "content": "Count the rows."},
                "assistantMessageId": format!("{stream}-a")}}})
            .to_string()
        }

        fn stored(&self) -> Vec<Json> {
            group(
                &self.core,
                &self.ws,
                "library",
                "chatMessagesList",
                json!({"chatId": self.chat}),
            )
            .unwrap()["value"]["messages"]
                .as_array()
                .unwrap()
                .clone()
        }

        /// Some text, then the provider holds the stream open.
        fn stall_after_text(&self) {
            self.mock.reply(Reply::Sse {
                events: vec![openai_chunk(
                    json!({"choices":[{"index":0,"delta":{"content":"Partial answer"}}]}),
                )],
                piece: 64,
                gap: Duration::ZERO,
                end: SseEnd::Stall,
            });
        }

        /// The turn runs in the background, as `core_stream` runs it.
        fn background(
            &self,
            label: &'static str,
            body: String,
        ) -> (
            mpsc::Receiver<Json>,
            tauri::async_runtime::JoinHandle<Result<u64, RpcError>>,
        ) {
            let (tx, rx) = mpsc::channel();
            let (core, ws) = (self.core.clone(), self.ws.clone());
            let task = tauri::async_runtime::spawn(async move {
                run_core_stream(&core, &ws, label, body.as_bytes(), |event| {
                    tx.send(serde_json::to_value(event).unwrap()).is_ok()
                })
                .await
            });
            (rx, task)
        }
    }

    #[test]
    fn core_stream_serves_a_turn_and_counts_its_events() {
        let w = world();
        w.mock.reply(Reply::sse(openai_text("Seven rows.")));
        let mut events = Vec::new();
        let sent = tauri::async_runtime::block_on(run_core_stream(
            &w.core,
            &w.ws,
            "main",
            w.chat_body("t1").as_bytes(),
            |event| {
                events.push(serde_json::to_value(event).unwrap());
                true
            },
        ))
        .unwrap();
        assert_eq!(sent, events.len() as u64);
        assert!(events
            .iter()
            .all(|e| e["type"] == "ai" && e["streamId"] == "t1"));
        assert_eq!(events[0]["event"]["type"], "started");
        let done = &events.last().unwrap()["event"];
        assert_eq!(done["type"], "done", "{done}");
        assert_eq!(done["messages"][1]["content"], "Seven rows.");
        assert!(Webviews::lock(&w.ws.webviews).streams.is_empty());
        assert_eq!(w.stored().len(), 2);
    }

    /// Task 4's contract: a reload cancels the webview's turn through Core
    /// and the stream is polled to its end, so the reply keeps what
    /// streamed. Nothing follows the cancel.
    #[test]
    fn a_reload_cancels_a_turn_and_its_reply_is_stored() {
        let w = world();
        tauri::async_runtime::block_on(w.ws.set_event_sink(&w.core, "main", Box::new(|_| true)));
        w.stall_after_text();
        let (rx, task) = w.background("main", w.chat_body("t2"));
        loop {
            let event = rx.recv_timeout(WAIT).unwrap();
            if event["event"]["type"] == "text" {
                break;
            }
        }
        assert!(Webviews::lock(&w.ws.webviews).streams["main"].contains("t2"));
        tauri::async_runtime::block_on(w.ws.set_event_sink(&w.core, "main", Box::new(|_| true)));
        let sent = tauri::async_runtime::block_on(task).unwrap().unwrap();
        while let Ok(event) = rx.try_recv() {
            assert!(!event["event"]["type"].as_str().unwrap().ends_with("done"));
            assert_ne!(event["event"]["type"], "error", "{event}");
        }
        assert!(sent >= 2);
        let stored = w.stored();
        assert_eq!(stored.len(), 2, "{stored:?}");
        assert_eq!(stored[1]["content"], "Partial answer");
        assert!(Webviews::lock(&w.ws.webviews).streams.is_empty());
        // The turn is gone: answering it is NOT_FOUND, at once.
        let err = group(
            &w.core,
            &w.ws,
            "ai",
            "respond",
            json!({"streamId": "t2", "callId": "c", "decision": "allow"}),
        )
        .unwrap_err();
        assert_eq!(err.code, "NOT_FOUND");
    }

    /// A channel the webview dropped (its window closed under the turn)
    /// cancels the turn rather than dropping it: the reply is stored.
    #[test]
    fn a_gone_channel_cancels_a_turn_and_its_reply_is_stored() {
        let w = world();
        w.stall_after_text();
        let mut seen = Vec::new();
        let sent = tauri::async_runtime::block_on(tokio_timeout(run_core_stream(
            &w.core,
            &w.ws,
            "main",
            w.chat_body("t3").as_bytes(),
            |event| {
                let event = serde_json::to_value(event).unwrap();
                let text = event["event"]["type"] == "text";
                seen.push(event);
                // Gone once the first text arrived.
                !text
            },
        )))
        .unwrap();
        assert_eq!(sent, 1, "only `started` was taken: {seen:?}");
        let stored = w.stored();
        assert_eq!(stored.len(), 2, "{stored:?}");
        assert_eq!(stored[1]["content"], "Partial answer");
        assert_eq!(
            tauri::async_runtime::block_on(w.ws.db(&w.core))
                .unwrap()
                .ws
                .ai_turn_count(),
            0
        );
    }

    /// The test's own bound on a future, so a turn that never ends fails
    /// the test instead of hanging it.
    async fn tokio_timeout<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(WAIT, fut)
            .await
            .expect("the stream didn't end")
    }

    /// Core reads AI keys itself, so the webview can't read,
    /// set or delete one through the `secret` group; other keys work.
    #[test]
    fn the_secret_group_refuses_ai_keys() {
        let core = ai_core();
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::new());
        tauri::async_runtime::block_on(store.set("ai-api-key:p1", "test-key-not-real")).unwrap();
        let ws = DesktopWorkspace::new(Ok(tmp.path().join("data")), store.clone());
        for (method, params) in [
            ("get", json!({"key": "ai-api-key:p1"})),
            ("set", json!({"key": "ai-api-key:p1", "value": "other"})),
            ("delete", json!({"key": "ai-api-key:p1"})),
        ] {
            let err = group(&core, &ws, "secret", method, params).unwrap_err();
            assert_eq!(err.code, "INVALID_ARGUMENT", "{method}");
            assert!(!err.message.contains("test-key-not-real"));
        }
        assert_eq!(
            tauri::async_runtime::block_on(store.get("ai-api-key:p1"))
                .unwrap()
                .as_deref(),
            Some("test-key-not-real")
        );
        group(
            &core,
            &ws,
            "secret",
            "set",
            json!({"key": "db:c1", "value": "pw"}),
        )
        .unwrap();
        assert_eq!(
            group(&core, &ws, "secret", "get", json!({"key": "db:c1"})).unwrap(),
            "pw"
        );
        group(&core, &ws, "secret", "delete", json!({"key": "db:c1"})).unwrap();
    }
}
