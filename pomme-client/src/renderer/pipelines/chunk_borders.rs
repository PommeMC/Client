use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use crate::renderer::camera::CameraUniform;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}

const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 0.6];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 0.6];
const BLUE: [f32; 4] = [0.0, 0.5, 1.0, 0.6];

pub struct ChunkBorderPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    desc_layout: vk::DescriptorSetLayout,
    desc_pool: vk::DescriptorPool,
    desc_sets: Vec<vk::DescriptorSet>,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocs: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_alloc: Allocation,
    vertex_count: u32,
}

impl ChunkBorderPipeline {
    pub fn new(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<gpu_allocator::vulkan::Allocator>>,
    ) -> Self {
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::VERTEX);
        let layout_info =
            vk::DescriptorSetLayoutCreateInfo::default().bindings(std::slice::from_ref(&binding));
        let desc_layout =
            unsafe { device.create_descriptor_set_layout(&layout_info, None) }.unwrap();

        let layout_info =
            vk::PipelineLayoutCreateInfo::default().set_layouts(std::slice::from_ref(&desc_layout));
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }.unwrap();

        let vert_spv = shader::include_spirv!("chunk_border.vert.spv");
        let frag_spv = shader::include_spirv!("chunk_border.frag.spv");
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

        let binding_desc = [vk::VertexInputBindingDescription {
            binding: 0,
            stride: std::mem::size_of::<LineVertex>() as u32,
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
                format: vk::Format::R32G32B32A32_SFLOAT,
                offset: 12,
            },
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&binding_desc)
            .vertex_attribute_descriptions(&attr_descs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::LINE_LIST);

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
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);

        let blend_attachment = [vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::TRUE,
            src_color_blend_factor: vk::BlendFactor::SRC_ALPHA,
            dst_color_blend_factor: vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
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
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0)];

        let pipeline = unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
        }
        .unwrap()[0];

        unsafe {
            device.destroy_shader_module(vert_mod, None);
            device.destroy_shader_module(frag_mod, None);
        }

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_FRAMES_IN_FLIGHT as u32)
            .pool_sizes(&pool_sizes);
        let desc_pool = unsafe { device.create_descriptor_pool(&pool_info, None) }.unwrap();

        let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| desc_layout).collect();
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(desc_pool)
            .set_layouts(&layouts);
        let desc_sets = unsafe { device.allocate_descriptor_sets(&alloc_info) }.unwrap();

        let mut camera_buffers = Vec::new();
        let mut camera_allocs = Vec::new();
        for desc_set in &desc_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "chunk_border_cam",
            );
            let buf_info = [vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
            }];
            let write = [vk::WriteDescriptorSet::default()
                .dst_set(*desc_set)
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buf_info)];
            unsafe { device.update_descriptor_sets(&write, &[]) };
            camera_buffers.push(buf);
            camera_allocs.push(alloc);
        }

        let max_verts = 4096;
        let (vertex_buffer, vertex_alloc) = util::create_host_buffer(
            device,
            allocator,
            (max_verts * std::mem::size_of::<LineVertex>()) as u64,
            vk::BufferUsageFlags::VERTEX_BUFFER,
            "chunk_border_verts",
        );

        Self {
            pipeline,
            pipeline_layout,
            desc_layout,
            desc_pool,
            desc_sets,
            camera_buffers,
            camera_allocs,
            vertex_buffer,
            vertex_alloc,
            vertex_count: 0,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let data = self.camera_allocs[frame].mapped_slice_mut().unwrap();
        data[..std::mem::size_of::<CameraUniform>()].copy_from_slice(bytemuck::bytes_of(uniform));
    }

    pub fn update_lines(&mut self, cam_x: f32, cam_y: f32, cam_z: f32, min_y: i32, max_y: i32) {
        let chunk_x = (cam_x as i32).div_euclid(16) * 16;
        let chunk_z = (cam_z as i32).div_euclid(16) * 16;

        let mut verts: Vec<LineVertex> = Vec::new();
        let y_min = min_y as f32;
        let y_max = max_y as f32;

        let cam = [cam_x, cam_y, cam_z];
        let push_line = |verts: &mut Vec<LineVertex>,
                         x0: f32,
                         y0: f32,
                         z0: f32,
                         x1: f32,
                         y1: f32,
                         z1: f32,
                         color: [f32; 4]| {
            verts.push(LineVertex {
                position: [x0 - cam[0], y0 - cam[1], z0 - cam[2]],
                color,
            });
            verts.push(LineVertex {
                position: [x1 - cam[0], y1 - cam[1], z1 - cam[2]],
                color,
            });
        };

        // Vertical lines at chunk corners (red)
        for dx in [0, 16] {
            for dz in [0, 16] {
                let x = (chunk_x + dx) as f32;
                let z = (chunk_z + dz) as f32;
                push_line(&mut verts, x, y_min, z, x, y_max, z, RED);
            }
        }

        // Vertical lines along chunk edges (blue) - every block along edges
        for d in 1..16 {
            let x0 = chunk_x as f32;
            let x1 = (chunk_x + 16) as f32;
            let z0 = chunk_z as f32;
            let z1 = (chunk_z + 16) as f32;
            let p = (chunk_x + d) as f32;
            let q = (chunk_z + d) as f32;
            push_line(&mut verts, p, y_min, z0, p, y_max, z0, BLUE);
            push_line(&mut verts, p, y_min, z1, p, y_max, z1, BLUE);
            push_line(&mut verts, x0, y_min, q, x0, y_max, q, BLUE);
            push_line(&mut verts, x1, y_min, q, x1, y_max, q, BLUE);
        }

        // Horizontal lines at section boundaries (yellow)
        for section in 0..=((max_y - min_y) / 16) {
            let y = (min_y + section * 16) as f32;
            let x0 = chunk_x as f32;
            let x1 = (chunk_x + 16) as f32;
            let z0 = chunk_z as f32;
            let z1 = (chunk_z + 16) as f32;
            push_line(&mut verts, x0, y, z0, x1, y, z0, YELLOW);
            push_line(&mut verts, x0, y, z1, x1, y, z1, YELLOW);
            push_line(&mut verts, x0, y, z0, x0, y, z1, YELLOW);
            push_line(&mut verts, x1, y, z0, x1, y, z1, YELLOW);
        }

        let max_verts = 4096;
        let count = verts.len().min(max_verts);
        let data = self.vertex_alloc.mapped_slice_mut().unwrap();
        let bytes = bytemuck::cast_slice(&verts[..count]);
        data[..bytes.len()].copy_from_slice(bytes);
        self.vertex_count = count as u32;
    }

    pub fn draw(&self, device: &ash::Device, cmd: vk::CommandBuffer, frame: usize) {
        if self.vertex_count == 0 {
            return;
        }
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[self.desc_sets[frame]],
                &[],
            );
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vertex_buffer], &[0]);
            device.cmd_draw(cmd, self.vertex_count, 1, 0, 0);
        }
    }

    pub fn destroy(
        &mut self,
        device: &ash::Device,
        allocator: &Arc<Mutex<gpu_allocator::vulkan::Allocator>>,
    ) {
        let mut alloc = allocator.lock().unwrap();
        unsafe {
            device.destroy_buffer(self.vertex_buffer, None);
        }
        alloc
            .free(std::mem::replace(&mut self.vertex_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            unsafe { device.destroy_buffer(self.camera_buffers[i], None) };
            alloc
                .free(std::mem::replace(&mut self.camera_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        drop(alloc);
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.desc_pool, None);
            device.destroy_descriptor_set_layout(self.desc_layout, None);
        }
    }
}
