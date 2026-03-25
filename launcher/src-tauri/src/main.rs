#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;
use tokio::sync::Mutex;

mod auth;
mod commands;
mod downloader;
mod ping;
mod settings;
mod storage;

use std::collections::VecDeque;

#[derive(Default)]
struct AppState {
    client_logs: VecDeque<String>,
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "0") };
    }

    tauri::Builder::default()
        .setup(|app| {
            storage::ensure_dirs();
            app.manage(Mutex::new(AppState::default()));
            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_all_accounts,
            commands::add_account,
            commands::remove_account,
            commands::ensure_assets,
            commands::get_versions,
            commands::refresh_account,
            commands::get_skin_url,
            commands::get_patch_notes,
            commands::get_patch_content,
            commands::launch_game,
            commands::get_client_logs,
            commands::load_launcher_settings,
            commands::set_launcher_language,
            commands::set_keep_launcher_open,
            commands::set_launch_with_console,
            commands::ping_server,
            commands::load_servers,
            commands::save_servers,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run POMC launcher");
}
