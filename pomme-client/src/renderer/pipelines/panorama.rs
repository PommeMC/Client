use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator};

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
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let params_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::FRAGMENT,
        );
        let cube_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [params_layout, cube_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create panorama pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: 1,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(2)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create panorama descriptor pool");

        let params_layouts = [params_layout];
        let params_alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&params_layouts);
        let params_set = unsafe { device.allocate_descriptor_sets(&params_alloc) }
            .expect("failed to allocate params descriptor set")[0];

        let cube_layouts = [cube_layout];
        let cube_alloc = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&cube_layouts);
        let cube_set = unsafe { device.allocate_descriptor_sets(&cube_alloc) }
            .expect("failed to allocate cube descriptor set")[0];

        let (params_buffer, params_allocation) =
            util::create_uniform_buffer(device, allocator, 16, "panorama_params");

        let buffer_info = [vk::DescriptorBufferInfo {
            buffer: params_buffer,
            offset: 0,
            range: 16,
        }];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(params_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&buffer_info);
        unsafe { device.update_descriptor_sets(&[write], &[]) };

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

        let image_info = [vk::DescriptorImageInfo {
            sampler: cube_sampler,
            image_view: cube_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let cube_write = vk::WriteDescriptorSet::default()
            .dst_set(cube_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[cube_write], &[]) };

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
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        scroll: f32,
        aspect: f32,
        blur: f32,
    ) {
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

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.params_set, self.cube_set],
                &[],
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    pub fn reload_cubemap(
        &mut self,
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &std::path::Path,
        asset_index: &Option<AssetIndex>,
    ) {
        unsafe {
            let _ = device.device_wait_idle();
        }

        {
            let mut alloc = allocator.lock().unwrap();
            unsafe {
                device.destroy_sampler(self.cube_sampler, None);
            }
            unsafe {
                device.destroy_image_view(self.cube_view, None);
            }
            if let Some(a) = self.cube_allocation.take() {
                alloc.free(a).ok();
            }
            unsafe {
                device.destroy_image(self.cube_image, None);
            }
            if let Some(a) = self.staging_allocation.take() {
                alloc.free(a).ok();
            }
            unsafe {
                device.destroy_buffer(self.staging_buffer, None);
            }
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

        let image_info = [vk::DescriptorImageInfo {
            sampler: self.cube_sampler,
            image_view: self.cube_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let cube_write = vk::WriteDescriptorSet::default()
            .dst_set(self.cube_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[cube_write], &[]) };
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();

        unsafe { device.destroy_buffer(self.params_buffer, None) };
        if let Some(a) = self.params_allocation.take() {
            alloc.free(a).ok();
        }

        unsafe {
            device.destroy_sampler(self.cube_sampler, None);
            device.destroy_image_view(self.cube_view, None);
        }
        if let Some(a) = self.cube_allocation.take() {
            alloc.free(a).ok();
        }
        unsafe { device.destroy_image(self.cube_image, None) };

        if let Some(a) = self.staging_allocation.take() {
            alloc.free(a).ok();
        }
        unsafe { device.destroy_buffer(self.staging_buffer, None) };

        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.params_layout, None);
            device.destroy_descriptor_set_layout(self.cube_layout, None);
        }
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
    device: &ash::Device,
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

    let sampler_info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR)
        .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
        .max_lod(mip_levels as f32);
    let sampler = unsafe { device.create_sampler(&sampler_info, None) }
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
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    width: u32,
    height: u32,
) -> (vk::Image, Allocation) {
    let mip_levels = mip_levels_for(width, height);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_SRGB)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(mip_levels)
        .array_layers(6)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(
            vk::ImageUsageFlags::TRANSFER_DST
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::SAMPLED,
        )
        .flags(vk::ImageCreateFlags::CUBE_COMPATIBLE);

    let image =
        unsafe { device.create_image(&image_info, None) }.expect("failed to create cubemap image");
    let mem_reqs = unsafe { device.get_image_memory_requirements(image) };

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

fn create_cubemap_view(device: &ash::Device, image: vk::Image, mip_levels: u32) -> vk::ImageView {
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::CUBE)
        .format(vk::Format::R8G8B8A8_SRGB)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: mip_levels,
            base_array_layer: 0,
            layer_count: 6,
        });
    unsafe { device.create_image_view(&view_info, None) }.expect("failed to create cubemap view")
}

fn upload_cubemap(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    staging_buffer: vk::Buffer,
    image: vk::Image,
    face_w: u32,
    face_h: u32,
) {
    let mip_levels = mip_levels_for(face_w, face_h);

    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let cmd = unsafe { device.allocate_command_buffers(&alloc_info) }
        .expect("failed to allocate upload cmd")[0];

    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    unsafe { device.begin_command_buffer(cmd, &begin) }.expect("failed to begin cmd");

    let all_mips_range = vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: mip_levels,
        base_array_layer: 0,
        layer_count: 6,
    };

    let barrier_to_transfer = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .subresource_range(all_mips_range);

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_to_transfer],
        );
    }

    let face_bytes = (face_w * face_h * 4) as u64;
    let regions: Vec<vk::BufferImageCopy> = (0..6)
        .map(|layer| vk::BufferImageCopy {
            buffer_offset: layer as u64 * face_bytes,
            buffer_row_length: 0,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
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

    unsafe {
        device.cmd_copy_buffer_to_image(
            cmd,
            staging_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &regions,
        );
    }

    let mut mip_w = face_w as i32;
    let mut mip_h = face_h as i32;

    for level in 1..mip_levels {
        let barrier_src = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: level - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 6,
            });

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_src],
            );
        }

        let next_w = (mip_w / 2).max(1);
        let next_h = (mip_h / 2).max(1);

        let blit = vk::ImageBlit {
            src_subresource: vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
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
                aspect_mask: vk::ImageAspectFlags::COLOR,
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

        unsafe {
            device.cmd_blit_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[blit],
                vk::Filter::LINEAR,
            );
        }

        let barrier_read = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: level - 1,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 6,
            });

        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier_read],
            );
        }

        mip_w = next_w;
        mip_h = next_h;
    }

    let barrier_last = vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: mip_levels - 1,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 6,
        });

    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier_last],
        );
        device.end_command_buffer(cmd).expect("failed to end cmd");
    }

    let cmd_buffers = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&cmd_buffers);
    unsafe {
        device
            .queue_submit(queue, &[submit], vk::Fence::null())
            .expect("failed to submit cubemap upload");
        device
            .queue_wait_idle(queue)
            .expect("failed to wait for cubemap upload");
        device.free_command_buffers(command_pool, &cmd_buffers);
    }
}

fn create_fallback_cubemap(
    device: &ash::Device,
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

    let sampler_info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::LINEAR)
        .min_filter(vk::Filter::LINEAR);
    let sampler = unsafe { device.create_sampler(&sampler_info, None) }
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
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("panorama.vert.spv");
    let frag_spv = shader::include_spirv!("panorama.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_module)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_module)
            .name(c"main"),
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);

    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .line_width(1.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false);

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blending =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);

    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let pipeline_info = [vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(layout)
        .render_pass(render_pass)
        .subpass(0)];

    let pipeline = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
    }
    .expect("failed to create panorama pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
