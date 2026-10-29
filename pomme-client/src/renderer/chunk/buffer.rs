use std::collections::{HashMap, VecDeque};
use std::mem::offset_of;

use glam::DVec3;

use super::mesher::{ChunkAABB, ChunkMeshData, PackedVertex, SectionMesh};
use crate::position::ChunkPos;
use crate::renderer::rhi::IndirectDrawFeatures;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

const BUCKET_VERTICES: u32 = 32768;
const BUCKET_INDICES: u32 = 49152;
const VERTEX_SIZE: u64 = size_of::<PackedVertex>() as u64;
const INDEX_SIZE: u64 = size_of::<u32>() as u64;
const BYTES_PER_BUCKET: u64 =
    BUCKET_VERTICES as u64 * VERTEX_SIZE + BUCKET_INDICES as u64 * INDEX_SIZE;
const MIN_BUCKETS: u32 = 128;
const MAX_BUCKETS: u32 = 2048;
const VRAM_BUDGET_FRACTION: f64 = 0.25;

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
const COUNT_SIZE: u64 = 4;
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
    let dx = pos.x as f64 * 16.0 + 8.0 - eye.x;
    let dz = pos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dz * dz < NEARBY_DIST_SQ as f64
}

/// First-fit free-list sub-allocator over a fixed element range, coalescing on
/// free. Each section gets an exact-size vertex (and index) slice instead of
/// whole fixed buckets — vanilla's `UberGpuBuffer` model — so re-uploading one
/// section never disturbs the rest and there is no per-section bucket waste.
struct FreeList {
    capacity: u32,
    /// Free regions `(offset, len)`, sorted by offset and coalesced (no two
    /// adjacent).
    free: Vec<(u32, u32)>,
}

impl FreeList {
    fn new(capacity: u32) -> Self {
        Self {
            capacity,
            free: vec![(0, capacity)],
        }
    }

    fn reset(&mut self) {
        self.free.clear();
        self.free.push((0, self.capacity));
    }

    /// Allocate `n` contiguous elements; `None` if no region is large enough.
    fn alloc(&mut self, n: u32) -> Option<u32> {
        for i in 0..self.free.len() {
            let (off, len) = self.free[i];
            if len >= n {
                if len == n {
                    self.free.remove(i);
                } else {
                    self.free[i] = (off + n, len - n);
                }
                return Some(off);
            }
        }
        None
    }

    /// Return a region, coalescing with an adjacent free region on either side.
    fn free_region(&mut self, off: u32, n: u32) {
        let pos = self.free.partition_point(|&(o, _)| o < off);
        self.free.insert(pos, (off, n));
        if pos + 1 < self.free.len() {
            let (o, l) = self.free[pos];
            let (no, nl) = self.free[pos + 1];
            if o + l == no {
                self.free[pos] = (o, l + nl);
                self.free.remove(pos + 1);
            }
        }
        if pos > 0 {
            let (po, pl) = self.free[pos - 1];
            let (o, l) = self.free[pos];
            if po + pl == o {
                self.free[pos - 1] = (po, pl + l);
                self.free.remove(pos);
            }
        }
    }
}

fn compute_bucket_count(device_local_bytes: u64) -> u32 {
    let budget = (device_local_bytes as f64 * VRAM_BUDGET_FRACTION) as u64;
    let buckets = (budget / BYTES_PER_BUCKET) as u32;
    let count = buckets.clamp(MIN_BUCKETS, MAX_BUCKETS);
    tracing::info!(
        "GPU VRAM: {} MB, chunk budget: {} MB, buckets: {}",
        device_local_bytes / (1024 * 1024),
        (count as u64 * BYTES_PER_BUCKET) / (1024 * 1024),
        count
    );
    count
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ChunkMeta {
    /// Section-local vertex bounds; the cull shader rebases them via `origin`.
    aabb_min: [f32; 4],
    aabb_max: [f32; 4],
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

/// Vertex input for the chunk pipeline: buffer 0 is the packed per-vertex
/// pool, buffer 1 is the meta buffer read per-instance (origin + fade),
/// indexed by the `first_instance` the cull shader writes.
pub const CHUNK_VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 2] = {
    const POS: u32 = offset_of!(PackedVertex, pos) as u32;
    [
        rhi::VertexBufferLayout {
            stride: size_of::<PackedVertex>() as u32,
            step_mode: rhi::VertexStepMode::Vertex,
            attributes: &[
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
            ],
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
};

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

/// Rebuilds the draw list into `out`: one [`ChunkMeta`] per uploaded section
/// the occlusion mask keeps (all of them without one), with its fade-in
/// factor at `now`.
pub(crate) fn build_meta(
    chunks: &HashMap<ChunkPos, ChunkAlloc>,
    chunk_visibility: Option<&HashMap<ChunkPos, u32>>,
    fade_enabled: bool,
    eye: DVec3,
    now: std::time::Instant,
    out: &mut Vec<ChunkMeta>,
) {
    out.clear();
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
            out.push(ChunkMeta {
                aabb_min: sec.aabb.min,
                aabb_max: sec.aabb.max,
                index_count: sec.index_count,
                first_index: sec.first_index,
                vertex_offset: sec.vertex_offset,
                visibility: vis.to_bits(),
                origin: sec.origin,
                solid_index_count: sec.solid_index_count,
            });
        }
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
/// `vertex_offset` its vertex slice base; `vtx_len` is the slice length, kept
/// so the slices can be returned to the free-lists on removal. `uploaded_at`
/// drives the per-section fade so editing one section never re-fades the rest
/// of the column.
pub(crate) struct SectionAlloc {
    section_index: i32,
    aabb: ChunkAABB,
    /// Section world origin (`chunk*16`, `min_y + si*16`), used to rebase the
    /// quantized vertices and passed to the GPU via `ChunkMeta.origin`.
    origin: [i32; 3],
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

pub(crate) struct ChunkAlloc {
    sections: Vec<SectionAlloc>,
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

pub struct ChunkBufferStore {
    indirect_draw: IndirectDrawFeatures,
    /// Capacity (in draws) of the per-frame meta/indirect buffers. Grown on
    /// demand because per-section packing yields many more draws than buckets.
    max_meta: usize,
    vertex_buffer: rhi::backend::Buffer,
    index_buffer: rhi::backend::Buffer,
    staging_buffer: rhi::backend::Buffer,
    staging_size: u64,
    use_staging: bool,

    /// Exact-size sub-allocators over the vertex and index pools (in elements).
    vtx_free: FreeList,
    idx_free: FreeList,
    chunks: HashMap<ChunkPos, ChunkAlloc>,
    /// Per-column bitmask of occlusion-visible section indices (bit `si`), from
    /// the CPU visibility graph. A column absent here defaults to fully
    /// visible, so freshly-loaded-but-not-yet-graphed columns still draw.
    chunk_visibility: HashMap<ChunkPos, u32>,
    meta_dirty: bool,
    /// End of the current fade-in window. While `now < fade_until` the
    /// per-section fade values change each frame, so `cached_meta` must be
    /// rebuilt; an O(1) check replacing the old all-sections scan.
    fade_until: std::time::Instant,
    /// Eye position at the last front-to-back sort; the sort (an early-Z
    /// optimization) is only redone once the camera moves past a threshold.
    last_sort_cam: DVec3,

    compute_pipeline: rhi::backend::ComputePipeline,
    compute_layout: rhi::backend::PipelineLayout,
    cull_layout: rhi::backend::BindGroupLayout,

    camera: DrawList,
    /// A shader pack's shadow view, made on its first cull.
    shadow: Option<DrawList>,
    /// The shadow list needs rebuilding from the sections.
    shadow_dirty: bool,
    /// The camera's draw counts, copied after each cull for the debug overlay.
    count_readbacks: Vec<rhi::backend::Buffer>,
    fade_enabled: bool,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,

    /// Monotonic frame counter, bumped once per rendered frame in
    /// `begin_frame`.
    frame_seq: u64,
    /// Slices freed by a re-mesh or unload, each tagged with the `frame_seq` at
    /// which it's safe to reclaim (`MAX_FRAMES_IN_FLIGHT` out, so no in-flight
    /// frame still draws it). Drained in `begin_frame`.
    pending_free: VecDeque<(u64, (u32, u32, u32, u32))>,
}

impl ChunkBufferStore {
    pub fn new(device: &rhi::backend::Device, indirect_draw: IndirectDrawFeatures) -> Self {
        let total_buckets = compute_bucket_count(device.device_local_bytes);
        let vertex_size = total_buckets as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;
        let index_size = total_buckets as u64 * BUCKET_INDICES as u64 * INDEX_SIZE;

        let use_staging = device.discrete;

        let create_pool = |label: &str, size: u64, usage: rhi::BufferUsages| {
            if use_staging {
                rhi::backend::Buffer::new(
                    device,
                    label,
                    size,
                    usage | rhi::BufferUsages::COPY_DST,
                    rhi::MemoryLocation::Device,
                )
            } else {
                rhi::backend::Buffer::new(device, label, size, usage, rhi::MemoryLocation::Host)
            }
        };
        let vertex_buffer = create_pool("vertex_pool", vertex_size, rhi::BufferUsages::VERTEX);
        let index_buffer = create_pool("index_pool", index_size, rhi::BufferUsages::INDEX);

        // Discrete GPUs batch a frame's uploads through this buffer in one
        // transfer, so size it to hold several columns and keep sub-flushes rare.
        // The integrated path writes mapped memory directly and never touches it.
        let staging_size = if use_staging {
            BYTES_PER_BUCKET * 16
        } else {
            BYTES_PER_BUCKET * 4
        };
        let staging_buffer = rhi::backend::Buffer::new(
            device,
            "staging",
            staging_size,
            rhi::BufferUsages::COPY_SRC,
            rhi::MemoryLocation::Host,
        );

        tracing::info!(
            "Chunk buffers: {} (vertex={} MB, index={} MB, staging={} KB)",
            if use_staging {
                "DEVICE_LOCAL + staging"
            } else {
                "HOST_VISIBLE"
            },
            vertex_size / (1024 * 1024),
            index_size / (1024 * 1024),
            staging_size / 1024,
        );

        let vtx_free = FreeList::new(total_buckets * BUCKET_VERTICES);
        let idx_free = FreeList::new(total_buckets * BUCKET_INDICES);

        // Per-section packing yields many more draws than buckets, so pre-size
        // generously: growth (`ensure_meta_capacity`) needs a `device.wait_idle`
        // to safely rewrite the bind groups, and we don't want that stall
        // firing mid-stream. The remaining grow path stays as a rare safety net.
        let max_meta = (total_buckets * 16).max(8192) as usize;
        let meta_size = (max_meta * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (max_meta * size_of::<DrawCommand>()) as u64;

        let count_readbacks = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                rhi::backend::Buffer::new(
                    device,
                    "draw_count_readback",
                    2 * COUNT_SIZE,
                    rhi::BufferUsages::COPY_DST,
                    rhi::MemoryLocation::Readback,
                )
            })
            .collect();

        let cull_layout = rhi::backend::create_bind_group_layout(device, CULL_BINDINGS);
        let camera = DrawList::new(device, cull_layout, meta_size, indirect_size);
        let compute_layout = rhi::backend::create_pipeline_layout(device, &[cull_layout], None);

        let compute_pipeline = rhi::backend::create_compute_pipeline(
            device,
            compute_layout,
            "cull",
            shader::include_shader!("cull.comp"),
        );

        Self {
            indirect_draw,
            max_meta,
            vertex_buffer,
            index_buffer,
            staging_buffer,
            staging_size,
            use_staging,
            vtx_free,
            idx_free,
            chunks: HashMap::new(),
            chunk_visibility: HashMap::new(),
            meta_dirty: true,
            fade_until: std::time::Instant::now(),
            last_sort_cam: DVec3::MAX,
            compute_pipeline,
            compute_layout,
            cull_layout,
            camera,
            shadow: None,
            shadow_dirty: true,
            count_readbacks,
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
        &mut self,
        device: &rhi::backend::Device,
        copy_v: &[rhi::BufferCopy],
        copy_i: &[rhi::BufferCopy],
    ) {
        if copy_v.is_empty() && copy_i.is_empty() {
            return;
        }
        device.submit(|encoder| {
            for (dst, copies) in [(&self.vertex_buffer, copy_v), (&self.index_buffer, copy_i)] {
                if !copies.is_empty() {
                    encoder.copy_buffer(self.staging_buffer.handle(), dst.handle(), copies);
                }
            }
        });
    }

    /// Upload a batch of mesh results, each replacing the sections in its
    /// `mesh.replaced` range. Staging copies for the whole batch are coalesced
    /// into as few transfers as the staging buffer holds (one per overflow,
    /// plus a final flush), each synchronized by a single fence wait — so a
    /// streaming frame stalls once, not once per mesh. Returns, per mesh
    /// that hit pool exhaustion, the section indices that were dropped and
    /// need re-meshing.
    pub fn upload_batch(
        &mut self,
        device: &rhi::backend::Device,
        meshes: &[ChunkMeshData],
    ) -> Vec<(ChunkPos, Vec<i32>)> {
        let mut needs_remesh: Vec<(ChunkPos, Vec<i32>)> = Vec::new();
        if meshes.is_empty() {
            return needs_remesh;
        }

        // Sub-allocate an exact-size vertex + index slice for each non-empty
        // section. Indices stay section-local and `vertex_offset` rebases the draw,
        // so no packing or rebasing is needed — just one slice per section.
        struct Plan<'a> {
            section_index: i32,
            verts: &'a [PackedVertex],
            indices: &'a [u32],
            water_indices: &'a [u32],
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
        }

        // Retired slices only reclaim in `begin_frame`; if rendering is paused
        // while meshing continues (e.g. minimized window) the backlog grows
        // unbounded. Past a sane bound, force a GPU wait and reclaim it all.
        const PENDING_FREE_DRAIN_THRESHOLD: usize = 8192;
        if self.pending_free.len() > PENDING_FREE_DRAIN_THRESHOLD {
            device.wait_idle();
            while let Some((_, slice)) = self.pending_free.pop_front() {
                self.free_slice(slice);
            }
        }

        let staging_half = self.staging_size as usize / 2;
        // Copies accumulated for the current (not-yet-submitted) transfer, and the
        // running write cursors into each half of the staging buffer.
        let mut copy_v: Vec<rhi::BufferCopy> = Vec::new();
        let mut copy_i: Vec<rhi::BufferCopy> = Vec::new();
        let mut stg_v = 0usize;
        let mut stg_i = 0usize;

        for mesh in meshes {
            // The covered sections this job is authoritative for: reject any where
            // a newer upload (higher epoch) already landed. See
            // `ChunkMeshData::upload_epoch`.
            let accepted: std::collections::HashSet<i32> = mesh
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

            // Retire the slices of every accepted covered section: the re-meshed
            // ones are re-allocated below, the now-empty ones simply vanish.
            // Remember which were present so a re-meshed section swaps instantly
            // while a freshly revealed one still fades in. Rejected sections are
            // left untouched.
            let mut freed: Vec<(u32, u32, u32, u32)> = Vec::new();
            let mut was_present: std::collections::HashSet<i32> = std::collections::HashSet::new();
            if let Some(entry) = self.chunks.get_mut(&mesh.pos) {
                entry.sections.retain(|s| {
                    if accepted.contains(&s.section_index) {
                        was_present.insert(s.section_index);
                        freed.push((s.vertex_offset as u32, s.vtx_len, s.first_index, s.idx_len));
                        false
                    } else {
                        true
                    }
                });
            }
            self.retire_slices(freed.iter().copied());
            // Sections were removed/replaced, so the draw list must be rebuilt even
            // if this mesh is skipped below (otherwise it keeps drawing a retired,
            // soon-reused slice).
            self.meta_dirty = true;

            let upload_secs: Vec<&SectionMesh> = mesh
                .sections
                .iter()
                .filter(|s| accepted.contains(&s.section_index))
                .collect();

            if upload_secs.is_empty() {
                // Every accepted section is now empty (freed above); drop the
                // column if nothing remains.
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
                // Verts and indices share the staging buffer (two halves). A chunk
                // too large for one half is skipped rather than overflowing the
                // buffer. This is permanent, so it's not reported for retry.
                let v_bytes: usize = upload_secs
                    .iter()
                    .map(|s| s.vertices.len() * VERTEX_SIZE as usize)
                    .sum();
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
                    continue;
                }
            }

            let mut plans: Vec<Plan> = Vec::with_capacity(upload_secs.len());
            // (vtx_off, vtx_len, idx_off, idx_len) taken for this mesh, for
            // rollback if the pool runs out partway through a column.
            let mut taken: Vec<(u32, u32, u32, u32)> = Vec::new();
            let mut pool_full = false;
            for sec in &upload_secs {
                let vcount = sec.vertices.len() as u32;
                // Opaque and water indices share one slice (opaque first, water after).
                let icount = (sec.indices.len() + sec.water_indices.len()) as u32;
                if vcount == 0 || icount == 0 {
                    continue;
                }
                let Some(vtx_off) = self.vtx_free.alloc(vcount) else {
                    self.free_slices(&taken);
                    tracing::debug!("Vertex pool full, skipping {:?}", mesh.pos);
                    pool_full = true;
                    break;
                };
                let Some(idx_off) = self.idx_free.alloc(icount) else {
                    self.vtx_free.free_region(vtx_off, vcount);
                    self.free_slices(&taken);
                    tracing::debug!("Index pool full, skipping {:?}", mesh.pos);
                    pool_full = true;
                    break;
                };
                taken.push((vtx_off, vcount, idx_off, icount));
                plans.push(Plan {
                    section_index: sec.section_index,
                    verts: &sec.vertices,
                    indices: &sec.indices,
                    water_indices: &sec.water_indices,
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
            if pool_full {
                // The accepted sections were retired above; report them so the
                // next rescan re-enqueues them.
                needs_remesh.push((mesh.pos, accepted.iter().copied().collect()));
                continue;
            }
            if plans.is_empty() {
                // Nothing to upload (all accepted sections were empty).
                continue;
            }

            if self.use_staging {
                let mv: usize = plans
                    .iter()
                    .map(|p| p.verts.len() * VERTEX_SIZE as usize)
                    .sum();
                let mi: usize = plans
                    .iter()
                    .map(|p| (p.indices.len() + p.water_indices.len()) * INDEX_SIZE as usize)
                    .sum();
                // This mesh alone fits a half (checked above), so a flush always
                // makes room: submit the pending transfer and reset the cursors.
                if stg_v + mv > staging_half || stg_i + mi > staging_half {
                    self.flush_transfer(device, &copy_v, &copy_i);
                    copy_v.clear();
                    copy_i.clear();
                    stg_v = 0;
                    stg_i = 0;
                }
                for p in &plans {
                    self.staging_buffer
                        .write_at(stg_v, bytemuck::cast_slice(p.verts));
                    let vbytes = p.verts.len() * VERTEX_SIZE as usize;
                    copy_v.push(rhi::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: p.vtx_off as u64 * VERTEX_SIZE,
                        size: vbytes as u64,
                    });
                    stg_v += vbytes;

                    let off = staging_half + stg_i;
                    let ibytes = p.write_indices(&mut self.staging_buffer, off);
                    copy_i.push(rhi::BufferCopy {
                        src_offset: off as u64,
                        dst_offset: p.idx_off as u64 * INDEX_SIZE,
                        size: ibytes as u64,
                    });
                    stg_i += ibytes;
                }
            } else {
                for p in &plans {
                    let base = p.vtx_off as usize * VERTEX_SIZE as usize;
                    self.vertex_buffer
                        .write_at(base, bytemuck::cast_slice(p.verts));
                    p.write_indices(
                        &mut self.index_buffer,
                        p.idx_off as usize * INDEX_SIZE as usize,
                    );
                }
            }

            let now = std::time::Instant::now();
            let new_sections = plans.iter().map(|p| SectionAlloc {
                section_index: p.section_index,
                aabb: p.aabb,
                origin: p.origin,
                first_index: p.idx_off,
                index_count: p.indices.len() as u32,
                solid_index_count: p.solid_index_count,
                water_first_index: p.idx_off + p.indices.len() as u32,
                water_index_count: p.water_indices.len() as u32,
                idx_len: (p.indices.len() + p.water_indices.len()) as u32,
                vertex_offset: p.vtx_off as i32,
                vtx_len: p.verts.len() as u32,
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
        if self.use_staging {
            self.flush_transfer(device, &copy_v, &copy_i);
        }

        let total_sections: usize = self.chunks.values().map(|c| c.sections.len()).sum();
        self.ensure_meta_capacity(device, total_sections);

        needs_remesh
    }

    /// Grow the per-frame meta and indirect buffers so they can hold `needed`
    /// section draws. No-op while capacity suffices.
    fn ensure_meta_capacity(&mut self, device: &rhi::backend::Device, needed: usize) {
        if needed <= self.max_meta {
            return;
        }
        let new_max = (needed.saturating_mul(3) / 2)
            .next_power_of_two()
            .max(self.max_meta * 2);

        // The meta/indirect buffers are referenced by every in-flight frame's
        // bind group; wait the GPU out before freeing them.
        device.wait_idle();

        let meta_size = (new_max * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (new_max * size_of::<DrawCommand>()) as u64;
        for list in std::iter::once(&mut self.camera).chain(&mut self.shadow) {
            list.regrow(device, self.cull_layout, meta_size, indirect_size);
        }

        self.max_meta = new_max;
    }

    /// Return one slice's vertex and index ranges to the pools.
    fn free_slice(&mut self, (vo, vl, io, il): (u32, u32, u32, u32)) {
        self.vtx_free.free_region(vo, vl);
        self.idx_free.free_region(io, il);
    }

    /// Return slices immediately. Only safe for slices never submitted to a
    /// frame (e.g. rolling back allocations made earlier in the same `upload`);
    /// slices that may still be drawn by an in-flight frame must go through
    /// `retire_slices`.
    fn free_slices(&mut self, slices: &[(u32, u32, u32, u32)]) {
        for &slice in slices {
            self.free_slice(slice);
        }
    }

    /// Defer returning slices to the pools until `MAX_FRAMES_IN_FLIGHT` frames
    /// have passed, so the GPU can't still be reading them from an in-flight
    /// frame. Use for slices that were potentially drawn (re-mesh replacement,
    /// chunk unload).
    fn retire_slices(&mut self, slices: impl IntoIterator<Item = (u32, u32, u32, u32)>) {
        let retire_at = self.frame_seq + MAX_FRAMES_IN_FLIGHT as u64;
        for slice in slices {
            self.pending_free.push_back((retire_at, slice));
        }
    }

    /// Advance one frame and reclaim any slices whose retirement deadline has
    /// passed. Call once per rendered frame, right after the frame's fence has
    /// been waited (that wait guarantees the frame from `MAX_FRAMES_IN_FLIGHT`
    /// ago — and everything before it — is done on the GPU).
    pub fn begin_frame(&mut self) {
        self.frame_seq += 1;
        while self
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.frame_seq)
        {
            let (_, slice) = self.pending_free.pop_front().unwrap();
            self.free_slice(slice);
        }
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        if let Some(alloc) = self.chunks.remove(pos) {
            self.retire_slices(alloc.sections.iter().map(|sec| {
                (
                    sec.vertex_offset as u32,
                    sec.vtx_len,
                    sec.first_index,
                    sec.idx_len,
                )
            }));
            self.meta_dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.vtx_free.reset();
        self.idx_free.reset();
        self.pending_free.clear();
        self.camera.meta.clear();
        self.meta_dirty = true;
        self.fade_enabled = false;
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
                &mut self.camera.meta,
            );
            self.meta_dirty = false;
            self.shadow_dirty = true;
        }

        let cam_moved = (eye - self.last_sort_cam).length_squared() > SORT_RECAM_SQ;
        if content_changed || cam_moved {
            sort_meta(&mut self.camera.meta, eye);
            self.last_sort_cam = eye;
            // Draw list reordered: every frame slot's meta buffer needs the refresh.
            self.camera.upload_pending = MAX_FRAMES_IN_FLIGHT as u32;
        }

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so its readback holds the previous cull's total (solid + cutout draws).
        self.last_draw_count = self.count_readbacks[frame].read(|bytes| {
            let counts: [u32; 2] = bytemuck::pod_read_unaligned(&bytes[..8]);
            counts[0] + counts[1]
        });
        self.camera.cull(
            encoder,
            &self.compute_pipeline,
            frame,
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
                    dst_offset: i as u64 * COUNT_SIZE,
                    size: COUNT_SIZE,
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
        let shadow = self.shadow.get_or_insert_with(|| {
            let meta_size = (self.max_meta * size_of::<ChunkMeta>()) as u64;
            let indirect_size = (self.max_meta * size_of::<DrawCommand>()) as u64;
            DrawList::new(device, self.cull_layout, meta_size, indirect_size)
        });
        if self.shadow_dirty {
            build_meta(
                &self.chunks,
                None,
                self.fade_enabled,
                eye,
                std::time::Instant::now(),
                &mut shadow.meta,
            );
            shadow.upload_pending = MAX_FRAMES_IN_FLIGHT as u32;
            self.shadow_dirty = false;
        }
        shadow.cull(
            encoder,
            &self.compute_pipeline,
            frame,
            frustum,
            anchor,
            eye,
            !self.indirect_draw.count,
        );
    }

    /// Issue one render layer's indirect draws for `view`. `cutout` selects
    /// the discard pass's draw list (drawn after `solid`, which lays down
    /// depth); the caller binds the matching pipeline first. Both layers share
    /// the vertex/index/meta buffers and the cull-written draw lists.
    pub fn draw_indirect(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        cutout: bool,
        view: View,
    ) {
        let list = match view {
            View::Camera => &self.camera,
            View::Shadow => match &self.shadow {
                Some(shadow) => shadow,
                None => return,
            },
        };
        if self.chunks.is_empty() {
            return;
        }

        let max_draws = self
            .chunks
            .values()
            .map(|c| c.sections.len() as u32)
            .sum::<u32>();
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

        // Binding 0: packed vertex pool. Binding 1: the meta buffer, read per
        // instance for the section origin + fade (indexed by `first_instance`).
        pass.set_vertex_buffer(0, self.vertex_buffer.handle());
        pass.set_vertex_buffer(1, list.meta_buffers[frame].handle());
        pass.set_index_buffer(self.index_buffer.handle());
        let stride = size_of::<DrawCommand>() as u32;
        if self.indirect_draw.count {
            pass.draw_indexed_indirect_count(indirect, count, max_draws, stride);
        } else if self.indirect_draw.multi_draw {
            pass.draw_indexed_indirect(indirect, 0, max_draws, stride);
        } else {
            for i in 0..max_draws {
                pass.draw_indexed_indirect(indirect, (i * stride) as u64, 1, stride);
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
    /// cull. Reuses the shared vertex/index buffers (water indices live right
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

        pass.set_vertex_buffer(0, self.vertex_buffer.handle());
        pass.set_index_buffer(self.index_buffer.handle());

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
        for buffer in [&mut self.vertex_buffer, &mut self.index_buffer] {
            std::mem::take(buffer).destroy(device);
        }
        for list in std::iter::once(&mut self.camera).chain(&mut self.shadow) {
            list.destroy(device);
        }
        util::destroy_buffers(device, &mut self.count_readbacks);
        std::mem::take(&mut self.staging_buffer).destroy(device);

        self.compute_pipeline.destroy(device);
        self.compute_layout.destroy(device);
        self.cull_layout.destroy(device);
    }
}

/// Which view's draw lists [`ChunkBufferStore::draw_indirect`] draws.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Camera,
    /// A shader pack's shadow pass.
    Shadow,
}

/// One view's sections and, per frame slot, the cull's frustum and the draw
/// lists it writes.
struct DrawList {
    meta: Vec<ChunkMeta>,
    /// Frame slots still needing the latest `meta` uploaded. Set to
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
        meta_size: u64,
        indirect_size: u64,
    ) -> Self {
        let count = |name: &str| {
            util::create_per_frame_host_buffers(
                device,
                COUNT_SIZE,
                rhi::BufferUsages::STORAGE
                    | rhi::BufferUsages::INDIRECT
                    | rhi::BufferUsages::COPY_SRC,
                name,
            )
        };
        let mut this = Self {
            meta: Vec::new(),
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
        this.regrow(device, layout, meta_size, indirect_size);
        this
    }

    /// Remakes the per-frame meta and indirect buffers at these sizes and
    /// points each frame's cull group at its buffers.
    fn regrow(
        &mut self,
        device: &rhi::backend::Device,
        layout: rhi::backend::BindGroupLayout,
        meta_size: u64,
        indirect_size: u64,
    ) {
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
                (3, storage(&self.count_buffers[i], COUNT_SIZE)),
                (4, storage(&self.indirect_cutout_buffers[i], indirect_size)),
                (5, storage(&self.count_cutout_buffers[i], COUNT_SIZE)),
            ];
            self.cull_sets[i].replace(device, layout, &bindings);
        }
    }

    /// Records the cull of `meta` against `frustum` into this frame slot's
    /// draw lists, uploading `meta` first if the slot hasn't seen it.
    /// `zero_indirect`: without drawIndirectCount the whole indirect buffer
    /// is drawn, so slots the cull leaves unfilled must read as no-op draws.
    #[allow(clippy::too_many_arguments)]
    fn cull(
        &mut self,
        encoder: &rhi::backend::CommandEncoder,
        pipeline: &rhi::backend::ComputePipeline,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        zero_indirect: bool,
    ) {
        if self.upload_pending > 0 {
            self.meta_buffers[frame].write(bytemuck::cast_slice(&self.meta));
            self.upload_pending -= 1;
        }
        let count = self.meta.len() as u32;
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
            buffer.write(&0u32.to_ne_bytes());
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

        let mut meta = Vec::new();
        build_meta(&chunks, Some(&vis), true, eye, now, &mut meta);
        sort_meta(&mut meta, eye);

        assert_eq!(meta.len(), 2);
        assert_eq!(meta[0].origin, [0, 0, 0]);
        assert_eq!(f32::from_bits(meta[0].visibility), 1.0);
        assert_eq!(meta[1].origin, [64, 0, 0]);
        let far_vis = f32::from_bits(meta[1].visibility);
        assert!((far_vis - 0.5).abs() < 0.01, "far fade {far_vis}");

        build_meta(&chunks, Some(&vis), false, eye, now, &mut meta);
        assert!(meta.iter().all(|m| f32::from_bits(m.visibility) == 1.0));

        build_meta(&chunks, None, false, eye, now, &mut meta);
        assert_eq!(meta.len(), 3);
    }
}
