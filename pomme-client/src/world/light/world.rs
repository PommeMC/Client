//! Block-state access for light propagation: vanilla `LightEngine.getState`
//! plus its 2-entry chunk cache. Positions in unloaded chunks read as bedrock
//! so propagation stops at loaded-area borders exactly like vanilla.

use std::cell::RefCell;
use std::sync::Arc;

use azalea_block::BlockState;
use parking_lot::RwLock;

use super::storage::LightPos;
use crate::world::block::BlockStateTable;
use crate::world::chunk::{Chunk, ChunkStore};

pub(crate) trait LightBlockGetter {
    fn block_state_table(&self) -> BlockStateTable;

    fn state(&self, pos: LightPos) -> BlockState;

    /// Vanilla clears the engine's chunk cache between the decrease and
    /// increase passes; getters without a cache ignore this.
    fn clear_cache(&self) {}
}

type CacheEntry = ((i32, i32), Option<Arc<RwLock<Chunk>>>);

/// [`ChunkStore`]-backed getter, constructed fresh per light-update run.
pub(crate) struct StoreWorld<'a> {
    block_state_table: BlockStateTable,
    store: &'a ChunkStore,
    bedrock: BlockState,
    min_y: i32,
    cache: RefCell<[CacheEntry; 2]>,
}

const INVALID_CHUNK: (i32, i32) = (i32::MAX, i32::MAX);

impl<'a> StoreWorld<'a> {
    pub fn new(store: &'a ChunkStore, block_state_table: BlockStateTable) -> Self {
        Self {
            block_state_table,
            store,
            bedrock: block_state_table
                .first_state_of("bedrock")
                .unwrap_or(BlockState::AIR),
            min_y: store.min_y(),
            cache: RefCell::new([(INVALID_CHUNK, None), (INVALID_CHUNK, None)]),
        }
    }

    fn chunk(&self, cx: i32, cz: i32) -> Option<Arc<RwLock<Chunk>>> {
        let mut cache = self.cache.borrow_mut();
        for (pos, chunk) in cache.iter() {
            if *pos == (cx, cz) {
                return chunk.clone();
            }
        }
        let chunk = self
            .store
            .get_chunk(&crate::position::ChunkPos::new(cx, cz));
        cache[1] = cache[0].clone();
        cache[0] = ((cx, cz), chunk.clone());
        chunk
    }
}

impl LightBlockGetter for StoreWorld<'_> {
    fn block_state_table(&self) -> BlockStateTable {
        self.block_state_table
    }

    fn state(&self, pos: LightPos) -> BlockState {
        let Some(chunk) = self.chunk(pos.x >> 4, pos.z >> 4) else {
            return self.bedrock;
        };
        let chunk = chunk.read();
        chunk.block_state(pos.x, pos.y, pos.z, self.min_y)
    }

    fn clear_cache(&self) {
        *self.cache.borrow_mut() = [(INVALID_CHUNK, None), (INVALID_CHUNK, None)];
    }
}
