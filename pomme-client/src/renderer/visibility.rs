use std::slice;
use std::sync::{Arc, Mutex};

use azalea_core::position::ChunkPos;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::Camera;
use crate::renderer::hiz::HizPipeline;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::util::{CHUNK_RING_SIZE as MAX_SIZE_SQ, MAX_RD, MAX_SIZE, SIZE_Y};

/// Bytes of one frame's visibility bitset: one `u32` mask per column slot.
const MASK_BYTES: u64 = (MAX_SIZE_SQ * 4) as u64;

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct VisibilityUniform {
    pub view_proj: [[f32; 4]; 4],
    pub vis_center: [i32; 4], // x,z = center in section coords, y = min_section, w = max_section
    pub camera_pos: [f32; 4],
    pub radius: i32,
    pub section_count: i32,
    pub padding0: i32,
    pub padding1: i32,
    pub frustum_planes: [[f32; 4]; 6],
}

pub struct PerFrameData {
    pub uniform_buffer: vk::Buffer,
    pub uniform_allocation: Allocation,
    pub output_buffer: vk::Buffer,
    pub output_allocation: Allocation,
    pub readback_buffer: vk::Buffer,
    pub readback_allocation: Allocation,
    pub frame_descriptor_set: vk::DescriptorSet,
    pub hiz_descriptor_set: vk::DescriptorSet,
    pub vis_center: ChunkPos,
    /// Section layout the slot's mask was written with; the cull that reads
    /// the mask three frames later must decode bits with these, not the
    /// current frame's values.
    pub mask_min_section: i32,
    pub mask_section_count: i32,
}

impl PerFrameData {
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        frame_layout: &vk::DescriptorSetLayout,
        hiz_layout: &vk::DescriptorSetLayout,
        frame_pool: &vk::DescriptorPool,
        hiz_pool: &vk::DescriptorPool,
        hiz_pipeline: &HizPipeline,
        frame_idx: usize,
    ) -> Self {
        let ubo_size = std::mem::size_of::<VisibilityUniform>() as u64;
        let sbo_size = MASK_BYTES;

        let (uniform_buffer, uniform_allocation) = util::create_host_buffer(
            device,
            allocator,
            ubo_size,
            vk::BufferUsageFlags::UniformBuffer,
            &format!("visibility_ubo_{}", frame_idx),
        );

        let (output_buffer, output_allocation) = util::create_device_buffer(
            device,
            allocator,
            sbo_size,
            vk::BufferUsageFlags::StorageBuffer | vk::BufferUsageFlags::TransferSrc,
            &format!("visibility_output_sbo_{}", frame_idx),
        );

        let (readback_buffer, readback_allocation) = util::create_host_buffer(
            device,
            allocator,
            sbo_size,
            vk::BufferUsageFlags::TransferDst,
            &format!("visibility_readback_{}", frame_idx),
        );

        // Allocate frame descriptor set
        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: *frame_pool,
            descriptor_set_count: 1,
            set_layouts: frame_layout,
            ..Default::default()
        };
        let mut frame_descriptor_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&alloc_info, slice::from_mut(&mut frame_descriptor_set))
            .expect("failed to allocate visibility frame set");

        // Update frame descriptors
        let sbo_info = vk::DescriptorBufferInfo {
            buffer: output_buffer,
            offset: 0,
            range: sbo_size,
        };
        let ubo_info = vk::DescriptorBufferInfo {
            buffer: uniform_buffer,
            offset: 0,
            range: ubo_size,
        };

        let writes = [
            vk::WriteDescriptorSet {
                dst_set: frame_descriptor_set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::StorageBuffer,
                descriptor_count: 1,
                buffer_info: &sbo_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: frame_descriptor_set,
                dst_binding: 1,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &ubo_info,
                ..Default::default()
            },
        ];
        device.update_descriptor_sets(&writes, &[]);

        // Allocate HiZ descriptor set
        let hiz_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: *hiz_pool,
            descriptor_set_count: 1,
            set_layouts: hiz_layout,
            ..Default::default()
        };
        let mut hiz_descriptor_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&hiz_alloc_info, slice::from_mut(&mut hiz_descriptor_set))
            .expect("failed to allocate visibility hiz set");

        // Update HiZ descriptor with the per-frame HiZ pyramid
        let hiz_info = vk::DescriptorImageInfo {
            sampler: hiz_pipeline.sampler(frame_idx),
            image_view: hiz_pipeline.full_view(frame_idx),
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let hiz_write = vk::WriteDescriptorSet {
            dst_set: hiz_descriptor_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &hiz_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[hiz_write], &[]);

        Self {
            uniform_buffer,
            uniform_allocation,
            output_buffer,
            output_allocation,
            readback_buffer,
            readback_allocation,
            frame_descriptor_set,
            hiz_descriptor_set,
            vis_center: ChunkPos::default(),
            mask_min_section: 0,
            mask_section_count: 0,
        }
    }

    fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.uniform_buffer, None);
        let allocation = std::mem::take(&mut self.uniform_allocation);
        alloc.free(allocation).ok();

        device.destroy_buffer(self.output_buffer, None);
        let allocation = std::mem::take(&mut self.output_allocation);
        alloc.free(allocation).ok();

        device.destroy_buffer(self.readback_buffer, None);
        let allocation = std::mem::take(&mut self.readback_allocation);
        alloc.free(allocation).ok();
    }
}

pub struct VisibilityPipeline {
    layout_frame: vk::DescriptorSetLayout,
    layout_hiz: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,

    frame_descriptor_pool: vk::DescriptorPool,
    hiz_descriptor_pool: vk::DescriptorPool,

    per_frame: [PerFrameData; MAX_FRAMES_IN_FLIGHT],
    /// Whether `execute` has ever recorded into each frame slot; gates
    /// `readback`.
    executed: [bool; MAX_FRAMES_IN_FLIGHT],
}

impl VisibilityPipeline {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        hiz_pipeline: &HizPipeline,
    ) -> Self {
        // 1. Create descriptor layouts
        let frame_bindings = [
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
        ];
        let layout_frame_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: frame_bindings.len() as u32,
            bindings: frame_bindings.as_ptr(),
            ..Default::default()
        };
        let layout_frame = device
            .create_descriptor_set_layout(&layout_frame_info, None)
            .expect("failed to create visibility frame layout");

        let hiz_bindings = [vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Compute,
            ..Default::default()
        }];
        let layout_hiz_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: hiz_bindings.len() as u32,
            bindings: hiz_bindings.as_ptr(),
            ..Default::default()
        };
        let layout_hiz = device
            .create_descriptor_set_layout(&layout_hiz_info, None)
            .expect("failed to create visibility hiz layout");

        // 2. Create pipeline layout
        let set_layouts = [layout_frame, layout_hiz];
        let pipeline_layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: set_layouts.len() as u32,
            set_layouts: set_layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&pipeline_layout_info, None)
            .expect("failed to create visibility pipeline layout");

        // 3. Create shaders and pipeline
        let comp_spv = shader::include_spirv!("visibility.comp.spv");
        let comp_module = shader::create_shader_module(device, comp_spv);
        let spec_entries = [vk::SpecializationMapEntry {
            constant_id: 0,
            offset: 0,
            size: 4,
        }];
        let spec_data = [MAX_RD as i32];
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
            layout: pipeline_layout,
            ..Default::default()
        }];
        let mut pipeline = vk::Pipeline::null();
        device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &pipe_info,
                None,
                slice::from_mut(&mut pipeline),
            )
            .expect("failed to create visibility compute pipeline");
        device.destroy_shader_module(comp_module, None);

        // 4. Create descriptor pools
        let frame_pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::StorageBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
        ];
        let frame_pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: frame_pool_sizes.len() as u32,
            pool_sizes: frame_pool_sizes.as_ptr(),
            ..Default::default()
        };
        let frame_descriptor_pool = device
            .create_descriptor_pool(&frame_pool_info, None)
            .expect("failed to create visibility frame descriptor pool");

        let hiz_pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
        }];
        let hiz_pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: hiz_pool_sizes.len() as u32,
            pool_sizes: hiz_pool_sizes.as_ptr(),
            ..Default::default()
        };
        let hiz_descriptor_pool = device
            .create_descriptor_pool(&hiz_pool_info, None)
            .expect("failed to create visibility hiz descriptor pool");

        // 5. Create per-frame data
        let per_frame: [PerFrameData; MAX_FRAMES_IN_FLIGHT] = std::array::from_fn(|i| {
            PerFrameData::new(
                device,
                allocator,
                &layout_frame,
                &layout_hiz,
                &frame_descriptor_pool,
                &hiz_descriptor_pool,
                hiz_pipeline,
                i,
            )
        });

        Self {
            layout_frame,
            layout_hiz,
            pipeline_layout,
            pipeline,
            frame_descriptor_pool,
            hiz_descriptor_pool,
            per_frame,
            executed: [false; MAX_FRAMES_IN_FLIGHT],
        }
    }

    /// Updates all per-frame HiZ descriptors. Call this after HiZ pipeline
    /// resize or recreation.
    pub fn update_hiz_descriptors(&self, device: &vk::Device, hiz_pipeline: &HizPipeline) {
        for frame_idx in 0..MAX_FRAMES_IN_FLIGHT {
            let hiz_info = vk::DescriptorImageInfo {
                sampler: hiz_pipeline.sampler(frame_idx),
                image_view: hiz_pipeline.full_view(frame_idx),
                image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            };

            let write = vk::WriteDescriptorSet {
                dst_set: self.per_frame[frame_idx].hiz_descriptor_set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                image_info: &hiz_info,
                ..Default::default()
            };

            device.update_descriptor_sets(&[write], &[]);
        }
    }

    /// The frame slot's visibility bitset, or `None` while the slot has never
    /// been written (its buffer is zeroed, i.e. "all occluded" — callers must
    /// fail open to fully visible instead).
    pub fn readback(&self, frame: usize) -> Option<&[u32]> {
        if !self.executed[frame] {
            return None;
        }
        let size_in_bytes = MASK_BYTES as usize;
        let mapped = self.per_frame[frame]
            .readback_allocation
            .mapped_slice()
            .unwrap();
        Some(bytemuck::cast_slice(&mapped[..size_in_bytes]))
    }

    pub fn vis_center(&self, frame: usize) -> ChunkPos {
        self.per_frame[frame].vis_center
    }

    /// The slot's mask decode parameters `(center, min_section,
    /// section_count)` for the GPU cull, or `None` (fail open) while the slot
    /// has never been written.
    pub fn mask_params(&self, frame: usize) -> Option<(ChunkPos, i32, i32)> {
        if !self.executed[frame] {
            return None;
        }
        let data = &self.per_frame[frame];
        Some((
            data.vis_center,
            data.mask_min_section,
            data.mask_section_count,
        ))
    }

    pub fn output_buffers(&self) -> [vk::Buffer; MAX_FRAMES_IN_FLIGHT] {
        std::array::from_fn(|i| self.per_frame[i].output_buffer)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        camera: &Camera,
        radius: u32,
        height: u32,
        min_y: i32,
        extra_radians: f32,
    ) {
        self.executed[frame] = true;
        let frame_data = &mut self.per_frame[frame];

        // This frame's cull dispatch read the slot's previous mask earlier in
        // this command buffer; order that read before the clear (WAR).
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[],
        );

        // 1. Reset the mask to all-visible; the shader only clears bits it
        // proves occluded, so fail-open paths cost no writes at all.
        cmd.fill_buffer(frame_data.output_buffer, 0, MASK_BYTES, u32::MAX);

        // 2. Barrier to make sure fill command completes before compute writes to SBO
        let clear_barrier = vk::BufferMemoryBarrier {
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::ShaderWrite | vk::AccessFlags::ShaderRead,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            buffer: frame_data.output_buffer,
            offset: 0,
            size: MASK_BYTES,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[clear_barrier],
            &[],
        );

        // 3. Update the Uniform Block UBO
        let view_proj = camera.view_projection();

        // Calculate section coordinates
        let center_section_x = (camera.position.x / 16.0).floor() as i32;
        let center_section_z = (camera.position.z / 16.0).floor() as i32;

        let offset = camera.third_person_offset();
        let pos = camera.position.as_vec3() + offset;

        // Store vis center as chunk position for CPU-side reference
        frame_data.vis_center = ChunkPos::new(center_section_x, center_section_z);

        let min_section = min_y.div_euclid(16);

        // Calculate vis_center in section coordinates for the shader
        // The output bitset holds one bit per section layer, so the pass caps
        // at SIZE_Y (32) layers regardless of world height.
        let section_count = (height / 16).min(SIZE_Y as u32);
        frame_data.mask_min_section = min_section;
        frame_data.mask_section_count = section_count as i32;
        let max_section = min_section + section_count as i32;

        let uniform_data = VisibilityUniform {
            view_proj: view_proj.to_cols_array_2d(),
            vis_center: [
                center_section_x, // center_x in section coords
                min_section,      // min_section
                center_section_z, // center_z in section coords
                max_section,      // max_section
            ],
            camera_pos: [pos.x, pos.y, pos.z, 0.0],
            radius: radius as i32,
            section_count: section_count as i32,
            padding0: 0,
            padding1: 0,
            frustum_planes: camera.frustum_planes_dilated(extra_radians),
        };

        let ubo_bytes = bytemuck::bytes_of(&uniform_data);
        frame_data.uniform_allocation.mapped_slice_mut().unwrap()[..ubo_bytes.len()]
            .copy_from_slice(ubo_bytes);

        // 4. Bind compute pipeline and sets
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.pipeline);

        let sets = [
            frame_data.frame_descriptor_set,
            frame_data.hiz_descriptor_set,
        ];
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            self.pipeline_layout,
            0,
            &sets,
            &[],
        );

        // 5. Dispatch shader
        const WG_X: u32 = 8;
        const WG_Y: u32 = 8;
        const WG_Z: u32 = 4;

        let groups_x = (MAX_SIZE as u32).div_ceil(WG_X);
        let groups_y = section_count.div_ceil(WG_Y);
        let groups_z = (MAX_SIZE as u32).div_ceil(WG_Z);

        cmd.dispatch(groups_x, groups_y, groups_z);

        // 6. Make the mask write visible to the readback copy below AND to
        // the cull dispatch that reads this buffer three frames from now.
        let sbo_copy_barrier = vk::BufferMemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::TransferRead | vk::AccessFlags::ShaderRead,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            buffer: frame_data.output_buffer,
            offset: 0,
            size: MASK_BYTES,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::Transfer | vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[sbo_copy_barrier],
            &[],
        );

        // 7. Copy output to CPU readback buffer
        cmd.copy_buffer(
            frame_data.output_buffer,
            frame_data.readback_buffer,
            &[vk::BufferCopy {
                src_offset: 0,
                dst_offset: 0,
                size: MASK_BYTES,
            }],
        );
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_descriptor_set_layout(self.layout_frame, None);
        device.destroy_descriptor_set_layout(self.layout_hiz, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_descriptor_pool(self.frame_descriptor_pool, None);
        device.destroy_descriptor_pool(self.hiz_descriptor_pool, None);

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            self.per_frame[i].destroy(device, allocator);
        }
    }
}
