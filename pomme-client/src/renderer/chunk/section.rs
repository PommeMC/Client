use azalea_block::BlockState;
use azalea_core::position::{
    ChunkPos, ChunkSectionBiomePos, ChunkSectionBlockPos, ChunkSectionPos,
};
use azalea_registry::data::Biome;
use azalea_world::{Chunk, Section};
use crossbeam_epoch::{self as epoch};

use crate::world::chunk::{ChunkLightData, SharedChunkStore};

/// The section at world section-y within `chunk`, or `None` when the column is
/// absent or the y falls outside the built sections.
#[inline]
fn section_at(chunk: Option<&Chunk>, section_y: i32) -> Option<&Section> {
    let idx = usize::try_from(section_y).ok()?;
    chunk?.sections.get(idx)
}

pub struct LocalSection {
    pub blocks: [[[BlockState; 18]; 18]; 18],
    pub biomes: [[[Biome; 6]; 6]; 6],
    pub light: [[[u8; 18]; 18]; 18],
}

impl LocalSection {
    pub fn new_boxed(shared: &SharedChunkStore, spos: ChunkSectionPos) -> Box<Self> {
        let mut section = Box::new(Self {
            blocks: [[[BlockState::AIR; 18]; 18]; 18],
            biomes: [[[Biome::default(); 6]; 6]; 6],
            light: [[[0; 18]; 18]; 18],
        });
        section.build(shared, spos);
        section
    }

    #[inline]
    fn build(&mut self, shared: &SharedChunkStore, spos: ChunkSectionPos) {
        let base_y = spos.y - shared.min_section_y();
        let guard = epoch::pin();
        let chunk_grid: [[Option<&Chunk>; 3]; 3] = std::array::from_fn(|x| {
            std::array::from_fn(|z| {
                let pos = ChunkPos {
                    x: spos.x + (x as i32) - 1,
                    z: spos.z + (z as i32) - 1,
                };
                shared.get_chunk_guard(pos, &guard)
            })
        });
        let light_grid: [[Option<&ChunkLightData>; 3]; 3] = std::array::from_fn(|x| {
            std::array::from_fn(|z| {
                let pos = ChunkPos {
                    x: spos.x + (x as i32) - 1,
                    z: spos.z + (z as i32) - 1,
                };
                shared.get_light_guard(pos, &guard)
            })
        });
        for lx in -1i32..17 {
            for ly in -1i32..17 {
                for lz in -1i32..17 {
                    let chunk = chunk_grid[(lx.div_euclid(16) + 1) as usize]
                        [(lz.div_euclid(16) + 1) as usize];
                    let section_y = base_y + ly.div_euclid(16);
                    self.blocks[(lx + 1) as usize][(ly + 1) as usize][(lz + 1) as usize] =
                        section_at(chunk, section_y)
                            .map(|s| {
                                s.get_block_state(ChunkSectionBlockPos {
                                    x: lx.rem_euclid(16) as u8,
                                    y: ly.rem_euclid(16) as u8,
                                    z: lz.rem_euclid(16) as u8,
                                })
                            })
                            .unwrap_or_default();
                }
            }
        }
        for bx in -1i32..5 {
            for by in -1i32..5 {
                for bz in -1i32..5 {
                    let chunk = chunk_grid[(bx.div_euclid(4) + 1) as usize]
                        [(bz.div_euclid(4) + 1) as usize];
                    let section_y = base_y + by.div_euclid(4);
                    self.biomes[(bx + 1) as usize][(by + 1) as usize][(bz + 1) as usize] =
                        section_at(chunk, section_y)
                            .map(|s| {
                                s.get_biome(ChunkSectionBiomePos {
                                    x: bx.rem_euclid(4) as u8,
                                    y: by.rem_euclid(4) as u8,
                                    z: bz.rem_euclid(4) as u8,
                                })
                            })
                            .unwrap_or_default();
                }
            }
        }
        for lx in -1i32..17 {
            for ly in -1i32..17 {
                for lz in -1i32..17 {
                    let cx = (lx.div_euclid(16) + 1) as usize;
                    let cz = (lz.div_euclid(16) + 1) as usize;
                    let rx = lx.rem_euclid(16);
                    let ry = spos.y * 16 + ly;
                    let rz = lz.rem_euclid(16);

                    let sky = light_grid[cx][cz]
                        .map(|l| l.get_sky_light(rx, ry, rz))
                        .unwrap_or(15);
                    let block = light_grid[cx][cz]
                        .map(|l| l.get_block_light(rx, ry, rz))
                        .unwrap_or(0);

                    self.light[(lx + 1) as usize][(ly + 1) as usize][(lz + 1) as usize] =
                        sky.max(block);
                }
            }
        }
    }

    /// Gets a block state at local coordinates (-1..17).
    /// Returns AIR if coordinates are out of bounds.
    #[inline]
    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockState {
        if !(-1..17).contains(&x) || !(-1..17).contains(&y) || !(-1..17).contains(&z) {
            return BlockState::AIR;
        }
        self.blocks[(x + 1) as usize][(y + 1) as usize][(z + 1) as usize]
    }

    /// Gets a biome at local block coordinates (-1..17).
    /// Returns default biome if coordinates are out of bounds.
    #[inline]
    pub fn get_biome(&self, x: i32, y: i32, z: i32) -> Biome {
        if !(-1..17).contains(&x) || !(-1..17).contains(&y) || !(-1..17).contains(&z) {
            return Biome::default();
        }
        self.biomes[(x.div_euclid(4) + 1) as usize][(y.div_euclid(4) + 1) as usize]
            [(z.div_euclid(4) + 1) as usize]
    }

    /// Gets the maximum of sky and block light at local block coordinates
    /// (-1..17). Returns 15 if coordinates are out of bounds.
    #[inline]
    pub fn get_light(&self, x: i32, y: i32, z: i32) -> u8 {
        if !(-1..17).contains(&x) || !(-1..17).contains(&y) || !(-1..17).contains(&z) {
            return 15;
        }
        self.light[(x + 1) as usize][(y + 1) as usize][(z + 1) as usize]
    }
}
