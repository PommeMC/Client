//! Block-state access for light propagation: vanilla `LightEngine.getState`.
//! Positions in unloaded chunks read as bedrock so propagation stops at
//! loaded-area borders exactly like vanilla.

use azalea_block::BlockState;
use crossbeam_epoch as epoch;

use super::storage::LightPos;
use crate::world::chunk::{ChunkStore, block_state_from_section};

pub(crate) trait LightBlockGetter {
    fn state(&self, pos: LightPos) -> BlockState;

    /// Vanilla clears the engine's chunk cache between the decrease and
    /// increase passes; getters without a cache ignore this.
    fn clear_cache(&self) {}
}

/// [`ChunkStore`]-backed getter, constructed fresh per light-update run. Holds
/// one epoch guard for the whole run, so chunk reads are plain ring loads
/// (vanilla's 2-entry chunk cache existed to skip map/lock overhead the
/// lock-free store doesn't have).
pub(crate) struct StoreWorld<'a> {
    store: &'a ChunkStore,
    bedrock: BlockState,
    min_y: i32,
    guard: epoch::Guard,
}

impl<'a> StoreWorld<'a> {
    pub fn new(store: &'a ChunkStore) -> Self {
        Self {
            store,
            bedrock: crate::world::block::first_state_of("bedrock").unwrap_or(BlockState::AIR),
            min_y: store.min_y(),
            guard: epoch::pin(),
        }
    }
}

impl LightBlockGetter for StoreWorld<'_> {
    fn state(&self, pos: LightPos) -> BlockState {
        let chunk_pos = azalea_core::position::ChunkPos::new(pos.x >> 4, pos.z >> 4);
        let Some(chunk) = self.store.shared.get_chunk_guard(chunk_pos, &self.guard) else {
            return self.bedrock;
        };
        block_state_from_section(chunk, pos.x, pos.y, pos.z, self.min_y)
    }
}
