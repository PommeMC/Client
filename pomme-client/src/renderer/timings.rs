use pyronyx::vk;
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timestamp {
    FrameStart = 0,
    FrameEnd,
    CullStart,
    CullEnd,
    GuiBakeStart,
    GuiBakeEnd,
    TerrainStart,
    TerrainEnd,
    EntitiesStart,
    EntitiesEnd,
    TranslucentStart,
    TranslucentEnd,
    UiStart,
    UiEnd,
    HizStart,
    HizEnd,
    VisibilityStart,
    VisibilityEnd,
    Count, // Automatically tracks the total number of timestamps needed
}
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderTimings {
    pub ticks: [u64; Timestamp::Count as usize],
    pub timestamp_period: f32,
}
impl RenderTimings {
    pub fn duration(&self, start: Timestamp, end: Timestamp) -> f32 {
        let diff_ticks = self.ticks[end as usize].saturating_sub(self.ticks[start as usize]);
        (diff_ticks as f64 * self.timestamp_period as f64 / 1_000_000.0) as f32
    }
    pub fn frame_ms(&self) -> f32 {
        self.duration(Timestamp::FrameStart, Timestamp::FrameEnd)
    }
    pub fn cull_ms(&self) -> f32 {
        self.duration(Timestamp::CullStart, Timestamp::CullEnd)
    }
    pub fn gui_bake_ms(&self) -> f32 {
        self.duration(Timestamp::GuiBakeStart, Timestamp::GuiBakeEnd)
    }
    pub fn terrain_ms(&self) -> f32 {
        self.duration(Timestamp::TerrainStart, Timestamp::TerrainEnd)
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
    pub fn hiz_ms(&self) -> f32 {
        self.duration(Timestamp::HizStart, Timestamp::HizEnd)
    }
    pub fn visibility_ms(&self) -> f32 {
        self.duration(Timestamp::VisibilityStart, Timestamp::VisibilityEnd)
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
    /// Returns a drop-guard that automatically writes the end timestamp when it
    /// goes out of scope.
    pub fn scope<'a>(&'a self, start: Timestamp, end: Timestamp) -> TimerScope<'a> {
        self.write(start, vk::PipelineStageFlags::TopOfPipe);
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
