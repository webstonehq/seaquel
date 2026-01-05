use arboard::Clipboard;
use image::ImageReader;
use std::sync::Mutex;
use tauri::menu::{AboutMetadata, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

mod ssh_tunnel;

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

    // App menu (macOS)
    let app_menu = Submenu::with_items(
        app,
        "Seaquel",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Seaquel"), Some(about_metadata))?,
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
        .manage(PendingUpdate { bytes: Mutex::new(None) })
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            copy_image_to_clipboard,
            install_update,
            ssh_tunnel::create_ssh_tunnel,
            ssh_tunnel::close_ssh_tunnel,
            ssh_tunnel::check_tunnel_status,
            ssh_tunnel::list_active_tunnels,
        ])
        .setup(|app| {
            // Set up custom menu
            let menu = create_menu(app.handle())?;
            app.set_menu(menu)?;

            // Listen for "Close Tab" menu click and emit to frontend
            app.on_menu_event(|app, event| {
                if event.id().as_ref() == "close_tab" {
                    let _ = app.emit("menu-close-tab", ());
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
