use arboard::Clipboard;
use image::ImageReader;
use log::{debug, error, info};
use seaquel_core::git::Git;
use seaquel_core::license::desktop::DesktopClient;
use seaquel_core::secrets::{KeychainStore, SecretStore};
use seaquel_core::storage::{LEGACY_STORAGE, STORAGE_CORRUPT};
use seaquel_core::{Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_rpc::{Request, Response, RpcError};
use std::borrow::Cow;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::ipc::InvokeBody;
use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, State};
use tauri_plugin_log::{Target, TargetKind, TimezoneStrategy};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::OnceCell;

mod db;
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
struct DesktopWorkspace {
    /// `seaquel_storage::data_dir(identifier)`, or why there is none.
    data_dir: Result<PathBuf, RpcError>,
    secrets: Arc<dyn SecretStore>,
    /// The license server's activation client ([`license_base_url`]).
    license: DesktopClient,
    /// Shared projects' git, with the user's home for the default SSH keys.
    git: Git,
    /// Set once storage opens, or once it fails for good.
    workspace: OnceCell<Result<Arc<Workspace>, RpcError>>,
}

impl DesktopWorkspace {
    fn new(data_dir: Result<PathBuf, RpcError>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            data_dir,
            secrets,
            license: DesktopClient::new(license_base_url()),
            git: Git::from_env(),
            workspace: OnceCell::new(),
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
            req => {
                let ws = self.workspace(core).await?;
                seaquel_rpc::dispatch_workspace(core, &ws, req).await
            }
        }
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

    // App menu (macOS)
    let app_menu = Submenu::with_items(
        app,
        "Seaquel",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Seaquel"), Some(about_metadata))?,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

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
        .max_file_size(5_000_000)
        .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll);
    // Workspace calls log their method names at debug; dev builds show them.
    #[cfg(debug_assertions)]
    let logger = logger.level_for("seaquel_rpc", log::LevelFilter::Debug);
    tauri::Builder::default()
        .plugin(logger.build())
        .plugin(tauri_plugin_os::init())
        .manage(seaquel_core::with_default_plugins().build())
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
            db::commands::db_connect,
            db::commands::db_query,
            db::commands::db_query_stream,
            db::commands::db_cancel_stream,
            db::commands::db_execute,
            db::commands::db_transaction,
            db::commands::db_disconnect,
            db::commands::db_engine,
            db::commands::db_test,
        ])
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Destroyed => {
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
