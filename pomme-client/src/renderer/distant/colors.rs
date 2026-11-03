//! What distant terrain's models bake from, and the biome tints the shaders
//! look them up under.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;

use super::gpu::TINT_BIOMES;
use super::models::{Metadata, ModelFactory, Models, pack_tint};
use crate::renderer::chunk::atlas::AtlasUVMap;
use crate::renderer::chunk::mesher::{
    BiomeClimate, Colormap, dry_foliage_color, foliage_color, grass_color,
};
use crate::world::block::BlockStateTable;
use crate::world::block::render_assets::OwnedBlockRenderView;

/// Biome tint kinds in the tint buffer: grass, foliage, dry foliage, water
/// ([`super::models::ModelTint`]'s biome kinds, in order).
pub(crate) const TINT_KINDS: usize = 4;

pub(crate) struct DistantColors {
    render_view: OwnedBlockRenderView,
    uv_map: Arc<AtlasUVMap>,
    climates: Arc<HashMap<u32, BiomeClimate>>,
    grass: Arc<Colormap>,
    foliage: Arc<Colormap>,
    dry_foliage: Arc<Colormap>,
}

impl DistantColors {
    pub fn new(
        render_view: OwnedBlockRenderView,
        uv_map: Arc<AtlasUVMap>,
        climates: Arc<HashMap<u32, BiomeClimate>>,
        (grass, foliage, dry_foliage): (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>),
    ) -> Self {
        Self {
            render_view,
            uv_map,
            climates,
            grass,
            foliage,
            dry_foliage,
        }
    }

    pub fn table(&self) -> BlockStateTable {
        self.render_view.block_state_table()
    }

    /// Whether `models` were baked from the same textures and state ids as
    /// these would bake them.
    pub fn same_models(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.uv_map, &other.uv_map)
            && self.table().table_slot() == other.table().table_slot()
    }

    /// The models as the mesher asks for them, baked into `models` the
    /// first time.
    pub fn baking<'a>(&'a self, models: &'a mut ModelFactory) -> Baking<'a> {
        Baking {
            colors: self,
            models,
        }
    }

    /// The tint buffer, [`TINT_KINDS`] rows of [`TINT_BIOMES`]: each biome's
    /// grass, foliage, dry foliage and water colours; white for biomes
    /// unknown. Without a position, the biome's base colour (no swamp or
    /// dark forest noise).
    pub fn tint_lut(&self) -> Vec<u32> {
        let mut lut = vec![u32::MAX; TINT_KINDS * TINT_BIOMES];
        for (&biome, climate) in self.climates.iter() {
            let biome = biome as usize;
            if biome >= TINT_BIOMES {
                continue;
            }
            let colours = [
                grass_color(climate, &self.grass, 0, 0),
                foliage_color(climate, &self.foliage),
                dry_foliage_color(climate, &self.dry_foliage),
                climate.water_color,
            ];
            for (kind, rgb) in colours.into_iter().enumerate() {
                lut[kind * TINT_BIOMES + biome] = pack_tint(rgb);
            }
        }
        lut
    }
}

/// Block states' models for the mesher.
pub(crate) struct Baking<'a> {
    colors: &'a DistantColors,
    models: &'a mut ModelFactory,
}

impl Models for Baking<'_> {
    fn model(&mut self, block: u32) -> u16 {
        BlockState::try_from(block).map_or(0, |state| {
            self.models.model(
                self.colors.render_view.as_view(),
                &self.colors.uv_map,
                state,
            )
        })
    }

    fn metadata(&self, model: u16) -> Metadata {
        self.models.info(model).metadata
    }

    fn fluid_surface(&mut self, fluid: u16, level: u8, gap: u8) -> u16 {
        self.models.fluid_surface(fluid, level, gap)
    }

    fn fluid_model(&self, model: u16) -> u16 {
        self.models.info(model).fluid
    }
}
