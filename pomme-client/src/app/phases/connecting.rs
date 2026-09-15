use azalea_protocol::packets::game::ServerboundGamePacket;

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
        let ready_meshes: Vec<_> = game.mesh_dispatcher.drain_results().collect();
        gfx.renderer.upload_chunk_meshes(&ready_meshes);
        for mesh in ready_meshes {
            game.mesh_dispatcher.recycle(mesh);
        }

        let ready = game.position_set && (game.dead || gfx.renderer.loaded_chunk_count() > 0);

        // Mirror vanilla's `notifyPlayerLoaded`; servers gate
        // per-player entity tracking on it.
        if ready && !game.player_loaded_sent {
            connection
                .packet_tx
                .send(ServerboundGamePacket::PlayerLoaded(
                    azalea_protocol::packets::game::s_player_loaded::ServerboundPlayerLoaded,
                ));
            game.player_loaded_sent = true;
        }

        if ready {
            return ConnectingUpdateResult::JoinGame;
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
