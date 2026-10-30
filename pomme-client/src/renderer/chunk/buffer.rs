use std::collections::{HashMap, HashSet, VecDeque};
use std::mem::offset_of;

use glam::DVec3;

use super::mesher::{
    ChunkAABB, ChunkMeshData, PackVertex, PackedVertex, SectionMesh, TerrainFormat,
};
use super::tlsf::Tlsf;
use crate::position::{BlockPos, ChunkPos};
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// Vertices per arena: vanilla's 128 MiB vertex heap (`UberGpuBuffer`) at
/// the vanilla format's 16-byte vertex.
const ARENA_VERTICES: u32 = 8 << 20;
/// Indices per arena: six per four-vertex quad.
const ARENA_INDICES: u32 = ARENA_VERTICES / 2 * 3;
const INDEX_SIZE: u64 = size_of::<u32>() as u64;
/// Discrete GPUs batch a frame's uploads through the staging buffer in one
/// transfer, so it holds several columns to keep sub-flushes rare. Sized for
/// vanilla vertices; a shader pack's larger ones flush sooner.
const STAGING_SIZE: u64 = 12 << 20;
/// Without `VK_EXT_memory_budget`, the share of device-local memory the
/// arenas may take.
const VRAM_BUDGET_FRACTION: f64 = 0.25;
/// With it, the least of the budget left for everything else.
const MIN_BUDGET_HEADROOM: u64 = 512 << 20;
/// Compaction starts once the arenas' contents would fit one fewer arena
/// filled this far.
const COMPACT_FILL: f64 = 0.8;
/// Vertices compaction moves per frame.
const COMPACT_VERTICES_PER_FRAME: u32 = 1 << 18;
/// Past the eviction frontier, starved columns come back only while this
/// fraction of an arena (as `1 / n`) stays free after them.
const FRONTIER_SLACK: u64 = 16;
/// Frames before compaction tries again after running out of room.
const COMPACT_BACKOFF_FRAMES: u64 = 600;
/// Frames between checks, while columns are starved, for a budget that grew.
const BUDGET_RECHECK_FRAMES: u64 = 60;
/// Draws the meta and indirect buffers start with room for. Growing them
/// waits the GPU out, so start generously.
const INITIAL_META: usize = 16384;
/// Arenas the draw count buffers have room for: far more than any budget
/// allows.
const MAX_ARENAS: usize = 256;

/// The cull pass's bindings: meta, frustum, solid indirect/count, cutout
/// indirect/count.
const CULL_BINDINGS: &[rhi::BindGroupLayoutEntry] = &[
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::uniform(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::COMPUTE),
];
/// A draw count per arena.
const COUNTS_SIZE: u64 = (MAX_ARENAS * size_of::<u32>()) as u64;
const FRUSTUM_SIZE: u64 = size_of::<FrustumData>() as u64;
/// Per-section fade-in length, shared by the opaque indirect path and water.
const FADE_DURATION_MS: f32 = 1000.0;
/// Columns within this squared X/Z distance of the camera render opaque
/// immediately and never fade in.
const NEARBY_DIST_SQ: f32 = 768.0;

/// Whether a column's center is within the always-near X/Z radius of the eye
/// (vanilla `isNearby`), rebased in f64 for precision at extreme coordinates.
/// Also gates mesh-scheduling tiers (`in_game::apply_visibility`).
pub fn column_is_near(pos: ChunkPos, eye: DVec3) -> bool {
    column_dist_sq(pos, eye) < NEARBY_DIST_SQ as f64
}

/// The squared X/Z distance from `eye` to a column's center.
fn column_dist_sq(pos: ChunkPos, eye: DVec3) -> f64 {
    let dx = pos.x as f64 * 16.0 + 8.0 - eye.x;
    let dz = pos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dz * dz
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ChunkMeta {
    /// Section-local vertex bounds; the cull shader rebases them via `origin`.
    aabb_min: [f32; 3],
    /// Where the section's arena's draws start in the draw lists.
    draw_base: u32,
    aabb_max: [f32; 3],
    /// The section's arena, which picks its draw count.
    arena: u32,
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    visibility: u32,
    /// Absolute section origin as integers (vanilla `ChunkPosition`), bound
    /// as a per-instance vertex attribute; the vertex shader subtracts the
    /// camera block position in integer math, so no large f32 is ever
    /// formed.
    origin: [i32; 3],
    /// Read by the cull shader to split the solid/cutout draws; fills the
    /// fourth lane of `origin`'s 16-byte slot, keeping the struct at 64 bytes.
    solid_index_count: u32,
}

impl ChunkMeta {
    /// An instance entry for a draw outside the section pool: just the
    /// origin, fully faded in.
    pub(crate) fn instance(origin: [i32; 3]) -> Self {
        Self {
            origin,
            visibility: 1.0f32.to_bits(),
            ..bytemuck::Zeroable::zeroed()
        }
    }
}

/// [`PackedVertex`]'s attributes; a [`PackVertex`] starts with one.
const VERTEX_ATTRIBUTES: [rhi::VertexAttribute; 5] = {
    const POS: u32 = offset_of!(PackedVertex, pos) as u32;
    [
        // pos, split into xy + z lanes
        rhi::VertexAttribute {
            location: 0,
            format: rhi::VertexFormat::Unorm16x2,
            offset: POS,
        },
        rhi::VertexAttribute {
            location: 1,
            format: rhi::VertexFormat::Unorm16,
            offset: POS + 4,
        },
        rhi::VertexAttribute {
            location: 2,
            format: rhi::VertexFormat::Uint32,
            offset: offset_of!(PackedVertex, uv) as u32,
        },
        rhi::VertexAttribute {
            location: 3,
            format: rhi::VertexFormat::Uint16,
            offset: offset_of!(PackedVertex, sprite) as u32,
        },
        rhi::VertexAttribute {
            location: 4,
            format: rhi::VertexFormat::Unorm8x4,
            offset: offset_of!(PackedVertex, light_tint) as u32,
        },
    ]
};

const PACK_VERTEX_ATTRIBUTES: [rhi::VertexAttribute; 7] = {
    let [pos_xy, pos_z, uv, sprite, light_tint] = VERTEX_ATTRIBUTES;
    [
        pos_xy,
        pos_z,
        uv,
        sprite,
        light_tint,
        rhi::VertexAttribute {
            location: 7,
            format: rhi::VertexFormat::Unorm8x4,
            offset: offset_of!(PackVertex, shade) as u32,
        },
        rhi::VertexAttribute {
            location: 8,
            format: rhi::VertexFormat::Uint32,
            offset: offset_of!(PackVertex, entity) as u32,
        },
    ]
};

/// Vertex input for the chunk pipeline: buffer 0 is the packed per-vertex
/// pool, buffer 1 is the meta buffer read per-instance (origin + fade),
/// indexed by the `first_instance` the cull shader writes.
pub const CHUNK_VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] =
    chunk_vertex_buffers(size_of::<PackedVertex>(), &VERTEX_ATTRIBUTES);

/// [`CHUNK_VERTEX_BUFFERS`] over [`PackVertex`]es, for a shader pack.
pub const PACK_CHUNK_VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] =
    chunk_vertex_buffers(size_of::<PackVertex>(), &PACK_VERTEX_ATTRIBUTES);

const fn chunk_vertex_buffers(
    stride: usize,
    attributes: &'static [rhi::VertexAttribute],
) -> [rhi::VertexBufferLayout<'static>; 2] {
    [
        rhi::VertexBufferLayout {
            stride: stride as u32,
            step_mode: rhi::VertexStepMode::Vertex,
            attributes,
        },
        rhi::VertexBufferLayout {
            stride: size_of::<ChunkMeta>() as u32,
            step_mode: rhi::VertexStepMode::Instance,
            attributes: &[
                rhi::VertexAttribute {
                    location: 5,
                    format: rhi::VertexFormat::Sint32x3,
                    offset: offset_of!(ChunkMeta, origin) as u32,
                },
                rhi::VertexAttribute {
                    location: 6,
                    format: rhi::VertexFormat::Float32,
                    offset: offset_of!(ChunkMeta, visibility) as u32,
                },
            ],
        },
    ]
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawCommand {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
}

/// Camera-relative frustum test of a section-local AABB, mirroring `cull.comp`
/// (the GPU opaque path); used by the CPU-driven water pass. The section
/// origin is rebased against the eye in f64 for precision at extreme
/// coordinates.
fn aabb_in_frustum(aabb: &ChunkAABB, origin: [i32; 3], planes: &[[f32; 4]; 6], eye: DVec3) -> bool {
    let base = (origin_dvec(origin) - eye).as_vec3();
    let mn = [
        base.x + aabb.min[0],
        base.y + aabb.min[1],
        base.z + aabb.min[2],
    ];
    let mx = [
        base.x + aabb.max[0],
        base.y + aabb.max[1],
        base.z + aabb.max[2],
    ];
    for p in planes {
        let d = p[0] * if p[0] >= 0.0 { mx[0] } else { mn[0] }
            + p[1] * if p[1] >= 0.0 { mx[1] } else { mn[1] }
            + p[2] * if p[2] >= 0.0 { mx[2] } else { mn[2] }
            + p[3];
        if d < 0.0 {
            return false;
        }
    }
    true
}

/// An integer section origin widened for f64 math.
fn origin_dvec(origin: [i32; 3]) -> DVec3 {
    DVec3::new(origin[0] as f64, origin[1] as f64, origin[2] as f64)
}

/// Whether `pos`'s column renders opaque immediately: fading is off, or the
/// column is near the eye (a nearby column never fades in).
fn column_nearby(fade_enabled: bool, pos: ChunkPos, eye: DVec3) -> bool {
    !fade_enabled || column_is_near(pos, eye)
}

/// Rebuilds the draw lists into `out`, one per arena: one [`ChunkMeta`] per
/// uploaded section the occlusion mask keeps (all of them without one), with
/// its fade-in factor at `now`. The lists are uploaded back to back, each
/// arena's draws starting where the previous arena's end.
pub(crate) fn build_meta(
    chunks: &HashMap<ChunkPos, ChunkAlloc>,
    chunk_visibility: Option<&HashMap<ChunkPos, u32>>,
    fade_enabled: bool,
    eye: DVec3,
    now: std::time::Instant,
    out: &mut [Vec<ChunkMeta>],
) {
    out.iter_mut().for_each(Vec::clear);
    for (pos, alloc) in chunks.iter() {
        // Near columns never fade; otherwise each section fades on its own
        // timer (X/Z distance is per-column).
        let nearby = column_nearby(fade_enabled, *pos, eye);

        // CPU omission: the visibility graph's mask skips sections proven
        // occluded, so they never reach the GPU cull (absent => all draw).
        let col_vis = chunk_visibility
            .and_then(|v| v.get(pos).copied())
            .unwrap_or(u32::MAX);

        for sec in &alloc.sections {
            if col_vis & (1u32 << sec.section_index) == 0 {
                continue;
            }
            let vis = ChunkBufferStore::section_visibility(nearby, sec, now);
            let [min_x, min_y, min_z, _] = sec.aabb.min;
            let [max_x, max_y, max_z, _] = sec.aabb.max;
            out[sec.arena as usize].push(ChunkMeta {
                aabb_min: [min_x, min_y, min_z],
                draw_base: 0,
                aabb_max: [max_x, max_y, max_z],
                arena: u32::from(sec.arena),
                index_count: sec.index_count,
                first_index: sec.first_index,
                vertex_offset: sec.vertex_offset,
                visibility: vis.to_bits(),
                origin: sec.origin,
                solid_index_count: sec.solid_index_count,
            });
        }
    }
    let mut base = 0;
    for list in out {
        for meta in list.iter_mut() {
            meta.draw_base = base;
        }
        base += list.len() as u32;
    }
}

/// Sorts the draw list front-to-back by section center distance to `eye`.
pub(crate) fn sort_meta(meta: &mut [ChunkMeta], eye: DVec3) {
    // Section centers rebased against the eye in f64, for precision at
    // extreme coordinates.
    let center_dist_sq = |m: &ChunkMeta| {
        let center = DVec3::new(
            ((m.aabb_min[0] + m.aabb_max[0]) * 0.5) as f64,
            ((m.aabb_min[1] + m.aabb_max[1]) * 0.5) as f64,
            ((m.aabb_min[2] + m.aabb_max[2]) * 0.5) as f64,
        );
        (origin_dvec(m.origin) + center - eye).length_squared()
    };
    meta.sort_unstable_by(|a, b| {
        center_dist_sq(a)
            .partial_cmp(&center_dist_sq(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrustumData {
    planes: [[f32; 4]; 6],
    chunk_count: u32,
    /// Camera block position (the render anchor as integers); the cull
    /// subtracts it from the absolute integer section origins.
    cam_block: [i32; 3],
    /// Eye position relative to `cam_block` (small, full precision).
    frac: [f32; 3],
    /// Pads the struct to a 16-byte multiple so the buffer always covers the
    /// std140 block size.
    _pad: f32,
}

/// One uploaded 16³ section: a self-contained indexed draw plus its tight AABB.
/// `first_index`/`index_count` are the section's index slice and
/// `vertex_offset` its vertex slice base, both in `arena`; `vtx_len` is the
/// slice length, kept so the slices can be returned to the arena on removal.
/// `uploaded_at` drives the per-section fade so editing one section never
/// re-fades the rest of the column.
pub(crate) struct SectionAlloc {
    section_index: i32,
    aabb: ChunkAABB,
    /// Section world origin (`chunk*16`, `min_y + si*16`), used to rebase the
    /// quantized vertices and passed to the GPU via `ChunkMeta.origin`.
    origin: [i32; 3],
    arena: u16,
    first_index: u32,
    /// Opaque index count (the GPU-culled draw); water is excluded.
    index_count: u32,
    /// Leading indices belonging to the solid (no-discard) pass; the rest are
    /// cutout. Passed to the GPU via `ChunkMeta.origin[3]`.
    solid_index_count: u32,
    /// Translucent water index slice, stored right after the opaque indices in
    /// the same index allocation. Drawn in a separate blended pass.
    water_first_index: u32,
    water_index_count: u32,
    /// Total allocated index slice length (opaque + water), for freeing.
    idx_len: u32,
    vertex_offset: i32,
    vtx_len: u32,
    uploaded_at: std::time::Instant,
    /// Upload epoch this section's geometry came from; an older upload is
    /// rejected. See [`ChunkMeshData::upload_epoch`].
    epoch: u64,
}

impl SectionAlloc {
    fn slice(&self) -> Slice {
        Slice {
            arena: self.arena,
            vertex_offset: self.vertex_offset as u32,
            #[cfg(feature = "test-harness")]
            vertex_len: self.vtx_len,
            first_index: self.first_index,
            #[cfg(feature = "test-harness")]
            index_len: self.idx_len,
        }
    }
}

pub(crate) struct ChunkAlloc {
    sections: Vec<SectionAlloc>,
}

impl ChunkAlloc {
    /// What the column's sections take.
    fn need(&self) -> Need {
        let mut need = Need::default();
        for sec in &self.sections {
            need.add(sec.vtx_len, sec.idx_len);
        }
        need
    }
}

#[cfg(any(test, feature = "cpu-bench"))]
impl ChunkAlloc {
    /// A column of synthetic full-size sections at `section_indices`, all
    /// uploaded at `uploaded_at`, for tests and the CPU bench.
    pub(crate) fn synthetic(
        pos: ChunkPos,
        min_y: i32,
        section_indices: impl IntoIterator<Item = i32>,
        uploaded_at: std::time::Instant,
    ) -> Self {
        let sections = section_indices
            .into_iter()
            .map(|si| SectionAlloc {
                section_index: si,
                aabb: ChunkAABB {
                    min: [0.0, 0.0, 0.0, 0.0],
                    max: [16.0, 16.0, 16.0, 0.0],
                },
                origin: [pos.x * 16, min_y + si * 16, pos.z * 16],
                arena: 0,
                first_index: 0,
                index_count: 6,
                solid_index_count: 6,
                water_first_index: 0,
                water_index_count: 0,
                idx_len: 6,
                vertex_offset: 0,
                vtx_len: 4,
                uploaded_at,
                epoch: 0,
            })
            .collect();
        Self { sections }
    }
}

/// A slice of one arena's vertex and index heaps. Freeing goes by offset;
/// the lengths are for the harness's overlap check.
#[derive(Clone, Copy)]
struct Slice {
    arena: u16,
    vertex_offset: u32,
    #[cfg(feature = "test-harness")]
    vertex_len: u32,
    first_index: u32,
    #[cfg(feature = "test-harness")]
    index_len: u32,
}

/// What a column's sections take: in total, and the largest one's slices.
#[derive(Clone, Copy, Default)]
struct Need {
    vertices: u64,
    indices: u64,
    max_vertices: u32,
    max_indices: u32,
}

impl Need {
    fn add(&mut self, vertices: u32, indices: u32) {
        self.vertices += u64::from(vertices);
        self.indices += u64::from(indices);
        self.max_vertices = self.max_vertices.max(vertices);
        self.max_indices = self.max_indices.max(indices);
    }

    /// Whether `free_vertices` and `free_indices` hold this.
    fn fits_in(self, free_vertices: u64, free_indices: u64) -> bool {
        self.vertices <= free_vertices && self.indices <= free_indices
    }

    fn max(self, other: Self) -> Self {
        Self {
            vertices: self.vertices.max(other.vertices),
            indices: self.indices.max(other.indices),
            max_vertices: self.max_vertices.max(other.max_vertices),
            max_indices: self.max_indices.max(other.max_indices),
        }
    }
}

/// A column's sections that found no room, and what they need.
#[derive(Default)]
struct Starved {
    sections: HashSet<i32>,
    need: Need,
}

/// One vertex and one index heap, sub-allocated by [`Tlsf`]: a heap of
/// vanilla's `UberGpuBuffer`, its vertex and index halves paired so each
/// arena draws in one call. Device-local behind the staging buffer, in
/// memory of its own so destroying it hands the VRAM back; host-visible on
/// integrated GPUs.
struct Arena {
    vertex_buffer: rhi::backend::Buffer,
    index_buffer: rhi::backend::Buffer,
    vertices: Tlsf,
    indices: Tlsf,
    /// Empty, and destroyed at this `frame_seq` once no in-flight frame reads
    /// it. Never allocated from or drawn meanwhile.
    doomed: Option<u64>,
}

impl Arena {
    fn new(device: &rhi::backend::Device, use_staging: bool, format: TerrainFormat) -> Self {
        Self {
            vertex_buffer: create_pool(
                device,
                use_staging,
                "vertex_arena",
                u64::from(ARENA_VERTICES) * format.vertex_size(),
                rhi::BufferUsages::VERTEX,
            ),
            index_buffer: create_pool(
                device,
                use_staging,
                "index_arena",
                u64::from(ARENA_INDICES) * INDEX_SIZE,
                rhi::BufferUsages::INDEX,
            ),
            vertices: Tlsf::new(ARENA_VERTICES),
            indices: Tlsf::new(ARENA_INDICES),
            doomed: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.vertices.used() == 0 && self.indices.used() == 0
    }

    fn destroy(self, device: &rhi::backend::Device) {
        self.vertex_buffer.destroy(device);
        self.index_buffer.destroy(device);
    }
}

/// The meta and indirect buffer sizes for `draws` draws.
fn list_sizes(draws: usize) -> (u64, u64) {
    (
        (draws * size_of::<ChunkMeta>()) as u64,
        (draws * size_of::<DrawCommand>()) as u64,
    )
}

/// The arena that can be drawn from and allocated in: neither destroyed nor
/// doomed.
fn live(slot: &Option<Arena>) -> Option<&Arena> {
    slot.as_ref().filter(|arena| arena.doomed.is_none())
}

/// A vertex and an index slice from one live arena other than `exclude`,
/// trying the fullest first so the emptiest can drain.
fn alloc_in(
    arenas: &mut [Option<Arena>],
    exclude: Option<u16>,
    vertices: u32,
    indices: u32,
) -> Option<(u16, u32, u32)> {
    let mut order: Vec<usize> = (0..arenas.len())
        .filter(|&id| live(&arenas[id]).is_some() && exclude != Some(id as u16))
        .collect();
    order.sort_unstable_by_key(|&id| {
        std::cmp::Reverse(arenas[id].as_ref().map_or(0, |a| a.vertices.used()))
    });
    for id in order {
        let Some(arena) = arenas[id].as_mut() else {
            continue;
        };
        let Some(vertex) = arena.vertices.alloc(vertices) else {
            continue;
        };
        let Some(index) = arena.indices.alloc(indices) else {
            arena.vertices.free(vertex);
            continue;
        };
        return Some((id as u16, vertex, index));
    }
    None
}

/// Groups `copies` by destination arena and records them from `src`'s
/// buffers, or the staging buffer's halves when `src` is `None`.
fn record_copies(
    encoder: &rhi::backend::CommandEncoder,
    arenas: &[Option<Arena>],
    src: Option<&Arena>,
    staging: &rhi::backend::Buffer,
    copies: &mut [(u16, rhi::BufferCopy, rhi::BufferCopy)],
) {
    copies.sort_unstable_by_key(|&(arena, ..)| arena);
    for group in copies.chunk_by(|a, b| a.0 == b.0) {
        let Some(dst) = arenas[group[0].0 as usize].as_ref() else {
            continue;
        };
        let vertex: Vec<_> = group.iter().map(|&(_, v, _)| v).collect();
        let index: Vec<_> = group.iter().map(|&(.., i)| i).collect();
        let (src_vertex, src_index) = match src {
            Some(src) => (src.vertex_buffer.handle(), src.index_buffer.handle()),
            None => (staging.handle(), staging.handle()),
        };
        encoder.copy_buffer(src_vertex, dst.vertex_buffer.handle(), &vertex);
        encoder.copy_buffer(src_index, dst.index_buffer.handle(), &index);
    }
}

pub struct ChunkBufferStore {
    indirect_draw: IndirectDrawFeatures,
    /// By id; a destroyed arena's slot is reused by the next one.
    arenas: Vec<Option<Arena>>,
    staging_buffer: rhi::backend::Buffer,
    use_staging: bool,
    format: TerrainFormat,
    /// Without `VK_EXT_memory_budget`, what the arenas' share is of.
    device_local_bytes: u64,
    /// Columns whose sections found no room, or were evicted for nearer
    /// ones; handed back for re-meshing once there's room for them.
    starved: HashMap<ChunkPos, Starved>,
    /// The starved columns are worth retrying in the next
    /// [`Self::upload_batch`]: a slice was reclaimed, the eye reached another
    /// column, or the budget grew.
    retry_due: bool,
    /// Columns this far from the eye, squared, or farther were evicted for
    /// nearer ones, or are past those that were, and wait for room. It moves
    /// back out as room frees.
    frontier_sq: f64,
    /// The arena compaction is emptying, kept out of allocation.
    draining: Option<u16>,
    /// The `frame_seq` before which compaction doesn't start.
    compact_after: u64,

    chunks: HashMap<ChunkPos, ChunkAlloc>,
    /// Per-column bitmask of occlusion-visible section indices (bit `si`), from
    /// the CPU visibility graph. A column absent here defaults to fully
    /// visible, so freshly-loaded-but-not-yet-graphed columns still draw.
    chunk_visibility: HashMap<ChunkPos, u32>,
    /// Per arena, the camera's and the shadow view's draws.
    camera_meta: Vec<Vec<ChunkMeta>>,
    shadow_meta: Vec<Vec<ChunkMeta>>,
    /// Capacity (in draws, over every arena) of the draw lists' meta and
    /// indirect buffers.
    max_meta: usize,
    camera: DrawList,
    /// A shader pack's shadow view, made on its first cull.
    shadow: Option<DrawList>,
    /// The camera's per-arena draw counts, solid then cutout, copied after
    /// each cull for the debug overlay.
    count_readbacks: Vec<rhi::backend::Buffer>,
    meta_dirty: bool,
    /// End of the current fade-in window. While `now < fade_until` the
    /// per-section fade values change each frame, so the draw lists must be
    /// rebuilt; an O(1) check replacing the old all-sections scan.
    fade_until: std::time::Instant,
    /// Eye position at the last front-to-back sort; the sort (an early-Z
    /// optimization) is only redone once the camera moves past a threshold.
    last_sort_cam: DVec3,
    /// The eye of the last cull, which eviction measures distance from.
    eye: Option<DVec3>,

    compute_pipeline: rhi::backend::ComputePipeline,
    compute_layout: rhi::backend::PipelineLayout,
    cull_layout: rhi::backend::BindGroupLayout,

    /// The shadow lists need rebuilding from the sections.
    shadow_dirty: bool,
    fade_enabled: bool,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,

    /// Monotonic frame counter, bumped once per rendered frame in
    /// `begin_frame`.
    frame_seq: u64,
    /// Slices freed by a re-mesh, unload, eviction or compaction, each tagged
    /// with the `frame_seq` at which it's safe to reclaim
    /// (`MAX_FRAMES_IN_FLIGHT` out, so no in-flight frame still draws it).
    /// Drained in `begin_frame`.
    pending_free: VecDeque<(u64, Slice)>,
}

impl ChunkBufferStore {
    pub fn new(device: &rhi::backend::Device, indirect_draw: IndirectDrawFeatures) -> Self {
        let use_staging = device.discrete;
        let format = TerrainFormat::Vanilla;

        // The integrated path writes mapped memory directly and never touches
        // the staging buffer.
        let staging_buffer = rhi::backend::Buffer::new(
            device,
            "staging",
            if use_staging { STAGING_SIZE } else { 64 << 10 },
            rhi::BufferUsages::COPY_SRC,
            rhi::MemoryLocation::Host,
        );

        tracing::info!(
            "Chunk arenas: {}, {} MB each; GPU VRAM {} MB, budget query {}",
            if use_staging {
                "DEVICE_LOCAL + staging"
            } else {
                "HOST_VISIBLE"
            },
            arena_bytes(format) / (1024 * 1024),
            device.device_local_bytes / (1024 * 1024),
            if device.device_local_budget().is_some() {
                "on"
            } else {
                "off"
            },
        );

        let cull_layout = rhi::backend::create_bind_group_layout(device, CULL_BINDINGS);
        let compute_layout = rhi::backend::create_pipeline_layout(device, &[cull_layout], None);
        let compute_pipeline = rhi::backend::create_compute_pipeline(
            device,
            compute_layout,
            "cull",
            shader::include_shader!("cull.comp"),
        );

        Self {
            indirect_draw,
            arenas: Vec::new(),
            staging_buffer,
            use_staging,
            format,
            device_local_bytes: device.device_local_bytes,
            starved: HashMap::new(),
            retry_due: false,
            frontier_sq: f64::INFINITY,
            draining: None,
            compact_after: 0,
            chunks: HashMap::new(),
            chunk_visibility: HashMap::new(),
            camera_meta: Vec::new(),
            shadow_meta: Vec::new(),
            max_meta: INITIAL_META,
            camera: DrawList::new(device, cull_layout, INITIAL_META),
            shadow: None,
            count_readbacks: (0..MAX_FRAMES_IN_FLIGHT)
                .map(|_| {
                    rhi::backend::Buffer::new(
                        device,
                        "draw_count_readback",
                        2 * COUNTS_SIZE,
                        rhi::BufferUsages::COPY_DST,
                        rhi::MemoryLocation::Readback,
                    )
                })
                .collect(),
            meta_dirty: true,
            fade_until: std::time::Instant::now(),
            last_sort_cam: DVec3::MAX,
            eye: None,
            compute_pipeline,
            compute_layout,
            cull_layout,
            shadow_dirty: true,
            fade_enabled: false,
            last_draw_count: 0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
        }
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    /// Submit the accumulated staging copies as a single transfer and wait for
    /// it, so a frame's uploads synchronize once instead of once per mesh.
    fn flush_transfer(
        &self,
        device: &rhi::backend::Device,
        copies: &mut Vec<(u16, rhi::BufferCopy, rhi::BufferCopy)>,
    ) {
        if copies.is_empty() {
            return;
        }
        device.submit(|encoder| {
            record_copies(encoder, &self.arenas, None, &self.staging_buffer, copies);
        });
        copies.clear();
    }

    /// Upload a batch of mesh results, each replacing the sections in its
    /// `mesh.replaced` range. Staging copies for the whole batch are coalesced
    /// into as few transfers as the staging buffer holds (one per overflow,
    /// plus a final flush), each synchronized by a single fence wait — so a
    /// streaming frame stalls once, not once per mesh. Returns the starved
    /// sections that may fit now, per column, for re-meshing.
    pub fn upload_batch(
        &mut self,
        device: &rhi::backend::Device,
        meshes: &[ChunkMeshData],
    ) -> Vec<(ChunkPos, Vec<i32>)> {
        let mut needs_remesh: Vec<(ChunkPos, Vec<i32>)> = Vec::new();
        // Until something changes, the arenas are as full as before and the
        // retry would fail too.
        if std::mem::take(&mut self.retry_due) {
            self.retry_starved(device, &mut needs_remesh);
        }
        if meshes.is_empty() {
            return needs_remesh;
        }

        // Sub-allocate an exact-size vertex + index slice for each non-empty
        // section. Indices stay section-local and `vertex_offset` rebases the draw,
        // so no packing or rebasing is needed — just one slice per section.
        struct Plan<'a> {
            section_index: i32,
            verts: &'a [u8],
            vertex_count: u32,
            indices: &'a [u32],
            water_indices: &'a [u32],
            arena: u16,
            vtx_off: u32,
            idx_off: u32,
            solid_index_count: u32,
            aabb: ChunkAABB,
            origin: [i32; 3],
        }

        impl Plan<'_> {
            /// Writes the opaque then water indices at byte `off`, returning
            /// how many bytes that took.
            fn write_indices(&self, buffer: &mut rhi::backend::Buffer, off: usize) -> usize {
                let opaque: &[u8] = bytemuck::cast_slice(self.indices);
                buffer.write_at(off, opaque);
                buffer.write_at(off + opaque.len(), bytemuck::cast_slice(self.water_indices));
                opaque.len() + size_of_val(self.water_indices)
            }

            /// Opaque then water indices, which share one slice.
            fn index_len(&self) -> u32 {
                (self.indices.len() + self.water_indices.len()) as u32
            }

            fn slice(&self) -> Slice {
                Slice {
                    arena: self.arena,
                    vertex_offset: self.vtx_off,
                    #[cfg(feature = "test-harness")]
                    vertex_len: self.vertex_count,
                    first_index: self.idx_off,
                    #[cfg(feature = "test-harness")]
                    index_len: self.index_len(),
                }
            }
        }

        // Retired slices only reclaim in `begin_frame`; if rendering is paused
        // while meshing continues (e.g. minimized window) the backlog grows
        // unbounded. Past a sane bound, force a GPU wait and reclaim it all.
        const PENDING_FREE_DRAIN_THRESHOLD: usize = 8192;
        if self.pending_free.len() > PENDING_FREE_DRAIN_THRESHOLD {
            device.wait_idle();
            while let Some((_, slice)) = self.pending_free.pop_front() {
                self.free_slice(slice);
                self.retry_due = true;
            }
        }

        let staging_half = STAGING_SIZE as usize / 2;
        let vertex_size = self.format.vertex_size();
        // Copies accumulated for the current (not-yet-submitted) transfer, and the
        // running write cursors into each half of the staging buffer.
        let mut copies: Vec<(u16, rhi::BufferCopy, rhi::BufferCopy)> = Vec::new();
        let mut stg_v = 0usize;
        let mut stg_i = 0usize;

        for mesh in meshes {
            // The covered sections this job is authoritative for: reject any where
            // a newer upload (higher epoch) already landed. See
            // `ChunkMeshData::upload_epoch`.
            let accepted: HashSet<i32> = mesh
                .replaced
                .clone()
                .filter(|si| {
                    let stored = self
                        .chunks
                        .get(&mesh.pos)
                        .and_then(|c| c.sections.iter().find(|s| s.section_index == *si))
                        .map(|s| s.epoch)
                        .unwrap_or(0);
                    mesh.upload_epoch >= stored
                })
                .collect();

            // A mesh from before a format change is remeshed anyway.
            let upload_secs: Vec<&SectionMesh> = mesh
                .sections
                .iter()
                .filter(|s| accepted.contains(&s.section_index))
                .filter(|s| s.vertices.format() == self.format)
                .collect();

            if self.use_staging && !upload_secs.is_empty() {
                // Verts and indices share the staging buffer (two halves). A chunk
                // too large for one half is dropped rather than overflowing the
                // buffer. This is permanent, so it's not reported for retry.
                let v_bytes: usize = upload_secs.iter().map(|s| s.vertices.bytes().len()).sum();
                let i_bytes: usize = upload_secs
                    .iter()
                    .map(|s| (s.indices.len() + s.water_indices.len()) * INDEX_SIZE as usize)
                    .sum();
                if v_bytes > staging_half || i_bytes > staging_half {
                    tracing::warn!(
                        "Chunk {:?} too large for staging ({} v / {} i bytes), skipping",
                        mesh.pos,
                        v_bytes,
                        i_bytes,
                    );
                    self.retire_sections(mesh.pos, &accepted);
                    continue;
                }
            }

            // Allocate the new slices before retiring the old: no room then
            // keeps the old mesh drawn instead of dropping it.
            let mut plans: Vec<Plan> = Vec::with_capacity(upload_secs.len());
            let mut need = Need::default();
            // Past the eviction frontier a column waits for room instead.
            let beyond = self.beyond_frontier(mesh.pos);
            let mut no_room = beyond;
            for sec in &upload_secs {
                let vcount = sec.vertices.len() as u32;
                // Opaque and water indices share one slice (opaque first, water after).
                let icount = (sec.indices.len() + sec.water_indices.len()) as u32;
                if vcount == 0 || icount == 0 {
                    continue;
                }
                need.add(vcount, icount);
                if no_room {
                    continue;
                }
                let Some((arena, vtx_off, idx_off)) = self.alloc_slice(device, vcount, icount)
                else {
                    no_room = true;
                    continue;
                };
                debug_assert_eq!(
                    sec.vertices.bytes().len() as u64,
                    u64::from(vcount) * vertex_size
                );
                plans.push(Plan {
                    section_index: sec.section_index,
                    verts: sec.vertices.bytes(),
                    vertex_count: vcount,
                    indices: &sec.indices,
                    water_indices: &sec.water_indices,
                    arena,
                    vtx_off,
                    idx_off,
                    solid_index_count: sec.solid_index_count,
                    aabb: sec.aabb,
                    origin: [
                        mesh.pos.x * 16,
                        mesh.min_y + sec.section_index * 16,
                        mesh.pos.z * 16,
                    ],
                });
            }
            if no_room {
                // Never drawn, so these go straight back.
                for p in &plans {
                    self.free_slice(p.slice());
                }
                if beyond {
                    // Its old mesh would go stale; it waits with the rest.
                    self.evict(mesh.pos);
                } else {
                    tracing::debug!("No room for {:?}'s mesh, keeping its old one", mesh.pos);
                    self.evict_beyond(mesh.pos, need);
                }
                self.starve(mesh.pos, accepted, need);
                continue;
            }

            // Remember which sections were present so a re-meshed one swaps
            // instantly while a freshly revealed one still fades in.
            let was_present = self.retire_sections(mesh.pos, &accepted);
            if plans.is_empty() {
                // Every accepted section is now empty; drop the column if
                // nothing remains.
                if self
                    .chunks
                    .get(&mesh.pos)
                    .is_some_and(|c| c.sections.is_empty())
                {
                    self.chunks.remove(&mesh.pos);
                }
                continue;
            }

            if self.use_staging {
                let mv: usize = plans.iter().map(|p| p.verts.len()).sum();
                let mi: usize = plans
                    .iter()
                    .map(|p| p.index_len() as usize * INDEX_SIZE as usize)
                    .sum();
                // This mesh alone fits a half (checked above), so a flush always
                // makes room: submit the pending transfer and reset the cursors.
                if stg_v + mv > staging_half || stg_i + mi > staging_half {
                    self.flush_transfer(device, &mut copies);
                    stg_v = 0;
                    stg_i = 0;
                }
                for p in &plans {
                    self.staging_buffer.write_at(stg_v, p.verts);
                    let vbytes = p.verts.len();
                    let vertex = rhi::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: u64::from(p.vtx_off) * vertex_size,
                        size: vbytes as u64,
                    };
                    stg_v += vbytes;

                    let off = staging_half + stg_i;
                    let ibytes = p.write_indices(&mut self.staging_buffer, off);
                    let index = rhi::BufferCopy {
                        src_offset: off as u64,
                        dst_offset: u64::from(p.idx_off) * INDEX_SIZE,
                        size: ibytes as u64,
                    };
                    stg_i += ibytes;
                    copies.push((p.arena, vertex, index));
                }
            } else {
                for p in &plans {
                    let Some(arena) = self.arenas[p.arena as usize].as_mut() else {
                        continue;
                    };
                    let base = u64::from(p.vtx_off) * vertex_size;
                    arena.vertex_buffer.write_at(base as usize, p.verts);
                    p.write_indices(
                        &mut arena.index_buffer,
                        p.idx_off as usize * INDEX_SIZE as usize,
                    );
                }
            }

            let now = std::time::Instant::now();
            let new_sections = plans.iter().map(|p| SectionAlloc {
                section_index: p.section_index,
                aabb: p.aabb,
                origin: p.origin,
                arena: p.arena,
                first_index: p.idx_off,
                index_count: p.indices.len() as u32,
                solid_index_count: p.solid_index_count,
                water_first_index: p.idx_off + p.indices.len() as u32,
                water_index_count: p.water_indices.len() as u32,
                idx_len: p.index_len(),
                vertex_offset: p.vtx_off as i32,
                vtx_len: p.vertex_count,
                // A re-meshed section swaps instantly; a freshly revealed one fades in.
                uploaded_at: if was_present.contains(&p.section_index) {
                    now.checked_sub(std::time::Duration::from_secs(2))
                        .unwrap_or(now)
                } else {
                    now
                },
                epoch: mesh.upload_epoch,
            });

            // Freshly revealed sections fade in, so extend the fade window the
            // cull's O(1) check reads; re-meshed-only uploads swap instantly.
            // Nearby columns never fade, so extending for them only forces
            // redundant rebuilds — skip them. `last_sort_cam` is the camera the
            // draw list is keyed to (unset => far, the safe default).
            let revealed = plans
                .iter()
                .any(|p| !was_present.contains(&p.section_index));
            if revealed && !column_nearby(self.fade_enabled, mesh.pos, self.last_sort_cam) {
                let dur = std::time::Duration::from_secs_f32(FADE_DURATION_MS / 1000.0);
                self.fade_until = self.fade_until.max(now + dur);
            }

            self.chunks
                .entry(mesh.pos)
                .or_insert_with(|| ChunkAlloc {
                    sections: Vec::new(),
                })
                .sections
                .extend(new_sections);
        }

        // Flush whatever remains accumulated from the last (or only) batch.
        self.flush_transfer(device, &mut copies);
        self.ensure_meta_capacity(device);

        needs_remesh
    }

    /// Grow the draw lists' meta and indirect buffers to hold every
    /// section's draw. No-op while capacity suffices.
    fn ensure_meta_capacity(&mut self, device: &rhi::backend::Device) {
        let needed: usize = self.chunks.values().map(|c| c.sections.len()).sum();
        if needed <= self.max_meta {
            return;
        }
        // The meta/indirect buffers are referenced by every in-flight frame's
        // bind group; wait the GPU out before freeing them.
        device.wait_idle();
        self.max_meta = (needed.saturating_mul(3) / 2)
            .next_power_of_two()
            .max(self.max_meta * 2);
        let (layout, draws) = (self.cull_layout, self.max_meta);
        for list in self.draw_lists_mut() {
            list.regrow(device, layout, draws);
        }
    }

    /// The camera's draw list and a shader pack's shadow one, if made.
    fn draw_lists_mut(&mut self) -> impl Iterator<Item = &mut DrawList> {
        std::iter::once(&mut self.camera).chain(&mut self.shadow)
    }

    /// Return one slice's vertex and index ranges to its arena.
    fn free_slice(&mut self, slice: Slice) {
        if let Some(arena) = self.arenas[slice.arena as usize].as_mut() {
            arena.vertices.free(slice.vertex_offset);
            arena.indices.free(slice.first_index);
        }
    }

    /// A vertex and an index slice in one arena, adding an arena when they're
    /// all full; `None` once that would pass the budget.
    fn alloc_slice(
        &mut self,
        device: &rhi::backend::Device,
        vertices: u32,
        indices: u32,
    ) -> Option<(u16, u32, u32)> {
        if let Some(found) = alloc_in(&mut self.arenas, self.draining, vertices, indices) {
            return Some(found);
        }
        if !self.can_add_arena(device) {
            return None;
        }
        self.add_arena(device);
        alloc_in(&mut self.arenas, self.draining, vertices, indices)
    }

    /// Whether one more arena fits the budget: what `VK_EXT_memory_budget`
    /// says this process may use, less headroom for everything else, or
    /// without it a share of device-local memory. The first always fits.
    fn can_add_arena(&self, device: &rhi::backend::Device) -> bool {
        let arenas = self.arena_count();
        if arenas == 0 {
            return true;
        }
        if arenas as usize >= MAX_ARENAS {
            return false;
        }
        let bytes = arena_bytes(self.format);
        match device.device_local_budget() {
            Some(budget) => {
                let headroom = MIN_BUDGET_HEADROOM.max(budget.budget / 8);
                budget.usage + bytes + headroom <= budget.budget
            }
            None => {
                (arenas + 1) * bytes
                    <= (self.device_local_bytes as f64 * VRAM_BUDGET_FRACTION) as u64
            }
        }
    }

    fn add_arena(&mut self, device: &rhi::backend::Device) {
        let arena = Arena::new(device, self.use_staging, self.format);
        match self.arenas.iter().position(Option::is_none) {
            Some(id) => self.arenas[id] = Some(arena),
            None => self.arenas.push(Some(arena)),
        }
        self.camera_meta.resize_with(self.arenas.len(), Vec::new);
        self.shadow_meta.resize_with(self.arenas.len(), Vec::new);
        self.log_arenas("added");
    }

    /// Arenas allocated, doomed ones included.
    fn arena_count(&self) -> u64 {
        self.arenas.iter().flatten().count() as u64
    }

    fn log_arenas(&self, change: &str) {
        let arenas = self.arena_count();
        tracing::info!(
            "Chunk arena {change}: {arenas} arenas, {} MB",
            arenas * arena_bytes(self.format) / (1024 * 1024)
        );
    }

    /// Evicts the columns farthest from the eye, all farther than `pos`, until
    /// they free what `pos` needs, so a full budget draws a shorter distance
    /// instead of leaving holes nearby. The evicted columns starve until
    /// there's room for them again.
    fn evict_beyond(&mut self, pos: ChunkPos, need: Need) {
        let Some(eye) = self.eye else {
            return;
        };
        let loaded = self
            .chunks
            .iter()
            .map(|(&p, alloc)| (column_dist_sq(p, eye), p, alloc.need()));
        let victims = eviction_victims(loaded, column_dist_sq(pos, eye), need);
        let Some(&(nearest, _)) = victims.last() else {
            return;
        };
        for (_, victim) in victims {
            self.evict(victim);
            tracing::debug!("Evicted {victim:?} for {pos:?}");
        }
        self.frontier_sq = self.frontier_sq.min(nearest);
    }

    /// Drops `pos`'s mesh, starving the column until there's room for it.
    fn evict(&mut self, pos: ChunkPos) {
        let Some(alloc) = self.chunks.remove(&pos) else {
            return;
        };
        self.retire_slices(alloc.sections.iter().map(SectionAlloc::slice));
        let sections = alloc.sections.iter().map(|s| s.section_index);
        self.starve(pos, sections, alloc.need());
        self.meta_dirty = true;
    }

    fn starve(&mut self, pos: ChunkPos, sections: impl IntoIterator<Item = i32>, need: Need) {
        let starved = self.starved.entry(pos).or_default();
        starved.sections.extend(sections);
        starved.need = starved.need.max(need);
    }

    /// Whether `pos` is at or past the eviction frontier.
    fn beyond_frontier(&self, pos: ChunkPos) -> bool {
        self.eye
            .is_some_and(|eye| column_dist_sq(pos, eye) >= self.frontier_sq)
    }

    /// Hands the starved columns that could fit now back for re-meshing: all
    /// of them while another arena fits the budget, else those
    /// [`columns_to_retry`] picks.
    fn retry_starved(
        &mut self,
        device: &rhi::backend::Device,
        out: &mut Vec<(ChunkPos, Vec<i32>)>,
    ) {
        if self.starved.is_empty() {
            return;
        }
        if self.can_add_arena(device) {
            self.frontier_sq = f64::INFINITY;
            out.extend(
                self.starved
                    .drain()
                    .map(|(pos, starved)| (pos, starved.sections.into_iter().collect())),
            );
            return;
        }
        let arenas: Vec<&Arena> = self
            .arenas
            .iter()
            .enumerate()
            .filter(|&(id, _)| self.draining != Some(id as u16))
            .filter_map(|(_, slot)| live(slot))
            .collect();
        let free_vertices: u64 = arenas
            .iter()
            .map(|a| u64::from(a.vertices.capacity() - a.vertices.used()))
            .sum();
        let free_indices: u64 = arenas
            .iter()
            .map(|a| u64::from(a.indices.capacity() - a.indices.used()))
            .sum();
        let dist = |pos| self.eye.map_or(0.0, |eye| column_dist_sq(pos, eye));
        let farthest_loaded = self
            .chunks
            .keys()
            .map(|&pos| dist(pos))
            .fold(f64::NEG_INFINITY, f64::max);
        let waiting = self.starved.iter().map(|(&pos, s)| Waiting {
            dist: dist(pos),
            pos,
            need: s.need,
            fits_an_arena: arenas.iter().any(|a| {
                a.vertices.fits(s.need.max_vertices) && a.indices.fits(s.need.max_indices)
            }),
        });
        let (retry, frontier_sq) = columns_to_retry(
            waiting,
            (free_vertices, free_indices),
            self.frontier_sq,
            farthest_loaded,
        );
        self.frontier_sq = frontier_sq;
        for pos in retry {
            if let Some(starved) = self.starved.remove(&pos) {
                out.push((pos, starved.sections.into_iter().collect()));
            }
        }
    }

    /// Retires `pos`'s `sections`, returning those it had; the others stay.
    fn retire_sections(&mut self, pos: ChunkPos, sections: &HashSet<i32>) -> HashSet<i32> {
        let mut freed = Vec::new();
        let mut was_present = HashSet::new();
        if let Some(entry) = self.chunks.get_mut(&pos) {
            entry.sections.retain(|s| {
                if sections.contains(&s.section_index) {
                    was_present.insert(s.section_index);
                    freed.push(s.slice());
                    false
                } else {
                    true
                }
            });
        }
        self.retire_slices(freed);
        // The draw list must drop them before their slices are reused.
        self.meta_dirty = true;
        was_present
    }

    /// Defer returning slices to the arenas until `MAX_FRAMES_IN_FLIGHT`
    /// frames have passed, so the GPU can't still be reading them from an
    /// in-flight frame. Use for slices that were potentially drawn.
    fn retire_slices(&mut self, slices: impl IntoIterator<Item = Slice>) {
        let retire_at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        for slice in slices {
            self.pending_free.push_back((retire_at, slice));
        }
    }

    /// Advance one frame, reclaim any slices whose retirement deadline has
    /// passed, then free empty arenas and compact. Call once per rendered
    /// frame, right after the frame's fence has been waited (that wait
    /// guarantees the frame from `MAX_FRAMES_IN_FLIGHT` ago — and everything
    /// before it — is done on the GPU).
    pub fn begin_frame(&mut self, device: &rhi::backend::Device) {
        self.frame_seq += 1;
        while self
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.frame_seq)
        {
            let (_, slice) = self.pending_free.pop_front().unwrap();
            self.free_slice(slice);
            self.retry_due = true;
        }
        if !self.starved.is_empty()
            && self.frame_seq.is_multiple_of(BUDGET_RECHECK_FRAMES)
            && self.can_add_arena(device)
        {
            self.retry_due = true;
        }
        self.free_empty_arenas(device);
        self.compact(device);
    }

    /// Destroys doomed arenas no frame reads any more, and dooms empty ones,
    /// as vanilla destroys an empty heap, keeping one for the next upload.
    fn free_empty_arenas(&mut self, device: &rhi::backend::Device) {
        let mut destroyed = false;
        for (id, slot) in self.arenas.iter_mut().enumerate() {
            if slot
                .as_ref()
                .and_then(|a| a.doomed)
                .is_some_and(|at| at <= self.frame_seq)
            {
                if let Some(arena) = slot.take() {
                    arena.destroy(device);
                }
                self.camera_meta[id].clear();
                self.shadow_meta[id].clear();
                destroyed = true;
            }
        }
        if destroyed {
            self.log_arenas("freed");
        }
        let mut live_count = self.arenas.iter().filter_map(live).count();
        for (id, slot) in self.arenas.iter_mut().enumerate() {
            let Some(arena) = slot.as_mut() else {
                continue;
            };
            if live_count > 1 && arena.doomed.is_none() && arena.is_empty() {
                // Frames from here on skip it; earlier ones are done by then.
                arena.doomed = Some(self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64);
                live_count -= 1;
                if self.draining == Some(id as u16) {
                    self.draining = None;
                }
            }
        }
    }

    /// Once the arenas' contents would fit one fewer, drains the emptiest
    /// into the rest, a bounded amount per frame, so freeing it can shrink
    /// the total.
    /// Not while columns are starved: they want the room, and the arena it
    /// frees would only be added back for them.
    fn compact(&mut self, device: &rhi::backend::Device) {
        if !self.starved.is_empty() {
            self.draining = None;
            return;
        }
        if self.frame_seq < self.compact_after {
            return;
        }
        let from = match self.draining {
            Some(id) => id,
            None => {
                let live = self.arenas.iter().enumerate().filter_map(|(id, slot)| {
                    live(slot).map(|a| (id as u16, a.vertices.used(), a.indices.used()))
                });
                let Some(id) = arena_to_drain(live) else {
                    return;
                };
                tracing::debug!("Compacting chunk arena {id}");
                self.draining = Some(id);
                id
            }
        };

        let vertex_size = self.format.vertex_size();
        let mut copies: Vec<(u16, rhi::BufferCopy, rhi::BufferCopy)> = Vec::new();
        let mut retired = Vec::new();
        let mut moved = 0;
        'columns: for alloc in self.chunks.values_mut() {
            for sec in alloc.sections.iter_mut().filter(|s| s.arena == from) {
                if moved >= COMPACT_VERTICES_PER_FRAME {
                    break 'columns;
                }
                let Some((to, vertex, index)) =
                    alloc_in(&mut self.arenas, Some(from), sec.vtx_len, sec.idx_len)
                else {
                    tracing::debug!("No room to compact chunk arena {from}");
                    self.draining = None;
                    self.compact_after = self.frame_seq + COMPACT_BACKOFF_FRAMES;
                    break 'columns;
                };
                copies.push((
                    to,
                    rhi::BufferCopy {
                        src_offset: sec.vertex_offset as u64 * vertex_size,
                        dst_offset: u64::from(vertex) * vertex_size,
                        size: u64::from(sec.vtx_len) * vertex_size,
                    },
                    rhi::BufferCopy {
                        src_offset: u64::from(sec.first_index) * INDEX_SIZE,
                        dst_offset: u64::from(index) * INDEX_SIZE,
                        size: u64::from(sec.idx_len) * INDEX_SIZE,
                    },
                ));
                retired.push(sec.slice());
                sec.arena = to;
                sec.water_first_index = index + (sec.water_first_index - sec.first_index);
                sec.first_index = index;
                sec.vertex_offset = vertex as i32;
                moved += sec.vtx_len;
            }
        }
        if copies.is_empty() {
            return;
        }
        let src = self.arenas[from as usize].as_ref();
        device.submit(|encoder| {
            record_copies(
                encoder,
                &self.arenas,
                src,
                &self.staging_buffer,
                &mut copies,
            );
        });
        self.retire_slices(retired);
        self.meta_dirty = true;
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        self.starved.remove(pos);
        if let Some(alloc) = self.chunks.remove(pos) {
            self.retire_slices(alloc.sections.iter().map(SectionAlloc::slice));
            self.meta_dirty = true;
        }
    }

    /// Drops every mesh and, for a new format, the arenas, remade at its
    /// stride on the next upload. The caller waits for in-flight frames first.
    pub fn set_format(&mut self, device: &rhi::backend::Device, format: TerrainFormat) {
        self.clear();
        if format == self.format {
            return;
        }
        self.format = format;
        for arena in self.arenas.drain(..).flatten() {
            arena.destroy(device);
        }
        self.camera_meta.clear();
        self.shadow_meta.clear();
    }

    pub fn format(&self) -> TerrainFormat {
        self.format
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        for arena in self.arenas.iter_mut().flatten() {
            arena.vertices.reset();
            arena.indices.reset();
        }
        self.pending_free.clear();
        self.starved.clear();
        self.frontier_sq = f64::INFINITY;
        self.draining = None;
        for meta in self.camera_meta.iter_mut().chain(&mut self.shadow_meta) {
            meta.clear();
        }
        self.meta_dirty = true;
        self.fade_enabled = false;
    }

    /// A column not drawing its newest mesh, or none, for lack of room.
    #[cfg(feature = "test-harness")]
    pub fn starved_column(&self) -> Option<ChunkPos> {
        self.starved.keys().next().copied()
    }

    /// Where an arena's ranges overlap, if they do: live sections, slices
    /// awaiting reclaim and free ranges must never share a vertex or index.
    #[cfg(feature = "test-harness")]
    pub fn slice_overlap(&self) -> Option<String> {
        let mut ranges: Vec<[Vec<(u32, u32)>; 2]> = self
            .arenas
            .iter()
            .map(|slot| match slot {
                Some(a) => [
                    a.vertices.free_blocks().collect(),
                    a.indices.free_blocks().collect(),
                ],
                None => Default::default(),
            })
            .collect();
        let live = self
            .chunks
            .values()
            .flat_map(|c| &c.sections)
            .map(SectionAlloc::slice);
        let retired = self.pending_free.iter().map(|&(_, slice)| slice);
        for slice in live.chain(retired) {
            let Some([vertex, index]) = ranges
                .get_mut(slice.arena as usize)
                .filter(|_| self.arenas[slice.arena as usize].is_some())
            else {
                return Some(format!("a slice in arena {}, which is gone", slice.arena));
            };
            vertex.push((slice.vertex_offset, slice.vertex_len));
            index.push((slice.first_index, slice.index_len));
        }
        ranges
            .into_iter()
            .enumerate()
            .flat_map(|(id, [vertex, index])| [(id, "vertex", vertex), (id, "index", index)])
            .find_map(|(id, pool, mut ranges)| {
                ranges.sort_unstable();
                ranges
                    .windows(2)
                    .find(|w| w[0].0 + w[0].1 > w[1].0)
                    .map(|w| {
                        format!(
                            "arena {id}'s {pool} ranges {:?} and {:?} overlap",
                            w[0], w[1]
                        )
                    })
            })
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Push the CPU visibility graph's per-column visible-section masks.
    /// Columns not present default to fully visible, so the cull only omits
    /// sections the graph proved occluded.
    pub fn set_chunk_visibility(&mut self, vis: HashMap<ChunkPos, u32>) {
        self.chunk_visibility = vis;
        self.meta_dirty = true;
    }

    /// `anchor` must be the same `Camera::anchor()` this frame's
    /// `CameraUniform` was built with, so the cull's block/fraction split
    /// matches the vertex shader's.
    pub fn dispatch_cull(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
    ) {
        let column = |eye| {
            let block = BlockPos::containing(eye);
            ChunkPos::new(block.x >> 4, block.z >> 4)
        };
        if !self.starved.is_empty() && self.eye.is_some_and(|last| column(last) != column(eye)) {
            self.retry_due = true;
        }
        self.eye = Some(eye);
        if self.chunks.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        // Re-sort only once the camera moves ~8 blocks; front-to-back order is an
        // early-Z optimization, so finer staleness is harmless.
        const SORT_RECAM_SQ: f64 = 64.0;

        // A fade in flight changes per-section visibility every frame, so the draw
        // list must rebuild; otherwise it only changes on edits/loads/visibility
        // (`meta_dirty`). The fade check is O(1) against `fade_until`.
        let any_fading = self.fade_enabled && now < self.fade_until;
        let content_changed = self.meta_dirty || any_fading;

        if content_changed {
            build_meta(
                &self.chunks,
                Some(&self.chunk_visibility),
                self.fade_enabled,
                eye,
                now,
                &mut self.camera_meta,
            );
            self.meta_dirty = false;
            self.shadow_dirty = true;
        }

        let cam_moved = (eye - self.last_sort_cam).length_squared() > SORT_RECAM_SQ;
        if content_changed || cam_moved {
            for meta in &mut self.camera_meta {
                sort_meta(meta, eye);
            }
            self.last_sort_cam = eye;
            // Draw lists reordered: every frame slot's meta buffer needs the refresh.
            self.camera.upload_pending = MAX_FRAMES_IN_FLIGHT as u32;
        }

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so its readback holds the previous cull's totals (solid + cutout draws).
        self.last_draw_count = self.count_readbacks[frame].read(|bytes| {
            let (counts, _) = bytes.as_chunks::<4>();
            counts.iter().map(|&count| u32::from_ne_bytes(count)).sum()
        });
        self.camera.cull(
            encoder,
            &self.compute_pipeline,
            frame,
            &self.camera_meta,
            frustum,
            anchor,
            eye,
            !self.indirect_draw.count,
        );
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::CopySrc);
        let readback = self.count_readbacks[frame].handle();
        for (i, count) in [
            &self.camera.count_buffers[frame],
            &self.camera.count_cutout_buffers[frame],
        ]
        .into_iter()
        .enumerate()
        {
            encoder.copy_buffer(
                count.handle(),
                readback,
                &[rhi::BufferCopy {
                    src_offset: 0,
                    dst_offset: i as u64 * COUNTS_SIZE,
                    size: COUNTS_SIZE,
                }],
            );
        }
        encoder.transition_buffers(rhi::BufferUse::CopyDst, rhi::BufferUse::HostRead);

        if !self.fade_enabled {
            self.fade_enabled = true;
        }
    }

    /// Culls every section, the camera's occluded ones too, against a shader
    /// pack's shadow view `frustum`, after this frame's
    /// [`Self::dispatch_cull`].
    pub fn dispatch_shadow_cull(
        &mut self,
        device: &rhi::backend::Device,
        encoder: &rhi::backend::CommandEncoder,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
    ) {
        if self.chunks.is_empty() {
            return;
        }
        if self.shadow_dirty {
            build_meta(
                &self.chunks,
                None,
                self.fade_enabled,
                eye,
                std::time::Instant::now(),
                &mut self.shadow_meta,
            );
            if let Some(shadow) = &mut self.shadow {
                shadow.upload_pending = MAX_FRAMES_IN_FLIGHT as u32;
            }
            self.shadow_dirty = false;
        }
        let shadow = self
            .shadow
            .get_or_insert_with(|| DrawList::new(device, self.cull_layout, self.max_meta));
        shadow.cull(
            encoder,
            &self.compute_pipeline,
            frame,
            &self.shadow_meta,
            frustum,
            anchor,
            eye,
            !self.indirect_draw.count,
        );
    }

    /// Issue one render layer's indirect draws for `view`, one call per
    /// arena over its range of the draw lists. `cutout` selects the discard
    /// pass's draw lists (drawn after `solid`, which lays down depth); the
    /// caller binds the matching pipeline first. Both layers share each
    /// arena's vertex/index buffers, the meta buffer and the cull-written
    /// draw lists.
    pub fn draw_indirect(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        cutout: bool,
        view: View,
    ) {
        if self.chunks.is_empty() {
            return;
        }
        let (metas, list) = match (view, &self.shadow) {
            (View::Camera, _) => (&self.camera_meta, &self.camera),
            (View::Shadow, Some(shadow)) => (&self.shadow_meta, shadow),
            (View::Shadow, None) => return,
        };
        let (indirect, count) = if cutout {
            (
                list.indirect_cutout_buffers[frame].handle(),
                list.count_cutout_buffers[frame].handle(),
            )
        } else {
            (
                list.indirect_buffers[frame].handle(),
                list.count_buffers[frame].handle(),
            )
        };
        // Binding 1: the meta buffer, read per instance for the section
        // origin + fade (indexed by `first_instance`).
        pass.set_vertex_buffer(1, list.meta_buffers[frame].handle());
        let stride = size_of::<DrawCommand>() as u32;
        for (id, (slot, meta)) in self.arenas.iter().zip(metas).enumerate() {
            let Some(arena) = live(slot).filter(|_| !meta.is_empty()) else {
                continue;
            };
            // Binding 0: the arena's packed vertices.
            pass.set_vertex_buffer(0, arena.vertex_buffer.handle());
            pass.set_index_buffer(arena.index_buffer.handle());
            let max_draws = meta.len() as u32;
            let offset = u64::from(meta[0].draw_base * stride);
            if self.indirect_draw.count {
                let count_offset = (id * size_of::<u32>()) as u64;
                pass.draw_indexed_indirect_count(
                    indirect,
                    offset,
                    count,
                    count_offset,
                    max_draws,
                    stride,
                );
            } else if self.indirect_draw.multi_draw {
                pass.draw_indexed_indirect(indirect, offset, max_draws, stride);
            } else {
                for i in 0..max_draws {
                    pass.draw_indexed_indirect(indirect, offset + u64::from(i * stride), 1, stride);
                }
            }
        }
    }

    /// Per-section fade-in factor in `[0, 1]`: near columns appear instantly,
    /// the rest ramp over [`FADE_DURATION_MS`] from their upload time. Drives
    /// both the opaque indirect meta and the water pass so they fade in
    /// together.
    fn section_visibility(nearby: bool, sec: &SectionAlloc, now: std::time::Instant) -> f32 {
        if nearby {
            return 1.0;
        }
        let elapsed_ms = now.duration_since(sec.uploaded_at).as_secs_f32() * 1000.0;
        (elapsed_ms / FADE_DURATION_MS).min(1.0)
    }

    /// Draw the translucent water of every section that survives a CPU frustum
    /// cull. Reuses the arenas' vertex/index buffers (water indices live right
    /// after the opaque ones in each section's slice); the caller binds the
    /// blended water pipeline first. Not GPU-culled — water sections are a
    /// small subset, so a per-section draw is cheap and keeps the opaque
    /// indirect path untouched.
    ///
    /// `anchor` must be the same `Camera::anchor()` this frame's
    /// `CameraUniform` was built with: the push-constant origins are rebased
    /// against it and the shader adds back `camera_pos` (the eye's offset
    /// from that anchor).
    ///
    /// TODO: water isn't depth-sorted, so overlapping translucent surfaces
    /// (oceans at grazing angles, water seen through water) can blend out of
    /// order.
    pub fn draw_water(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
    ) {
        if self.chunks.is_empty() {
            return;
        }

        let mut bound = None;
        let now = std::time::Instant::now();
        for (pos, alloc) in self.chunks.iter() {
            let col_vis = self.chunk_visibility.get(pos).copied().unwrap_or(u32::MAX);
            let nearby = column_nearby(self.fade_enabled, *pos, eye);
            for sec in &alloc.sections {
                if sec.water_index_count == 0
                    || col_vis & (1u32 << sec.section_index) == 0
                    || !aabb_in_frustum(&sec.aabb, sec.origin, frustum, eye)
                {
                    continue;
                }
                if bound != Some(sec.arena) {
                    let Some(arena) = self.arenas[sec.arena as usize].as_ref() else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, arena.vertex_buffer.handle());
                    pass.set_index_buffer(arena.index_buffer.handle());
                    bound = Some(sec.arena);
                }
                let vis = Self::section_visibility(nearby, sec, now);
                let rel = (origin_dvec(sec.origin) - anchor).as_vec3();
                let origin_fade = [rel.x, rel.y, rel.z, vis];
                pass.set_push_constants(
                    rhi::ShaderStages::VERTEX,
                    0,
                    bytemuck::bytes_of(&origin_fade),
                );
                pass.draw_indexed(
                    sec.water_index_count,
                    1,
                    sec.water_first_index,
                    sec.vertex_offset,
                    0,
                );
            }
        }
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        for arena in self.arenas.drain(..).flatten() {
            arena.destroy(device);
        }
        std::mem::take(&mut self.staging_buffer).destroy(device);
        for list in self.draw_lists_mut() {
            list.destroy(device);
        }
        util::destroy_buffers(device, &mut self.count_readbacks);

        self.compute_pipeline.destroy(device);
        self.compute_layout.destroy(device);
        self.cull_layout.destroy(device);
    }
}

/// One arena's vertex and index heaps in bytes, at `format`'s vertex size.
fn arena_bytes(format: TerrainFormat) -> u64 {
    u64::from(ARENA_VERTICES) * format.vertex_size() + u64::from(ARENA_INDICES) * INDEX_SIZE
}

/// Of the `loaded` columns, as (distance from the eye squared, position,
/// what they take), those to evict for `need`: the farthest first, all
/// farther than `limit`, until they free it. Nearest last.
fn eviction_victims(
    loaded: impl Iterator<Item = (f64, ChunkPos, Need)>,
    limit: f64,
    need: Need,
) -> Vec<(f64, ChunkPos)> {
    let mut far: Vec<_> = loaded.filter(|&(dist, ..)| dist > limit).collect();
    far.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    let mut freed = Need::default();
    let mut victims = Vec::new();
    for (dist, pos, taken) in far {
        if need.fits_in(freed.vertices, freed.indices) {
            break;
        }
        freed.vertices += taken.vertices;
        freed.indices += taken.indices;
        victims.push((dist, pos));
    }
    victims
}

/// A starved column, for [`columns_to_retry`].
struct Waiting {
    /// From the eye, squared.
    dist: f64,
    pos: ChunkPos,
    need: Need,
    /// Its largest section fits some arena.
    fits_an_arena: bool,
}

/// The starved columns to hand back, nearest first, and the eviction
/// frontier after. Inside the frontier a column comes back when it fits the
/// free vertices and indices, or when a loaded column farther out (the
/// farthest is at `farthest_loaded`) can be evicted for it. Past it, only
/// while [`FRONTIER_SLACK`] of an arena stays free after, moving the frontier
/// out past it, so the frontier doesn't swing back and forth as columns come
/// and go.
fn columns_to_retry(
    waiting: impl Iterator<Item = Waiting>,
    (mut free_vertices, mut free_indices): (u64, u64),
    mut frontier_sq: f64,
    farthest_loaded: f64,
) -> (Vec<ChunkPos>, f64) {
    let mut waiting: Vec<Waiting> = waiting.collect();
    waiting.sort_unstable_by(|a, b| a.dist.total_cmp(&b.dist));
    let mut retry = Vec::new();
    for column in waiting {
        let past = column.dist >= frontier_sq;
        let (keep_vertices, keep_indices) = if past {
            (
                u64::from(ARENA_VERTICES) / FRONTIER_SLACK,
                u64::from(ARENA_INDICES) / FRONTIER_SLACK,
            )
        } else {
            (0, 0)
        };
        let fits = column.fits_an_arena
            && column.need.fits_in(
                free_vertices.saturating_sub(keep_vertices),
                free_indices.saturating_sub(keep_indices),
            );
        if fits {
            free_vertices -= column.need.vertices;
            free_indices -= column.need.indices;
            if past {
                frontier_sq = column.dist.next_up();
            }
            retry.push(column.pos);
        } else if past {
            break;
        } else if column.dist < farthest_loaded {
            retry.push(column.pos);
        }
    }
    (retry, frontier_sq)
}

/// The arena compaction should drain, of the `live` ones as (id, vertices
/// used, indices used): the emptiest, once the others could hold everything
/// filled to [`COMPACT_FILL`].
fn arena_to_drain(live: impl Iterator<Item = (u16, u32, u32)>) -> Option<u16> {
    let live: Vec<_> = live.collect();
    if live.len() < 2 {
        return None;
    }
    let used_vertices: u64 = live.iter().map(|&(_, v, _)| u64::from(v)).sum();
    let used_indices: u64 = live.iter().map(|&(.., i)| u64::from(i)).sum();
    let room = (live.len() - 1) as f64 * COMPACT_FILL;
    if used_vertices as f64 > room * f64::from(ARENA_VERTICES)
        || used_indices as f64 > room * f64::from(ARENA_INDICES)
    {
        return None;
    }
    live.iter().min_by_key(|&&(_, v, _)| v).map(|&(id, ..)| id)
}

/// An arena heap: device-local behind the staging buffer, or host-visible.
/// Either way a copy source and target, for compaction.
fn create_pool(
    device: &rhi::backend::Device,
    use_staging: bool,
    label: &str,
    size: u64,
    usage: rhi::BufferUsages,
) -> rhi::backend::Buffer {
    let usage = usage | rhi::BufferUsages::COPY_SRC | rhi::BufferUsages::COPY_DST;
    if use_staging {
        rhi::backend::Buffer::dedicated(device, label, size, usage, rhi::MemoryLocation::Device)
    } else {
        rhi::backend::Buffer::new(device, label, size, usage, rhi::MemoryLocation::Host)
    }
}

/// Which view's draw lists [`ChunkBufferStore::draw_indirect`] draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Camera,
    /// A shader pack's shadow pass.
    Shadow,
}

/// One view's sections over every arena and, per frame slot, the cull's
/// frustum and the draw lists it writes, a range and a draw count per arena.
struct DrawList {
    /// Frame slots still needing the latest meta uploaded. Set to
    /// `MAX_FRAMES_IN_FLIGHT` whenever the draw list changes, decremented per
    /// frame; at steady state the per-frame meta copy stops.
    upload_pending: u32,
    cull_sets: Vec<rhi::backend::BindGroup>,
    meta_buffers: Vec<rhi::backend::Buffer>,
    frustum_buffers: Vec<rhi::backend::Buffer>,
    // Solid (no-discard, early-Z) draw list, written by the cull shader.
    indirect_buffers: Vec<rhi::backend::Buffer>,
    count_buffers: Vec<rhi::backend::Buffer>,
    // Cutout (discard) draw list. Same sections, the back of each section's
    // index slice; drawn in a second pass after solid lays down depth.
    indirect_cutout_buffers: Vec<rhi::backend::Buffer>,
    count_cutout_buffers: Vec<rhi::backend::Buffer>,
}

impl DrawList {
    fn new(
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        draws: usize,
    ) -> Self {
        let count = |name: &str| {
            util::create_per_frame_host_buffers(
                device,
                COUNTS_SIZE,
                rhi::BufferUsages::STORAGE
                    | rhi::BufferUsages::INDIRECT
                    | rhi::BufferUsages::COPY_SRC,
                name,
            )
        };
        let mut this = Self {
            upload_pending: 0,
            cull_sets: vec![rhi::backend::BindGroup::default(); MAX_FRAMES_IN_FLIGHT],
            meta_buffers: Vec::new(),
            frustum_buffers: util::create_per_frame_host_buffers(
                device,
                FRUSTUM_SIZE,
                rhi::BufferUsages::UNIFORM,
                "frustum_ubo",
            ),
            indirect_buffers: Vec::new(),
            count_buffers: count("draw_count"),
            indirect_cutout_buffers: Vec::new(),
            count_cutout_buffers: count("draw_count_cutout"),
        };
        this.regrow(device, layout, draws);
        this
    }

    /// Remakes the per-frame meta and indirect buffers to hold `draws` and
    /// points each frame's cull group at its buffers.
    fn regrow(
        &mut self,
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        draws: usize,
    ) {
        let (meta_size, indirect_size) = list_sizes(draws);
        for buffers in [
            &mut self.meta_buffers,
            &mut self.indirect_buffers,
            &mut self.indirect_cutout_buffers,
        ] {
            util::destroy_buffers(device, buffers);
        }
        let indirect = |name: &str| {
            util::create_per_frame_host_buffers(
                device,
                indirect_size,
                rhi::BufferUsages::STORAGE | rhi::BufferUsages::INDIRECT,
                name,
            )
        };
        self.meta_buffers = util::create_per_frame_host_buffers(
            device,
            meta_size,
            rhi::BufferUsages::STORAGE | rhi::BufferUsages::VERTEX,
            "chunk_meta",
        );
        self.indirect_buffers = indirect("indirect_cmds");
        self.indirect_cutout_buffers = indirect("indirect_cmds_cutout");
        self.upload_pending = MAX_FRAMES_IN_FLIGHT as u32;

        let storage = rhi::backend::BindingResource::storage;
        let uniform = rhi::backend::BindingResource::uniform;
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let bindings = [
                (0, storage(&self.meta_buffers[i], meta_size)),
                (1, uniform(&self.frustum_buffers[i], FRUSTUM_SIZE)),
                (2, storage(&self.indirect_buffers[i], indirect_size)),
                (3, storage(&self.count_buffers[i], COUNTS_SIZE)),
                (4, storage(&self.indirect_cutout_buffers[i], indirect_size)),
                (5, storage(&self.count_cutout_buffers[i], COUNTS_SIZE)),
            ];
            self.cull_sets[i].replace(device, layout, &bindings);
        }
    }

    /// Records the cull of `metas`, this list's sections per arena, against
    /// `frustum` into this frame slot's draw lists, uploading `metas` back to
    /// back first if the slot hasn't seen them.
    /// `zero_indirect`: without drawIndirectCount the whole indirect buffer
    /// is drawn, so slots the cull leaves unfilled must read as no-op draws.
    #[allow(clippy::too_many_arguments)]
    fn cull(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        pipeline: &rhi::backend::ComputePipeline,
        frame: usize,
        metas: &[Vec<ChunkMeta>],
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        zero_indirect: bool,
    ) {
        if self.upload_pending > 0 {
            let mut offset = 0;
            for meta in metas {
                let bytes: &[u8] = bytemuck::cast_slice(meta);
                self.meta_buffers[frame].write_at(offset, bytes);
                offset += bytes.len();
            }
            self.upload_pending -= 1;
        }
        let count = metas.iter().map(Vec::len).sum::<usize>() as u32;
        let frustum_data = FrustumData {
            planes: *frustum,
            chunk_count: count,
            cam_block: anchor.as_ivec3().to_array(),
            frac: (eye - anchor).as_vec3().to_array(),
            _pad: 0.0,
        };
        self.frustum_buffers[frame].write(bytemuck::bytes_of(&frustum_data));
        for buffer in [
            &mut self.count_buffers[frame],
            &mut self.count_cutout_buffers[frame],
        ] {
            buffer.zero();
        }
        if zero_indirect {
            for buffer in [
                &mut self.indirect_buffers[frame],
                &mut self.indirect_cutout_buffers[frame],
            ] {
                buffer.zero();
            }
        }
        encoder.dispatch(pipeline, self.cull_sets[frame], [count.div_ceil(64), 1, 1]);
        encoder.transition_buffers(rhi::BufferUse::StorageWrite, rhi::BufferUse::Indirect);
    }

    fn destroy(&mut self, device: &rhi::backend::Device) {
        for buffers in [
            &mut self.meta_buffers,
            &mut self.frustum_buffers,
            &mut self.indirect_buffers,
            &mut self.count_buffers,
            &mut self.indirect_cutout_buffers,
            &mut self.count_cutout_buffers,
        ] {
            util::destroy_buffers(device, buffers);
        }
        util::destroy_bind_groups(device, self.cull_sets.drain(..));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn build_meta_masks_fades_and_sorts_front_to_back() {
        let now = Instant::now();
        let near = ChunkPos::new(0, 0);
        let far = ChunkPos::new(4, 0);
        let mut chunks = HashMap::new();
        chunks.insert(near, ChunkAlloc::synthetic(near, -64, [4, 5], now));
        chunks.insert(
            far,
            ChunkAlloc::synthetic(far, -64, [4], now - Duration::from_millis(500)),
        );
        // Section 5 of the near column is occluded.
        let vis = HashMap::from([(near, 1u32 << 4)]);
        let eye = DVec3::new(8.0, 8.0, 8.0);

        let mut metas = [Vec::new()];
        build_meta(&chunks, Some(&vis), true, eye, now, &mut metas);
        let meta = &mut metas[0];
        sort_meta(meta, eye);

        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0].origin, [0, 0, 0]);
        assert_eq!(f32::from_bits(meta[0].visibility), 1.0);
        assert_eq!(meta[1].origin, [64, 0, 0]);
        let far_vis = f32::from_bits(meta[1].visibility);
        assert!((far_vis - 0.5).abs() < 0.01, "far fade {far_vis}");

        build_meta(&chunks, Some(&vis), false, eye, now, &mut metas);
        assert!(metas[0].iter().all(|m| f32::from_bits(m.visibility) == 1.0));

        build_meta(&chunks, None, false, eye, now, &mut metas);
        assert_eq!(metas[0].len(), 3);
    }

    #[test]
    fn build_meta_lays_arenas_out_back_to_back() {
        let now = Instant::now();
        let pos = ChunkPos::new(0, 0);
        let mut alloc = ChunkAlloc::synthetic(pos, -64, [0, 1, 2], now);
        alloc.sections[2].arena = 2;
        let chunks = HashMap::from([(pos, alloc)]);

        let mut metas = [Vec::new(), Vec::new(), Vec::new()];
        build_meta(&chunks, None, false, DVec3::ZERO, now, &mut metas);

        let layout: Vec<Vec<(u32, u32)>> = metas
            .iter()
            .map(|list| list.iter().map(|m| (m.arena, m.draw_base)).collect())
            .collect();
        assert_eq!(layout, [vec![(0, 0), (0, 0)], vec![], vec![(2, 2)]]);
    }

    fn need(vertices: u64) -> Need {
        let indices = vertices / 2 * 3;
        Need {
            vertices,
            indices,
            max_vertices: vertices as u32,
            max_indices: indices as u32,
        }
    }

    #[test]
    fn eviction_takes_the_farthest_until_the_need_is_met() {
        let loaded = [
            (1.0, ChunkPos::new(1, 0), need(100)),
            (9.0, ChunkPos::new(3, 0), need(100)),
            (4.0, ChunkPos::new(2, 0), need(100)),
            (16.0, ChunkPos::new(4, 0), need(100)),
        ];
        let victims = eviction_victims(loaded.into_iter(), 2.0, need(150));
        assert_eq!(
            victims,
            [(16.0, ChunkPos::new(4, 0)), (9.0, ChunkPos::new(3, 0))]
        );
        // Never one nearer than the column it's for.
        let victims = eviction_victims(loaded.into_iter(), 8.0, need(1000));
        assert_eq!(
            victims,
            [(16.0, ChunkPos::new(4, 0)), (9.0, ChunkPos::new(3, 0))]
        );
        assert!(eviction_victims(loaded.into_iter(), 0.0, Need::default()).is_empty());
    }

    fn waiting(dist: f64, x: i32, vertices: u64) -> Waiting {
        Waiting {
            dist,
            pos: ChunkPos::new(x, 0),
            need: need(vertices),
            fits_an_arena: true,
        }
    }

    #[test]
    fn starved_columns_inside_the_frontier_come_back_for_eviction() {
        let free = (100, 150);
        // The near one fits; the next doesn't, but a farther loaded column
        // can be evicted for it.
        let (retry, frontier) = columns_to_retry(
            [waiting(1.0, 1, 50), waiting(4.0, 2, 500)].into_iter(),
            free,
            100.0,
            9.0,
        );
        assert_eq!(retry, [ChunkPos::new(1, 0), ChunkPos::new(2, 0)]);
        assert_eq!(frontier, 100.0);
        // Nothing loaded farther out, so it waits.
        let (retry, _) = columns_to_retry([waiting(4.0, 2, 500)].into_iter(), free, 100.0, 1.0);
        assert!(retry.is_empty());
    }

    #[test]
    fn starved_columns_past_the_frontier_need_slack() {
        let slack = u64::from(ARENA_VERTICES) / FRONTIER_SLACK;
        let free = (slack + 100, u64::MAX);
        let (retry, frontier) = columns_to_retry(
            [
                waiting(25.0, 5, 100),
                waiting(36.0, 6, 100),
                waiting(49.0, 7, 1),
            ]
            .into_iter(),
            free,
            16.0,
            f64::INFINITY,
        );
        // The first fits with the slack left and moves the frontier past it;
        // the second doesn't, and nothing farther comes back.
        assert_eq!(retry, [ChunkPos::new(5, 0)]);
        assert_eq!(frontier, 25.0f64.next_up());
    }

    #[test]
    fn compaction_drains_the_emptiest_once_the_rest_hold_it() {
        let quarter = ARENA_VERTICES / 4;
        assert_eq!(arena_to_drain([(0, quarter, 0)].into_iter()), None);
        assert_eq!(
            arena_to_drain([(0, quarter * 2, 0), (3, quarter / 2, 0)].into_iter()),
            Some(3)
        );
        assert_eq!(
            arena_to_drain([(0, quarter * 3, 0), (3, quarter, 0)].into_iter()),
            None
        );
    }
}
