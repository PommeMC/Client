//! What a mesh dispatcher is built from: the block models and the atlas
//! layout, from the live renderer or, for a headless run, loaded without a GPU.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::assets::AssetIndex;
use crate::renderer::chunk::atlas::{AtlasUVMap, TextureAtlas, atlas_texture_names};
use crate::renderer::chunk::mesher::{BiomeClimate, MeshDispatcher};
use crate::resource_pack::ResourcePackManager;
use crate::world::block::model::CardinalLightType;
use crate::world::block::render_assets::{BlockRenderAssets, OwnedBlockRenderView};

pub trait MeshSource {
    fn shared_render_assets(&self) -> Arc<BlockRenderAssets>;
    fn uv_map(&self) -> &AtlasUVMap;
    fn jar_assets_dir(&self) -> &Path;
    fn asset_index(&self) -> &Option<AssetIndex>;

    fn create_mesh_dispatcher(
        &self,
        render_view: OwnedBlockRenderView,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        packs: Option<&ResourcePackManager>,
        cardinal_light: CardinalLightType,
    ) -> MeshDispatcher {
        MeshDispatcher::load(
            render_view,
            self.uv_map().clone(),
            self.jar_assets_dir(),
            self.asset_index(),
            packs,
            biome_climate,
            cardinal_light.table(),
        )
    }
}

/// The vanilla block models and atlas layout, with no resource packs.
pub struct CpuMeshAssets {
    pub jar_assets_dir: PathBuf,
    pub asset_index: Option<AssetIndex>,
    pub render_assets: Arc<BlockRenderAssets>,
    pub uv_map: AtlasUVMap,
}

impl CpuMeshAssets {
    pub fn load(jar_assets_dir: &Path, asset_index: Option<AssetIndex>) -> Self {
        // No block cache, so a run neither reads a stale one nor rewrites the
        // real one.
        let render_assets = Arc::new(BlockRenderAssets::load(
            jar_assets_dir,
            &asset_index,
            None,
            None,
        ));
        let (texture_names, generated_item_textures) = atlas_texture_names(&render_assets);
        let uv_map = TextureAtlas::build_cpu(
            jar_assets_dir,
            &asset_index,
            &texture_names,
            &generated_item_textures,
            None,
        )
        .uv_map;
        Self {
            jar_assets_dir: jar_assets_dir.to_path_buf(),
            asset_index,
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

    fn jar_assets_dir(&self) -> &Path {
        &self.jar_assets_dir
    }

    fn asset_index(&self) -> &Option<AssetIndex> {
        &self.asset_index
    }
}
