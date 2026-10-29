//! What a mesh dispatcher is built from: the block models and the atlas
//! layout, from the live renderer or, for a headless run, loaded without a GPU.

use std::collections::HashMap;
use std::sync::Arc;

use crate::renderer::chunk::atlas::{AtlasUVMap, TextureAtlas, atlas_texture_names};
use crate::renderer::chunk::mesher::{BiomeClimate, MeshDispatcher};
use crate::resources::ResourceManager;
use crate::world::block::colormap::Colormaps;
use crate::world::block::model::{
    BlockstateSources, CardinalLightType, ItemDefinitionSources, ModelSources,
};
use crate::world::block::render_assets::{BlockRenderAssets, OwnedBlockRenderView};

pub trait MeshSource {
    fn shared_render_assets(&self) -> Arc<BlockRenderAssets>;
    fn uv_map(&self) -> &AtlasUVMap;

    fn create_mesh_dispatcher(
        &self,
        render_view: OwnedBlockRenderView,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        cardinal_light: CardinalLightType,
    ) -> MeshDispatcher {
        let colormaps = render_view.colormaps().clone();
        MeshDispatcher::new(
            render_view,
            self.uv_map().clone(),
            Arc::clone(colormaps.grass()),
            Arc::clone(colormaps.foliage()),
            Arc::clone(colormaps.dry_foliage()),
            biome_climate,
            cardinal_light.table(),
        )
    }
}

/// The vanilla block models and atlas layout, with no resource packs.
pub struct CpuMeshAssets {
    pub render_assets: Arc<BlockRenderAssets>,
    pub uv_map: AtlasUVMap,
}

impl CpuMeshAssets {
    pub fn load(resources: &ResourceManager) -> Self {
        let blockstates = BlockstateSources::load(resources)
            .expect("failed to enumerate Minecraft blockstates for CPU mesh assets");
        let models = ModelSources::load(resources)
            .expect("failed to enumerate Minecraft geometry models for CPU mesh assets");
        let items = ItemDefinitionSources::load(resources)
            .expect("failed to enumerate Minecraft item definitions for CPU mesh assets");
        let render_assets = Arc::new(BlockRenderAssets::load(
            Colormaps::load(resources),
            blockstates,
            models,
            items,
        ));
        let (texture_names, generated_item_textures) = atlas_texture_names(&render_assets);
        let uv_map =
            TextureAtlas::build_cpu(resources, &texture_names, &generated_item_textures).uv_map;
        Self {
            render_assets,
            uv_map,
        }
    }
}

impl MeshSource for CpuMeshAssets {
    fn shared_render_assets(&self) -> Arc<BlockRenderAssets> {
        Arc::clone(&self.render_assets)
    }

    fn uv_map(&self) -> &AtlasUVMap {
        &self.uv_map
    }
}
