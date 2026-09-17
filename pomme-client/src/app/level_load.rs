//! Port of vanilla `client/multiplayer/LevelLoadTracker.java` (26.2), client
//! side only: the state machine that decides when a freshly joined (or
//! respawned) player has loaded enough of the level to be let into it.
//!
//! Vanilla drives it from `ClientPacketListener.tick`, which sends
//! `ServerboundPlayerLoadedPacket` the first tick `isLevelReady` holds and
//! keeps `LocalPlayer.tick` (physics and every movement packet) from running
//! until then. The server-progress half of the vanilla class (the loading
//! screen's progress bar, `LevelLoadListener`) is not ported.

use std::time::{Duration, Instant};

/// Vanilla `CLIENT_WAIT_TIMEOUT_MS`: let the player in regardless after this
/// long, counted from the login/respawn that started the load.
const CLIENT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// What `WaitingForPlayerChunk.isReady` reads each tick.
pub struct ReadyInputs {
    /// Player block Y is outside the dimension's build height.
    pub player_outside_build_height: bool,
    /// Camera block Y is outside the dimension's build height.
    pub camera_outside_build_height: bool,
    pub spectator: bool,
    pub alive: bool,
    /// Vanilla `playerSectionReady`: the render section at the camera block
    /// has a compiled, uploaded mesh.
    pub player_section_ready: bool,
}

enum ClientState {
    /// Waiting for the server to say it has started sending the level.
    WaitingForServer {
        timeout_after: Instant,
    },
    WaitingForPlayerChunk {
        timeout_after: Instant,
    },
    ClientLevelReady {
        ready_at: Instant,
    },
}

pub struct LevelLoadTracker {
    state: ClientState,
    close_delay: Duration,
}

impl LevelLoadTracker {
    /// Vanilla `new LevelLoadTracker(closeDelayMs)` + `startClientLoad`, which
    /// always run together on login and respawn.
    pub fn start_client_load(close_delay: Duration, now: Instant) -> Self {
        Self {
            state: ClientState::WaitingForServer {
                timeout_after: now + CLIENT_WAIT_TIMEOUT,
            },
            close_delay,
        }
    }

    /// Vanilla `loadingPacketsReceived`, from the `LEVEL_CHUNKS_LOAD_START`
    /// game event. Only the first state reacts.
    pub fn loading_packets_received(&mut self) {
        if let ClientState::WaitingForServer { timeout_after } = self.state {
            self.state = ClientState::WaitingForPlayerChunk { timeout_after };
        }
    }

    /// Vanilla `tickClientLoad`. Only `WaitingForPlayerChunk` advances, so the
    /// 30s timeout doesn't apply until the server has started the level.
    pub fn tick_client_load(&mut self, now: Instant, inputs: &ReadyInputs) {
        if let ClientState::WaitingForPlayerChunk { timeout_after } = self.state
            && Self::is_ready(now, timeout_after, inputs)
        {
            self.state = ClientState::ClientLevelReady { ready_at: now };
        }
    }

    fn is_ready(now: Instant, timeout_after: Instant, inputs: &ReadyInputs) -> bool {
        if now > timeout_after {
            tracing::warn!(
                "Timed out while waiting for the client to load chunks, letting the player into the world anyway"
            );
            return true;
        }
        if inputs.player_outside_build_height
            || inputs.camera_outside_build_height
            || inputs.spectator
            || !inputs.alive
        {
            return true;
        }
        inputs.player_section_ready
    }

    /// Vanilla `isLevelReady`: ready, and the close delay has elapsed.
    pub fn is_level_ready(&self, now: Instant) -> bool {
        match self.state {
            ClientState::ClientLevelReady { ready_at } => now >= ready_at + self.close_delay,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(player_section_ready: bool) -> ReadyInputs {
        ReadyInputs {
            player_outside_build_height: false,
            camera_outside_build_height: false,
            spectator: false,
            alive: true,
            player_section_ready,
        }
    }

    #[test]
    fn waits_for_the_server_then_the_player_chunk() {
        let now = Instant::now();
        let mut tracker = LevelLoadTracker::start_client_load(Duration::ZERO, now);

        // No `LEVEL_CHUNKS_LOAD_START` yet: a ready chunk doesn't count.
        tracker.tick_client_load(now, &inputs(true));
        assert!(!tracker.is_level_ready(now));

        tracker.loading_packets_received();
        tracker.tick_client_load(now, &inputs(false));
        assert!(!tracker.is_level_ready(now));

        tracker.tick_client_load(now, &inputs(true));
        assert!(tracker.is_level_ready(now));
    }

    #[test]
    fn ready_when_the_player_cannot_wait_for_a_chunk() {
        let now = Instant::now();
        for patch in [
            |i: &mut ReadyInputs| i.player_outside_build_height = true,
            |i: &mut ReadyInputs| i.camera_outside_build_height = true,
            |i: &mut ReadyInputs| i.spectator = true,
            |i: &mut ReadyInputs| i.alive = false,
        ] {
            let mut tracker = LevelLoadTracker::start_client_load(Duration::ZERO, now);
            tracker.loading_packets_received();
            let mut i = inputs(false);
            patch(&mut i);
            tracker.tick_client_load(now, &i);
            assert!(tracker.is_level_ready(now));
        }
    }

    #[test]
    fn times_out_only_after_the_server_started_the_level() {
        let start = Instant::now();
        let late = start + CLIENT_WAIT_TIMEOUT + Duration::from_millis(1);

        let mut waiting_for_server = LevelLoadTracker::start_client_load(Duration::ZERO, start);
        waiting_for_server.tick_client_load(late, &inputs(false));
        assert!(!waiting_for_server.is_level_ready(late));

        let mut tracker = LevelLoadTracker::start_client_load(Duration::ZERO, start);
        tracker.loading_packets_received();
        tracker.tick_client_load(late, &inputs(false));
        assert!(tracker.is_level_ready(late));
    }

    #[test]
    fn close_delay_holds_the_ready_state_back() {
        // Vanilla `LEVEL_LOAD_CLOSE_DELAY_MS`.
        let close_delay = Duration::from_millis(500);
        let now = Instant::now();
        let mut tracker = LevelLoadTracker::start_client_load(close_delay, now);
        tracker.loading_packets_received();
        tracker.tick_client_load(now, &inputs(true));

        assert!(!tracker.is_level_ready(now));
        assert!(tracker.is_level_ready(now + close_delay));
    }
}
