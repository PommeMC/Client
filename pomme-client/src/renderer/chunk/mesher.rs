use std::collections::HashMap;
use std::sync::Arc;

use azalea_block::BlockState;
use azalea_core::position::ChunkSectionPos;
use pyronyx::vk;

use super::greedy;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap, WHITE_TEXTURE};
use crate::renderer::chunk::section::LocalSection;
use crate::world::block::model::{BakedModel, Direction, face_positions, face_uvs};
use crate::world::block::registry::{BlockRegistry, FaceTextures, Tint};
use crate::world::block::{block_id, is_air};
use crate::world::chunk::ChunkStore;
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkVertex {
    pub position: [f32; 3],
    pub tex_coords: [u16; 2],
    pub light_tint: u32,
}
impl ChunkVertex {
    pub const STRIDE: u32 = size_of::<Self>() as u32;
    pub fn binding_description() -> vk::VertexInputBindingDescription {
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: Self::STRIDE,
            input_rate: vk::VertexInputRate::Vertex,
        }
    }
    pub fn attribute_descriptions() -> [vk::VertexInputAttributeDescription; 3] {
        [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32Sfloat,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R16G16Unorm,
                offset: 12,
            },
            vk::VertexInputAttributeDescription {
                location: 2,
                binding: 0,
                format: vk::Format::R8G8B8A8Unorm,
                offset: 16,
            },
        ]
    }
}
include!("packing_consts.rs");

/// Compact GPU vertex (14 bytes): section-local position quantized to u16 (see
/// `POS_RANGE`), rebased in the vertex shader via the integer section origin.
/// `light_tint` is `[u8; 4]` (not `u32`) so the struct packs to 14 bytes with
/// no alignment padding; byte order matches the old `R8G8B8A8_UNORM` (light,
/// r,g,b).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackedVertex {
    pub pos: [u16; 3],
    pub uv: [u16; 2],
    pub light_tint: [u8; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkAABB {
    pub min: [f32; 4],
    pub max: [f32; 4],
}

fn unorm_to_u16(x: f32) -> u16 {
    (x.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

fn quantize_coord(local: f32) -> u16 {
    unorm_to_u16((local + POS_BIAS) / POS_RANGE)
}

fn pack_vertex(v: &ChunkVertex) -> PackedVertex {
    PackedVertex {
        pos: [
            quantize_coord(v.position[0]),
            quantize_coord(v.position[1]),
            quantize_coord(v.position[2]),
        ],
        uv: v.tex_coords,
        light_tint: v.light_tint.to_le_bytes(),
    }
}

fn section_aabb<'a>(verts: impl Iterator<Item = &'a ChunkVertex>) -> ChunkAABB {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in verts {
        for k in 0..3 {
            mn[k] = mn[k].min(v.position[k]);
            mx[k] = mx[k].max(v.position[k]);
        }
    }
    ChunkAABB {
        min: [mn[0], mn[1], mn[2], 0.0],
        max: [mx[0], mx[1], mx[2], 0.0],
    }
}

pub fn pack_uv(u: f32, v: f32) -> [u16; 2] {
    [unorm_to_u16(u), unorm_to_u16(v)]
}
pub fn pack_light_tint(light: f32, tint: u32) -> u32 {
    let l = (light.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    l | (tint & 0xFFFFFF00)
}
pub const fn pack_tint_shifted(rgb: [f32; 3]) -> u32 {
    const fn channel(v: f32) -> u32 {
        let c = (v * 255.0 + 0.5) as i32;
        if c < 0 {
            0
        } else if c > 255 {
            255
        } else {
            c as u32
        }
    }
    (channel(rgb[0]) << 8) | (channel(rgb[1]) << 16) | (channel(rgb[2]) << 24)
}
pub const PACKED_WHITE_SHIFTED: u32 = pack_tint_shifted([1.0, 1.0, 1.0]);

/// Fold a sprite's average color into a packed tint's lanes for flat-color
/// LOD faces. The shader raises tint to the 2.2 power while sampled texels
/// pass through linearly, so the average is inverse-gamma-encoded here to
/// land back on its true value — otherwise flat faces render ~half as bright
/// as their textured neighbors.
fn mul_tint(packed: u32, rgb: [f32; 3]) -> u32 {
    let ch = |shift: u32| ((packed >> shift) & 0xFF) as f32 / 255.0;
    let enc = |c: f32| c.powf(1.0 / 2.2);
    pack_tint_shifted([
        ch(8) * enc(rgb[0]),
        ch(16) * enc(rgb[1]),
        ch(24) * enc(rgb[2]),
    ])
}
/// One 16³ section's geometry as a pure quad list (4 vertices per quad, no
/// indices): vertices are grouped `[solid quads][cutout quads][water quads]`
/// so each pass draws a contiguous vertex range against the shared static
/// quad index buffer (`0,1,2,2,3,0` per quad, `vertex_offset` rebases).
pub struct SectionMeshData {
    pub spos: ChunkSectionPos,
    pub relative_si: i32,
    /// Length is `4 * (solid + cutout + water)` quads.
    pub vertices: Vec<PackedVertex>,
    /// Section-local bounds of the un-quantized vertices, for culling.
    pub aabb: ChunkAABB,
    /// Quads per pass, in slice order: solid (no-discard), cutout (discard),
    /// then translucent water (separate blended pass).
    pub solid_quads: u32,
    pub cutout_quads: u32,
    pub water_quads: u32,
    /// Content generation this mesh was built from (see
    /// `GameState::content_gen`). Lets the drain drop a stale result whose
    /// column has since been edited.
    pub content_gen: u64,
    /// Globally monotonic stamp assigned at enqueue. The buffer keeps the
    /// highest epoch uploaded per section and rejects any older upload, so an
    /// in-flight bulk mesh can never clobber a section a newer edit already
    /// uploaded (the edit always enqueues a higher epoch after its write).
    pub upload_epoch: u64,
    /// Worker-side stamps: time waiting in the mesh queue and time meshing.
    /// Aggregated by the chunk-load benchmark.
    pub queue_ms: f32,
    pub mesh_ms: f32,
}
impl SectionMeshData {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}

/// Per-section meshing accumulator: one vertex list per pass, each holding
/// whole quads (4 vertices). Finalized into a [`SectionMeshData`] by packing
/// the three lists back to back.
#[derive(Default)]
struct MeshSink {
    solid: Vec<ChunkVertex>,
    cutout: Vec<ChunkVertex>,
    water: Vec<ChunkVertex>,
}

impl MeshSink {
    /// Vertex list a quad goes in: solid sprites render in the no-discard
    /// pass, everything else in the discard (cutout) pass.
    fn pass_for(&mut self, opaque: bool) -> &mut Vec<ChunkVertex> {
        if opaque {
            &mut self.solid
        } else {
            &mut self.cutout
        }
    }
}

/// Append one quad's vertices. When the smooth-lighting diagonal flip fires
/// (vanilla's anisotropy fix), the quad is rotated by one vertex instead of
/// re-indexed, so the shared `0,1,2,2,3,0` index pattern triangulates on the
/// other diagonal; rotation preserves winding.
fn push_quad(dst: &mut Vec<ChunkVertex>, quad: [ChunkVertex; 4], lights: [f32; 4]) {
    if lights[0] + lights[2] > lights[1] + lights[3] {
        dst.extend_from_slice(&[quad[1], quad[2], quad[3], quad[0]]);
    } else {
        dst.extend_from_slice(&quad);
    }
}
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
    pub dry_foliage_color_override: Option<[f32; 3]>,
}
impl Default for BiomeClimate {
    fn default() -> Self {
        Self {
            temperature: 0.8,
            downfall: 0.4,
            grass_color_override: None,
            grass_color_modifier: GrassColorModifier::None,
            foliage_color_override: None,
            dry_foliage_color_override: None,
        }
    }
}
fn tint_color(tint: Tint, grass: [f32; 3], foliage: [f32; 3], dry_foliage: [f32; 3]) -> u32 {
    match tint {
        Tint::None => PACKED_WHITE_SHIFTED,
        Tint::Grass => pack_tint_shifted(grass),
        Tint::Foliage => pack_tint_shifted(foliage),
        Tint::DryFoliage => pack_tint_shifted(dry_foliage),
    }
}
#[derive(Clone)]
pub struct Colormap {
    pixels: Vec<[u8; 3]>,
}
impl Colormap {
    pub fn load(
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<crate::assets::AssetIndex>,
        colormap_path: &str,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
    ) -> Self {
        let path = crate::assets::resolve_asset_path_with_packs(
            jar_assets_dir,
            asset_index,
            colormap_path,
            packs,
        );
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

pub fn grass_color(climate: &BiomeClimate, colormap: &Colormap, x: i32, z: i32) -> [f32; 3] {
    let base = climate
        .grass_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall));
    apply_grass_modifier(climate.grass_color_modifier, base, x, z)
}

pub fn foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

pub fn dry_foliage_color(climate: &BiomeClimate, colormap: &Colormap) -> [f32; 3] {
    climate
        .dry_foliage_color_override
        .unwrap_or_else(|| colormap.lookup(climate.temperature, climate.downfall))
}

/// Average a biome color over the vanilla 5x5 horizontal blend
/// (`BiomeColors` with the default blend radius of 2).
pub fn blend_color(x: i32, z: i32, mut color_at: impl FnMut(i32, i32) -> [f32; 3]) -> [f32; 3] {
    const RADIUS: i32 = 2;
    const COUNT: f32 = ((RADIUS * 2 + 1) * (RADIUS * 2 + 1)) as f32;
    let mut sum = [0.0f32; 3];
    for dz in -RADIUS..=RADIUS {
        for dx in -RADIUS..=RADIUS {
            let c = color_at(x + dx, z + dz);
            for (s, v) in sum.iter_mut().zip(c) {
                *s += v;
            }
        }
    }
    sum.map(|s| s / COUNT)
}

/// Brightness at a block position from the chunk store's light data:
/// `LIGHT_TABLE[max(sky, block)]`.
pub fn world_brightness(chunks: &ChunkStore, x: i32, y: i32, z: i32) -> f32 {
    let level = chunks
        .get_sky_light(x, y, z)
        .max(chunks.get_block_light(x, y, z));
    LIGHT_TABLE[level as usize]
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
pub(crate) struct SectionStoreSnapshot {
    pub(crate) section: Box<LocalSection>,
    pub(crate) grass_colormap: Arc<Colormap>,
    pub(crate) foliage_colormap: Arc<Colormap>,
    pub(crate) dry_foliage_colormap: Arc<Colormap>,
    pub(crate) biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    pub(crate) min_y: i32,
    /// Section position, so section-local sample coords convert to world for
    /// spatial lookups (the swamp grass noise).
    pub(crate) spos: ChunkSectionPos,
}
impl SectionStoreSnapshot {
    fn climate_at(&self, x: i32, y: i32, z: i32) -> BiomeClimate {
        let biome = self.section.get_biome(x, y, z);
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
        // The swamp noise is world-space, so rebase the section-local coords.
        let wx = self.spos.x * 16 + x;
        let wz = self.spos.z * 16 + z;
        apply_grass_modifier(climate.grass_color_modifier, base, wx, wz)
    }
    fn foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        climate.foliage_color_override.unwrap_or_else(|| {
            self.foliage_colormap
                .lookup(climate.temperature, climate.downfall)
        })
    }
    fn dry_foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let climate = self.climate_at(x, y, z);
        climate.dry_foliage_color_override.unwrap_or_else(|| {
            self.dry_foliage_colormap
                .lookup(climate.temperature, climate.downfall)
        })
    }
    fn grass_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::grass_color_at)
    }
    fn foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::foliage_color_at)
    }
    fn dry_foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.blend_color(x, y, z, Self::dry_foliage_color_at)
    }
    fn blend_color(
        &self,
        x: i32,
        y: i32,
        z: i32,
        color_fn: fn(&Self, i32, i32, i32) -> [f32; 3],
    ) -> [f32; 3] {
        blend_color(x, z, |bx, bz| color_fn(self, bx, y, bz))
    }
    fn get_light(&self, x: i32, y: i32, z: i32) -> f32 {
        let light = self.section.get_light(x, y, z);
        LIGHT_TABLE[light as usize]
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
    fn build(snapshot: &SectionStoreSnapshot, registry: &BlockRegistry) -> Self {
        let mut state_to_id = HashMap::new();
        let mut id_to_info: Vec<GreedyBlockInfo> = Vec::new();
        let mut next_id = 1u16;
        for lz in -1..17 {
            for lx in -1..17 {
                for ly in -1..17 {
                    let state = snapshot.section.get_block_state(lx, ly, lz);
                    if is_air(state) || state_to_id.contains_key(&state) {
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
        if is_air(state) {
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
use super::block_ao::AO_BRIGHTNESS;
#[allow(clippy::too_many_arguments)]
fn greedy_mesh_section(
    solid: &mut Vec<ChunkVertex>,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    type_map: &BlockTypeMap,
    uv_map: &AtlasUVMap,
) {
    type M = greedy::GreedyMesher<SECTION_SIZE>;
    let mut mesher = M::new();
    let mut voxels = vec![0u16; M::CS_P3];
    let mut occluders = vec![false; M::CS_P3];
    let mut light = vec![0.0f32; M::CS_P3];
    for ly in 0..18 {
        for lx in 0..18 {
            for lz in 0..18 {
                let state = snapshot.section.blocks[lx][ly][lz];
                let idx = greedy::pad_linearize::<SECTION_SIZE>(lx, ly, lz);
                voxels[idx] = type_map.get_id(state);
                occluders[idx] = registry.is_opaque_full_cube(state);
                light[idx] = LIGHT_TABLE[snapshot.section.light[lx][ly][lz] as usize];
            }
        }
    }
    let transparent_set = std::collections::BTreeSet::new();
    mesher.mesh(&voxels, &occluders, &light, &transparent_set);
    for face_idx in 0..6 {
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
            let [x0, y0, z0] = verts_uvs[0].0;
            let lx = x0 as i32;
            let ly = y0 as i32;
            let lz = z0 as i32;
            let tint = tint_color(
                info.textures.tint,
                snapshot.grass_tint(lx, ly, lz),
                snapshot.foliage_tint(lx, ly, lz),
                snapshot.dry_foliage_tint(lx, ly, lz),
            );
            let ao = quad.ao_levels();
            // Per-vertex smooth light (averaged across chunk borders in the mesher); `i`
            // matches `ao`.
            let lights: [f32; 4] = core::array::from_fn(|i| {
                AO_BRIGHTNESS[ao[i] as usize] * (quad.light[i] as f32 / 255.0) * dir_shade
            });
            let u_span = region.u_max - region.u_min;
            let v_span = region.v_max - region.v_min;
            let quad_verts: [ChunkVertex; 4] = core::array::from_fn(|i| {
                let (pos, uv) = verts_uvs[i];
                ChunkVertex {
                    // Greedy quads are already section-local.
                    position: pos,
                    tex_coords: pack_uv(
                        region.u_min + uv[0] * u_span,
                        region.v_min + uv[1] * v_span,
                    ),
                    light_tint: pack_light_tint(lights[i], tint),
                }
            });
            push_quad(solid, quad_verts, lights);
        }
    }
}
pub(crate) fn mesh_section(
    snapshot: &SectionStoreSnapshot,
    spos: ChunkSectionPos,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lod: u32,
    content_gen: u64,
    upload_epoch: u64,
) -> SectionMeshData {
    let relative_si = spos.y - snapshot.min_y.div_euclid(16);
    let mut sink = MeshSink::default();
    let mut logged_missing: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();
    let step = 1i32 << lod;
    if lod > 0 {
        mesh_lod_cells(&mut sink, snapshot, registry, uv_map, step);
        return finalize_section(sink, spos, relative_si, content_gen, upload_epoch);
    }
    let type_map = BlockTypeMap::build(snapshot, registry);
    // Greedy geometry is opaque full cubes only, so it lands in the solid pass.
    greedy_mesh_section(&mut sink.solid, snapshot, registry, &type_map, uv_map);
    // 2. Complex & Fluid Pass (Block-by-Block)
    for local_z in 0..16 {
        for local_x in 0..16 {
            for local_y in 0..16 {
                let state = snapshot.section.get_block_state(local_x, local_y, local_z);
                let kind = classify_block(state);
                if matches!(kind, BlockKind::Air) {
                    continue;
                }
                // Skip blocks already handled by the greedy mesher.
                if type_map.get_id(state) != 0 {
                    continue;
                }
                // Section-local vertex base (matching the origin the buffer derives),
                // so positions never pass through absolute f32 world space.
                let block_pos = [local_x as f32, local_y as f32, local_z as f32];
                if let BlockKind::Water | BlockKind::Lava = kind {
                    emit_fluid(
                        &mut sink, kind, block_pos, state, snapshot, registry, uv_map, local_x,
                        local_y, local_z,
                    );
                } else if let Some(baked) = registry.get_baked_model(state) {
                    emit_baked_model(
                        &mut sink, block_pos, baked, snapshot, registry, uv_map, local_x, local_y,
                        local_z,
                    );
                } else if let Some(quads) = registry.get_multipart_quads(state) {
                    emit_multipart(
                        &mut sink, block_pos, quads, snapshot, registry, uv_map, local_x, local_y,
                        local_z,
                    );
                } else if let Some(textures) = registry.get_textures(state) {
                    emit_cube_faces(
                        &mut sink, block_pos, textures, snapshot, registry, uv_map, local_x,
                        local_y, local_z,
                    );
                } else {
                    let id = block_id(state);
                    if logged_missing.insert(id) {
                        tracing::warn!("Missing model: {id}");
                    }
                    emit_missing_cube(
                        &mut sink, block_pos, snapshot, registry, local_x, local_y, local_z,
                    );
                }
            }
        }
    }

    finalize_section(sink, spos, relative_si, content_gen, upload_epoch)
}

/// Pack the pass groups back to back (solid, cutout, water — recording the
/// quad counts), take the section-local AABB from the float positions, then
/// quantize.
fn finalize_section(
    sink: MeshSink,
    spos: ChunkSectionPos,
    relative_si: i32,
    content_gen: u64,
    upload_epoch: u64,
) -> SectionMeshData {
    let all_verts = || {
        sink.solid
            .iter()
            .chain(sink.cutout.iter())
            .chain(sink.water.iter())
    };
    let aabb = section_aabb(all_verts());
    let vertices: Vec<PackedVertex> = all_verts().map(pack_vertex).collect();
    SectionMeshData {
        spos,
        relative_si,
        vertices,
        aabb,
        solid_quads: (sink.solid.len() / 4) as u32,
        cutout_quads: (sink.cutout.len() / 4) as u32,
        water_quads: (sink.water.len() / 4) as u32,
        content_gen,
        upload_epoch,
        queue_ms: 0.0,
        mesh_ms: 0.0,
    }
}
#[allow(clippy::too_many_arguments)]
fn emit_baked_model(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    model: &BakedModel,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    for quad in &model.quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor =
                snapshot
                    .section
                    .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
            if registry.occludes_neighbor(neighbor) {
                continue;
            }
        }
        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(lx, ly, lz),
            snapshot.foliage_tint(lx, ly, lz),
            snapshot.dry_foliage_tint(lx, ly, lz),
        );
        let lights = if let Some(dir) = quad.cullface {
            compute_face_ao(snapshot, registry, lx, ly, lz, dir)
        } else {
            [quad.shade_light; 4]
        };
        emit_face(
            sink,
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
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    textures: &crate::world::block::registry::FaceTextures,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    let tint = tint_color(
        textures.tint,
        snapshot.grass_tint(lx, ly, lz),
        snapshot.foliage_tint(lx, ly, lz),
        snapshot.dry_foliage_tint(lx, ly, lz),
    );
    for (i, dir) in CUBE_FACE_DIRS.iter().enumerate() {
        let offset = dir.offset();
        let neighbor =
            snapshot
                .section
                .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
        if registry.occludes_neighbor(neighbor) {
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
        let lights = compute_face_ao(snapshot, registry, lx, ly, lz, *dir);
        let is_side = i >= 2;
        if let Some(overlay) = textures.side_overlay.as_deref().filter(|_| is_side) {
            emit_face(
                sink,
                block_pos,
                &positions,
                &uvs,
                lights,
                region,
                PACKED_WHITE_SHIFTED,
            );
            let overlay_region = uv_map.get_region(overlay);
            emit_face(
                sink,
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
            let face_tint = if is_tinted {
                tint
            } else {
                PACKED_WHITE_SHIFTED
            };
            emit_face(sink, block_pos, &positions, &uvs, lights, region, face_tint);
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
    if is_air(state) {
        return BlockKind::Air;
    }
    match block_id(state) {
        "cave_air" | "void_air" | "light" | "barrier" | "structure_void" | "moving_piston" => {
            BlockKind::Air
        }
        "water" | "bubble_column" => BlockKind::Water,
        "lava" => BlockKind::Lava,
        // Drawn by the block-entity pipeline; nothing to mesh.
        id if crate::world::block_entity::rendered_kind(id).is_some() => BlockKind::Air,
        _ => BlockKind::Solid,
    }
}
// TODO: biome-based water color
// TODO: per-corner height averaging for smooth water surfaces
// TODO: flowing water texture (water_flow) with direction-based rotation
// TODO: per-level height for flowing water (level / 9.0 per corner)
const FLUID_MAX_HEIGHT: f32 = 8.0 / 9.0;
#[allow(clippy::too_many_arguments)]
fn block_face_tex_tint(
    state: azalea_block::BlockState,
    dir: Direction,
    uv_map: &AtlasUVMap,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) -> (AtlasRegion, u32) {
    match classify_block(state) {
        BlockKind::Water => (
            uv_map.get_region("water_still"),
            pack_tint_shifted([0.247, 0.463, 0.894]),
        ),
        BlockKind::Lava => (uv_map.get_region("lava_still"), PACKED_WHITE_SHIFTED),
        _ => {
            if let Some(textures) = registry.get_textures(state) {
                let tint = tint_color(
                    textures.tint,
                    snapshot.grass_tint(lx, ly, lz),
                    snapshot.foliage_tint(lx, ly, lz),
                    snapshot.dry_foliage_tint(lx, ly, lz),
                );
                let tex_name = match dir {
                    Direction::Up => &textures.top,
                    Direction::Down => &textures.bottom,
                    Direction::North => &textures.north,
                    Direction::South => &textures.south,
                    Direction::East => &textures.east,
                    Direction::West => &textures.west,
                };
                (uv_map.get_region(tex_name), tint)
            } else {
                (uv_map.get_region(""), MISSING_TINT)
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn emit_fluid(
    sink: &mut MeshSink,
    kind: BlockKind,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    let (region, tint) =
        block_face_tex_tint(state, Direction::Up, uv_map, snapshot, registry, lx, ly, lz);

    // Water is translucent (separate blended pass); lava is opaque.
    let dst = if matches!(kind, BlockKind::Water) {
        &mut sink.water
    } else {
        &mut sink.solid
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor =
            snapshot
                .section
                .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
        if matches!(classify_block(neighbor), BlockKind::Water | BlockKind::Lava)
            || registry.occludes_neighbor(neighbor)
        {
            continue;
        }
        let (mut positions, uvs, light) = cube_face_geometry(*dir);
        if matches!(dir, Direction::Up) {
            // A water/lava block above would have culled this face already, so
            // the surface always sits at the lowered fluid height.
            for p in &mut positions {
                p[1] = FLUID_MAX_HEIGHT;
            }
            emit_quad_into(dst, block_pos, &positions, &uvs, [light; 4], region, tint);
            // Vanilla's backward up-face: the surface seen from below (underwater
            // looking up). Reversed winding so it survives back-face culling.
            let rev_positions = [positions[0], positions[3], positions[2], positions[1]];
            let rev_uvs = [uvs[0], uvs[3], uvs[2], uvs[1]];
            emit_quad_into(
                dst,
                block_pos,
                &rev_positions,
                &rev_uvs,
                [light; 4],
                region,
                tint,
            );
            continue;
        }
        emit_quad_into(dst, block_pos, &positions, &uvs, [light; 4], region, tint);
    }
}
#[allow(clippy::too_many_arguments)]
fn emit_multipart(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    quads: &[crate::world::block::model::BakedQuad],
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    for quad in quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor =
                snapshot
                    .section
                    .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
            if registry.occludes_neighbor(neighbor) {
                continue;
            }
        }
        let region = uv_map.get_region(&quad.texture);
        let tint = tint_color(
            quad.tint,
            snapshot.grass_tint(lx, ly, lz),
            snapshot.foliage_tint(lx, ly, lz),
            snapshot.dry_foliage_tint(lx, ly, lz),
        );
        emit_face(
            sink,
            block_pos,
            &quad.positions,
            &quad.uvs,
            [quad.shade_light; 4],
            region,
            tint,
        );
    }
}
/// What occupies a LOD cell's neighbor, deciding face culling: drawn LOD
/// cubes are full-size and opaque, so a `Solid` neighbor fully covers the
/// shared face. `Outside` (past the section) keeps the face — conservative
/// overdraw beats holes at borders where the padding can't answer.
#[derive(Clone, Copy, PartialEq)]
enum LodNeighbor {
    Empty,
    Solid,
    Fluid,
    Outside,
}

/// LOD sections render one cube per step³ cell, colored by the cell's
/// *dominant* block: aggregate sampling (any non-air occupies, most frequent
/// state wins), so thin features neither vanish nor shimmer — the DH/Voxy
/// downsampling model.
fn mesh_lod_cells(
    sink: &mut MeshSink,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    step: i32,
) {
    let cells = (16 / step) as usize;
    let cell_at = |cx: usize, cy: usize, cz: usize| (cy * cells + cz) * cells + cx;
    let mut grid: Vec<Option<BlockState>> = vec![None; cells * cells * cells];
    let mut surface: Vec<(BlockState, u8)> = Vec::new();
    let mut all: Vec<(BlockState, u8)> = Vec::new();
    fn vote(counts: &mut Vec<(BlockState, u8)>, state: BlockState) {
        match counts.iter_mut().find(|(s, _)| *s == state) {
            Some((_, n)) => *n += 1,
            None => counts.push((state, 1)),
        }
    }
    fn dominant(counts: &[(BlockState, u8)]) -> Option<BlockState> {
        counts.iter().max_by_key(|&&(_, n)| n).map(|&(s, _)| s)
    }
    for cy in 0..cells {
        for cz in 0..cells {
            for cx in 0..cells {
                surface.clear();
                all.clear();
                // The surface vote counts each XZ column's topmost block when
                // air sits directly above it — the block actually seen from
                // above — so a one-block grass skin over dirt reads as grass,
                // not the dirt that outnumbers it. Fully buried cells (side or
                // underside exposure only) fall back to the all-blocks vote.
                let ly_base = cy as i32 * step;
                for oz in 0..step {
                    for ox in 0..step {
                        let lx = cx as i32 * step + ox;
                        let lz = cz as i32 * step + oz;
                        let mut top: Option<(BlockState, i32)> = None;
                        for oy in 0..step {
                            let state = snapshot.section.get_block_state(lx, ly_base + oy, lz);
                            if matches!(classify_block(state), BlockKind::Air) {
                                continue;
                            }
                            vote(&mut all, state);
                            top = Some((state, oy));
                        }
                        if let Some((state, top_oy)) = top {
                            // The topmost block has air above it by
                            // construction unless it sits at the cell
                            // ceiling, where the row above (section padding
                            // at worst) decides.
                            let exposed = top_oy < step - 1
                                || matches!(
                                    classify_block(snapshot.section.get_block_state(
                                        lx,
                                        ly_base + step,
                                        lz
                                    )),
                                    BlockKind::Air
                                );
                            if exposed {
                                vote(&mut surface, state);
                            }
                        }
                    }
                }
                grid[cell_at(cx, cy, cz)] = dominant(&surface).or_else(|| dominant(&all));
            }
        }
    }

    let neighbor_of = |cx: i32, cy: i32, cz: i32| -> LodNeighbor {
        let range = 0..cells as i32;
        if !range.contains(&cx) || !range.contains(&cy) || !range.contains(&cz) {
            return LodNeighbor::Outside;
        }
        match grid[cell_at(cx as usize, cy as usize, cz as usize)] {
            None => LodNeighbor::Empty,
            Some(s) if matches!(classify_block(s), BlockKind::Water | BlockKind::Lava) => {
                LodNeighbor::Fluid
            }
            Some(_) => LodNeighbor::Solid,
        }
    };

    for cy in 0..cells {
        for cz in 0..cells {
            for cx in 0..cells {
                let Some(state) = grid[cell_at(cx, cy, cz)] else {
                    continue;
                };
                let neighbor = |dir: Direction| {
                    let o = dir.offset();
                    neighbor_of(cx as i32 + o[0], cy as i32 + o[1], cz as i32 + o[2])
                };
                emit_lod_cube(
                    sink,
                    state,
                    snapshot,
                    registry,
                    uv_map,
                    [cx as i32 * step, cy as i32 * step, cz as i32 * step],
                    step,
                    neighbor,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lod_cube(
    sink: &mut MeshSink,
    state: azalea_block::BlockState,
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    uv_map: &AtlasUVMap,
    cell_base: [i32; 3],
    step: i32,
    neighbor: impl Fn(Direction) -> LodNeighbor,
) {
    let [lx, ly, lz] = cell_base;
    let is_fluid = matches!(classify_block(state), BlockKind::Water | BlockKind::Lava);
    // A lowered fluid surface only where nothing fluid sits above, or a seam
    // shows at the LOD border.
    let fluid_top = if is_fluid && neighbor(Direction::Up) != LodNeighbor::Fluid {
        FLUID_MAX_HEIGHT
    } else {
        1.0
    };
    for dir in &CUBE_FACE_DIRS {
        // A solid neighbor cube fully covers the shared face; fluids also cull
        // against each other (no internal water faces).
        let covered = match neighbor(*dir) {
            LodNeighbor::Solid => true,
            LodNeighbor::Fluid => is_fluid,
            LodNeighbor::Empty | LodNeighbor::Outside => false,
        };
        if covered {
            continue;
        }
        let (region, tint) =
            block_face_tex_tint(state, *dir, uv_map, snapshot, registry, lx, ly, lz);
        // Past 2x, magnified sprites read as pixel mush: distant cubes render
        // as the sprite's average color instead (a white texel carries the
        // color through the tint lanes), the DH flat-color model.
        let (region, tint) = if step >= 4 {
            (uv_map.get_region(WHITE_TEXTURE), mul_tint(tint, region.avg))
        } else {
            (region, tint)
        };
        let (positions, uvs, light) = cube_face_geometry(*dir);
        let s = step as f32;
        let sy = if is_fluid { fluid_top } else { s };
        let scaled = positions.map(|p| [p[0] * s, p[1] * sy, p[2] * s]);
        // LOD cubes go in the solid pass (matching the full-detail cube path).
        emit_quad_into(
            &mut sink.solid,
            [lx as f32, ly as f32, lz as f32],
            &scaled,
            &uvs,
            [light; 4],
            region,
            tint,
        );
    }
}
const MISSING_TINT: u32 = pack_tint_shifted([1.0, 0.0, 1.0]);
#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
) {
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor =
            snapshot
                .section
                .get_block_state(lx + offset[0], ly + offset[1], lz + offset[2]);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }
        let (positions, _, light) = cube_face_geometry(*dir);
        let quad: [ChunkVertex; 4] = core::array::from_fn(|i| ChunkVertex {
            position: [
                block_pos[0] + positions[i][0],
                block_pos[1] + positions[i][1],
                block_pos[2] + positions[i][2],
            ],
            tex_coords: pack_uv(0.0, 0.0),
            light_tint: pack_light_tint(light, MISSING_TINT),
        });
        // The missing tile is a solid checker, so the cube goes in the solid pass.
        sink.solid.extend_from_slice(&quad);
    }
}
pub(crate) const CUBE_FACE_DIRS: [Direction; 6] = [
    Direction::Up,
    Direction::Down,
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];
/// Emit a face into the vertex list picked by the quad's sprite opacity
/// (solid vs cutout pass). Fluids route explicitly via [`emit_quad_into`].
#[allow(clippy::too_many_arguments)]
fn emit_face(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [f32; 4],
    region: AtlasRegion,
    tint: u32,
) {
    emit_quad_into(
        sink.pass_for(region.opaque),
        block_pos,
        positions,
        uvs,
        lights,
        region,
        tint,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_quad_into(
    dst: &mut Vec<ChunkVertex>,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [f32; 4],
    region: AtlasRegion,
    tint: u32,
) {
    let u_span = region.u_max - region.u_min;
    let v_span = region.v_max - region.v_min;
    let quad: [ChunkVertex; 4] = core::array::from_fn(|i| ChunkVertex {
        position: [
            block_pos[0] + positions[i][0],
            block_pos[1] + positions[i][1],
            block_pos[2] + positions[i][2],
        ],
        tex_coords: pack_uv(
            region.u_min + uvs[i][0] * u_span,
            region.v_min + uvs[i][1] * v_span,
        ),
        light_tint: pack_light_tint(lights[i], tint),
    });
    push_quad(dst, quad, lights);
}
fn shade_brightness(state: azalea_block::BlockState, registry: &BlockRegistry) -> f32 {
    // TODO: non-occluding full cubes (leaves, glass, ice) still darken adjacent
    // faces here. Vanilla's are `isViewBlocking=never` and don't contribute AO.
    if registry.is_opaque_full_cube(state) {
        0.2
    } else {
        1.0
    }
}
/// Centre-relative offset of vanilla's `AdjacencyInfo.corners[0]` neighbour
/// (`centre + dir + corners[0]`), the `shade0` occlusion fallback.
fn corners0_offset(dir: Direction) -> [i32; 3] {
    match dir {
        // corners[0] = EAST(+x)
        Direction::Up => [1, 1, 0],
        // corners[0] = WEST(-x)
        Direction::Down => [-1, -1, 0],
        // corners[0] = UP(+y)
        Direction::North => [0, 1, -1],
        // corners[0] = WEST(-x)
        Direction::South => [-1, 0, 1],
        // corners[0] = UP(+y)
        Direction::West => [-1, 1, 0],
        // corners[0] = DOWN(-y)
        Direction::East => [1, -1, 0],
    }
}
fn compute_face_ao(
    snapshot: &SectionStoreSnapshot,
    registry: &BlockRegistry,
    lx: i32,
    ly: i32,
    lz: i32,
    dir: Direction,
) -> [f32; 4] {
    let s = |[dx, dy, dz]: [i32; 3]| -> f32 {
        shade_brightness(
            snapshot.section.get_block_state(lx + dx, ly + dy, lz + dz),
            registry,
        )
    };
    let l = |[dx, dy, dz]: [i32; 3]| -> f32 { snapshot.get_light(lx + dx, ly + dy, lz + dz) };

    let shade0 = s(corners0_offset(dir));

    // Each vertex's (side1, side2, corner) neighbour offsets, in
    // `face_positions`' vertex order.
    let rows: [[[i32; 3]; 3]; 4] = match dir {
        Direction::Up => [
            [[0, 1, -1], [-1, 1, 0], [-1, 1, -1]],
            [[0, 1, 1], [-1, 1, 0], [-1, 1, 1]],
            [[0, 1, 1], [1, 1, 0], [1, 1, 1]],
            [[0, 1, -1], [1, 1, 0], [1, 1, -1]],
        ],
        Direction::Down => [
            [[0, -1, 1], [-1, -1, 0], [-1, -1, 1]],
            [[0, -1, -1], [-1, -1, 0], [-1, -1, -1]],
            [[0, -1, -1], [1, -1, 0], [1, -1, -1]],
            [[0, -1, 1], [1, -1, 0], [1, -1, 1]],
        ],
        Direction::North => [
            [[1, 0, -1], [0, 1, -1], [1, 1, -1]],
            [[1, 0, -1], [0, -1, -1], [1, -1, -1]],
            [[-1, 0, -1], [0, -1, -1], [-1, -1, -1]],
            [[-1, 0, -1], [0, 1, -1], [-1, 1, -1]],
        ],
        Direction::South => [
            [[-1, 0, 1], [0, 1, 1], [-1, 1, 1]],
            [[-1, 0, 1], [0, -1, 1], [-1, -1, 1]],
            [[1, 0, 1], [0, -1, 1], [1, -1, 1]],
            [[1, 0, 1], [0, 1, 1], [1, 1, 1]],
        ],
        Direction::West => [
            [[-1, 0, -1], [-1, 1, 0], [-1, 1, -1]],
            [[-1, 0, -1], [-1, -1, 0], [-1, -1, -1]],
            [[-1, 0, 1], [-1, -1, 0], [-1, -1, 1]],
            [[-1, 0, 1], [-1, 1, 0], [-1, 1, 1]],
        ],
        Direction::East => [
            [[1, 0, 1], [1, 1, 0], [1, 1, 1]],
            [[1, 0, 1], [1, -1, 0], [1, -1, 1]],
            [[1, 0, -1], [1, -1, 0], [1, -1, -1]],
            [[1, 0, -1], [1, 1, 0], [1, 1, -1]],
        ],
    };

    let n = dir.offset();
    let dir_shade = dir.shade_light();
    rows.map(|[side1, side2, corner]| {
        let ao = super::block_ao::vertex_brightness(s(side1), s(side2), s(corner), shade0);
        let light = avg4(l(n), l(side1), l(side2), l(corner));
        ao * light * dir_shade
    })
}
fn avg4(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (a + b + c + d) * 0.25
}
pub(crate) fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4], f32) {
    let (from, to) = ([0.0; 3], [1.0; 3]);
    (
        face_positions(dir, from, to),
        face_uvs(dir, from, to, None, None),
        dir.shade_light(),
    )
}
