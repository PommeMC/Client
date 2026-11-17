use std::time::{Duration, Instant};

use glam::{DVec2, DVec3};

use crate::position::BlockPos;

pub const DEFAULT_BORDER_SIZE: f64 = 29999984.0;
pub const DEFAULT_MAX_SIZE: f64 = 29999984.0;
pub const DEFAULT_WARNING_BLOCKS: f64 = 5.0;
pub const DEFAULT_WARNING_TIME_SECS: u32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum BorderStatus {
    Stationary,
    Shrinking,
    Growing,
}

#[derive(Clone, Copy, Debug)]
pub struct WorldBorder {
    pub center: DVec2,
    pub old_size: f64,
    pub new_size: f64,
    pub lerp_start: Instant,
    pub lerp_duration: Duration,
    pub max_size: f64,
    pub warning_blocks: f64,
    pub warning_time_secs: u32,
}

impl Default for WorldBorder {
    fn default() -> Self {
        Self {
            center: DVec2::ZERO,
            old_size: DEFAULT_BORDER_SIZE,
            new_size: DEFAULT_BORDER_SIZE,
            lerp_start: Instant::now(),
            lerp_duration: Duration::ZERO,
            max_size: DEFAULT_MAX_SIZE,
            warning_blocks: DEFAULT_WARNING_BLOCKS,
            warning_time_secs: DEFAULT_WARNING_TIME_SECS,
        }
    }
}

#[allow(dead_code)]
impl WorldBorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn size(&self, now: Instant) -> f64 {
        if self.lerp_duration.is_zero() {
            return self.new_size;
        }

        let elapsed = now.saturating_duration_since(self.lerp_start);
        if elapsed >= self.lerp_duration {
            return self.new_size;
        }

        let progress = elapsed.as_secs_f64() / self.lerp_duration.as_secs_f64();
        let clamped = progress.clamp(0.0, 1.0);
        self.old_size + (self.new_size - self.old_size) * clamped
    }

    pub fn status(&self, now: Instant) -> BorderStatus {
        if self.lerp_duration.is_zero() {
            return BorderStatus::Stationary;
        }

        let elapsed = now.saturating_duration_since(self.lerp_start);
        if elapsed >= self.lerp_duration {
            return BorderStatus::Stationary;
        }

        if self.new_size < self.old_size {
            BorderStatus::Shrinking
        } else if self.new_size > self.old_size {
            BorderStatus::Growing
        } else {
            BorderStatus::Stationary
        }
    }

    pub fn color(&self, now: Instant) -> [f32; 4] {
        match self.status(now) {
            BorderStatus::Stationary => [0.13, 0.55, 0.95, 0.60],
            BorderStatus::Shrinking => [0.95, 0.18, 0.18, 0.65],
            BorderStatus::Growing => [0.18, 0.92, 0.28, 0.60],
        }
    }

    pub fn min_x(&self, now: Instant) -> f64 {
        let half = self.size(now) * 0.5;
        (self.center.x - half).max(-self.max_size)
    }

    pub fn max_x(&self, now: Instant) -> f64 {
        let half = self.size(now) * 0.5;
        (self.center.x + half).min(self.max_size)
    }

    pub fn min_z(&self, now: Instant) -> f64 {
        let half = self.size(now) * 0.5;
        (self.center.y - half).max(-self.max_size)
    }

    pub fn max_z(&self, now: Instant) -> f64 {
        let half = self.size(now) * 0.5;
        (self.center.y + half).min(self.max_size)
    }

    pub fn is_inside(&self, pos: DVec3, now: Instant) -> bool {
        let min_x = self.min_x(now);
        let max_x = self.max_x(now);
        let min_z = self.min_z(now);
        let max_z = self.max_z(now);

        pos.x >= min_x && pos.x <= max_x && pos.z >= min_z && pos.z <= max_z
    }

    pub fn is_block_inside(&self, pos: BlockPos, now: Instant) -> bool {
        let min_x = self.min_x(now);
        let max_x = self.max_x(now);
        let min_z = self.min_z(now);
        let max_z = self.max_z(now);

        let bx = pos.x as f64;
        let bz = pos.z as f64;
        bx >= min_x && (bx + 1.0) <= max_x && bz >= min_z && (bz + 1.0) <= max_z
    }

    pub fn distance_to_border(&self, pos: DVec3, now: Instant) -> f64 {
        let min_x = self.min_x(now);
        let max_x = self.max_x(now);
        let min_z = self.min_z(now);
        let max_z = self.max_z(now);

        let dist_min_x = pos.x - min_x;
        let dist_max_x = max_x - pos.x;
        let dist_min_z = pos.z - min_z;
        let dist_max_z = max_z - pos.z;

        dist_min_x.min(dist_max_x).min(dist_min_z).min(dist_max_z)
    }

    pub fn warning_factor(&self, pos: DVec3, now: Instant) -> f32 {
        let dist = self.distance_to_border(pos, now);
        if dist <= 0.0 {
            return 1.0;
        }
        if self.warning_blocks <= 0.0 || dist >= self.warning_blocks {
            return 0.0;
        }

        let ratio = 1.0 - (dist / self.warning_blocks);
        ratio.clamp(0.0, 1.0) as f32
    }

    pub fn apply_initialize(
        &mut self,
        center_x: f64,
        center_z: f64,
        old_size: f64,
        new_size: f64,
        lerp_time_ms: u64,
        max_size: u32,
        warning_blocks: u32,
        warning_time: u32,
        now: Instant,
    ) {
        self.center = DVec2::new(center_x, center_z);
        self.old_size = old_size;
        self.new_size = new_size;
        self.lerp_start = now;
        self.lerp_duration = Duration::from_millis(lerp_time_ms);
        self.max_size = if max_size == 0 {
            DEFAULT_MAX_SIZE
        } else {
            max_size as f64
        };
        self.warning_blocks = warning_blocks as f64;
        self.warning_time_secs = warning_time;
    }

    pub fn apply_center(&mut self, center_x: f64, center_z: f64) {
        self.center = DVec2::new(center_x, center_z);
    }

    pub fn apply_size(&mut self, size: f64) {
        self.old_size = size;
        self.new_size = size;
        self.lerp_duration = Duration::ZERO;
    }

    pub fn apply_lerp_size(
        &mut self,
        old_size: f64,
        new_size: f64,
        lerp_time_ms: u64,
        now: Instant,
    ) {
        self.old_size = old_size;
        self.new_size = new_size;
        self.lerp_start = now;
        self.lerp_duration = Duration::from_millis(lerp_time_ms);
    }

    pub fn apply_warning_distance(&mut self, warning_blocks: u32) {
        self.warning_blocks = warning_blocks as f64;
    }

    pub fn apply_warning_delay(&mut self, warning_time: u32) {
        self.warning_time_secs = warning_time;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_border_is_centered_and_huge() {
        let border = WorldBorder::new();
        let now = Instant::now();
        assert_eq!(border.center, DVec2::ZERO);
        assert_eq!(border.size(now), DEFAULT_BORDER_SIZE);
        assert_eq!(border.status(now), BorderStatus::Stationary);
        assert!(border.is_inside(DVec3::new(0.0, 64.0, 0.0), now));
        assert!(border.is_inside(DVec3::new(1000.0, 64.0, -1000.0), now));
    }

    #[test]
    fn small_border_bounds_and_inside() {
        let mut border = WorldBorder::new();
        let now = Instant::now();
        border.apply_initialize(10.0, 20.0, 100.0, 100.0, 0, 29999984, 5, 15, now);
        assert_eq!(border.min_x(now), -40.0);
        assert_eq!(border.max_x(now), 60.0);
        assert_eq!(border.min_z(now), -30.0);
        assert_eq!(border.max_z(now), 70.0);

        assert!(border.is_inside(DVec3::new(10.0, 64.0, 20.0), now));
        assert!(border.is_inside(DVec3::new(-40.0, 64.0, -30.0), now));
        assert!(border.is_inside(DVec3::new(60.0, 64.0, 70.0), now));
        assert!(!border.is_inside(DVec3::new(61.0, 64.0, 20.0), now));
        assert!(!border.is_inside(DVec3::new(10.0, 64.0, -31.0), now));
    }

    #[test]
    fn lerp_size_and_status() {
        let mut border = WorldBorder::new();
        let start = Instant::now();
        border.apply_lerp_size(100.0, 50.0, 1000, start);
        assert_eq!(border.status(start), BorderStatus::Shrinking);

        let half = start + Duration::from_millis(500);
        assert!((border.size(half) - 75.0).abs() < 1e-4);
        assert_eq!(border.status(half), BorderStatus::Shrinking);

        let end = start + Duration::from_millis(1000);
        assert_eq!(border.size(end), 50.0);
        assert_eq!(border.status(end), BorderStatus::Stationary);
    }

    #[test]
    fn warning_factor_near_border() {
        let mut border = WorldBorder::new();
        let now = Instant::now();
        border.apply_initialize(0.0, 0.0, 20.0, 20.0, 0, 29999984, 5, 15, now);
        assert_eq!(border.warning_factor(DVec3::new(0.0, 64.0, 0.0), now), 0.0);
        assert_eq!(border.warning_factor(DVec3::new(5.0, 64.0, 0.0), now), 0.0);
        assert!((border.warning_factor(DVec3::new(7.5, 64.0, 0.0), now) - 0.5).abs() < 1e-4);
        assert_eq!(border.warning_factor(DVec3::new(10.0, 64.0, 0.0), now), 1.0);
        assert_eq!(border.warning_factor(DVec3::new(11.0, 64.0, 0.0), now), 1.0);
    }
}
