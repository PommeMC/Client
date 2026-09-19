use crate::app::TICK_RATE;
use crate::app::core::AppCore;
use crate::app::phases::in_game::{GameState, build_server_screens};
use crate::app::phases::{ConnectionPhase, Gfx, Panorama, draw_status};
use crate::net::connection::ConnectionHandle;
use crate::singleplayer::World;
use crate::ui::{common, hud};

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
        // Vanilla runs `ClientLevel.update()` every frame, loading screen
        // included; the load gate waits on the light this applies.
        game.update_light(core.menu.chunk_detail);

        // Vanilla keeps ticking behind the loading screen: the tracker advances
        // and every tick is still marked with `client_tick_end`, while
        // `LocalPlayer.tick` stays parked. Those ticks need a level, which
        // arrives with the login that also starts the tracker.
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

    if game.server_dialog.is_some() || game.chat.has_pending_modal_prompt() {
        draw_server_dialog(core, dt, gfx, panorama, connection, game);
    } else if draw_status(core, dt, gfx, panorama, status_text, Some("Cancel")) {
        return ConnectingUpdateResult::ManualDisconnect;
    }

    ConnectingUpdateResult::None
}

/// A configuration-phase dialog, shown in place of the connect screen with
/// the confirm screen its links can open.
fn draw_server_dialog(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
    connection: &ConnectionHandle,
    game: &mut GameState,
) {
    panorama.update(dt);

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = hud::gui_scale(sw, sh, core.menu.gui_scale_setting);
    if !game.chat.has_pending_modal_prompt()
        && let Some(dialog) = game.server_dialog.as_mut()
    {
        let fs = common::FONT_SIZE * gs;
        dialog.handle_text_input(&core.input.drain_text_events(), gs, &|s| {
            gfx.renderer.menu_text_width(s, fs)
        });
    }

    // A configuration-phase dialog can carry object glyphs, which load into
    // the same atlas the in-game text uses.
    core.sync_game_dynamic_atlas(game, &mut gfx.renderer, false);

    let mut elements = Vec::new();
    // The connecting screen runs no client ticks, so the dialog's own
    // timers fall back to wall time.
    build_server_screens(&mut elements, sw, sh, gs, core, gfx, connection, game, None);
    core.input.clear_just_pressed_actions();

    let cursor = core.input.cursor_pos();
    if let Err(e) =
        gfx.renderer
            .render_menu(&gfx.window, panorama.scroll(), 2.0, elements, cursor, false)
    {
        tracing::error!("Render error: {e}");
    }
}
