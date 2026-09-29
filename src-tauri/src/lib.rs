use arboard::Clipboard;
use futures::stream::{BoxStream, StreamExt};
use image::ImageReader;
use log::{debug, error, info};
use seaquel_core::git::Git;
use seaquel_core::license::desktop::DesktopClient;
use seaquel_core::secrets::{KeychainStore, SecretStore};
use seaquel_core::storage::{LEGACY_STORAGE, STORAGE_CORRUPT};
use seaquel_core::{ConnectPolicy, Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_rpc::{ConnectTargetParams, CoreEvent, DbRequest, Request, Response, RpcError};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
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
mod logging;

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

/// The license server: `LICENSE_API_URL` at compile time, else the dev
/// server in debug builds and seaquel.app in release builds.
fn license_base_url() -> &'static str {
    option_env!("LICENSE_API_URL").unwrap_or(if cfg!(debug_assertions) {
        "http://localhost:5173"
    } else {
        "https://seaquel.app"
    })
}

struct PendingUpdate {
    bytes: Mutex<Option<Vec<u8>>>,
}

/// The desktop's workspace. Storage opens on the first `core_call` that
/// needs it, not at startup:
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
    /// Where [`CoreEvent::ConnectionClosed`] events go: every live sink gets
    /// every event.
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
    /// Picking it subscribes to its events once, for `core_events`.
    pub(crate) async fn db(&self, core: &Core) -> Result<&DbWorkspace, RpcError> {
        self.db
            .get_or_try_init(|| async {
                let db = match self.workspace(core).await {
                    Ok(ws) => DbWorkspace {
                        ws,
                        storage_error: None,
                        _scratch: None,
                    },
                    Err(e) if self.storage_failed_for_good() => Self::stand_in(core, e).await?,
                    Err(e) => return Err(e),
                };
                tauri::async_runtime::spawn(pump_events(
                    seaquel_rpc::workspace_events(&db.ws),
                    self.webviews.clone(),
                ));
                Ok(db)
            })
            .await
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

    /// Send [`CoreEvent::ConnectionClosed`] events to `sink` for webview
    /// `label` from now on. Every webview's sink gets every event.
    ///
    /// A second call for the same label is a reload: its sink replaces the
    /// old one (whose channel may never report that it's gone), and the
    /// label's running streams, which the reloaded page can no longer read,
    /// are cancelled.
    fn set_event_sink(&self, core: &Core, label: &str, sink: EventSink) {
        let stale = {
            let mut webviews = Webviews::lock(&self.webviews);
            match webviews.sinks.insert(label.to_string(), sink) {
                Some(_) => webviews.take_streams(label),
                None => HashSet::new(),
            }
        };
        self.cancel_streams(core, label, stale);
    }

    /// Webview `label` is gone: drop its sink and cancel its streams.
    fn forget_webview(&self, core: &Core, label: &str) {
        let stale = {
            let mut webviews = Webviews::lock(&self.webviews);
            webviews.sinks.remove(label);
            webviews.take_streams(label)
        };
        self.cancel_streams(core, label, stale);
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
                let spec = WorkspaceSpec::new(data_dir).with_secrets(self.secrets.clone());
                match core.open_workspace(spec).await {
                    Ok(ws) => {
                        info!(activity = "workspace.open", data_dir = data_dir.display().to_string().as_str(); "Workspace open");
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

    async fn call(&self, core: &Core, req: Request) -> Result<Response, RpcError> {
        match req {
            Request::Secret(r) => seaquel_rpc::dispatch_secret(Some(&*self.secrets), r)
                .await
                .map(Response::Secret),
            Request::License(r) => seaquel_rpc::dispatch_license(&self.license, r)
                .await
                .map(Response::License),
            Request::Git(r) => seaquel_rpc::dispatch_git(&self.git, r)
                .await
                .map(Response::Git),
            // Core owns the tunnels; they need no storage.
            Request::Ssh(r) => seaquel_rpc::dispatch_ssh(core, r).await.map(Response::Ssh),
            Request::Db(r) => {
                let db = self.db(core).await?;
                if let Some(e) = &db.storage_error {
                    if reads_saved_row(&r) {
                        return Err(e.clone());
                    }
                }
                seaquel_rpc::dispatch_workspace(core, &db.ws, Request::Db(r)).await
            }
            req => {
                let ws = self.workspace(core).await?;
                seaquel_rpc::dispatch_workspace(core, &ws, req).await
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
#[tauri::command]
async fn core_call(
    request: tauri::ipc::Request<'_>,
    core: State<'_, Core>,
    workspace: State<'_, DesktopWorkspace>,
) -> Result<Response, RpcError> {
    handle_core_call(&core, &workspace, request.body()).await
}

async fn handle_core_call(
    core: &Core,
    workspace: &DesktopWorkspace,
    body: &InvokeBody,
) -> Result<Response, RpcError> {
    let req = seaquel_rpc::parse_request(&core_call_body(body)?)?;
    workspace.call(core, req).await
}

/// A `db.queryStream`, `db.run`, `db.page` or `db.tablePage` request, its
/// events pushed to `channel` as [`CoreEvent::Stream`]s (a query stream's
/// batches) or [`CoreEvent::Run`]s (a run's, page's or table page's
/// statements and batches), then one
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
/// `core_events` from the same label) or closing it cancels the stream.
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
    // A query stream's, a run's, a page's or a table page's id.
    let stream_id = match &req {
        Request::Db(db) => db.stream_id().map(str::to_string),
        _ => None,
    };
    let db = workspace.db(core).await?;
    let mut events = seaquel_rpc::dispatch_stream(core, &db.ws, req)?;
    // Tracked once registered, so a reload's cancel always finds it.
    let _tracking = stream_id.map(|stream_id| {
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
            break;
        }
        sent += 1;
    }
    Ok(sent)
}

/// Push the workspace's [`CoreEvent::ConnectionClosed`] events to `channel`.
/// Each webview calls it once when its page loads; every webview gets every
/// event. A second call from the same webview is a reload: the new channel
/// replaces the old one and the webview's running streams are cancelled
/// ([`DesktopWorkspace::set_event_sink`]). Closing the window drops both.
#[tauri::command]
fn core_events(
    channel: Channel<CoreEvent>,
    webview: tauri::Webview,
    core: State<'_, Core>,
    workspace: State<'_, DesktopWorkspace>,
) {
    workspace.set_event_sink(
        &core,
        webview.label(),
        Box::new(move |event| channel.send(event).is_ok()),
    );
}

#[tauri::command]
fn read_dbeaver_config() -> Result<Option<String>, CommandError> {
    let home = dirs::home_dir().ok_or(CommandError {
        message: "Could not find home directory".to_string(),
        code: "HOME_DIR_ERROR".to_string(),
    })?;

    #[cfg(target_os = "macos")]
    let config_path =
        home.join("Library/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    #[cfg(target_os = "windows")]
    let config_path =
        home.join("AppData/Roaming/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    #[cfg(target_os = "linux")]
    let config_path =
        home.join(".local/share/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    if config_path.exists() {
        let content = fs::read_to_string(&config_path).map_err(|e| CommandError {
            message: format!("Failed to read DBeaver config: {}", e),
            code: "READ_ERROR".to_string(),
        })?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

#[tauri::command]
fn read_tableplus_config() -> Result<Option<String>, CommandError> {
    #[cfg(not(target_os = "macos"))]
    return Ok(None);

    // TablePlus only runs on macOS
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or(CommandError {
            message: "Could not find home directory".to_string(),
            code: "HOME_DIR_ERROR".to_string(),
        })?;

        let config_path =
            home.join("Library/Application Support/com.tinyapp.TablePlus/Data/Connections.plist");
        if config_path.exists() {
            let value: plist::Value = plist::from_file(&config_path).map_err(|e| CommandError {
                message: format!("Failed to parse TablePlus plist: {}", e),
                code: "PARSE_ERROR".to_string(),
            })?;
            let json = serde_json::to_string(&value).map_err(|e| CommandError {
                message: format!("Failed to serialize plist to JSON: {}", e),
                code: "SERIALIZE_ERROR".to_string(),
            })?;
            Ok(Some(json))
        } else {
            Ok(None)
        }
    }
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

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<(), CommandError> {
    info!(activity = "app.update"; "Installing update");
    let bytes = pending.bytes.lock().map_err(|e| {
        error!(activity = "app.update", error_code = "LOCK_ERROR"; "Failed to lock update state");
        CommandError {
            message: format!("Failed to lock update state: {}", e),
            code: "LOCK_ERROR".to_string(),
        }
    })?.take();
    if let Some(bytes) = bytes {
        // Re-check for update to get the Update object needed for install
        if let Some(update) = app.updater().map_err(|e| {
            error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to get updater");
            CommandError {
                message: format!("Failed to get updater: {}", e),
                code: "UPDATE_ERROR".to_string(),
            }
        })?.check().await.map_err(|e| {
            error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to check for update");
            CommandError {
                message: format!("Failed to check for update: {}", e),
                code: "UPDATE_ERROR".to_string(),
            }
        })? {
            update.install(&bytes).map_err(|e| {
                error!(activity = "app.update", error_code = "UPDATE_ERROR"; "Failed to install update");
                CommandError {
                    message: format!("Failed to install update: {}", e),
                    code: "UPDATE_ERROR".to_string(),
                }
            })?;
            info!(activity = "app.update"; "Update installed, restarting");
            app.restart();
        }
    }
    Ok(())
}

#[tauri::command]
async fn check_for_update_command(
    app: tauri::AppHandle,
) -> Result<Option<UpdateInfo>, CommandError> {
    debug!(activity = "app.update"; "Checking for updates");
    let update = app
        .updater()
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
        .max_file_size(5_000_000)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll);
    // Workspace calls log their method names at debug; dev builds show them.
    #[cfg(debug_assertions)]
    let logger = logger.level_for("seaquel_rpc", log::LevelFilter::Debug);
    tauri::Builder::default()
        .plugin(logger.build())
        .plugin(tauri_plugin_os::init())
        // Every engine, file and tunnel: the desktop connects wherever its
        // user asks, so no config check.
        .manage(
            seaquel_core::with_default_plugins()
                .connect_policy(ConnectPolicy::Unrestricted)
                .executor(std::sync::Arc::new(seaquel_runtime::TokioExecutor))
                .build(),
        )
        .manage(PendingUpdate {
            bytes: Mutex::new(None),
        })
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
            copy_image_to_clipboard,
            open_path,
            get_data_dir,
            read_log_file,
            clear_log_file,
            get_username,
            install_update,
            check_for_update_command,
            read_dbeaver_config,
            read_tableplus_config,
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Destroyed => {
                let app = window.app_handle();
                if let (Some(core), Some(ws)) =
                    (app.try_state::<Core>(), app.try_state::<DesktopWorkspace>())
                {
                    ws.forget_webview(&core, window.label());
                }
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
        .setup(|app| {
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
            // Storage itself opens on the first call that needs it.
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn check_for_update(app: tauri::AppHandle) -> tauri_plugin_updater::Result<()> {
    if let Some(update) = app.updater()?.check().await? {
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

        info!(activity = "app.update"; "Update downloaded, notifying frontend");

        // Store the bytes for later installation
        let pending = app.state::<PendingUpdate>();
        *pending
            .bytes
            .lock()
            .expect("Failed to lock pending update bytes") = Some(bytes);

        let _ = app.emit("update-downloaded", info);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"method":"storage","params":{"method":"onboardingSave","params":{"data":{"z":1e+21,"a":1}}}}"#;

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

    const LOAD: &str = r#"{"method":"storage","params":{"method":"projectsLoadAll"}}"#;
    const SET: &str =
        r#"{"method":"secret","params":{"method":"set","params":{"key":"db:c1","value":"pw"}}}"#;
    const GET: &str = r#"{"method":"secret","params":{"method":"get","params":{"key":"db:c1"}}}"#;

    fn call(core: &Core, ws: &DesktopWorkspace, json: &str) -> Result<serde_json::Value, RpcError> {
        tauri::async_runtime::block_on(handle_core_call(core, ws, &body(json)))
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
        let res = db_call(core, ws, "connect", form(file)).unwrap();
        let id = res["connectionId"].as_str().unwrap().to_string();
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
        let id = sqlite(&core, &ws, &tmp.path().join("rr.db"), 2000);
        ws.set_event_sink(&core, "main", Box::new(|_| true));

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

        ws.set_event_sink(&core, "main", Box::new(|_| true));
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
        let id = sqlite(&core, &ws, &tmp.path().join("tp.db"), 20000);
        // Every row of the view is a scan of a 20,000 × 20,000 join.
        db_call(
            &core,
            &ws,
            "execute",
            json!({"connectionId": id, "sql": "CREATE VIEW slow AS SELECT a.x AS x, b.x AS y FROM t a, t b"}),
        )
        .unwrap();
        ws.set_event_sink(&core, "main", Box::new(|_| true));

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

        ws.set_event_sink(&core, "main", Box::new(|_| true));
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
        ws.set_event_sink(&core, "main", sink(old_tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        ws.set_event_sink(&core, "theme-editor", sink(editor_tx));
        let (tx, rx) = mpsc::channel();
        ws.set_event_sink(&core, "main", sink(tx));

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

    /// A reload (`core_events` again from the same webview) cancels that
    /// webview's streams, whose channels the old page can't read; another
    /// webview's streams go on.
    #[test]
    fn a_reload_cancels_the_webviews_streams() {
        let core = Arc::new(sqlite_core());
        let tmp = tempfile::tempdir().unwrap();
        let ws = Arc::new(desktop(tmp.path().join("data")));
        let id = sqlite(&core, &ws, &tmp.path().join("r.db"), 2000);
        let big = "SELECT a.x, b.x FROM t a, t b";
        ws.set_event_sink(&core, "main", Box::new(|_| true));

        let (main_rx, main_task) =
            background_stream(&core, &ws, "main", stream_body(&id, "m", big));
        let (editor_rx, editor_task) =
            background_stream(&core, &ws, "theme-editor", stream_body(&id, "e", big));
        main_rx.recv_timeout(WAIT).unwrap();
        editor_rx.recv_timeout(WAIT).unwrap();

        ws.set_event_sink(&core, "main", Box::new(|_| true));
        ends_cancelled(&main_rx);
        tauri::async_runtime::block_on(main_task).unwrap().unwrap();

        // The editor's stream still runs, until its window is destroyed.
        let db = tauri::async_runtime::block_on(ws.db(&core)).unwrap();
        assert_eq!(db.ws.stream_count(&core), 1);
        assert!(Webviews::lock(&ws.webviews)
            .streams
            .contains_key("theme-editor"));
        assert!(!Webviews::lock(&ws.webviews).streams.contains_key("main"));
        ws.forget_webview(&core, "theme-editor");
        ends_cancelled(&editor_rx);
        tauri::async_runtime::block_on(editor_task)
            .unwrap()
            .unwrap();
        assert!(Webviews::lock(&ws.webviews).streams.is_empty());
    }

    /// A destroyed window's sink is dropped; the others still get events.
    #[test]
    fn a_destroyed_webview_gets_no_more_events() {
        let core = sqlite_core();
        let tmp = tempfile::tempdir().unwrap();
        let ws = desktop(tmp.path().join("data"));
        let (tx, rx) = mpsc::channel();
        ws.set_event_sink(&core, "main", sink(tx));
        let (editor_tx, editor_rx) = mpsc::channel();
        ws.set_event_sink(&core, "theme-editor", sink(editor_tx));

        ws.forget_webview(&core, "theme-editor");
        assert_eq!(editor_rx.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        // Forgetting a label with nothing registered is fine.
        ws.forget_webview(&core, "never-seen");

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
}
