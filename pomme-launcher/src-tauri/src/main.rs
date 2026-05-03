#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "0") };
    }

    let builder = pomme_launcher::get_builder();

    #[cfg(debug_assertions)]
    pomme_launcher::generate_bindings();

    let invoke_handler = builder.invoke_handler();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            builder.mount_events(app);
            pomme_launcher::storage::ensure_dirs();
            app.manage(pomme_launcher::AppState::default());
            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(invoke_handler)
        .run(tauri::generate_context!())
        .expect("failed to run Pomme launcher");
}
