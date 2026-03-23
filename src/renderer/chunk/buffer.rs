use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use ash::vk;
use azalea_core::position::ChunkPos;
use gpu_allocator::vulkan::{Allocation, Allocator};

use super::mesher::{ChunkMeshData, ChunkVertex};
use crate::renderer::shader;
use crate::renderer::util;
use crate::renderer::MAX_FRAMES_IN_FLIGHT;

const BUCKET_VERTICES: u32 = 32768;
const BUCKET_INDICES: u32 = 49152;
const TOTAL_BUCKETS: u32 = 1024;
const VERTEX_SIZE: u64 = std::mem::size_of::<ChunkVertex>() as u64;
const INDEX_SIZE: u64 = std::mem::size_of::<u32>() as u64;

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
    _pad: u32,
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
    _pad: [u32; 3],
}

struct ChunkAlloc {
    buckets: Vec<u32>,
    index_counts: Vec<u32>,
    aabb: ChunkAABB,
}

pub struct ChunkBufferStore {
    vertex_buffer: vk::Buffer,
    vertex_alloc: Allocation,
    index_buffer: vk::Buffer,
    index_alloc: Allocation,

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
}

impl ChunkBufferStore {
    pub fn new(device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) -> Self {
        let vertex_size = TOTAL_BUCKETS as u64 * BUCKET_VERTICES as u64 * VERTEX_SIZE;
        let index_size = TOTAL_BUCKETS as u64 * BUCKET_INDICES as u64 * INDEX_SIZE;

        let (vertex_buffer, vertex_alloc) = util::create_host_buffer(
            device,
            allocator,
            vertex_size,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "vertex_pool",
        );
        let (index_buffer, index_alloc) = util::create_host_buffer(
            device,
            allocator,
            index_size,
            vk::BufferUsageFlags::INDEX_BUFFER,
            "index_pool",
        );

        let mut free_buckets = VecDeque::with_capacity(TOTAL_BUCKETS as usize);
        for i in 0..TOTAL_BUCKETS {
            free_buckets.push_back(i);
        }

        let max_meta = (TOTAL_BUCKETS * 2) as u64;
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
            vertex_buffer,
            vertex_alloc,
            index_buffer,
            index_alloc,
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
        }
    }

    pub fn upload(&mut self, mesh: &ChunkMeshData) {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            self.remove(&mesh.pos);
            return;
        }

        self.remove(&mesh.pos);

        let num_buckets = mesh.vertices.len().div_ceil(BUCKET_VERTICES as usize) as u32;
        if self.free_buckets.len() < num_buckets as usize {
            log::warn!(
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

        let vb_ptr = self.vertex_alloc.mapped_slice_mut().unwrap();
        let ib_ptr = self.index_alloc.mapped_slice_mut().unwrap();

        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vert_cursor = 0usize;
        let mut idx_cursor = 0usize;

        for _ in 0..num_buckets {
            let bucket = self.free_buckets.pop_front().unwrap();
            let vert_end = (vert_cursor + BUCKET_VERTICES as usize).min(verts.len());
            let _vert_count = vert_end - vert_cursor;

            let vb_offset = bucket as usize * BUCKET_VERTICES as usize * VERTEX_SIZE as usize;
            let src = bytemuck::cast_slice(&verts[vert_cursor..vert_end]);
            vb_ptr[vb_offset..vb_offset + src.len()].copy_from_slice(src);

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
            ib_ptr[ib_offset..ib_offset + idx_bytes.len()].copy_from_slice(idx_bytes);

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
            ib_ptr[start..start + idx_bytes.len()].copy_from_slice(idx_bytes);
            *index_counts.last_mut().unwrap() += remaining.len() as u32;
        }

        self.chunks.insert(
            mesh.pos,
            ChunkAlloc {
                buckets: bucket_ids,
                index_counts,
                aabb,
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
        for i in 0..TOTAL_BUCKETS {
            self.free_buckets.push_back(i);
        }
        self.cached_meta.clear();
        self.meta_dirty = true;
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
        _camera_pos: [f32; 3],
    ) {
        if self.chunks.is_empty() {
            return;
        }

        if self.meta_dirty {
            self.cached_meta.clear();
            for alloc in self.chunks.values() {
                for (i, &bucket) in alloc.buckets.iter().enumerate() {
                    self.cached_meta.push(ChunkMeta {
                        aabb_min: alloc.aabb.min,
                        aabb_max: alloc.aabb.max,
                        index_count: alloc.index_counts[i],
                        first_index: bucket * BUCKET_INDICES,
                        vertex_offset: (bucket * BUCKET_VERTICES) as i32,
                        _pad: 0,
                    });
                }
            }
            self.meta_dirty = false;
        }

        let count = self.cached_meta.len() as u32;
        let meta_bytes = bytemuck::cast_slice(&self.cached_meta);
        self.meta_allocs[frame].mapped_slice_mut().unwrap()[..meta_bytes.len()]
            .copy_from_slice(meta_bytes);

        let frustum_data = FrustumData {
            planes: *frustum,
            chunk_count: count,
            _pad: [0; 3],
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
        drop(alloc);

        unsafe {
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
