//! Borrowed two-column read cache for collision scans.

use std::cell::RefCell;

use azalea_block::BlockState;
use parking_lot::RwLockReadGuard;

use super::chunk::{Chunk, ChunkStore};
use crate::position::ChunkPos;

type CachedColumn<'a> = Option<(ChunkPos, Option<RwLockReadGuard<'a, Chunk>>)>;

/// Retains at most two column read locks for a read-only pass. Missing
/// columns are cached too. Drop the reader before mutating columns.
pub(crate) struct BlockReader<'a> {
    store: &'a ChunkStore,
    columns: RefCell<[CachedColumn<'a>; 2]>,
}

impl<'a> BlockReader<'a> {
    /// Starts an empty cache borrowing `store` for the duration of the pass.
    pub fn new(store: &'a ChunkStore) -> Self {
        Self {
            store,
            columns: RefCell::new([None, None]),
        }
    }

    /// Reads without cloning a column Arc or reacquiring a cached lock.
    /// `None` distinguishes unloaded columns from loaded air/out-of-height.
    pub fn state(&self, x: i32, y: i32, z: i32) -> Option<BlockState> {
        let pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let mut columns = self.columns.borrow_mut();
        let hit = columns
            .iter()
            .position(|column| column.as_ref().is_some_and(|(p, _)| *p == pos));
        match hit {
            Some(0) => {}
            Some(_) => columns.swap(0, 1),
            None => {
                columns[1] = columns[0].take();
                columns[0] = Some((pos, self.store.borrow_chunk(&pos).map(|chunk| chunk.read())));
            }
        }
        columns[0]
            .as_ref()
            .and_then(|(_, chunk)| chunk.as_ref())
            .map(|chunk| chunk.block_state(x, y, z, self.store.min_y()))
    }

    /// Reads unloaded columns as air, matching `ChunkStore::get_block_state`.
    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.state(x, y, z).unwrap_or(BlockState::AIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockStateTable;

    /// Eviction, negative coordinates and unloaded cells match direct reads;
    /// dropping the cache releases its locks for subsequent mutations.
    #[test]
    fn borrowed_reader_matches_direct_reads() {
        let blocks = BlockStateTable::native();
        let stone = blocks.find_state("stone", &[]);
        let dirt = blocks.find_state("dirt", &[]);
        let mut store = ChunkStore::new(8);
        for pos in [
            ChunkPos::new(-1, -1),
            ChunkPos::new(0, 0),
            ChunkPos::new(1, 0),
        ] {
            store.insert_chunk(pos, Chunk::empty(store.section_count() as usize));
        }
        for (x, z) in [(-1, -1), (0, 0), (16, 0)] {
            store.set_block_state(x, 0, z, stone, blocks);
        }
        {
            let reader = BlockReader::new(&store);
            for (x, y, z) in [
                (-1, 0, -1),
                (0, 0, 0),
                (-1, 0, -1),
                (16, 0, 0),
                (0, 0, 0),
                (32, 0, 0),
                (32, 0, 0),
                (0, -65, 0),
                (0, 320, 0),
            ] {
                assert_eq!(
                    reader.get_block_state(x, y, z),
                    store.get_block_state(x, y, z)
                );
                assert_eq!(
                    reader.state(x, y, z).is_some(),
                    store.borrow_chunk(&ChunkPos::new(x >> 4, z >> 4)).is_some()
                );
            }
        }
        store.set_block_state(-1, 0, -1, dirt, blocks);
        assert_eq!(BlockReader::new(&store).get_block_state(-1, 0, -1), dirt);
    }
}
