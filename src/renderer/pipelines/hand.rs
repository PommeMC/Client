use std::path::Path;
use std::sync::{Arc, Mutex};

use ash::vk;
use glam::{Mat4, Vec3};
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::shader;
use crate::renderer::util;
const NEAR: f32 = 0.05;
const FAR: f32 = 10.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HandUniform {
    mvp: [[f32; 4]; 4],
}

pub struct HandPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    mvp_layout: vk::DescriptorSetLayout,
    skin_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    mvp_sets: Vec<vk::DescriptorSet>,
    skin_set: vk::DescriptorSet,
    mvp_buffers: Vec<vk::Buffer>,
    mvp_allocations: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    vertex_count: u32,
    skin_image: vk::Image,
    skin_view: vk::ImageView,
    skin_sampler: vk::Sampler,
    skin_allocation: Allocation,
}

impl HandPipeline {
    pub fn new(
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let mvp_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let skin_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [mvp_layout, skin_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create hand pipeline layout");

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
            .expect("failed to create hand descriptor pool");

        let mvp_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| mvp_layout).collect();
        let mvp_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&mvp_layouts);
        let mvp_sets = unsafe { device.allocate_descriptor_sets(&mvp_alloc_info) }
            .expect("failed to allocate hand mvp descriptor sets");

        let skin_layouts = [skin_layout];
        let skin_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&skin_layouts);
        let skin_set = unsafe { device.allocate_descriptor_sets(&skin_alloc_info) }
            .expect("failed to allocate hand skin descriptor set")[0];

        let mut mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &mvp_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<HandUniform>() as u64,
                "hand_uniform",
            );

            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<HandUniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };

            mvp_buffers.push(buf);
            mvp_allocations.push(alloc);
        }

        let (skin_image, skin_view, skin_allocation, skin_w, skin_h) = load_skin_texture(
            device,
            queue,
            command_pool,
            allocator,
            assets_dir,
            asset_index,
        );

        let skin_sampler = unsafe { util::create_nearest_sampler(device) };

        update_skin_descriptor(device, skin_set, skin_view, skin_sampler);

        let vertices = build_arm_vertices(skin_w, skin_h);
        let vertex_count = vertices.len() as u32;
        let vertex_bytes = bytemuck::cast_slice::<HandVertex, u8>(&vertices);
        let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            vertex_bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "hand_vertices",
        );

        log::info!("Hand pipeline initialized ({vertex_count} vertices, skin {skin_w}x{skin_h})");

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            skin_layout,
            descriptor_pool,
            mvp_sets,
            skin_set,
            mvp_buffers,
            mvp_allocations,
            vertex_buffer,
            vertex_allocation,
            vertex_count,
            skin_image,
            skin_view,
            skin_sampler,
            skin_allocation,
        }
    }

    pub fn update_and_draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        aspect: f32,
        swing_progress: f32,
    ) {
        let mut proj = Mat4::perspective_rh(
            crate::renderer::camera::DEFAULT_FOV_DEGREES.to_radians(),
            aspect,
            NEAR,
            FAR,
        );
        proj.y_axis.y *= -1.0;

        let sp = swing_progress;
        let sqrt_sp = sp.sqrt();
        let pi = std::f32::consts::PI;

        let x_off = -0.3 * (sqrt_sp * pi).sin();
        let y_off = 0.4 * (sqrt_sp * pi * 2.0).sin();
        let z_off = -0.4 * (sp * pi).sin();

        let swing_y = (sqrt_sp * pi).sin() * 70.0_f32.to_radians();
        let swing_z = (sp * sp * pi).sin() * (-20.0_f32).to_radians();

        let model = Mat4::from_translation(Vec3::new(x_off + 0.64, y_off - 0.6, z_off - 0.72))
            * Mat4::from_rotation_y(45.0_f32.to_radians())
            * Mat4::from_rotation_y(swing_y)
            * Mat4::from_rotation_z(swing_z)
            * Mat4::from_translation(Vec3::new(-1.0, 3.6, 3.5))
            * Mat4::from_rotation_z(120.0_f32.to_radians())
            * Mat4::from_rotation_x(200.0_f32.to_radians())
            * Mat4::from_rotation_y((-135.0_f32).to_radians())
            * Mat4::from_translation(Vec3::new(5.6, 0.0, 0.0))
            * Mat4::from_translation(Vec3::new(-5.0 / 16.0, 2.0 / 16.0, 0.0))
            * Mat4::from_rotation_z(0.1);

        let mvp = proj * model;
        let uniform = HandUniform {
            mvp: mvp.to_cols_array_2d(),
        };
        let bytes = bytemuck::bytes_of(&uniform);
        self.mvp_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.mvp_sets[frame], self.skin_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
        }
    }

    pub fn skin_view(&self) -> vk::ImageView {
        self.skin_view
    }

    pub fn skin_sampler(&self) -> vk::Sampler {
        self.skin_sampler
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reload_skin(
        &mut self,
        device: &ash::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        allocator: &Arc<Mutex<Allocator>>,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) {
        let (image, view, allocation) = upload_skin_to_gpu(
            device,
            queue,
            command_pool,
            allocator,
            pixels,
            width,
            height,
        );

        unsafe {
            device.destroy_image_view(self.skin_view, None);
            device.destroy_image(self.skin_image, None);
        }
        allocator
            .lock()
            .unwrap()
            .free(std::mem::replace(&mut self.skin_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        self.skin_image = image;
        self.skin_view = view;
        self.skin_allocation = allocation;
        update_skin_descriptor(device, self.skin_set, self.skin_view, self.skin_sampler);

        log::info!("Skin reloaded: {width}x{height}");
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.mvp_buffers[i], None) };
            alloc
                .free(std::mem::replace(&mut self.mvp_allocations[i], unsafe {
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
            device.destroy_sampler(self.skin_sampler, None);
            device.destroy_image_view(self.skin_view, None);
        }
        alloc
            .free(std::mem::replace(&mut self.skin_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        unsafe { device.destroy_image(self.skin_image, None) };

        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.mvp_layout, None);
            device.destroy_descriptor_set_layout(self.skin_layout, None);
        }
    }
}

fn build_arm_vertices(skin_w: u32, skin_h: u32) -> Vec<HandVertex> {
    let sw = skin_w as f32;
    let sh = skin_h as f32;

    // Vanilla addBox(-3, -2, -2, 4, 12, 4) scaled to blocks (1/16)
    let x0: f32 = -3.0 / 16.0;
    let x1: f32 = 1.0 / 16.0;
    let y0: f32 = -2.0 / 16.0;
    let y1: f32 = 10.0 / 16.0;
    let z0: f32 = -2.0 / 16.0;
    let z1: f32 = 2.0 / 16.0;

    // texOffs(40, 16) on Steve skin, box dimensions w=4 h=12 d=4
    let u0 = 40.0;
    let v0 = 16.0;
    let w = 4.0;
    let h = 12.0;
    let d = 4.0;

    let right_uv = [u0, v0 + d, u0 + d, v0 + d + h];
    let front_uv = [u0 + d, v0 + d, u0 + d + w, v0 + d + h];
    let left_uv = [u0 + d + w, v0 + d, u0 + d + w + d, v0 + d + h];
    let back_uv = [u0 + d + w + d, v0 + d, u0 + d + w + d + w, v0 + d + h];
    let top_uv = [u0 + d, v0, u0 + d + w, v0 + d];
    let bot_uv = [u0 + d + w, v0, u0 + d + w + w, v0 + d];

    let mut verts = Vec::with_capacity(36);

    let mut quad = |positions: [[f32; 3]; 4], uv_px: [f32; 4]| {
        let u_min = uv_px[0] / sw;
        let v_min = uv_px[1] / sh;
        let u_max = uv_px[2] / sw;
        let v_max = uv_px[3] / sh;
        let uvs = [
            [u_min, v_max],
            [u_max, v_max],
            [u_max, v_min],
            [u_min, v_min],
        ];
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            verts.push(HandVertex {
                position: positions[i],
                uv: uvs[i],
            });
        }
    };

    // -X face (outer side of right arm)
    quad(
        [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
        right_uv,
    );

    // +X face (inner side)
    quad(
        [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
        left_uv,
    );

    // +Y face (shoulder/top)
    quad(
        [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
        top_uv,
    );

    // -Y face (wrist/bottom)
    quad(
        [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        bot_uv,
    );

    // -Z face (front, facing camera)
    quad(
        [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
        front_uv,
    );

    // +Z face (back)
    quad(
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        back_uv,
    );

    verts
}

fn load_skin_texture(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation, u32, u32) {
    let skin_key = "minecraft/textures/entity/player/wide/steve.png";
    let skin_path = resolve_asset_path(assets_dir, asset_index, skin_key);

    let (pixels, width, height) = util::load_png(&skin_path).unwrap_or_else(|| {
        log::warn!(
            "Failed to load skin from {}, using fallback",
            skin_path.display()
        );
        fallback_skin()
    });

    let (image, view, allocation) = upload_skin_to_gpu(
        device,
        queue,
        command_pool,
        allocator,
        &pixels,
        width,
        height,
    );
    (image, view, allocation, width, height)
}

fn upload_skin_to_gpu(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> (vk::Image, vk::ImageView, Allocation) {
    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, width, height, "skin");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, pixels, "skin_staging");
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
    (image, view, allocation)
}

fn update_skin_descriptor(
    device: &ash::Device,
    set: vk::DescriptorSet,
    view: vk::ImageView,
    sampler: vk::Sampler,
) {
    let image_info = [vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }];
    let write = vk::WriteDescriptorSet::default()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_info);
    unsafe { device.update_descriptor_sets(&[write], &[]) };
}

fn fallback_skin() -> (Vec<u8>, u32, u32) {
    let w = 64u32;
    let h = 64u32;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[196, 161, 125, 255]);
    }
    (pixels, w, h)
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("hand.vert.spv");
    let frag_spv = shader::include_spirv!("hand.frag.spv");

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
        stride: std::mem::size_of::<HandVertex>() as u32,
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
        .line_width(1.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);

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
    .expect("failed to create hand pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
