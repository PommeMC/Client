use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use azalea_core::position::ChunkPos;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use super::mesher::{ChunkMeshData, ChunkVertex};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

const BUCKET_VERTICES: u32 = 32768;
const BUCKET_INDICES: u32 = 49152;
const VERTEX_SIZE: u64 = size_of::<ChunkVertex>() as u64;
const INDEX_SIZE: u64 = size_of::<u32>() as u64;
const BYTES_PER_BUCKET: u64 =
    BUCKET_VERTICES as u64 * VERTEX_SIZE + BUCKET_INDICES as u64 * INDEX_SIZE;
const MIN_BUCKETS: u32 = 128;
const MAX_BUCKETS: u32 = 2048;
const VRAM_BUDGET_FRACTION: f64 = 0.25;
/// Spare pool fraction the hidden-column backfill leaves free, so visible/near
/// columns can always upload (see `GameState::rescan_mesh_jobs`).
const BACKFILL_HEADROOM_FRACTION: f32 = 0.1;

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

    fn free_fraction(&self) -> f32 {
        let free: u32 = self.free.iter().map(|&(_, l)| l).sum();
        free as f32 / self.capacity.max(1) as f32
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
pub struct ChunkAABB {
    pub min: [f32; 4],
    pub max: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ChunkMeta {
    aabb_min: [f32; 4],
    aabb_max: [f32; 4],
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    visibility: u32,
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

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FrustumData {
    planes: [[f32; 4]; 6],
    chunk_count: u32,
    camera_pos: [f32; 3],
    // std140: camera_pos ends at offset 112 (16-aligned), so the mat4 packs with
    // no extra padding and the Rust/GLSL layouts match.
    view_proj: [[f32; 4]; 4],
    screen_size: [f32; 2],
    occlusion_enabled: u32,
    _pad: u32,
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
    first_index: u32,
    index_count: u32,
    vertex_offset: i32,
    vtx_len: u32,
    uploaded_at: std::time::Instant,
}

struct ChunkAlloc {
    sections: Vec<SectionAlloc>,
}

pub struct ChunkBufferStore {
    /// Capacity (in draws) of the per-frame meta/indirect buffers. Grown on
    /// demand because per-section packing yields many more draws than buckets.
    max_meta: usize,
    vertex_buffer: vk::Buffer,
    vertex_alloc: Allocation,
    index_buffer: vk::Buffer,
    index_alloc: Allocation,
    staging_buffer: vk::Buffer,
    staging_alloc: Allocation,
    staging_size: u64,
    transfer_pool: vk::CommandPool,
    transfer_cmd: vk::CommandBuffer,
    use_staging: bool,

    /// Exact-size sub-allocators over the vertex and index pools (in elements).
    vtx_free: FreeList,
    idx_free: FreeList,
    chunks: HashMap<ChunkPos, ChunkAlloc>,
    cached_meta: Vec<ChunkMeta>,
    meta_dirty: bool,

    compute_pipeline: vk::Pipeline,
    compute_layout: vk::PipelineLayout,
    compute_desc_layout: vk::DescriptorSetLayout,
    compute_pool: vk::DescriptorPool,
    compute_sets: Vec<vk::DescriptorSet>,

    meta_buffers: Vec<vk::Buffer>,
    meta_allocs: Vec<Allocation>,
    indirect_buffers: Vec<vk::Buffer>,
    indirect_allocs: Vec<Allocation>,
    count_buffers: Vec<vk::Buffer>,
    count_allocs: Vec<Allocation>,
    frustum_buffers: Vec<vk::Buffer>,
    frustum_allocs: Vec<Allocation>,
    fade_enabled: bool,
    /// Post-cull section draw count read back from the GPU (lags a few frames);
    /// exposed for the debug overlay so occlusion's effect is visible.
    last_draw_count: u32,
}

impl ChunkBufferStore {
    pub fn new(
        device: &vk::Device,
        physical_device: vk::PhysicalDevice,
        graphics_family: u32,
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

        let staging_size = BYTES_PER_BUCKET * 4;
        let (staging_buffer, staging_alloc) = util::create_host_buffer(
            device,
            allocator,
            staging_size,
            vk::BufferUsageFlags::TransferSrc,
            "staging",
        );

        let pool_info = vk::CommandPoolCreateInfo {
            queue_family_index: graphics_family,
            flags: vk::CommandPoolCreateFlags::Transient
                | vk::CommandPoolCreateFlags::ResetCommandBuffer,
            ..Default::default()
        };
        let transfer_pool = device
            .create_command_pool(&pool_info, None)
            .expect("failed to create transfer pool");
        let cmd_info = vk::CommandBufferAllocateInfo {
            command_pool: transfer_pool,
            level: vk::CommandBufferLevel::Primary,
            command_buffer_count: 1,
            ..Default::default()
        };
        let mut transfer_cmd = vk::CommandBuffer::null();
        unsafe {
            device.allocate_command_buffers(&cmd_info, std::slice::from_mut(&mut transfer_cmd))
        }
        .expect("failed to alloc transfer cmd");

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

        let max_meta = (total_buckets * 2) as usize;
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
        let mut frustum_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut frustum_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer,
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
        let stage = vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module: comp_module,
            name: c"main".as_ptr(),
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
                descriptor_count: 3 * MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
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

            meta_write.buffer_info = meta_info.as_ptr();
            frustum_write.buffer_info = frustum_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            count_write.buffer_info = count_info.as_ptr();

            let writes = [meta_write, frustum_write, indirect_write, count_write];

            device.update_descriptor_sets(&writes, &[]);
        }

        Self {
            max_meta,
            vertex_buffer,
            vertex_alloc,
            index_buffer,
            index_alloc,
            staging_buffer,
            staging_alloc,
            staging_size,
            transfer_pool,
            transfer_cmd,
            use_staging,
            vtx_free,
            idx_free,
            chunks: HashMap::new(),
            cached_meta: Vec::new(),
            meta_dirty: true,
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
            frustum_buffers,
            frustum_allocs,
            fade_enabled: false,
            last_draw_count: 0,
        }
    }

    /// Sections drawn last time this frame slot ran (post frustum + occlusion
    /// cull). Read back from the GPU count buffer, so it lags a few frames.
    pub fn sections_drawn(&self) -> u32 {
        self.last_draw_count
    }

    pub fn upload(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        mesh: &ChunkMeshData,
    ) {
        // Tight AABB over a section's own vertices (better cull granularity than
        // the chunk-column bounds; also robust to LOD cubes that exceed 16 tall).
        fn section_aabb(verts: &[ChunkVertex]) -> ChunkAABB {
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

        // Free the slices of every section index this job covers (`replaced`): the
        // re-meshed ones are re-allocated below, the now-empty ones simply vanish.
        // Remember which were present so a re-meshed section swaps instantly while
        // a freshly revealed one still fades in.
        let mut freed: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut was_present: std::collections::HashSet<i32> = std::collections::HashSet::new();
        if let Some(entry) = self.chunks.get_mut(&mesh.pos) {
            entry.sections.retain(|s| {
                if mesh.replaced.contains(&s.section_index) {
                    was_present.insert(s.section_index);
                    freed.push((
                        s.vertex_offset as u32,
                        s.vtx_len,
                        s.first_index,
                        s.index_count,
                    ));
                    false
                } else {
                    true
                }
            });
        }
        self.free_slices(&freed);

        if mesh.sections.is_empty() {
            // Every replaced section is now empty (freed above); drop the column
            // if nothing remains.
            if self
                .chunks
                .get(&mesh.pos)
                .is_some_and(|c| c.sections.is_empty())
            {
                self.chunks.remove(&mesh.pos);
            }
            self.meta_dirty = true;
            return;
        }

        let staging_half = self.staging_size as usize / 2;
        if self.use_staging {
            // Verts and indices share the staging buffer (two halves), copied in
            // one transfer. A chunk too large for staging is skipped rather than
            // overflowing the buffer (matches the prior column-sized limit).
            let v_bytes: usize = mesh
                .sections
                .iter()
                .map(|s| s.vertices.len() * VERTEX_SIZE as usize)
                .sum();
            let i_bytes: usize = mesh
                .sections
                .iter()
                .map(|s| s.indices.len() * INDEX_SIZE as usize)
                .sum();
            if v_bytes > staging_half || i_bytes > staging_half {
                tracing::warn!(
                    "Chunk {:?} too large for staging ({} v / {} i bytes), skipping",
                    mesh.pos,
                    v_bytes,
                    i_bytes,
                );
                return;
            }
        }

        // Sub-allocate an exact-size vertex + index slice for each non-empty
        // section. Indices stay section-local and `vertex_offset` rebases the draw,
        // so no packing or rebasing is needed — just one slice per section.
        struct Plan<'a> {
            section_index: i32,
            verts: &'a [ChunkVertex],
            indices: &'a [u32],
            vtx_off: u32,
            idx_off: u32,
            aabb: ChunkAABB,
        }

        let mut plans: Vec<Plan> = Vec::with_capacity(mesh.sections.len());
        // (vtx_off, vtx_len, idx_off, idx_len) taken this call, for rollback if the
        // pool runs out partway through a column.
        let mut taken: Vec<(u32, u32, u32, u32)> = Vec::new();
        for sec in &mesh.sections {
            let vcount = sec.vertices.len() as u32;
            let icount = sec.indices.len() as u32;
            if vcount == 0 || icount == 0 {
                continue;
            }
            let Some(vtx_off) = self.vtx_free.alloc(vcount) else {
                self.free_slices(&taken);
                tracing::debug!("Vertex pool full, skipping {:?}", mesh.pos);
                return;
            };
            let Some(idx_off) = self.idx_free.alloc(icount) else {
                self.vtx_free.free_region(vtx_off, vcount);
                self.free_slices(&taken);
                tracing::debug!("Index pool full, skipping {:?}", mesh.pos);
                return;
            };
            taken.push((vtx_off, vcount, idx_off, icount));
            plans.push(Plan {
                section_index: sec.section_index,
                verts: &sec.vertices,
                indices: &sec.indices,
                vtx_off,
                idx_off,
                aabb: section_aabb(&sec.vertices),
            });
        }

        if plans.is_empty() {
            return;
        }

        if self.use_staging {
            let mut copy_v: Vec<vk::BufferCopy> = Vec::new();
            let mut copy_i: Vec<vk::BufferCopy> = Vec::new();
            {
                let buf = self.staging_alloc.mapped_slice_mut().unwrap();
                let mut stg_v = 0usize;
                let mut stg_i = 0usize;
                for p in &plans {
                    let vb: &[u8] = bytemuck::cast_slice(p.verts);
                    buf[stg_v..stg_v + vb.len()].copy_from_slice(vb);
                    copy_v.push(vk::BufferCopy {
                        src_offset: stg_v as u64,
                        dst_offset: p.vtx_off as u64 * VERTEX_SIZE,
                        size: vb.len() as u64,
                    });
                    stg_v += vb.len();

                    let ib: &[u8] = bytemuck::cast_slice(p.indices);
                    let off = staging_half + stg_i;
                    buf[off..off + ib.len()].copy_from_slice(ib);
                    copy_i.push(vk::BufferCopy {
                        src_offset: off as u64,
                        dst_offset: p.idx_off as u64 * INDEX_SIZE,
                        size: ib.len() as u64,
                    });
                    stg_i += ib.len();
                }
            }

            let begin = vk::CommandBufferBeginInfo {
                flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
                ..Default::default()
            };
            self.transfer_cmd.begin(&begin).unwrap();
            self.transfer_cmd
                .copy_buffer(self.staging_buffer, self.vertex_buffer, &copy_v);
            self.transfer_cmd
                .copy_buffer(self.staging_buffer, self.index_buffer, &copy_i);
            self.transfer_cmd.end().unwrap();
            let submit = [vk::SubmitInfo {
                command_buffer_count: 1,
                command_buffers: &self.transfer_cmd.handle(),
                ..Default::default()
            }];
            queue.submit(&submit, vk::Fence::null()).unwrap();
            queue.wait_idle().unwrap();
        } else {
            {
                let vbuf = self.vertex_alloc.mapped_slice_mut().unwrap();
                for p in &plans {
                    let vb: &[u8] = bytemuck::cast_slice(p.verts);
                    let off = p.vtx_off as usize * VERTEX_SIZE as usize;
                    vbuf[off..off + vb.len()].copy_from_slice(vb);
                }
            }
            {
                let ibuf = self.index_alloc.mapped_slice_mut().unwrap();
                for p in &plans {
                    let ib: &[u8] = bytemuck::cast_slice(p.indices);
                    let off = p.idx_off as usize * INDEX_SIZE as usize;
                    ibuf[off..off + ib.len()].copy_from_slice(ib);
                }
            }
        }

        let now = std::time::Instant::now();
        let new_sections = plans.iter().map(|p| SectionAlloc {
            section_index: p.section_index,
            aabb: p.aabb,
            first_index: p.idx_off,
            index_count: p.indices.len() as u32,
            vertex_offset: p.vtx_off as i32,
            vtx_len: p.verts.len() as u32,
            // A re-meshed section swaps instantly; a freshly revealed one fades in.
            uploaded_at: if was_present.contains(&p.section_index) {
                now.checked_sub(std::time::Duration::from_secs(2))
                    .unwrap_or(now)
            } else {
                now
            },
        });

        self.chunks
            .entry(mesh.pos)
            .or_insert_with(|| ChunkAlloc {
                sections: Vec::new(),
            })
            .sections
            .extend(new_sections);
        self.meta_dirty = true;

        let total_sections: usize = self.chunks.values().map(|c| c.sections.len()).sum();
        self.ensure_meta_capacity(device, allocator, total_sections);
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
            }
        }

        let meta_size = (new_max * size_of::<ChunkMeta>()) as u64;
        let indirect_size = (new_max * size_of::<DrawCommand>()) as u64;
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                meta_size,
                vk::BufferUsageFlags::StorageBuffer,
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
            meta_write.buffer_info = meta_info.as_ptr();
            indirect_write.buffer_info = indirect_info.as_ptr();
            device.update_descriptor_sets(&[meta_write, indirect_write], &[]);
        }

        self.max_meta = new_max;
    }

    /// Return a batch of `(vtx_off, vtx_len, idx_off, idx_len)` slices to the
    /// pools.
    fn free_slices(&mut self, slices: &[(u32, u32, u32, u32)]) {
        for &(vo, vl, io, il) in slices {
            self.vtx_free.free_region(vo, vl);
            self.idx_free.free_region(io, il);
        }
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        if let Some(alloc) = self.chunks.remove(pos) {
            for sec in &alloc.sections {
                self.vtx_free
                    .free_region(sec.vertex_offset as u32, sec.vtx_len);
                self.idx_free.free_region(sec.first_index, sec.index_count);
            }
            self.meta_dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.vtx_free.reset();
        self.idx_free.reset();
        self.cached_meta.clear();
        self.meta_dirty = true;
        self.fade_enabled = false;
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Whether the pools have spare capacity for discretionary hidden-column
    /// backfill (both the vertex and index pools must keep a margin free).
    pub fn has_bucket_headroom(&self) -> bool {
        self.vtx_free.free_fraction() > BACKFILL_HEADROOM_FRACTION
            && self.idx_free.free_fraction() > BACKFILL_HEADROOM_FRACTION
    }

    /// Point each frame's cull descriptor at the Hi-Z pyramid (binding 4).
    /// Called once after the pyramid is created and again whenever it is
    /// recreated (e.g. on resize), so the cull shader's occlusion test
    /// reads a live view.
    pub fn set_hiz(&self, device: &vk::Device, hiz_view: vk::ImageView, hiz_sampler: vk::Sampler) {
        for &set in &self.compute_sets {
            let image_info = [vk::DescriptorImageInfo {
                sampler: hiz_sampler,
                image_view: hiz_view,
                image_layout: vk::ImageLayout::General,
            }];
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 4,
                descriptor_count: 1,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                image_info: image_info.as_ptr(),
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_cull(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        camera_pos: [f32; 3],
        view_proj: [[f32; 4]; 4],
        screen_size: [f32; 2],
        occlusion_enabled: bool,
    ) {
        if self.chunks.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        const FADE_DURATION_MS: f32 = 1000.0;
        const NEARBY_DIST_SQ: f32 = 768.0;

        let any_fading = self.fade_enabled
            && self.chunks.values().flat_map(|a| &a.sections).any(|s| {
                now.duration_since(s.uploaded_at).as_secs_f32() * 1000.0 < FADE_DURATION_MS
            });

        if self.meta_dirty || any_fading {
            self.cached_meta.clear();
            for (pos, alloc) in self.chunks.iter() {
                // Near columns never fade; otherwise each section fades on its own
                // timer (X/Z distance is per-column).
                let dx = pos.x as f32 * 16.0 + 8.0 - camera_pos[0];
                let dz = pos.z as f32 * 16.0 + 8.0 - camera_pos[2];
                let nearby = !self.fade_enabled || dx * dx + dz * dz < NEARBY_DIST_SQ;

                for sec in &alloc.sections {
                    let vis = if nearby {
                        1.0
                    } else {
                        let elapsed_ms = now.duration_since(sec.uploaded_at).as_secs_f32() * 1000.0;
                        (elapsed_ms / FADE_DURATION_MS).min(1.0)
                    };
                    self.cached_meta.push(ChunkMeta {
                        aabb_min: sec.aabb.min,
                        aabb_max: sec.aabb.max,
                        index_count: sec.index_count,
                        first_index: sec.first_index,
                        vertex_offset: sec.vertex_offset,
                        visibility: vis.to_bits(),
                    });
                }
            }
            self.meta_dirty = false;
        }

        self.cached_meta.sort_unstable_by(|a, b| {
            let center_a = [
                (a.aabb_min[0] + a.aabb_max[0]) * 0.5 - camera_pos[0],
                (a.aabb_min[1] + a.aabb_max[1]) * 0.5 - camera_pos[1],
                (a.aabb_min[2] + a.aabb_max[2]) * 0.5 - camera_pos[2],
            ];
            let center_b = [
                (b.aabb_min[0] + b.aabb_max[0]) * 0.5 - camera_pos[0],
                (b.aabb_min[1] + b.aabb_max[1]) * 0.5 - camera_pos[1],
                (b.aabb_min[2] + b.aabb_max[2]) * 0.5 - camera_pos[2],
            ];
            let dist_a =
                center_a[0] * center_a[0] + center_a[1] * center_a[1] + center_a[2] * center_a[2];
            let dist_b =
                center_b[0] * center_b[0] + center_b[1] * center_b[1] + center_b[2] * center_b[2];
            dist_a
                .partial_cmp(&dist_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let count = self.cached_meta.len() as u32;
        let meta_bytes = bytemuck::cast_slice(&self.cached_meta);
        self.meta_allocs[frame].mapped_slice_mut().unwrap()[..meta_bytes.len()]
            .copy_from_slice(meta_bytes);

        let frustum_data = FrustumData {
            planes: *frustum,
            chunk_count: count,
            camera_pos,
            view_proj,
            screen_size,
            occlusion_enabled: occlusion_enabled as u32,
            _pad: 0,
        };
        let frustum_bytes = bytemuck::bytes_of(&frustum_data);
        self.frustum_allocs[frame].mapped_slice_mut().unwrap()[..frustum_bytes.len()]
            .copy_from_slice(frustum_bytes);

        // This frame slot's GPU work has completed (fence-waited at frame start),
        // so the count buffer still holds its previous cull result; capture it for
        // the debug overlay before clearing it for this dispatch.
        {
            let s = self.count_allocs[frame].mapped_slice_mut().unwrap();
            self.last_draw_count = u32::from_ne_bytes([s[0], s[1], s[2], s[3]]);
        }
        self.count_allocs[frame].mapped_slice_mut().unwrap()[..4]
            .copy_from_slice(&0u32.to_ne_bytes());

        // macOS draws the whole indirect buffer (no drawIndirectCount), so slots
        // the cull shader leaves unfilled must read as no-op draws, not stale data.
        #[cfg(target_os = "macos")]
        self.indirect_allocs[frame]
            .mapped_slice_mut()
            .unwrap()
            .fill(0);

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

    pub fn draw_indirect(&self, cmd: vk::CommandBuffer, frame: usize) {
        if self.chunks.is_empty() {
            return;
        }

        let max_draws = self
            .chunks
            .values()
            .map(|c| c.sections.len() as u32)
            .sum::<u32>();

        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.bind_index_buffer(self.index_buffer, 0, vk::IndexType::Uint32);
        if cfg!(target_os = "macos") {
            cmd.draw_indexed_indirect(
                self.indirect_buffers[frame],
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
        } else {
            cmd.draw_indexed_indirect_count(
                self.indirect_buffers[frame],
                0,
                self.count_buffers[frame],
                0,
                max_draws,
                size_of::<DrawCommand>() as u32,
            );
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
                .free(std::mem::replace(&mut self.frustum_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        device.destroy_buffer(self.staging_buffer, None);
        alloc
            .free(std::mem::replace(&mut self.staging_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        drop(alloc);

        device.destroy_command_pool(self.transfer_pool, None);
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
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
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
