// Per-thread-heap allocator (see Cargo.toml): keeps the chunk-mesh worker
// pool's cross-thread Vec churn from serializing on the system heap's global
// lock and stalling the main thread.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
mod particle;
mod physics;
mod player;
mod renderer;
mod resource_pack;
mod ui;
mod user;
mod util;
mod version;
mod world;

use std::sync::Arc;

use clap::Parser;

use crate::app::App;
use crate::user::UserData;
use crate::version::VERSION_PROTOCOL_MAP;

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
    if let Some(protocol) = version::protocol_for(version) {
        version::set_selected_protocol(protocol);
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

    // Block-state tables must be loaded before any world/render code runs.
    world::block::init(version);

    if let Err(e) = data_dirs.verify() {
        eprintln!("Failed to verify directories: {e}");
        std::process::exit(1);
    }
    data_dirs.ensure_game_dir().ok();
    tracing::info!("Installation directory: {}", data_dirs.game_dir.display());

    // A single connection needs only a few async workers; the default runtime
    // spawns one per core and floods them decoding the chunk-load burst, starving
    // the render/mesh threads. Cap it so those cores stay free.
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime"),
    );

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
