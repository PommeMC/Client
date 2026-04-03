use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::ChunkPos;

use super::greedy;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::world::block::model::{BakedModel, Direction};
use crate::world::block::registry::{BlockRegistry, FaceTextures, Tint};
use crate::world::chunk::{self, ChunkStore};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub tex_coords: [f32; 2],
    pub light: f32,
    pub tint: [f32; 3],
}

pub struct ChunkMeshData {
    pub pos: ChunkPos,
    pub vertices: Vec<ChunkVertex>,
    pub indices: Vec<u32>,
}

const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

#[derive(Clone, Copy, Debug, Default)]
pub enum GrassColorModifier {
    #[default]
    None,
    DarkForest,
    Swamp,
}

#[derive(Clone, Copy, Debug)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub downfall: f32,
    pub grass_color_override: Option<[f32; 3]>,
    pub grass_color_modifier: GrassColorModifier,
    pub foliage_color_override: Option<[f32; 3]>,
}

impl Default for BiomeClimate {
    fn default() -> Self {
        Self {
            temperature: 0.8,
            downfall: 0.4,
            grass_color_override: None,
            grass_color_modifier: GrassColorModifier::None,
            foliage_color_override: None,
        }
    }
}

fn tint_color(tint: Tint, grass: [f32; 3], foliage: [f32; 3]) -> [f32; 3] {
    match tint {
        Tint::None => WHITE,
        Tint::Grass => grass,
        Tint::Foliage => foliage,
    }
}

const MAX_MESH_UPLOADS_PER_FRAME: usize = 16;

pub struct Colormap {
    pixels: Vec<[u8; 3]>,
}

impl Colormap {
    pub fn load(
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<crate::assets::AssetIndex>,
        colormap_path: &str,
    ) -> Self {
        let path = crate::assets::resolve_asset_path(jar_assets_dir, asset_index, colormap_path);
        let pixels = crate::renderer::util::load_png(&path)
            .map(|(data, _w, _h)| {
                data.chunks(4)
                    .take(256 * 256)
                    .map(|c| [c[0], c[1], c[2]])
                    .collect()
            })
            .unwrap_or_else(|| vec![[145, 189, 89]; 256 * 256]);
        Self { pixels }
    }

    fn lookup(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        let t = temperature.clamp(0.0, 1.0);
        let d = (downfall.clamp(0.0, 1.0)) * t;
        let x = ((1.0 - t) * 255.0) as usize;
        let y = ((1.0 - d) * 255.0) as usize;
        let idx = (y * 256 + x).min(256 * 256 - 1);
        let [r, g, b] = self.pixels[idx];
        [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
    }
}

fn apply_grass_modifier(modifier: GrassColorModifier, base: [f32; 3], x: i32, z: i32) -> [f32; 3] {
    match modifier {
        GrassColorModifier::None => base,
        GrassColorModifier::DarkForest => {
            let r = ((to_u8(base[0]) & 0xFE) as u32 + 0x28) >> 1;
            let g = ((to_u8(base[1]) & 0xFE) as u32 + 0x34) >> 1;
            let b = ((to_u8(base[2]) & 0xFE) as u32 + 0x0A) >> 1;
            [
                r.min(255) as f32 / 255.0,
                g.min(255) as f32 / 255.0,
                b.min(255) as f32 / 255.0,
            ]
        }
        GrassColorModifier::Swamp => {
            use std::sync::LazyLock;
            static BIOME_NOISE: LazyLock<SimplexNoise> =
                LazyLock::new(SimplexNoise::new_biome_info);
            let noise = BIOME_NOISE.value_2d(x as f64 * 0.0225, z as f64 * 0.0225);
            if noise < -0.1 {
                [
                    0x4C as f32 / 255.0,
                    0x76 as f32 / 255.0,
                    0x3C as f32 / 255.0,
                ]
            } else {
                [
                    0x6A as f32 / 255.0,
                    0x70 as f32 / 255.0,
                    0x39 as f32 / 255.0,
                ]
            }
        }
    }
}

fn to_u8(f: f32) -> u8 {
    (f * 255.0).round() as u8
}

struct SimplexNoise {
    perm: [u8; 256],
    #[allow(dead_code)]
    xo: f64,
    #[allow(dead_code)]
    yo: f64,
}

const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

impl SimplexNoise {
    fn new_biome_info() -> Self {
        let mut rng = JavaRng::new(2345);
        let xo = rng.next_double() * 256.0;
        let yo = rng.next_double() * 256.0;
        let _zo = rng.next_double() * 256.0;
        let mut perm = [0u8; 256];
        for (i, p) in perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        for i in 0..256 {
            let j = rng.next_int((256 - i) as i32) as usize + i;
            perm.swap(i, j);
        }
        Self { perm, xo, yo }
    }

    fn p(&self, i: i32) -> i32 {
        self.perm[(i & 0xFF) as usize] as i32
    }

    fn value_2d(&self, x: f64, y: f64) -> f64 {
        let sqrt3: f64 = 3.0_f64.sqrt();
        let f2 = 0.5 * (sqrt3 - 1.0);
        let g2 = (3.0 - sqrt3) / 6.0;

        let s = (x + y) * f2;
        let i = (x + s).floor() as i32;
        let j = (y + s).floor() as i32;
        let t = (i + j) as f64 * g2;
        let x0 = x - (i as f64 - t);
        let y0 = y - (j as f64 - t);

        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - i1 as f64 + g2;
        let y1 = y0 - j1 as f64 + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;

        let gi0 = (self.p(i + self.p(j)) % 12) as usize;
        let gi1 = (self.p(i + i1 + self.p(j + j1)) % 12) as usize;
        let gi2 = (self.p(i + 1 + self.p(j + 1)) % 12) as usize;

        let n0 = corner_noise(gi0, x0, y0, 0.0, 0.5);
        let n1 = corner_noise(gi1, x1, y1, 0.0, 0.5);
        let n2 = corner_noise(gi2, x2, y2, 0.0, 0.5);

        70.0 * (n0 + n1 + n2)
    }
}

fn corner_noise(gi: usize, x: f64, y: f64, z: f64, falloff: f64) -> f64 {
    let t = falloff - x * x - y * y - z * z;
    if t < 0.0 {
        0.0
    } else {
        let t2 = t * t;
        let g = &GRADIENT[gi];
        t2 * t2 * (g[0] as f64 * x + g[1] as f64 * y + g[2] as f64 * z)
    }
}

struct JavaRng {
    seed: i64,
}

impl JavaRng {
    fn new(seed: i64) -> Self {
        Self {
            seed: (seed ^ 0x5DEECE66D) & ((1i64 << 48) - 1),
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB)) & ((1i64 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & (bound - 1) == 0 {
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }

    fn next_double(&mut self) -> f64 {
        let hi = self.next(26) as i64;
        let lo = self.next(27) as i64;
        ((hi << 27) + lo) as f64 / ((1i64 << 53) as f64)
    }
}

pub fn int_to_rgb(color: i32) -> [f32; 3] {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    [r, g, b]
}

pub struct MeshDispatcher {
    result_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    result_tx: crossbeam_channel::Sender<ChunkMeshData>,
    registry: Arc<BlockRegistry>,
    uv_map: Arc<AtlasUVMap>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
}

impl MeshDispatcher {
    pub fn new(
        registry: BlockRegistry,
        uv_map: AtlasUVMap,
        grass_colormap: Colormap,
        foliage_colormap: Colormap,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    ) -> Self {
        let (result_tx, result_rx) = crossbeam_channel::unbounded();
        Self {
            result_rx,
            result_tx,
            registry: Arc::new(registry),
            uv_map: Arc::new(uv_map),
            grass_colormap: Arc::new(grass_colormap),
            foliage_colormap: Arc::new(foliage_colormap),
            biome_climate,
        }
    }

    pub fn set_biome_climate(&mut self, climate: Arc<HashMap<u32, BiomeClimate>>) {
        self.biome_climate = climate;
    }

    pub fn enqueue(&self, chunk_store: &ChunkStore, pos: ChunkPos, _lod: u32) {
        let registry = Arc::clone(&self.registry);
        let uv_map = Arc::clone(&self.uv_map);
        let grass_colormap = Arc::clone(&self.grass_colormap);
        let foliage_colormap = Arc::clone(&self.foliage_colormap);
        let biome_climate = Arc::clone(&self.biome_climate);
        let tx = self.result_tx.clone();

        let chunks_needed = [
            pos,
            ChunkPos::new(pos.x - 1, pos.z),
            ChunkPos::new(pos.x + 1, pos.z),
            ChunkPos::new(pos.x, pos.z - 1),
            ChunkPos::new(pos.x, pos.z + 1),
        ];
        let chunk_arcs: Vec<_> = chunks_needed
            .iter()
            .map(|p| chunk_store.get_chunk(p))
            .collect();

        let min_y = chunk_store.min_y();
        let height = chunk_store.height();

        let light: std::collections::HashMap<(i32, i32), crate::world::chunk::ChunkLightData> =
            chunks_needed
                .iter()
                .filter_map(|p| {
                    chunk_store
                        .light_data
                        .get(&(p.x, p.z))
                        .map(|ld| ((p.x, p.z), ld.clone()))
                })
                .collect();

        rayon::spawn(move || {
            let snapshot = ChunkStoreSnapshot {
                chunks: chunks_needed.into_iter().zip(chunk_arcs).collect(),
                light,
                grass_colormap,
                foliage_colormap,
                biome_climate,
                min_y,
                height,
            };
            let mesh = mesh_chunk_snapshot(&snapshot, pos, &registry, &uv_map, 0);
            let _ = tx.send(mesh);
        });
    }

    pub fn drain_results(&self) -> impl Iterator<Item = ChunkMeshData> + '_ {
        self.result_rx.try_iter().take(MAX_MESH_UPLOADS_PER_FRAME)
    }
}

struct ChunkStoreSnapshot {
    chunks: Vec<(
        ChunkPos,
        Option<Arc<parking_lot::RwLock<azalea_world::Chunk>>>,
    )>,
    light: std::collections::HashMap<(i32, i32), crate::world::chunk::ChunkLightData>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    min_y: i32,
    height: u32,
}

impl ChunkStoreSnapshot {
    fn get_block_state(&self, x: i32, y: i32, z: i32) -> azalea_block::BlockState {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c): &(ChunkPos, _)| c.as_ref());

        let Some(chunk_lock) = chunk_lock else {
            return azalea_block::BlockState::AIR;
        };

        let c: parking_lot::RwLockReadGuard<'_, azalea_world::Chunk> = chunk_lock.read();
        chunk::block_state_from_section(&c, x, y, z, self.min_y)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn get_biome(&self, x: i32, y: i32, z: i32) -> azalea_registry::data::Biome {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c)| c.as_ref());
        let Some(chunk_lock) = chunk_lock else {
            return azalea_registry::data::Biome::default();
        };
        let c = chunk_lock.read();
        let biome_pos = azalea_core::position::ChunkBiomePos {
            x: (x.rem_euclid(16) / 4) as u8,
            y,
            z: (z.rem_euclid(16) / 4) as u8,
        };
        c.get_biome(biome_pos, self.min_y).unwrap_or_default()
    }

    fn climate_at(&self, x: i32, y: i32, z: i32) -> BiomeClimate {
        let biome = self.get_biome(x, y, z);
        self.biome_climate
            .get(&u32::from(biome))
            .copied()
            .unwrap_or_default()
    }

    fn grass_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        let base = climate.grass_color_override.unwrap_or_else(|| {
            self.grass_colormap
                .lookup(climate.temperature, climate.downfall)
        });
        apply_grass_modifier(climate.grass_color_modifier, base, x, z)
    }

    fn foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        climate.foliage_color_override.unwrap_or_else(|| {
            self.foliage_colormap
                .lookup(climate.temperature, climate.downfall)
        })
    }

    fn grass_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::grass_color_at)
    }

    fn foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::foliage_color_at)
    }

    fn blend_color(
        &self,
        x: i32,
        y: i32,
        z: i32,
        color_fn: fn(&Self, i32, i32, i32) -> [f32; 3],
    ) -> [f32; 3] {
        const RADIUS: i32 = 2;
        const COUNT: f32 = ((RADIUS * 2 + 1) * (RADIUS * 2 + 1)) as f32;
        let mut r = 0.0f32;
        let mut g = 0.0f32;
        let mut b = 0.0f32;
        for dz in -RADIUS..=RADIUS {
            for dx in -RADIUS..=RADIUS {
                let c = color_fn(self, x + dx, y, z + dz);
                r += c[0];
                g += c[1];
                b += c[2];
            }
        }
        [r / COUNT, g / COUNT, b / COUNT]
    }

    fn get_light(&self, x: i32, y: i32, z: i32) -> f32 {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        let lx = x.rem_euclid(16);
        let lz = z.rem_euclid(16);
        let level = if let Some(light) = self.light.get(&(cx, cz)) {
            light
                .get_sky_light(lx, y, lz)
                .max(light.get_block_light(lx, y, lz))
        } else {
            15
        };
        LIGHT_TABLE[level as usize]
    }
}

pub const LIGHT_TABLE: [f32; 16] = [
    0.05, 0.067, 0.085, 0.106, 0.129, 0.156, 0.188, 0.227, 0.272, 0.328, 0.393, 0.472, 0.566,
    0.679, 0.815, 1.0,
];

struct GreedyBlockInfo {
    textures: FaceTextures,
}

struct BlockTypeMap {
    state_to_id: HashMap<BlockState, u16>,
    id_to_info: Vec<GreedyBlockInfo>,
}

impl BlockTypeMap {
    fn build(
        snapshot: &ChunkStoreSnapshot,
        registry: &BlockRegistry,
        world_x: i32,
        world_z: i32,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        let mut state_to_id = HashMap::new();
        let mut id_to_info: Vec<GreedyBlockInfo> = Vec::new();
        let mut next_id = 1u16;

        for lz in -1..17i32 {
            for lx in -1..17i32 {
                let bx = world_x + lx;
                let bz = world_z + lz;
                for by in (min_y - 1)..=(max_y) {
                    let state = snapshot.get_block_state(bx, by, bz);
                    if state.is_air() || state_to_id.contains_key(&state) {
                        continue;
                    }
                    let has_baked = registry.get_baked_model(state).is_some();
                    let has_multipart = registry.get_multipart_quads(state).is_some();
                    if has_baked || has_multipart {
                        state_to_id.insert(state, 0);
                        continue;
                    }
                    if let Some(textures) = registry.get_textures(state) {
                        if textures.side_overlay.is_some() || !registry.is_opaque_full_cube(state) {
                            state_to_id.insert(state, 0);
                            continue;
                        }
                        state_to_id.insert(state, next_id);
                        id_to_info.push(GreedyBlockInfo {
                            textures: textures.clone(),
                        });
                        next_id += 1;
                    } else {
                        state_to_id.insert(state, 0);
                    }
                }
            }
        }

        Self {
            state_to_id,
            id_to_info,
        }
    }

    fn get_id(&self, state: BlockState) -> u16 {
        if state.is_air() {
            return 0;
        }
        self.state_to_id.get(&state).copied().unwrap_or(0)
    }

    fn get_info(&self, id: u16) -> Option<&GreedyBlockInfo> {
        if id == 0 {
            return None;
        }
        self.id_to_info.get((id - 1) as usize)
    }
}

const SECTION_SIZE: usize = 16;

fn face_texture_name(textures: &FaceTextures, face: greedy::Face) -> &str {
    match face {
        greedy::Face::Up => &textures.top,
        greedy::Face::Down => &textures.bottom,
        greedy::Face::Right => &textures.east,
        greedy::Face::Left => &textures.west,
        greedy::Face::Front => &textures.south,
        greedy::Face::Back => &textures.north,
    }
}

const AO_BRIGHTNESS: [f32; 4] = [0.2, 0.467, 0.733, 1.0];

#[allow(clippy::too_many_arguments)]
fn greedy_mesh_section(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    type_map: &BlockTypeMap,
    uv_map: &AtlasUVMap,
    world_x: i32,
    section_y: i32,
    world_z: i32,
) {
    type M = greedy::GreedyMesher<SECTION_SIZE>;
    let mut mesher = M::new();
    let mut voxels = vec![0u16; M::CS_P3];
    let mut occluders = vec![false; M::CS_P3];

    for ly in 0..18 {
        for lx in 0..18 {
            for lz in 0..18 {
                let bx = world_x + lx as i32 - 1;
                let by = section_y + ly as i32 - 1;
                let bz = world_z + lz as i32 - 1;
                let state = snapshot.get_block_state(bx, by, bz);
                let idx = greedy::pad_linearize::<SECTION_SIZE>(lx, ly, lz);
                voxels[idx] = type_map.get_id(state);
                occluders[idx] = registry.is_opaque_full_cube(state);
            }
        }
    }

    let transparent_set = std::collections::BTreeSet::new();
    mesher.mesh(&voxels, &occluders, &transparent_set);

    for face_idx in 0..6usize {
        let face = greedy::Face::from(face_idx);
        let dir_shade = face.shade_light();

        for quad in &mesher.quads[face_idx] {
            let block_id = quad.voxel_id();
            let info = match type_map.get_info(block_id) {
                Some(i) => i,
                None => continue,
            };

            let tex_name = face_texture_name(&info.textures, face);
            let region = uv_map.get_region(tex_name);
            let verts_uvs = face.vertices(quad);

            let [x0, _, z0] = verts_uvs[0].0;
            let block_x = x0 as i32 + world_x;
            let block_z = z0 as i32 + world_z;
            let tint = tint_color(
                info.textures.tint,
                snapshot.grass_tint(block_x, section_y, block_z),
                snapshot.foliage_tint(block_x, section_y, block_z),
            );

            let ao = quad.ao_levels();
            let [qx, qy, qz] = quad.xyz();
            let face_offset = face.offset();
            let light_sample = snapshot.get_light(
                world_x + qx as i32 + face_offset[0],
                section_y + qy as i32 + face_offset[1],
                world_z + qz as i32 + face_offset[2],
            );
            let lights: [f32; 4] =
                core::array::from_fn(|i| AO_BRIGHTNESS[ao[i] as usize] * light_sample * dir_shade);

            let base = vertices.len() as u32;
            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;

            for (i, (pos, uv)) in verts_uvs.iter().enumerate() {
                vertices.push(ChunkVertex {
                    position: [
                        pos[0] + world_x as f32,
                        pos[1] + section_y as f32,
                        pos[2] + world_z as f32,
                    ],
                    tex_coords: [region.u_min + uv[0] * u_span, region.v_min + uv[1] * v_span],
                    light: lights[i],
                    tint,
                });
            }

            if lights[0] + lights[2] > lights[1] + lights[3] {
                indices.extend_from_slice(&[
                    base + 1,
                    base + 2,
                    base + 3,
                    base + 3,
                    base,
                    base + 1,
                ]);
            } else {
                indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
            }
        }
    }
}

fn mesh_chunk_snapshot(
    snapshot: &ChunkStoreSnapshot,
    pos: ChunkPos,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lod: u32,
) -> ChunkMeshData {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut logged_missing: std::collections::HashSet<String> = std::collections::HashSet::new();

    let step = 1i32 << lod;
    let scale = step as f32;

    let min_y = snapshot.min_y();
    let max_y = min_y + snapshot.height() as i32;
    let world_x = pos.x * 16;
    let world_z = pos.z * 16;

    let type_map = if lod == 0 {
        Some(BlockTypeMap::build(
            snapshot, registry, world_x, world_z, min_y, max_y,
        ))
    } else {
        None
    };
    if let Some(ref tm) = type_map {
        let sections = (max_y - min_y) / 16;
        for section in 0..sections {
            let section_y = min_y + section * 16;
            greedy_mesh_section(
                &mut vertices,
                &mut indices,
                snapshot,
                registry,
                tm,
                uv_map,
                world_x,
                section_y,
                world_z,
            );
        }
    }

    let mut local_z = 0i32;
    while local_z < 16 {
        let mut local_x = 0i32;
        while local_x < 16 {
            let bx = world_x + local_x;
            let bz = world_z + local_z;

            let mut by = min_y;
            while by < max_y {
                let state = snapshot.get_block_state(bx, by, bz);
                let kind = classify_block(state);
                if matches!(kind, BlockKind::Air) {
                    by += step;
                    continue;
                }

                if lod == 0
                    && let Some(ref tm) = type_map
                    && tm.get_id(state) != 0
                {
                    by += step;
                    continue;
                }

                let block_pos = [bx as f32, by as f32, bz as f32];

                if lod > 0 {
                    emit_lod_cube(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        scale,
                        state,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                        step,
                    );
                } else if let BlockKind::Water | BlockKind::Lava = kind {
                    let fluid = if matches!(kind, BlockKind::Lava) {
                        "lava"
                    } else {
                        "water"
                    };
                    emit_fluid(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        fluid,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(baked) = registry.get_baked_model(state) {
                    emit_baked_model(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        baked,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(quads) = registry.get_multipart_quads(state) {
                    emit_multipart(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        &quads,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else if let Some(textures) = registry.get_textures(state) {
                    emit_cube_faces(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        textures,
                        snapshot,
                        registry,
                        uv_map,
                        bx,
                        by,
                        bz,
                    );
                } else {
                    let block: Box<dyn azalea_block::BlockTrait> = state.into();
                    let id = block.id().to_string();
                    if logged_missing.insert(id.clone()) {
                        log::warn!("Missing model: {id}");
                    }
                    emit_missing_cube(
                        &mut vertices,
                        &mut indices,
                        block_pos,
                        snapshot,
                        registry,
                        bx,
                        by,
                        bz,
                    );
                }
                by += step;
            }
            local_x += step;
        }
        local_z += step;
    }

    ChunkMeshData {
        pos,
        vertices,
        indices,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_baked_model(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    model: &BakedModel,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in &model.quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.is_opaque_full_cube(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(bx, by, bz),
            snapshot.foliage_tint(bx, by, bz),
        );
        let lights = if let Some(dir) = quad.cullface {
            compute_face_ao(snapshot, registry, bx, by, bz, dir)
        } else {
            [quad.shade_light; 4]
        };
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            lights,
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_cube_faces(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    textures: &crate::world::block::registry::FaceTextures,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let tint = tint_color(
        textures.tint,
        snapshot.grass_tint(bx, by, bz),
        snapshot.foliage_tint(bx, by, bz),
    );

    for (i, dir) in CUBE_FACE_DIRS.iter().enumerate() {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let face_tex = match i {
            0 => &textures.top,
            1 => &textures.bottom,
            2 => &textures.north,
            3 => &textures.south,
            4 => &textures.east,
            _ => &textures.west,
        };
        let region = uv_map.get_region(face_tex);
        let (positions, uvs, _) = cube_face_geometry(*dir);
        let lights = compute_face_ao(snapshot, registry, bx, by, bz, *dir);

        let is_side = i >= 2;
        if let Some(overlay) = textures.side_overlay.as_deref().filter(|_| is_side) {
            emit_face(
                vertices, indices, block_pos, &positions, &uvs, lights, region, WHITE,
            );
            let overlay_region = uv_map.get_region(overlay);
            emit_face(
                vertices,
                indices,
                block_pos,
                &positions,
                &uvs,
                lights,
                overlay_region,
                tint,
            );
        } else {
            let is_tinted =
                !matches!(textures.tint, Tint::None) && (textures.side_overlay.is_none() || i == 0);
            let face_tint = if is_tinted { tint } else { WHITE };
            emit_face(
                vertices, indices, block_pos, &positions, &uvs, lights, region, face_tint,
            );
        }
    }
}

enum BlockKind {
    Air,
    Water,
    Lava,
    Solid,
}

fn classify_block(state: azalea_block::BlockState) -> BlockKind {
    if state.is_air() {
        return BlockKind::Air;
    }
    let block: Box<dyn azalea_block::BlockTrait> = state.into();
    match block.id() {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            BlockKind::Air
        }
        "water" | "bubble_column" => BlockKind::Water,
        "lava" => BlockKind::Lava,
        _ => BlockKind::Solid,
    }
}

// TODO: biome-based water color
// TODO: per-corner height averaging for smooth water surfaces
// TODO: flowing water texture (water_flow) with direction-based rotation
// TODO: per-level height for flowing water (level / 9.0 per corner)

const FLUID_MAX_HEIGHT: f32 = 8.0 / 9.0;

#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    fluid: &str,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let (tex_name, tint) = if fluid == "lava" {
        ("lava_still", [1.0, 1.0, 1.0])
    } else {
        ("water_still", [0.247, 0.463, 0.894])
    };
    let region = uv_map.get_region(tex_name);

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);

        if matches!(classify_block(neighbor), BlockKind::Water | BlockKind::Lava)
            || registry.is_opaque_full_cube(neighbor)
        {
            continue;
        }

        let (mut positions, uvs, light) = cube_face_geometry(*dir);

        if matches!(dir, Direction::Up) {
            let above = snapshot.get_block_state(bx, by + 1, bz);
            let top = if matches!(classify_block(above), BlockKind::Water | BlockKind::Lava) {
                1.0
            } else {
                FLUID_MAX_HEIGHT
            };
            for p in &mut positions {
                p[1] = top;
            }
        }

        emit_face(
            vertices, indices, block_pos, &positions, &uvs, [light; 4], region, tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_multipart(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    quads: &[&crate::world::block::model::BakedQuad],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for quad in quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.is_opaque_full_cube(neighbor) {
                continue;
            }
        }

        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(bx, by, bz),
            snapshot.foliage_tint(bx, by, bz),
        );
        emit_face(
            vertices,
            indices,
            block_pos,
            &quad.positions,
            &quad.uvs,
            [quad.shade_light; 4],
            region,
            tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lod_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    scale: f32,
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
    step: i32,
) {
    let region = if let Some(textures) = registry.get_textures(state) {
        let tint = tint_color(textures.tint, WHITE, WHITE);
        let tex = uv_map.get_region(&textures.top);
        (tex, tint)
    } else {
        (uv_map.get_region(""), MISSING_TINT)
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let nx = bx + offset[0] * step;
        let ny = by + offset[1] * step;
        let nz = bz + offset[2] * step;
        let neighbor = snapshot.get_block_state(nx, ny, nz);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let (positions, uvs, light) = cube_face_geometry(*dir);
        let base = vertices.len() as u32;
        for i in 0..4 {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + positions[i][0] * scale,
                    block_pos[1] + positions[i][1] * scale,
                    block_pos[2] + positions[i][2] * scale,
                ],
                tex_coords: [
                    region.0.u_min + uvs[i][0] * (region.0.u_max - region.0.u_min),
                    region.0.v_min + uvs[i][1] * (region.0.v_max - region.0.v_min),
                ],
                light,
                tint: region.1,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

const MISSING_TINT: [f32; 3] = [1.0, 0.0, 1.0];

#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
) {
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.is_opaque_full_cube(neighbor) {
            continue;
        }

        let (positions, _, light) = cube_face_geometry(*dir);
        let base = vertices.len() as u32;
        for pos in &positions {
            vertices.push(ChunkVertex {
                position: [
                    block_pos[0] + pos[0],
                    block_pos[1] + pos[1],
                    block_pos[2] + pos[2],
                ],
                tex_coords: [0.0, 0.0],
                light,
                tint: MISSING_TINT,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

const CUBE_FACE_DIRS: [Direction; 6] = [
    Direction::Up,
    Direction::Down,
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];

#[allow(clippy::too_many_arguments)]
fn emit_face(
    vertices: &mut Vec<ChunkVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [f32; 4],
    region: AtlasRegion,
    tint: [f32; 3],
) {
    let base = vertices.len() as u32;
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;

    for i in 0..4 {
        vertices.push(ChunkVertex {
            position: [
                block_pos[0] + positions[i][0],
                block_pos[1] + positions[i][1],
                block_pos[2] + positions[i][2],
            ],
            tex_coords: [
                region.u_min + uvs[i][0] * u_span,
                region.v_min + uvs[i][1] * v_span,
            ],
            light: lights[i],
            tint,
        });
    }

    if lights[0] + lights[2] > lights[1] + lights[3] {
        indices.extend_from_slice(&[base + 1, base + 2, base + 3, base + 3, base, base + 1]);
    } else {
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

fn shade_brightness(state: azalea_block::BlockState, registry: &BlockRegistry) -> f32 {
    if registry.is_opaque_full_cube(state) {
        0.2
    } else {
        1.0
    }
}

fn vertex_ao(side1: f32, side2: f32, corner: f32) -> f32 {
    (side1 + side2 + corner + 1.0) * 0.25
}

fn compute_face_ao(
    snapshot: &ChunkStoreSnapshot,
    registry: &BlockRegistry,
    bx: i32,
    by: i32,
    bz: i32,
    dir: Direction,
) -> [f32; 4] {
    let s = |dx: i32, dy: i32, dz: i32| -> f32 {
        shade_brightness(
            snapshot.get_block_state(bx + dx, by + dy, bz + dz),
            registry,
        )
    };
    let l = |dx: i32, dy: i32, dz: i32| -> f32 { snapshot.get_light(bx + dx, by + dy, bz + dz) };
    let dir_shade = match dir {
        Direction::Up => 1.0,
        Direction::Down => 0.5,
        Direction::North | Direction::South => 0.8,
        Direction::East | Direction::West => 0.6,
    };

    let (ao, lights) = match dir {
        Direction::Up => {
            let n = [0, 1, 0];
            (
                [
                    vertex_ao(s(0, 1, 1), s(-1, 1, 0), s(-1, 1, 1)),
                    vertex_ao(s(0, 1, 1), s(1, 1, 0), s(1, 1, 1)),
                    vertex_ao(s(0, 1, -1), s(1, 1, 0), s(1, 1, -1)),
                    vertex_ao(s(0, 1, -1), s(-1, 1, 0), s(-1, 1, -1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(0, 1, 1), l(-1, 1, 0), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, 1), l(1, 1, 0), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, -1), l(1, 1, 0), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(0, 1, -1), l(-1, 1, 0), l(-1, 1, -1)),
                ],
            )
        }
        Direction::Down => {
            let n = [0, -1, 0];
            (
                [
                    vertex_ao(s(0, -1, -1), s(-1, -1, 0), s(-1, -1, -1)),
                    vertex_ao(s(0, -1, -1), s(1, -1, 0), s(1, -1, -1)),
                    vertex_ao(s(0, -1, 1), s(1, -1, 0), s(1, -1, 1)),
                    vertex_ao(s(0, -1, 1), s(-1, -1, 0), s(-1, -1, 1)),
                ],
                [
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(0, -1, -1),
                        l(-1, -1, 0),
                        l(-1, -1, -1),
                    ),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, -1), l(1, -1, 0), l(1, -1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, 1), l(1, -1, 0), l(1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(0, -1, 1), l(-1, -1, 0), l(-1, -1, 1)),
                ],
            )
        }
        Direction::North => {
            let n = [0, 0, -1];
            (
                [
                    vertex_ao(s(-1, 0, -1), s(0, -1, -1), s(-1, -1, -1)),
                    vertex_ao(s(-1, 0, -1), s(0, 1, -1), s(-1, 1, -1)),
                    vertex_ao(s(1, 0, -1), s(0, 1, -1), s(1, 1, -1)),
                    vertex_ao(s(1, 0, -1), s(0, -1, -1), s(1, -1, -1)),
                ],
                [
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(-1, 0, -1),
                        l(0, -1, -1),
                        l(-1, -1, -1),
                    ),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, -1), l(0, 1, -1), l(-1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(0, 1, -1), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(0, -1, -1), l(1, -1, -1)),
                ],
            )
        }
        Direction::South => {
            let n = [0, 0, 1];
            (
                [
                    vertex_ao(s(1, 0, 1), s(0, -1, 1), s(1, -1, 1)),
                    vertex_ao(s(1, 0, 1), s(0, 1, 1), s(1, 1, 1)),
                    vertex_ao(s(-1, 0, 1), s(0, 1, 1), s(-1, 1, 1)),
                    vertex_ao(s(-1, 0, 1), s(0, -1, 1), s(-1, -1, 1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(0, -1, 1), l(1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(0, 1, 1), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(0, 1, 1), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(0, -1, 1), l(-1, -1, 1)),
                ],
            )
        }
        Direction::East => {
            let n = [1, 0, 0];
            (
                [
                    vertex_ao(s(1, 0, -1), s(1, -1, 0), s(1, -1, -1)),
                    vertex_ao(s(1, 0, -1), s(1, 1, 0), s(1, 1, -1)),
                    vertex_ao(s(1, 0, 1), s(1, 1, 0), s(1, 1, 1)),
                    vertex_ao(s(1, 0, 1), s(1, -1, 0), s(1, -1, 1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(1, -1, 0), l(1, -1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, -1), l(1, 1, 0), l(1, 1, -1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(1, 1, 0), l(1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(1, 0, 1), l(1, -1, 0), l(1, -1, 1)),
                ],
            )
        }
        Direction::West => {
            let n = [-1, 0, 0];
            (
                [
                    vertex_ao(s(-1, 0, 1), s(-1, -1, 0), s(-1, -1, 1)),
                    vertex_ao(s(-1, 0, 1), s(-1, 1, 0), s(-1, 1, 1)),
                    vertex_ao(s(-1, 0, -1), s(-1, 1, 0), s(-1, 1, -1)),
                    vertex_ao(s(-1, 0, -1), s(-1, -1, 0), s(-1, -1, -1)),
                ],
                [
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(-1, -1, 0), l(-1, -1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, 1), l(-1, 1, 0), l(-1, 1, 1)),
                    avg4(l(n[0], n[1], n[2]), l(-1, 0, -1), l(-1, 1, 0), l(-1, 1, -1)),
                    avg4(
                        l(n[0], n[1], n[2]),
                        l(-1, 0, -1),
                        l(-1, -1, 0),
                        l(-1, -1, -1),
                    ),
                ],
            )
        }
    };
    [
        ao[0] * lights[0] * dir_shade,
        ao[1] * lights[1] * dir_shade,
        ao[2] * lights[2] * dir_shade,
        ao[3] * lights[3] * dir_shade,
    ]
}

fn avg4(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (a + b + c + d) * 0.25
}

fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4], f32) {
    match dir {
        Direction::Up => (
            [
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            1.0,
        ),
        Direction::Down => (
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            0.5,
        ),
        Direction::North => (
            [
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            0.8,
        ),
        Direction::South => (
            [
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.8,
        ),
        Direction::East => (
            [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.6,
        ),
        Direction::West => (
            [
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
            [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            0.6,
        ),
    }
}
