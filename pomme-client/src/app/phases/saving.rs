use crate::app::core::AppCore;
use crate::app::phases::{Gfx, Panorama, draw_status};
use crate::singleplayer::World;
use crate::ui::hud::saving_level_text;

/// Vanilla's `disconnect` holds this screen until the server reports itself
/// shut down. Returns whether it has.
pub fn update_saving(
    core: &mut AppCore,
    dt: f32,
    gfx: &mut Gfx,
    panorama: &mut Panorama,
    world: &World,
) -> bool {
    draw_status(core, dt, gfx, panorama, saving_level_text(), None);
    world.is_closed()
}
