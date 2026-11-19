//! Vanilla `WorldBorder`: the square the server keeps players in, which the
//! client collides with, draws and warns about.

use glam::{DVec3, dvec3};

use crate::physics::aabb::Aabb;
use crate::position::BlockPos;

/// How far the collision boxes reach past the border, standing in for
/// vanilla's infinite shape.
const BOX_EXTENT: f64 = 3.0e7;

/// Vanilla `BorderStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStatus {
    Growing,
    Shrinking,
    Stationary,
}

impl BorderStatus {
    /// `BorderStatus.getColor`, as RGB.
    pub fn color(self) -> [f32; 3] {
        let rgb: u32 = match self {
            Self::Growing => 0x40FF80,
            Self::Shrinking => 0xFF3030,
            Self::Stationary => 0x20A0FF,
        };
        [16, 8, 0].map(|shift| ((rgb >> shift) & 0xFF) as f32 / 255.0)
    }
}

/// `ClientboundInitializeBorderPacket`'s fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBorderInit {
    pub center_x: f64,
    pub center_z: f64,
    pub old_size: f64,
    pub new_size: f64,
    pub lerp_ticks: i64,
    pub absolute_max_size: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
}

/// The border's four sides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl Bounds {
    /// `getDistanceToBorder`: to the nearest side, negative outside.
    pub fn distance_to(&self, x: f64, z: f64) -> f64 {
        let from_north = z - self.min_z;
        let from_south = self.max_z - z;
        let from_west = x - self.min_x;
        let from_east = self.max_x - x;
        from_west.min(from_east).min(from_north).min(from_south)
    }
}

/// `WorldBorderRenderState`: the border as a frame draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderRenderState {
    pub bounds: Bounds,
    pub tint: [f32; 3],
}

/// `WorldBorder.BorderExtent`: a border at rest, or one moving to a new size
/// over a number of game ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Extent {
    Static {
        size: f64,
    },
    Moving {
        from: f64,
        to: f64,
        lerp_duration: i64,
        lerp_progress: i64,
        size: f64,
        previous_size: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldBorder {
    center_x: f64,
    center_z: f64,
    absolute_max_size: i32,
    warning_blocks: i32,
    warning_time: i32,
    extent: Extent,
}

impl Default for WorldBorder {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            absolute_max_size: 29999984,
            warning_blocks: 5,
            warning_time: 15,
            extent: Extent::Static { size: 5.9999968E7 },
        }
    }
}

impl WorldBorder {
    /// `ClientPacketListener.handleInitializeBorder`.
    pub fn initialize(&mut self, init: WorldBorderInit) {
        self.set_center(init.center_x, init.center_z);
        if init.lerp_ticks > 0 {
            self.lerp_size_between(init.old_size, init.new_size, init.lerp_ticks);
        } else {
            self.set_size(init.new_size);
        }
        self.absolute_max_size = init.absolute_max_size;
        self.warning_blocks = init.warning_blocks;
        self.warning_time = init.warning_time;
    }

    pub fn set_center(&mut self, x: f64, z: f64) {
        self.center_x = x;
        self.center_z = z;
    }

    pub fn set_size(&mut self, size: f64) {
        self.extent = Extent::Static { size };
    }

    /// `lerpSizeBetween`: the size moves from `from` to `to` over `ticks`
    /// game ticks.
    pub fn lerp_size_between(&mut self, from: f64, to: f64, ticks: i64) {
        self.extent = if from == to {
            Extent::Static { size: to }
        } else {
            Extent::moving(from, to, ticks, ticks, from)
        };
    }

    pub fn set_warning_blocks(&mut self, blocks: i32) {
        self.warning_blocks = blocks;
    }

    pub fn set_warning_time(&mut self, time: i32) {
        self.warning_time = time;
    }

    /// `WorldBorder.tick`, once a game tick.
    pub fn tick(&mut self) {
        if let Extent::Moving {
            from,
            to,
            lerp_duration,
            lerp_progress,
            size,
            ..
        } = self.extent
        {
            self.extent = if lerp_progress <= 1 {
                Extent::Static { size: to }
            } else {
                Extent::moving(from, to, lerp_duration, lerp_progress - 1, size)
            };
        }
    }

    pub fn status(&self) -> BorderStatus {
        match self.extent {
            Extent::Static { .. } => BorderStatus::Stationary,
            Extent::Moving { from, to, .. } if to < from => BorderStatus::Shrinking,
            Extent::Moving { .. } => BorderStatus::Growing,
        }
    }

    /// The size `partial_tick` of the way from the last tick to this one.
    fn size_at(&self, partial_tick: f32) -> f64 {
        match self.extent {
            Extent::Static { size } => size,
            Extent::Moving {
                size,
                previous_size,
                ..
            } => previous_size + f64::from(partial_tick) * (size - previous_size),
        }
    }

    /// `getMinX` and its three siblings `partial_tick` into the tick: half
    /// the size either side of the centre, within the absolute maximum.
    pub fn bounds(&self, partial_tick: f32) -> Bounds {
        let half = self.size_at(partial_tick) / 2.0;
        let max = f64::from(self.absolute_max_size);
        let side = |value: f64| value.clamp(-max, max);
        Bounds {
            min_x: side(self.center_x - half),
            max_x: side(self.center_x + half),
            min_z: side(self.center_z - half),
            max_z: side(self.center_z + half),
        }
    }

    /// `WorldBorderRenderer.extract`.
    pub fn render_state(&self, partial_tick: f32) -> BorderRenderState {
        BorderRenderState {
            bounds: self.bounds(partial_tick),
            tint: self.status().color(),
        }
    }

    /// `isWithinBounds(x, z, margin)`.
    fn is_within_bounds(&self, x: f64, z: f64, margin: f64) -> bool {
        let bounds = self.bounds(0.0);
        x >= bounds.min_x - margin
            && x < bounds.max_x + margin
            && z >= bounds.min_z - margin
            && z < bounds.max_z + margin
    }

    /// `isWithinBounds(BlockPos)`: the block's low corner is inside.
    pub fn contains_block(&self, pos: BlockPos) -> bool {
        self.is_within_bounds(f64::from(pos.x), f64::from(pos.z), 0.0)
    }

    pub fn distance_to_border(&self, x: f64, z: f64) -> f64 {
        self.bounds(0.0).distance_to(x, z)
    }

    /// `isInsideCloseToBorder` for an entity at `position` with `bounding_box`.
    fn is_inside_close_to_border(&self, position: DVec3, bounding_box: &Aabb) -> bool {
        let size = bounding_box.max - bounding_box.min;
        let bb_max = size.x.abs().max(size.z.abs()).max(1.0);
        self.distance_to_border(position.x, position.z) < bb_max * 2.0
            && self.is_within_bounds(position.x, position.z, bb_max)
    }

    /// `getCollisionShape` for an entity `Entity.collectAllColliders` finds
    /// inside and close to the border: everything outside the border's whole
    /// blocks, as four boxes. `None` for any other entity.
    pub fn collision_boxes(&self, position: DVec3, bounding_box: &Aabb) -> Option<[Aabb; 4]> {
        if !self.is_inside_close_to_border(position, bounding_box) {
            return None;
        }
        let bounds = self.bounds(0.0);
        let (min_x, max_x) = (bounds.min_x.floor(), bounds.max_x.ceil());
        let (min_z, max_z) = (bounds.min_z.floor(), bounds.max_z.ceil());
        let far = BOX_EXTENT;
        let wall = |min: (f64, f64), max: (f64, f64)| {
            Aabb::new(dvec3(min.0, -far, min.1), dvec3(max.0, far, max.1))
        };
        Some([
            wall((min_x - far, min_z - far), (min_x, max_z + far)),
            wall((max_x, min_z - far), (max_x + far, max_z + far)),
            wall((min_x - far, min_z - far), (max_x + far, min_z)),
            wall((min_x - far, max_z), (max_x + far, max_z + far)),
        ])
    }

    /// `getLerpSpeed`: blocks of size a tick.
    fn lerp_speed(&self) -> f64 {
        match self.extent {
            Extent::Static { .. } => 0.0,
            Extent::Moving {
                from,
                to,
                lerp_duration,
                ..
            } => (from - to).abs() / lerp_duration as f64,
        }
    }

    /// `Hud.extractVignette`'s border warning for a camera at `(x, z)`: 0 away
    /// from the border, rising to 1 at it. The warning distance grows to what
    /// a moving border covers in the warning time.
    pub fn warning_strength(&self, x: f64, z: f64) -> f32 {
        let (size, target) = match self.extent {
            Extent::Static { size } => (size, size),
            Extent::Moving { size, to, .. } => (size, to),
        };
        let distance = self.distance_to_border(x, z) as f32;
        let moving_threshold =
            (self.lerp_speed() * f64::from(self.warning_time)).min((target - size).abs());
        let warning_distance = f64::from(self.warning_blocks).max(moving_threshold);
        if f64::from(distance) < warning_distance {
            (1.0 - (f64::from(distance) / warning_distance) as f32).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

impl Extent {
    /// A moving border `lerp_progress` ticks from its end, which was
    /// `previous_size` wide a tick ago.
    fn moving(
        from: f64,
        to: f64,
        lerp_duration: i64,
        lerp_progress: i64,
        previous_size: f64,
    ) -> Self {
        let duration = lerp_duration as f64;
        // `MovingBorderExtent.calculateSize`.
        let progress = (duration - lerp_progress as f64) / duration;
        let size = if progress < 1.0 {
            from + progress * (to - from)
        } else {
            to
        };
        Self::Moving {
            from,
            to,
            lerp_duration,
            lerp_progress,
            size,
            previous_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn border(center: (f64, f64), size: f64) -> WorldBorder {
        let mut border = WorldBorder::default();
        border.set_center(center.0, center.1);
        border.set_size(size);
        border
    }

    fn player_box(x: f64) -> (DVec3, Aabb) {
        let position = dvec3(x, 64.0, 0.0);
        let half = dvec3(0.3, 0.0, 0.3);
        (
            position,
            Aabb::new(position - half, position + half + dvec3(0.0, 1.8, 0.0)),
        )
    }

    #[test]
    fn the_default_border_is_vanillas() {
        let border = WorldBorder::default();
        assert_eq!(border.status(), BorderStatus::Stationary);
        // The size is wider than the absolute maximum clamps it to.
        assert_eq!(border.bounds(0.0).max_x, 29999984.0);
        assert_eq!(border.bounds(0.0).min_z, -29999984.0);
        assert!(border.contains_block(BlockPos::new(1000, 64, -1000)));
    }

    #[test]
    fn bounds_follow_the_centre_and_size() {
        let border = border((10.0, 20.0), 100.0);
        assert_eq!(
            border.bounds(0.0),
            Bounds {
                min_x: -40.0,
                max_x: 60.0,
                min_z: -30.0,
                max_z: 70.0
            }
        );
        // The low corner counts; the high side is exclusive.
        assert!(border.contains_block(BlockPos::new(-40, 0, -30)));
        assert!(border.contains_block(BlockPos::new(59, 0, 69)));
        assert!(!border.contains_block(BlockPos::new(60, 0, 20)));
        assert_eq!(border.distance_to_border(10.0, 20.0), 50.0);
        assert_eq!(border.distance_to_border(61.0, 20.0), -1.0);
    }

    #[test]
    fn a_lerp_moves_once_a_tick_and_ends_at_rest() {
        let mut border = WorldBorder::default();
        border.lerp_size_between(100.0, 50.0, 10);
        assert_eq!(border.status(), BorderStatus::Shrinking);
        assert_eq!(border.bounds(0.0).max_x, 50.0);

        border.tick();
        assert_eq!(border.bounds(1.0).max_x, 47.5);
        // Half way through the tick the drawn side is half way there.
        assert_eq!(border.bounds(0.5).max_x, 48.75);

        for _ in 0..8 {
            border.tick();
        }
        assert_eq!(border.status(), BorderStatus::Shrinking);
        border.tick();
        assert_eq!(border.status(), BorderStatus::Stationary);
        assert_eq!(border.bounds(0.0).max_x, 25.0);

        border.lerp_size_between(50.0, 100.0, 10);
        assert_eq!(border.status(), BorderStatus::Growing);
        border.lerp_size_between(50.0, 50.0, 10);
        assert_eq!(border.status(), BorderStatus::Stationary);
    }

    #[test]
    fn initializing_takes_every_field_and_lerps_only_with_ticks() {
        let init = WorldBorderInit {
            center_x: 5.0,
            center_z: -5.0,
            old_size: 40.0,
            new_size: 20.0,
            lerp_ticks: 0,
            absolute_max_size: 1000,
            warning_blocks: 3,
            warning_time: 100,
        };
        let mut border = WorldBorder::default();
        border.initialize(init);
        assert_eq!(border.status(), BorderStatus::Stationary);
        assert_eq!(border.bounds(0.0).max_x, 15.0);

        border.initialize(WorldBorderInit {
            lerp_ticks: 20,
            ..init
        });
        assert_eq!(border.status(), BorderStatus::Shrinking);
        assert_eq!(border.bounds(0.0).max_x, 25.0);

        border.set_size(5000.0);
        assert_eq!(border.bounds(0.0).max_x, 1000.0);
    }

    #[test]
    fn collision_boxes_sit_on_the_whole_blocks_around_the_border() {
        let border = border((0.0, 0.0), 25.0);
        let (position, bounding_box) = player_box(12.0);
        let boxes = border.collision_boxes(position, &bounding_box).unwrap();
        // 12.5 rounds out to 13: west, east, north, south.
        assert_eq!(boxes[0].max.x, -13.0);
        assert_eq!(boxes[1].min.x, 13.0);
        assert_eq!(boxes[2].max.z, -13.0);
        assert_eq!(boxes[3].min.z, 13.0);
    }

    #[test]
    fn only_an_entity_inside_and_close_collides_with_the_border() {
        let border = border((0.0, 0.0), 20.0);
        let collides = |x: f64| {
            let (position, bounding_box) = player_box(x);
            border.collision_boxes(position, &bounding_box).is_some()
        };
        assert!(!collides(0.0));
        assert!(collides(8.5));
        // Just outside still collides, within the box's width of the side.
        assert!(collides(10.5));
        assert!(!collides(15.0));
    }

    #[test]
    fn the_border_stops_a_move_out_and_lets_one_back_in() {
        use crate::entity::components::Velocity;
        use crate::physics::block_shape::CollisionContext;
        use crate::physics::collision::{CollisionWorld, resolve_collision};
        use crate::world::block::BlockStateTable;
        use crate::world::chunk::ChunkStore;

        let border = border((0.0, 0.0), 25.0);
        let (position, bounding_box) = player_box(12.5);
        let boxes = border.collision_boxes(position, &bounding_box).unwrap();
        let chunks = ChunkStore::new(2);
        let world =
            CollisionWorld::new(&chunks, BlockStateTable::native()).with_extra_boxes(&boxes);
        let resolve = |dx: f64| {
            resolve_collision(
                world,
                bounding_box,
                Velocity::new(dx, 0.0, 0.0),
                0.6,
                true,
                &CollisionContext::EMPTY,
            )
            .0
            .x
        };
        // The box's east side is at 12.8, two tenths short of the wall at 13.
        assert!((resolve(1.0) - 0.2).abs() < 1.0e-9);
        assert_eq!(resolve(-1.0), -1.0);
    }

    #[test]
    fn the_warning_rises_toward_the_border_and_widens_while_it_moves() {
        let mut border = border((0.0, 0.0), 20.0);
        border.set_warning_blocks(5);
        assert_eq!(border.warning_strength(0.0, 0.0), 0.0);
        assert_eq!(border.warning_strength(5.0, 0.0), 0.0);
        assert_eq!(border.warning_strength(7.5, 0.0), 0.5);
        assert_eq!(border.warning_strength(10.0, 0.0), 1.0);
        assert_eq!(border.warning_strength(11.0, 0.0), 1.0);

        // Shrinking 80 blocks in 100 ticks with 50 ticks of warning: 40
        // blocks of warning, so 20 from the side is half way in.
        border.set_warning_time(50);
        border.lerp_size_between(100.0, 20.0, 100);
        assert_eq!(border.warning_strength(30.0, 0.0), 0.5);
    }

    #[test]
    fn status_colours_are_vanillas() {
        assert_eq!(
            BorderStatus::Shrinking.color(),
            [1.0, 48.0 / 255.0, 48.0 / 255.0]
        );
        assert_eq!(BorderStatus::Stationary.color()[2], 1.0);
    }
}
