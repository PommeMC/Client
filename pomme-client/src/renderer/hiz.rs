//! GPU Hi-Z (hierarchical max-depth) pyramid used for chunk occlusion culling.
//!
//! Each frame the renderer draws the visible chunk set depth-only, then this
//! module reduces that depth into a max-depth mip chain. The next frame's cull
//! compute shader projects every section's AABB into screen space and rejects
//! it if it lies fully behind the recorded occluder depth (1-frame latency).
//!
//! The image is kept in `GENERAL` layout for its whole life (it is both written
//! as a storage image during the build and sampled during the build/cull),
//! which keeps the layout bookkeeping trivial.

use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

const FORMAT: vk::Format = vk::Format::R32Sfloat;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Push {
    dst_w: i32,
    dst_h: i32,
    src_lod: i32,
}

pub struct HiZPyramid {
    image: vk::Image,
    allocation: Allocation,
    /// Samples the whole chain (used by the cull shader and by build passes
    /// that read the previous mip).
    sampled_view: vk::ImageView,
    /// One storage view per mip (the build writes into these).
    mip_views: Vec<vk::ImageView>,
    sampler: vk::Sampler,
    mip_count: u32,
    mip_sizes: Vec<(u32, u32)>,

    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    desc_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    /// One descriptor set per mip: {binding0 src sampler, binding1 dst
    /// storage}.
    sets: Vec<vk::DescriptorSet>,

    /// A coarse mip copied to the CPU each frame so the mesh scheduler can skip
    /// columns that are fully behind nearer terrain (frame-lagged, like the
    /// cull count readback). One host buffer per frame-in-flight.
    readback_mip: u32,
    readback_dims: (u32, u32),
    readback_buffers: Vec<vk::Buffer>,
    readback_allocs: Vec<Allocation>,
}

impl HiZPyramid {
    pub fn new(
        device: &vk::Device,
        allocator: &Arc<Mutex<Allocator>>,
        depth_extent: vk::Extent2D,
        depth_view: vk::ImageView,
    ) -> Self {
        // Mip 0 is half the framebuffer resolution; the whole pyramid then halves
        // down to 1x1.
        let w0 = (depth_extent.width / 2).max(1);
        let h0 = (depth_extent.height / 2).max(1);
        let mip_count = (32 - w0.max(h0).leading_zeros()).max(1);

        let mut mip_sizes = Vec::with_capacity(mip_count as usize);
        for m in 0..mip_count {
            mip_sizes.push(((w0 >> m).max(1), (h0 >> m).max(1)));
        }

        // Finest mip whose width <= 192: small enough to copy/read every frame,
        // still fine enough to tell whether a 16-wide column is fully occluded.
        let readback_mip = mip_sizes
            .iter()
            .position(|&(w, _)| w <= 192)
            .unwrap_or(mip_count as usize - 1) as u32;
        let readback_dims = mip_sizes[readback_mip as usize];
        let readback_bytes = (readback_dims.0 * readback_dims.1 * 4) as u64;

        let mut readback_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut readback_allocs = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (b, a) = util::create_host_buffer(
                device,
                allocator,
                readback_bytes,
                vk::BufferUsageFlags::TransferDst,
                "hiz_readback",
            );
            readback_buffers.push(b);
            readback_allocs.push(a);
        }

        let image_info = vk::ImageCreateInfo {
            image_type: vk::ImageType::Type2D,
            format: FORMAT,
            extent: vk::Extent3D {
                width: w0,
                height: h0,
                depth: 1,
            },
            mip_levels: mip_count,
            array_layers: 1,
            samples: vk::SampleCountFlags::Type1,
            tiling: vk::ImageTiling::Optimal,
            usage: vk::ImageUsageFlags::Sampled | vk::ImageUsageFlags::Storage,
            ..Default::default()
        };
        let image = device.create_image(&image_info, None).expect("hiz image");
        let mem_reqs = device.get_image_memory_requirements(image);
        let allocation = allocator
            .lock()
            .unwrap()
            .allocate(&AllocationCreateDesc {
                name: "hiz_pyramid",
                requirements: mem_reqs,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .expect("hiz alloc");
        unsafe {
            device
                .bind_image_memory(image, allocation.memory(), allocation.offset())
                .expect("hiz bind");
        }

        let sampled_view = device
            .create_image_view(
                &vk::ImageViewCreateInfo {
                    image,
                    view_type: vk::ImageViewType::Type2D,
                    format: FORMAT,
                    subresource_range: vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::Color,
                        base_mip_level: 0,
                        level_count: mip_count,
                        base_array_layer: 0,
                        layer_count: 1,
                    },
                    ..Default::default()
                },
                None,
            )
            .expect("hiz sampled view");

        let mip_views: Vec<vk::ImageView> = (0..mip_count)
            .map(|m| {
                device
                    .create_image_view(
                        &vk::ImageViewCreateInfo {
                            image,
                            view_type: vk::ImageViewType::Type2D,
                            format: FORMAT,
                            subresource_range: vk::ImageSubresourceRange {
                                aspect_mask: vk::ImageAspectFlags::Color,
                                base_mip_level: m,
                                level_count: 1,
                                base_array_layer: 0,
                                layer_count: 1,
                            },
                            ..Default::default()
                        },
                        None,
                    )
                    .expect("hiz mip view")
            })
            .collect();

        let sampler = device
            .create_sampler(
                &vk::SamplerCreateInfo {
                    mag_filter: vk::Filter::Nearest,
                    min_filter: vk::Filter::Nearest,
                    mipmap_mode: vk::SamplerMipmapMode::Nearest,
                    address_mode_u: vk::SamplerAddressMode::ClampToEdge,
                    address_mode_v: vk::SamplerAddressMode::ClampToEdge,
                    address_mode_w: vk::SamplerAddressMode::ClampToEdge,
                    min_lod: 0.0,
                    max_lod: mip_count as f32,
                    ..Default::default()
                },
                None,
            )
            .expect("hiz sampler");

        // Descriptor layout + compute pipeline.
        let bindings = [
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
        let desc_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo {
                    binding_count: bindings.len() as u32,
                    bindings: bindings.as_ptr(),
                    ..Default::default()
                },
                None,
            )
            .expect("hiz desc layout");

        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Compute,
            offset: 0,
            size: size_of::<Push>() as u32,
        };
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo {
                    set_layout_count: 1,
                    set_layouts: &desc_layout,
                    push_constant_range_count: 1,
                    push_constant_ranges: &push_range,
                    ..Default::default()
                },
                None,
            )
            .expect("hiz pipeline layout");

        let comp_spv = shader::include_spirv!("hiz_downsample.comp.spv");
        let comp_module = shader::create_shader_module(device, comp_spv);
        let stage = vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Compute,
            module: comp_module,
            name: c"main".as_ptr(),
            ..Default::default()
        };
        let mut pipeline = vk::Pipeline::null();
        device
            .create_compute_pipelines(
                vk::PipelineCache::null(),
                &[vk::ComputePipelineCreateInfo {
                    stage,
                    layout,
                    ..Default::default()
                }],
                None,
                std::slice::from_mut(&mut pipeline),
            )
            .expect("hiz pipeline");
        device.destroy_shader_module(comp_module, None);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: mip_count,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::StorageImage,
                descriptor_count: mip_count,
            },
        ];
        let pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo {
                    max_sets: mip_count,
                    pool_size_count: pool_sizes.len() as u32,
                    pool_sizes: pool_sizes.as_ptr(),
                    ..Default::default()
                },
                None,
            )
            .expect("hiz desc pool");

        let set_layouts: Vec<_> = (0..mip_count).map(|_| desc_layout).collect();
        let mut sets = vec![vk::DescriptorSet::null(); mip_count as usize];
        device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo {
                    descriptor_pool: pool,
                    descriptor_set_count: mip_count,
                    set_layouts: set_layouts.as_ptr(),
                    ..Default::default()
                },
                &mut sets,
            )
            .expect("hiz desc sets");

        // mip 0 reads the scene depth (shader-read layout); mips >0 read the
        // previous Hi-Z mip (general layout, via the full sampled view).
        for m in 0..mip_count as usize {
            let (src_view, src_layout) = if m == 0 {
                (depth_view, vk::ImageLayout::ShaderReadOnlyOptimal)
            } else {
                (sampled_view, vk::ImageLayout::General)
            };
            let src_info = [vk::DescriptorImageInfo {
                sampler,
                image_view: src_view,
                image_layout: src_layout,
            }];
            let dst_info = [vk::DescriptorImageInfo {
                sampler: vk::Sampler::null(),
                image_view: mip_views[m],
                image_layout: vk::ImageLayout::General,
            }];
            let writes = [
                vk::WriteDescriptorSet {
                    dst_set: sets[m],
                    dst_binding: 0,
                    descriptor_count: 1,
                    descriptor_type: vk::DescriptorType::CombinedImageSampler,
                    image_info: src_info.as_ptr(),
                    ..Default::default()
                },
                vk::WriteDescriptorSet {
                    dst_set: sets[m],
                    dst_binding: 1,
                    descriptor_count: 1,
                    descriptor_type: vk::DescriptorType::StorageImage,
                    image_info: dst_info.as_ptr(),
                    ..Default::default()
                },
            ];
            device.update_descriptor_sets(&writes, &[]);
        }

        Self {
            image,
            allocation,
            sampled_view,
            mip_views,
            sampler,
            mip_count,
            mip_sizes,
            pipeline,
            layout,
            desc_layout,
            pool,
            sets,
            readback_mip,
            readback_dims,
            readback_buffers,
            readback_allocs,
        }
    }

    pub fn sampled_view(&self) -> vk::ImageView {
        self.sampled_view
    }

    pub fn sampler(&self) -> vk::Sampler {
        self.sampler
    }

    /// Dimensions of the coarse mip exposed for CPU occlusion readback.
    pub fn readback_dims(&self) -> (u32, u32) {
        self.readback_dims
    }

    /// Copy the coarse readback mip into this frame's host buffer. Must be
    /// recorded after `build`; the next frame's WAR barrier waits on this read.
    pub fn record_readback(&self, cmd: vk::CommandBuffer, frame: usize) {
        // The build wrote the mip (compute ShaderWrite); make it visible to the
        // transfer read. The pyramid stays in General, valid as a transfer src.
        let to_transfer = vk::ImageMemoryBarrier {
            image: self.image,
            old_layout: vk::ImageLayout::General,
            new_layout: vk::ImageLayout::General,
            src_access_mask: vk::AccessFlags::ShaderWrite,
            dst_access_mask: vk::AccessFlags::TransferRead,
            subresource_range: self.mip_range(self.readback_mip),
            ..Default::default()
        };
        cmd.pipeline_barrier(
            vk::PipelineStageFlags::ComputeShader,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_transfer],
        );

        let (w, h) = self.readback_dims;
        let region = vk::BufferImageCopy {
            buffer_offset: 0,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: self.readback_mip,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: w,
                height: h,
                depth: 1,
            },
        };
        cmd.copy_image_to_buffer(
            self.image,
            vk::ImageLayout::General,
            self.readback_buffers[frame],
            &[region],
        );
    }

    /// Read the coarse mip copied for `frame` (valid once that frame's fence
    /// has been waited). Row-major max-depth, `readback_dims` wide.
    pub fn read_mip(&mut self, frame: usize) -> Vec<f32> {
        let (w, h) = self.readback_dims;
        let n = (w * h) as usize;
        let bytes = self.readback_allocs[frame].mapped_slice_mut().unwrap();
        bytemuck::cast_slice::<u8, f32>(&bytes[..n * 4]).to_vec()
    }

    /// Record the pyramid build. `depth` must already be in
    /// `ShaderReadOnlyOptimal` and the WAR hazard against the previous
    /// frame's cull read must already be handled by the caller. `first_use`
    /// performs the one-time Undefined->General layout init (the image then
    /// stays in General for its whole life).
    pub fn build(&self, cmd: vk::CommandBuffer, first_use: bool) {
        let compute = vk::PipelineStageFlags::ComputeShader;
        if first_use {
            self.barrier(
                cmd,
                vk::ImageLayout::Undefined,
                vk::PipelineStageFlags::TopOfPipe,
                vk::AccessFlags::empty(),
                vk::AccessFlags::ShaderWrite,
                self.full_range(),
            );
        } else {
            // WAR: this frame's cull sampled the pyramid (ShaderRead) and the
            // previous frame's readback copied a mip (TransferRead); finish both
            // before overwriting it.
            self.barrier(
                cmd,
                vk::ImageLayout::General,
                compute | vk::PipelineStageFlags::Transfer,
                vk::AccessFlags::ShaderRead | vk::AccessFlags::TransferRead,
                vk::AccessFlags::ShaderWrite,
                self.full_range(),
            );
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Compute, self.pipeline);
        for m in 0..self.mip_count as usize {
            if m > 0 {
                // Make mip m-1's writes visible to this pass's reads.
                self.barrier(
                    cmd,
                    vk::ImageLayout::General,
                    compute,
                    vk::AccessFlags::ShaderWrite,
                    vk::AccessFlags::ShaderRead,
                    self.mip_range(m as u32 - 1),
                );
            }

            let (w, h) = self.mip_sizes[m];
            cmd.bind_descriptor_sets(
                vk::PipelineBindPoint::Compute,
                self.layout,
                0,
                &[self.sets[m]],
                &[],
            );
            let push = Push {
                dst_w: w as i32,
                dst_h: h as i32,
                src_lod: if m == 0 { 0 } else { m as i32 - 1 },
            };
            cmd.push_constants(
                self.layout,
                vk::ShaderStageFlags::Compute,
                0,
                bytemuck::bytes_of(&push),
            );
            cmd.dispatch(w.div_ceil(8), h.div_ceil(8), 1);
        }

        // Make the whole pyramid readable by the next frame's cull sampling.
        self.barrier(
            cmd,
            vk::ImageLayout::General,
            vk::PipelineStageFlags::ComputeShader,
            vk::AccessFlags::ShaderWrite,
            vk::AccessFlags::ShaderRead,
            self.full_range(),
        );
    }

    /// Pipeline barrier on the pyramid image. The pyramid lives in `General`,
    /// so `src_layout` is also the destination layout for everything after
    /// init.
    fn barrier(
        &self,
        cmd: vk::CommandBuffer,
        src_layout: vk::ImageLayout,
        src_stage: vk::PipelineStageFlags,
        src_access: vk::AccessFlags,
        dst_access: vk::AccessFlags,
        range: vk::ImageSubresourceRange,
    ) {
        let barrier = vk::ImageMemoryBarrier {
            image: self.image,
            old_layout: src_layout,
            new_layout: vk::ImageLayout::General,
            src_access_mask: src_access,
            dst_access_mask: dst_access,
            subresource_range: range,
            ..Default::default()
        };
        cmd.pipeline_barrier(
            src_stage,
            vk::PipelineStageFlags::ComputeShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }

    fn full_range(&self) -> vk::ImageSubresourceRange {
        self.mip_range_n(0, self.mip_count)
    }

    fn mip_range(&self, mip: u32) -> vk::ImageSubresourceRange {
        self.mip_range_n(mip, 1)
    }

    fn mip_range_n(&self, base: u32, count: u32) -> vk::ImageSubresourceRange {
        vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: base,
            level_count: count,
            base_array_layer: 0,
            layer_count: 1,
        }
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.layout, None);
        device.destroy_descriptor_pool(self.pool, None);
        device.destroy_descriptor_set_layout(self.desc_layout, None);
        device.destroy_sampler(self.sampler, None);
        for &v in &self.mip_views {
            device.destroy_image_view(v, None);
        }
        device.destroy_image_view(self.sampled_view, None);
        device.destroy_image(self.image, None);

        {
            let mut alloc = allocator.lock().unwrap();
            for i in 0..self.readback_buffers.len() {
                device.destroy_buffer(self.readback_buffers[i], None);
                alloc
                    .free(std::mem::replace(&mut self.readback_allocs[i], unsafe {
                        std::mem::zeroed()
                    }))
                    .ok();
            }
        }

        allocator
            .lock()
            .unwrap()
            .free(std::mem::replace(&mut self.allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
    }
}
