use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use azalea_block::BlockState;

use super::block_lighter::{self, LightLevels, LightSource, VertexLight};
use super::occlusion_graph::{VisibilitySet, compute_visibility};
use crate::position::ChunkPos;
use crate::renderer::chunk::atlas::{AtlasRegion, AtlasUVMap};
use crate::world::block::model::{CardinalLighting, Direction, face_positions, face_uvs};
use crate::world::block::render_assets::{BlockRenderView, OwnedBlockRenderView, Tint};
use crate::world::block::{BlockStateData, BlockStateTable, FluidKind};
use crate::world::chunk;
use crate::world::chunk::{ChunkRing, ChunkStore, SectionBlocks};

#[derive(Copy, Clone)]
struct TerrainVertex {
    position: [f32; 3],
    /// Sprite-local UV where 1.0 spans one full sprite.
    sprite_uv: [f32; 2],
    /// `AtlasRegion::sprite`, resolved to a rectangle in the fragment shader.
    sprite: u16,
    light: VertexLight,
    /// The quad's tint, as [`pack_tint_shifted`] packs it.
    tint: u32,
}

include!("packing_consts.rs");

/// Compact terrain GPU vertex (16 bytes). `uv` packs the sprite-local UV as
/// two [`TERRAIN_UV_BITS`]-bit fixed-point axes, u low and v above it, and sky
/// light in its top byte. `sprite` indexes the atlas's rectangle buffer, which
/// the shader resolves the UV inside, avoiding atlas-boundary rounding.
/// `light_tint` is block light, then the RGB colour (tint × AO × face shade).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackedVertex {
    pub pos: [u16; 3],
    pub sprite: u16,
    pub uv: u32,
    pub light_tint: [u8; 4],
}

impl PackedVertex {
    /// The sprite-local UV in `TERRAIN_UV_FIXED_SCALE` units.
    #[cfg(any(test, feature = "cpu-bench"))]
    pub fn sprite_uv_fixed(&self) -> [u16; 2] {
        const MASK: u32 = (1 << TERRAIN_UV_BITS) - 1;
        [
            (self.uv & MASK) as u16,
            (self.uv >> TERRAIN_UV_BITS & MASK) as u16,
        ]
    }
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

fn pack_sprite_uv(x: f32) -> u32 {
    (x.clamp(0.0, 1.0) * TERRAIN_UV_FIXED_SCALE + 0.5) as u32
}

fn pack_vertex(v: &TerrainVertex) -> PackedVertex {
    PackedVertex {
        pos: [
            quantize_coord(v.position[0]),
            quantize_coord(v.position[1]),
            quantize_coord(v.position[2]),
        ],
        sprite: v.sprite,
        uv: pack_sprite_uv(v.sprite_uv[0])
            | pack_sprite_uv(v.sprite_uv[1]) << TERRAIN_UV_BITS
            | u32::from(v.light.sky) << 24,
        light_tint: {
            let [_, r, g, b] = v.tint.to_le_bytes();
            let shaded = |c: u8| (f32::from(c) * v.light.shade + 0.5) as u8;
            [v.light.block, shaded(r), shaded(g), shaded(b)]
        },
    }
}

fn section_aabb(verts: &[TerrainVertex]) -> ChunkAABB {
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

/// One 16³ section's geometry. Indices are section-local (0-based into
/// `vertices`) so each section can be uploaded as a self-contained draw with
/// its own tight AABB, giving per-section cull granularity instead of
/// per-column.
pub struct SectionMesh {
    /// 0-based section index from the column's min_y; stable identity for
    /// per-section upload/replace.
    pub section_index: i32,
    /// Vertices already quantized against the section origin in the worker, so
    /// upload is a plain memcpy.
    pub vertices: Vec<PackedVertex>,
    /// Section-local bounds of the un-quantized vertex positions, for
    /// culling (rebase via the section origin).
    pub aabb: ChunkAABB,
    /// Solid (opaque) indices first, then cutout indices. `solid_index_count`
    /// splits the two so each renders in its own pass.
    pub indices: Vec<u32>,
    /// Number of leading `indices` that belong to the solid (no-discard) pass;
    /// the rest are cutout (discard) geometry.
    pub solid_index_count: u32,
    /// Translucent (water) indices into the same `vertices`, drawn in a
    /// separate blended pass after opaque geometry.
    pub water_indices: Vec<u32>,
}

/// Per-section meshing accumulator: one shared vertex pool plus separate
/// solid, cutout, and water index lists. Finalized into a [`SectionMesh`]
/// with solid and cutout concatenated solid-first; water stays separate for
/// the blended pass.
#[derive(Default)]
struct MeshSink {
    vertices: Vec<TerrainVertex>,
    solid: Vec<u32>,
    cutout: Vec<u32>,
    water: Vec<u32>,
}

impl MeshSink {
    /// The finished section, sized exactly, with cutout indices after solid
    /// (recording the split), the section-local AABB from the float positions
    /// and the vertices quantized so upload is a plain memcpy; `None` if it
    /// drew nothing. Leaves the sink empty for the next section.
    fn finish(&mut self, section_index: i32) -> Option<SectionMesh> {
        let section = (!self.solid.is_empty() || !self.cutout.is_empty() || !self.water.is_empty())
            .then(|| {
                let mut indices = Vec::with_capacity(self.solid.len() + self.cutout.len());
                indices.extend_from_slice(&self.solid);
                indices.extend_from_slice(&self.cutout);
                SectionMesh {
                    section_index,
                    vertices: self.vertices.iter().map(pack_vertex).collect(),
                    aabb: section_aabb(&self.vertices),
                    indices,
                    solid_index_count: self.solid.len() as u32,
                    water_indices: self.water.clone(),
                }
            });
        self.vertices.clear();
        self.solid.clear();
        self.cutout.clear();
        self.water.clear();
        section
    }

    /// Index list a quad's triangles go in: solid sprites render in the
    /// no-discard pass, everything else in the discard (cutout) pass.
    fn indices_for(&mut self, opaque: bool) -> &mut Vec<u32> {
        if opaque {
            &mut self.solid
        } else {
            &mut self.cutout
        }
    }
}

/// One moving block's terrain mesh, from [`MeshDispatcher::mesh_moving_block`].
pub struct MovingBlockMesh {
    pub vertices: Vec<PackedVertex>,
    pub solid: Vec<u32>,
    pub cutout: Vec<u32>,
}

pub struct ChunkMeshData {
    pub pos: ChunkPos,
    /// Which load of `pos` this was meshed for (see `MeshColumns`);
    /// 0 for the synchronous edit path, which uploads directly.
    pub incarnation: u64,
    /// World Y of section index 0, so the buffer can derive each section's
    /// origin (`min_y + section_index * 16`) for vertex quantization.
    pub min_y: i32,
    /// Non-empty meshed sections (each tagged with its `section_index`).
    pub sections: Vec<SectionMesh>,
    /// The section-index range this job (re)meshed. Upload replaces exactly
    /// these indices: any index in the range with no `SectionMesh` is now
    /// empty and its slice is freed. `0..section_count` for a whole-column
    /// (re)mesh.
    pub replaced: std::ops::Range<i32>,
    /// The column generation this mesh was asked for (see `MeshColumns`); its
    /// dirty sections clear only if no newer generation has been marked since.
    pub content_gen: u64,
    /// Globally monotonic stamp assigned at enqueue. The buffer keeps the
    /// highest epoch uploaded per section and rejects any older upload, so an
    /// in-flight bulk mesh can never clobber a section a newer edit already
    /// uploaded (the edit always enqueues a higher epoch after its write).
    pub upload_epoch: u64,
    /// Per-section cave-cull visibility, one entry per index in `replaced`
    /// (including now-empty sections, which connect all faces).
    pub visibility: Vec<(i32, VisibilitySet)>,
    /// Latency stamps for edit remeshes (diagnostic); `None` for bulk loads.
    /// Also the drain's edit-vs-bulk discriminator, so it stays edit-only.
    pub timing: Option<RemeshTiming>,
    /// Worker-side stamps, set for every job: time spent waiting in the mesh
    /// queue and time spent meshing. Aggregated by the chunk-load benchmark.
    pub queue_ms: f32,
    pub mesh_ms: f32,
}

pub struct RemeshTiming {
    pub enqueued_at: std::time::Instant,
    pub started_at: std::time::Instant,
    pub meshed_at: std::time::Instant,
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
    pub water_color: [f32; 3],
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
            water_color: int_to_rgb(0x3F76E4),
        }
    }
}

pub(crate) fn tint_sample_y(
    tint: Tint,
    state: BlockState,
    y: i32,
    block_state_table: BlockStateTable,
) -> i32 {
    if tint == Tint::DoubleGrass
        && block_state_table
            .block_state_data(state)
            .properties()
            .get("half")
            == Some("upper")
    {
        y - 1
    } else {
        y
    }
}

fn tint_color(
    tint: Tint,
    snapshot: &ChunkStoreSnapshot,
    state: BlockState,
    x: i32,
    y: i32,
    z: i32,
) -> u32 {
    let sample_y = tint_sample_y(tint, state, y, snapshot.block_state_table);
    let color = match tint {
        Tint::None => return PACKED_WHITE_SHIFTED,
        Tint::Grass | Tint::DoubleGrass => snapshot.grass_tint(x, sample_y, z),
        Tint::Foliage => snapshot.foliage_tint(x, sample_y, z),
        Tint::DryFoliage => snapshot.dry_foliage_tint(x, sample_y, z),
        Tint::Water => snapshot.water_tint(x, sample_y, z),
        Tint::Constant(color) => int_to_rgb(color as i32),
        Tint::Redstone => snapshot
            .block_state_table
            .block_state_data(state)
            .redstone_wire_rgb(),
        Tint::Stem => snapshot
            .block_state_table
            .block_state_data(state)
            .stem_rgb(),
    };
    pack_tint_shifted(color)
}

const MAX_MESH_UPLOADS_PER_FRAME: usize = 32;

/// Bound on un-drained bulk results: past this, workers block on send (back-
/// pressure) rather than piling finished meshes — and their pooled buffers —
/// into an unbounded queue, which would starve the buffer pool.
const MAX_PENDING_RESULTS: usize = 256;

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
            .unwrap_or_else(|| Self::fallback().pixels);
        Self { pixels }
    }

    /// A map filled with vanilla's default grass color, used when the PNG
    /// can't be loaded.
    pub(crate) fn fallback() -> Self {
        Self::from_pixels(vec![[145, 189, 89]; 256 * 256])
    }

    pub(crate) fn from_pixels(pixels: Vec<[u8; 3]>) -> Self {
        Self { pixels }
    }

    fn pixel(&self, temperature: f32, downfall: f32) -> [u8; 3] {
        let t = temperature.clamp(0.0, 1.0);
        let d = (downfall.clamp(0.0, 1.0)) * t;
        let x = ((1.0 - t) * 255.0) as usize;
        let y = ((1.0 - d) * 255.0) as usize;
        let idx = (y * 256 + x).min(256 * 256 - 1);
        self.pixels[idx]
    }

    fn lookup(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        self.pixel(temperature, downfall)
            .map(|channel| channel as f32 / 255.0)
    }

    /// Vanilla `GrassColor.get` as packed `0xRRGGBB`.
    pub(crate) fn rgb(&self, temperature: f32, downfall: f32) -> u32 {
        let [r, g, b] = self.pixel(temperature, downfall);
        u32::from_be_bytes([0, r, g, b])
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

/// Vanilla's default biome-blend radius. Pomme does not expose the graphics
/// option yet, so tint rendering uses the vanilla default while matching
/// `ClientLevel.calculateBlockTint`'s integer-channel averaging exactly.
pub const DEFAULT_BIOME_BLEND_RADIUS: i32 = 2;

pub fn blend_color(x: i32, z: i32, color_at: impl FnMut(i32, i32) -> [f32; 3]) -> [f32; 3] {
    blend_color_with_radius(x, z, DEFAULT_BIOME_BLEND_RADIUS, color_at)
}

fn blend_color_with_radius(
    x: i32,
    z: i32,
    radius: i32,
    mut color_at: impl FnMut(i32, i32) -> [f32; 3],
) -> [f32; 3] {
    let radius = radius.max(0);
    let count = ((radius * 2 + 1) * (radius * 2 + 1)) as u32;
    let mut sum = [0_u32; 3];
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let color = color_at(x + dx, z + dz);
            for (total, channel) in sum.iter_mut().zip(color) {
                *total += u32::from(to_u8(channel));
            }
        }
    }
    sum.map(|total| (total / count) as f32 / 255.0)
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

/// Pre-allocation hints sized to a typical section so a fresh buffer fills
/// without reallocating (indices run ~1.5x vertices: 6 per quad vs 4).
const SECTION_VERTEX_HINT: usize = 2048;
const SECTION_INDEX_HINT: usize = 3072;

/// Recycles each worker's section scratch (float vertices and solid indices)
/// so meshing doesn't allocate per section (vanilla reuses its
/// `ByteBufferBuilder`s the same way). Holds one set per worker; a `Vec`
/// grown far past its hint is shrunk on return so one dense section doesn't
/// keep it large. Finished meshes are sized exactly and not pooled.
struct BufferPool {
    scratch_tx: crossbeam_channel::Sender<Vec<TerrainVertex>>,
    scratch_rx: crossbeam_channel::Receiver<Vec<TerrainVertex>>,
    idx_tx: crossbeam_channel::Sender<Vec<u32>>,
    idx_rx: crossbeam_channel::Receiver<Vec<u32>>,
}

impl BufferPool {
    fn new(workers: usize) -> Self {
        let (scratch_tx, scratch_rx) = crossbeam_channel::bounded(workers);
        let (idx_tx, idx_rx) = crossbeam_channel::bounded(workers);
        Self {
            scratch_tx,
            scratch_rx,
            idx_tx,
            idx_rx,
        }
    }

    // A fresh buffer is pre-sized so filling it doesn't realloc-grow; recycled
    // buffers keep their capacity, so the pool self-tunes to real section sizes.
    fn take<T>(rx: &crossbeam_channel::Receiver<Vec<T>>, hint: usize) -> Vec<T> {
        rx.try_recv().unwrap_or_else(|_| Vec::with_capacity(hint))
    }

    fn give<T>(tx: &crossbeam_channel::Sender<Vec<T>>, mut buf: Vec<T>, hint: usize) {
        if buf.capacity() > 0 {
            buf.clear();
            if buf.capacity() > 4 * hint {
                buf.shrink_to(hint);
            }
            let _ = tx.try_send(buf);
        }
    }

    fn take_scratch(&self) -> Vec<TerrainVertex> {
        Self::take(&self.scratch_rx, SECTION_VERTEX_HINT)
    }

    fn take_indices(&self) -> Vec<u32> {
        Self::take(&self.idx_rx, SECTION_INDEX_HINT)
    }

    fn recycle_scratch(&self, vertices: Vec<TerrainVertex>) {
        Self::give(&self.scratch_tx, vertices, SECTION_VERTEX_HINT);
    }

    fn recycle_indices(&self, indices: Vec<u32>) {
        Self::give(&self.idx_tx, indices, SECTION_INDEX_HINT);
    }
}

pub struct MeshDispatcher {
    result_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    result_tx: crossbeam_channel::Sender<ChunkMeshData>,
    // Edits drain ahead of and uncapped by the bulk load lane (see drain_results).
    priority_rx: crossbeam_channel::Receiver<ChunkMeshData>,
    priority_tx: crossbeam_channel::Sender<ChunkMeshData>,
    queue: Arc<MeshQueue>,
    /// The newest generation a worker has taken per column, so a generation
    /// meshes once even while it sits in successive published batches.
    claimed: Arc<ChunkRing<AtomicU64>>,
    /// The last published batch's jobs by column, reused by the next publish
    /// when it asks for the same work.
    published: HashMap<ChunkPos, Arc<PendingJob>>,
    workers: Vec<std::thread::JoinHandle<()>>,
    // Monotonic per-enqueue stamp; see `ChunkMeshData::upload_epoch`. Starts at 1
    // so 0 means "never uploaded" on the buffer side.
    next_epoch: AtomicU64,
    render_view: OwnedBlockRenderView,
    uv_map: Arc<AtlasUVMap>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    /// The dimension's face-shade table; a dimension change builds a new
    /// dispatcher.
    cardinal_lighting: CardinalLighting,
    pool: Arc<BufferPool>,
}

impl MeshDispatcher {
    /// [`Self::new`] with the grass, foliage and dry-foliage colormaps loaded
    /// from the assets (and resource packs).
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        render_view: OwnedBlockRenderView,
        uv_map: AtlasUVMap,
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<crate::assets::AssetIndex>,
        packs: Option<&crate::resource_pack::ResourcePackManager>,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        cardinal_lighting: CardinalLighting,
    ) -> Self {
        let colormap = |name| {
            Colormap::load(
                jar_assets_dir,
                asset_index,
                &format!("minecraft/textures/colormap/{name}.png"),
                packs,
            )
        };
        Self::new(
            render_view,
            uv_map,
            colormap("grass"),
            colormap("foliage"),
            colormap("dry_foliage"),
            biome_climate,
            cardinal_lighting,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        render_view: OwnedBlockRenderView,
        uv_map: AtlasUVMap,
        grass_colormap: Colormap,
        foliage_colormap: Colormap,
        dry_foliage_colormap: Colormap,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        cardinal_lighting: CardinalLighting,
    ) -> Self {
        Self::with_colormaps(
            render_view,
            Arc::new(uv_map),
            Arc::new(grass_colormap),
            Arc::new(foliage_colormap),
            Arc::new(dry_foliage_colormap),
            biome_climate,
            cardinal_lighting,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_colormaps(
        render_view: OwnedBlockRenderView,
        uv_map: Arc<AtlasUVMap>,
        grass_colormap: Arc<Colormap>,
        foliage_colormap: Arc<Colormap>,
        dry_foliage_colormap: Arc<Colormap>,
        biome_climate: Arc<HashMap<u32, BiomeClimate>>,
        cardinal_lighting: CardinalLighting,
    ) -> Self {
        // Bulk results are bounded for back-pressure; edit results use the
        // unbounded priority channel so they never queue behind the load backlog.
        let (result_tx, result_rx) = crossbeam_channel::bounded(MAX_PENDING_RESULTS);
        let (priority_tx, priority_rx) = crossbeam_channel::unbounded();

        let queue = Arc::new(MeshQueue::new());
        let claimed = Arc::new(ChunkRing::new(|| AtomicU64::new(0)));
        // Half the cores, capped. Too many saturated workers starve the
        // main/render thread during a load burst (frame spikes); the cap trades
        // some load throughput for that.
        let worker_count = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).clamp(2, 16))
            .unwrap_or(2);
        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let claimed = Arc::clone(&claimed);
            workers.push(
                std::thread::Builder::new()
                    .name("chunk-mesher".into())
                    .spawn(move || {
                        lower_current_thread_priority();
                        queue.run_worker(&claimed)
                    })
                    .expect("spawn chunk-mesher thread"),
            );
        }

        Self {
            result_rx,
            result_tx,
            priority_rx,
            priority_tx,
            queue,
            claimed,
            published: HashMap::new(),
            workers,
            next_epoch: AtomicU64::new(1),
            render_view,
            uv_map,
            grass_colormap,
            foliage_colormap,
            dry_foliage_colormap,
            biome_climate,
            cardinal_lighting,
            pool: Arc::new(BufferPool::new(worker_count)),
        }
    }

    /// Negotiation precedes world population. Capture the session's new view
    /// of the same CPU assets, retaining UVs, colormaps and dimension lighting.
    pub(crate) fn recreate_for_version(&self, render_view: OwnedBlockRenderView) -> Self {
        Self::with_colormaps(
            render_view,
            Arc::clone(&self.uv_map),
            Arc::clone(&self.grass_colormap),
            Arc::clone(&self.foliage_colormap),
            Arc::clone(&self.dry_foliage_colormap),
            Arc::clone(&self.biome_climate),
            self.cardinal_lighting,
        )
    }

    pub fn set_biome_climate(&mut self, climate: Arc<HashMap<u32, BiomeClimate>>) {
        self.biome_climate = climate;
    }

    /// (grass, foliage, dry foliage) colormaps, shared with the particle
    /// store for break-particle tinting.
    pub fn colormaps(&self) -> (Arc<Colormap>, Arc<Colormap>, Arc<Colormap>) {
        (
            Arc::clone(&self.grass_colormap),
            Arc::clone(&self.foliage_colormap),
            Arc::clone(&self.dry_foliage_colormap),
        )
    }

    /// Replaces the workers' pending work with `requests`, in vanilla
    /// `SectionTaskDynamicQueue` order from `camera`. Async worker path,
    /// vanilla's default `prioritizeChunkUpdates = NONE`; player edits use
    /// [`Self::mesh_sections_now`] instead.
    pub fn publish(
        &mut self,
        chunk_store: &ChunkStore,
        camera: glam::DVec3,
        requests: Vec<MeshRequest>,
    ) {
        let mut previous = std::mem::take(&mut self.published);
        let mut recompiles = Vec::new();
        let mut initial = Vec::new();
        for request in requests {
            let job = match previous.remove(&request.pos) {
                Some(job) if job.serves(&request) => job,
                _ => Arc::new(self.pending_job(chunk_store, request)),
            };
            let entry = (column_dist_sq(job.pos, camera), job);
            if entry.1.is_recompile {
                recompiles.push(entry);
            } else {
                initial.push(entry);
            }
        }
        let jobs = poll_order(recompiles, initial);
        self.published = jobs.iter().map(|job| (job.pos, Arc::clone(job))).collect();
        self.queue.publish(jobs);
    }

    fn pending_job(&self, chunk_store: &ChunkStore, request: MeshRequest) -> PendingJob {
        PendingJob {
            pos: request.pos,
            lod: request.lod,
            incarnation: request.incarnation,
            content_gen: request.content_gen,
            upload_epoch: self.next_epoch.fetch_add(1, Ordering::Relaxed),
            sections: request.sections,
            is_recompile: request.recompile,
            enqueued_at: std::time::Instant::now(),
            snapshot: Mutex::new(Some(self.build_snapshot(chunk_store, request.pos))),
            render_view: self.render_view.clone(),
            uv_map: Arc::clone(&self.uv_map),
            tx: if request.recompile {
                self.priority_tx.clone()
            } else {
                self.result_tx.clone()
            },
            pool: Arc::clone(&self.pool),
        }
    }

    /// The newest generation a worker has taken for `pos`.
    pub fn claimed_generation(&self, pos: ChunkPos) -> u64 {
        self.claimed.get(pos).load(Ordering::Acquire)
    }

    /// Retires `pos`'s queued work when it unloads: `generation` is newer
    /// than anything queued for it, and older than its next load's.
    pub fn cancel(&self, pos: ChunkPos, generation: u64) {
        self.claimed
            .get(pos)
            .fetch_max(generation, Ordering::AcqRel);
    }

    /// Vanilla `compileSync` (`PrioritizeChunkUpdates.PLAYER_AFFECTED`): mesh
    /// a column's edited sections on the calling thread so a player edit is
    /// renderable the same frame, skipping the worker round-trip. One
    /// snapshot serves the whole span.
    pub fn mesh_sections_now(
        &self,
        chunk_store: &ChunkStore,
        pos: ChunkPos,
        sections: std::ops::Range<i32>,
        content_gen: u64,
    ) -> ChunkMeshData {
        let started_at = std::time::Instant::now();
        let mut snapshot = self.build_snapshot(chunk_store, pos);
        let mut mesh = mesh_chunk_snapshot(
            &mut snapshot,
            pos,
            self.render_view.as_view(),
            &self.uv_map,
            0,
            sections,
            &self.pool,
        );
        mesh.content_gen = content_gen;
        mesh.upload_epoch = self.next_epoch.fetch_add(1, Ordering::Relaxed);
        mesh.mesh_ms = started_at.elapsed().as_secs_f32() * 1000.0;
        mesh
    }

    /// Vanilla `MovingBlockFeatureRenderer`: one block tessellated as terrain
    /// against a world that is air apart from it, so every face draws and only
    /// the light and biome come from the level. `cell` is the block's
    /// `MovingBlockRenderState.blockPos` (light, biome and model offset);
    /// vertices sit at `base` plus the model, in the coordinates of whatever
    /// origin the caller draws them at.
    pub fn mesh_moving_block(
        &self,
        chunk_store: &ChunkStore,
        state: azalea_block::BlockState,
        cell: [i32; 3],
        base: [f32; 3],
    ) -> MovingBlockMesh {
        let [x, y, z] = cell;
        let mut snapshot = self.build_snapshot(
            chunk_store,
            ChunkPos::new(x.div_euclid(16), z.div_euclid(16)),
        );
        let blocks = GridBox::spanning([x - 1, y - 1, z - 1], [x + 1, y + 1, z + 1]);
        let mut states = vec![azalea_block::BlockState::AIR; blocks.len()];
        states[blocks.index(cell).expect("the cell is the box's centre")] = state;
        snapshot.blocks = Some(BlockSlice { blocks, states });
        snapshot.tint_biome = Some(snapshot.get_biome(x, y, z));

        let mut sink = MeshSink::default();
        let model_offset = snapshot
            .block_state_table
            .block_state_data(state)
            .block_position_offset(x, z);
        let model_pos = (glam::Vec3::from(base) + model_offset.as_vec3()).to_array();
        emit_block_model(
            &mut sink,
            model_pos,
            state,
            &snapshot,
            self.render_view.as_view(),
            &self.uv_map,
            x,
            y,
            z,
        );
        MovingBlockMesh {
            vertices: sink.vertices.iter().map(pack_vertex).collect(),
            solid: sink.solid,
            cutout: sink.cutout,
        }
    }

    /// Point-in-time snapshot of `pos`'s mesh neighbourhood: chunk arcs plus
    /// shared handles to their light data.
    fn build_snapshot(&self, chunk_store: &ChunkStore, pos: ChunkPos) -> ChunkStoreSnapshot {
        let chunks_needed = chunk::mesh_neighborhood(pos);
        ChunkStoreSnapshot {
            block_state_table: self.render_view.block_state_table(),
            chunks: chunks_needed
                .iter()
                .map(|p| (*p, chunk_store.get_chunk(p)))
                .collect(),
            light: chunks_needed
                .iter()
                .filter_map(|p| {
                    chunk_store
                        .light_data
                        .get(&(p.x, p.z))
                        .map(|ld| ((p.x, p.z), Arc::clone(ld)))
                })
                .collect(),
            grass_colormap: Arc::clone(&self.grass_colormap),
            foliage_colormap: Arc::clone(&self.foliage_colormap),
            dry_foliage_colormap: Arc::clone(&self.dry_foliage_colormap),
            biome_climate: Arc::clone(&self.biome_climate),
            cardinal_lighting: self.cardinal_lighting,
            min_y: chunk_store.min_y(),
            height: chunk_store.height(),
            blocks: None,
            climates: None,
            light_levels: None,
            tint_biome: None,
        }
    }

    pub fn drain_results(&self) -> impl Iterator<Item = ChunkMeshData> + '_ {
        // Edits drain fully and first; bulk chunk loads stay capped per frame.
        self.priority_rx
            .try_iter()
            .chain(self.result_rx.try_iter().take(MAX_MESH_UPLOADS_PER_FRAME))
    }
}

impl Drop for MeshDispatcher {
    fn drop(&mut self) {
        self.queue.close();
        // Drop the result receiver so a worker blocked in a full bounded send
        // unblocks with a disconnect error instead of deadlocking the joins.
        let (_tx, rx) = crossbeam_channel::bounded(0);
        drop(std::mem::replace(&mut self.result_rx, rx));
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

const MAX_RECOMPILE_QUOTA: i32 = 2;

/// One column's dirty span, as `MeshColumns::requests` asks for it.
pub struct MeshRequest {
    pub pos: ChunkPos,
    pub lod: u32,
    pub incarnation: u64,
    pub content_gen: u64,
    pub sections: std::ops::Range<i32>,
    /// Re-meshes sections an edit changed (vanilla's "recompile").
    pub recompile: bool,
}

/// A pending chunk-mesh job: a point-in-time snapshot of the neighbourhood plus
/// everything `mesh_chunk_snapshot` needs. Gathered on the calling thread
/// (chunk data isn't shareable across threads), then meshed by the worker
/// that claims its generation, which takes the snapshot.
struct PendingJob {
    pos: ChunkPos,
    lod: u32,
    incarnation: u64,
    content_gen: u64,
    upload_epoch: u64,
    sections: std::ops::Range<i32>,
    is_recompile: bool,
    enqueued_at: std::time::Instant,
    snapshot: Mutex<Option<ChunkStoreSnapshot>>,
    render_view: OwnedBlockRenderView,
    uv_map: Arc<AtlasUVMap>,
    tx: crossbeam_channel::Sender<ChunkMeshData>,
    pool: Arc<BufferPool>,
}

impl PendingJob {
    fn serves(&self, request: &MeshRequest) -> bool {
        self.incarnation == request.incarnation
            && self.content_gen == request.content_gen
            && self.lod == request.lod
            && self.sections == request.sections
            && self.is_recompile == request.recompile
    }

    fn run(&self, mut snapshot: ChunkStoreSnapshot) {
        let started_at = std::time::Instant::now();
        let mut mesh = mesh_chunk_snapshot(
            &mut snapshot,
            self.pos,
            self.render_view.as_view(),
            &self.uv_map,
            self.lod,
            self.sections.clone(),
            &self.pool,
        );
        let meshed_at = std::time::Instant::now();
        mesh.incarnation = self.incarnation;
        mesh.content_gen = self.content_gen;
        mesh.upload_epoch = self.upload_epoch;
        mesh.queue_ms = (started_at - self.enqueued_at).as_secs_f32() * 1000.0;
        mesh.mesh_ms = (meshed_at - started_at).as_secs_f32() * 1000.0;
        if self.is_recompile {
            mesh.timing = Some(RemeshTiming {
                enqueued_at: self.enqueued_at,
                started_at,
                meshed_at,
            });
        }
        let _ = self.tx.send(mesh);
    }
}

/// X/Z (column) distance from `cam` to a chunk's centre. Meshing order is
/// purely horizontal distance; occlusion gates drawing, not meshing.
fn column_dist_sq(pos: ChunkPos, cam: glam::DVec3) -> f64 {
    let dx = (pos.x as f64 * 16.0 + 8.0) - cam.x;
    let dz = (pos.z as f64 * 16.0 + 8.0) - cam.z;
    dx * dx + dz * dz
}

/// Vanilla `SectionTaskDynamicQueue.poll` run over a whole batch: the nearest
/// task first, with a recompile closer than the nearest initial compile taken
/// ahead of it at most `MAX_RECOMPILE_QUOTA` times in a row.
fn poll_order<T>(mut recompiles: Vec<(f64, T)>, mut initial: Vec<(f64, T)>) -> Vec<T> {
    recompiles.sort_by(|a, b| a.0.total_cmp(&b.0));
    initial.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut order = Vec::with_capacity(recompiles.len() + initial.len());
    let mut recompiles = recompiles.into_iter().peekable();
    let mut initial = initial.into_iter().peekable();
    let mut quota = MAX_RECOMPILE_QUOTA;
    loop {
        let take_recompile = match (recompiles.peek(), initial.peek()) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some((rd, _)), Some((id, _))) => quota > 0 && rd < id,
        };
        let next = if take_recompile {
            quota -= 1;
            recompiles.next()
        } else {
            quota = MAX_RECOMPILE_QUOTA;
            initial.next()
        };
        order.extend(next.map(|(_, job)| job));
    }
    order
}

/// One published batch, taken front to back by the workers.
struct Batch {
    jobs: Box<[Arc<PendingJob>]>,
    next: AtomicUsize,
}

impl Batch {
    fn take(&self) -> Option<&Arc<PendingJob>> {
        self.jobs.get(self.next.fetch_add(1, Ordering::Relaxed))
    }

    fn exhausted(&self) -> bool {
        self.next.load(Ordering::Relaxed) >= self.jobs.len()
    }
}

/// The workers' pending work: the latest published batch, which each publish
/// replaces wholesale. A worker leaves a superseded batch at its next job.
struct MeshQueue {
    batch: Mutex<Arc<Batch>>,
    /// Bumped by every publish.
    published: AtomicU64,
    available: Condvar,
    closed: AtomicBool,
}

impl MeshQueue {
    fn new() -> Self {
        Self {
            batch: Mutex::new(Arc::new(Batch {
                jobs: Box::new([]),
                next: AtomicUsize::new(0),
            })),
            published: AtomicU64::new(0),
            available: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn publish(&self, jobs: Vec<Arc<PendingJob>>) {
        let batch = Arc::new(Batch {
            jobs: jobs.into_boxed_slice(),
            next: AtomicUsize::new(0),
        });
        // Bound so the replaced batch drops after the lock is released.
        let replaced = {
            let mut current = self.batch.lock().unwrap();
            self.published.fetch_add(1, Ordering::Release);
            std::mem::replace(&mut *current, batch)
        };
        self.available.notify_all();
        drop(replaced);
    }

    fn close(&self) {
        // Under the lock, so a worker between its check and its wait sees it.
        let guard = self.batch.lock().unwrap();
        self.closed.store(true, Ordering::Release);
        drop(guard);
        self.available.notify_all();
    }

    fn run_worker(&self, claimed: &ChunkRing<AtomicU64>) {
        let mut seen = u64::MAX;
        loop {
            let batch = {
                let mut current = self.batch.lock().unwrap();
                loop {
                    if self.closed.load(Ordering::Acquire) {
                        return;
                    }
                    let published = self.published.load(Ordering::Acquire);
                    if published != seen || !current.exhausted() {
                        seen = published;
                        break Arc::clone(&current);
                    }
                    current = self.available.wait(current).unwrap();
                }
            };
            while self.published.load(Ordering::Acquire) == seen
                && let Some(job) = batch.take()
            {
                // Each generation meshes once: the first worker to raise the
                // column's claim takes the job's snapshot.
                let generation = job.content_gen;
                let won = claimed
                    .get(job.pos)
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
                        (taken < generation).then_some(generation)
                    })
                    .is_ok();
                let Some(snapshot) = won.then(|| job.snapshot.lock().unwrap().take()).flatten()
                else {
                    continue;
                };
                // A panicking job must not kill the worker thread; its column
                // stays unmeshed until it is next marked dirty.
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.run(snapshot)))
                    .is_err()
                {
                    tracing::error!("chunk mesh job panicked; worker continuing");
                }
            }
        }
    }
}

/// Run mesh workers below normal priority so the OS preempts them for the
/// main/render thread during a load burst, while they still use idle cores.
#[cfg(windows)]
fn lower_current_thread_priority() {
    const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
fn lower_current_thread_priority() {
    // TODO: lower priority on non-Windows (libc::nice / pthread_setschedparam).
}

struct ChunkStoreSnapshot {
    block_state_table: BlockStateTable,
    chunks: Vec<(ChunkPos, Option<Arc<parking_lot::RwLock<chunk::Chunk>>>)>,
    light: std::collections::HashMap<(i32, i32), Arc<crate::world::chunk::ChunkLightData>>,
    grass_colormap: Arc<Colormap>,
    foliage_colormap: Arc<Colormap>,
    dry_foliage_colormap: Arc<Colormap>,
    biome_climate: Arc<HashMap<u32, BiomeClimate>>,
    cardinal_lighting: CardinalLighting,
    min_y: i32,
    height: u32,
    /// The meshed span's blocks, copied in by `mesh_chunk_snapshot`.
    blocks: Option<BlockSlice>,
    /// The meshed span's biome climates, filled as tints sample them.
    climates: Option<ClimateCache>,
    /// The meshed span's light levels, filled as faces sample them.
    light_levels: Option<LazyGrid<u8>>,
    /// One biome every tint reads, unblended: vanilla `MovingBlockRenderState`
    /// tints a moving block from the biome of the cell it left. `None` for
    /// terrain, which blends the biomes around each block.
    tint_biome: Option<u32>,
}

/// An axis-aligned box of grid cells, laid out X-major then Z, Y innermost
/// (the mesher walks columns bottom to top).
struct GridBox {
    min: [i32; 3],
    size: [i32; 3],
}

impl GridBox {
    /// The meshed span `ys` of column `pos`, plus a one-block border.
    fn bordered_column(pos: ChunkPos, ys: std::ops::Range<i32>) -> Self {
        Self::spanning(
            [pos.x * 16 - 1, ys.start - 1, pos.z * 16 - 1],
            [pos.x * 16 + 16, ys.end, pos.z * 16 + 16],
        )
    }

    fn spanning(min: [i32; 3], max_inclusive: [i32; 3]) -> Self {
        let size = std::array::from_fn(|i| max_inclusive[i] - min[i] + 1);
        Self { min, size }
    }

    fn len(&self) -> usize {
        (self.size[0] * self.size[1] * self.size[2]) as usize
    }

    fn index(&self, cell: [i32; 3]) -> Option<usize> {
        let [x, y, z]: [i32; 3] = std::array::from_fn(|i| cell[i] - self.min[i]);
        let inside = (0..self.size[0]).contains(&x)
            && (0..self.size[1]).contains(&y)
            && (0..self.size[2]).contains(&z);
        inside.then(|| ((x * self.size[2] + z) * self.size[1] + y) as usize)
    }
}

/// A [`GridBox`] of values computed on first read.
struct LazyGrid<T: Copy> {
    cells: GridBox,
    slots: Vec<std::cell::Cell<Option<T>>>,
}

impl<T: Copy> LazyGrid<T> {
    fn new(cells: GridBox) -> Self {
        Self {
            slots: vec![std::cell::Cell::new(None); cells.len()],
            cells,
        }
    }

    /// The cached value at `cell`, computing it with `f` on first read, or
    /// `None` outside the grid.
    fn get_or_insert_with(&self, cell: [i32; 3], f: impl FnOnce() -> T) -> Option<T> {
        let slot = &self.slots[self.cells.index(cell)?];
        Some(slot.get().unwrap_or_else(|| {
            let value = f();
            slot.set(Some(value));
            value
        }))
    }
}

/// Biome climate per biome cell (4 blocks in X/Z, exact Y) over the meshed
/// span plus the tint blend radius: each tinted face blends 25 samples, and
/// neighbouring faces share nearly all of them.
struct ClimateCache {
    /// Per cell, its index into `climates`.
    indices: LazyGrid<u16>,
    /// Distinct `(biome id, climate)` pairs seen so far.
    climates: std::cell::RefCell<Vec<(u32, BiomeClimate)>>,
}

impl ClimateCache {
    fn new(pos: ChunkPos, ys: std::ops::Range<i32>) -> Self {
        // A tint samples its block's Y or one off it (`tint_sample_y`), blended
        // over the X/Z radius around blocks up to one outside the column.
        let reach = 1 + DEFAULT_BIOME_BLEND_RADIUS;
        Self {
            indices: LazyGrid::new(GridBox::spanning(
                Self::cell([pos.x * 16 - reach, ys.start - 2, pos.z * 16 - reach]),
                Self::cell([pos.x * 16 + 15 + reach, ys.end + 1, pos.z * 16 + 15 + reach]),
            )),
            climates: Default::default(),
        }
    }

    fn cell([x, y, z]: [i32; 3]) -> [i32; 3] {
        [x.div_euclid(4), y, z.div_euclid(4)]
    }
}

/// The meshed span's block states plus a one-block border, copied once per
/// job so the mesher's per-block and neighbour reads are array loads
/// (Sodium's `WorldSlice`). Cells in chunks outside the snapshot hold air,
/// as [`ChunkStoreSnapshot::get_block_state`] reads them.
struct BlockSlice {
    blocks: GridBox,
    states: Vec<azalea_block::BlockState>,
}

impl BlockSlice {
    fn build(
        chunks: &[(ChunkPos, Option<Arc<parking_lot::RwLock<chunk::Chunk>>>)],
        pos: ChunkPos,
        min_y: i32,
        ys: std::ops::Range<i32>,
    ) -> Self {
        let blocks = GridBox::bordered_column(pos, ys);
        let mut states = vec![azalea_block::BlockState::AIR; blocks.len()];
        let [min_x, min_y_box, min_z] = blocks.min;
        let [size_x, size_y, size_z] = blocks.size;
        for (chunk_pos, chunk) in chunks {
            let Some(chunk) = chunk else { continue };
            let chunk = chunk.read();
            let span = |chunk_min: i32, box_min: i32, box_size: i32| {
                chunk_min.max(box_min)..(chunk_min + 16).min(box_min + box_size)
            };
            for y in min_y_box..min_y_box + size_y {
                // Out-of-range Y stays air.
                let Some(section) = chunk.section(y, min_y) else {
                    continue;
                };
                let single = section.single_block_state();
                for x in span(chunk_pos.x * 16, min_x, size_x) {
                    for z in span(chunk_pos.z * 16, min_z, size_z) {
                        let state = single.unwrap_or_else(|| chunk.block_state(x, y, z, min_y));
                        states[blocks.index([x, y, z]).expect("span lies inside the box")] = state;
                    }
                }
            }
        }
        Self { blocks, states }
    }

    fn get(&self, x: i32, y: i32, z: i32) -> Option<azalea_block::BlockState> {
        Some(self.states[self.blocks.index([x, y, z])?])
    }
}

impl LightSource for ChunkStoreSnapshot {
    fn state_data(&self, [x, y, z]: [i32; 3]) -> &BlockStateData {
        self.block_state_table
            .block_state_data(self.get_block_state(x, y, z))
    }

    fn levels(&self, [x, y, z]: [i32; 3]) -> LightLevels {
        self.get_light(x, y, z)
    }
}

impl ChunkStoreSnapshot {
    /// Vanilla `BlockModelLighter`: an unshaded face takes the table's up
    /// value, which is the brightest in both tables.
    fn shade(&self, face: Option<Direction>) -> f32 {
        face.map_or(self.cardinal_lighting.up, |dir| {
            self.cardinal_lighting.by_face(dir)
        })
    }

    fn get_block_state(&self, x: i32, y: i32, z: i32) -> azalea_block::BlockState {
        if let Some(state) = self.blocks.as_ref().and_then(|b| b.get(x, y, z)) {
            return state;
        }
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c): &(ChunkPos, _)| c.as_ref());

        let Some(chunk_lock) = chunk_lock else {
            return azalea_block::BlockState::AIR;
        };

        let c = chunk_lock.read();
        c.block_state(x, y, z, self.min_y)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn get_biome(&self, x: i32, y: i32, z: i32) -> u32 {
        let chunk_pos = ChunkPos::new(x.div_euclid(16), z.div_euclid(16));
        let chunk_lock = self
            .chunks
            .iter()
            .find(|(p, _)| *p == chunk_pos)
            .and_then(|(_, c)| c.as_ref());
        let Some(chunk_lock) = chunk_lock else {
            return 0;
        };
        chunk_lock
            .read()
            .biome(x, y, z, self.min_y)
            .unwrap_or_default()
    }

    fn climate_at(&self, x: i32, y: i32, z: i32) -> BiomeClimate {
        if let Some(biome) = self.tint_biome {
            return self.biome_climate_of(biome);
        }
        let biome = || self.get_biome(x, y, z);
        let Some(cache) = &self.climates else {
            return self.biome_climate_of(biome());
        };
        let index = cache
            .indices
            .get_or_insert_with(ClimateCache::cell([x, y, z]), || {
                let biome = biome();
                let mut climates = cache.climates.borrow_mut();
                let i = climates
                    .iter()
                    .position(|&(b, _)| b == biome)
                    .unwrap_or_else(|| {
                        climates.push((biome, self.biome_climate_of(biome)));
                        climates.len() - 1
                    });
                i as u16
            });
        match index {
            Some(i) => cache.climates.borrow()[i as usize].1,
            None => self.biome_climate_of(biome()),
        }
    }

    fn biome_climate_of(&self, biome: u32) -> BiomeClimate {
        self.biome_climate.get(&biome).copied().unwrap_or_default()
    }

    fn grass_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        grass_color(&self.climate_at(x, y, z), &self.grass_colormap, x, z)
    }

    fn foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        foliage_color(&self.climate_at(x, y, z), &self.foliage_colormap)
    }

    fn dry_foliage_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        dry_foliage_color(&self.climate_at(x, y, z), &self.dry_foliage_colormap)
    }

    fn water_color_at(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        self.climate_at(x, y, z).water_color
    }

    fn grass_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        if self.tint_biome.is_some() {
            return self.grass_color_at(x, y, z);
        }
        blend_color(x, z, |bx, bz| self.grass_color_at(bx, y, bz))
    }

    fn foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        if self.tint_biome.is_some() {
            return self.foliage_color_at(x, y, z);
        }
        blend_color(x, z, |bx, bz| self.foliage_color_at(bx, y, bz))
    }

    fn dry_foliage_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        if self.tint_biome.is_some() {
            return self.dry_foliage_color_at(x, y, z);
        }
        blend_color(x, z, |bx, bz| self.dry_foliage_color_at(bx, y, bz))
    }

    fn water_tint(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        if self.tint_biome.is_some() {
            return self.water_color_at(x, y, z);
        }
        blend_color(x, z, |bx, bz| self.water_color_at(bx, y, bz))
    }

    /// The block's light levels; a column without light data reads as open
    /// sky.
    fn get_light(&self, x: i32, y: i32, z: i32) -> LightLevels {
        let levels = || {
            let lx = x.rem_euclid(16);
            let lz = z.rem_euclid(16);
            match self.light.get(&(x.div_euclid(16), z.div_euclid(16))) {
                Some(light) => {
                    LightLevels::new(
                        light.get_sky_light(lx, y, lz),
                        light.get_block_light(lx, y, lz),
                    )
                    .0
                }
                None => LightLevels::new(15, 0).0,
            }
        };
        LightLevels(
            self.light_levels
                .as_ref()
                .and_then(|cache| cache.get_or_insert_with([x, y, z], levels))
                .unwrap_or_else(levels),
        )
    }
}

/// Whether a section provably emits nothing: every block is hidden once
/// enclosed, and every block touching it from outside occludes. `at` reads
/// section-relative coordinates.
fn section_hidden(registry: BlockRenderView<'_>, at: impl Fn(i32, i32, i32) -> BlockState) -> bool {
    let occludes = |x, y, z| registry.occludes_neighbor(at(x, y, z));
    let border_occludes = (0..16).all(|a| {
        (0..16).all(|b| {
            occludes(-1, a, b)
                && occludes(16, a, b)
                && occludes(a, -1, b)
                && occludes(a, 16, b)
                && occludes(a, b, -1)
                && occludes(a, b, 16)
        })
    });
    border_occludes
        && (0..16)
            .all(|y| (0..16).all(|z| (0..16).all(|x| registry.hidden_when_enclosed(at(x, y, z)))))
}

fn mesh_chunk_snapshot(
    snapshot: &mut ChunkStoreSnapshot,
    pos: ChunkPos,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    lod: u32,
    sections_to_mesh: std::ops::Range<i32>,
    pool: &BufferPool,
) -> ChunkMeshData {
    let mut logged_missing: std::collections::HashSet<&'static str> =
        std::collections::HashSet::new();

    let step = 1i32 << lod;

    let min_y = snapshot.min_y();
    let max_y = min_y + snapshot.height() as i32;
    let world_x = pos.x * 16;
    let world_z = pos.z * 16;

    let section_count = ((max_y - min_y) / 16).max(0);
    // Clamp the request; only these sections are meshed.
    let range = sections_to_mesh.start.max(0)..sections_to_mesh.end.min(section_count);
    let by_start = min_y + range.start * 16;
    let by_end = min_y + range.end * 16;

    snapshot.blocks = Some(BlockSlice::build(
        &snapshot.chunks,
        pos,
        min_y,
        by_start..by_end,
    ));
    snapshot.climates = Some(ClimateCache::new(pos, by_start..by_end));
    snapshot.light_levels = Some(LazyGrid::new(GridBox::bordered_column(
        pos,
        by_start..by_end,
    )));
    let snapshot = &*snapshot;

    // One section at a time, so a worker holds one sink's buffers, not a
    // column's.
    let mut visibility = Vec::with_capacity(range.len());
    let mut sections = Vec::new();
    let mut sink = MeshSink {
        vertices: pool.take_scratch(),
        solid: pool.take_indices(),
        ..MeshSink::default()
    };
    for si in range.clone() {
        let section_y = min_y + si * 16;
        if lod > 0 {
            // LOD > 0 (distant): treat as fully see-through. Cave culling is a
            // near-field win; the long-range pass is deferred.
            visibility.push((si, VisibilitySet::all()));
        } else {
            let at = |x: i32, y: i32, z: i32| {
                snapshot.get_block_state(world_x + x, section_y + y, world_z + z)
            };
            visibility.push((
                si,
                compute_visibility(|x, y, z| {
                    registry.is_opaque_full_cube(at(x as i32, y as i32, z as i32))
                }),
            ));
            if section_hidden(registry, at) {
                continue;
            }
        }
        let mut local_z = 0i32;
        while local_z < 16 {
            let mut local_x = 0i32;
            while local_x < 16 {
                let bx = world_x + local_x;
                let bz = world_z + local_z;

                let mut by = section_y;
                while by < section_y + 16 {
                    let mut state = snapshot.get_block_state(bx, by, bz);
                    let mut kind =
                        classify_block(snapshot.block_state_table.block_state_data(state));
                    // Checks for non air block in the cube region to represent the area if the
                    // picked block is air
                    if lod > 0 && matches!(kind, BlockKind::Air) {
                        for try_y in (by + 1)..(by + step) {
                            let s = snapshot.get_block_state(bx, try_y, bz);
                            let k = classify_block(snapshot.block_state_table.block_state_data(s));
                            if !matches!(k, BlockKind::Air) {
                                state = s;
                                kind = k;
                                break;
                            }
                        }
                    }

                    if matches!(kind, BlockKind::Air) {
                        by += step;
                        continue;
                    }

                    // Section-local base (matching the origin buffer.rs derives), so
                    // vertex positions never pass through absolute f32 world space.
                    let block_pos = [
                        (bx - world_x) as f32,
                        (by - section_y) as f32,
                        (bz - world_z) as f32,
                    ];
                    let model_offset = snapshot
                        .block_state_table
                        .block_state_data(state)
                        .block_position_offset(bx, bz);
                    let model_pos =
                        (glam::Vec3::from(block_pos) + model_offset.as_vec3()).to_array();

                    if lod > 0 {
                        emit_lod_cube(
                            &mut sink, block_pos, state, snapshot, registry, uv_map, bx, by, bz,
                            step,
                        );
                    } else if let BlockKind::Water | BlockKind::Lava = kind {
                        emit_fluid(
                            &mut sink, kind, block_pos, state, snapshot, registry, uv_map, bx, by,
                            bz,
                        );
                    } else if !emit_block_model(
                        &mut sink, model_pos, state, snapshot, registry, uv_map, bx, by, bz,
                    ) {
                        let id = snapshot.block_state_table.block_state_data(state).id();
                        if logged_missing.insert(id) {
                            tracing::warn!("Missing model: {id}");
                        }
                    }
                    by += step;
                }
                local_x += step;
            }
            local_z += step;
        }
        if let Some(section) = sink.finish(si) {
            sections.push(section);
        }
    }
    pool.recycle_scratch(sink.vertices);
    pool.recycle_indices(sink.solid);

    ChunkMeshData {
        pos,
        min_y,
        sections,
        replaced: range,
        incarnation: 0,
        content_gen: 0,
        upload_epoch: 0,
        visibility,
        timing: None,
        queue_ms: 0.0,
        mesh_ms: 0.0,
    }
}

/// A block's model: its baked variant, multipart quads or plain cube, else
/// the missing cube, in which case it returns `false`.
// Inlined: a call here costs the chunk mesher ~3%.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn emit_block_model(
    sink: &mut MeshSink,
    model_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) -> bool {
    if let Some(baked) = registry.get_baked_model(state) {
        emit_quads(
            sink,
            model_pos,
            state,
            baked.quads.iter(),
            snapshot,
            registry,
            uv_map,
            bx,
            by,
            bz,
        );
    } else if let Some(quads) = registry.get_multipart_quads(state) {
        emit_quads(
            sink,
            model_pos,
            state,
            quads.iter().copied(),
            snapshot,
            registry,
            uv_map,
            bx,
            by,
            bz,
        );
    } else if let Some(textures) = registry.get_textures(state) {
        emit_cube_faces(
            sink, model_pos, state, textures, snapshot, registry, uv_map, bx, by, bz,
        );
    } else {
        emit_missing_cube(sink, model_pos, snapshot, registry, uv_map, bx, by, bz);
        return false;
    }
    true
}

/// Vanilla `ModelBlockRenderer.tesselateBlock`'s choice: smooth lighting
/// unless the block gives off light or its model (the first part, for a
/// multipart) opts out of ambient occlusion.
fn smooth_lighting(
    snapshot: &ChunkStoreSnapshot,
    state: BlockState,
    ambient_occlusion: bool,
) -> bool {
    ambient_occlusion
        && snapshot
            .block_state_table
            .block_state_data(state)
            .light_props()
            .emission
            == 0
}

/// One quad's vertex light, through [`block_lighter`]: smooth, or flat from
/// its culled neighbour ([`neighbour_light`]).
fn quad_light(
    snapshot: &ChunkStoreSnapshot,
    smooth: bool,
    [bx, by, bz]: [i32; 3],
    positions: &[[f32; 3]; 4],
    shade_face: Option<Direction>,
    cullface: Option<Direction>,
    emission: u8,
) -> [VertexLight; 4] {
    let quad = block_lighter::Quad {
        positions,
        direction: crate::world::block::model::direction_from_positions(positions)
            .unwrap_or(Direction::Up),
        shade_face,
        emission,
    };
    let shade = |face| snapshot.shade(face);
    if smooth {
        return block_lighter::ambient_occlusion(snapshot, shade, [bx, by, bz], &quad);
    }
    let culled = cullface.map(|dir| neighbour_light(snapshot, [bx, by, bz], dir));
    block_lighter::flat(snapshot, shade, [bx, by, bz], &quad, culled)
}

/// The light a flat face toward `dir` takes: its neighbour's levels, read
/// with the block's own state, as vanilla's `tesselateFlat` does.
fn neighbour_light(snapshot: &ChunkStoreSnapshot, pos: [i32; 3], dir: Direction) -> LightLevels {
    let at = block_lighter::offset(pos, dir.offset());
    block_lighter::light_coords_of(snapshot.state_data(pos), snapshot, at)
}

/// A baked model's or multipart's quads, culled against their neighbours.
#[allow(clippy::too_many_arguments)]
fn emit_quads<'q>(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    quads: impl Iterator<Item = &'q crate::world::block::model::BakedQuad> + Clone,
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let first_ao = quads
        .clone()
        .next()
        .is_some_and(|quad| quad.ambient_occlusion);
    let smooth = smooth_lighting(snapshot, state, first_ao);
    for quad in quads {
        if let Some(cullface) = quad.cullface {
            let offset = cullface.offset();
            let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
            if registry.occludes_neighbor(neighbor) {
                continue;
            }
        }

        let region = uv_map.region(quad.texture);
        let tint = tint_color(quad.tint, snapshot, state, bx, by, bz);
        let lights = quad_light(
            snapshot,
            smooth,
            [bx, by, bz],
            &quad.positions,
            quad.shade_face,
            quad.cullface,
            quad.emission,
        );
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
    state: BlockState,
    textures: &crate::world::block::render_assets::FaceTextures,
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let tint = tint_color(textures.tint, snapshot, state, bx, by, bz);
    let smooth = smooth_lighting(snapshot, state, true);

    for (i, dir) in CUBE_FACE_DIRS.iter().enumerate() {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
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
        let (positions, uvs) = cube_face_geometry(*dir);
        let lights = quad_light(
            snapshot,
            smooth,
            [bx, by, bz],
            &positions,
            Some(*dir),
            Some(*dir),
            0,
        );

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

fn classify_block(block_state_data: &BlockStateData) -> BlockKind {
    if block_state_data.is_air() {
        return BlockKind::Air;
    }
    match block_state_data.id() {
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
// TODO: flowing water texture (water_flow) with direction-based rotation

const MAX_FLUID_HEIGHT: f32 = 8.0 / 9.0;
/// `FluidRenderer`'s `offs` anti-z-fighting inset.
const FLUID_INSET: f32 = 0.001;

fn same_fluid(block_state_data: &BlockStateData, kind: FluidKind) -> bool {
    block_state_data.fluid().kind == kind
}

/// Vanilla `FluidRenderer.isFaceOccludedByState` over the generated 16x16
/// face masks; on side faces the fluid box covers rows `0..height`.
fn fluid_face_occluded_by_state(
    block_state_data: &BlockStateData,
    direction: Direction,
    height: f32,
) -> bool {
    let props = block_state_data.light_props();
    if !props.can_occlude {
        return false;
    }
    let full_row = |v: usize| match props.face_occlusion {
        Some(masks) => masks[direction.opposite() as usize][v] == 0xFFFF,
        // TODO: masks are only dumped for useShapeForLightOcclusion states, so
        // other partial occlusion shapes (fence posts, chests) count as empty.
        None => block_state_data.outline_boxes().is_none(),
    };
    let rows = match direction {
        // `Shapes.blockOccludes` needs the fluid box to reach the top boundary.
        Direction::Up if (height - 1.0).abs() > 1.0e-7 => return false,
        Direction::Up | Direction::Down => 16,
        _ => ((height.clamp(0.0, 1.0) * 16.0).ceil() as usize).min(16),
    };
    (0..rows).all(full_row)
}

fn fluid_face_occluded_by_self(block_state_data: &BlockStateData, direction: Direction) -> bool {
    fluid_face_occluded_by_state(block_state_data, direction.opposite(), 1.0)
}

fn fluid_render_height(
    snapshot: &ChunkStoreSnapshot,
    kind: FluidKind,
    x: i32,
    y: i32,
    z: i32,
) -> f32 {
    let state = snapshot.get_block_state(x, y, z);
    let block_state_data = snapshot.block_state_table.block_state_data(state);
    let state_fluid = block_state_data.fluid();
    if state_fluid.kind == kind {
        if same_fluid(
            snapshot
                .block_state_table
                .block_state_data(snapshot.get_block_state(x, y + 1, z)),
            kind,
        ) {
            1.0
        } else {
            state_fluid.height()
        }
    } else if !block_state_data.legacy_solid() {
        0.0
    } else {
        -1.0
    }
}

fn add_weighted_fluid_height(sum: &mut f32, weight: &mut f32, height: f32) {
    if height >= 0.8 {
        *sum += height * 10.0;
        *weight += 10.0;
    } else if height >= 0.0 {
        *sum += height;
        *weight += 1.0;
    }
}

/// `FluidRenderer.calculateAverageHeight`; `corner` is only sampled when a
/// side neighbor holds fluid.
fn average_fluid_corner_height(
    height_self: f32,
    height2: f32,
    height1: f32,
    corner: impl FnOnce() -> f32,
) -> f32 {
    if height1 >= 1.0 || height2 >= 1.0 {
        return 1.0;
    }
    let mut sum = 0.0;
    let mut weight = 0.0;
    if height1 > 0.0 || height2 > 0.0 {
        let corner = corner();
        if corner >= 1.0 {
            return 1.0;
        }
        add_weighted_fluid_height(&mut sum, &mut weight, corner);
    }
    add_weighted_fluid_height(&mut sum, &mut weight, height_self);
    add_weighted_fluid_height(&mut sum, &mut weight, height1);
    add_weighted_fluid_height(&mut sum, &mut weight, height2);
    debug_assert!(weight > 0.0);
    sum / weight
}

/// Corner heights in `FluidRenderer` order (north-west, south-west,
/// south-east, north-east), before [`FLUID_INSET`].
fn fluid_corner_heights(
    snapshot: &ChunkStoreSnapshot,
    kind: FluidKind,
    bx: i32,
    by: i32,
    bz: i32,
) -> [f32; 4] {
    let height = |x, z| fluid_render_height(snapshot, kind, x, by, z);
    let self_height = height(bx, bz);
    if self_height >= 1.0 {
        return [1.0; 4];
    }
    let (north, south) = (height(bx, bz - 1), height(bx, bz + 1));
    let (west, east) = (height(bx - 1, bz), height(bx + 1, bz));
    [
        (north, west, -1, -1),
        (south, west, -1, 1),
        (south, east, 1, 1),
        (north, east, 1, -1),
    ]
    .map(|(height2, height1, dx, dz)| {
        average_fluid_corner_height(self_height, height2, height1, || height(bx + dx, bz + dz))
    })
}

#[allow(clippy::too_many_arguments)]
fn block_face_tex_tint(
    state: azalea_block::BlockState,
    dir: Direction,
    uv_map: &AtlasUVMap,
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    bx: i32,
    by: i32,
    bz: i32,
) -> (AtlasRegion, u32) {
    match classify_block(snapshot.block_state_table.block_state_data(state)) {
        BlockKind::Water => (
            uv_map.get_region("water_still"),
            pack_tint_shifted(snapshot.water_tint(bx, by, bz)),
        ),
        BlockKind::Lava => (uv_map.get_region("lava_still"), PACKED_WHITE_SHIFTED),
        _ => {
            if let Some(textures) = registry.get_textures(state) {
                let tint = tint_color(textures.tint, snapshot, state, bx, by, bz);
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
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let (region, tint) =
        block_face_tex_tint(state, Direction::Up, uv_map, snapshot, registry, bx, by, bz);
    // TODO: vanilla `FluidRenderer`'s per-face light positions.
    let fluid_light = block_lighter::light_coords(snapshot, [bx, by, bz])
        .max(block_lighter::light_coords(snapshot, [bx, by + 1, bz]));

    // Water is translucent (separate blended pass); lava is opaque.
    let MeshSink {
        vertices,
        solid,
        water,
        ..
    } = sink;
    let indices = if matches!(kind, BlockKind::Water) {
        water
    } else {
        solid
    };

    let block_state_table = snapshot.block_state_table;
    let block_state_data = block_state_table.block_state_data(state);
    let fluid_kind = block_state_data.fluid().kind;
    debug_assert!(matches!(fluid_kind, FluidKind::Water | FluidKind::Lava));
    let above = snapshot.get_block_state(bx, by + 1, bz);
    let below = snapshot.get_block_state(bx, by - 1, bz);

    // The occlusion test sees the surface before the top face's inset.
    let mut heights = fluid_corner_heights(snapshot, fluid_kind, bx, by, bz);
    let min_top = heights.iter().copied().fold(1.0_f32, f32::min);
    let render_up = !same_fluid(block_state_table.block_state_data(above), fluid_kind)
        && !fluid_face_occluded_by_state(
            block_state_table.block_state_data(above),
            Direction::Up,
            min_top,
        );
    let render_down = !same_fluid(block_state_table.block_state_data(below), fluid_kind)
        && !fluid_face_occluded_by_self(block_state_data, Direction::Down)
        && !fluid_face_occluded_by_state(
            block_state_table.block_state_data(below),
            Direction::Down,
            MAX_FLUID_HEIGHT,
        );
    let bottom_offset = if render_down { FLUID_INSET } else { 0.0 };

    if render_up {
        for height in &mut heights {
            *height -= FLUID_INSET;
        }
    }
    let [north_west, south_west, south_east, north_east] = heights;

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);

        if same_fluid(block_state_table.block_state_data(neighbor), fluid_kind) {
            continue;
        }

        let (mut positions, uvs) = cube_face_geometry(*dir);
        let light = [VertexLight::flat(snapshot.cardinal_lighting.by_face(*dir), fluid_light); 4];
        match dir {
            Direction::Up => {
                if !render_up {
                    continue;
                }
                positions[0][1] = north_west;
                positions[1][1] = south_west;
                positions[2][1] = south_east;
                positions[3][1] = north_east;

                emit_face_into(
                    vertices, indices, block_pos, &positions, &uvs, light, region, tint,
                );

                // TODO: gate on `FluidState.shouldRenderBackwardUpFace`.
                let rev_positions = [positions[0], positions[3], positions[2], positions[1]];
                let rev_uvs = [uvs[0], uvs[3], uvs[2], uvs[1]];
                emit_face_into(
                    vertices,
                    indices,
                    block_pos,
                    &rev_positions,
                    &rev_uvs,
                    light,
                    region,
                    tint,
                );
                continue;
            }
            Direction::Down => {
                if !render_down {
                    continue;
                }
                for p in &mut positions {
                    p[1] = bottom_offset;
                }
            }
            _ => {
                // (top of vertex 0, top of vertex 3, inset axis, inset plane)
                let (top0, top3, axis, plane) = match dir {
                    Direction::North => (north_east, north_west, 2, FLUID_INSET),
                    Direction::South => (south_west, south_east, 2, 1.0 - FLUID_INSET),
                    Direction::West => (north_west, south_west, 0, FLUID_INSET),
                    _ => (south_east, north_east, 0, 1.0 - FLUID_INSET),
                };
                if fluid_face_occluded_by_self(block_state_data, *dir)
                    || fluid_face_occluded_by_state(
                        block_state_table.block_state_data(neighbor),
                        *dir,
                        top0.max(top3),
                    )
                {
                    continue;
                }
                positions[0][1] = top0;
                positions[1][1] = bottom_offset;
                positions[2][1] = bottom_offset;
                positions[3][1] = top3;
                for p in &mut positions {
                    p[axis] = plane;
                }
            }
        }

        emit_face_into(
            vertices, indices, block_pos, &positions, &uvs, light, region, tint,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_lod_cube(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    state: azalea_block::BlockState,
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
    step: i32,
) {
    let block_state_table = snapshot.block_state_table;
    let block_state_data = block_state_table.block_state_data(state);
    let is_fluid = matches!(
        classify_block(block_state_data),
        BlockKind::Water | BlockKind::Lava
    );
    // We have to do this otherwise there becomes a visible seam at the LOD border
    let fluid_top = if is_fluid {
        let state_fluid = block_state_data.fluid();
        let above = snapshot.get_block_state(bx, by + 1, bz);
        if same_fluid(block_state_table.block_state_data(above), state_fluid.kind) {
            1.0
        } else {
            state_fluid.height()
        }
    } else {
        1.0
    };

    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let nx = bx + offset[0] * step;
        let ny = by + offset[1] * step;
        let nz = bz + offset[2] * step;
        let neighbor = snapshot.get_block_state(nx, ny, nz);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }
        if is_fluid
            && matches!(
                classify_block(block_state_table.block_state_data(neighbor)),
                BlockKind::Water | BlockKind::Lava
            )
        {
            continue;
        }

        let (region, tint) =
            block_face_tex_tint(state, *dir, uv_map, snapshot, registry, bx, by, bz);

        let (positions, uvs) = cube_face_geometry(*dir);
        let light = VertexLight::flat(
            snapshot.cardinal_lighting.by_face(*dir),
            block_lighter::light_coords_of(
                snapshot.state_data([bx, by, bz]),
                snapshot,
                [nx, ny, nz],
            ),
        );
        let s = step as f32;
        let sy = if is_fluid { fluid_top } else { s };
        let base = sink.vertices.len() as u32;
        for i in 0..4 {
            sink.vertices.push(TerrainVertex {
                position: [
                    block_pos[0] + positions[i][0] * s,
                    block_pos[1] + positions[i][1] * sy,
                    block_pos[2] + positions[i][2] * s,
                ],
                sprite_uv: uvs[i],
                sprite: region.sprite,
                light,
                tint,
            });
        }
        sink.indices_for(region.opaque).extend_from_slice(&[
            base,
            base + 1,
            base + 2,
            base + 2,
            base + 3,
            base,
        ]);
    }
}

const MISSING_TINT: u32 = pack_tint_shifted([1.0, 0.0, 1.0]);

#[allow(clippy::too_many_arguments)]
fn emit_missing_cube(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    snapshot: &ChunkStoreSnapshot,
    registry: BlockRenderView<'_>,
    uv_map: &AtlasUVMap,
    bx: i32,
    by: i32,
    bz: i32,
) {
    let missing = uv_map.missing_region();
    for dir in &CUBE_FACE_DIRS {
        let offset = dir.offset();
        let neighbor = snapshot.get_block_state(bx + offset[0], by + offset[1], bz + offset[2]);
        if registry.occludes_neighbor(neighbor) {
            continue;
        }

        let (positions, uvs) = cube_face_geometry(*dir);
        let light = VertexLight::flat(
            snapshot.cardinal_lighting.by_face(*dir),
            neighbour_light(snapshot, [bx, by, bz], *dir),
        );
        let base = sink.vertices.len() as u32;
        for (pos, uv) in positions.iter().zip(uvs) {
            sink.vertices.push(TerrainVertex {
                position: [
                    block_pos[0] + pos[0],
                    block_pos[1] + pos[1],
                    block_pos[2] + pos[2],
                ],
                sprite_uv: uv,
                sprite: missing.sprite,
                light,
                tint: MISSING_TINT,
            });
        }
        // The missing tile is a solid checker, so the cube goes in the solid pass.
        sink.solid
            .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
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

/// Emit a face into the index list picked by the quad's sprite opacity
/// (solid vs cutout pass). Fluids route explicitly via [`emit_face_into`].
#[allow(clippy::too_many_arguments)]
fn emit_face(
    sink: &mut MeshSink,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [VertexLight; 4],
    region: AtlasRegion,
    tint: u32,
) {
    let opaque = region.opaque;
    let MeshSink {
        vertices,
        solid,
        cutout,
        ..
    } = sink;
    let indices = if opaque { solid } else { cutout };
    emit_face_into(
        vertices, indices, block_pos, positions, uvs, lights, region, tint,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_face_into(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    block_pos: [f32; 3],
    positions: &[[f32; 3]; 4],
    uvs: &[[f32; 2]; 4],
    lights: [VertexLight; 4],
    region: AtlasRegion,
    tint: u32,
) {
    let base = vertices.len() as u32;
    for i in 0..4 {
        vertices.push(TerrainVertex {
            position: [
                block_pos[0] + positions[i][0],
                block_pos[1] + positions[i][1],
                block_pos[2] + positions[i][2],
            ],
            sprite_uv: uvs[i],
            sprite: region.sprite,
            light: lights[i],
            tint,
        });
    }

    let brightness = lights.map(VertexLight::brightness);
    if brightness[0] + brightness[2] > brightness[1] + brightness[3] {
        indices.extend_from_slice(&[base + 1, base + 2, base + 3, base + 3, base, base + 1]);
    } else {
        indices.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }
}

pub(crate) fn cube_face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4]) {
    let (from, to) = ([0.0; 3], [1.0; 3]);
    (
        face_positions(dir, from, to),
        face_uvs(dir, from, to, None, None),
    )
}

#[cfg(test)]
mod fluid_height_tests {
    use super::{
        average_fluid_corner_height, fluid_face_occluded_by_self, fluid_face_occluded_by_state,
    };
    use crate::world::block::BlockStateTable;
    use crate::world::block::model::Direction;

    #[test]
    fn fluid_corner_averaging_matches_vanilla_weighting() {
        let unsampled = || unreachable!("corner sampled without a fluid side");
        let low = 1.0_f32 / 9.0;
        let source = 8.0_f32 / 9.0;
        let isolated = average_fluid_corner_height(low, 0.0, 0.0, unsampled);
        assert!((isolated - low / 3.0).abs() < 1e-7);
        let flat = average_fluid_corner_height(low, low, low, || low);
        assert!((flat - low).abs() < 1e-7);
        let weighted = average_fluid_corner_height(source, source, source, || source);
        assert!((weighted - source).abs() < 1e-7);

        assert_eq!(average_fluid_corner_height(low, 1.0, 0.0, unsampled), 1.0);
        assert_eq!(average_fluid_corner_height(low, low, low, || 1.0), 1.0);
    }

    #[test]
    fn fluid_face_occlusion_matches_vanilla_height_aware_slab_rules() {
        let block_state_table = BlockStateTable::native();
        let stone = block_state_table.find_state("stone", &[]);
        let bottom = block_state_table
            .find_state("oak_slab", &[("type", "bottom"), ("waterlogged", "false")]);
        let top =
            block_state_table.find_state("oak_slab", &[("type", "top"), ("waterlogged", "false")]);
        let occluded = |state, direction, height| {
            fluid_face_occluded_by_state(
                block_state_table.block_state_data(state),
                direction,
                height,
            )
        };

        // Full blocks hide side faces at any height; a bottom slab only the
        // lower half.
        assert!(occluded(stone, Direction::North, 1.0 / 9.0));
        assert!(occluded(bottom, Direction::North, 0.5));
        assert!(!occluded(bottom, Direction::North, 0.75));
        assert!(!occluded(top, Direction::North, 0.5));

        // The surface is only hidden once the fluid reaches y=1.
        assert!(!occluded(stone, Direction::Up, 8.0 / 9.0));
        assert!(occluded(stone, Direction::Up, 1.0));

        // The bottom face tests the below neighbor's top face.
        assert!(occluded(top, Direction::Down, 8.0 / 9.0));
        assert!(!occluded(bottom, Direction::Down, 8.0 / 9.0));
    }

    #[test]
    fn partial_occluders_without_face_masks_keep_fluid_faces() {
        let block_state_table = BlockStateTable::native();
        for name in ["oak_fence", "cobblestone_wall", "chest"] {
            let state = block_state_table.find_state(name, &[("waterlogged", "true")]);
            for dir in [Direction::North, Direction::South, Direction::Down] {
                assert!(
                    !fluid_face_occluded_by_self(block_state_table.block_state_data(state), dir),
                    "{name} hides its own {dir:?} water face"
                );
                assert!(
                    !fluid_face_occluded_by_state(
                        block_state_table.block_state_data(state),
                        dir,
                        8.0 / 9.0
                    ),
                    "{name} hides a neighbor's {dir:?} water face"
                );
            }
        }
    }
}

#[cfg(test)]
mod terrain_uv_tests {
    use super::{
        LightLevels, PackedVertex, TERRAIN_UV_FIXED_SCALE, TerrainVertex, VertexLight, pack_vertex,
    };

    fn packed(sprite_uv: [f32; 2]) -> PackedVertex {
        pack_vertex(&TerrainVertex {
            position: [0.0; 3],
            sprite_uv,
            sprite: 0,
            light: VertexLight::flat(1.0, LightLevels::new(0, 0)),
            tint: 0,
        })
    }

    /// The shader's `unpack_sprite_uv`.
    fn decoded(sprite_uv: [f32; 2]) -> [f32; 2] {
        packed(sprite_uv)
            .sprite_uv_fixed()
            .map(|x| f32::from(x) / TERRAIN_UV_FIXED_SCALE)
    }

    #[test]
    fn sprite_edges_round_trip_exactly() {
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            assert_eq!(decoded(uv), uv);
        }
    }

    #[test]
    fn fractions_keep_their_precision() {
        const HALF_STEP: f32 = 0.5 / TERRAIN_UV_FIXED_SCALE + f32::EPSILON;
        for uv in [[0.25_f32, 0.75], [0.5, 0.0625], [0.3, 0.9]] {
            let [u, v] = decoded(uv);
            assert!((u - uv[0]).abs() <= HALF_STEP, "{uv:?} -> {u}");
            assert!((v - uv[1]).abs() <= HALF_STEP, "{uv:?} -> {v}");
        }
    }

    #[test]
    fn uvs_outside_one_sprite_clamp() {
        assert_eq!(decoded([-0.5, 1.5]), [0.0, 1.0]);
    }

    #[test]
    fn u_packs_low_v_above_and_sky_light_on_top() {
        assert_eq!(size_of::<PackedVertex>(), 16);
        assert_eq!(packed([1.0, 0.0]).uv, 0x0000_0FFF);
        assert_eq!(packed([0.0, 1.0]).uv, 0x00FF_F000);
        assert_eq!(packed([1.0, 1.0]).uv >> 24, 0);
    }

    #[test]
    fn light_packs_block_and_sky_with_the_shaded_tint() {
        let vertex = pack_vertex(&TerrainVertex {
            position: [0.0; 3],
            sprite_uv: [0.0; 2],
            sprite: 0,
            light: VertexLight::flat(0.5, LightLevels::new(15, 7)),
            tint: super::pack_tint_shifted([1.0, 0.5, 0.0]),
        });
        assert_eq!(vertex.uv >> 24, 240);
        assert_eq!(vertex.light_tint, [112, 128, 64, 0]);
    }
}

#[cfg(test)]
mod tint_tests {
    use super::{blend_color_with_radius, tint_sample_y};
    use crate::world::block::BlockStateTable;
    use crate::world::block::render_assets::Tint;

    #[test]
    fn biome_blend_matches_vanilla_integer_channel_division() {
        let color = blend_color_with_radius(0, 0, 1, |x, z| {
            let red = if x == 0 && z == 0 { 16 } else { 17 };
            [red as f32 / 255.0, 0.0, 0.0]
        });
        assert_eq!(color, [16.0 / 255.0, 0.0, 0.0]);
    }

    #[test]
    fn double_grass_upper_half_samples_the_lower_block_y() {
        let block_state_table = BlockStateTable::native();
        let lower = block_state_table.find_state("tall_grass", &[("half", "lower")]);
        let upper = block_state_table.find_state("tall_grass", &[("half", "upper")]);
        assert_eq!(
            tint_sample_y(Tint::DoubleGrass, lower, 64, block_state_table),
            64
        );
        assert_eq!(
            tint_sample_y(Tint::DoubleGrass, upper, 64, block_state_table),
            63
        );
        assert_eq!(tint_sample_y(Tint::Grass, upper, 64, block_state_table), 64);
    }
}

#[cfg(test)]
mod poll_order_tests {
    use super::poll_order;

    #[test]
    fn closer_recompiles_go_first_at_most_twice_in_a_row() {
        let recompiles = vec![(1.0, "r1"), (2.0, "r2"), (3.0, "r3")];
        let initial = vec![(5.0, "i5"), (9.0, "i9")];
        assert_eq!(
            poll_order(recompiles, initial),
            ["r1", "r2", "i5", "r3", "i9"]
        );
    }

    #[test]
    fn a_farther_recompile_waits_for_closer_initial_compiles() {
        let recompiles = vec![(5.0, "r5")];
        let initial = vec![(2.0, "i2"), (1.0, "i1")];
        assert_eq!(poll_order(recompiles, initial), ["i1", "i2", "r5"]);
    }
}
