use crate::app::core::AppCore;
use crate::app::phases::Gfx;
use crate::app::phases::in_game::GameState;
use crate::ui::components::debug::entries::fps::FpsDebugInfo;
use crate::ui::components::debug::entries::frame_timings::FrameTimings;
use crate::ui::components::debug::entries::game_version::GameVersionDebugInfo;
use crate::ui::components::debug::entries::position::PositionDebugInfo;
use crate::ui::components::debug::entries::system_specs::SystemSpecsInfo;
use crate::ui::components::debug::entries::tps::TpsDebugInfo;

pub struct DebugInfo<'a> {
    pub game_version: GameVersionDebugInfo<'a>,
    pub fps: FpsDebugInfo,
    pub tps: Option<TpsDebugInfo<'a>>,
    pub system: SystemSpecsInfo<'a>,
    pub position: Option<PositionDebugInfo<'a>>,
    pub frame_timings: FrameTimings,
}

impl<'a> DebugInfo<'a> {
    pub fn new(
        core: &AppCore,
        gfx: &Gfx,
        tps: Option<TpsDebugInfo<'a>>,
        system: SystemSpecsInfo<'a>,
        game: Option<&'a GameState>,
    ) -> Self {
        let t = gfx.renderer.last_timings();
        Self {
            game_version: GameVersionDebugInfo::new(core),
            fps: FpsDebugInfo::new(core, gfx),
            tps,
            system,
            position: game.map(|game| PositionDebugInfo::new(game, gfx)),
            frame_timings: FrameTimings {
                pace_ms: t.pace_ms,
                frame_ms: t.frame_ms,
                fence_ms: t.fence_ms,
                acquire_ms: t.acquire_ms,
                cull_ms: t.cull_ms,
                draw_ms: t.draw_ms,
                present_ms: t.present_ms,
            },
        }
    }
}
