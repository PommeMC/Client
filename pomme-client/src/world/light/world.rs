//! Block-state access for light propagation: vanilla `LightEngine.getState`
//! plus its 2-entry chunk cache, over the chunks the light thread was handed.
//! Positions in unloaded chunks read as bedrock so propagation stops at
//! loaded-area borders exactly like vanilla.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use parking_lot::RwLock;

use super::storage::LightPos;
use crate::world::block::BlockStateTable;
use crate::world::chunk::Chunk;

pub(crate) trait LightBlockGetter {
    fn block_state_table(&self) -> BlockStateTable;

    fn state(&self, pos: LightPos) -> BlockState;

    /// Vanilla clears the engine's chunk cache between the decrease and
    /// increase passes; getters without a cache ignore this.
    fn clear_cache(&self) {}
}

type CacheEntry = ((i32, i32), Option<Arc<RwLock<Chunk>>>);

/// The light thread's view of the level: the loaded chunks, shared with the
/// game thread that edits them.
pub(crate) struct LightWorld {
    block_state_table: BlockStateTable,
    chunks: HashMap<(i32, i32), Arc<RwLock<Chunk>>>,
    bedrock: BlockState,
    min_y: i32,
    cache: RefCell<[CacheEntry; 2]>,
}

const INVALID_CHUNK: (i32, i32) = (i32::MAX, i32::MAX);

impl LightWorld {
    pub fn new(block_state_table: BlockStateTable, min_y: i32) -> Self {
        Self {
            block_state_table,
            chunks: HashMap::new(),
            bedrock: block_state_table
                .first_state_of("bedrock")
                .unwrap_or(BlockState::AIR),
            min_y,
            cache: RefCell::new([(INVALID_CHUNK, None), (INVALID_CHUNK, None)]),
        }
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }

    pub fn insert(&mut self, pos: (i32, i32), chunk: Arc<RwLock<Chunk>>) {
        self.chunks.insert(pos, chunk);
        self.clear_cache();
    }

    pub fn remove(&mut self, pos: (i32, i32)) {
        self.chunks.remove(&pos);
        self.clear_cache();
    }

    pub fn chunk(&self, pos: (i32, i32)) -> Option<Arc<RwLock<Chunk>>> {
        let mut cache = self.cache.borrow_mut();
        for (cached, chunk) in cache.iter() {
            if *cached == pos {
                return chunk.clone();
            }
        }
        let chunk = self.chunks.get(&pos).cloned();
        cache[1] = cache[0].clone();
        cache[0] = (pos, chunk.clone());
        chunk
    }

    /// Vanilla `LevelChunk.getSection(...).hasOnlyAir()`; unloaded reads as
    /// empty.
    pub fn section_is_empty(&self, pos: (i32, i32), section_y: i32) -> bool {
        let Some(chunk) = self.chunk(pos) else {
            return true;
        };
        let index = section_y - (self.min_y >> 4);
        let chunk = chunk.read();
        match usize::try_from(index)
            .ok()
            .and_then(|i| chunk.sections.get(i))
        {
            Some(section) => section.block_count == 0,
            None => true,
        }
    }
}

impl LightBlockGetter for LightWorld {
    fn block_state_table(&self) -> BlockStateTable {
        self.block_state_table
    }

    fn state(&self, pos: LightPos) -> BlockState {
        let Some(chunk) = self.chunk((pos.x >> 4, pos.z >> 4)) else {
            return self.bedrock;
        };
        let chunk = chunk.read();
        chunk.block_state(pos.x, pos.y, pos.z, self.min_y)
    }

    fn clear_cache(&self) {
        *self.cache.borrow_mut() = [(INVALID_CHUNK, None), (INVALID_CHUNK, None)];
    }
}
