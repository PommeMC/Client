use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::MemoryLocation;
use pomme_gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::shader;
use crate::renderer::util;

// Minecraft panorama face order differs from Vulkan cubemap layer order
const FACE_TO_LAYER: [u32; 6] = [4, 1, 5, 0, 2, 3];

pub struct PanoramaPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    params_layout: vk::DescriptorSetLayout,
    cube_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    params_set: vk::DescriptorSet,
    cube_set: vk::DescriptorSet,
    params_buffer: vk::Buffer,
    params_allocation: Option<Allocation>,
    cube_image: vk::Image,
    cube_view: vk::ImageView,
    cube_sampler: vk::Sampler,
    cube_allocation: Option<Allocation>,
    staging_buffer: vk::Buffer,
    staging_allocation: Option<Allocation>,
    has_cubemap: bool,
}

impl PanoramaPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let params_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Fragment,
        );
        let cube_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [params_layout, cube_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create panorama pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: 2,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create panorama descriptor pool");

        let params_alloc = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &params_layout,
            ..Default::default()
        };
        let mut params_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&params_alloc, slice::from_mut(&mut params_set))
            .expect("failed to allocate params descriptor set");

        let cube_alloc = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &cube_layout,
            ..Default::default()
        };
        let mut cube_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&cube_alloc, slice::from_mut(&mut cube_set))
            .expect("failed to allocate cube descriptor set");

        let (params_buffer, params_allocation) =
            util::create_uniform_buffer(device, allocator, 16, "panorama_params");

        let buffer_info = vk::DescriptorBufferInfo {
            buffer: params_buffer,
            offset: 0,
            range: 16,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: params_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            buffer_info: &buffer_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);

        let (
            cube_image,
            cube_view,
            cube_sampler,
            cube_alloc_mem,
            staging_buffer,
            staging_alloc_mem,
            has_cubemap,
        ) = load_cubemap(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let image_info = vk::DescriptorImageInfo {
            sampler: cube_sampler,
            image_view: cube_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let cube_write = vk::WriteDescriptorSet {
            dst_set: cube_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[cube_write], &[]);

        Self {
            pipeline,
            pipeline_layout,
            params_layout,
            cube_layout,
            descriptor_pool,
            params_set,
            cube_set,
            params_buffer,
            params_allocation: Some(params_allocation),
            cube_image,
            cube_view,
            cube_sampler,
            cube_allocation: Some(cube_alloc_mem),
            staging_buffer,
            staging_allocation: Some(staging_alloc_mem),
            has_cubemap,
        }
    }

    pub fn draw(
        &mut self,
        device: &vk::Device,
        cmd: vk::CommandBuffer,
        scroll: f32,
        aspect: f32,
        blur: f32,
    ) {
        let _ = device;
        if !self.has_cubemap {
            return;
        }

        let data: [f32; 4] = [scroll, aspect, blur, 0.0];
        self.params_allocation
            .as_mut()
            .unwrap()
            .mapped_slice_mut()
            .unwrap()[..16]
            .copy_from_slice(bytemuck::cast_slice(&data));

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.params_set, self.cube_set],
            &[],
        );
        cmd.draw(3, 1, 0, 0);
    }

    pub fn reload_cubemap(
        &mut self,
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<AssetIndex>,
    ) {
        let _ = device.wait_idle();

        {
            let mut alloc = allocator.lock().unwrap();
            device.destroy_sampler(self.cube_sampler, None);
            device.destroy_image_view(self.cube_view, None);
            if let Some(a) = self.cube_allocation.take() {
                alloc.free(a).ok();
            }
            device.destroy_image(self.cube_image, None);
            if let Some(a) = self.staging_allocation.take() {
                alloc.free(a).ok();
            }
            device.destroy_buffer(self.staging_buffer, None);
        }

        let (
            cube_image,
            cube_view,
            cube_sampler,
            cube_alloc,
            staging_buffer,
            staging_alloc,
            has_cubemap,
        ) = load_cubemap(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        self.cube_image = cube_image;
        self.cube_view = cube_view;
        self.cube_sampler = cube_sampler;
        self.cube_allocation = Some(cube_alloc);
        self.staging_buffer = staging_buffer;
        self.staging_allocation = Some(staging_alloc);
        self.has_cubemap = has_cubemap;

        let image_info = vk::DescriptorImageInfo {
            sampler: self.cube_sampler,
            image_view: self.cube_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let cube_write = vk::WriteDescriptorSet {
            dst_set: self.cube_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[cube_write], &[]);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        device.destroy_buffer(self.params_buffer, None);
        if let Some(a) = self.params_allocation.take() {
            alloc.free(a).ok();
        }

        device.destroy_sampler(self.cube_sampler, None);
        device.destroy_image_view(self.cube_view, None);
        if let Some(a) = self.cube_allocation.take() {
            alloc.free(a).ok();
        }
        device.destroy_image(self.cube_image, None);

        if let Some(a) = self.staging_allocation.take() {
            alloc.free(a).ok();
        }
        device.destroy_buffer(self.staging_buffer, None);

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.params_layout, None);
        device.destroy_descriptor_set_layout(self.cube_layout, None);
    }
}

fn resolve_panorama_face(
    i: u32,
    jar_assets_dir: &std::path::Path,
    asset_index: &Option<AssetIndex>,
) -> Option<std::path::PathBuf> {
    let flat = jar_assets_dir.join(format!("panorama_{i}.png"));
    if flat.exists() {
        return Some(flat);
    }
    let asset_key = format!("minecraft/textures/gui/title/background/panorama_{i}.png");
    let path = resolve_asset_path(jar_assets_dir, asset_index, &asset_key);
    path.exists().then_some(path)
}

fn flip_horizontal(data: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let stride = (w * 4) as usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let src = y * stride + x * 4;
            let dst = y * stride + (w as usize - 1 - x) * 4;
            out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
        }
    }
    out
}

fn load_cubemap(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &std::path::Path,
    asset_index: &Option<AssetIndex>,
) -> (
    vk::Image,
    vk::ImageView,
    vk::Sampler,
    Allocation,
    vk::Buffer,
    Allocation,
    bool,
) {
    let mut faces: Vec<Vec<u8>> = Vec::new();
    let mut face_w = 0u32;
    let mut face_h = 0u32;

    for i in 0..6 {
        let path = match resolve_panorama_face(i, jar_assets_dir, asset_index) {
            Some(p) => p,
            None => {
                tracing::info!("Panorama face {i} not found, skipping cubemap");
                return create_fallback_cubemap(device, allocator);
            }
        };
        match util::load_png(&path) {
            Some((data, w, h)) if w > 1 && h > 1 => {
                face_w = w;
                face_h = h;
                faces.push(data);
            }
            _ => {
                tracing::info!("Panorama face {i} is a placeholder, skipping cubemap");
                return create_fallback_cubemap(device, allocator);
            }
        }
    }

    let face_bytes = (face_w * face_h * 4) as usize;
    let mut staging_data = vec![0u8; face_bytes * 6];

    for (panorama_idx, face_data) in faces.iter().enumerate() {
        let layer = FACE_TO_LAYER[panorama_idx] as usize;
        let flipped = flip_horizontal(face_data, face_w, face_h);
        staging_data[layer * face_bytes..(layer + 1) * face_bytes].copy_from_slice(&flipped);
    }

    let (image, allocation) = create_cubemap_image(device, allocator, face_w, face_h);
    let (staging_buffer, staging_allocation) =
        util::create_staging_buffer(device, allocator, &staging_data, "panorama_cubemap_staging");

    upload_cubemap(
        device,
        queue,
        command_pool,
        staging_buffer,
        image,
        face_w,
        face_h,
    );

    let mip_levels = mip_levels_for(face_w, face_h);
    let view = create_cubemap_view(device, image, mip_levels);

    let sampler_info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Linear,
        min_filter: vk::Filter::Linear,
        mipmap_mode: vk::SamplerMipmapMode::Linear,
        address_mode_u: vk::SamplerAddressMode::ClampToEdge,
        address_mode_v: vk::SamplerAddressMode::ClampToEdge,
        address_mode_w: vk::SamplerAddressMode::ClampToEdge,
        max_lod: mip_levels as f32,
        ..Default::default()
    };
    let sampler = device
        .create_sampler(&sampler_info, None)
        .expect("failed to create cubemap sampler");

    tracing::info!("Panorama cubemap loaded: {face_w}x{face_h} per face, {mip_levels} mip levels");

    (
        image,
        view,
        sampler,
        allocation,
        staging_buffer,
        staging_allocation,
        true,
    )
}

fn mip_levels_for(w: u32, h: u32) -> u32 {
    (w.max(h) as f32).log2().floor() as u32 + 1
}

fn create_cubemap_image(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
) -> (vk::Image, Allocation) {
    let mip_levels = mip_levels_for(width, height);
    let image_info = vk::ImageCreateInfo {
        image_type: vk::ImageType::Type2D,
        format: vk::Format::R8G8B8A8Srgb,
        extent: vk::Extent3D {
            width,
            height,
            depth: 1,
        },
        mip_levels,
        array_layers: 6,
        samples: vk::SampleCountFlags::Type1,
        tiling: vk::ImageTiling::Optimal,
        usage: vk::ImageUsageFlags::TransferDst
            | vk::ImageUsageFlags::TransferSrc
            | vk::ImageUsageFlags::Sampled,
        flags: vk::ImageCreateFlags::CubeCompatible,
        ..Default::default()
    };

    let image = device
        .create_image(&image_info, None)
        .expect("failed to create cubemap image");
    let mem_reqs = device.get_image_memory_requirements(image);

    let allocation = allocator
        .lock()
        .unwrap()
        .allocate(&AllocationCreateDesc {
            name: "panorama_cubemap",
            requirements: mem_reqs,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        })
        .expect("failed to allocate cubemap memory");

    unsafe {
        device
            .bind_image_memory(image, allocation.memory(), allocation.offset())
            .expect("failed to bind cubemap memory");
    }

    (image, allocation)
}

fn create_cubemap_view(device: &vk::Device, image: vk::Image, mip_levels: u32) -> vk::ImageView {
    let view_info = vk::ImageViewCreateInfo {
        image,
        view_type: vk::ImageViewType::Cube,
        format: vk::Format::R8G8B8A8Srgb,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 6,
        },
        ..Default::default()
    };
    device
        .create_image_view(&view_info, None)
        .expect("failed to create cubemap view")
}

fn upload_cubemap(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    staging_buffer: vk::Buffer,
    image: vk::Image,
    face_w: u32,
    face_h: u32,
) {
    let mip_levels = mip_levels_for(face_w, face_h);

    let alloc_info = vk::CommandBufferAllocateInfo {
        command_pool,
        level: vk::CommandBufferLevel::Primary,
        command_buffer_count: 1,
        ..Default::default()
    };
    let mut cmd = vk::CommandBuffer::null();
    unsafe { device.allocate_command_buffers(&alloc_info, slice::from_mut(&mut cmd)) }
        .expect("failed to allocate upload cmd");

    let begin = vk::CommandBufferBeginInfo {
        flags: vk::CommandBufferUsageFlags::OneTimeSubmit,
        ..Default::default()
    };
    cmd.begin(&begin).expect("failed to begin cmd");

    let all_mips_range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::Color,
        base_mip_level: 0,
        level_count: mip_levels,
        base_array_layer: 0,
        layer_count: 6,
    };

    let barrier_to_transfer = vk::ImageMemoryBarrier {
        image,
        old_layout: vk::ImageLayout::Undefined,
        new_layout: vk::ImageLayout::TransferDstOptimal,
        src_access_mask: vk::AccessFlags::empty(),
        dst_access_mask: vk::AccessFlags::TransferWrite,
        subresource_range: all_mips_range,
        ..Default::default()
    };

    cmd.pipeline_barrier(
        vk::PipelineStageFlags::TopOfPipe,
        vk::PipelineStageFlags::Transfer,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier_to_transfer],
    );

    let face_bytes = (face_w * face_h * 4) as u64;
    let regions: Vec<vk::BufferImageCopy> = (0..6)
        .map(|layer| vk::BufferImageCopy {
            buffer_offset: layer as u64 * face_bytes,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: 0,
                base_array_layer: layer,
                layer_count: 1,
            },
            image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
            image_extent: vk::Extent3D {
                width: face_w,
                height: face_h,
                depth: 1,
            },
        })
        .collect();

    cmd.copy_buffer_to_image(
        staging_buffer,
        image,
        vk::ImageLayout::TransferDstOptimal,
        &regions,
    );

    let mut mip_w = face_w as i32;
    let mut mip_h = face_h as i32;

    for level in 1..mip_levels {
        let barrier_src = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::TransferDstOptimal,
            new_layout: vk::ImageLayout::TransferSrcOptimal,
            src_access_mask: vk::AccessFlags::TransferWrite,
            dst_access_mask: vk::AccessFlags::TransferRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: level - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 6,
            },
            ..Default::default()
        };

        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::Transfer,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_src],
        );

        let next_w = (mip_w / 2).max(1);
        let next_h = (mip_h / 2).max(1);

        let blit = vk::ImageBlit {
            src_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: level - 1,
                base_array_layer: 0,
                layer_count: 6,
            },
            src_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: mip_w,
                    y: mip_h,
                    z: 1,
                },
            ],
            dst_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::Color,
                mip_level: level,
                base_array_layer: 0,
                layer_count: 6,
            },
            dst_offsets: [
                vk::Offset3D { x: 0, y: 0, z: 0 },
                vk::Offset3D {
                    x: next_w,
                    y: next_h,
                    z: 1,
                },
            ],
        };

        cmd.blit_image(
            image,
            vk::ImageLayout::TransferSrcOptimal,
            image,
            vk::ImageLayout::TransferDstOptimal,
            &[blit],
            vk::Filter::Linear,
        );

        let barrier_read = vk::ImageMemoryBarrier {
            image,
            old_layout: vk::ImageLayout::TransferSrcOptimal,
            new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
            src_access_mask: vk::AccessFlags::TransferRead,
            dst_access_mask: vk::AccessFlags::ShaderRead,
            subresource_range: vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::Color,
                base_mip_level: level - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 6,
            },
            ..Default::default()
        };

        cmd.pipeline_barrier(
            vk::PipelineStageFlags::Transfer,
            vk::PipelineStageFlags::FragmentShader,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_read],
        );

        mip_w = next_w;
        mip_h = next_h;
    }

    let barrier_last = vk::ImageMemoryBarrier {
        image,
        old_layout: vk::ImageLayout::TransferDstOptimal,
        new_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        src_access_mask: vk::AccessFlags::TransferWrite,
        dst_access_mask: vk::AccessFlags::ShaderRead,
        subresource_range: vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::Color,
            base_mip_level: mip_levels - 1,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 6,
        },
        ..Default::default()
    };

    cmd.pipeline_barrier(
        vk::PipelineStageFlags::Transfer,
        vk::PipelineStageFlags::FragmentShader,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[barrier_last],
    );
    cmd.end().expect("failed to end cmd");

    let submit = vk::SubmitInfo {
        command_buffer_count: 1,
        command_buffers: &cmd.handle(),
        ..Default::default()
    };

    queue
        .submit(&[submit], vk::Fence::null())
        .expect("failed to submit cubemap upload");
    queue
        .wait_idle()
        .expect("failed to wait for cubemap upload");
    device.free_command_buffers(command_pool, &[cmd.handle()]);
}

fn create_fallback_cubemap(
    device: &vk::Device,
    allocator: &Arc<Mutex<Allocator>>,
) -> (
    vk::Image,
    vk::ImageView,
    vk::Sampler,
    Allocation,
    vk::Buffer,
    Allocation,
    bool,
) {
    let pixels = vec![0u8; 4 * 6];
    let (image, allocation) = create_cubemap_image(device, allocator, 1, 1);
    let view = create_cubemap_view(device, image, 1);
    let (staging_buffer, staging_allocation) =
        util::create_staging_buffer(device, allocator, &pixels, "panorama_fallback_staging");

    let sampler_info = vk::SamplerCreateInfo {
        mag_filter: vk::Filter::Linear,
        min_filter: vk::Filter::Linear,
        ..Default::default()
    };
    let sampler = device
        .create_sampler(&sampler_info, None)
        .expect("failed to create fallback sampler");

    (
        image,
        view,
        sampler,
        allocation,
        staging_buffer,
        staging_allocation,
        false,
    )
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("panorama.vert.spv");
    let frag_spv = shader::include_spirv!("panorama.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::None,
        line_width: 1.0,
        ..Default::default()
    };

    let multisampling = vk::PipelineMultisampleStateCreateInfo {
        rasterization_samples: vk::SampleCountFlags::Type1,
        ..Default::default()
    };

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::FALSE,
        depth_write_enable: vk::FALSE,
        ..Default::default()
    };

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blending = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    let dynamic_states = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo {
        dynamic_state_count: dynamic_states.len() as u32,
        dynamic_states: dynamic_states.as_ptr(),
        ..Default::default()
    };

    let pipeline_info = [vk::GraphicsPipelineCreateInfo {
        stage_count: stages.len() as u32,
        stages: stages.as_ptr(),
        vertex_input_state: &vertex_input,
        input_assembly_state: &input_assembly,
        viewport_state: &viewport_state,
        rasterization_state: &rasterizer,
        multisample_state: &multisampling,
        depth_stencil_state: &depth_stencil,
        color_blend_state: &color_blending,
        dynamic_state: &dynamic_state,
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create panorama pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}
