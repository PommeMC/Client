use crate::ui::components::debug::displayer::DebugScreenDisplayer;
use crate::ui::components::debug::entry::DebugEntry;
use crate::ui::components::debug::groups::DebugGroup;
use crate::ui::components::debug::info::DebugInfo;

pub struct FrameTimingsDebugEntry;

#[derive(Clone, Copy, Default)]
pub struct FrameTimings {
    pub pace_ms: f32,
    pub frame_ms: f32,
    pub fence_ms: f32,
    pub acquire_ms: f32,
    pub cull_ms: f32,
    pub draw_ms: f32,
    pub present_ms: f32,
}

impl DebugEntry for FrameTimingsDebugEntry {
    fn display(&mut self, displayer: &mut DebugScreenDisplayer, info: &DebugInfo) {
        let t = info.frame_timings;
        let rows = [
            ("Pace", t.pace_ms),
            ("Frame", t.frame_ms),
            ("Fence", t.fence_ms),
            ("Acquire", t.acquire_ms),
            ("Cull", t.cull_ms),
            ("Draw", t.draw_ms),
            ("Present", t.present_ms),
        ];
        for (name, ms) in rows {
            displayer.add_fact_to_group(DebugGroup::FrameTimings, name, |f| {
                f.value(format!("{ms:.2}")).text("ms");
            });
        }
    }
}
