use azalea_protocol::packets::game::ServerboundGamePacket;

use crate::app::core::AppCore;
use crate::app::phases::in_game::GameState;
use crate::app::phases::{ConnectionPhase, Gfx, Panorama};
use crate::net::connection::ConnectionHandle;
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::ui::{common, hud};

pub enum ConnectingUpdateResult {
    None,
    ManualDisconnect,
    Disconnected { reason: String },
    JoinGame,
}

pub fn update_connecting(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
    connect_phase: &mut ConnectionPhase,
    connection: &ConnectionHandle,
    game: &mut GameState,
) -> ConnectingUpdateResult {
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
        for mesh in game.mesh_dispatcher.drain_results() {
            gfx.renderer.upload_chunk_mesh(&mesh);
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
        ConnectionPhase::Loading => "Loading terrain...",
        ConnectionPhase::Connecting => "Connecting to the server...",
    };

    panorama.update(dt);

    let mut cancel = false;

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;
    let gs = hud::gui_scale(sw, sh, core.menu.gui_scale_setting);
    let fs = 11.0 * gs;
    let btn_h = 30.0 * gs;
    let btn_w = 160.0 * gs;

    let cx = sw / 2.0;
    let cy = sh / 2.0;

    let mut elements = Vec::new();
    let clicked = core.input.left_just_pressed();
    let cursor = core.input.cursor_pos();

    elements.push(MenuElement::Text {
        x: cx,
        y: cy - fs,
        text: status_text.into(),
        scale: fs,
        color: common::WHITE,
        centered: true,
    });

    let btn_y = cy + fs;
    if common::push_button(
        &mut elements,
        cursor,
        cx - btn_w / 2.0,
        btn_y,
        btn_w,
        btn_h,
        gs,
        fs,
        "Cancel",
        true,
    ) && clicked
    {
        cancel = true;
    }

    core.input.clear_click_events();

    if let Err(e) = gfx.renderer.render_menu(
        &gfx.window,
        panorama.scroll(),
        2.0,
        elements,
        core.input.cursor_pos(),
        false,
    ) {
        tracing::error!("Render error: {e}");
    }

    if cancel {
        return ConnectingUpdateResult::ManualDisconnect;
    }

    ConnectingUpdateResult::None
}
