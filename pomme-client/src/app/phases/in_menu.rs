use crate::app::core::AppCore;
use crate::app::phases::{Gfx, Panorama};
use crate::net::connection::ConnectArgs;
use crate::ui::menu::{MenuAction, PanoramaTheme};

pub enum MenuUpdateResult {
    None,
    Connect { connect_args: ConnectArgs },
    Quit,
}

pub fn update_menu(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
) -> MenuUpdateResult {
    panorama.update(dt);

    let sw = gfx.renderer.screen_width() as f32;
    let sh = gfx.renderer.screen_height() as f32;

    let menu_input = core.build_menu_input();

    let result = core.menu.build(sw, sh, &menu_input, |t, s| {
        gfx.renderer.menu_text_width(t, s)
    });
    let action = result.action;

    let cursor_icon = if result.cursor_pointer {
        winit::window::CursorIcon::Pointer
    } else {
        winit::window::CursorIcon::Default
    };
    if core.input.cursor_moved_this_frame() {
        gfx.window.set_cursor(cursor_icon);
    }

    if core.menu.is_server_list_screen() && core.menu.favicons_changed() {
        let favicons = core.menu.collect_favicons();
        if !favicons.is_empty() {
            gfx.renderer.update_favicon_atlas(&favicons);
        }
    }

    if let Err(e) = gfx.renderer.render_menu(
        &gfx.window,
        panorama.scroll(),
        result.blur,
        result.elements,
        core.input.cursor_pos(),
        core.menu.is_main_screen(),
    ) {
        tracing::error!("Render error: {e}");
    }

    core.input.clear_click_events();

    if core.menu.display_mode != core.display_mode {
        core.display_mode = core.menu.display_mode;
        core.apply_display_mode(&gfx.window);
    }

    if core.menu.rescan_packs {
        core.menu.rescan_packs = false;
        core.resource_packs.scan_local_packs();
        core.menu.available_packs = core.resource_packs.available_local_packs().to_vec();
        core.menu.active_packs = core.resource_packs.active_pack_info();
    }

    if let Some((name, enable)) = core.menu.pack_toggle.take() {
        if enable {
            core.resource_packs.enable_local_pack(&name);
        } else {
            core.resource_packs.disable_local_pack(&name);
        }
        core.menu.active_packs = core.resource_packs.active_pack_info();
        core.menu.available_packs = core.resource_packs.available_local_packs().to_vec();
    }

    if core.menu.reload_assets {
        core.menu.reload_assets = false;
        gfx.renderer
            .reload_assets(&core.data_dirs.game_dir, &core.resource_packs);
    }

    if result.clicked_button {
        gfx.renderer.trigger_skin_swing();
    }

    match action {
        MenuAction::Connect { server, username } => {
            let connect_args = ConnectArgs {
                server,
                username,
                uuid: core.user.uuid,
                access_token: core.user.access_token.clone(),
                view_distance: core.menu.render_distance as u8,
            };

            return MenuUpdateResult::Connect { connect_args };
        }
        MenuAction::ChangeTheme(theme) => {
            let panorama_dir = match theme {
                PanoramaTheme::Default => core.data_dirs.jar_assets_dir.clone(),
                PanoramaTheme::Pomme => core.data_dirs.pomme_assets_dir.join("panoramas"),
            };
            gfx.renderer
                .reload_panorama(&panorama_dir, &core.asset_index);
            core.menu.start_transition_open();
        }
        MenuAction::Quit => {
            return MenuUpdateResult::Quit;
        }
        MenuAction::None => {}
    }

    MenuUpdateResult::None
}
