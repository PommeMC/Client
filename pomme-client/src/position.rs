//! Block and chunk coordinates (vanilla `BlockPos` and `ChunkPos`).

use crate::world::block::model::Direction;

/// A block's integer world position.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Vanilla `BlockPos.relative`: the neighbour one step towards
    /// `direction`.
    pub fn relative(self, direction: Direction) -> Self {
        let [x, y, z] = direction.offset();
        Self::new(self.x + x, self.y + y, self.z + z)
    }
}

/// A chunk column's position: block coordinates floor-divided by 16.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

// TODO: drop this once the serverbound packets carrying positions are
// pomme-encoded.
impl From<BlockPos> for azalea_core::position::BlockPos {
    fn from(pos: BlockPos) -> Self {
        Self::new(pos.x, pos.y, pos.z)
    }
}
