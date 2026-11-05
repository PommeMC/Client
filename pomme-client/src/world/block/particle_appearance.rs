use std::collections::HashMap;

use azalea_block::BlockState;

use super::BlockStateTable;
use super::block_colors::BlockColors;
use super::render_assets::BlockRenderView;
use crate::position::BlockPos;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::renderer::chunk::mesher::BiomeClimate;
use crate::world::chunk::ChunkStore;

/// Block particle sprite and tint queries over borrowed CPU resources.
pub(crate) struct BlockParticleAppearance<'a> {
    registry: BlockRenderView<'a>,
    atlas: &'a AtlasUVMap,
    colors: &'a BlockColors,
    biome_climate: &'a HashMap<u32, BiomeClimate>,
}

impl<'a> BlockParticleAppearance<'a> {
    pub(crate) fn new(
        registry: BlockRenderView<'a>,
        atlas: &'a AtlasUVMap,
        colors: &'a BlockColors,
        biome_climate: &'a HashMap<u32, BiomeClimate>,
    ) -> Self {
        Self {
            registry,
            atlas,
            colors,
            biome_climate,
        }
    }

    pub(crate) fn sprite(&self, state: BlockState) -> AtlasRegion {
        match self.registry.get_textures(state) {
            Some(faces) => self
                .atlas
                .get_region(faces.particle.as_deref().unwrap_or(&faces.top)),
            None => self.atlas.get_region(""),
        }
    }

    pub(crate) fn tint(&self, state: BlockState, pos: BlockPos, chunks: &ChunkStore) -> [f32; 3] {
        self.colors
            .terrain_particle_color(state, pos, self.registry, chunks, self.biome_climate)
    }

    pub(crate) fn block_state_table(&self) -> BlockStateTable {
        self.registry.block_state_table()
    }
}
