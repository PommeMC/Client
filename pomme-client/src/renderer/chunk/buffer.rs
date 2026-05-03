use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use ash::vk;
use azalea_core::position::ChunkPos;
use gpu_allocator::vulkan::{Allocation, Allocator};

use super::mesher::{ChunkMeshData, ChunkVertex};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::shader;
use crate::renderer::util;

const BUCKET_VERTICES: u32 = 32768;
const BUCKET_INDICES: u32 = 49152;
const VERTEX_SIZE: u64 = std::mem::size_of::<ChunkVertex>() as u64;
const INDEX_SIZE: u64 = std::mem::size_of::<u32>() as u64;
const BYTES_PER_BUCKET: u64 =
    BUCKET_VERTICES as u64 * VERTEX_SIZE + BUCKET_INDICES as u64 * INDEX_SIZE;
const MIN_BUCKETS: u32 = 128;
const MAX_BUCKETS: u32 = 2048;
const VRAM_BUDGET_FRACTION: f64 = 0.25;

fn compute_bucket_count(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> u32 {
    let mem_props = unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let mut device_local_bytes: u64 = 0;
    for i in 0..mem_props.memory_type_count as usize {
        let mem_type = mem_props.memory_types[i];
        if mem_type
            .property_flags
            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
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
}

struct ChunkAlloc {
    buckets: Vec<u32>,
    index_counts: Vec<u32>,
    aabb: ChunkAABB,
    uploaded_at: std::time::Instant,
}

pub struct ChunkBufferStore {
    total_buckets: u32,
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

    free_buckets: VecDeque<u32>,
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
}

impl ChunkBufferStore {
    pub fn new(
        device: &ash::Device,
        instance: &ash::Instance,
        physical_device: vk::PhysicalDevice,
        graphics_family: u32,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> Self {
        let total_buckets = compute_bucket_count(instance, physical_device);
        let vertex_size = total_buckets as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;
        let index_size = total_buckets as u64 * BUCKET_INDICES as u64 * INDEX_SIZE;

        let dev_props = unsafe { instance.get_physical_device_properties(physical_device) };
        let use_staging = dev_props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;

        let (vertex_buffer, vertex_alloc, index_buffer, index_alloc) = if use_staging {
            let (vb, va) = util::create_device_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "vertex_pool",
            );
            let (ib, ia) = util::create_device_buffer(
                device,
                allocator,
                index_size,
                vk::BufferUsageFlags::INDEX_BUFFER,
                "index_pool",
            );
            (vb, va, ib, ia)
        } else {
            let (vb, va) = util::create_host_buffer(
                device,
                allocator,
                vertex_size,
                vk::BufferUsageFlags::VERTEX_BUFFER,
                "vertex_pool",
            );
            let (ib, ia) = util::create_host_buffer(
                device,
                allocator,
                index_size,
                vk::BufferUsageFlags::INDEX_BUFFER,
                "index_pool",
            );
            (vb, va, ib, ia)
        };

        let staging_size = BYTES_PER_BUCKET * 4;
        let (staging_buffer, staging_alloc) = util::create_host_buffer(
            device,
            allocator,
            staging_size,
            vk::BufferUsageFlags::TRANSFER_SRC,
            "staging",
        );

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(graphics_family)
            .flags(
                vk::CommandPoolCreateFlags::TRANSIENT
                    | vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER,
            );
        let transfer_pool = unsafe { device.create_command_pool(&pool_info, None) }
            .expect("failed to create transfer pool");
        let cmd_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(transfer_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let transfer_cmd = unsafe { device.allocate_command_buffers(&cmd_info) }
            .expect("failed to alloc transfer cmd")[0];

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

        let mut free_buckets = VecDeque::with_capacity(total_buckets as usize);
        for i in 0..total_buckets {
            free_buckets.push_back(i);
        }

        let max_meta = (total_buckets * 2) as u64;
        let meta_size = max_meta * std::mem::size_of::<ChunkMeta>() as u64;
        let indirect_size = max_meta * std::mem::size_of::<DrawCommand>() as u64;
        let count_size = 4u64;
        let frustum_size = std::mem::size_of::<FrustumData>() as u64;

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
                vk::BufferUsageFlags::STORAGE_BUFFER,
                "chunk_meta",
            );
            meta_buffers.push(b);
            meta_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                indirect_size,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::INDIRECT_BUFFER,
                "indirect_cmds",
            );
            indirect_buffers.push(b);
            indirect_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                count_size,
                vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::INDIRECT_BUFFER,
                "draw_count",
            );
            count_buffers.push(b);
            count_allocs.push(a);

            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                frustum_size,
                vk::BufferUsageFlags::UNIFORM_BUFFER,
                "frustum_ubo",
            );
            frustum_buffers.push(b);
            frustum_allocs.push(a);
        }

        let compute_desc_layout = create_cull_desc_layout(device);
        let set_layouts = [compute_desc_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts);
        let compute_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create compute pipeline layout");

        let comp_spv = shader::include_spirv!("cull.comp.spv");
        let comp_module = shader::create_shader_module(device, comp_spv);
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(comp_module)
            .name(c"main");
        let pipe_info = [vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(compute_layout)];
        let compute_pipeline =
            unsafe { device.create_compute_pipelines(vk::PipelineCache::null(), &pipe_info, None) }
                .expect("failed to create cull pipeline")[0];
        unsafe { device.destroy_shader_module(comp_module, None) };

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::STORAGE_BUFFER,
                descriptor_count: 3 * MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_FRAMES_IN_FLIGHT as u32)
            .pool_sizes(&pool_sizes);
        let compute_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create cull desc pool");

        let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| compute_desc_layout)
            .collect();
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(compute_pool)
            .set_layouts(&layouts);
        let compute_sets = unsafe { device.allocate_descriptor_sets(&alloc_info) }
            .expect("failed to allocate cull desc sets");

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let writes = [
                desc_write(
                    compute_sets[i],
                    0,
                    vk::DescriptorType::STORAGE_BUFFER,
                    meta_buffers[i],
                    meta_size,
                ),
                desc_write(
                    compute_sets[i],
                    1,
                    vk::DescriptorType::UNIFORM_BUFFER,
                    frustum_buffers[i],
                    frustum_size,
                ),
                desc_write(
                    compute_sets[i],
                    2,
                    vk::DescriptorType::STORAGE_BUFFER,
                    indirect_buffers[i],
                    indirect_size,
                ),
                desc_write(
                    compute_sets[i],
                    3,
                    vk::DescriptorType::STORAGE_BUFFER,
                    count_buffers[i],
                    count_size,
                ),
            ];
            unsafe { device.update_descriptor_sets(&writes, &[]) };
        }

        Self {
            total_buckets,
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
            free_buckets,
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
        }
    }

    pub fn upload(&mut self, device: &ash::Device, queue: vk::Queue, mesh: &ChunkMeshData) {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            self.remove(&mesh.pos);
            return;
        }

        self.remove(&mesh.pos);

        let num_buckets = mesh.vertices.len().div_ceil(BUCKET_VERTICES as usize) as u32;
        if self.free_buckets.len() < num_buckets as usize {
            tracing::warn!(
                "Bucket pool full ({} free, need {}), skipping {:?}",
                self.free_buckets.len(),
                num_buckets,
                mesh.pos,
            );
            return;
        }

        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for v in &mesh.vertices {
            min_y = min_y.min(v.position[1]);
            max_y = max_y.max(v.position[1]);
        }
        let cx = mesh.pos.x as f32 * 16.0;
        let cz = mesh.pos.z as f32 * 16.0;
        let aabb = ChunkAABB {
            min: [cx, min_y, cz, 0.0],
            max: [cx + 16.0, max_y, cz + 16.0, 0.0],
        };

        let mut bucket_ids = Vec::with_capacity(num_buckets as usize);
        let mut index_counts = Vec::with_capacity(num_buckets as usize);
        let mut copy_regions_v: Vec<vk::BufferCopy> = Vec::new();
        let mut copy_regions_i: Vec<vk::BufferCopy> = Vec::new();

        let write_buf = if self.use_staging {
            self.staging_alloc.mapped_slice_mut().unwrap()
        } else {
            self.vertex_alloc.mapped_slice_mut().unwrap()
        };
        let staging_half = self.staging_size as usize / 2;

        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vert_cursor = 0usize;
        let mut idx_cursor = 0usize;
        let mut stg_v_cursor = 0usize;
        let mut stg_i_cursor = 0usize;

        for _ in 0..num_buckets {
            let bucket = self.free_buckets.pop_front().unwrap();
            let vert_end = (vert_cursor + BUCKET_VERTICES as usize).min(verts.len());

            let vb_offset = bucket as usize * BUCKET_VERTICES as usize * VERTEX_SIZE as usize;
            let src = bytemuck::cast_slice(&verts[vert_cursor..vert_end]);

            if self.use_staging {
                write_buf[stg_v_cursor..stg_v_cursor + src.len()].copy_from_slice(src);
                copy_regions_v.push(vk::BufferCopy {
                    src_offset: stg_v_cursor as u64,
                    dst_offset: vb_offset as u64,
                    size: src.len() as u64,
                });
                stg_v_cursor += src.len();
            } else {
                write_buf[vb_offset..vb_offset + src.len()].copy_from_slice(src);
            }

            let local_base = vert_cursor as u32;
            let local_end = vert_end as u32;
            let mut bucket_indices: Vec<u32> = Vec::new();

            while idx_cursor + 6 <= indices.len() {
                let max_idx = indices[idx_cursor..idx_cursor + 6]
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(0);
                if max_idx >= local_end {
                    break;
                }
                for &idx in &indices[idx_cursor..idx_cursor + 6] {
                    bucket_indices.push(idx - local_base);
                }
                idx_cursor += 6;
            }

            let ib_offset = bucket as usize * BUCKET_INDICES as usize * INDEX_SIZE as usize;
            let idx_bytes = bytemuck::cast_slice(&bucket_indices);

            if self.use_staging {
                let stg_off = staging_half + stg_i_cursor;
                write_buf[stg_off..stg_off + idx_bytes.len()].copy_from_slice(idx_bytes);
                copy_regions_i.push(vk::BufferCopy {
                    src_offset: stg_off as u64,
                    dst_offset: ib_offset as u64,
                    size: idx_bytes.len() as u64,
                });
                stg_i_cursor += idx_bytes.len();
            } else {
                let ib_ptr = self.index_alloc.mapped_slice_mut().unwrap();
                ib_ptr[ib_offset..ib_offset + idx_bytes.len()].copy_from_slice(idx_bytes);
            }

            index_counts.push(bucket_indices.len() as u32);
            bucket_ids.push(bucket);
            vert_cursor = vert_end;
        }

        if idx_cursor < indices.len() {
            let last_bucket = *bucket_ids.last().unwrap();
            let local_base = (verts.len() - (verts.len() % BUCKET_VERTICES as usize).max(1)) as u32;
            let remaining: Vec<u32> = indices[idx_cursor..]
                .iter()
                .map(|&idx| idx - local_base)
                .collect();
            let ib_offset = last_bucket as usize * BUCKET_INDICES as usize * INDEX_SIZE as usize;
            let existing_count = *index_counts.last().unwrap() as usize;
            let idx_bytes = bytemuck::cast_slice(&remaining);
            let start = ib_offset + existing_count * INDEX_SIZE as usize;

            if self.use_staging {
                let stg_off = staging_half + stg_i_cursor;
                write_buf[stg_off..stg_off + idx_bytes.len()].copy_from_slice(idx_bytes);
                copy_regions_i.push(vk::BufferCopy {
                    src_offset: stg_off as u64,
                    dst_offset: start as u64,
                    size: idx_bytes.len() as u64,
                });
            } else {
                let ib_ptr = self.index_alloc.mapped_slice_mut().unwrap();
                ib_ptr[start..start + idx_bytes.len()].copy_from_slice(idx_bytes);
            }
            *index_counts.last_mut().unwrap() += remaining.len() as u32;
        }

        if self.use_staging && (!copy_regions_v.is_empty() || !copy_regions_i.is_empty()) {
            unsafe {
                let begin = vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
                device
                    .begin_command_buffer(self.transfer_cmd, &begin)
                    .unwrap();
                if !copy_regions_v.is_empty() {
                    device.cmd_copy_buffer(
                        self.transfer_cmd,
                        self.staging_buffer,
                        self.vertex_buffer,
                        &copy_regions_v,
                    );
                }
                if !copy_regions_i.is_empty() {
                    device.cmd_copy_buffer(
                        self.transfer_cmd,
                        self.staging_buffer,
                        self.index_buffer,
                        &copy_regions_i,
                    );
                }
                device.end_command_buffer(self.transfer_cmd).unwrap();
                let cmds = [self.transfer_cmd];
                let submit = [vk::SubmitInfo::default().command_buffers(&cmds)];
                device
                    .queue_submit(queue, &submit, vk::Fence::null())
                    .unwrap();
                device.queue_wait_idle(queue).unwrap();
            }
        }

        self.chunks.insert(
            mesh.pos,
            ChunkAlloc {
                buckets: bucket_ids,
                index_counts,
                aabb,
                uploaded_at: std::time::Instant::now(),
            },
        );
        self.meta_dirty = true;
    }

    pub fn remove(&mut self, pos: &ChunkPos) {
        if let Some(alloc) = self.chunks.remove(pos) {
            for bucket in alloc.buckets {
                self.free_buckets.push_back(bucket);
            }
            self.meta_dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.free_buckets.clear();
        for i in 0..self.total_buckets {
            self.free_buckets.push_back(i);
        }
        self.cached_meta.clear();
        self.meta_dirty = true;
        self.fade_enabled = false;
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunks.len() as u32
    }

    pub fn dispatch_cull(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        frustum: &[[f32; 4]; 6],
        camera_pos: [f32; 3],
    ) {
        if self.chunks.is_empty() {
            return;
        }

        let now = std::time::Instant::now();
        const FADE_DURATION_MS: f32 = 1000.0;
        const NEARBY_DIST_SQ: f32 = 768.0;

        let any_fading = self.fade_enabled
            && self.chunks.values().any(|alloc| {
                now.duration_since(alloc.uploaded_at).as_secs_f32() * 1000.0 < FADE_DURATION_MS
            });

        if self.meta_dirty || any_fading {
            self.cached_meta.clear();
            for alloc in self.chunks.values() {
                let dx = (alloc.aabb.min[0] + alloc.aabb.max[0]) * 0.5 - camera_pos[0];
                let dz = (alloc.aabb.min[2] + alloc.aabb.max[2]) * 0.5 - camera_pos[2];
                let dist_sq = dx * dx + dz * dz;
                let vis = if !self.fade_enabled || dist_sq < NEARBY_DIST_SQ {
                    1.0
                } else {
                    let elapsed_ms = now.duration_since(alloc.uploaded_at).as_secs_f32() * 1000.0;
                    (elapsed_ms / FADE_DURATION_MS).min(1.0)
                };
                let vis_bits = vis.to_bits();

                for (i, &bucket) in alloc.buckets.iter().enumerate() {
                    self.cached_meta.push(ChunkMeta {
                        aabb_min: alloc.aabb.min,
                        aabb_max: alloc.aabb.max,
                        index_count: alloc.index_counts[i],
                        first_index: bucket * BUCKET_INDICES,
                        vertex_offset: (bucket * BUCKET_VERTICES) as i32,
                        visibility: vis_bits,
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
        };
        let frustum_bytes = bytemuck::bytes_of(&frustum_data);
        self.frustum_allocs[frame].mapped_slice_mut().unwrap()[..frustum_bytes.len()]
            .copy_from_slice(frustum_bytes);

        self.count_allocs[frame].mapped_slice_mut().unwrap()[..4]
            .copy_from_slice(&0u32.to_ne_bytes());

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.compute_pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::COMPUTE,
                self.compute_layout,
                0,
                &[self.compute_sets[frame]],
                &[],
            );
            device.cmd_dispatch(cmd, count.div_ceil(64), 1, 1);

            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                .dst_access_mask(vk::AccessFlags::INDIRECT_COMMAND_READ);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::PipelineStageFlags::DRAW_INDIRECT,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }

        if !self.fade_enabled {
            self.fade_enabled = true;
        }
    }

    pub fn draw_indirect(&self, device: &ash::Device, cmd: vk::CommandBuffer, frame: usize) {
        if self.chunks.is_empty() {
            return;
        }

        let max_draws = self
            .chunks
            .values()
            .map(|c| c.buckets.len() as u32)
            .sum::<u32>();

        unsafe {
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_bind_index_buffer(cmd, self.index_buffer, 0, vk::IndexType::UINT32);
            if cfg!(target_os = "macos") {
                device.cmd_draw_indexed_indirect(
                    cmd,
                    self.indirect_buffers[frame],
                    0,
                    max_draws,
                    std::mem::size_of::<DrawCommand>() as u32,
                );
            } else {
                device.cmd_draw_indexed_indirect_count(
                    cmd,
                    self.indirect_buffers[frame],
                    0,
                    self.count_buffers[frame],
                    0,
                    max_draws,
                    std::mem::size_of::<DrawCommand>() as u32,
                );
            }
        }
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        unsafe {
            device.destroy_buffer(self.vertex_buffer, None);
            device.destroy_buffer(self.index_buffer, None);
        }
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
            unsafe {
                device.destroy_buffer(self.meta_buffers[i], None);
                device.destroy_buffer(self.indirect_buffers[i], None);
                device.destroy_buffer(self.count_buffers[i], None);
                device.destroy_buffer(self.frustum_buffers[i], None);
            }
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
        unsafe { device.destroy_buffer(self.staging_buffer, None) };
        alloc
            .free(std::mem::replace(&mut self.staging_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        drop(alloc);

        unsafe {
            device.destroy_command_pool(self.transfer_pool, None);
            device.destroy_pipeline(self.compute_pipeline, None);
            device.destroy_pipeline_layout(self.compute_layout, None);
            device.destroy_descriptor_pool(self.compute_pool, None);
            device.destroy_descriptor_set_layout(self.compute_desc_layout, None);
        }
    }
}

fn create_cull_desc_layout(device: &ash::Device) -> vk::DescriptorSetLayout {
    let bindings = [
        vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 1,
            descriptor_type: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 2,
            descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            ..Default::default()
        },
        vk::DescriptorSetLayoutBinding {
            binding: 3,
            descriptor_type: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::COMPUTE,
            ..Default::default()
        },
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
        .expect("failed to create cull desc layout")
}

fn desc_write(
    set: vk::DescriptorSet,
    binding: u32,
    ty: vk::DescriptorType,
    buffer: vk::Buffer,
    range: u64,
) -> vk::WriteDescriptorSet<'static> {
    // Safety: the DescriptorBufferInfo is stored inline in WriteDescriptorSet via the builder
    // pattern, but ash's lifetime requirements need a reference. We use a leaked Box here
    // because these writes only happen once at init time.
    let info = Box::leak(Box::new([vk::DescriptorBufferInfo {
        buffer,
        offset: 0,
        range,
    }]));
    vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(ty)
        .buffer_info(info)
}
