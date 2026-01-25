use arboard::Clipboard;
use image::ImageReader;
use std::fs;
use std::sync::Mutex;
use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

mod duckdb_commands;
mod git;
mod mssql;
mod ssh_tunnel;

use duckdb_commands::DuckDBState;
use mssql::MssqlConnectionManager;
use ssh_tunnel::TunnelManager;

struct PendingUpdate {
    bytes: Mutex<Option<Vec<u8>>>,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust! Yep", name)
}

#[tauri::command]
fn read_dbeaver_config() -> Result<Option<String>, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;

    #[cfg(target_os = "macos")]
    let config_path = home.join("Library/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    #[cfg(target_os = "windows")]
    let config_path = home.join("AppData/Roaming/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    #[cfg(target_os = "linux")]
    let config_path = home.join(".local/share/DBeaverData/workspace6/General/.dbeaver/data-sources.json");

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read DBeaver config: {}", e))?;
        Ok(Some(content))
    } else {
        Ok(None)
    }
}

#[tauri::command]
fn copy_image_to_clipboard(path: String) -> Result<(), String> {
    let img = ImageReader::open(&path)
        .map_err(|e| format!("Failed to open image: {}", e))?
        .decode()
        .map_err(|e| format!("Failed to decode image: {}", e))?;

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    let img_data = arboard::ImageData {
        width: width as usize,
        height: height as usize,
        bytes: rgba.into_raw().into(),
    };

    let mut clipboard = Clipboard::new()
        .map_err(|e| format!("Failed to access clipboard: {}", e))?;

    clipboard.set_image(img_data)
        .map_err(|e| format!("Failed to copy image: {}", e))?;

    Ok(())
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    opener::open(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_data_dir(app: tauri::AppHandle) -> Result<String, String> {
    if let Ok(custom_dir) = std::env::var("SEAQUEL_DATA_DIR") {
        let path = std::path::PathBuf::from(&custom_dir);
        if !path.exists() {
            std::fs::create_dir_all(&path)
                .map_err(|e| format!("Failed to create data dir: {}", e))?;
        }
        Ok(custom_dir)
    } else {
        app.path()
            .app_data_dir()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
async fn install_update(
    app: tauri::AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<(), String> {
    let bytes = pending.bytes.lock().unwrap().take();
    if let Some(bytes) = bytes {
        // Re-check for update to get the Update object needed for install
        if let Some(update) = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())? {
            update.install(&bytes).map_err(|e| e.to_string())?;
            app.restart();
        }
    }
    Ok(())
}

fn create_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    // Load and decode the app icon for the About dialog
    let icon = {
        let icon_bytes = include_bytes!("../icons/128x128@2x.png");
        image::load_from_memory(icon_bytes)
            .ok()
            .map(|img| {
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

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &close_tab,
        ],
    )?;

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
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .manage(TunnelManager::new())
        .manage(MssqlConnectionManager::new())
        .manage(DuckDBState::default())
        .manage(PendingUpdate { bytes: Mutex::new(None) })
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_keyring::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            copy_image_to_clipboard,
            open_path,
            get_data_dir,
            install_update,
            read_dbeaver_config,
            ssh_tunnel::create_ssh_tunnel,
            ssh_tunnel::close_ssh_tunnel,
            ssh_tunnel::check_tunnel_status,
            ssh_tunnel::list_active_tunnels,
            mssql::mssql_connect,
            mssql::mssql_disconnect,
            mssql::mssql_query,
            mssql::mssql_execute,
            duckdb_commands::duckdb_connect,
            duckdb_commands::duckdb_disconnect,
            duckdb_commands::duckdb_query,
            duckdb_commands::duckdb_execute,
            duckdb_commands::duckdb_test,
            git::git_clone_repo,
            git::git_init_repo,
            git::git_pull_repo,
            git::git_push_repo,
            git::git_get_repo_status,
            git::git_commit_changes,
            git::git_stage_file,
            git::git_discard_file,
            git::git_resolve_conflict,
            git::git_get_conflict_content,
            git::git_set_remote,
            git::git_get_remote_url,
        ])
        .setup(|app| {
            // Set up custom menu
            let menu = create_menu(app.handle())?;
            app.set_menu(menu)?;

            // Listen for menu clicks and emit to frontend
            app.on_menu_event(|app, event| {
                if event.id().as_ref() == "close_tab" {
                    let _ = app.emit("menu-close-tab", ());
                } else if event.id().as_ref() == "settings" {
                    let _ = app.emit("menu-settings", ());
                }
            });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = check_for_update(handle).await;
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn check_for_update(app: tauri::AppHandle) -> tauri_plugin_updater::Result<()> {
    if let Some(update) = app.updater()?.check().await? {
        let mut downloaded = 0;
        let version = update.version.clone();

        let bytes = update
            .download(
                |chunk_length, content_length| {
                    downloaded += chunk_length;
                    println!("downloaded {downloaded} from {content_length:?}");
                },
                || {
                    println!("download finished");
                },
            )
            .await?;

        println!("update downloaded, notifying frontend");

        // Store the bytes for later installation
        let pending = app.state::<PendingUpdate>();
        *pending.bytes.lock().unwrap() = Some(bytes);

        let _ = app.emit("update-downloaded", version);
    }

    Ok(())
}
