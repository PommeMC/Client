//! Bench inputs: block render assets and atlas UV map loaded from the version's
//! assets on disk (CPU-only), and deterministic synthetic worlds.

use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use glam::DVec3;

use crate::dirs::DataDirs;
use crate::position::ChunkPos;
use crate::renderer::chunk::mesh_source::{CpuMeshAssets, MeshSource};
use crate::renderer::chunk::mesher::MeshDispatcher;
use crate::resources::ResourceManager;
use crate::world::block::BlockStateTable;
use crate::world::block::model::CardinalLightType;
use crate::world::block::render_assets::OwnedBlockRenderView;
use crate::world::chunk::{Chunk, ChunkLightData, ChunkStore};
use crate::world::light::DataLayer;

pub struct Assets {
    pub mesh: CpuMeshAssets,
    pub block_state_table: BlockStateTable,
}

impl Assets {
    pub fn load(
        version: &str,
        dirs: &DataDirs,
        block_state_table: BlockStateTable,
        resources: &ResourceManager,
    ) -> Result<Self, String> {
        if !dirs.jar_assets_dir.is_dir() {
            return Err(format!(
                "no {version} assets at {} (launch {version} once through the launcher to download them)",
                dirs.jar_assets_dir.display()
            ));
        }
        eprintln!("cpu-bench: loading block models and packing the atlas...");
        Ok(Self {
            mesh: CpuMeshAssets::load(resources),
            block_state_table,
        })
    }

    /// An overworld dispatcher with no biome climates or resource packs.
    pub fn mesh_dispatcher(&self) -> MeshDispatcher {
        self.mesh.create_mesh_dispatcher(
            OwnedBlockRenderView::new(self.mesh.render_assets.clone(), self.block_state_table),
            Arc::new(HashMap::new()),
            CardinalLightType::Default,
        )
    }
}

/// A synthetic world of columns around (0, 0), by default the 3×3 of the
/// centre column's meshing neighbourhood.
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
/// Block-space X/Z extent of the columns within `radius` of (0, 0).
fn xz(radius: i32) -> std::ops::RangeInclusive<i32> {
    -16 * radius..=16 * (radius + 1) - 1
}

/// Every column within render distance `rd` of (0, 0).
pub fn columns_within(rd: i32) -> impl Iterator<Item = ChunkPos> {
    (-rd..=rd).flat_map(move |x| (-rd..=rd).map(move |z| ChunkPos::new(x, z)))
}

fn columns(radius: i32) -> impl Iterator<Item = (i32, i32)> {
    xz(radius).flat_map(move |x| xz(radius).map(move |z| (x, z)))
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
    fn lookup(block_state_table: BlockStateTable) -> Self {
        Self {
            stone: block_state_table.find_state("stone", &[]),
            dirt: block_state_table.find_state("dirt", &[]),
            grass_block: block_state_table.find_state("grass_block", &[("snowy", "false")]),
            short_grass: block_state_table.find_state("short_grass", &[]),
            poppy: block_state_table.find_state("poppy", &[]),
            dandelion: block_state_table.find_state("dandelion", &[]),
            water: block_state_table.find_state("water", &[("level", "0")]),
            coal_ore: block_state_table.find_state("coal_ore", &[]),
            iron_ore: block_state_table.find_state("iron_ore", &[]),
            oak_log: block_state_table.find_state("oak_log", &[("axis", "y")]),
            oak_leaves: block_state_table.find_state(
                "oak_leaves",
                &[
                    ("distance", "1"),
                    ("persistent", "true"),
                    ("waterlogged", "false"),
                ],
            ),
            air: block_state_table.find_state("air", &[]),
        }
    }
}

/// Builds `scene` from a fixed seed, so runs compare like for like.
pub fn build_world(scene: Scene, block_state_table: BlockStateTable) -> ChunkStore {
    build_world_within(scene, block_state_table, 1)
}

/// [`build_world`] over every column within `radius` of (0, 0).
pub fn build_world_within(
    scene: Scene,
    block_state_table: BlockStateTable,
    radius: i32,
) -> ChunkStore {
    let s = States::lookup(block_state_table);
    let mut store = ChunkStore::new(8);
    for pos in columns_within(radius) {
        let sections = store.section_count() as usize;
        store.insert_chunk(pos, Chunk::empty(sections));
    }
    let min_y = store.min_y();
    let mut rng = fastrand::Rng::with_seed(0x0050_4F4D_4D45);

    for (x, z) in columns(radius) {
        for y in min_y..SURFACE_Y - 3 {
            store.set_block_state(x, y, z, s.stone, block_state_table);
        }
        for y in SURFACE_Y - 3..SURFACE_Y {
            store.set_block_state(x, y, z, s.dirt, block_state_table);
        }
        store.set_block_state(x, SURFACE_Y, z, s.grass_block, block_state_table);
    }

    match scene {
        Scene::Plains => {
            for (x, z) in columns(radius) {
                let plant = match rng.u32(0..100) {
                    0..15 => s.short_grass,
                    15 => s.poppy,
                    16 => s.dandelion,
                    _ => continue,
                };
                store.set_block_state(x, SURFACE_Y + 1, z, plant, block_state_table);
            }
            // Straddles the centre column's east edge.
            for x in 10..22 {
                for z in 4..12 {
                    store.set_block_state(x, SURFACE_Y + 1, z, s.air, block_state_table);
                    store.set_block_state(x, SURFACE_Y, z, s.water, block_state_table);
                    store.set_block_state(x, SURFACE_Y - 1, z, s.water, block_state_table);
                }
            }
        }
        Scene::Caves => {
            for (x, z) in columns(radius) {
                for y in min_y + 1..SURFACE_Y - 3 {
                    match rng.u32(0..200) {
                        0 | 1 => store.set_block_state(x, y, z, s.coal_ore, block_state_table),
                        2 => store.set_block_state(x, y, z, s.iron_ore, block_state_table),
                        _ => {}
                    }
                }
            }
            // 120 pockets per 3×3 columns.
            let side = 2 * radius + 1;
            for _ in 0..120 * side * side / 9 {
                let (cx, cy, cz) = (
                    rng.i32(xz(radius)),
                    rng.i32(min_y + 4..SURFACE_Y - 8),
                    rng.i32(xz(radius)),
                );
                let r = rng.i32(2..=5);
                for x in cx - r..=cx + r {
                    for z in cz - r..=cz + r {
                        for y in cy - r..=cy + r {
                            if (x - cx).pow(2) + (y - cy).pow(2) + (z - cz).pow(2) <= r * r {
                                store.set_block_state(x, y, z, s.air, block_state_table);
                            }
                        }
                    }
                }
            }
        }
        Scene::Foliage => {
            let (start, end) = xz(radius).into_inner();
            for tx in (start + 2..end - 1).step_by(4) {
                for tz in (start + 2..end - 1).step_by(4) {
                    let top = SURFACE_Y + rng.i32(4..=6);
                    for y in top - 2..=top + 1 {
                        let r = if y > top { 1 } else { 2 };
                        for x in tx - r..=tx + r {
                            for z in tz - r..=tz + r {
                                store.set_block_state(x, y, z, s.oak_leaves, block_state_table);
                            }
                        }
                    }
                    for y in SURFACE_Y + 1..=top {
                        store.set_block_state(tx, y, tz, s.oak_log, block_state_table);
                    }
                }
            }
        }
    }
    add_sky_light(&mut store, block_state_table, radius);
    store
}

/// Sky light 15 above each column's highest block and 0 below, no block
/// light: the light reads a real world pays, without running the engine.
// TODO: run the light engine for propagated light under overhangs and caves.
fn add_sky_light(store: &mut ChunkStore, block_state_table: BlockStateTable, radius: i32) {
    let min_y = store.min_y();
    let top_y = min_y + store.height() as i32;
    let layer_count = store.section_count() as usize + 2;
    for pos in columns_within(radius) {
        let mut light = ChunkLightData {
            sky_sections: vec![Some(DataLayer::new()); layer_count],
            block_sections: vec![None; layer_count],
            min_y,
            has_sky: true,
            sky_top_section: Some(layer_count as i32),
        };
        for x in pos.x * 16..pos.x * 16 + 16 {
            for z in pos.z * 16..pos.z * 16 + 16 {
                let surface = (min_y..top_y)
                    .rev()
                    .find(|&y| {
                        !block_state_table
                            .block_state_data(store.get_block_state(x, y, z))
                            .is_air()
                    })
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
