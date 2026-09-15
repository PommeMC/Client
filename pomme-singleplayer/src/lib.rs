//! Runs SteelMC inside the client's own process, so singleplayer joins a real
//! server over an in-memory pipe rather than a socket.
//!
//! The caller owns the pipe. It hands over the server's two halves and keeps
//! its own, then drives [`PendingWorld::poll`] each frame until the world is
//! up.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::{self, JoinHandle};

use steel_core::command::CommandRegistry;
use steel_core::config::{
    DomainConfig, RuntimeConfig, StorageSelection, WorldEntryConfig, WorldsConfig,
};
use steel_core::permission::{PermissionGroupManager, PermissionGroupsConfig};
use steel_core::server::Server;
use steel_login::{JavaTcpClient, ServerConnectionSession};
use steel_utils::Identifier;
use steel_utils::threading::{available_worker_threads, worker_threads_for_available};
pub use steel_utils::types::{Difficulty, GameType};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// What the server reports as the player's address. Nothing dials it.
const CLIENT_ADDRESS: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

/// The server's ends of the byte stream the client speaks Java Edition over.
pub struct ServerTransport {
    pub read: Box<dyn AsyncRead + Send + Unpin>,
    pub write: Box<dyn AsyncWrite + Send + Unpin>,
}

/// Everything the integrated server needs to open one world.
pub struct LaunchOptions {
    /// Absolute path of the world folder.
    pub save_path: PathBuf,
    /// The seed the generator uses, already parsed from what the player typed.
    pub seed: i64,
    /// Chunk radius the server streams, from the client's own video settings.
    pub view_distance: u8,
    /// Radius the server ticks entities and blocks in.
    pub simulation_distance: u8,
    pub game_mode: GameType,
    pub difficulty: Difficulty,
    /// Cheats. Puts the player in the op group rather than setting a flag.
    pub allow_commands: bool,
    pub transport: ServerTransport,
}

/// How far along a launch is, as of the last [`PendingWorld::poll`].
pub enum Progress {
    /// Still starting. Keep polling.
    Starting,
    /// The world is up and the pipe is live.
    Ready(WorldHandle),
    /// Startup failed and the server thread has stopped.
    Failed(String),
}

/// A world that is starting up. Dropping it before it is ready still stops the
/// server.
pub struct PendingWorld {
    started: Receiver<Result<(), String>>,
    world: Option<WorldHandle>,
}

/// A running world. Dropping it stops the server, saves, and waits.
pub struct WorldHandle {
    cancel: CancellationToken,
    /// Taken by [`Drop`], which is the only thing that joins.
    thread: Option<JoinHandle<()>>,
}

/// Starts a world on its own thread and returns immediately.
#[must_use]
pub fn launch(options: LaunchOptions) -> PendingWorld {
    let cancel = CancellationToken::new();
    let (started, receiver) = sync_channel(1);
    let thread = thread::Builder::new()
        .name("integrated-server".to_owned())
        .spawn({
            let cancel = cancel.clone();
            move || drive(options, &cancel, &started)
        })
        .expect("spawning the integrated server thread should not fail");

    PendingWorld {
        started: receiver,
        world: Some(WorldHandle {
            cancel,
            thread: Some(thread),
        }),
    }
}

impl PendingWorld {
    /// Checks on the launch without blocking. Hands the world over once, so
    /// drop this as soon as it does.
    pub fn poll(&mut self) -> Progress {
        match self.started.try_recv() {
            Err(TryRecvError::Empty) => Progress::Starting,
            Ok(Ok(())) => self
                .world
                .take()
                .map_or(Progress::Starting, Progress::Ready),
            Ok(Err(error)) => Progress::Failed(error),
            // The thread ended without reporting, so it panicked.
            Err(TryRecvError::Disconnected) => {
                Progress::Failed("the integrated server stopped while starting".to_owned())
            }
        }
    }

    /// Asks the server to save and stop. Returns without waiting.
    pub fn begin_shutdown(&self) {
        if let Some(world) = &self.world {
            world.begin_shutdown();
        }
    }

    /// Whether the server has finished saving and stopped.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.world.as_ref().is_none_or(WorldHandle::is_finished)
    }
}

impl WorldHandle {
    /// Asks the server to save and stop. Returns without waiting.
    pub fn begin_shutdown(&self) {
        self.cancel.cancel();
    }

    /// Whether the server has finished saving and stopped.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }
}

impl Drop for WorldHandle {
    fn drop(&mut self) {
        self.begin_shutdown();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("The integrated server thread panicked");
        }
    }
}

/// Owns both runtimes for the life of the world, so neither is dropped from
/// inside itself.
fn drive(
    options: LaunchOptions,
    cancel: &CancellationToken,
    started: &SyncSender<Result<(), String>>,
) {
    let (main_threads, chunk_threads) = runtime_threads();
    let runtimes = runtime("chunk-worker", chunk_threads).and_then(|chunk| {
        runtime("server-worker", main_threads).map(|main| (Arc::new(chunk), main))
    });
    let Some((chunk_runtime, main_runtime)) = report_failure(started, runtimes) else {
        return;
    };

    main_runtime.block_on(serve(
        options,
        cancel,
        started,
        Arc::clone(&chunk_runtime),
        chunk_threads,
    ));
}

/// Splits steel's share of the machine between its two runtimes.
///
/// Steel's own helper already reserves half the machine; the client needs the
/// rest for rendering and chunk meshing. Generation is the heavier half.
fn runtime_threads() -> (usize, usize) {
    let budget = worker_threads_for_available(None, available_worker_threads());
    let main = (budget / 2).max(1);
    (main, (budget - main).max(1))
}

fn runtime(name: &str, worker_threads: usize) -> Result<Runtime, String> {
    Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name(name)
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the {name} runtime: {error}"))
}

/// Passes a startup failure back to [`PendingWorld::poll`].
fn report_failure<T>(
    started: &SyncSender<Result<(), String>>,
    result: Result<T, String>,
) -> Option<T> {
    result
        .map_err(|error| {
            let _ = started.send(Err(error));
        })
        .ok()
}

async fn serve(
    options: LaunchOptions,
    cancel: &CancellationToken,
    started: &SyncSender<Result<(), String>>,
    chunk_runtime: Arc<Runtime>,
    chunk_threads: usize,
) {
    let opened = open_world(&options, cancel, chunk_runtime, chunk_threads).await;
    let Some(server) = report_failure(started, opened) else {
        return;
    };

    let running = tokio::spawn({
        let server = Arc::clone(&server);
        let cancel = cancel.clone();
        async move { server.run(cancel).await }
    });

    let tasks = TaskTracker::new();
    let (client, outgoing, incoming) = JavaTcpClient::from_transport(
        options.transport.read,
        options.transport.write,
        CLIENT_ADDRESS,
        0,
        cancel.child_token(),
        Arc::clone(&server),
        Arc::new(ServerConnectionSession::default()),
        tasks.clone(),
    );
    let client = Arc::new(client);
    client.start_outgoing_packet_task(outgoing);
    client.start_incoming_packet_task(incoming);

    let _ = started.send(Ok(()));

    cancel.cancelled().await;
    let _ = running.await;

    // save_and_shutdown requires packet processing to have stopped.
    tasks.close();
    tasks.wait().await;
    server.save_and_shutdown().await;
}

/// Builds the server and generates its spawn area.
async fn open_world(
    options: &LaunchOptions,
    cancel: &CancellationToken,
    chunk_runtime: Arc<Runtime>,
    chunk_threads: usize,
) -> Result<Arc<Server>, String> {
    let groups = PermissionGroupManager::new(permission_groups(options.allow_commands), None)
        .map_err(|error| format!("failed to build the permission groups: {error}"))?;

    let server = Arc::new(
        Server::new_with_commands(
            chunk_runtime,
            cancel.clone(),
            runtime_config(options, chunk_threads),
            worlds_config(options),
            groups,
            CommandRegistry::new(),
        )
        .await?,
    );

    if server.prepare_spawn_area().await {
        Ok(server)
    } else {
        server.save_and_shutdown().await;
        Err("failed to prepare the spawn area".to_owned())
    }
}

/// Steel's default config always defines the op group, and its name is not
/// exported, so this names it directly.
fn permission_groups(allow_commands: bool) -> PermissionGroupsConfig {
    let mut groups = PermissionGroupsConfig::default();
    if allow_commands {
        groups.default_groups.push("op".to_owned());
    }
    groups
}

fn runtime_config(options: &LaunchOptions, chunk_threads: usize) -> RuntimeConfig {
    RuntimeConfig {
        max_players: 1,
        view_distance: options.view_distance,
        simulation_distance: options.simulation_distance,
        // The only player is this process, so there is nobody to authenticate
        // and nothing on the wire to encrypt or compress.
        online_mode: false,
        encryption: false,
        compression: None,
        // Pomme does not sign chat.
        enforce_secure_chat: false,
        use_favicon: false,
        chunk_generation_threads: Some(chunk_threads),
        chunk_encoding_threads: Some(chunk_threads),
        ..RuntimeConfig::default()
    }
}

fn worlds_config(options: &LaunchOptions) -> WorldsConfig {
    let world = |name: &str, default: bool| WorldEntryConfig {
        name: name.to_owned(),
        generator: Identifier::vanilla(name.to_owned()),
        default,
        seed: None,
        default_gamemode: None,
        difficulty: None,
        storage: None,
        nether_portal_target: None,
        end_portal_target: None,
        config: None,
    };

    WorldsConfig {
        save_path: options.save_path.to_string_lossy().into_owned(),
        seed: Some(options.seed.to_string()),
        default_gamemode: Some(options.game_mode),
        difficulty: Some(options.difficulty),
        storage: Some(StorageSelection::default_world_disk()),
        player_storage: Some(StorageSelection::default_player_file()),
        domains: BTreeMap::from([(
            "minecraft".to_owned(),
            DomainConfig {
                default: true,
                seed: None,
                default_gamemode: None,
                difficulty: None,
                storage: None,
                worlds: vec![
                    world("overworld", true),
                    world("the_nether", false),
                    world("the_end", false),
                ],
            },
        )]),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::{
        Difficulty, GameType, LaunchOptions, PathBuf, Progress, ServerTransport, launch,
        permission_groups, thread, worlds_config,
    };

    /// Removes the world whether or not the assertions pass.
    struct TempWorld(PathBuf);

    impl Drop for TempWorld {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn options(save_path: PathBuf) -> LaunchOptions {
        LaunchOptions {
            save_path,
            seed: 1,
            view_distance: 4,
            simulation_distance: 4,
            game_mode: GameType::Creative,
            difficulty: Difficulty::Peaceful,
            allow_commands: true,
            // Nothing joins, so the server reads end of file and writes nowhere.
            transport: ServerTransport {
                read: Box::new(tokio::io::empty()),
                write: Box::new(tokio::io::sink()),
            },
        }
    }

    /// Opens a world, waits for it, then stops it and waits for the save.
    fn open_and_close(label: &str) -> TempWorld {
        let world = TempWorld(
            std::env::temp_dir().join(format!("pomme-singleplayer-{}-{label}", std::process::id())),
        );
        fs::create_dir_all(&world.0).expect("the save directory should be creatable");

        let mut pending = launch(options(world.0.clone()));

        let handle = loop {
            match pending.poll() {
                Progress::Starting => thread::sleep(Duration::from_millis(50)),
                Progress::Ready(handle) => break handle,
                Progress::Failed(error) => panic!("the world should open: {error}"),
            }
        };

        // Both drops save and wait, so the world is on disk once this returns.
        drop(handle);
        drop(pending);
        world
    }

    /// Covers the config mapping, the two runtimes, spawn preparation and the
    /// shutdown save. Opening the second world is the guard on steel's global
    /// registry init staying idempotent, which a session needs to load more
    /// than one world.
    ///
    /// Ignored by default because it generates the spawn area.
    #[test]
    #[ignore = "generates terrain and writes worlds to disk"]
    fn opens_two_worlds_in_one_process() {
        for world in [open_and_close("first"), open_and_close("second")] {
            assert!(
                world.0.join("minecraft").is_dir(),
                "the default domain should have been written to {}",
                world.0.display()
            );
        }
    }

    /// The per-world settings have to reach the config steel actually reads,
    /// and cheats are a group rather than a flag.
    #[test]
    fn world_settings_reach_the_steel_config() {
        let config = worlds_config(&options(PathBuf::from("saves/world")));
        assert_eq!(config.default_gamemode, Some(GameType::Creative));
        assert_eq!(config.difficulty, Some(Difficulty::Peaceful));
        assert_eq!(config.seed.as_deref(), Some("1"));

        let op = "op".to_owned();
        assert!(permission_groups(true).default_groups.contains(&op));
        assert!(!permission_groups(false).default_groups.contains(&op));
    }
}
