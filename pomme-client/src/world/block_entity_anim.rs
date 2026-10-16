use std::collections::HashMap;

use azalea_block::BlockState;
use azalea_core::position::BlockPos;

use crate::world::block::model::Direction;

/// Open/close animation for containers whose lid swings (chest) or translates
/// (shulker box). One instance per BE position; the same shape is reused for
/// both kinds, with per-kind interpretation at render time.
#[derive(Default)]
pub struct ContainerAnim {
    /// 0.0 = closed, 1.0 = fully open. Cubic easing is applied at render time.
    openness: f32,
    prev_openness: f32,
    pub opening: bool,
}

impl ContainerAnim {
    /// Vanilla `ChestBlockEntity` advances openness by 0.1 per tick.
    const RATE: f32 = 0.1;

    pub fn tick(&mut self) {
        self.prev_openness = self.openness;
        let delta = if self.opening {
            Self::RATE
        } else {
            -Self::RATE
        };
        self.openness = (self.openness + delta).clamp(0.0, 1.0);
    }

    /// Openness interpolated between the previous and current tick, matching
    /// vanilla `ChestLidController.getOpenness(partialTicks)`.
    pub fn openness(&self, partial_tick: f32) -> f32 {
        self.prev_openness + (self.openness - self.prev_openness) * partial_tick
    }
}

/// Dynamic interpolation state for moving pistons.
#[derive(Clone, Debug)]
pub struct MovingPistonAnim {
    pub moved_state: BlockState,
    pub facing: Direction,
    pub extending: bool,
    pub source: bool,
    pub progress: f32,
    pub prev_progress: f32,
}

impl MovingPistonAnim {
    pub fn tick(&mut self) {
        self.prev_progress = self.progress;
        self.progress = (self.progress + 0.5).min(1.0);
    }

    pub fn progress(&self, partial_tick: f32) -> f32 {
        self.prev_progress + (self.progress - self.prev_progress) * partial_tick
    }

    pub fn offset(&self, partial_tick: f32) -> glam::Vec3 {
        let p = self.progress(partial_tick);
        let amount = if self.extending { p - 1.0 } else { 1.0 - p };
        let (dx, dy, dz) = match self.facing {
            Direction::Down => (0.0, -1.0, 0.0),
            Direction::Up => (0.0, 1.0, 0.0),
            Direction::North => (0.0, 0.0, -1.0),
            Direction::South => (0.0, 0.0, 1.0),
            Direction::West => (-1.0, 0.0, 0.0),
            Direction::East => (1.0, 0.0, 0.0),
        };
        glam::Vec3::new(dx * amount, dy * amount, dz * amount)
    }
}

#[derive(Default)]
pub struct BlockEntityAnimStore {
    containers: HashMap<BlockPos, ContainerAnim>,
    moving_pistons: HashMap<BlockPos, MovingPistonAnim>,
}

impl BlockEntityAnimStore {
    pub fn tick(&mut self) {
        self.containers.retain(|_, anim| {
            anim.tick();
            // Drop fully-closed, non-opening entries to keep the map small;
            // keep the final closing tick so it still lerps to zero.
            anim.opening || anim.openness > 0.0 || anim.prev_openness > 0.0
        });
        self.moving_pistons.retain(|_, anim| {
            anim.tick();
            anim.prev_progress < 1.0
        });
    }

    /// Set the open-viewer count for a container. >0 starts opening, 0 starts
    /// closing.
    pub fn set_open_count(&mut self, pos: BlockPos, count: u8) {
        let entry = self.containers.entry(pos).or_default();
        entry.opening = count > 0;
    }

    pub fn container(&self, pos: &BlockPos) -> Option<&ContainerAnim> {
        self.containers.get(pos)
    }

    pub fn set_moving_piston(
        &mut self,
        pos: BlockPos,
        moved_state: BlockState,
        facing: Direction,
        extending: bool,
        source: bool,
        progress: f32,
    ) {
        let entry = self
            .moving_pistons
            .entry(pos)
            .or_insert_with(|| MovingPistonAnim {
                moved_state,
                facing,
                extending,
                source,
                progress,
                prev_progress: progress,
            });
        entry.moved_state = moved_state;
        entry.facing = facing;
        entry.extending = extending;
        entry.source = source;
    }

    pub fn moving_piston(&self, pos: &BlockPos) -> Option<&MovingPistonAnim> {
        self.moving_pistons.get(pos)
    }

    pub fn moving_pistons(&self) -> impl Iterator<Item = (&BlockPos, &MovingPistonAnim)> {
        self.moving_pistons.iter()
    }

    pub fn remove_moving_piston(&mut self, pos: &BlockPos) {
        self.moving_pistons.remove(pos);
    }

    pub fn drop_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        self.containers
            .retain(|p, _| p.x.div_euclid(16) != chunk_x || p.z.div_euclid(16) != chunk_z);
        self.moving_pistons
            .retain(|p, _| p.x.div_euclid(16) != chunk_x || p.z.div_euclid(16) != chunk_z);
    }
}
