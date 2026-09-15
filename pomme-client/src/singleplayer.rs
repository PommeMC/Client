//! The integrated server, as the rest of the client sees it.
//!
//! This is the only module that knows whether SteelMC is compiled in, so the
//! app phases can carry a world without a `cfg` at every construction site.

/// Whether this build can open a world.
pub const AVAILABLE: bool = cfg!(feature = "singleplayer");

/// Shown wherever the feature is compiled out.
pub const UNAVAILABLE_MESSAGE: &str = "This build was compiled without singleplayer.";

#[cfg(feature = "singleplayer")]
mod steel {
    use std::fs::{File, OpenOptions, TryLockError};
    use std::path::Path;

    use pomme_singleplayer::{
        Difficulty, GameType, LaunchOptions, PendingWorld, Progress, ServerTransport, WorldHandle,
        launch,
    };

    use crate::net::conn::{MemoryEnd, memory_pipes};
    use crate::ui::world_list::{self, WorldSummary};

    /// Holds the world folder for the session. Two clients writing the same
    /// region files corrupts the save. Released when the file handle drops.
    struct SessionLock {
        _file: File,
    }

    impl SessionLock {
        fn take(dir: &Path) -> Result<Self, String> {
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join("session.lock"))
                .map_err(|error| format!("Could not open the world's session lock: {error}"))?;
            file.try_lock().map_err(|error| match error {
                // Vanilla `selectWorld.locked`.
                TryLockError::WouldBlock => {
                    "Locked by another running instance of Minecraft.".to_owned()
                }
                TryLockError::Error(error) => {
                    format!("Could not lock the world's session lock: {error}")
                }
            })?;
            Ok(Self { _file: file })
        }
    }

    enum Server {
        Starting(PendingWorld),
        /// Dropping the handle stops the server and saves.
        Running(WorldHandle),
    }

    /// The integrated server for this session.
    pub struct World {
        // Declared first so the server saves and stops before the lock is
        // released, not after.
        server: Server,
        _lock: SessionLock,
    }

    impl World {
        /// Advances a starting world and watches a running one, returning the
        /// reason the server is gone.
        pub fn poll(&mut self) -> Result<(), String> {
            match &mut self.server {
                Server::Starting(pending) => match pending.poll() {
                    Progress::Starting => Ok(()),
                    Progress::Ready(handle) => {
                        self.server = Server::Running(handle);
                        Ok(())
                    }
                    Progress::Failed(error) => Err(error),
                },
                Server::Running(handle) if handle.is_finished() => {
                    Err("The integrated server stopped unexpectedly.".to_owned())
                }
                Server::Running(_) => Ok(()),
            }
        }

        fn handle(&self) -> Option<&WorldHandle> {
            match &self.server {
                Server::Starting(pending) => pending.handle(),
                Server::Running(handle) => Some(handle),
            }
        }

        /// Asks the server to save and stop, whether or not it finished
        /// starting. Returns without waiting.
        pub fn begin_close(&self) {
            if let Some(handle) = self.handle() {
                handle.begin_shutdown();
            }
        }

        /// Whether the server has finished saving and stopped.
        pub fn is_closed(&self) -> bool {
            self.handle().is_none_or(WorldHandle::is_finished)
        }
    }

    /// Starts `world` and hands back the client's end of the pipe.
    pub fn open(
        world: &WorldSummary,
        dir: &Path,
        view_distance: u8,
    ) -> Result<(World, MemoryEnd), String> {
        let lock = SessionLock::take(dir)?;
        let (client, server) = memory_pipes();

        // An empty seed means the server picks one on first launch and keeps it
        // in its own level data from then on.
        let seed = world_list::parse_seed(&world.seed).unwrap_or_else(|| fastrand::i64(..));

        let pending = launch(LaunchOptions {
            save_path: dir.to_path_buf(),
            seed,
            view_distance,
            simulation_distance: view_distance,
            game_mode: match world.game_mode {
                world_list::GameMode::Survival => GameType::Survival,
                world_list::GameMode::Creative => GameType::Creative,
            },
            difficulty: match world.difficulty {
                world_list::Difficulty::Peaceful => Difficulty::Peaceful,
                world_list::Difficulty::Easy => Difficulty::Easy,
                world_list::Difficulty::Normal => Difficulty::Normal,
                world_list::Difficulty::Hard => Difficulty::Hard,
            },
            allow_commands: world.allow_commands,
            transport: ServerTransport {
                read: Box::new(server.rx),
                write: Box::new(server.tx),
            },
        });

        Ok((
            World {
                server: Server::Starting(pending),
                _lock: lock,
            },
            client,
        ))
    }
}

#[cfg(not(feature = "singleplayer"))]
mod steel {
    use std::path::Path;

    use crate::net::conn::MemoryEnd;
    use crate::ui::world_list::WorldSummary;

    /// Uninhabited without the feature, so `Option<World>` costs nothing and
    /// the compiler proves every singleplayer arm dead.
    pub enum World {}

    impl World {
        pub const fn poll(&mut self) -> Result<(), String> {
            match *self {}
        }

        pub const fn begin_close(&self) {
            match *self {}
        }

        pub const fn is_closed(&self) -> bool {
            match *self {}
        }
    }

    pub fn open(
        _world: &WorldSummary,
        _dir: &Path,
        _view_distance: u8,
    ) -> Result<(World, MemoryEnd), String> {
        Err(super::UNAVAILABLE_MESSAGE.to_owned())
    }
}

pub use steel::{World, open};
