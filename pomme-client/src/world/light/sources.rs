//! Vanilla `ChunkSkyLightSources`: per-column "lowest Y that still sees full
//! sky", maintained incrementally on block changes and consumed by the sky
//! engine's source add/remove passes.
//!
//! Vanilla packs offsets into a `SimpleBitStorage`; this stores absolute Y
//! per column, same semantics (including the `min_y` slot doubling as the
//! open-column sentinel that reads as `i32::MIN`).

use azalea_block::BlockState;
use azalea_world::Chunk;

use crate::world::block;

pub(crate) struct ChunkSkyLightSources {
    /// `level.getMinY() - 1`; the stored value for fully open columns.
    min_y: i32,
    /// Lowest full-sky Y per column, indexed `x + z * 16`.
    heightmap: Box<[i32; 256]>,
}

impl ChunkSkyLightSources {
    pub fn new(world_min_y: i32) -> Self {
        let min_y = world_min_y - 1;
        Self {
            min_y,
            heightmap: Box::new([min_y; 256]),
        }
    }

    /// Initial build from a loaded chunk (vanilla `fillFrom`).
    pub fn fill_from(world_min_y: i32, chunk: &Chunk) -> Self {
        let mut sources = Self::new(world_min_y);
        let Some(top_index) = chunk.sections.iter().rposition(|s| s.block_count > 0) else {
            return sources;
        };
        for z in 0..16 {
            for x in 0..16 {
                let y = sources
                    .find_lowest_source_y(chunk, top_index, x, z)
                    .max(sources.min_y);
                sources.heightmap[Self::index(x, z)] = y;
            }
        }
        sources
    }

    fn find_lowest_source_y(&self, chunk: &Chunk, top_index: usize, x: i32, z: i32) -> i32 {
        let min_section_y = (self.min_y + 1) >> 4;
        let mut top_state = BlockState::AIR;
        let mut top_y = (min_section_y + top_index as i32 + 1) << 4;
        for index in (0..=top_index).rev() {
            let section = &chunk.sections[index];
            if section.block_count == 0 {
                top_state = BlockState::AIR;
                top_y = (min_section_y + index as i32) << 4;
                continue;
            }
            for y in (0..16).rev() {
                let bottom_state =
                    section.get_block_state(azalea_core::position::ChunkSectionBlockPos {
                        x: x as u8,
                        y: y as u8,
                        z: z as u8,
                    });
                if Self::is_edge_occluded(top_state, bottom_state) {
                    return top_y;
                }
                top_state = bottom_state;
                top_y = ((min_section_y + index as i32) << 4) + y;
            }
        }
        self.min_y
    }

    /// Incremental update after the block at (local x, world y, local z)
    /// changed light properties; `column_state` reads this column's block
    /// state by world Y. Returns whether the heightmap changed.
    pub fn update(
        &mut self,
        column_state: &impl Fn(i32) -> BlockState,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        let index = Self::index(x, z);
        let current = self.heightmap[index];
        if y + 1 < current {
            return false;
        }
        let top_state = column_state(y + 1);
        let middle_state = column_state(y);
        if self.update_edge(column_state, index, current, y + 1, top_state, middle_state) {
            return true;
        }
        let bottom_state = column_state(y - 1);
        self.update_edge(column_state, index, current, y, middle_state, bottom_state)
    }

    fn update_edge(
        &mut self,
        column_state: &impl Fn(i32) -> BlockState,
        index: usize,
        old_top_edge_y: i32,
        checked_edge_y: i32,
        top_state: BlockState,
        bottom_state: BlockState,
    ) -> bool {
        if Self::is_edge_occluded(top_state, bottom_state) {
            if checked_edge_y > old_top_edge_y {
                self.heightmap[index] = checked_edge_y;
                return true;
            }
        } else if checked_edge_y == old_top_edge_y {
            self.heightmap[index] =
                self.find_lowest_source_below(column_state, checked_edge_y - 1, bottom_state);
            return true;
        }
        false
    }

    fn find_lowest_source_below(
        &self,
        column_state: &impl Fn(i32) -> BlockState,
        start_y: i32,
        start_state: BlockState,
    ) -> i32 {
        let mut top_state = start_state;
        let mut top_y = start_y;
        let mut bottom_y = start_y - 1;
        while bottom_y >= self.min_y {
            let bottom_state = column_state(bottom_y);
            if Self::is_edge_occluded(top_state, bottom_state) {
                return top_y;
            }
            top_state = bottom_state;
            top_y = bottom_y;
            bottom_y -= 1;
        }
        self.min_y
    }

    /// Whether full sky stops between a block and the one below it.
    fn is_edge_occluded(top_state: BlockState, bottom_state: BlockState) -> bool {
        block::light_props(bottom_state).dampening != 0
            || block::shape_occludes(top_state, bottom_state, 0 /* DOWN */)
    }

    /// Lowest full-sky Y for the column, `i32::MIN` when open below world.
    pub fn lowest_source_y(&self, x: i32, z: i32) -> i32 {
        self.extend_below_world(self.heightmap[Self::index(x, z)])
    }

    pub fn highest_lowest_source_y(&self) -> i32 {
        self.extend_below_world(*self.heightmap.iter().max().expect("non-empty"))
    }

    fn extend_below_world(&self, value: i32) -> i32 {
        if value == self.min_y { i32::MIN } else { value }
    }

    fn index(x: i32, z: i32) -> usize {
        (x + z * 16) as usize
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::world::block::find_state;

    fn column(blocks: &HashMap<i32, BlockState>) -> impl Fn(i32) -> BlockState + '_ {
        |y| blocks.get(&y).copied().unwrap_or(BlockState::AIR)
    }

    #[test]
    fn update_tracks_placed_and_removed_occluders() {
        crate::world::block::init("26.2");
        let stone = find_state("stone", &[]);
        let mut blocks: HashMap<i32, BlockState> = HashMap::new();
        let mut sources = ChunkSkyLightSources::new(-64);
        assert_eq!(sources.lowest_source_y(3, 5), i32::MIN);

        blocks.insert(64, stone);
        assert!(sources.update(&column(&blocks), 3, 64, 5));
        assert_eq!(sources.lowest_source_y(3, 5), 65);

        // A second occluder below doesn't move the edge...
        blocks.insert(60, stone);
        assert!(!sources.update(&column(&blocks), 3, 60, 5));
        assert_eq!(sources.lowest_source_y(3, 5), 65);

        // ...until the top one breaks and the rescan finds it.
        blocks.remove(&64);
        assert!(sources.update(&column(&blocks), 3, 64, 5));
        assert_eq!(sources.lowest_source_y(3, 5), 61);

        // Breaking the last occluder opens the column below the world.
        blocks.remove(&60);
        assert!(sources.update(&column(&blocks), 3, 60, 5));
        assert_eq!(sources.lowest_source_y(3, 5), i32::MIN);
        // Other columns were never touched.
        assert_eq!(sources.lowest_source_y(4, 5), i32::MIN);
    }
}
