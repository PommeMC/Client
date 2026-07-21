use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use azalea_core::position::{ChunkPos, ChunkSectionPos};
use glam::DVec3;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::dispatcher::pack_section_pos;
use super::mesher::{ChunkAABB, PackedVertex, SectionMeshData};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::util::{ChunkRing, section_bit};

const BUCKET_VERTICES: u32 = 32768;
const BUCKET_INDICES: u32 = 49152;
const VERTEX_SIZE: u64 = size_of::<PackedVertex>() as u64;
const INDEX_SIZE: u64 = size_of::<u32>() as u64;
const BYTES_PER_BUCKET: u64 =
    BUCKET_VERTICES as u64 * VERTEX_SIZE + BUCKET_INDICES as u64 * INDEX_SIZE;
const MIN_BUCKETS: u32 = 128;
// A full-world reload (dimension change, render-distance toggle) transiently
// holds both worlds: unloaded slices stay allocated until their frame deadline
// while the new world uploads. The cap leaves room for that 2x so the reload
// never hits the emergency GPU-wait reclaim; `VRAM_BUDGET_FRACTION` still
// bounds smaller cards.
const MAX_BUCKETS: u32 = 4096;
const VRAM_BUDGET_FRACTION: f64 = 0.25;
/// Per-section fade-in length, shared by the opaque indirect path and water.
const FADE_DURATION_MS: f32 = 1000.0;
/// Columns within this squared X/Z distance of the camera render opaque
/// immediately and never fade in.
const NEARBY_DIST_SQ: f32 = 768.0;

/// Whether a column's center is within the always-near X/Z radius of the eye
/// (vanilla `isNearby`), rebased in f64 for precision at extreme coordinates.
/// Also suppresses the section fade-in for near columns (`column_nearby`).
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

fn compute_bucket_count(physical_device: vk::PhysicalDevice) -> u32 {
    let mem_props = physical_device.get_memory_properties();
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
struct ChunkMeta {
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

/// Copy already-packed `verts` into `dst` starting at byte `off`.
fn write_verts(dst: &mut [u8], off: usize, verts: &[PackedVertex]) {
    let bytes: &[u8] = bytemuck::cast_slice(verts);
    dst[off..off + bytes.len()].copy_from_slice(bytes);
}

/// Copy a section's opaque indices immediately followed by its water indices
/// into `dst` starting at byte `off`, returning the total bytes written.
fn write_indices(dst: &mut [u8], off: usize, opaque: &[u32], water: &[u32]) -> usize {
    let opaque: &[u8] = bytemuck::cast_slice(opaque);
    let water: &[u8] = bytemuck::cast_slice(water);
    dst[off..off + opaque.len()].copy_from_slice(opaque);
    dst[off + opaque.len()..off + opaque.len() + water.len()].copy_from_slice(water);
    opaque.len() + water.len()
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
    let vis_off = std::mem::offset_of!(ChunkMeta, visibility) as u32;
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
        // binding 1 — per-instance meta (origin + fade)
        vk::VertexInputAttributeDescription {
            location: 4,
            binding: 1,
            format: vk::Format::R32G32B32Sint,
            offset: origin_off,
        },
        vk::VertexInputAttributeDescription {
            location: 5,
            binding: 1,
            format: vk::Format::R32Sfloat,
            offset: vis_off,
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
    /// mask was written with (three frames ago). `mask_valid = 0` skips the
    /// mask test entirely (slot never written).
    mask_center: [i32; 2],
    mask_min_section: i32,
    mask_section_count: i32,
    mask_valid: u32,
}

/// One uploaded 16³ section: a self-contained indexed draw plus its tight AABB.
/// `first_index`/`index_count` are the section's index slice and
/// `vertex_offset` its vertex slice base; `vtx_len` is the slice length, kept
/// so the slices can be returned to the free-lists on removal. `uploaded_at`
/// drives the per-section fade so editing one section never re-fades the rest
/// of the column.
struct SectionAlloc {
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

struct ChunkAlloc {
    sections: Vec<SectionAlloc>,
}

/// The `(vertex_offset, vtx_len, first_index, idx_len)` pool slice a section
/// occupies, in the shape [`ChunkBufferStore::retire_slices`] consumes.
fn slice_of(s: &SectionAlloc) -> (u32, u32, u32, u32) {
    (s.vertex_offset as u32, s.vtx_len, s.first_index, s.idx_len)
}

/// One accepted section's payload, waiting for `record_copies` to write it
/// into the frame's staging slab (its pool slices are already reserved).
struct PendingCopy {
    vertices: Vec<PackedVertex>,
    indices: Vec<u32>,
    water_indices: Vec<u32>,
    vtx_off: u32,
    idx_off: u32,
}

pub struct ChunkBufferStore {
    /// Capacity (in draws) of the per-frame meta/indirect buffers. Grown on
    /// demand because per-section packing yields many more draws than buckets.
    max_meta: usize,
    vertex_buffer: vk::Buffer,
    vertex_alloc: Allocation,
    index_buffer: vk::Buffer,
    index_alloc: Allocation,
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
    /// Bytes `pending_copies` will occupy in each staging half, so a staging
    /// pass that carried over (skipped frame) still bounds the next batch.
    pending_v_bytes: usize,
    pending_i_bytes: usize,

    /// Exact-size sub-allocators over the vertex and index pools (in elements).
    vtx_free: FreeList,
    idx_free: FreeList,
    chunks: HashMap<ChunkPos, ChunkAlloc>,
    /// Time spent in `reclaim_retired`'s GPU wait during the last
    /// `stage_mesh_batch`, for the benchmark's upload breakdown.
    pub last_reclaim_ms: f32,
    cached_meta: Vec<ChunkMeta>,
    meta_dirty: bool,
    /// End of the current fade-in window. While `now < fade_until` the
    /// per-section fade values change each frame, so `cached_meta` must be
    /// rebuilt; an O(1) check replacing the old all-sections scan.
    fade_until: std::time::Instant,
    /// Eye position at the last front-to-back sort; the sort (an early-Z
    /// optimization) is only redone once the camera moves past a threshold.
    last_sort_cam: DVec3,
    /// Frame slots still needing the latest `cached_meta` uploaded. Set to
    /// `MAX_FRAMES_IN_FLIGHT` whenever the draw list changes, decremented per
    /// frame; at steady state the per-frame meta copy stops.
    meta_upload_pending: u32,

    compute_pipeline: vk::Pipeline,
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
    frustum_buffers: Vec<vk::Buffer>,
    frustum_allocs: Vec<Allocation>,
    fade_enabled: bool,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,
    /// CPU cost of the last cull's meta rebuild + sort, for the chunk-load
    /// bench's frame breakdown.
    last_meta_rebuild_ms: f32,

    /// Monotonic frame counter, bumped once per rendered frame in
    /// `begin_frame`.
    frame_seq: u64,
    /// Slices freed by a re-mesh or unload, each tagged with the `frame_seq` at
    /// which it's safe to reclaim (`MAX_FRAMES_IN_FLIGHT` out, so no in-flight
    /// frame still draws it). Drained in `begin_frame`.
    pending_free: VecDeque<(u64, (u32, u32, u32, u32))>,
    /// Last player column / render distance the draw list was rebuilt for; a
    /// change re-marks the meta dirty so the `limit_rd` column cull re-runs.
    last_player_chunk: ChunkPos,
    last_limit_rd: Option<u32>,
}

impl ChunkBufferStore {
    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> Self {
        let total_buckets = compute_bucket_count(physical_device);
        let vertex_size = total_buckets as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;
        let index_size = total_buckets as u64 * BUCKET_INDICES as u64 * INDEX_SIZE;

        let dev_props = physical_device.get_properties();
        let use_staging = dev_props.device_type == vk::PhysicalDeviceType::DiscreteGpu;

        let (vertex_buffer, vertex_alloc, index_buffer, index_alloc) = if use_staging {
            let (vb, va) = util::create_device_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            );
            let (ib, ia) = util::create_device_buffer(
                device,
                allocator,
                index_size,
                vk::BufferUsageFlags::IndexBuffer,
                "index_pool",
            );
            (vb, va, ib, ia)
        } else {
            let (vb, va) = util::create_host_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VertexBuffer,
                "vertex_pool",
            );
            let (ib, ia) = util::create_host_buffer(
                device,
                allocator,
                index_size,
                vk::BufferUsageFlags::IndexBuffer,
                "index_pool",
            );
            (vb, va, ib, ia)
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
        }

        let compute_desc_layout = create_cull_desc_layout(device);
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &compute_desc_layout,
            ..Default::default()
        };
        let compute_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create compute pipeline layout");

        let comp_spv = shader::include_spirv!("cull.comp.spv");
        let comp_module = shader::create_shader_module(device, comp_spv);
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
        let stage = vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module: comp_module,
            name: c"main".as_ptr(),
            specialization_info: &spec_info,
            ..Default::default()
        };
        let pipe_info = [vk::ComputePipelineCreateInfo {
            stage,
            layout: compute_layout,
            ..Default::default()
        }];
        let mut compute_pipeline = vk::Pipeline::null();
        device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &pipe_info,
                None,
                std::slice::from_mut(&mut compute_pipeline),
            )
            .expect("failed to create cull pipeline");
        device.destroy_shader_module(comp_module, None);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::StorageBuffer,
                // meta + solid indirect/count + cutout indirect/count +
                // visibility mask = 6 per frame.
                descriptor_count: 6 * MAX_FRAMES_IN_FLIGHT as u32,
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

            meta_write.buffer_info = meta_info.as_ptr();
            frustum_write.buffer_info = frustum_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            count_write.buffer_info = count_info.as_ptr();
            indirect_c_write.buffer_info = indirect_c_info.as_ptr();
            count_c_write.buffer_info = count_c_info.as_ptr();

            let writes = [
                meta_write,
                frustum_write,
                indirect_write,
                count_write,
                indirect_c_write,
                count_c_write,
            ];

            device.update_descriptor_sets(&writes, &[]);
        }

        Self {
            max_meta,
            vertex_buffer,
            vertex_alloc,
            index_buffer,
            index_alloc,
            staging_buffers,
            staging_allocs,
            staging_size,
            use_staging,
            pending_copies: Vec::new(),
            pending_v_bytes: 0,
            pending_i_bytes: 0,
            vtx_free,
            idx_free,
            chunks: HashMap::new(),
            last_reclaim_ms: 0.0,
            cached_meta: Vec::new(),
            meta_dirty: true,
            fade_until: std::time::Instant::now(),
            last_sort_cam: DVec3::MAX,
            meta_upload_pending: 0,
            compute_pipeline,
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
            frustum_buffers,
            frustum_allocs,
            fade_enabled: false,
            last_draw_count: 0,
            last_meta_rebuild_ms: 0.0,
            frame_seq: 0,
            pending_free: VecDeque::new(),
            last_player_chunk: ChunkPos::new(0, 0),
            last_limit_rd: None,
        }
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    pub fn meta_rebuild_ms(&self) -> f32 {
        self.last_meta_rebuild_ms
    }

    /// Whether `pos`'s column is near enough to the eye to render opaque
    /// immediately (a nearby column never fades in).
    fn column_nearby(&self, pos: ChunkPos, eye: DVec3) -> bool {
        !self.fade_enabled || column_is_near(pos, eye)
    }

    /// Write the staged sections into this frame's staging slab and record
    /// their pool copies into the frame command buffer, with a barrier so the
    /// frame's vertex/index reads see them. Runs after the frame fence wait,
    /// so rewriting the slab can't race an in-flight transfer.
    pub fn record_copies(&mut self, cmd: vk::CommandBuffer, frame: usize) {
        if self.pending_copies.is_empty() {
            return;
        }
        let staging_half = self.staging_size as usize / 2;
        let mut copy_v: Vec<vk::BufferCopy> = Vec::with_capacity(self.pending_copies.len());
        let mut copy_i: Vec<vk::BufferCopy> = Vec::with_capacity(self.pending_copies.len());
        let mut stg_v = 0usize;
        let mut stg_i = staging_half;
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

                let ibytes = write_indices(buf, stg_i, &pending.indices, &pending.water_indices);
                copy_i.push(vk::BufferCopy {
                    src_offset: stg_i as u64,
                    dst_offset: pending.idx_off as u64 * INDEX_SIZE,
                    size: ibytes as u64,
                });
                stg_i += ibytes;
            }
        }
        cmd.copy_buffer(self.staging_buffers[frame], self.vertex_buffer, &copy_v);
        cmd.copy_buffer(self.staging_buffers[frame], self.index_buffer, &copy_i);
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
        self.pending_i_bytes = 0;
    }

    /// Drain `mesh_queue` into the GPU pools, newest-epoch-per-section wins.
    /// Each accepted section replaces its slot's slices; an empty mesh retires
    /// the slot and drops the column when it goes empty. CPU-only: the staging
    /// path defers its byte writes and copies to `record_copies` in the frame
    /// command buffer instead of blocking on a transfer fence. If the staging
    /// budget or a pool fills, the loop stops and leaves the rest queued for
    /// next frame. Returns the accepted `(section pos, epoch)` list.
    pub fn stage_mesh_batch(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        mesh_queue: &mut VecDeque<SectionMeshData>,
    ) -> Vec<(ChunkSectionPos, u64)> {
        self.last_reclaim_ms = 0.0;
        // Keep only the newest result per section before draining: the stale
        // check below reads `self.chunks`, which only reflects this batch's
        // uploads after the loop, so two same-section results in one drain would
        // otherwise both be accepted and the section drawn twice.
        // (Keyed by packed pos: azalea's ChunkSectionPos doesn't impl Hash.)
        let mut best: HashMap<u64, u64> = HashMap::new();
        for mesh in mesh_queue.iter() {
            let key = pack_section_pos(mesh.spos);
            let epoch = best.entry(key).or_insert(mesh.upload_epoch);
            *epoch = (*epoch).max(mesh.upload_epoch);
        }
        if best.len() < mesh_queue.len() {
            let mut seen = HashSet::new();
            mesh_queue.retain(|m| {
                let key = pack_section_pos(m.spos);
                m.upload_epoch == best[&key] && seen.insert(key)
            });
        }
        if mesh_queue.is_empty() {
            return Vec::new();
        }

        let mut uploaded_info: Vec<(ChunkSectionPos, u64)> = Vec::new();
        let staging_half = self.staging_size as usize / 2;

        struct BatchEntry {
            mesh: SectionMeshData,
            col_pos: ChunkPos,
            si: i32,
            was_present: bool,
            vtx_off: u32,
            idx_off: u32,
            vcount: u32,
            icount: u32,
        }
        let mut entries: Vec<BatchEntry> = Vec::new();

        // Include copies carried over from a skipped frame in the budget.
        let mut current_v_bytes = self.pending_v_bytes;
        let mut current_i_bytes = self.pending_i_bytes;
        while let Some(mesh) = mesh_queue.front() {
            let col_pos = ChunkPos::new(mesh.spos.x, mesh.spos.z);
            let si = mesh.relative_si;
            let stored = self
                .chunks
                .get(&col_pos)
                .and_then(|c| c.sections.iter().find(|s| s.section_index == si))
                .map(|s| s.epoch)
                .unwrap_or(0);
            // Reject a stale upload a newer edit already superseded.
            if mesh.upload_epoch < stored {
                mesh_queue.pop_front();
                continue;
            }
            if mesh.is_empty() {
                self.take_section(col_pos, si);
                if self
                    .chunks
                    .get(&col_pos)
                    .is_some_and(|c| c.sections.is_empty())
                {
                    self.chunks.remove(&col_pos);
                }
                mesh_queue.pop_front();
                continue;
            }
            let vcount = mesh.vertices.len() as u32;
            // Opaque and water indices share one slice (opaque first, water after).
            let icount = (mesh.indices.len() + mesh.water_indices.len()) as u32;
            if self.use_staging {
                let v_bytes = vcount as usize * VERTEX_SIZE as usize;
                let i_bytes = icount as usize * INDEX_SIZE as usize;
                // A section too large for one staging half is skipped, not overflowed.
                if v_bytes > staging_half || i_bytes > staging_half {
                    tracing::warn!(
                        "Section {:?} too large for staging ({} v / {} i bytes), skipping",
                        mesh.spos,
                        v_bytes,
                        i_bytes,
                    );
                    mesh_queue.pop_front();
                    continue;
                }
                // This transfer's staging budget is full; leave the rest queued.
                if current_v_bytes + v_bytes > staging_half
                    || current_i_bytes + i_bytes > staging_half
                {
                    break;
                }
                current_v_bytes += v_bytes;
                current_i_bytes += i_bytes;
            }
            let Some(vtx_off) = self.alloc_vertices(device, vcount) else {
                tracing::debug!(
                    "Vertex pool full, stopping upload batch for {:?}",
                    mesh.spos
                );
                break;
            };
            let Some(idx_off) = self.alloc_indices(device, icount) else {
                self.vtx_free.free_region(vtx_off, vcount);
                tracing::debug!("Index pool full, stopping upload batch for {:?}", mesh.spos);
                break;
            };
            let mesh = mesh_queue.pop_front().unwrap();
            let was_present = self.take_section(col_pos, si);
            uploaded_info.push((mesh.spos, mesh.upload_epoch));
            entries.push(BatchEntry {
                mesh,
                col_pos,
                si,
                was_present,
                vtx_off,
                idx_off,
                vcount,
                icount,
            });
        }

        if entries.is_empty() {
            return uploaded_info;
        }

        let now = std::time::Instant::now();
        // Freshly revealed sections fade in, so extend the fade window the cull's
        // O(1) check reads; re-meshed-only uploads swap instantly.
        if entries.iter().any(|e| !e.was_present) {
            let dur = std::time::Duration::from_secs_f32(FADE_DURATION_MS / 1000.0);
            self.fade_until = self.fade_until.max(now + dur);
        }
        for entry in &entries {
            let spos = entry.mesh.spos;
            let sec_alloc = SectionAlloc {
                section_index: entry.si,
                aabb: entry.mesh.aabb,
                origin: [spos.x * 16, spos.y * 16, spos.z * 16],
                first_index: entry.idx_off,
                index_count: entry.mesh.indices.len() as u32,
                solid_index_count: entry.mesh.solid_index_count,
                water_first_index: entry.idx_off + entry.mesh.indices.len() as u32,
                water_index_count: entry.mesh.water_indices.len() as u32,
                idx_len: entry.icount,
                vertex_offset: entry.vtx_off as i32,
                vtx_len: entry.vcount,
                // A re-meshed section swaps instantly; a freshly revealed one fades in.
                uploaded_at: if entry.was_present {
                    now.checked_sub(std::time::Duration::from_secs(2))
                        .unwrap_or(now)
                } else {
                    now
                },
                epoch: entry.mesh.upload_epoch,
            };
            self.chunks
                .entry(entry.col_pos)
                .or_insert_with(|| ChunkAlloc {
                    sections: Vec::new(),
                })
                .sections
                .push(sec_alloc);
        }

        if self.use_staging {
            for entry in &mut entries {
                self.pending_v_bytes += entry.mesh.vertices.len() * VERTEX_SIZE as usize;
                self.pending_i_bytes += (entry.mesh.indices.len() + entry.mesh.water_indices.len())
                    * INDEX_SIZE as usize;
                self.pending_copies.push(PendingCopy {
                    vertices: std::mem::take(&mut entry.mesh.vertices),
                    indices: std::mem::take(&mut entry.mesh.indices),
                    water_indices: std::mem::take(&mut entry.mesh.water_indices),
                    vtx_off: entry.vtx_off,
                    idx_off: entry.idx_off,
                });
            }
        } else {
            {
                let vbuf = self.vertex_alloc.mapped_slice_mut().unwrap();
                for entry in &entries {
                    let base = entry.vtx_off as usize * VERTEX_SIZE as usize;
                    write_verts(vbuf, base, &entry.mesh.vertices);
                }
            }
            {
                let ibuf = self.index_alloc.mapped_slice_mut().unwrap();
                for entry in &entries {
                    let off = entry.idx_off as usize * INDEX_SIZE as usize;
                    write_indices(ibuf, off, &entry.mesh.indices, &entry.mesh.water_indices);
                }
            }
        }

        let total_sections: usize = self.chunks.values().map(|c| c.sections.len()).sum();
        self.ensure_meta_capacity(device, allocator, total_sections);

        uploaded_info
    }

    /// Grow the per-frame meta and indirect buffers so they can hold `needed`
    /// section draws. No-op while capacity suffices.
    fn ensure_meta_capacity(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        needed: usize,
    ) {
        if needed <= self.max_meta {
            return;
        }
        let new_max = (needed.saturating_mul(3) / 2)
            .next_power_of_two()
            .max(self.max_meta * 2);

        // The meta/indirect buffers are referenced by every in-flight frame's
        // descriptor set; wait the GPU out before freeing them.
        device.wait_idle().ok();

        {
            let mut alloc = allocator.lock().unwrap();
            for i in 0..MAX_FRAMES_IN_FLIGHT {
                device.destroy_buffer(self.meta_buffers[i], None);
                alloc
                    .free(std::mem::replace(&mut self.meta_allocs[i], unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                device.destroy_buffer(self.indirect_buffers[i], None);
                alloc
                    .free(std::mem::replace(&mut self.indirect_allocs[i], unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
                device.destroy_buffer(self.indirect_cutout_buffers[i], None);
                alloc
                    .free(std::mem::replace(
                        &mut self.indirect_cutout_allocs[i],
                        unsafe { std::mem::zeroed() },
                    ))
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

    fn alloc_indices(&mut self, device: &vk::Device, count: u32) -> Option<u32> {
        if let Some(off) = self.idx_free.alloc(count) {
            return Some(off);
        }
        self.reclaim_retired(device)
            .then(|| self.idx_free.alloc(count))
            .flatten()
    }

    /// Emergency reclaim when a pool runs dry: waits the GPU out and returns
    /// every retired slice immediately instead of at its frame deadline.
    /// False when there was nothing to reclaim.
    fn reclaim_retired(&mut self, device: &vk::Device) -> bool {
        if self.pending_free.is_empty() {
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

    /// Return one slice's vertex and index ranges to the pools.
    fn free_slice(&mut self, (vo, vl, io, il): (u32, u32, u32, u32)) {
        self.vtx_free.free_region(vo, vl);
        self.idx_free.free_region(io, il);
    }

    /// Remove the section at `si` from `col_pos` if present, retiring its GPU
    /// slices and marking the meta dirty. Returns whether the section existed.
    fn take_section(&mut self, col_pos: ChunkPos, si: i32) -> bool {
        let mut freed = Vec::new();
        if let Some(entry) = self.chunks.get_mut(&col_pos) {
            entry.sections.retain(|s| {
                if s.section_index == si {
                    freed.push(slice_of(s));
                    false
                } else {
                    true
                }
            });
        }
        let was_present = !freed.is_empty();
        self.retire_slices(freed);
        self.meta_dirty = true;
        was_present
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
            self.retire_slices(alloc.sections.iter().map(slice_of));
            self.meta_dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.vtx_free.reset();
        self.idx_free.reset();
        self.pending_free.clear();
        // Staged copies target pool offsets that just died with the pools.
        self.drop_pending_copies();
        self.cached_meta.clear();
        self.meta_dirty = true;
        self.fade_enabled = false;
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Wires each frame slot's Hi-Z visibility mask buffer into the cull
    /// descriptor set (binding 6). One-time: the mask buffers are never
    /// recreated.
    pub fn set_visibility_mask_buffers(
        &mut self,
        device: &vk::Device,
        buffers: &[vk::Buffer; MAX_FRAMES_IN_FLIGHT],
    ) {
        for (i, &buffer) in buffers.iter().enumerate() {
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
    /// matches the vertex shader's; `eye` drives the front-to-back sort and
    /// near checks. `mask` is the Hi-Z decode parameters `(center,
    /// min_section, section_count)` of the frame slot's visibility mask,
    /// applied GPU-side in cull.comp; `None` fails open.
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
        if self.chunks.is_empty() {
            return;
        }
        // A change in player column or render distance re-runs the `limit_rd`
        // column cull below.
        if player_chunk != self.last_player_chunk || limit_rd != self.last_limit_rd {
            self.last_player_chunk = player_chunk;
            self.last_limit_rd = limit_rd;
            self.meta_dirty = true;
        }

        let now = std::time::Instant::now();
        // Re-sort only once the camera moves ~8 blocks; front-to-back order is an
        // early-Z optimization, so finer staleness is harmless.
        const SORT_RECAM_SQ: f64 = 64.0;

        // A fade in flight changes per-section visibility every frame, so the draw
        // list must rebuild; otherwise it only changes on edits/loads/visibility
        // (`meta_dirty`). The fade check is O(1) against `fade_until`.
        let any_fading = self.fade_enabled && now < self.fade_until;
        let t_rebuild = std::time::Instant::now();
        let content_changed = self.meta_dirty || any_fading;

        if content_changed {
            self.cached_meta.clear();
            for (pos, alloc) in self.chunks.iter() {
                // Columns beyond the render distance never draw.
                if let Some(rd) = limit_rd {
                    let dx = (pos.x - player_chunk.x).abs();
                    let dz = (pos.z - player_chunk.z).abs();
                    if dx.max(dz) > rd as i32 {
                        continue;
                    }
                }
                // Near columns never fade; otherwise each section fades on its own
                // timer (X/Z distance is per-column).
                let nearby = self.column_nearby(*pos, eye);

                for sec in &alloc.sections {
                    let vis = Self::section_visibility(nearby, sec, now);
                    self.cached_meta.push(ChunkMeta {
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
            self.meta_dirty = false;
        }

        let cam_moved = (eye - self.last_sort_cam).length_squared() > SORT_RECAM_SQ;
        if content_changed || cam_moved {
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
            self.cached_meta.sort_unstable_by(|a, b| {
                center_dist_sq(a)
                    .partial_cmp(&center_dist_sq(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.last_sort_cam = eye;
            // Draw list reordered: every frame slot's meta buffer needs the refresh.
            self.meta_upload_pending = MAX_FRAMES_IN_FLIGHT as u32;
        }
        self.last_meta_rebuild_ms = t_rebuild.elapsed().as_secs_f32() * 1000.0;

        let count = self.cached_meta.len() as u32;
        // Each frame slot has its own meta buffer; copy only into slots that
        // haven't yet seen the current draw list. Steady state stops copying.
        if self.meta_upload_pending > 0 {
            let meta_bytes = bytemuck::cast_slice(&self.cached_meta);
            self.meta_allocs[frame].mapped_slice_mut().unwrap()[..meta_bytes.len()]
                .copy_from_slice(meta_bytes);
            self.meta_upload_pending -= 1;
        }

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
        for a in [
            &mut self.indirect_allocs[frame],
            &mut self.indirect_cutout_allocs[frame],
        ] {
            a.mapped_slice_mut().unwrap().fill(0);
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.compute_pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            self.compute_layout,
            0,
            &[self.compute_sets[frame]],
            &[],
        );
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

    /// Issue one render layer's indirect draws. `cutout` selects the discard
    /// pass's draw list (drawn after `solid`, which lays down depth); the
    /// caller binds the matching pipeline first. Both layers share the
    /// vertex/index/meta buffers and the cull-written draw lists.
    pub fn draw_indirect(&self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
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
                self.indirect_cutout_buffers[frame],
                self.count_cutout_buffers[frame],
            )
        } else {
            (self.indirect_buffers[frame], self.count_buffers[frame])
        };

        // Binding 0: packed vertex pool. Binding 1: the meta buffer, read per
        // instance for the section origin + fade (indexed by `first_instance`).
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer, self.meta_buffers[frame]], &[0, 0]);
        cmd.bind_index_buffer(self.index_buffer, 0, vk::IndexType::Uint32);
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
    /// `CameraUniform` was built with: the push-constant origins are
    /// rebased against it and the shader adds back the eye's fractional
    /// offset.
    ///
    /// TODO: water isn't depth-sorted, so overlapping translucent surfaces
    /// (oceans at grazing angles, water seen through water) can blend out of
    /// order.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_water(
        &self,
        cmd: vk::CommandBuffer,
        layout: vk::PipelineLayout,
        frustum: &[[f32; 4]; 6],
        anchor: DVec3,
        eye: DVec3,
        vis_mask: &ChunkRing<u32>,
        visibility_center: ChunkPos,
    ) {
        if self.chunks.is_empty() {
            return;
        }

        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.bind_index_buffer(self.index_buffer, 0, vk::IndexType::Uint32);

        let now = std::time::Instant::now();
        for (pos, alloc) in self.chunks.iter() {
            // Fail open outside the visibility ring's range.
            let col_vis = vis_mask
                .get_in_range(*pos, visibility_center)
                .copied()
                .unwrap_or(u32::MAX);
            let nearby = self.column_nearby(*pos, eye);
            for sec in &alloc.sections {
                if sec.water_index_count == 0
                    || col_vis & section_bit(sec.section_index as u32) == 0
                    || !aabb_in_frustum(&sec.aabb, sec.origin, frustum, eye)
                {
                    continue;
                }
                let vis = Self::section_visibility(nearby, sec, now);
                let rel = (origin_dvec(sec.origin) - anchor).as_vec3();
                let origin_fade = [rel.x, rel.y, rel.z, vis];
                cmd.push_constants(
                    layout,
                    vk::ShaderStageFlags::Vertex,
                    0,
                    bytemuck::bytes_of(&origin_fade),
                );
                cmd.draw_indexed(
                    sec.water_index_count,
                    1,
                    sec.water_first_index,
                    sec.vertex_offset,
                    0,
                );
            }
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.vertex_buffer, None);
        device.destroy_buffer(self.index_buffer, None);

        alloc
            .free(std::mem::replace(&mut self.vertex_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        alloc
            .free(std::mem::replace(&mut self.index_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.meta_buffers[i], None);
            device.destroy_buffer(self.indirect_buffers[i], None);
            device.destroy_buffer(self.count_buffers[i], None);
            device.destroy_buffer(self.indirect_cutout_buffers[i], None);
            device.destroy_buffer(self.count_cutout_buffers[i], None);
            device.destroy_buffer(self.frustum_buffers[i], None);

            alloc
                .free(std::mem::replace(&mut self.meta_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(&mut self.indirect_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(&mut self.count_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.indirect_cutout_allocs[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.count_cutout_allocs[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
            alloc
                .free(std::mem::replace(&mut self.frustum_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        for buffer in self.staging_buffers.drain(..) {
            device.destroy_buffer(buffer, None);
        }
        for allocation in self.staging_allocs.drain(..) {
            alloc.free(allocation).ok();
        }
        drop(alloc);

        device.destroy_pipeline(self.compute_pipeline, None);
        device.destroy_pipeline_layout(self.compute_layout, None);
        device.destroy_descriptor_pool(self.compute_pool, None);
        device.destroy_descriptor_set_layout(self.compute_desc_layout, None);
    }
}

fn create_cull_desc_layout(device: &vk::Device) -> vk::DescriptorSetLayout {
    let bindings = [
        vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 1,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 2,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 3,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 4,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 5,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 6,
            descriptor_type: vk::DescriptorType::StorageBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        },
    ];
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
