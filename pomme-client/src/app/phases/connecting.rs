use crate::app::TICK_RATE;
use crate::app::core::AppCore;
use crate::app::phases::in_game::GameState;
use crate::app::phases::{ConnectionPhase, Gfx, Panorama, draw_status};
use crate::net::connection::ConnectionHandle;
use crate::singleplayer::World;

pub enum ConnectingUpdateResult {
    None,
    ManualDisconnect,
    Disconnected { reason: String },
    JoinGame,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per field of the phase it updates"
)]
pub fn update_connecting(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
    connect_phase: &mut ConnectionPhase,
    connection: &ConnectionHandle,
    game: &mut GameState,
    world: Option<&mut World>,
) -> ConnectingUpdateResult {
    // Polled before the network, so a server that failed to start reports its
    // own reason rather than the end of file its death also causes. The phase
    // stays `StartingWorld` until the connection reports in; vanilla shows one
    // screen from server start until terrain appears.
    if let Some(world) = world
        && let Err(reason) = world.poll()
    {
        return ConnectingUpdateResult::Disconnected { reason };
    }

    let disconnect_reason = core.drain_network_events(
        connection,
        Some(connect_phase),
        &mut gfx.renderer,
        &gfx.window,
        game,
    );
    if let Some(reason) = disconnect_reason {
        return ConnectingUpdateResult::Disconnected { reason };
    }

    if matches!(connect_phase, ConnectionPhase::Loading) {
        game.mesh_dispatcher
            .set_camera_position(*game.player.position);
        game.drain_and_upload_meshes(&mut gfx.renderer);

        // Vanilla keeps ticking behind the loading screen: the level load
        // tracker advances and every tick is still marked with
        // `client_tick_end`, while `LocalPlayer.tick` stays parked. Vanilla
        // gates those ticks on having a level, which is the login that also
        // starts the tracker.
        if game.level_load.is_some() {
            core.tick_accumulator += dt;
            while core.tick_accumulator >= TICK_RATE {
                AppCore::tick_level_load(&gfx.renderer, connection, game);
                if game.client_loaded {
                    // Vanilla closes `LevelLoadingScreen` on the same tick that
                    // sends `player_loaded`, and the local player then ticks
                    // (and moves) later in it. Hand this tick to the game phase
                    // unspent so it plays out there.
                    return ConnectingUpdateResult::JoinGame;
                }
                AppCore::send_client_tick_end(connection);
                core.tick_accumulator -= TICK_RATE;
            }
        } else {
            core.tick_accumulator = 0.0;
        }
    }

    let status_text = match connect_phase {
        ConnectionPhase::StartingWorld | ConnectionPhase::Loading => "Loading terrain...",
        ConnectionPhase::Connecting => "Connecting to the server...",
    };

    if draw_status(core, dt, gfx, panorama, status_text, Some("Cancel")) {
        return ConnectingUpdateResult::ManualDisconnect;
    }

    ConnectingUpdateResult::None
}
