mod app;
mod args;
mod assets;
mod audio;
mod benchmark;
mod dirs;
mod discord;
mod entity;
mod lang;
mod logging;
mod net;
mod physics;
mod player;
mod renderer;
mod resource_pack;
mod ui;
mod user;
mod util;
mod world;

use std::sync::Arc;

use clap::Parser;

use crate::app::App;
use crate::user::UserData;

/// Maps all supported versions to their protocol version.
/// Snapshots encode as `(1 << 30) | base_protocol`.
/// KEEP IN SYNC WITH pomme-launcher/src-tauri/src/lib.rs
const VERSION_PROTOCOL_MAP: [(&str, i32); 4] = [
    ("26.2", 776),
    ("26.1", 775),
    ("26.1.1-rc-1", 0x40000130),
    ("26.1.1", 775),
];

fn main() {
    let args = args::LaunchArgs::parse();

    #[cfg(not(debug_assertions))]
    {
        match &args.launch_token {
            Some(path) => {
                let token_path = std::path::Path::new(path);
                if !token_path.exists() {
                    eprintln!("Please use the Pomme Launcher to start the game.");
                    std::process::exit(1);
                }
                let _ = std::fs::remove_file(token_path);
            }
            None => {
                eprintln!("Please use the Pomme Launcher to start the game.");
                eprintln!("Download it at: https://github.com/PommeMC/Pomme-Client");
                std::process::exit(1);
            }
        }
    }

    let version = args
        .version
        .as_deref()
        .unwrap_or_else(|| VERSION_PROTOCOL_MAP.first().unwrap().0);

    if !VERSION_PROTOCOL_MAP.iter().any(|(v, _)| v == &version) {
        eprintln!(
            "{version} is not currently supported. Supported versions: {}",
            VERSION_PROTOCOL_MAP
                .iter()
                .map(|(v, _)| *v)
                .collect::<Vec<_>>()
                .join(", ")
        );
        #[cfg(not(debug_assertions))]
        std::process::exit(1);
    }

    let data_dirs = dirs::DataDirs::resolve(
        version,
        args.assets_dir.as_deref(),
        args.versions_dir.as_deref(),
        args.game_dir.as_deref(),
    );

    let log_dir = data_dirs.game_dir.join("logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    if let Err(e) = logging::rotate(&log_dir) {
        eprintln!("Failed to rotate logs: {e}. latest.log will probably be overwritten.");
    }
    let _guard = logging::init(&log_dir);

    if let Err(e) = data_dirs.verify() {
        eprintln!("Failed to verify directories: {e}");
        std::process::exit(1);
    }
    data_dirs.ensure_game_dir().ok();
    tracing::info!("Installation directory: {}", data_dirs.game_dir.display());

    let rt = Arc::new(tokio::runtime::Runtime::new().expect("Failed to create tokio runtime"));

    let user = UserData::from_args(args.username, args.uuid, args.access_token);

    let presence = crate::discord::DiscordPresence::start(version)
        .inspect_err(|e| tracing::warn!("Discord rich presence unavailable: {e}"))
        .ok();

    if let Err(e) = App::new(
        version.to_owned(),
        data_dirs,
        rt,
        presence,
        user,
        args.quick_access_multiplayer,
    )
    .run()
    {
        tracing::error!("Fatal: {e}");
        std::process::exit(1);
    }
}
