use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use azalea_core::position::BlockPos;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::camera::CameraUniform;
use crate::renderer::shader;
use crate::renderer::util;

const STAGE_COUNT: u32 = 10;
const EPSILON: f32 = 0.001;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

pub struct BlockOverlayPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    texture_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    atlas_image: vk::Image,
    atlas_view: vk::ImageView,
    atlas_sampler: vk::Sampler,
    atlas_allocation: Allocation,
}

impl BlockOverlayPipeline {
    pub fn new(
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let texture_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [camera_layout, texture_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create block overlay pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets((MAX_FRAMES_IN_FLIGHT + 1) as u32)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create block overlay descriptor pool");

        let cam_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let cam_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&cam_layouts);
        let camera_sets = unsafe { device.allocate_descriptor_sets(&cam_alloc_info) }
            .expect("failed to allocate block overlay camera sets");

        let tex_layouts = [texture_layout];
        let tex_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&tex_layouts);
        let texture_set = unsafe { device.allocate_descriptor_sets(&tex_alloc_info) }
            .expect("failed to allocate block overlay texture set")[0];

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "block_overlay_camera",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            camera_buffers.push(buf);
            camera_allocations.push(alloc);
        }

        let (atlas_image, atlas_view, atlas_allocation) = load_destroy_atlas(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let atlas_sampler = unsafe { util::create_nearest_sampler(device) };

        let image_info = [vk::DescriptorImageInfo {
            sampler: atlas_sampler,
            image_view: atlas_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let tex_write = vk::WriteDescriptorSet::default()
            .dst_set(texture_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[tex_write], &[]) };

        let placeholder = [OverlayVertex {
            position: [0.0; 3],
            uv: [0.0; 2],
        }; 36];
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&placeholder);
        let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "block_overlay_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            descriptor_pool,
            camera_sets,
            texture_set,
            camera_buffers,
            camera_allocations,
            vertex_buffer,
            vertex_allocation,
            atlas_image,
            atlas_view,
            atlas_sampler,
            atlas_allocation,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        block_pos: &BlockPos,
        stage: u32,
    ) {
        let vertices = build_overlay_vertices(block_pos, stage);
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&vertices);
        self.vertex_allocation.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.camera_sets[frame], self.texture_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_draw(cmd, 36, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.camera_buffers[i], None) };
            alloc
                .free(std::mem::replace(&mut self.camera_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }

        unsafe { device.destroy_buffer(self.vertex_buffer, None) };
        alloc
            .free(std::mem::replace(&mut self.vertex_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        unsafe {
            device.destroy_sampler(self.atlas_sampler, None);
            device.destroy_image_view(self.atlas_view, None);
        }
        alloc
            .free(std::mem::replace(&mut self.atlas_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        unsafe { device.destroy_image(self.atlas_image, None) };

        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.camera_layout, None);
            device.destroy_descriptor_set_layout(self.texture_layout, None);
        }
    }
}

fn build_overlay_vertices(pos: &BlockPos, stage: u32) -> [OverlayVertex; 36] {
    let x0 = pos.x as f32 - EPSILON;
    let y0 = pos.y as f32 - EPSILON;
    let z0 = pos.z as f32 - EPSILON;
    let x1 = pos.x as f32 + 1.0 + EPSILON;
    let y1 = pos.y as f32 + 1.0 + EPSILON;
    let z1 = pos.z as f32 + 1.0 + EPSILON;

    let v_top = stage as f32 / STAGE_COUNT as f32;
    let v_bot = (stage + 1) as f32 / STAGE_COUNT as f32;

    let mut verts = [OverlayVertex {
        position: [0.0; 3],
        uv: [0.0; 2],
    }; 36];
    let mut i = 0;

    let mut quad = |positions: [[f32; 3]; 4]| {
        let uvs = [[0.0, v_bot], [1.0, v_bot], [1.0, v_top], [0.0, v_top]];
        for &idx in &[0usize, 1, 2, 0, 2, 3] {
            verts[i] = OverlayVertex {
                position: positions[idx],
                uv: uvs[idx],
            };
            i += 1;
        }
    };

    quad([[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]]);
    quad([[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]]);
    quad([[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]]);
    quad([[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]]);
    quad([[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]]);
    quad([[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]]);

    verts
}

fn load_destroy_atlas(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation) {
    let mut atlas_pixels = Vec::new();
    let mut tile_size = 16u32;

    for stage in 0..STAGE_COUNT {
        let key = format!("minecraft/textures/block/destroy_stage_{stage}.png");
        let path = resolve_asset_path(jar_assets_dir, asset_index, &key);
        if let Some((pixels, w, _h)) = util::load_png(&path) {
            tile_size = w;
            atlas_pixels.extend_from_slice(&pixels);
        } else {
            atlas_pixels.extend(std::iter::repeat_n(
                0u8,
                (tile_size * tile_size * 4) as usize,
            ));
        }
    }

    let atlas_w = tile_size;
    let atlas_h = tile_size * STAGE_COUNT;

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, atlas_w, atlas_h, "destroy_atlas");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &atlas_pixels, "destroy_atlas_staging");

    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        atlas_w,
        atlas_h,
    );

    unsafe { device.destroy_buffer(staging_buf, None) };
    allocator.lock().unwrap().free(staging_alloc).ok();

    tracing::info!("Block overlay: loaded {STAGE_COUNT} destroy stages ({atlas_w}x{atlas_h})");

    (image, view, allocation)
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("block_overlay.vert.spv");
    let frag_spv = shader::include_spirv!("block_overlay.frag.spv");

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

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<OverlayVertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];

    let attr_descs = [
        vk::VertexInputAttributeDescription {
            location: 0,
            binding: 0,
            format: vk::Format::R32G32B32_SFLOAT,
            offset: 0,
        },
        vk::VertexInputAttributeDescription {
            location: 1,
            binding: 0,
            format: vk::Format::R32G32_SFLOAT,
            offset: 12,
        },
    ];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding_descs)
        .vertex_attribute_descriptions(&attr_descs);

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);

    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0)
        .depth_bias_enable(true)
        .depth_bias_constant_factor(-1.0)
        .depth_bias_slope_factor(-10.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);

    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::DST_COLOR,
        dst_color_blend_factor: vk::BlendFactor::SRC_COLOR,
        color_blend_op: vk::BlendOp::ADD,
        src_alpha_blend_factor: vk::BlendFactor::ONE,
        dst_alpha_blend_factor: vk::BlendFactor::ZERO,
        alpha_blend_op: vk::BlendOp::ADD,
        color_write_mask: vk::ColorComponentFlags::RGBA,
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
    .expect("failed to create block overlay pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
