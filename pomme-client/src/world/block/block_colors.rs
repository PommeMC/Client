use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;

use crate::position::BlockPos;
use crate::renderer::chunk::mesher::{
    BiomeClimate, Colormap, blend_color, dry_foliage_color, foliage_color, grass_color,
    tint_sample_y,
};
use crate::world::block::BlockStateTable;
use crate::world::block::render_assets::{BlockRenderView, Tint};
use crate::world::chunk::ChunkStore;

/// COMPAT: Implements Pomme's existing `Tint`-based color lookup, not Vanilla's
/// per-block, per-layer `BlockTintSource` registrations.
/// TODO: Port the full Vanilla `BlockColors` and tint-source capabilities.
pub(crate) struct BlockColors {
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
}

impl BlockColors {
    pub(crate) fn new(grass: Arc<Colormap>, foliage: Arc<Colormap>, dry: Arc<Colormap>) -> Self {
        Self {
            grass_colormap: grass,
            foliage_colormap: foliage,
            dry_foliage_colormap: dry,
        }
    }

    /// Returns the terrain-particle tint multiplier, excluding its base color.
    pub(crate) fn terrain_particle_color(
        &self,
        state: BlockState,
        pos: BlockPos,
        registry: BlockRenderView<'_>,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
    ) -> [f32; 3] {
        let block_state_data = registry.block_state_table().block_state_data(state);
        if block_state_data.id() == "grass_block" {
            return [1.0; 3];
        }
        let Some(faces) = registry.get_textures(state) else {
            return [1.0; 3];
        };
        if faces.tint == Tint::None {
            return [1.0; 3];
        }

        match faces.tint {
            Tint::Redstone => block_state_data.redstone_wire_rgb(),
            Tint::Stem => block_state_data.stem_rgb(),
            Tint::Constant(color) => crate::renderer::chunk::mesher::int_to_rgb(color as i32),
            _ => self.blend_tint(
                faces.tint,
                state,
                pos,
                chunks,
                biome_climate,
                registry.block_state_table(),
            ),
        }
    }

    /// The block's biome tint using vanilla's default blend radius.
    fn blend_tint(
        &self,
        tint: Tint,
        state: BlockState,
        pos: BlockPos,
        chunks: &ChunkStore,
        biome_climate: &HashMap<u32, BiomeClimate>,
        block_state_table: BlockStateTable,
    ) -> [f32; 3] {
        let sample_y = tint_sample_y(tint, state, pos.y, block_state_table);
        blend_color(pos.x, pos.z, |x, z| {
            let climate = biome_climate
                .get(&chunks.biome_id(x, sample_y, z))
                .copied()
                .unwrap_or_default();
            match tint {
                Tint::Grass | Tint::DoubleGrass => {
                    grass_color(&climate, &self.grass_colormap, x, z)
                }
                Tint::Foliage => foliage_color(&climate, &self.foliage_colormap),
                Tint::DryFoliage => dry_foliage_color(&climate, &self.dry_foliage_colormap),
                Tint::Water => climate.water_color,
                // State/fixed colors are resolved by the caller.
                Tint::None | Tint::Constant(_) | Tint::Redstone | Tint::Stem => [1.0; 3],
            }
        })
    }
}
