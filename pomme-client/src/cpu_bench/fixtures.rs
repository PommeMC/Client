//! Bench inputs: the block registry and atlas UV map loaded from the version's
//! assets on disk (CPU-only), and deterministic synthetic worlds.

use std::path::PathBuf;

use azalea_block::BlockState;
use glam::DVec3;

use crate::assets::AssetIndex;
use crate::dirs::DataDirs;
use crate::position::ChunkPos;
use crate::renderer::chunk::atlas::{AtlasUVMap, TextureAtlas, atlas_texture_names};
use crate::world::block::find_state;
use crate::world::block::registry::BlockRegistry;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};

pub struct Assets {
    pub jar_assets_dir: PathBuf,
    pub asset_index: Option<AssetIndex>,
    pub registry: BlockRegistry,
    pub uv_map: AtlasUVMap,
}

impl Assets {
    pub fn load(version: &str, dirs: &DataDirs) -> Result<Self, String> {
        if !dirs.jar_assets_dir.is_dir() {
            return Err(format!(
                "no {version} assets at {} (launch {version} once through the launcher to download them)",
                dirs.jar_assets_dir.display()
            ));
        }
        let asset_index = AssetIndex::load(&dirs.indexes_dir, &dirs.objects_dir, version);

        // No block cache, so the bench neither reads a stale one nor rewrites
        // the real one.
        eprintln!("cpu-bench: loading block models...");
        let registry = BlockRegistry::load(&dirs.jar_assets_dir, &asset_index, None, None);

        eprintln!("cpu-bench: packing texture atlas...");
        let (texture_names, generated_item_textures) = atlas_texture_names(&registry);
        let uv_map = TextureAtlas::build_cpu(
            &dirs.jar_assets_dir,
            &asset_index,
            &texture_names,
            &generated_item_textures,
            None,
        )
        .uv_map;

        Ok(Self {
            jar_assets_dir: dirs.jar_assets_dir.clone(),
            asset_index,
            registry,
            uv_map,
        })
    }
}

/// A synthetic world of 3×3 columns around (0, 0): the centre column's
/// meshing neighbourhood.
#[derive(Clone, Copy)]
pub enum Scene {
    /// Grass with short grass, flowers and a pond.
    Plains,
    /// Stone riddled with air pockets and ore.
    Caves,
    /// Grass under a dense oak canopy.
    Foliage,
}

impl Scene {
    pub const ALL: [Scene; 3] = [Scene::Plains, Scene::Caves, Scene::Foliage];

    pub fn name(self) -> &'static str {
        match self {
            Scene::Plains => "plains",
            Scene::Caves => "caves",
            Scene::Foliage => "foliage",
        }
    }
}

const SURFACE_Y: i32 = 63;
/// The camera the fade and walk benches view from: just above the surface of
/// column (0, 0).
pub const EYE: DVec3 = DVec3::new(8.0, 72.0, 8.0);
/// Block-space X/Z extent of the 3×3 columns.
const XZ: std::ops::RangeInclusive<i32> = -16..=31;

/// Every column within render distance `rd` of (0, 0).
pub fn columns_within(rd: i32) -> impl Iterator<Item = ChunkPos> {
    (-rd..=rd).flat_map(move |x| (-rd..=rd).map(move |z| ChunkPos::new(x, z)))
}

fn columns() -> impl Iterator<Item = (i32, i32)> {
    XZ.flat_map(|x| XZ.map(move |z| (x, z)))
}

struct States {
    stone: BlockState,
    dirt: BlockState,
    grass_block: BlockState,
    short_grass: BlockState,
    poppy: BlockState,
    dandelion: BlockState,
    water: BlockState,
    coal_ore: BlockState,
    iron_ore: BlockState,
    oak_log: BlockState,
    oak_leaves: BlockState,
    air: BlockState,
}

impl States {
    fn lookup() -> Self {
        Self {
            stone: find_state("stone", &[]),
            dirt: find_state("dirt", &[]),
            grass_block: find_state("grass_block", &[("snowy", "false")]),
            short_grass: find_state("short_grass", &[]),
            poppy: find_state("poppy", &[]),
            dandelion: find_state("dandelion", &[]),
            water: find_state("water", &[("level", "0")]),
            coal_ore: find_state("coal_ore", &[]),
            iron_ore: find_state("iron_ore", &[]),
            oak_log: find_state("oak_log", &[("axis", "y")]),
            oak_leaves: find_state(
                "oak_leaves",
                &[
                    ("distance", "1"),
                    ("persistent", "true"),
                    ("waterlogged", "false"),
                ],
            ),
            air: find_state("air", &[]),
        }
    }
}

/// Builds `scene` from a fixed seed, so runs compare like for like.
pub fn build_world(scene: Scene) -> ChunkStore {
    let s = States::lookup();
    let mut store = ChunkStore::new(8);
    for pos in columns_within(1) {
        let sections = store.section_count() as usize;
        store.insert_chunk(pos, Chunk::empty(sections));
    }
    let min_y = store.min_y();
    let mut rng = fastrand::Rng::with_seed(0x0050_4F4D_4D45);

    for (x, z) in columns() {
        for y in min_y..SURFACE_Y - 3 {
            store.set_block_state(x, y, z, s.stone);
        }
        for y in SURFACE_Y - 3..SURFACE_Y {
            store.set_block_state(x, y, z, s.dirt);
        }
        store.set_block_state(x, SURFACE_Y, z, s.grass_block);
    }

    match scene {
        Scene::Plains => {
            for (x, z) in columns() {
                let plant = match rng.u32(0..100) {
                    0..15 => s.short_grass,
                    15 => s.poppy,
                    16 => s.dandelion,
                    _ => continue,
                };
                store.set_block_state(x, SURFACE_Y + 1, z, plant);
            }
            // Straddles the centre column's east edge.
            for x in 10..22 {
                for z in 4..12 {
                    store.set_block_state(x, SURFACE_Y + 1, z, s.air);
                    store.set_block_state(x, SURFACE_Y, z, s.water);
                    store.set_block_state(x, SURFACE_Y - 1, z, s.water);
                }
            }
        }
        Scene::Caves => {
            for (x, z) in columns() {
                for y in min_y + 1..SURFACE_Y - 3 {
                    match rng.u32(0..200) {
                        0 | 1 => store.set_block_state(x, y, z, s.coal_ore),
                        2 => store.set_block_state(x, y, z, s.iron_ore),
                        _ => {}
                    }
                }
            }
            for _ in 0..120 {
                let (cx, cy, cz) = (rng.i32(XZ), rng.i32(min_y + 4..SURFACE_Y - 8), rng.i32(XZ));
                let r = rng.i32(2..=5);
                for (x, z) in columns() {
                    for y in cy - r..=cy + r {
                        if (x - cx).pow(2) + (y - cy).pow(2) + (z - cz).pow(2) <= r * r {
                            store.set_block_state(x, y, z, s.air);
                        }
                    }
                }
            }
        }
        Scene::Foliage => {
            for tx in (XZ.start() + 2..XZ.end() - 1).step_by(4) {
                for tz in (XZ.start() + 2..XZ.end() - 1).step_by(4) {
                    let top = SURFACE_Y + rng.i32(4..=6);
                    for y in top - 2..=top + 1 {
                        let r = if y > top { 1 } else { 2 };
                        for x in tx - r..=tx + r {
                            for z in tz - r..=tz + r {
                                store.set_block_state(x, y, z, s.oak_leaves);
                            }
                        }
                    }
                    for y in SURFACE_Y + 1..=top {
                        store.set_block_state(tx, y, tz, s.oak_log);
                    }
                }
            }
        }
    }
    add_sky_light(&mut store);
    store
}

/// Sky light 15 above each column's highest block and 0 below, no block
/// light: the light reads a real world pays, without running the engine.
// TODO: run the light engine for propagated light under overhangs and caves.
fn add_sky_light(store: &mut ChunkStore) {
    let min_y = store.min_y();
    let top_y = min_y + store.height() as i32;
    let layer_count = store.section_count() as usize + 2;
    for pos in columns_within(1) {
        let mut light = ChunkLightData {
            sky_sections: vec![Some(Box::new([0; 2048])); layer_count],
            block_sections: vec![None; layer_count],
            min_y,
            has_sky: true,
            sky_top_section: Some(layer_count as i32),
        };
        for x in pos.x * 16..pos.x * 16 + 16 {
            for z in pos.z * 16..pos.z * 16 + 16 {
                let surface = (min_y..top_y)
                    .rev()
                    .find(|&y| !crate::world::block::is_air(store.get_block_state(x, y, z)))
                    .unwrap_or(min_y - 1);
                for y in surface + 1..top_y {
                    light.set_sky_light(x, y, z, 15);
                }
            }
        }
        store
            .light_data
            .insert((pos.x, pos.z), std::sync::Arc::new(light));
    }
}
