use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::camera::Camera;
use crate::renderer::pipelines::sky::SkyState;
use crate::renderer::shader;
use crate::renderer::util;

const CLOUD_HEIGHT: f32 = 192.0;
const CLOUD_PLANE_RADIUS: f32 = 2048.0;
const TICKS_PER_DAY: f32 = 24000.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudUniform {
    view_proj: [[f32; 4]; 4],
    cloud_color: [f32; 4],
    camera_pos: [f32; 3],
    cloud_offset: f32,
    cloud_height: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CloudVertex {
    position: [f32; 3],
}

pub struct CloudPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    uniform_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    uniform_sets: Vec<vk::DescriptorSet>,
    tex_set: vk::DescriptorSet,
    uniform_buffers: Vec<vk::Buffer>,
    uniform_allocations: Vec<Option<Allocation>>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Option<Allocation>,
    cloud_image: vk::Image,
    cloud_view: vk::ImageView,
    cloud_allocation: Option<Allocation>,
    cloud_sampler: vk::Sampler,
}

impl CloudPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let uniform_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
        );
        let tex_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [uniform_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create cloud pipeline layout");

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
            .expect("failed to create cloud descriptor pool");

        let uniform_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| uniform_layout).collect();
        let uniform_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&uniform_layouts);
        let uniform_sets = unsafe { device.allocate_descriptor_sets(&uniform_alloc_info) }
            .expect("failed to allocate cloud uniform sets");

        let tex_layouts = [tex_layout];
        let tex_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&tex_layouts);
        let tex_set = unsafe { device.allocate_descriptor_sets(&tex_alloc_info) }
            .expect("failed to allocate cloud tex set")[0];

        let mut uniform_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut uniform_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &uniform_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CloudUniform>() as u64,
                "cloud_uniform",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CloudUniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            uniform_buffers.push(buf);
            uniform_allocations.push(Some(alloc));
        }

        let (cloud_image, cloud_view, cloud_allocation, cloud_sampler) = load_cloud_texture(
            device,
            allocator,
            queue,
            command_pool,
            assets_dir,
            asset_index,
        );

        let image_info = [vk::DescriptorImageInfo {
            sampler: cloud_sampler,
            image_view: cloud_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let tex_write = vk::WriteDescriptorSet::default()
            .dst_set(tex_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[tex_write], &[]) };

        let vertices = build_plane_vertices();
        let bytes = bytemuck::cast_slice(&vertices);
        let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "cloud_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            uniform_layout,
            tex_layout,
            descriptor_pool,
            uniform_sets,
            tex_set,
            uniform_buffers,
            uniform_allocations,
            vertex_buffer,
            vertex_allocation: Some(vertex_allocation),
            cloud_image,
            cloud_view,
            cloud_allocation: Some(cloud_allocation),
            cloud_sampler,
        }
    }

    pub fn draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        camera: &Camera,
        sky: &SkyState,
    ) {
        let day_frac = (sky.day_time % 24000) as f32 / TICKS_PER_DAY;
        let cloud_offset = (sky.game_time % (256 * 400)) as f32 * 0.03;

        let night_factor = cloud_night_factor(day_frac);
        let r = 1.0 - night_factor * 0.9;
        let g = 1.0 - night_factor * 0.9;
        let b = 1.0 - night_factor * 0.85;
        let a = 0.8 - night_factor * 0.3;

        let uniform = CloudUniform {
            view_proj: camera.view_projection().to_cols_array_2d(),
            cloud_color: [r, g, b, a],
            camera_pos: camera.position.into(),
            cloud_offset,
            cloud_height: CLOUD_HEIGHT,
            _pad: [0.0; 3],
        };

        let bytes = bytemuck::bytes_of(&uniform);
        if let Some(alloc) = &mut self.uniform_allocations[frame] {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.uniform_sets[frame], self.tex_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_draw(cmd, 6, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        unsafe { device.destroy_buffer(self.vertex_buffer, None) };
        if let Some(alloc) = self.vertex_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.uniform_buffers[i], None) };
            if let Some(alloc) = self.uniform_allocations[i].take() {
                allocator.lock().unwrap().free(alloc).ok();
            }
        }

        unsafe {
            device.destroy_sampler(self.cloud_sampler, None);
            device.destroy_image_view(self.cloud_view, None);
            device.destroy_image(self.cloud_image, None);
        }
        if let Some(alloc) = self.cloud_allocation.take() {
            allocator.lock().unwrap().free(alloc).ok();
        }

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.uniform_layout, None);
            device.destroy_descriptor_set_layout(self.tex_layout, None);
        }
    }
}

fn cloud_night_factor(day_frac: f32) -> f32 {
    let t = day_frac * 24000.0;
    if t < 11867.0 {
        0.0
    } else if t < 13670.0 {
        (t - 11867.0) / (13670.0 - 11867.0)
    } else if t < 22330.0 {
        1.0
    } else {
        1.0 - (t - 22330.0) / (24000.0 - 22330.0)
    }
}

fn build_plane_vertices() -> Vec<CloudVertex> {
    let r = CLOUD_PLANE_RADIUS;
    vec![
        CloudVertex {
            position: [-r, 0.0, -r],
        },
        CloudVertex {
            position: [r, 0.0, -r],
        },
        CloudVertex {
            position: [r, 0.0, r],
        },
        CloudVertex {
            position: [-r, 0.0, -r],
        },
        CloudVertex {
            position: [r, 0.0, r],
        },
        CloudVertex {
            position: [-r, 0.0, r],
        },
    ]
}

fn load_cloud_texture(
    device: &ash::Device,
    allocator: &Arc<Mutex<Allocator>>,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation, vk::Sampler) {
    let path = resolve_asset_path(
        assets_dir,
        asset_index,
        "minecraft/textures/environment/clouds.png",
    );

    let (pixels, width, height) = match crate::assets::load_image(&path) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let w = rgba.width();
            let h = rgba.height();
            (rgba.into_raw(), w, h)
        }
        Err(e) => {
            log::warn!("Failed to load clouds.png: {e}");
            (vec![255, 255, 255, 255], 1, 1)
        }
    };

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "cloud_tex");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "cloud_staging");
    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        width,
        height,
    );
    unsafe { device.destroy_buffer(staging_buf, None) };
    allocator.lock().unwrap().free(staging_alloc).ok();

    let sampler_info = vk::SamplerCreateInfo::default()
        .mag_filter(vk::Filter::NEAREST)
        .min_filter(vk::Filter::NEAREST)
        .address_mode_u(vk::SamplerAddressMode::REPEAT)
        .address_mode_v(vk::SamplerAddressMode::REPEAT);
    let sampler = unsafe { device.create_sampler(&sampler_info, None) }
        .expect("failed to create cloud sampler");

    (image, view, allocation, sampler)
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("cloud.vert.spv");
    let frag_spv = shader::include_spirv!("cloud.frag.spv");
    let vert_mod = shader::create_shader_module(device, vert_spv);
    let frag_mod = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(vert_mod)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(frag_mod)
            .name(c"main"),
    ];

    let binding = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<CloudVertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attrs = [vk::VertexInputAttributeDescription {
        location: 0,
        binding: 0,
        format: vk::Format::R32G32B32_SFLOAT,
        offset: 0,
    }];

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&binding)
        .vertex_attribute_descriptions(&attrs);
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
        .depth_test_enable(true)
        .depth_write_enable(false)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::ONE,
        dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        color_blend_op: vk::BlendOp::ADD,
        src_alpha_blend_factor: vk::BlendFactor::ONE,
        dst_alpha_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        alpha_blend_op: vk::BlendOp::ADD,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];
    let color_blending =
        vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);

    let info = [vk::GraphicsPipelineCreateInfo::default()
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

    let pipeline =
        unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &info, None) }
            .expect("failed to create cloud pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);
    }
    pipeline
}
