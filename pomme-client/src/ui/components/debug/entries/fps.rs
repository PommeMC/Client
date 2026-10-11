use pyronyx::vk::PresentModeKHR;

use crate::app::core::AppCore;
use crate::app::phases::Gfx;
use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::info::DebugInfo;
use crate::ui::menu::MAX_FRAMERATE_UNLIMITED;

pub struct FpsDebugEntry;

pub struct FpsDebugInfo {
    pub fps: u32,
    pub framerate_limit: u32,
    pub present_mode: PresentModeKHR,
    pub refresh_rate_millihertz: Option<u32>,
}

impl FpsDebugInfo {
    pub fn new(core: &AppCore, gfx: &Gfx) -> Self {
        Self {
            fps: gfx.fps_counter.display_fps(),
            framerate_limit: core.menu.max_framerate,
            present_mode: gfx.renderer.present_mode(),
            refresh_rate_millihertz: gfx
                .window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz()),
        }
    }
}

impl DebugEntry for FpsDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let f = &info.fps;

        let framerate_limit = match f.framerate_limit {
            MAX_FRAMERATE_UNLIMITED => "inf",
            lim => &lim.to_string(),
        };

        let present_mode = match f.present_mode {
            PresentModeKHR::Immediate => " (immediate)",
            PresentModeKHR::Mailbox => " (mailbox)",
            PresentModeKHR::Fifo => " (fifo)",
            PresentModeKHR::FifoRelaxed => " (fifo relaxed)",
            _ => "",
        };

        let refresh_rate = f
            .refresh_rate_millihertz
            .map(|mhz| {
                if mhz % 1000 == 0 {
                    (mhz / 1000).to_string()
                } else {
                    format!("{:.2}", mhz as f32 / 1000.0)
                }
            })
            .unwrap_or_else(|| "0".to_string());

        displayer.add_priority_line(format!(
            "{} fps T: {}{} @{}Hz",
            f.fps, framerate_limit, present_mode, refresh_rate
        ));
    }
}
