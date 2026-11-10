//! What a mesh dispatcher is built from: the block models and the atlas
//! layout, from the live renderer or, for a headless run, loaded without a GPU.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use crate::renderer::chunk::atlas::{AtlasUVMap, TextureAtlas, atlas_texture_names};
use crate::renderer::chunk::mesher::{BiomeClimate, MeshDispatcher};
use crate::resources::managers::ResourceManager;
use crate::world::block::model::{CardinalLightType, ModelInputs};
use crate::world::block::render_assets::{BlockRenderAssets, OwnedBlockRenderView};

pub trait MeshSource {
    fn shared_render_assets(&self) -> Arc<BlockRenderAssets>;
    fn uv_map(&self) -> &AtlasUVMap;

    fn create_mesh_dispatcher(
        &self,
        render_view: OwnedBlockRenderView,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        resources: &dyn ResourceManager,
        cardinal_light: CardinalLightType,
    ) -> MeshDispatcher {
        MeshDispatcher::load(
            render_view,
            self.uv_map().clone(),
            resources,
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
    pub fn load(resources: &dyn ResourceManager) -> io::Result<Self> {
        let inputs = ModelInputs::prepare(resources)?;
        // No block cache, so a run neither reads a stale one nor rewrites the
        // real one.
        let render_assets = Arc::new(BlockRenderAssets::load(&inputs, None, false, None));
        let (texture_names, generated_item_textures) = atlas_texture_names(&render_assets);
        let atlas_inputs = super::atlas::AtlasInputs::prepare(resources)?;
        let uv_map = TextureAtlas::build_cpu(
            &atlas_inputs,
            &texture_names,
            &generated_item_textures,
            None,
        )
        .uv_map;
        Ok(Self {
            render_assets,
            uv_map,
        })
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
