mod auth;
mod commands;
mod downloader;
mod installations;
mod ping;
mod settings;
pub mod storage;

use std::collections::VecDeque;
use tokio::sync::Mutex;

#[derive(Default)]
pub struct AppState {
    pub client_logs: Mutex<VecDeque<String>>,
    pub installations_lock: Mutex<()>,
}

/// Maps all supported versions to their protocol version.
/// Snapshots encode as `(1 << 30) | base_protocol`.
/// KEEP IN SYNC WITH src/main.rs
pub const VERSION_PROTOCOL_MAP: [(&str, i32); 3] =
    [("26.1", 775), ("26.1.1-rc-1", 0x40000130), ("26.1.1", 775)];

const TYPED_ERROR_IMPL: &str = r#"export type Result<T, E> =
  | { ok: true;  value: T }
  | { ok: false; error: E };

export const Result = {
  ok<T>(value: T): Result<T, never> {
    return { ok: true, value };
  },
  err<E>(error: E): Result<never, E> {
    return { ok: false, error };
  },
} as const;


export async function typedError<T, E>(promise: Promise<T>): Promise<Result<T, E>> {
  try {
    return Result.ok(await promise);
  } catch (e) {
    if (e instanceof Error) throw e;
    return Result.err(e as E);
  }
}"#;

pub fn get_builder() -> tauri_specta::Builder {
    tauri_specta::Builder::new()
        .commands(tauri_specta::collect_commands![
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
            commands::load_installations,
            commands::create_installation,
            commands::delete_installation,
            commands::duplicate_installation,
            commands::edit_installation,
            commands::get_downloaded_versions,
        ])
        .events(tauri_specta::collect_events![
            commands::ConsoleMessageEvent,
            commands::GameExitedEvent,
            downloader::DownloadProgressEvent,
        ])
        .typed_error_impl(TYPED_ERROR_IMPL)
}

pub fn generate_bindings() {
    crate::get_builder()
        .export(
            specta_typescript::Typescript::default().layout(specta_typescript::Layout::Files),
            "../src/bindings/",
        )
        .expect("tauri-specta failed to write TypeScript bindings");
}
