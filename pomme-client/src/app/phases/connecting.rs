use azalea_protocol::packets::game::{ServerboundGamePacket, s_client_tick_end};

use crate::app::TICK_RATE;
use crate::app::core::AppCore;
use crate::app::phases::in_game::GameState;
use crate::app::phases::{ConnectionPhase, Gfx, Panorama, draw_status};
use crate::net::connection::ConnectionHandle;
use crate::singleplayer::World;

fn player_section_key(
    position: crate::entity::components::Position,
    min_y: i32,
) -> (azalea_core::position::ChunkPos, i32) {
    let x = position.x.floor() as i32;
    let y = position.y.floor() as i32;
    let z = position.z.floor() as i32;
    (
        azalea_core::position::ChunkPos::new(x.div_euclid(16), z.div_euclid(16)),
        (y - min_y).div_euclid(16),
    )
}

fn outside_build_height(y: i32, min_y: i32, height: u32) -> bool {
    let max_y = min_y.saturating_add(i32::try_from(height).unwrap_or(i32::MAX));
    y < min_y || y >= max_y
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors LevelLoadTracker.WaitingForPlayerChunk::isReady predicates"
)]
fn level_load_ready(
    position_set: bool,
    player_section_ready: bool,
    timed_out: bool,
    player_y: i32,
    camera_y: i32,
    min_y: i32,
    height: u32,
    spectator: bool,
    dead: bool,
) -> bool {
    position_set
        && (timed_out
            || outside_build_height(player_y, min_y, height)
            || outside_build_height(camera_y, min_y, height)
            || spectator
            || dead
            || player_section_ready)
}

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

        // Vanilla's LevelLoadTracker is released by the renderer callback for
        // the player's compiled section, not when the backing chunk packet has
        // merely arrived. `replaced` includes empty sections too, so observing
        // the completed mesh job is the equivalent signal even when there is
        // no geometry to upload.
        if game.position_set {
            let key = player_section_key(game.player.position, game.chunk_store.min_y());
            if ready_meshes
                .iter()
                .any(|mesh| mesh.pos == key.0 && mesh.replaced.contains(&key.1))
            {
                game.player_compiled_section = Some(key);
            }
        }

        gfx.renderer.upload_chunk_meshes(&ready_meshes);
        for mesh in ready_meshes {
            game.mesh_dispatcher.recycle(mesh);
        }

        let player_key = player_section_key(game.player.position, game.chunk_store.min_y());
        let player_y = game.player.position.y.floor() as i32;
        let camera_y = gfx.renderer.camera_render_position().y.floor() as i32;
        let timed_out = std::time::Instant::now() > game.client_load_deadline;
        let ready = level_load_ready(
            game.position_set,
            game.player_compiled_section == Some(player_key),
            timed_out,
            player_y,
            camera_y,
            game.chunk_store.min_y(),
            game.chunk_store.height(),
            crate::player::is_spectator(game.player.game_mode),
            game.dead,
        );

        // A loading screen does not suspend Minecraft's client tick. Vanilla
        // keeps MultiPlayerGameMode/ClientPacketListener ticking at 20 Hz and
        // closes every play-state tick with CLIENT_TICK_END; only
        // LocalPlayer.tick itself waits for PlayerLoaded. Reuse the same
        // accumulator that gameplay will inherit so the transition cannot
        // create a bunched or missing boundary.
        core.tick_accumulator += dt;
        while core.tick_accumulator >= TICK_RATE {
            game.interaction
                .ensure_has_sent_carried_item(&connection.packet_tx, core.input.selected_slot());

            // Vanilla MultiPlayerGameMode.tick calls ClientPacketListener.tick
            // before ClientLevel.tickEntities. On the tick where
            // notifyPlayerLoaded flips hasClientLoaded, LocalPlayer.tick therefore
            // runs later in that *same* client tick before CLIENT_TICK_END.
            if ready && !game.player_loaded_sent {
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::PlayerLoaded(
                        azalea_protocol::packets::game::s_player_loaded::ServerboundPlayerLoaded,
                    ));
                game.player_loaded_sent = true;
            }

            if game.player_loaded_sent {
                // The loading screen still suppresses gameplay input/keybinds,
                // but the local player/entity tick is live immediately after
                // PlayerLoaded. Reuse the normal physics/packet tail with forced
                // neutral input so movement starts on the vanilla tick boundary.
                game.tick_count = game.tick_count.wrapping_add(1);
                core.tick_loading_physics(&mut gfx.renderer, connection, game);
                game.player.tick_hurt();
                game.player.effects.tick();
                game.player.tick_sleep();
                game.item_entity_store.tick(&game.chunk_store);
                game.particle_store.tick(&game.chunk_store);
                game.block_entity_anim.tick();
                game.title.tick();
            } else {
                connection
                    .packet_tx
                    .send(ServerboundGamePacket::ClientTickEnd(
                        s_client_tick_end::ServerboundClientTickEnd,
                    ));
            }
            core.tick_accumulator -= TICK_RATE;
        }

        // Do not enter LocalPlayer ticking until PlayerLoaded was actually sent
        // from a completed loading tick.
        if ready && game.player_loaded_sent {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::components::Position;

    #[test]
    fn player_section_key_uses_chunk_and_dimension_relative_section_coordinates() {
        assert_eq!(
            player_section_key(Position::new(31.9, 64.0, -0.1), -64),
            (azalea_core::position::ChunkPos::new(1, -1), 8)
        );
        assert_eq!(
            player_section_key(Position::new(-0.1, -64.0, -16.0), -64),
            (azalea_core::position::ChunkPos::new(-1, -1), 0)
        );
    }

    #[test]
    fn level_load_ready_matches_vanilla_player_chunk_exceptions() {
        let normal = |section_ready, timed_out, spectator, dead, player_y, camera_y| {
            level_load_ready(
                true,
                section_ready,
                timed_out,
                player_y,
                camera_y,
                -64,
                384,
                spectator,
                dead,
            )
        };

        assert!(!normal(false, false, false, false, 64, 64));
        assert!(normal(true, false, false, false, 64, 64));
        assert!(normal(false, true, false, false, 64, 64));
        assert!(normal(false, false, true, false, 64, 64));
        assert!(normal(false, false, false, true, 64, 64));
        assert!(normal(false, false, false, false, -65, 64));
        assert!(normal(false, false, false, false, 64, 320));
        assert!(normal(false, false, false, false, 64, -65));

        assert!(!level_load_ready(
            false, true, true, 64, 64, -64, 384, true, true
        ));
    }
}
