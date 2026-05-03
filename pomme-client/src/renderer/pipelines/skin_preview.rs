use std::sync::{Arc, Mutex};

use ash::vk;
use glam::{Mat4, Vec3};
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::shader;
use crate::renderer::util;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    mvp: [[f32; 4]; 4],
}

pub struct SkinPreviewPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    mvp_layout: vk::DescriptorSetLayout,
    tex_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    mvp_sets: Vec<vk::DescriptorSet>,
    head_mvp_sets: Vec<vk::DescriptorSet>,
    tex_set: vk::DescriptorSet,
    mvp_buffers: Vec<vk::Buffer>,
    mvp_allocations: Vec<Allocation>,
    head_mvp_buffers: Vec<vk::Buffer>,
    head_mvp_allocations: Vec<Allocation>,
    body_buffer: vk::Buffer,
    body_allocation: Allocation,
    body_count: u32,
    head_buffer: vk::Buffer,
    head_allocation: Allocation,
    head_count: u32,
    arm_buffer: vk::Buffer,
    arm_allocation: Allocation,
    arm_count: u32,
    arm_mvp_sets: Vec<vk::DescriptorSet>,
    arm_mvp_buffers: Vec<vk::Buffer>,
    arm_mvp_allocations: Vec<Allocation>,
    swing_start: Option<std::time::Instant>,
}

impl SkinPreviewPipeline {
    pub fn new(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        skin_view: vk::ImageView,
        skin_sampler: vk::Sampler,
    ) -> Self {
        let mvp_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let tex_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [mvp_layout, tex_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create skin preview pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UNIFORM_BUFFER,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32 * 3,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets((MAX_FRAMES_IN_FLIGHT * 3 + 1) as u32)
            .pool_sizes(&pool_sizes);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }
            .expect("failed to create skin preview descriptor pool");

        let mvp_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| mvp_layout).collect();
        let mvp_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&mvp_layouts);
        let mvp_sets = unsafe { device.allocate_descriptor_sets(&mvp_alloc_info) }
            .expect("failed to allocate skin preview mvp sets");

        let head_mvp_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&mvp_layouts);
        let head_mvp_sets = unsafe { device.allocate_descriptor_sets(&head_mvp_alloc_info) }
            .expect("failed to allocate skin preview head mvp sets");

        let arm_mvp_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&mvp_layouts);
        let arm_mvp_sets = unsafe { device.allocate_descriptor_sets(&arm_mvp_alloc_info) }
            .expect("failed to allocate skin preview arm mvp sets");

        let tex_layouts = [tex_layout];
        let tex_alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&tex_layouts);
        let tex_set = unsafe { device.allocate_descriptor_sets(&tex_alloc_info) }
            .expect("failed to allocate skin preview tex set")[0];

        let mut mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut head_mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut head_mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut arm_mvp_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut arm_mvp_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_body_mvp",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(mvp_sets[i])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            mvp_buffers.push(buf);
            mvp_allocations.push(alloc);

            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_head_mvp",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(head_mvp_sets[i])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            head_mvp_buffers.push(buf);
            head_mvp_allocations.push(alloc);

            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<Uniform>() as u64,
                "skin_arm_mvp",
            );
            let buffer_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<Uniform>() as u64,
            }];
            let write = vk::WriteDescriptorSet::default()
                .dst_set(arm_mvp_sets[i])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffer_info);
            unsafe { device.update_descriptor_sets(&[write], &[]) };
            arm_mvp_buffers.push(buf);
            arm_mvp_allocations.push(alloc);
        }

        let image_info = [vk::DescriptorImageInfo {
            sampler: skin_sampler,
            image_view: skin_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let tex_write = vk::WriteDescriptorSet::default()
            .dst_set(tex_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);
        unsafe { device.update_descriptor_sets(&[tex_write], &[]) };

        let body_verts = build_body_mesh();
        let body_bytes = bytemuck::cast_slice(&body_verts);
        let (body_buffer, body_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            body_bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "skin_body",
        );

        let arm_verts = build_right_arm_mesh();
        let arm_bytes = bytemuck::cast_slice(&arm_verts);
        let (arm_buffer, arm_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            arm_bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "skin_arm",
        );

        let head_verts = build_head_mesh();
        let head_bytes = bytemuck::cast_slice(&head_verts);
        let (head_buffer, head_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            head_bytes,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "skin_head",
        );

        Self {
            pipeline,
            pipeline_layout,
            mvp_layout,
            tex_layout,
            descriptor_pool,
            mvp_sets,
            head_mvp_sets,
            tex_set,
            mvp_buffers,
            mvp_allocations,
            head_mvp_buffers,
            head_mvp_allocations,
            arm_buffer,
            arm_allocation,
            arm_count: arm_verts.len() as u32,
            arm_mvp_sets,
            arm_mvp_buffers,
            arm_mvp_allocations,
            swing_start: None,
            body_buffer,
            body_allocation,
            body_count: body_verts.len() as u32,
            head_buffer,
            head_allocation,
            head_count: head_verts.len() as u32,
        }
    }

    pub fn trigger_swing(&mut self) {
        self.swing_start = Some(std::time::Instant::now());
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &ash::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        aspect: f32,
        screen_x: f32,
        screen_y: f32,
        mouse_px_x: f32,
        mouse_px_y: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        let center_px_x = screen_x * screen_w;
        let center_px_y = screen_y * screen_h;
        let body_yaw_raw = ((mouse_px_x - center_px_x) / 40.0).atan();
        let head_pitch_raw = ((mouse_px_y - center_px_y) / 40.0).atan();

        let head_yaw_deg = body_yaw_raw * 40.0;
        let body_yaw_deg = head_yaw_deg * 0.3;
        let body_rot = std::f32::consts::PI + body_yaw_deg.to_radians();
        let head_yaw = head_yaw_deg.to_radians();
        let head_pitch = head_pitch_raw * 20.0f32.to_radians();

        let fov = 0.6f32;
        let mut proj = Mat4::perspective_rh(fov, aspect, 0.1, 100.0);
        proj.y_axis.y *= -1.0;

        let ndc_x = screen_x * 2.0 - 1.0;
        let ndc_y = screen_y * 2.0 - 1.0;
        let clip_offset = Mat4::from_translation(Vec3::new(ndc_x, ndc_y, 0.0));

        let eye = Vec3::new(0.0, 0.0, -4.5);
        let target = Vec3::ZERO;
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);

        let body_rot_mat = Mat4::from_rotation_y(body_rot);

        let vp = clip_offset * proj * view;
        let body_mvp = vp * body_rot_mat;
        let head_rot_mat =
            Mat4::from_rotation_y(body_rot + head_yaw) * Mat4::from_rotation_x(head_pitch);
        let head_mvp = vp * head_rot_mat;

        // Right arm swing animation
        let swing_angle = if let Some(start) = self.swing_start {
            let t = start.elapsed().as_secs_f32();
            if t > 0.4 {
                self.swing_start = None;
                0.0
            } else {
                let progress = t / 0.4;
                (progress * std::f32::consts::PI).sin() * -1.5
            }
        } else {
            0.0
        };

        // Arm pivot is at shoulder: (-5, 2, 0) in vanilla coords
        // In our Y-flipped space: (-5*PX, -2*PX, 0)
        let shoulder = Vec3::new(-5.0 * PX, -2.0 * PX, 0.0);
        let arm_swing = Mat4::from_translation(shoulder)
            * Mat4::from_rotation_x(swing_angle)
            * Mat4::from_translation(-shoulder);
        let arm_mvp = vp * body_rot_mat * arm_swing;

        write_uniform(&self.mvp_allocations[frame], &body_mvp);
        write_uniform(&self.head_mvp_allocations[frame], &head_mvp);
        write_uniform(&self.arm_mvp_allocations[frame], &arm_mvp);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);

            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.mvp_sets[frame], self.tex_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.body_buffer], &[0]);
            device.cmd_draw(cmd, self.body_count, 1, 0, 0);

            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.head_mvp_sets[frame], self.tex_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.head_buffer], &[0]);
            device.cmd_draw(cmd, self.head_count, 1, 0, 0);

            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.arm_mvp_sets[frame], self.tex_set],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.arm_buffer], &[0]);
            device.cmd_draw(cmd, self.arm_count, 1, 0, 0);
        }
    }

    pub fn recreate_pipeline(&mut self, device: &ash::Device, render_pass: vk::RenderPass) {
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &ash::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe {
                device.destroy_buffer(self.mvp_buffers[i], None);
                device.destroy_buffer(self.head_mvp_buffers[i], None);
                device.destroy_buffer(self.arm_mvp_buffers[i], None);
            }
            alloc
                .free(std::mem::replace(&mut self.mvp_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.head_mvp_allocations[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
            alloc
                .free(std::mem::replace(
                    &mut self.arm_mvp_allocations[i],
                    unsafe { std::mem::zeroed() },
                ))
                .ok();
        }
        unsafe {
            device.destroy_buffer(self.body_buffer, None);
            device.destroy_buffer(self.head_buffer, None);
            device.destroy_buffer(self.arm_buffer, None);
        }
        alloc
            .free(std::mem::replace(&mut self.body_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        alloc
            .free(std::mem::replace(&mut self.head_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        alloc
            .free(std::mem::replace(&mut self.arm_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.mvp_layout, None);
            device.destroy_descriptor_set_layout(self.tex_layout, None);
        }
    }
}

fn write_uniform(alloc: &Allocation, mvp: &Mat4) {
    let uniform = Uniform {
        mvp: mvp.to_cols_array_2d(),
    };
    let bytes = bytemuck::bytes_of(&uniform);
    unsafe {
        let dst = alloc.mapped_ptr().unwrap().as_ptr() as *mut u8;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
    }
}

fn create_pipeline(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("hand.vert.spv");
    let frag_spv = shader::include_spirv!("hand.frag.spv");
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
        stride: std::mem::size_of::<Vertex>() as u32,
        input_rate: vk::VertexInputRate::VERTEX,
    }];
    let attrs = [
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
        .vertex_binding_descriptions(&binding)
        .vertex_attribute_descriptions(&attrs);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .polygon_mode(vk::PolygonMode::FILL)
        .cull_mode(vk::CullModeFlags::FRONT)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
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
            .expect("failed to create skin preview pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);
    }
    pipeline
}

// Vanilla model coordinates: Y-down, 1 unit = 1 pixel
// We convert to Y-up by negating Y, then scale by PX
const PX: f32 = 1.0 / 16.0;

fn uv(x: u32, y: u32, w: u32, h: u32) -> [[f32; 2]; 4] {
    let s = 64.0;
    [
        [x as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, y as f32 / s],
        [(x + w) as f32 / s, (y + h) as f32 / s],
        [x as f32 / s, (y + h) as f32 / s],
    ]
}

fn quad(verts: &mut Vec<Vertex>, pos: [[f32; 3]; 4], uvs: [[f32; 2]; 4]) {
    for &i in &[0u32, 1, 2, 2, 3, 0] {
        verts.push(Vertex {
            position: pos[i as usize],
            uv: uvs[i as usize],
        });
    }
}

// addBox(ox, oy, oz, w, h, d) at pivot (px, py, pz) with UV origin (tx, ty)
// Vanilla Y-down, we flip to Y-up
#[allow(clippy::too_many_arguments)]
fn add_box(
    verts: &mut Vec<Vertex>,
    px: f32,
    py: f32,
    _pz: f32,
    ox: f32,
    oy: f32,
    oz: f32,
    w: f32,
    h: f32,
    d: f32,
    tx: u32,
    ty: u32,
    tw: u32,
    th: u32,
    td: u32,
) {
    let x0 = (px + ox) * PX;
    let x1 = (px + ox + w) * PX;
    // Flip Y: vanilla Y-down, we want Y-up
    let y0 = -(py + oy + h) * PX;
    let y1 = -(py + oy) * PX;
    let z0 = (_pz + oz) * PX;
    let z1 = (_pz + oz + d) * PX;

    // Front (+Z)
    quad(
        verts,
        [[x0, y1, z1], [x1, y1, z1], [x1, y0, z1], [x0, y0, z1]],
        uv(tx + td, ty + td, tw, th),
    );
    // Back (-Z)
    quad(
        verts,
        [[x1, y1, z0], [x0, y1, z0], [x0, y0, z0], [x1, y0, z0]],
        uv(tx + td + tw + td, ty + td, tw, th),
    );
    // Right (+X)
    quad(
        verts,
        [[x1, y1, z1], [x1, y1, z0], [x1, y0, z0], [x1, y0, z1]],
        uv(tx + td + tw, ty + td, td, th),
    );
    // Left (-X)
    quad(
        verts,
        [[x0, y1, z0], [x0, y1, z1], [x0, y0, z1], [x0, y0, z0]],
        uv(tx, ty + td, td, th),
    );
    // Top (+Y in our space = -Y in vanilla = top of head)
    quad(
        verts,
        [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
        uv(tx + td, ty, tw, td),
    );
    // Bottom (-Y in our space = +Y in vanilla = bottom)
    quad(
        verts,
        [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
        uv(tx + td + tw, ty, tw, td),
    );
}

fn build_head_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.5; // hat overlay inflation (1 pixel bigger in vanilla)
    // Head: addBox(-4, -8, -4, 8, 8, 8) @ (0, 0, 0), UV (0, 0)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, -8.0, -4.0, 8.0, 8.0, 8.0, 0, 0, 8, 8, 8,
    );
    // Hat overlay: same box inflated by 0.5, UV (32, 0)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        -8.0 - e,
        -4.0 - e,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        8.0 + e * 2.0,
        32,
        0,
        8,
        8,
        8,
    );
    v
}

fn build_body_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25; // overlay inflation for body/limbs

    // Body: addBox(-4, 0, -2, 8, 12, 4) @ (0, 0, 0), UV (16, 16)
    add_box(
        &mut v, 0.0, 0.0, 0.0, -4.0, 0.0, -2.0, 8.0, 12.0, 4.0, 16, 16, 8, 12, 4,
    );
    // Jacket overlay: UV (16, 32)
    add_box(
        &mut v,
        0.0,
        0.0,
        0.0,
        -4.0 - e,
        0.0 - e,
        -2.0 - e,
        8.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        16,
        32,
        8,
        12,
        4,
    );

    // Left arm: addBox(-1, -2, -2, 4, 12, 4) @ (5, 2, 0), UV (32, 48)
    add_box(
        &mut v, 5.0, 2.0, 0.0, -1.0, -2.0, -2.0, 4.0, 12.0, 4.0, 32, 48, 4, 12, 4,
    );
    // Left sleeve overlay: UV (48, 48)
    add_box(
        &mut v,
        5.0,
        2.0,
        0.0,
        -1.0 - e,
        -2.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        48,
        48,
        4,
        12,
        4,
    );

    // Right leg: addBox(-2, 0, -2, 4, 12, 4) @ (-1.9, 12, 0), UV (0, 16)
    add_box(
        &mut v, -1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 0, 16, 4, 12, 4,
    );
    // Right pants overlay: UV (0, 32)
    add_box(
        &mut v,
        -1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        32,
        4,
        12,
        4,
    );

    // Left leg: addBox(-2, 0, -2, 4, 12, 4) @ (1.9, 12, 0), UV (16, 48)
    add_box(
        &mut v, 1.9, 12.0, 0.0, -2.0, 0.0, -2.0, 4.0, 12.0, 4.0, 16, 48, 4, 12, 4,
    );
    // Left pants overlay: UV (0, 48)
    add_box(
        &mut v,
        1.9,
        12.0,
        0.0,
        -2.0 - e,
        0.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        0,
        48,
        4,
        12,
        4,
    );
    v
}

fn build_right_arm_mesh() -> Vec<Vertex> {
    let mut v = Vec::new();
    let e = 0.25;
    add_box(
        &mut v, -5.0, 2.0, 0.0, -3.0, -2.0, -2.0, 4.0, 12.0, 4.0, 40, 16, 4, 12, 4,
    );
    add_box(
        &mut v,
        -5.0,
        2.0,
        0.0,
        -3.0 - e,
        -2.0 - e,
        -2.0 - e,
        4.0 + e * 2.0,
        12.0 + e * 2.0,
        4.0 + e * 2.0,
        40,
        32,
        4,
        12,
        4,
    );
    v
}
