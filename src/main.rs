mod args;
mod assets;
mod benchmark;
mod dirs;
mod discord;
mod entity;
mod logging;
mod net;
mod physics;
mod player;
mod renderer;
mod resource_pack;
mod ui;
mod window;
mod world;

use clap::Parser;
use net::connection::ConnectArgs;
use std::sync::Arc;

const SUPPORTED_VERSIONS: [&str; 3] = ["26.1", "26.1.1-rc-1", "26.1.1"];

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
        .unwrap_or_else(|| SUPPORTED_VERSIONS.last().unwrap());

    if !SUPPORTED_VERSIONS.contains(&version) {
        eprintln!(
            "{version} is not currently supported. Supported versions: {:#?}",
            SUPPORTED_VERSIONS
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

    let connection = if let Some(ref server) = args.quick_access_server {
        let connect_args = ConnectArgs {
            server: server.clone(),
            username: args.username.clone().unwrap_or_else(|| "Steve".into()),
            uuid: args
                .uuid
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(uuid::Uuid::nil),
            access_token: args.access_token.clone(),
            view_distance: 12,
        };

        Some(net::connection::spawn_connection(&rt, connect_args))
    } else {
        None
    };

    let launch_auth = match (&args.username, &args.uuid, &args.access_token) {
        (Some(username), Some(uuid_str), Some(token)) => {
            uuid_str.parse().ok().map(|uuid| window::LaunchAuth {
                username: username.clone(),
                uuid,
                access_token: token.clone(),
            })
        }
        _ => None,
    };

    let presence = crate::discord::DiscordPresence::start(version)
        .inspect_err(|e| tracing::warn!("Discord rich presence unavailable: {e}"))
        .ok();

    if let Err(e) = window::run(
        connection,
        version.to_owned(),
        data_dirs,
        rt,
        launch_auth,
        presence,
    ) {
        tracing::error!("Fatal: {e}");
        std::process::exit(1);
    }
}
