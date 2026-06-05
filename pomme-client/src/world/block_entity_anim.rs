use std::collections::HashMap;

use azalea_core::position::BlockPos;

/// Open/close animation for containers whose lid swings (chest) or translates
/// (shulker box). One instance per BE position; the same shape is reused for
/// both kinds, with per-kind interpretation at render time.
#[derive(Default)]
pub struct ContainerAnim {
    /// 0.0 = closed, 1.0 = fully open. Cubic easing is applied at render time.
    pub openness: f32,
    pub opening: bool,
}

impl ContainerAnim {
    /// Vanilla `ChestBlockEntity` advances openness by 0.1 per tick.
    const RATE: f32 = 0.1;

    pub fn tick(&mut self) {
        let delta = if self.opening {
            Self::RATE
        } else {
            -Self::RATE
        };
        self.openness = (self.openness + delta).clamp(0.0, 1.0);
    }
}

#[derive(Default)]
pub struct BlockEntityAnimStore {
    containers: HashMap<BlockPos, ContainerAnim>,
}

impl BlockEntityAnimStore {
    pub fn tick(&mut self) {
        self.containers.retain(|_, anim| {
            anim.tick();
            // Drop fully-closed, non-opening entries to keep the map small.
            anim.opening || anim.openness > 0.0
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

    pub fn drop_chunk(&mut self, chunk_x: i32, chunk_z: i32) {
        self.containers
            .retain(|p, _| p.x.div_euclid(16) != chunk_x || p.z.div_euclid(16) != chunk_z);
    }
}
