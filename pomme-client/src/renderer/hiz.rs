use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader};

pub struct HizPyramidResources {
    pub pyramid_image: vk::Image,
    pub pyramid_allocation: Option<Allocation>,
    pub pyramid_sampler: vk::Sampler,
    pub pyramid_mip_levels: u32,
    pub pyramid_mip_views: Vec<vk::ImageView>,
    pub pyramid_full_view: vk::ImageView,
    pub copy_set: vk::DescriptorSet,
    pub reduce_sets: Vec<vk::DescriptorSet>,
    pub desc_pool: vk::DescriptorPool,
}

pub struct HizPipeline {
    copy_layout: vk::DescriptorSetLayout,
    reduce_layout: vk::DescriptorSetLayout,
    copy_pipeline_layout: vk::PipelineLayout,
    reduce_pipeline_layout: vk::PipelineLayout,
    copy_pipeline: vk::Pipeline,
    reduce_pipeline: vk::Pipeline,
    depth_sampler: vk::Sampler,
    frame_resources: [HizPyramidResources; MAX_FRAMES_IN_FLIGHT],
    width: u32,
    height: u32,
}

impl HizPipeline {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        width: u32,
        height: u32,
        depth_view: vk::ImageView,
    ) -> Self {
        let copy_bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Compute,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 1,
                descriptor_type: vk::DescriptorType::StorageImage,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Compute,
                ..Default::default()
            },
        ];
        let copy_layout_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: copy_bindings.len() as u32,
            bindings: copy_bindings.as_ptr(),
            ..Default::default()
        };
        let copy_layout = device
            .create_descriptor_set_layout(&copy_layout_info, None)
            .expect("failed to create hiz copy desc layout");

        let reduce_bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::StorageImage,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Compute,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 1,
                descriptor_type: vk::DescriptorType::StorageImage,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Compute,
                ..Default::default()
            },
        ];
        let reduce_layout_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: reduce_bindings.len() as u32,
            bindings: reduce_bindings.as_ptr(),
            ..Default::default()
        };
        let reduce_layout = device
            .create_descriptor_set_layout(&reduce_layout_info, None)
            .expect("failed to create hiz reduce desc layout");

        let copy_pli = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &copy_layout,
            ..Default::default()
        };
        let copy_pipeline_layout = device
            .create_pipeline_layout(&copy_pli, None)
            .expect("failed to create hiz copy pipeline layout");

        let reduce_pli = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &reduce_layout,
            ..Default::default()
        };
        let reduce_pipeline_layout = device
            .create_pipeline_layout(&reduce_pli, None)
            .expect("failed to create hiz reduce pipeline layout");

        let copy_spv = shader::include_spirv!("hiz_copy.comp.spv");
        let reduce_spv = shader::include_spirv!("hiz_reduce.comp.spv");
        let copy_mod = shader::create_shader_module(device, copy_spv);
        let reduce_mod = shader::create_shader_module(device, reduce_spv);

        let mut copy_pipeline = vk::Pipeline::null();
        let copy_stage = vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module: copy_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        };
        let copy_pipe_info = [vk::ComputePipelineCreateInfo {
            stage: copy_stage,
            layout: copy_pipeline_layout,
            ..Default::default()
        }];
        device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &copy_pipe_info,
                None,
                slice::from_mut(&mut copy_pipeline),
            )
            .expect("failed to create hiz copy pipeline");

        let mut reduce_pipeline = vk::Pipeline::null();
        let reduce_stage = vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module: reduce_mod,
            name: c"main".as_ptr(),
            ..Default::default()
        };
        let reduce_pipe_info = [vk::ComputePipelineCreateInfo {
            stage: reduce_stage,
            layout: reduce_pipeline_layout,
            ..Default::default()
        }];
        device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &reduce_pipe_info,
                None,
                slice::from_mut(&mut reduce_pipeline),
            )
            .expect("failed to create hiz reduce pipeline");

        device.destroy_shader_module(copy_mod, None);
        device.destroy_shader_module(reduce_mod, None);

        let depth_sampler_info = vk::SamplerCreateInfo {
            mag_filter: vk::Filter::Nearest,
            min_filter: vk::Filter::Nearest,
            mipmap_mode: vk::SamplerMipmapMode::Nearest,
            address_mode_u: vk::SamplerAddressMode::ClampToEdge,
            address_mode_v: vk::SamplerAddressMode::ClampToEdge,
            address_mode_w: vk::SamplerAddressMode::ClampToEdge,
            min_lod: 0.0,
            max_lod: 0.0,
            ..Default::default()
        };
        let depth_sampler = device
            .create_sampler(&depth_sampler_info, None)
            .expect("failed to create hiz depth sampler");

        let max_dim = width.max(height).max(1);
        let mip_levels = u32::BITS - max_dim.leading_zeros();

        let frame_resources: [HizPyramidResources; MAX_FRAMES_IN_FLIGHT] =
            std::array::from_fn(|i| {
                create_frame_resources(
                    device,
                    allocator,
                    width,
                    height,
                    mip_levels,
                    depth_view,
                    &copy_layout,
                    &reduce_layout,
                    depth_sampler,
                    i,
                )
            });

        Self {
            copy_layout,
            reduce_layout,
            copy_pipeline_layout,
            reduce_pipeline_layout,
            copy_pipeline,
            reduce_pipeline,
            depth_sampler,
            frame_resources,
            width,
            height,
        }
    }

    pub fn resize(
        &mut self,
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        width: u32,
        height: u32,
        depth_view: vk::ImageView,
    ) {
        if self.width == width && self.height == height {
            // Just update depth view for all frames
            for frame_idx in 0..MAX_FRAMES_IN_FLIGHT {
                let resources = &self.frame_resources[frame_idx];
                let src_info = vk::DescriptorImageInfo {
                    sampler: self.depth_sampler,
                    image_view: depth_view,
                    image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
                };
                let write = vk::WriteDescriptorSet {
                    dst_set: resources.copy_set,
                    dst_binding: 0,
                    descriptor_type: vk::DescriptorType::CombinedImageSampler,
                    descriptor_count: 1,
                    image_info: &src_info,
                    ..Default::default()
                };
                device.update_descriptor_sets(&[write], &[]);
            }
            return;
        }

        for frame_idx in 0..MAX_FRAMES_IN_FLIGHT {
            destroy_frame_resources(device, allocator, &mut self.frame_resources[frame_idx]);
        }

        let max_dim = width.max(height).max(1);
        let mip_levels = u32::BITS - max_dim.leading_zeros();

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            self.frame_resources[i] = create_frame_resources(
                device,
                allocator,
                width,
                height,
                mip_levels,
                depth_view,
                &self.copy_layout,
                &self.reduce_layout,
                self.depth_sampler,
                i,
            );
        }

        self.width = width;
        self.height = height;
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        for frame_idx in 0..MAX_FRAMES_IN_FLIGHT {
            destroy_frame_resources(device, allocator, &mut self.frame_resources[frame_idx]);
        }
        device.destroy_descriptor_set_layout(self.copy_layout, None);
        device.destroy_descriptor_set_layout(self.reduce_layout, None);
        device.destroy_pipeline_layout(self.copy_pipeline_layout, None);
        device.destroy_pipeline_layout(self.reduce_pipeline_layout, None);
        device.destroy_pipeline(self.copy_pipeline, None);
        device.destroy_pipeline(self.reduce_pipeline, None);
        device.destroy_sampler(self.depth_sampler, None);
    }

    pub fn full_view(&self, frame_idx: usize) -> vk::ImageView {
        self.frame_resources[frame_idx].pyramid_full_view
    }

    pub fn sampler(&self, frame_idx: usize) -> vk::Sampler {
        self.frame_resources[frame_idx].pyramid_sampler
    }

    pub fn execute(
        &self,
        cmd: vk::CommandBuffer,
        frame_idx: usize,
        depth_image: vk::Image,
        extent: vk::Extent2D,
    ) {
        let resources = &self.frame_resources[frame_idx];
        if resources.pyramid_mip_levels == 0 {
            return;
        }

        // Transition depth
        let depth_barrier = vk::ImageMemoryBarrier {
            src_access_mask: vk::AccessFlags::DepthStencilAttachmentWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            old_layout: vk::ImageLayout::DepthStencilAttachmentOptimal,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: depth_image,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Depth,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::EarlyFragmentTests | vk::PipelineStageFlags::LateFragmentTests,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[depth_barrier],
        );

        let pyramid_full_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: resources.pyramid_mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        };
        let pyramid_barrier = vk::ImageMemoryBarrier {
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::ShaderWrite,
            old_layout: vk::ImageLayout::Undefined,
            new_layout: vk::ImageLayout::General,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: resources.pyramid_image,
            subresource_range: pyramid_full_range,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[pyramid_barrier],
        );

        // Copy
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.copy_pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Compute,
            self.copy_pipeline_layout,
            0,
            &[resources.copy_set],
            &[],
        );
        let gx = extent.width.div_ceil(16);
        let gy = extent.height.div_ceil(16);
        cmd.dispatch(gx.max(1), gy.max(1), 1);

        let mip0_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let mip0_barrier = vk::ImageMemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            old_layout: vk::ImageLayout::General,
            new_layout: vk::ImageLayout::General,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: resources.pyramid_image,
            subresource_range: mip0_range,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[mip0_barrier],
        );

        // Reduce
        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.reduce_pipeline);
        let mut w = (extent.width / 2).max(1);
        let mut h = (extent.height / 2).max(1);
        for level in 1..resources.pyramid_mip_levels {
            let prev = level - 1;
            let prev_range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: prev,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let level_barrier = vk::ImageMemoryBarrier {
                src_access_mask: vk::AccessFlags::ShaderWrite,
                dst_access_mask: vk::AccessFlags::ShaderRead,
                old_layout: vk::ImageLayout::General,
                new_layout: vk::ImageLayout::General,
                src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
                image: resources.pyramid_image,
                subresource_range: prev_range,
                ..Default::default()
            };
            cmd.pipeline_barrier(
                vk::PipelineStageFlags::ComputeShader,
                vk::PipelineStageFlags::ComputeShader,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[level_barrier],
            );
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Compute,
                self.reduce_pipeline_layout,
                0,
                &[resources.reduce_sets[(level - 1) as usize]],
                &[],
            );
            let gx = w.div_ceil(16);
            let gy = h.div_ceil(16);
            cmd.dispatch(gx.max(1), gy.max(1), 1);
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }

        let final_barrier = vk::ImageMemoryBarrier {
            src_access_mask: vk::AccessFlags::ShaderWrite | vk::AccessFlags::ShaderRead,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            old_layout: vk::ImageLayout::General,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            dst_queue_family_index: vk::QUEUE_FAMILY_IGNORED,
            image: resources.pyramid_image,
            subresource_range: pyramid_full_range,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[final_barrier],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn create_frame_resources(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
    mip_levels: u32,
    depth_view: vk::ImageView,
    copy_layout: &vk::DescriptorSetLayout,
    reduce_layout: &vk::DescriptorSetLayout,
    depth_sampler: vk::Sampler,
    frame_idx: usize,
) -> HizPyramidResources {
    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::Type2D,
        format: vk::Format::R32Sfloat,
        extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
        mip_levels,
        array_layers: 1,
        samples: vk::SampleCountFlags::Type1,
        tiling: vk::ImageTiling::Optimal,
        usage: vk::ImageUsageFlags::Storage
            | vk::ImageUsageFlags::Sampled
            | vk::ImageUsageFlags::TransferDst,
        ..Default::default()
    };
    let pyramid_image = device
        .create_image(&image_info, None)
        .expect("failed to create hiz pyramid image");
    let mem_reqs = device.get_image_memory_requirements(pyramid_image);
    let pyramid_allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name: &format!("hiz_pyramid_image_{}", frame_idx),
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate hiz pyramid memory");
    unsafe {
        device
            .bind_image_memory(
                pyramid_image,
                pyramid_allocation.memory(),
                pyramid_allocation.offset(),
            )
            .expect("failed to bind hiz pyramid memory");
    }

    let sampler_info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Nearest,
        min_filter: vk::Filter::Nearest,
        mipmap_mode: vk::SamplerMipmapMode::Nearest,
        address_mode_u: vk::SamplerAddressMode::ClampToEdge,
        address_mode_v: vk::SamplerAddressMode::ClampToEdge,
        address_mode_w: vk::SamplerAddressMode::ClampToEdge,
        max_lod: mip_levels as f32,
        ..Default::default()
    };
    let pyramid_sampler = device
        .create_sampler(&sampler_info, None)
        .expect("failed to create hiz pyramid sampler");

    let full_view_info = vk::ImageViewCreateInfo {
        image: pyramid_image,
        view_type: vk::ImageViewType::Type2D,
        format: vk::Format::R32Sfloat,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 1,
        },
        ..Default::default()
    };
    let pyramid_full_view = device
        .create_image_view(&full_view_info, None)
        .expect("failed to create hiz full image view");

    let mut pyramid_mip_views = Vec::with_capacity(mip_levels as usize);
    for level in 0..mip_levels {
        let view_info = vk::ImageViewCreateInfo {
            image: pyramid_image,
            view_type: vk::ImageViewType::Type2D,
            format: vk::Format::R32Sfloat,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: level,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            },
            ..Default::default()
        };
        pyramid_mip_views.push(
            device
                .create_image_view(&view_info, None)
                .expect("failed to create hiz mip view"),
        );
    }

    let copy_total = 1;
    let reduce_total = (mip_levels as usize).saturating_sub(1);
    let sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: copy_total as u32,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::StorageImage,
            descriptor_count: (copy_total + reduce_total * 2) as u32,
        },
    ];
    let pool_info = vk::DescriptorPoolCreateInfo {
        max_sets: (copy_total + reduce_total) as u32,
        pool_size_count: sizes.len() as u32,
        pool_sizes: sizes.as_ptr(),
        ..Default::default()
    };
    let desc_pool = device
        .create_descriptor_pool(&pool_info, None)
        .expect("failed to create hiz descriptor pool");

    let mut copy_set = vk::DescriptorSet::null();
    device
        .allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo {
                descriptor_pool: desc_pool,
                descriptor_set_count: 1,
                set_layouts: copy_layout,
                ..Default::default()
            },
            slice::from_mut(&mut copy_set),
        )
        .expect("failed to allocate hiz copy set");

    let mut reduce_sets = Vec::new();
    if reduce_total > 0 {
        let reduce_layouts = vec![*reduce_layout; reduce_total];
        reduce_sets.resize(reduce_total, vk::DescriptorSet::null());
        device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo {
                    descriptor_pool: desc_pool,
                    descriptor_set_count: reduce_total as u32,
                    set_layouts: reduce_layouts.as_ptr(),
                    ..Default::default()
                },
                &mut reduce_sets,
            )
            .expect("failed to allocate hiz reduce sets");
    }

    // Update copy descriptor
    let src_info = vk::DescriptorImageInfo {
        sampler: depth_sampler,
        image_view: depth_view,
        image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
    };
    let dst_info = vk::DescriptorImageInfo {
        sampler: vk::Sampler::null(),
        image_view: pyramid_mip_views[0],
        image_layout: vk::ImageLayout::General,
    };
    device.update_descriptor_sets(
        &[
            vk::WriteDescriptorSet {
                dst_set: copy_set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
                image_info: &src_info,
                ..Default::default()
            },
            vk::WriteDescriptorSet {
                dst_set: copy_set,
                dst_binding: 1,
                descriptor_type: vk::DescriptorType::StorageImage,
                descriptor_count: 1,
                image_info: &dst_info,
                ..Default::default()
            },
        ],
        &[],
    );

    // Update reduce descriptors
    for level in 1..mip_levels {
        let src_lvl_info = vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: pyramid_mip_views[(level - 1) as usize],
            image_layout: vk::ImageLayout::General,
        };
        let dst_lvl_info = vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: pyramid_mip_views[level as usize],
            image_layout: vk::ImageLayout::General,
        };
        let set = reduce_sets[(level - 1) as usize];
        device.update_descriptor_sets(
            &[
                vk::WriteDescriptorSet {
                    dst_set: set,
                    dst_binding: 0,
                    descriptor_type: vk::DescriptorType::StorageImage,
                    descriptor_count: 1,
                    image_info: &src_lvl_info,
                    ..Default::default()
                },
                vk::WriteDescriptorSet {
                    dst_set: set,
                    dst_binding: 1,
                    descriptor_type: vk::DescriptorType::StorageImage,
                    descriptor_count: 1,
                    image_info: &dst_lvl_info,
                    ..Default::default()
                },
            ],
            &[],
        );
    }

    HizPyramidResources {
        pyramid_image,
        pyramid_allocation: Some(pyramid_allocation),
        pyramid_sampler,
        pyramid_mip_levels: mip_levels,
        pyramid_mip_views,
        pyramid_full_view,
        copy_set,
        reduce_sets,
        desc_pool,
    }
}

fn destroy_frame_resources(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    resources: &mut HizPyramidResources,
) {
    device.destroy_descriptor_pool(resources.desc_pool, None);
    device.destroy_image_view(resources.pyramid_full_view, None);
    for view in resources.pyramid_mip_views.drain(..) {
        device.destroy_image_view(view, None);
    }
    device.destroy_image(resources.pyramid_image, None);
    device.destroy_sampler(resources.pyramid_sampler, None);
    if let Some(alloc) = resources.pyramid_allocation.take() {
        allocator.lock().unwrap().free(alloc).ok();
    }
}
