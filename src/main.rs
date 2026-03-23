mod args;
mod assets;
mod data;
mod net;
mod physics;
mod player;
mod renderer;
mod ui;
mod window;
mod world;

use std::sync::Arc;

use clap::Parser;

use net::connection::ConnectArgs;

fn main() {
    env_logger::init();

    let args = args::LaunchArgs::parse();

    if !cfg!(debug_assertions) && !args.dev {
        match &args.launch_token {
            Some(path) => {
                let token_path = std::path::Path::new(path);
                if !token_path.exists() {
                    eprintln!("Please use the POMC Launcher to start the game.");
                    std::process::exit(1);
                }
                let _ = std::fs::remove_file(token_path);
            }
            None => {
                eprintln!("Please use the POMC Launcher to start the game.");
                eprintln!("Download it at: https://github.com/Purdze/POMC");
                std::process::exit(1);
            }
        }
    }

    let data = data::DataDir::resolve(args.game_dir.as_deref(), args.assets_dir.as_deref());

    if let Err(e) = data.ensure_dirs() {
        log::error!("Failed to create data directories: {e}");
        std::process::exit(1);
    }

    log::info!("Data directory: {}", data.root.display());

    let rt = Arc::new(tokio::runtime::Runtime::new().expect("failed to create tokio runtime"));

    let connection = if let Some(ref server) = args.server {
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

    if let Err(e) = window::run(
        connection,
        data.assets_dir.clone(),
        data.instance_dir.clone(),
        rt,
        launch_auth,
    ) {
        log::error!("Fatal: {e}");
        std::process::exit(1);
    }
}
