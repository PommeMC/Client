use pyronyx::vk;
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timestamp {
    FrameStart = 0,
    FrameEnd,
    CullStart,
    CullEnd,
    CullFinalizeStart,
    CullFinalizeEnd,
    GuiBakeStart,
    GuiBakeEnd,
    TerrainStart,
    TerrainEnd,
    TerrainNewStart,
    TerrainNewEnd,
    EntitiesStart,
    EntitiesEnd,
    TranslucentStart,
    TranslucentEnd,
    UiStart,
    UiEnd,
    OcclusionRegionStart,
    OcclusionRegionEnd,
    OcclusionSectionStart,
    OcclusionSectionEnd,
    Count, // Automatically tracks the total number of timestamps needed
}
#[derive(Debug, Clone, Copy)]
pub struct RenderTimings {
    pub ticks: [u64; Timestamp::Count as usize],
    pub timestamp_period: f32,
    /// Modulus mask from the queue's `timestamp_valid_bits`; deltas wrap
    /// within this width, so subtract wrapping and mask instead of saturating.
    pub timestamp_mask: u64,
}
impl RenderTimings {
    pub fn duration(&self, start: Timestamp, end: Timestamp) -> f32 {
        let diff_ticks =
            self.ticks[end as usize].wrapping_sub(self.ticks[start as usize]) & self.timestamp_mask;
        (diff_ticks as f64 * self.timestamp_period as f64 / 1_000_000.0) as f32
    }
    pub fn frame_ms(&self) -> f32 {
        self.duration(Timestamp::FrameStart, Timestamp::FrameEnd)
    }
    pub fn cull_ms(&self) -> f32 {
        self.cull_prepare_ms() + self.cull_finalize_ms()
    }
    pub fn cull_prepare_ms(&self) -> f32 {
        self.duration(Timestamp::CullStart, Timestamp::CullEnd)
    }
    pub fn cull_finalize_ms(&self) -> f32 {
        self.duration(Timestamp::CullFinalizeStart, Timestamp::CullFinalizeEnd)
    }
    pub fn gui_bake_ms(&self) -> f32 {
        self.duration(Timestamp::GuiBakeStart, Timestamp::GuiBakeEnd)
    }
    pub fn terrain_ms(&self) -> f32 {
        self.terrain_seed_ms() + self.terrain_new_ms()
    }
    pub fn terrain_seed_ms(&self) -> f32 {
        self.duration(Timestamp::TerrainStart, Timestamp::TerrainEnd)
    }
    pub fn terrain_new_ms(&self) -> f32 {
        self.duration(Timestamp::TerrainNewStart, Timestamp::TerrainNewEnd)
    }
    pub fn entities_ms(&self) -> f32 {
        self.duration(Timestamp::EntitiesStart, Timestamp::EntitiesEnd)
    }
    pub fn translucent_ms(&self) -> f32 {
        self.duration(Timestamp::TranslucentStart, Timestamp::TranslucentEnd)
    }
    pub fn ui_ms(&self) -> f32 {
        self.duration(Timestamp::UiStart, Timestamp::UiEnd)
    }
    pub fn occlusion_ms(&self) -> f32 {
        self.occlusion_region_ms() + self.occlusion_section_ms()
    }
    pub fn occlusion_region_ms(&self) -> f32 {
        self.duration(
            Timestamp::OcclusionRegionStart,
            Timestamp::OcclusionRegionEnd,
        )
    }
    pub fn occlusion_section_ms(&self) -> f32 {
        self.duration(
            Timestamp::OcclusionSectionStart,
            Timestamp::OcclusionSectionEnd,
        )
    }
}
pub struct Timer {
    cmd: vk::CommandBuffer,
    pool: Option<vk::QueryPool>,
}
impl Timer {
    pub fn new(cmd: vk::CommandBuffer, pool: Option<vk::QueryPool>) -> Self {
        Self { cmd, pool }
    }
    pub fn write(&self, point: Timestamp, stage: vk::PipelineStageFlags) {
        if let Some(pool) = self.pool {
            self.cmd.write_timestamp(stage, pool, point as u32);
        }
    }
    pub fn zero_duration(&self, start: Timestamp, end: Timestamp) {
        self.write(start, vk::PipelineStageFlags::BottomOfPipe);
        self.write(end, vk::PipelineStageFlags::BottomOfPipe);
    }
    /// Returns a drop-guard that automatically writes the end timestamp when it
    /// goes out of scope. Both stamps use BottomOfPipe: a TopOfPipe start
    /// stamps before preceding work drains, overlapping the scopes.
    pub fn scope<'a>(&'a self, start: Timestamp, end: Timestamp) -> TimerScope<'a> {
        self.write(start, vk::PipelineStageFlags::BottomOfPipe);
        TimerScope { timer: self, end }
    }
}
pub struct TimerScope<'a> {
    timer: &'a Timer,
    end: Timestamp,
}
impl TimerScope<'_> {
    pub fn end(self) {}
}
impl<'a> Drop for TimerScope<'a> {
    fn drop(&mut self) {
        self.timer
            .write(self.end, vk::PipelineStageFlags::BottomOfPipe);
    }
}
