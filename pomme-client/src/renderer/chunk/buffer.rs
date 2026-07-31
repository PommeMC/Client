use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use azalea_core::position::{ChunkPos, ChunkSectionPos};
use glam::DVec3;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::dispatcher::pack_section_pos;
use super::mesher::{ChunkAABB, FADE_DURATION_MS, PackedVertex, SectionMeshData};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

const BUCKET_VERTICES: u32 = 32768;
const VERTEX_SIZE: u64 = size_of::<PackedVertex>() as u64;
const BYTES_PER_BUCKET: u64 = BUCKET_VERTICES as u64 * VERTEX_SIZE;
/// Initial capacity (in quads) of the shared static quad index buffer; grown
/// by doubling if a single section's pass ever exceeds it.
const INITIAL_QUAD_INDEX_QUADS: u32 = 16384;
/// Manhattan-distance buckets (in sections) ordering the translucent water
/// draw list back-to-front on the GPU; sections farther than the last bucket
/// clamp into it.
const WATER_BUCKETS: u32 = 512;
const MIN_BUCKETS: u32 = 128;
// A full-world reload (dimension change, render-distance toggle) transiently
// holds both worlds: unloaded slices stay allocated until their frame deadline
// while the new world uploads. The cap leaves room for that 2x so the reload
// never hits the emergency GPU-wait reclaim; `VRAM_BUDGET_FRACTION` still
// bounds smaller cards.
const MAX_BUCKETS: u32 = 4096;
/// Integrated GPUs share system RAM (reported as device-local), so their pool
/// caps at ~512 MB instead of the discrete cards' ~1.75 GB.
const MAX_BUCKETS_INTEGRATED: u32 = 1024;
const VRAM_BUDGET_FRACTION: f64 = 0.25;
/// Sections whose center sits within this squared distance of the camera
/// render opaque immediately and never fade in.
const NEARBY_DIST_SQ: f32 = 768.0;

/// Whether a section's center is within the always-near radius of the eye
/// (vanilla `isNearby`: a 3D distance on the section center, so a section 30
/// blocks overhead still fades), rebased in f64 for precision at extreme
/// coordinates.
fn section_is_near(spos: ChunkSectionPos, eye: DVec3) -> bool {
    let dx = spos.x as f64 * 16.0 + 8.0 - eye.x;
    let dy = spos.y as f64 * 16.0 + 8.0 - eye.y;
    let dz = spos.z as f64 * 16.0 + 8.0 - eye.z;
    dx * dx + dy * dy + dz * dz < NEARBY_DIST_SQ as f64
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

    /// Extend the managed range, making the new tail region available.
    fn grow(&mut self, new_capacity: u32) {
        let old = self.capacity;
        self.capacity = new_capacity;
        self.free_region(old, new_capacity - old);
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

    /// Largest contiguous free run, for the pool-exhaustion diagnostics
    /// (distinguishes "full" from "fragmented").
    fn largest_free(&self) -> u32 {
        self.free.iter().map(|&(_, n)| n).max().unwrap_or(0)
    }
}

fn compute_bucket_count(physical_device: vk::PhysicalDevice) -> u32 {
    let mem_props = physical_device.get_memory_properties();
    let props = physical_device.get_properties();
    let mut device_local_bytes: u64 = 0;
    for i in 0..mem_props.memory_type_count as usize {
        let mem_type = mem_props.memory_types[i];
        if mem_type
            .property_flags
            .contains(vk::MemoryPropertyFlags::DeviceLocal)
        {
            let heap = mem_props.memory_heaps[mem_type.heap_index as usize];
            if heap.size > device_local_bytes {
                device_local_bytes = heap.size;
            }
        }
    }
    let budget = (device_local_bytes as f64 * VRAM_BUDGET_FRACTION) as u64;
    // Integrated GPUs report system RAM as their device-local heap, so the
    // fraction would eagerly reserve gigabytes of host memory; cap them.
    let max_buckets = if props.device_type == vk::PhysicalDeviceType::DiscreteGpu {
        MAX_BUCKETS
    } else {
        MAX_BUCKETS_INTEGRATED
    };
    let buckets = (budget / BYTES_PER_BUCKET) as u32;
    let count = buckets.clamp(MIN_BUCKETS, max_buckets);
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
struct ChunkMeta {
    /// Section-local vertex bounds; the cull shader rebases them via `origin`.
    aabb_min: [f32; 4],
    aabb_max: [f32; 4],
    /// Leading solid-pass quads of the section's vertex slice; the cull shader
    /// derives each pass's draw against the shared quad index buffer from
    /// these (6 indices and 4 vertices per quad).
    solid_quads: u32,
    /// Cutout-pass quads following the solid group.
    cutout_quads: u32,
    vertex_offset: i32,
    /// Upload stamp in session millis (`camera::session_millis`); the vertex
    /// shader computes the fade-in from it, so fades cost nothing per frame.
    uploaded_ms: u32,
    /// Absolute section origin as integers (vanilla `ChunkPosition`), bound
    /// as a per-instance vertex attribute; the vertex shader subtracts the
    /// camera block position in integer math, so no large f32 is ever
    /// formed.
    origin: [i32; 3],
    /// Trailing water quads of the vertex slice; the cull emits them into the
    /// bucketed translucent draw list. Fills `origin`'s fourth lane, keeping
    /// the struct at 64 bytes.
    water_quads: u32,
}

/// Copy already-packed `verts` into `dst` starting at byte `off`.
fn write_verts(dst: &mut [u8], off: usize, verts: &[PackedVertex]) {
    let bytes: &[u8] = bytemuck::cast_slice(verts);
    dst[off..off + bytes.len()].copy_from_slice(bytes);
}

/// Vertex input for the chunk pipeline: binding 0 is the packed per-vertex
/// pool, binding 1 is the meta buffer read per-instance (origin + fade),
/// indexed by the `first_instance` the cull shader writes.
pub fn chunk_vertex_bindings() -> [vk::VertexInputBindingDescription; 2] {
    [
        vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<PackedVertex>() as u32,
            input_rate: vk::VertexInputRate::Vertex,
        },
        vk::VertexInputBindingDescription {
            binding: 1,
            stride: size_of::<ChunkMeta>() as u32,
            input_rate: vk::VertexInputRate::Instance,
        },
    ]
}

pub fn chunk_vertex_attributes() -> [vk::VertexInputAttributeDescription; 6] {
    let pos_off = std::mem::offset_of!(PackedVertex, pos) as u32;
    let uv_off = std::mem::offset_of!(PackedVertex, uv) as u32;
    let light_tint_off = std::mem::offset_of!(PackedVertex, light_tint) as u32;
    let origin_off = std::mem::offset_of!(ChunkMeta, origin) as u32;
    let uploaded_off = std::mem::offset_of!(ChunkMeta, uploaded_ms) as u32;
    [
        // binding 0 — packed vertex (pos split into xy + z lanes)
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R16G16Unorm,
            offset: pos_off,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R16Unorm,
            offset: pos_off + 4,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R16G16Unorm,
            offset: uv_off,
        },
        vk::VertexInputAttributeDescription {
            location: 3,
            binding: 0,
            format: vk::Format::R8G8B8A8Unorm,
            offset: light_tint_off,
        },
        // binding 1 — per-instance meta (origin + upload stamp)
        vk::VertexInputAttributeDescription {
            location: 4,
            binding: 1,
            format: vk::Format::R32G32B32Sint,
            offset: origin_off,
        },
        vk::VertexInputAttributeDescription {
            location: 5,
            binding: 1,
            format: vk::Format::R32Uint,
            offset: uploaded_off,
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
pub(crate) fn aabb_in_frustum(
    aabb: &ChunkAABB,
    origin: [i32; 3],
    planes: &[[f32; 4]; 6],
    eye: DVec3,
) -> bool {
    let base = (DVec3::new(origin[0] as f64, origin[1] as f64, origin[2] as f64) - eye).as_vec3();
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
    /// Hi-Z mask decode parameters: the center and section layout the slot's
    /// mask was written with (last frame). `mask_valid = 0` skips the
    /// mask test entirely (slot never written).
    mask_center: [i32; 2],
    mask_min_section: i32,
    mask_section_count: i32,
    mask_valid: u32,
    /// Player column + render distance for the shader-side column cull
    /// (`limit_rd = 0` disables it); replaces the CPU rebuild's column skip.
    player_chunk: [i32; 2],
    limit_rd: u32,
    /// Pads the struct to a 16-byte multiple.
    _pad: u32,
}

/// `meta_slot` of a tombstone: a section whose latest accepted result was
/// empty. The entry owns no GPU slices; it only preserves the section's
/// `(content_gen, epoch)` floor so a slower pre-edit mesh can't resurrect
/// geometry the empty result already replaced.
const TOMBSTONE_SLOT: u32 = u32::MAX;

/// One uploaded 16³ section: a vertex slice of whole quads grouped
/// `[solid][cutout][water]`, drawn against the shared quad index buffer.
/// `vertex_offset` is the slice base and `vtx_len` its length, kept so the
/// slice can be returned to the free-list on removal. Everything the draws
/// need (AABB, quad counts, origin, upload stamp) lives GPU-side in the
/// section's persistent meta entry.
struct SectionAlloc {
    section_index: i32,
    /// This section's stable slot in the GPU meta buffers ([`TOMBSTONE_SLOT`]
    /// for an empty section); freed slots recycle only after
    /// `MAX_FRAMES_IN_FLIGHT` frames (via `pending_free`) so an in-flight
    /// cull never reads a repurposed entry.
    meta_slot: u32,
    vertex_offset: i32,
    vtx_len: u32,
    /// Snapshot recency + upload stamp this section's geometry came from; an
    /// upload with a lexicographically older pair is rejected. `content_gen`
    /// (the dispatcher slot's version at claim time) orders by what the mesh
    /// *saw*; `upload_epoch` breaks ties between jobs claiming the same
    /// version. See [`SectionMeshData::upload_epoch`].
    content_gen: u64,
    epoch: u64,
}

impl SectionAlloc {
    fn is_tombstone(&self) -> bool {
        self.meta_slot == TOMBSTONE_SLOT
    }

    fn order_key(&self) -> (u64, u64) {
        (self.content_gen, self.epoch)
    }
}

struct ChunkAlloc {
    sections: Vec<SectionAlloc>,
}

/// The `(vertex_offset, vtx_len, meta_slot)` pool slices a section occupies,
/// in the shape [`ChunkBufferStore::retire_slices`] consumes.
fn slice_of(s: &SectionAlloc) -> (u32, u32, u32) {
    (s.vertex_offset as u32, s.vtx_len, s.meta_slot)
}

/// One accepted section's payload, waiting for `record_copies` to write it
/// into the frame's staging slab (its pool slice is already reserved).
struct PendingCopy {
    vertices: Vec<PackedVertex>,
    vtx_off: u32,
}

pub struct ChunkBufferStore {
    /// Capacity (in draws) of the per-frame meta/indirect buffers. Grown on
    /// demand because per-section packing yields many more draws than buckets.
    max_meta: usize,
    /// Device limit on drawCount/maxDrawCount per indirect multi-draw.
    max_draw_indirect_count: u32,
    /// One-shot log guard for the (pathological) case where live sections
    /// exceed that limit and draws start truncating.
    warned_draw_cap: bool,
    /// Rate limit for the vertex-pool-exhausted warning.
    last_pool_warn: Option<std::time::Instant>,
    vertex_buffer: vk::Buffer,
    vertex_alloc: Allocation,
    /// Shared static index buffer holding the repeating `0,1,2,2,3,0` quad
    /// pattern; every pass of every section draws against it with
    /// `first_index = 0` and its own `vertex_offset`, so sections store no
    /// indices at all. Grown by `ensure_quad_index_capacity`.
    quad_index_buffer: vk::Buffer,
    quad_index_alloc: Allocation,
    /// Quad capacity of `quad_index_buffer`; 0 until first created.
    quad_index_quads: u32,
    /// Host pattern source for the staged path, kept alive until the next
    /// growth so the copy recorded by `record_copies` can't outlive it.
    quad_index_src: Option<(vk::Buffer, Allocation)>,
    /// A (re)created quad index buffer whose pattern copy hasn't been
    /// recorded into a frame command buffer yet.
    quad_index_copy_pending: bool,
    /// Per-frame staging slabs: slot `frame` is only rewritten after that
    /// slot's fence was waited, so the copies recorded into the frame command
    /// buffer never race a previous frame's transfer reads.
    staging_buffers: Vec<vk::Buffer>,
    staging_allocs: Vec<Allocation>,
    staging_size: u64,
    use_staging: bool,
    /// Sections accepted by `stage_mesh_batch` (pool slices already reserved),
    /// written and recorded into the frame command buffer by `record_copies`.
    pending_copies: Vec<PendingCopy>,
    /// Bytes `pending_copies` will occupy in the staging slab, so a staging
    /// pass that carried over (skipped frame) still bounds the next batch.
    pending_v_bytes: usize,

    /// Exact-size sub-allocator over the vertex pool (in vertices).
    vtx_free: FreeList,
    chunks: HashMap<ChunkPos, ChunkAlloc>,
    /// Time spent in `reclaim_retired`'s GPU wait during the last
    /// `stage_mesh_batch`, for the benchmark's upload breakdown.
    pub last_reclaim_ms: f32,
    /// Stable-slot allocator over the GPU meta buffers (in entries). The meta
    /// is persistent: uploads write single entries, nothing per frame scales
    /// with loaded sections.
    meta_free: FreeList,
    /// CPU mirror of the meta entries by slot; the source for repopulating
    /// recreated buffers on growth.
    meta_mirror: Vec<ChunkMeta>,
    /// One past the highest slot ever allocated; the cull dispatch and the
    /// indirect draws' max count cover exactly this range.
    meta_high_water: u32,
    /// Meta entries written since the last time each frame slot caught up;
    /// applied to a slot's buffer in `dispatch_cull` after its fence wait.
    meta_writes: Vec<(u32, ChunkMeta)>,
    meta_applied: [usize; MAX_FRAMES_IN_FLIGHT],

    compute_pipeline: vk::Pipeline,
    water_scan_pipeline: vk::Pipeline,
    water_emit_pipeline: vk::Pipeline,
    compute_layout: vk::PipelineLayout,
    compute_desc_layout: vk::DescriptorSetLayout,
    compute_pool: vk::DescriptorPool,
    compute_sets: Vec<vk::DescriptorSet>,

    meta_buffers: Vec<vk::Buffer>,
    meta_allocs: Vec<Allocation>,
    // Solid (no-discard, early-Z) draw list, written by the cull shader.
    indirect_buffers: Vec<vk::Buffer>,
    indirect_allocs: Vec<Allocation>,
    count_buffers: Vec<vk::Buffer>,
    count_allocs: Vec<Allocation>,
    // Cutout (discard) draw list. Same sections, the back of each section's
    // index slice; drawn in a second pass after solid lays down depth.
    indirect_cutout_buffers: Vec<vk::Buffer>,
    indirect_cutout_allocs: Vec<Allocation>,
    count_cutout_buffers: Vec<vk::Buffer>,
    count_cutout_allocs: Vec<Allocation>,
    // Translucent water draw list: commands written by the emit pass in
    // back-to-front bucket order, count by the scan pass.
    water_indirect_buffers: Vec<vk::Buffer>,
    water_indirect_allocs: Vec<Allocation>,
    water_count_buffers: Vec<vk::Buffer>,
    water_count_allocs: Vec<Allocation>,
    // Per-frame scratch for the water ordering: reverse-distance bucket
    // counts (+ one candidate counter), turned into offsets by the scan, and
    // the packed (slot << 9 | bucket) candidate list from the cull.
    water_bucket_buffers: Vec<vk::Buffer>,
    water_bucket_allocs: Vec<Allocation>,
    water_candidate_buffers: Vec<vk::Buffer>,
    water_candidate_allocs: Vec<Allocation>,
    frustum_buffers: Vec<vk::Buffer>,
    frustum_allocs: Vec<Allocation>,
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
    pending_free: VecDeque<(u64, (u32, u32, u32))>,
}

impl ChunkBufferStore {
    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> Self {
        let total_buckets = compute_bucket_count(physical_device);
        let vertex_size = total_buckets as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;

        let dev_props = physical_device.get_properties();
        let use_staging = dev_props.device_type == vk::PhysicalDeviceType::DiscreteGpu;
        // Spec floor is 65535 even with multiDrawIndirect; meta_high_water
        // passes it from ~RD 32, so every draw clamps to the device cap.
        let max_draw_indirect_count = dev_props.limits.max_draw_indirect_count;

        let (vertex_buffer, vertex_alloc) = if use_staging {
            util::create_device_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            )
        } else {
            util::create_host_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            )
        };

        // Discrete GPUs batch a frame's uploads through this buffer in one
        // transfer, so size it to hold several columns and keep sub-flushes rare.
        // The integrated path writes mapped memory directly and never touches it.
        let staging_size = if use_staging {
            BYTES_PER_BUCKET * 16
        } else {
            BYTES_PER_BUCKET * 4
        };
        let mut staging_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut staging_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        if use_staging {
            for _ in 0..MAX_FRAMES_IN_FLIGHT {
                let (b, a) = util::create_host_buffer(
                    device,
                    allocator,
                    staging_size,
                    vk::BufferUsageFlags::TransferSrc,
                    "staging",
                );
                staging_buffers.push(b);
                staging_allocs.push(a);
            }
        }

        tracing::info!(
            "Chunk buffers: {} (vertex={} MB, staging={} KB)",
            if use_staging {
                "DEVICE_LOCAL + staging"
            } else {
                "HOST_VISIBLE"
            },
            vertex_size / (1024 * 1024),
            staging_size / 1024,
        );

        let vtx_free = FreeList::new(total_buckets * BUCKET_VERTICES);

        // Per-section packing yields many more draws than buckets, so pre-size
        // generously: growth (`ensure_meta_capacity`) needs a `device.wait_idle`
        // to safely rewrite the descriptor sets, and that stall showed up as a
        // 27ms frame when an RD-32 world (~45k section draws) crossed 16x. The
        // grow path stays as a rare safety net.
        let max_meta = (total_buckets * 32).max(8192) as usize;
        let meta_size = (max_meta * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (max_meta * size_of::<DrawCommand>()) as u64;
        let count_size = 4u64;
        let frustum_size = size_of::<FrustumData>() as u64;

        let mut meta_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut meta_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut indirect_cutout_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_cutout_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut count_cutout_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut frustum_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut frustum_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_count_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_count_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_bucket_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut water_bucket_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::VertexBuffer,
                "chunk_meta",
            );
            meta_buffers.push(b);
            meta_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds",
            );
            indirect_buffers.push(b);
            indirect_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "draw_count",
            );
            count_buffers.push(b);
            count_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds_cutout",
            );
            indirect_cutout_buffers.push(b);
            indirect_cutout_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "draw_count_cutout",
            );
            count_cutout_buffers.push(b);
            count_cutout_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                frustum_size,
                vk::BufferUsageFlags::UniformBuffer,
                "frustum_ubo",
            );
            frustum_buffers.push(b);
            frustum_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "water_count",
            );
            water_count_buffers.push(b);
            water_count_allocs.push(a);

            // +1 slot past the buckets holds the candidate counter.
            let (b, a) = util::create_device_buffer(
                device,
                allocator,
                (WATER_BUCKETS as u64 + 1) * 4,
                vk::BufferUsageFlags::StorageBuffer,
                "water_buckets",
            );
            water_bucket_buffers.push(b);
            water_bucket_allocs.push(a);
        }

        let (
            water_indirect_buffers,
            water_indirect_allocs,
            water_candidate_buffers,
            water_candidate_allocs,
        ) = create_water_scaled_buffers(device, allocator, max_meta);

        let compute_desc_layout = create_cull_desc_layout(device);
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &compute_desc_layout,
            ..Default::default()
        };
        let compute_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create compute pipeline layout");

        let spec_entries = [vk::SpecializationMapEntry {
            constant_id: 0,
            offset: 0,
            size: size_of::<i32>(),
        }];
        let spec_data = [crate::util::MAX_RD as i32];
        let spec_info = vk::SpecializationInfo {
            map_entry_count: spec_entries.len() as u32,
            map_entries: spec_entries.as_ptr(),
            data_size: std::mem::size_of_val(&spec_data),
            data: spec_data.as_ptr() as *const _,
            ..Default::default()
        };
        let compute_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("cull.comp.spv"),
            Some(&spec_info),
        );
        let water_scan_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("water_scan.comp.spv"),
            None,
        );
        let water_emit_pipeline = create_compute_pipeline(
            device,
            compute_layout,
            shader::include_spirv!("water_emit.comp.spv"),
            None,
        );

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::StorageBuffer,
                // meta + solid indirect/count + cutout indirect/count +
                // visibility mask + water indirect/count/buckets/candidates
                // = 10 per frame.
                descriptor_count: 10 * MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let compute_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create cull desc pool");

        let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| compute_desc_layout)
            .collect();
        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: compute_pool,
            descriptor_set_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let mut compute_sets = vec![vk::DescriptorSet::null(); layouts.len()];
        device
            .allocate_descriptor_sets(&alloc_info, &mut compute_sets)
            .expect("failed to allocate cull desc sets");

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (meta_info, mut meta_write) = desc_write(
                compute_sets[i],
                0,
                vk::DescriptorType::StorageBuffer,
                meta_buffers[i],
                meta_size,
            );

            let (frustum_info, mut frustum_write) = desc_write(
                compute_sets[i],
                1,
                vk::DescriptorType::UniformBuffer,
                frustum_buffers[i],
                frustum_size,
            );

            let (indirect_info, mut indirect_write) = desc_write(
                compute_sets[i],
                2,
                vk::DescriptorType::StorageBuffer,
                indirect_buffers[i],
                indirect_size,
            );

            let (count_info, mut count_write) = desc_write(
                compute_sets[i],
                3,
                vk::DescriptorType::StorageBuffer,
                count_buffers[i],
                count_size,
            );

            let (indirect_c_info, mut indirect_c_write) = desc_write(
                compute_sets[i],
                4,
                vk::DescriptorType::StorageBuffer,
                indirect_cutout_buffers[i],
                indirect_size,
            );

            let (count_c_info, mut count_c_write) = desc_write(
                compute_sets[i],
                5,
                vk::DescriptorType::StorageBuffer,
                count_cutout_buffers[i],
                count_size,
            );

            let (buckets_info, mut buckets_write) = desc_write(
                compute_sets[i],
                7,
                vk::DescriptorType::StorageBuffer,
                water_bucket_buffers[i],
                (WATER_BUCKETS as u64 + 1) * 4,
            );

            let (candidates_info, mut candidates_write) = desc_write(
                compute_sets[i],
                8,
                vk::DescriptorType::StorageBuffer,
                water_candidate_buffers[i],
                max_meta as u64 * 4,
            );

            let (water_ind_info, mut water_ind_write) = desc_write(
                compute_sets[i],
                9,
                vk::DescriptorType::StorageBuffer,
                water_indirect_buffers[i],
                indirect_size,
            );

            let (water_count_info, mut water_count_write) = desc_write(
                compute_sets[i],
                10,
                vk::DescriptorType::StorageBuffer,
                water_count_buffers[i],
                count_size,
            );

            meta_write.buffer_info = meta_info.as_ptr();
            frustum_write.buffer_info = frustum_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            count_write.buffer_info = count_info.as_ptr();
            indirect_c_write.buffer_info = indirect_c_info.as_ptr();
            count_c_write.buffer_info = count_c_info.as_ptr();
            buckets_write.buffer_info = buckets_info.as_ptr();
            candidates_write.buffer_info = candidates_info.as_ptr();
            water_ind_write.buffer_info = water_ind_info.as_ptr();
            water_count_write.buffer_info = water_count_info.as_ptr();

            let writes = [
                meta_write,
                frustum_write,
                indirect_write,
                count_write,
                indirect_c_write,
                count_c_write,
                buckets_write,
                candidates_write,
                water_ind_write,
                water_count_write,
            ];

            device.update_descriptor_sets(&writes, &[]);
        }

        let mut this = Self {
            max_meta,
            max_draw_indirect_count,
            warned_draw_cap: false,
            last_pool_warn: None,
            vertex_buffer,
            vertex_alloc,
            quad_index_buffer: vk::Buffer::null(),
            quad_index_alloc: Allocation::default(),
            quad_index_quads: 0,
            quad_index_src: None,
            quad_index_copy_pending: false,
            staging_buffers,
            staging_allocs,
            staging_size,
            use_staging,
            pending_copies: Vec::new(),
            pending_v_bytes: 0,
            vtx_free,
            chunks: HashMap::new(),
            last_reclaim_ms: 0.0,
            meta_free: FreeList::new(max_meta as u32),
            meta_mirror: vec![bytemuck::Zeroable::zeroed(); max_meta],
            meta_high_water: 0,
            meta_writes: Vec::new(),
            meta_applied: [0; MAX_FRAMES_IN_FLIGHT],
            compute_pipeline,
            water_scan_pipeline,
            water_emit_pipeline,
            compute_layout,
            compute_desc_layout,
            compute_pool,
            compute_sets,
            meta_buffers,
            meta_allocs,
            indirect_buffers,
            indirect_allocs,
            count_buffers,
            count_allocs,
            indirect_cutout_buffers,
            indirect_cutout_allocs,
            count_cutout_buffers,
            count_cutout_allocs,
            water_indirect_buffers,
            water_indirect_allocs,
            water_count_buffers,
            water_count_allocs,
            water_bucket_buffers,
            water_bucket_allocs,
            water_candidate_buffers,
            water_candidate_allocs,
            frustum_buffers,
            frustum_allocs,
            fade_enabled: false,
            last_draw_count: 0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
        };
        this.ensure_quad_index_capacity(device, allocator, INITIAL_QUAD_INDEX_QUADS);
        this
    }

    /// Make sure the shared static quad index buffer covers `quads` quads for
    /// a single draw. Growth doubles and only a record-size section triggers
    /// it, so the `wait_idle` needed to swap out the referenced buffer stays a
    /// rare safety net. The staged path's pattern copy is recorded by the next
    /// `record_copies`, which precedes the frame's draws.
    fn ensure_quad_index_capacity(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        quads: u32,
    ) {
        if quads <= self.quad_index_quads {
            return;
        }
        let new_quads = quads.next_power_of_two().max(INITIAL_QUAD_INDEX_QUADS);
        let size = new_quads as u64 * 6 * size_of::<u32>() as u64;

        if self.quad_index_quads > 0 {
            // In-flight frames may still reference the old buffer (and the
            // staged path's copy source).
            device.wait_idle().ok();
            let mut alloc = allocator.lock().unwrap();
            device.destroy_buffer(self.quad_index_buffer, None);
            alloc.free(std::mem::take(&mut self.quad_index_alloc)).ok();
            if let Some((buf, allocation)) = self.quad_index_src.take() {
                device.destroy_buffer(buf, None);
                alloc.free(allocation).ok();
            }
        }

        let mut pattern: Vec<u32> = Vec::with_capacity(new_quads as usize * 6);
        for q in 0..new_quads {
            let base = q * 4;
            pattern.extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&pattern);

        let create = if self.use_staging {
            util::create_device_buffer
        } else {
            util::create_host_buffer
        };
        let (buf, mut alloc) = create(
            device,
            allocator,
            size,
            vk::BufferUsageFlags::IndexBuffer,
            "quad_index",
        );
        if self.use_staging {
            let (src_buf, mut src_alloc) = util::create_host_buffer(
                device,
                allocator,
                size,
                vk::BufferUsageFlags::TransferSrc,
                "quad_index_src",
            );
            src_alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
            self.quad_index_src = Some((src_buf, src_alloc));
            self.quad_index_copy_pending = true;
        } else {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
        self.quad_index_buffer = buf;
        self.quad_index_alloc = alloc;
        self.quad_index_quads = new_quads;
        tracing::info!(
            "Quad index buffer: {} quads ({} KB)",
            new_quads,
            size / 1024
        );
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    /// Retained for the benchmark's frame breakdown: the per-frame meta
    /// rebuild no longer exists (the meta is GPU-persistent), so this is
    /// always zero.
    pub fn meta_rebuild_ms(&self) -> f32 {
        0.0
    }

    /// Write the staged sections into this frame's staging slab and record
    /// their pool copies into the frame command buffer, with a barrier so the
    /// frame's vertex/index reads see them. Runs after the frame fence wait,
    /// so rewriting the slab can't race an in-flight transfer.
    pub fn record_copies(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        // One-shot pattern upload for a (re)created quad index buffer; it
        // precedes the frame's draws in the same command buffer, so the
        // barrier below covers it.
        let quad_copy = self.quad_index_copy_pending;
        if quad_copy {
            self.quad_index_copy_pending = false;
            let (src, _) = self.quad_index_src.as_ref().unwrap();
            let copy = [vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: self.quad_index_quads as u64 * 6 * size_of::<u32>() as u64,
            }];
            cmd.copy_buffer(*src, self.quad_index_buffer, &copy);
        }
        if self.pending_copies.is_empty() && !quad_copy {
            return;
        }
        if !self.pending_copies.is_empty() {
            let mut copy_v: Vec<vk::BufferCopy> = Vec::with_capacity(self.pending_copies.len());
            let mut stg_v = 0usize;
            {
                let buf = self.staging_allocs[frame].mapped_slice_mut().unwrap();
                for pending in &self.pending_copies {
                    write_verts(buf, stg_v, &pending.vertices);
                    let vbytes = pending.vertices.len() * VERTEX_SIZE as usize;
                    copy_v.push(vk::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: pending.vtx_off as u64 * VERTEX_SIZE,
                        size: vbytes as u64,
                    });
                    stg_v += vbytes;
                }
            }
            cmd.copy_buffer(self.staging_buffers[frame], self.vertex_buffer, &copy_v);
        }
        let barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::VertexAttributeRead | vk::AccessFlags::IndexRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::VertexInput,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );
        self.drop_pending_copies();
    }

    /// Forget staged-but-unrecorded copies and their budget accounting.
    fn drop_pending_copies(&mut self) {
        self.pending_copies.clear();
        self.pending_v_bytes = 0;
    }

    /// Drop staged-but-unrecorded copies whose destination slice was just
    /// retired (their section is replaced or gone). Without this, a skipped
    /// frame's leftover copy plus an emergency reclaim re-issuing the range
    /// would put two overlapping destination regions into one
    /// `vkCmdCopyBuffer`, whose write order is undefined.
    fn drop_pending_copies_for(&mut self, freed: &[(u32, u32, u32)]) {
        if self.pending_copies.is_empty() || freed.is_empty() {
            return;
        }
        let mut dropped_bytes = 0usize;
        self.pending_copies.retain(|c| {
            if freed.iter().any(|&(vo, ..)| vo == c.vtx_off) {
                dropped_bytes += c.vertices.len() * VERTEX_SIZE as usize;
                false
            } else {
                true
            }
        });
        self.pending_v_bytes -= dropped_bytes;
    }

    /// Drain `mesh_queue` into the GPU pools, newest-epoch-per-section wins.
    /// Each accepted section replaces its slot's slices; an empty mesh retires
    /// the slot and drops the column when it goes empty. CPU-only: the staging
    /// path defers its byte writes and copies to `record_copies` in the frame
    /// command buffer instead of blocking on a transfer fence. If the staging
    /// budget or a pool fills, the loop stops and leaves the rest queued for
    /// next frame.
    pub fn stage_mesh_batch(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        mesh_queue: &mut VecDeque<SectionMeshData>,
        eye: DVec3,
    ) {
        self.last_reclaim_ms = 0.0;
        // Keep only the newest result per section before draining: the stale
        // check below reads `self.chunks`, which only reflects this batch's
        // uploads after the loop, so two same-section results in one drain would
        // otherwise both be accepted and the section drawn twice. Recency is
        // the `(content_gen, epoch)` pair — the slot version the mesh saw,
        // then the enqueue stamp — because a lower-epoch job can snapshot
        // *after* a higher-epoch one and carry the newer world state.
        // (Keyed by packed pos: azalea's ChunkSectionPos doesn't impl Hash.)
        let order = |m: &SectionMeshData| (m.content_gen, m.upload_epoch);
        let mut best: HashMap<u64, (u64, u64)> = HashMap::new();
        for mesh in mesh_queue.iter() {
            let key = pack_section_pos(mesh.spos);
            let cur = best.entry(key).or_insert_with(|| order(mesh));
            *cur = (*cur).max(order(mesh));
        }
        if best.len() < mesh_queue.len() {
            let mut seen = HashSet::new();
            mesh_queue.retain(|m| {
                let key = pack_section_pos(m.spos);
                order(m) == best[&key] && seen.insert(key)
            });
        }
        if mesh_queue.is_empty() {
            return;
        }

        let staging_budget = self.staging_size as usize;

        struct BatchEntry {
            mesh: SectionMeshData,
            col_pos: ChunkPos,
            si: i32,
            was_present: bool,
            vtx_off: u32,
            vcount: u32,
            meta_slot: u32,
        }
        let mut entries: Vec<BatchEntry> = Vec::new();

        // Include copies carried over from a skipped frame in the budget.
        let mut current_v_bytes = self.pending_v_bytes;
        while let Some(mesh) = mesh_queue.front() {
            let col_pos = ChunkPos::new(mesh.spos.x, mesh.spos.z);
            let si = mesh.relative_si;
            let stored = self
                .chunks
                .get(&col_pos)
                .and_then(|c| c.sections.iter().find(|s| s.section_index == si))
                .map(|s| s.order_key())
                .unwrap_or((0, 0));
            // Reject a stale upload a newer result already superseded.
            if order(mesh) < stored {
                mesh_queue.pop_front();
                continue;
            }
            if mesh.is_empty() {
                self.take_section(col_pos, si);
                // Keep a tombstone: the `(content_gen, epoch)` floor must
                // outlive the geometry, or a slower pre-edit mesh would pass
                // the gate above and resurrect the section the empty result
                // just cleared. Pruned with the column on unload.
                let (content_gen, epoch) = order(mesh);
                self.chunks
                    .entry(col_pos)
                    .or_insert_with(|| ChunkAlloc {
                        sections: Vec::new(),
                    })
                    .sections
                    .push(SectionAlloc {
                        section_index: si,
                        meta_slot: TOMBSTONE_SLOT,
                        vertex_offset: 0,
                        vtx_len: 0,
                        content_gen,
                        epoch,
                    });
                mesh_queue.pop_front();
                continue;
            }
            let vcount = mesh.vertices.len() as u32;
            if self.use_staging {
                let v_bytes = vcount as usize * VERTEX_SIZE as usize;
                // A section too large for the staging slab is skipped, not overflowed.
                if v_bytes > staging_budget {
                    tracing::warn!(
                        "Section {:?} too large for staging ({} bytes), skipping",
                        mesh.spos,
                        v_bytes,
                    );
                    mesh_queue.pop_front();
                    continue;
                }
                // This transfer's staging budget is full; leave the rest queued.
                if current_v_bytes + v_bytes > staging_budget {
                    break;
                }
                current_v_bytes += v_bytes;
            }
            // The shared quad index buffer must cover the section's largest
            // single-pass draw.
            let max_quads = mesh
                .solid_quads
                .max(mesh.cutout_quads)
                .max(mesh.water_quads);
            self.ensure_quad_index_capacity(device, allocator, max_quads);
            let Some(vtx_off) = self.alloc_vertices(device, vcount) else {
                // Rate-limited: exhaustion persists across frames.
                let now = std::time::Instant::now();
                if self
                    .last_pool_warn
                    .is_none_or(|t| now.duration_since(t).as_secs() >= 5)
                {
                    self.last_pool_warn = Some(now);
                    tracing::warn!(
                        "vertex pool exhausted (largest free run {} verts, wanted {});                          uploads stalled for {:?}",
                        self.vtx_free.largest_free(),
                        vcount,
                        mesh.spos,
                    );
                }
                break;
            };
            let meta_slot = self.alloc_meta_slot(device, allocator);
            let mesh = mesh_queue.pop_front().unwrap();
            let was_present = self.take_section(col_pos, si);
            entries.push(BatchEntry {
                mesh,
                col_pos,
                si,
                was_present,
                vtx_off,
                vcount,
                meta_slot,
            });
        }

        if entries.is_empty() {
            return;
        }

        let now_ms = crate::renderer::camera::session_millis();
        for entry in &entries {
            let spos = entry.mesh.spos;
            // A re-meshed section swaps instantly and near columns never fade
            // (vanilla `isNearby`); everything else fades in from its upload
            // stamp, computed shader-side against the session clock.
            let backdate =
                !self.fade_enabled || entry.was_present || section_is_near(entry.mesh.spos, eye);
            let uploaded_ms = if backdate {
                now_ms.wrapping_sub(2 * FADE_DURATION_MS as u32)
            } else {
                now_ms
            };
            self.queue_meta_write(
                entry.meta_slot,
                ChunkMeta {
                    aabb_min: entry.mesh.aabb.min,
                    aabb_max: entry.mesh.aabb.max,
                    solid_quads: entry.mesh.solid_quads,
                    cutout_quads: entry.mesh.cutout_quads,
                    vertex_offset: entry.vtx_off as i32,
                    uploaded_ms,
                    origin: [spos.x * 16, spos.y * 16, spos.z * 16],
                    water_quads: entry.mesh.water_quads,
                },
            );
            self.chunks
                .entry(entry.col_pos)
                .or_insert_with(|| ChunkAlloc {
                    sections: Vec::new(),
                })
                .sections
                .push(SectionAlloc {
                    section_index: entry.si,
                    meta_slot: entry.meta_slot,
                    vertex_offset: entry.vtx_off as i32,
                    vtx_len: entry.vcount,
                    content_gen: entry.mesh.content_gen,
                    epoch: entry.mesh.upload_epoch,
                });
        }

        if self.use_staging {
            for entry in &mut entries {
                self.pending_v_bytes += entry.mesh.vertices.len() * VERTEX_SIZE as usize;
                self.pending_copies.push(PendingCopy {
                    vertices: std::mem::take(&mut entry.mesh.vertices),
                    vtx_off: entry.vtx_off,
                });
            }
        } else {
            let vbuf = self.vertex_alloc.mapped_slice_mut().unwrap();
            for entry in &entries {
                let base = entry.vtx_off as usize * VERTEX_SIZE as usize;
                write_verts(vbuf, base, &entry.mesh.vertices);
            }
        }
    }

    /// Double the meta capacity: recreates the per-frame meta, indirect, and
    /// water buffers, then repopulates the meta from the CPU mirror. Needs a
    /// `wait_idle` (the buffers are referenced by every in-flight frame's
    /// descriptor set), so the initial capacity is pre-sized to make this a
    /// rare safety net.
    fn grow_meta(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let new_max = self.max_meta * 2;
        // cull.comp packs water candidates as (meta slot << 9) | bucket.
        debug_assert!(
            new_max <= 1 << 23,
            "meta slots exceed the water candidate packing"
        );

        device.wait_idle().ok();

        {
            let mut alloc = allocator.lock().unwrap();
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                device.destroy_buffer(self.meta_buffers[i], None);
                alloc.free(std::mem::take(&mut self.meta_allocs[i])).ok();
                device.destroy_buffer(self.indirect_buffers[i], None);
                alloc
                    .free(std::mem::take(&mut self.indirect_allocs[i]))
                    .ok();
                device.destroy_buffer(self.indirect_cutout_buffers[i], None);
                alloc
                    .free(std::mem::take(&mut self.indirect_cutout_allocs[i]))
                    .ok();
            }
        }

        let meta_size = (new_max * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (new_max * size_of::<DrawCommand>()) as u64;
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::VertexBuffer,
                "chunk_meta",
            );
            self.meta_buffers[i] = b;
            self.meta_allocs[i] = a;

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds",
            );
            self.indirect_buffers[i] = b;
            self.indirect_allocs[i] = a;

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
                "indirect_cmds_cutout",
            );
            self.indirect_cutout_buffers[i] = b;
            self.indirect_cutout_allocs[i] = a;

            let (meta_info, mut meta_write) = desc_write(
                self.compute_sets[i],
                0,
                vk::DescriptorType::StorageBuffer,
                self.meta_buffers[i],
                meta_size,
            );
            let (indirect_info, mut indirect_write) = desc_write(
                self.compute_sets[i],
                2,
                vk::DescriptorType::StorageBuffer,
                self.indirect_buffers[i],
                indirect_size,
            );
            let (indirect_c_info, mut indirect_c_write) = desc_write(
                self.compute_sets[i],
                4,
                vk::DescriptorType::StorageBuffer,
                self.indirect_cutout_buffers[i],
                indirect_size,
            );
            meta_write.buffer_info = meta_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            indirect_c_write.buffer_info = indirect_c_info.as_ptr();
            device.update_descriptor_sets(&[meta_write, indirect_write, indirect_c_write], &[]);
        }

        // The water command/candidate buffers scale with max_meta too.
        {
            let mut alloc = allocator.lock().unwrap();
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                device.destroy_buffer(self.water_indirect_buffers[i], None);
                device.destroy_buffer(self.water_candidate_buffers[i], None);
            }
            for allocation in self.water_indirect_allocs.drain(..) {
                alloc.free(allocation).ok();
            }
            for allocation in self.water_candidate_allocs.drain(..) {
                alloc.free(allocation).ok();
            }
        }
        (
            self.water_indirect_buffers,
            self.water_indirect_allocs,
            self.water_candidate_buffers,
            self.water_candidate_allocs,
        ) = create_water_scaled_buffers(device, allocator, new_max);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (water_ind_info, mut water_ind_write) = desc_write(
                self.compute_sets[i],
                9,
                vk::DescriptorType::StorageBuffer,
                self.water_indirect_buffers[i],
                indirect_size,
            );
            let (candidates_info, mut candidates_write) = desc_write(
                self.compute_sets[i],
                8,
                vk::DescriptorType::StorageBuffer,
                self.water_candidate_buffers[i],
                new_max as u64 * 4,
            );
            water_ind_write.buffer_info = water_ind_info.as_ptr();
            candidates_write.buffer_info = candidates_info.as_ptr();
            device.update_descriptor_sets(&[water_ind_write, candidates_write], &[]);
        }

        self.meta_free.grow(new_max as u32);
        self.meta_mirror
            .resize(new_max, bytemuck::Zeroable::zeroed());
        // Fresh buffers: repopulate every slot from the mirror and drop the
        // catch-up queue it superseded.
        let mirror_bytes: &[u8] = bytemuck::cast_slice(&self.meta_mirror);
        for a in &mut self.meta_allocs {
            a.mapped_slice_mut().unwrap()[..mirror_bytes.len()].copy_from_slice(mirror_bytes);
        }
        self.meta_writes.clear();
        self.meta_applied = [0; MAX_FRAMES_IN_FLIGHT];

        self.max_meta = new_max;
    }

    /// Pool alloc that, on exhaustion, reclaims retired slices early and
    /// retries once. A mass unload can retire tens of thousands of slices in
    /// a burst; reclaiming only when an alloc actually fails keeps the GPU
    /// wait off the common path (`begin_frame` returns them for free three
    /// frames later).
    fn alloc_vertices(&mut self, device: &vk::Device, count: u32) -> Option<u32> {
        if let Some(off) = self.vtx_free.alloc(count) {
            return Some(off);
        }
        self.reclaim_retired(device)
            .then(|| self.vtx_free.alloc(count))
            .flatten()
    }

    /// Allocate a stable meta slot, reclaiming retired slots and then growing
    /// the meta buffers (which can't fail short of OOM) as fallbacks.
    fn alloc_meta_slot(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) -> u32 {
        let slot = self
            .meta_free
            .alloc(1)
            .or_else(|| {
                self.reclaim_retired(device)
                    .then(|| self.meta_free.alloc(1))
                    .flatten()
            })
            .unwrap_or_else(|| {
                self.grow_meta(device, allocator);
                self.meta_free.alloc(1).expect("meta pool empty after grow")
            });
        self.meta_high_water = self.meta_high_water.max(slot + 1);
        slot
    }

    /// Record a meta entry into the CPU mirror and the catch-up queue; each
    /// frame slot's GPU buffer applies it in `dispatch_cull` after that slot's
    /// fence wait, so an in-flight cull never observes a partial write.
    fn queue_meta_write(&mut self, slot: u32, entry: ChunkMeta) {
        self.meta_mirror[slot as usize] = entry;
        self.meta_writes.push((slot, entry));
    }

    /// Apply queued meta writes this frame slot hasn't seen yet, trimming the
    /// queue once every slot has caught up. A backlog longer than half the
    /// capacity (e.g. a whole world staged on the loading screen, where no
    /// cull runs) applies as one mirror memcpy instead of a replay.
    fn apply_meta_writes(&mut self, frame: usize) {
        let buf = self.meta_allocs[frame].mapped_slice_mut().unwrap();
        let pending = &self.meta_writes[self.meta_applied[frame]..];
        if pending.len() > self.max_meta / 2 {
            let mirror_bytes: &[u8] = bytemuck::cast_slice(&self.meta_mirror);
            buf[..mirror_bytes.len()].copy_from_slice(mirror_bytes);
        } else {
            for &(slot, entry) in pending {
                let off = slot as usize * size_of::<ChunkMeta>();
                buf[off..off + size_of::<ChunkMeta>()].copy_from_slice(bytemuck::bytes_of(&entry));
            }
        }
        self.meta_applied[frame] = self.meta_writes.len();
        if self
            .meta_applied
            .iter()
            .all(|&a| a == self.meta_writes.len())
        {
            self.meta_writes.clear();
            self.meta_applied = [0; MAX_FRAMES_IN_FLIGHT];
        }
    }

    /// Emergency reclaim when a pool runs dry: waits the GPU out and returns
    /// every retired slice immediately instead of at its frame deadline.
    /// False when too little is pending to be worth the wait — with the pool
    /// exhausted and remeshes trickling in, an unconditional reclaim would
    /// `wait_idle` every frame for a handful of slices.
    fn reclaim_retired(&mut self, device: &vk::Device) -> bool {
        const MIN_RECLAIM_SLICES: usize = 64;
        if self.pending_free.len() < MIN_RECLAIM_SLICES {
            return false;
        }
        let start = std::time::Instant::now();
        device.wait_idle().ok();
        while let Some((_, slice)) = self.pending_free.pop_front() {
            self.free_slice(slice);
        }
        self.last_reclaim_ms += start.elapsed().as_secs_f32() * 1000.0;
        true
    }

    /// Return one slice's vertex range and meta slot to the pools.
    fn free_slice(&mut self, (vo, vl, slot): (u32, u32, u32)) {
        self.vtx_free.free_region(vo, vl);
        self.meta_free.free_region(slot, 1);
    }

    /// Remove the section at `si` from `col_pos` if present, zeroing its meta
    /// entry (freed slots self-cull) and retiring its GPU slices (tombstones
    /// own none). Returns whether an entry — real or tombstone — existed,
    /// which is exactly the fade question: only a never-seen section fades in
    /// (vanilla `wasPreviouslyEmpty` sections appear instantly).
    fn take_section(&mut self, col_pos: ChunkPos, si: i32) -> bool {
        let mut was_present = false;
        let mut freed = Vec::new();
        if let Some(entry) = self.chunks.get_mut(&col_pos) {
            entry.sections.retain(|s| {
                if s.section_index == si {
                    was_present = true;
                    if !s.is_tombstone() {
                        freed.push(slice_of(s));
                    }
                    false
                } else {
                    true
                }
            });
        }
        self.retire_freed(freed);
        was_present
    }

    /// Common teardown for replaced/unloaded sections: zero their meta
    /// entries (freed slots self-cull), drop any staged copy into their
    /// slices, and retire the slices.
    fn retire_freed(&mut self, freed: Vec<(u32, u32, u32)>) {
        for &(.., slot) in &freed {
            self.queue_meta_write(slot, bytemuck::Zeroable::zeroed());
        }
        self.drop_pending_copies_for(&freed);
        self.retire_slices(freed);
    }

    /// Defer returning slices to the pools until `MAX_FRAMES_IN_FLIGHT` frames
    /// have passed, so the GPU can't still be reading them from an in-flight
    /// frame. Use for slices that were potentially drawn (re-mesh replacement,
    /// chunk unload).
    fn retire_slices(&mut self, slices: impl IntoIterator<Item = (u32, u32, u32)>) {
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
        while self
            .pending_free
            .front()
            .is_some_and(|&(retire_at, _)| retire_at <= self.frame_seq)
        {
            let (_, slice) = self.pending_free.pop_front().unwrap();
            self.free_slice(slice);
        }
    }

    /// Count a submitted frame toward the retire deadlines. Called after a
    /// successful queue submit only: a skipped frame (swapchain out of date)
    /// re-waits the same fence, so counting it would shave a frame off the
    /// `MAX_FRAMES_IN_FLIGHT` margin `retire_slices` depends on.
    pub fn frame_submitted(&mut self) {
        self.frame_seq += 1;
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        if let Some(alloc) = self.chunks.remove(pos) {
            let freed = alloc
                .sections
                .iter()
                .filter(|s| !s.is_tombstone())
                .map(slice_of)
                .collect();
            self.retire_freed(freed);
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.vtx_free.reset();
        self.pending_free.clear();
        // Staged copies target pool offsets that just died with the pools.
        self.drop_pending_copies();
        // Dropping the high-water mark to 0 makes every stale GPU meta entry
        // unreachable; no buffer scrub needed.
        self.meta_free.reset();
        self.meta_high_water = 0;
        self.meta_mirror.fill(bytemuck::Zeroable::zeroed());
        self.meta_writes.clear();
        self.meta_applied = [0; MAX_FRAMES_IN_FLIGHT];
        self.fade_enabled = false;
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Wires the shared Hi-Z visibility mask buffer into every frame slot's
    /// cull descriptor set (binding 6). One-time: the mask buffer is never
    /// recreated.
    pub fn set_visibility_mask_buffer(&mut self, device: &vk::Device, buffer: vk::Buffer) {
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (info, mut write) = desc_write(
                self.compute_sets[i],
                6,
                vk::DescriptorType::StorageBuffer,
                buffer,
                (crate::util::CHUNK_RING_SIZE * 4) as u64,
            );
            write.buffer_info = info.as_ptr();
            device.update_descriptor_sets(&[write], &[]);
        }
    }

    /// `anchor` must be the same `Camera::anchor()` this frame's
    /// `CameraUniform` was built with, so the cull's block/fraction split
    /// matches the vertex shader's. `mask` is the Hi-Z decode parameters
    /// `(center, min_section, section_count)` of the frame slot's visibility
    /// mask, applied GPU-side in cull.comp; `None` fails open.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_cull(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        player_chunk: ChunkPos,
        limit_rd: Option<u32>,
        mask: Option<(ChunkPos, i32, i32)>,
    ) {
        if self.meta_high_water == 0 {
            return;
        }
        // Catch this frame slot's persistent meta buffer up with the entries
        // written since it last ran (its fence was waited at frame start, so
        // no in-flight cull reads it). This is the only per-frame CPU cost and
        // it scales with *changed* sections, never with loaded ones.
        self.apply_meta_writes(frame);
        let count = self.meta_high_water;

        let (mask_center, mask_min_section, mask_section_count) =
            mask.unwrap_or((ChunkPos::new(0, 0), 0, 0));
        let frustum_data = FrustumData {
            planes: *frustum,
            chunk_count: count,
            cam_block: anchor.as_ivec3().to_array(),
            frac: (eye - anchor).as_vec3().to_array(),
            mask_center: [mask_center.x, mask_center.z],
            mask_min_section,
            mask_section_count,
            mask_valid: mask.is_some() as u32,
            player_chunk: [player_chunk.x, player_chunk.z],
            limit_rd: limit_rd.unwrap_or(0),
            _pad: 0,
        };
        let frustum_bytes = bytemuck::bytes_of(&frustum_data);
        self.frustum_allocs[frame].mapped_slice_mut().unwrap()[..frustum_bytes.len()]
            .copy_from_slice(frustum_bytes);

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so the count buffers still hold their previous cull result; capture the
        // total (solid + cutout draws) for the debug overlay before clearing them.
        {
            let read_and_clear = |a: &mut Allocation| {
                let s = a.mapped_slice_mut().unwrap();
                let n = u32::from_ne_bytes([s[0], s[1], s[2], s[3]]);
                s[..4].copy_from_slice(&0u32.to_ne_bytes());
                n
            };
            self.last_draw_count = read_and_clear(&mut self.count_allocs[frame])
                + read_and_clear(&mut self.count_cutout_allocs[frame]);
        }

        // macOS draws the whole indirect buffer (no drawIndirectCount), so slots
        // the cull shader leaves unfilled must read as no-op draws, not stale data.
        #[cfg(target_os = "macos")]
        {
            // Only slots below the high water are ever drawn (max_draws
            // bounds the zero-fill).
            let live = self.meta_high_water as usize * size_of::<DrawCommand>();
            for a in [
                &mut self.indirect_allocs[frame],
                &mut self.indirect_cutout_allocs[frame],
                &mut self.water_indirect_allocs[frame],
            ] {
                a.mapped_slice_mut().unwrap()[..live].fill(0);
            }
        }

        // Clear the water bucket counters (+ candidate counter) before the
        // cull accumulates into them; the slot's previous use is fence-waited.
        cmd.fill_buffer(
            self.water_bucket_buffers[frame],
            0,
            (WATER_BUCKETS as u64 + 1) * 4,
            0,
        );
        let fill_barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead | vk::AccessFlags::ShaderWrite,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[fill_barrier],
            &[],
            &[],
        );

        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            self.compute_layout,
            0,
            &[self.compute_sets[frame]],
            &[],
        );
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.compute_pipeline);
        cmd.dispatch(count.div_ceil(64), 1, 1);

        // Cull → scan → emit: each pass reads what the previous wrote.
        let compute_barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead | vk::AccessFlags::ShaderWrite,
            ..Default::default()
        };
        let compute_to_compute = |cmd: vk::CommandBuffer| {
            cmd.pipeline_barrier(
                vk::PipelineStageFlags::ComputeShader,
                vk::PipelineStageFlags::ComputeShader,
                vk::DependencyFlags::empty(),
                &[compute_barrier],
                &[],
                &[],
            );
        };
        compute_to_compute(cmd);
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.water_scan_pipeline);
        cmd.dispatch(1, 1, 1);
        compute_to_compute(cmd);
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.water_emit_pipeline);
        cmd.dispatch(count.div_ceil(64), 1, 1);

        let barrier = vk::MemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::IndirectCommandRead,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::DrawIndirect,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        );

        if !self.fade_enabled {
            self.fade_enabled = true;
        }
    }

    /// drawCount/maxDrawCount for the indirect draws: `meta_high_water`
    /// clamped to the device limit, with a one-shot warning once truncation
    /// starts (draws past the cap are silently skipped by the spec).
    fn clamped_draw_count(&mut self) -> u32 {
        if self.meta_high_water > self.max_draw_indirect_count && !self.warned_draw_cap {
            self.warned_draw_cap = true;
            tracing::warn!(
                "live section draws ({}) exceed maxDrawIndirectCount ({}); \
                 excess draws are dropped",
                self.meta_high_water,
                self.max_draw_indirect_count
            );
        }
        self.meta_high_water.min(self.max_draw_indirect_count)
    }

    /// Issue one render layer's indirect draws. `cutout` selects the discard
    /// pass's draw list (drawn after `solid`, which lays down depth); the
    /// caller binds the matching pipeline first. Both layers share the
    /// vertex/index/meta buffers and the cull-written draw lists.
    pub fn draw_indirect(&mut self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        if self.meta_high_water == 0 {
            return;
        }

        let max_draws = self.clamped_draw_count();
        let (indirect, count) = if cutout {
            (
                self.indirect_cutout_buffers[frame],
                self.count_cutout_buffers[frame],
            )
        } else {
            (self.indirect_buffers[frame], self.count_buffers[frame])
        };

        // Binding 0: packed vertex pool. Binding 1: the meta buffer, read per
        // instance for the section origin + fade (indexed by `first_instance`).
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer, self.meta_buffers[frame]], &[0, 0]);
        cmd.bind_index_buffer(self.quad_index_buffer, 0, vk::IndexType::Uint32);
        if cfg!(target_os = "macos") {
            cmd.draw_indexed_indirect(indirect, 0, max_draws, size_of::<DrawCommand>() as u32);
        } else {
            cmd.draw_indexed_indirect_count(
                indirect,
                0,
                count,
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        }
    }

    /// Draw the translucent water list the cull/scan/emit chain produced this
    /// frame: GPU frustum + Hi-Z culled and ordered back-to-front by distance
    /// bucket. Shares the vertex pool, quad index buffer, and per-instance
    /// meta with the opaque passes; the caller binds the blended water
    /// pipeline first.
    pub fn draw_water(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        if self.meta_high_water == 0 {
            return;
        }

        let max_draws = self.clamped_draw_count();
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer, self.meta_buffers[frame]], &[0, 0]);
        cmd.bind_index_buffer(self.quad_index_buffer, 0, vk::IndexType::Uint32);
        if cfg!(target_os = "macos") {
            cmd.draw_indexed_indirect(
                self.water_indirect_buffers[frame],
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        } else {
            cmd.draw_indexed_indirect_count(
                self.water_indirect_buffers[frame],
                0,
                self.water_count_buffers[frame],
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.vertex_buffer, None);

        alloc.free(std::mem::take(&mut self.vertex_alloc)).ok();
        if self.quad_index_quads > 0 {
            device.destroy_buffer(self.quad_index_buffer, None);
            alloc.free(std::mem::take(&mut self.quad_index_alloc)).ok();
        }
        if let Some((buf, allocation)) = self.quad_index_src.take() {
            device.destroy_buffer(buf, None);
            alloc.free(allocation).ok();
        }

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.meta_buffers[i], None);
            device.destroy_buffer(self.indirect_buffers[i], None);
            device.destroy_buffer(self.count_buffers[i], None);
            device.destroy_buffer(self.indirect_cutout_buffers[i], None);
            device.destroy_buffer(self.count_cutout_buffers[i], None);
            device.destroy_buffer(self.water_indirect_buffers[i], None);
            device.destroy_buffer(self.water_count_buffers[i], None);
            device.destroy_buffer(self.water_bucket_buffers[i], None);
            device.destroy_buffer(self.water_candidate_buffers[i], None);
            device.destroy_buffer(self.frustum_buffers[i], None);

            alloc.free(std::mem::take(&mut self.meta_allocs[i])).ok();
            alloc
                .free(std::mem::take(&mut self.indirect_allocs[i]))
                .ok();
            alloc.free(std::mem::take(&mut self.count_allocs[i])).ok();
            alloc
                .free(std::mem::take(&mut self.indirect_cutout_allocs[i]))
                .ok();
            alloc
                .free(std::mem::take(&mut self.count_cutout_allocs[i]))
                .ok();
            alloc.free(std::mem::take(&mut self.frustum_allocs[i])).ok();
        }
        for allocation in self
            .water_indirect_allocs
            .drain(..)
            .chain(self.water_count_allocs.drain(..))
            .chain(self.water_bucket_allocs.drain(..))
            .chain(self.water_candidate_allocs.drain(..))
        {
            alloc.free(allocation).ok();
        }
        for buffer in self.staging_buffers.drain(..) {
            device.destroy_buffer(buffer, None);
        }
        for allocation in self.staging_allocs.drain(..) {
            alloc.free(allocation).ok();
        }
        drop(alloc);

        device.destroy_pipeline(self.compute_pipeline, None);
        device.destroy_pipeline(self.water_scan_pipeline, None);
        device.destroy_pipeline(self.water_emit_pipeline, None);
        device.destroy_pipeline_layout(self.compute_layout, None);
        device.destroy_descriptor_pool(self.compute_pool, None);
        device.destroy_descriptor_set_layout(self.compute_desc_layout, None);
    }
}

fn create_compute_pipeline(
    device: &vk::Device,
    layout: vk::PipelineLayout,
    spirv: &[u8],
    spec_info: Option<&vk::SpecializationInfo>,
) -> vk::Pipeline {
    let module = shader::create_shader_module(device, spirv);
    let stage = vk::PipelineShaderStageCreateInfo {
        stage: vk::ShaderStageFlags::Compute,
        module,
        name: c"main".as_ptr(),
        specialization_info: spec_info.map_or(std::ptr::null(), |s| s as *const _),
        ..Default::default()
    };
    let pipe_info = [vk::ComputePipelineCreateInfo {
        stage,
        layout,
        ..Default::default()
    }];
    let mut pipeline = vk::Pipeline::null();
    device
        .create_compute_pipelines(
            vk::PipelineCache::null(),
            &pipe_info,
            None,
            std::slice::from_mut(&mut pipeline),
        )
        .expect("failed to create compute pipeline");
    device.destroy_shader_module(module, None);
    pipeline
}

/// The per-frame water buffers that scale with `max_meta`: the indirect
/// command list (host-visible for the macOS zero-fill) and the candidate list
/// (device-local GPU scratch). Created at init and recreated by `grow_meta`.
fn create_water_scaled_buffers(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    max_meta: usize,
) -> (
    Vec<vk::Buffer>,
    Vec<Allocation>,
    Vec<vk::Buffer>,
    Vec<Allocation>,
) {
    let indirect_size = (max_meta * size_of::<DrawCommand>()) as u64;
    let mut indirect_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    let mut indirect_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    let mut candidate_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    let mut candidate_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
    for _ in 0..MAX_FRAMES_IN_FLIGHT {
        let (b, a) = util::create_host_buffer(
            device,
            allocator,
            indirect_size,
            vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::IndirectBuffer,
            "water_indirect",
        );
        indirect_buffers.push(b);
        indirect_allocs.push(a);

        let (b, a) = util::create_device_buffer(
            device,
            allocator,
            max_meta as u64 * 4,
            vk::BufferUsageFlags::StorageBuffer,
            "water_candidates",
        );
        candidate_buffers.push(b);
        candidate_allocs.push(a);
    }
    (
        indirect_buffers,
        indirect_allocs,
        candidate_buffers,
        candidate_allocs,
    )
}

fn create_cull_desc_layout(device: &vk::Device) -> vk::DescriptorSetLayout {
    // Binding 1 is the frustum UBO; the rest are storage: meta, solid
    // indirect/count, cutout indirect/count, Hi-Z visibility mask, water
    // buckets, candidates, indirect, count.
    let bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..=10)
        .map(|binding| vk::DescriptorSetLayoutBinding {
            binding,
            descriptor_type: if binding == 1 {
                vk::DescriptorType::UniformBuffer
            } else {
                vk::DescriptorType::StorageBuffer
            },
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        })
        .collect();
    let info = vk::DescriptorSetLayoutCreateInfo {
        binding_count: bindings.len() as u32,
        bindings: bindings.as_ptr(),
        ..Default::default()
    };
    device
        .create_descriptor_set_layout(&info, None)
        .expect("failed to create cull desc layout")
}

fn desc_write(
    set: vk::DescriptorSet,
    binding: u32,
    ty: vk::DescriptorType,
    buffer: vk::Buffer,
    range: u64,
) -> (
    [vk::DescriptorBufferInfo; 1],
    vk::WriteDescriptorSet<'static>,
) {
    let info = [vk::DescriptorBufferInfo {
        buffer,
        offset: 0,
        range,
    }];

    let write = vk::WriteDescriptorSet {
        dst_set: set,
        dst_binding: binding,
        descriptor_count: 1,
        descriptor_type: ty,
        ..Default::default()
    };

    (info, write)
}
