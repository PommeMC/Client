use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

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
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
    ) -> Self {
        let binding = vk::DescriptorSetLayoutBinding {
            binding: 0,
            descriptor_type: vk::DescriptorType::UniformBuffer,
            descriptor_count: 1,
            stage_flags: vk::ShaderStageFlags::Vertex,
            ..Default::default()
        };
        let layout_info = vk::DescriptorSetLayoutCreateInfo {
            binding_count: 1,
            bindings: &binding,
            ..Default::default()
        };
        let desc_layout = device
            .create_descriptor_set_layout(&layout_info, None)
            .unwrap();

        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: 1,
            set_layouts: &desc_layout,
            ..Default::default()
        };
        let pipeline_layout = device.create_pipeline_layout(&layout_info, None).unwrap();

        let vert_spv = shader::include_spirv!("chunk_border.vert.spv");
        let frag_spv = shader::include_spirv!("chunk_border.frag.spv");
        let vert_mod = shader::create_shader_module(device, vert_spv);
        let frag_mod = shader::create_shader_module(device, frag_spv);

        let stages = [
            vk::PipelineShaderStageCreateInfo {
                stage: vk::ShaderStageFlags::Vertex,
                module: vert_mod,
                name: c"main".as_ptr(),
                ..Default::default()
            },
            vk::PipelineShaderStageCreateInfo {
                stage: vk::ShaderStageFlags::Fragment,
                module: frag_mod,
                name: c"main".as_ptr(),
                ..Default::default()
            },
        ];

        let binding_desc = vk::VertexInputBindingDescription {
            binding: 0,
            stride: size_of::<LineVertex>() as u32,
            input_rate: vk::VertexInputRate::Vertex,
        };
        let attr_descs = [
            vk::VertexInputAttributeDescription {
                location: 0,
                binding: 0,
                format: vk::Format::R32G32B32Sfloat,
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                location: 1,
                binding: 0,
                format: vk::Format::R32G32B32A32Sfloat,
                offset: 12,
            },
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo {
            vertex_binding_description_count: 1,
            vertex_binding_descriptions: &binding_desc,
            vertex_attribute_description_count: attr_descs.len() as u32,
            vertex_attribute_descriptions: attr_descs.as_ptr(),
            ..Default::default()
        };

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
            topology: vk::PrimitiveTopology::LineList,
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
            front_face: vk::FrontFace::CounterClockwise,
            line_width: 1.0,
            ..Default::default()
        };

        let multisampling = vk::PipelineMultisampleStateCreateInfo {
            rasterization_samples: vk::SampleCountFlags::Type1,
            ..Default::default()
        };

        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
            depth_test_enable: vk::TRUE,
            depth_write_enable: vk::FALSE,
            depth_compare_op: vk::CompareOp::LessOrEqual,
            ..Default::default()
        };

        let blend_attachment = vk::PipelineColorBlendAttachmentState {
            blend_enable: vk::TRUE,
            src_color_blend_factor: vk::BlendFactor::SrcAlpha,
            dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
            color_blend_op: vk::BlendOp::Add,
            src_alpha_blend_factor: vk::BlendFactor::One,
            dst_alpha_blend_factor: vk::BlendFactor::Zero,
            alpha_blend_op: vk::BlendOp::Add,
            color_write_mask: vk::ColorComponentFlags::RGBA,
        };
        let color_blending = vk::PipelineColorBlendStateCreateInfo {
            attachment_count: 1,
            attachments: &blend_attachment,
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
            layout: pipeline_layout,
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
            .unwrap();

        device.destroy_shader_module(vert_mod, None);
        device.destroy_shader_module(frag_mod, None);

        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UniformBuffer,
            descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: MAX_FRAMES_IN_FLIGHT as u32,
            pool_size_count: 1,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let desc_pool = device.create_descriptor_pool(&pool_info, None).unwrap();

        let layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| desc_layout).collect();
        let alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool: desc_pool,
            descriptor_set_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let mut desc_sets = vec![vk::DescriptorSet::null(); layouts.len()];
        device
            .allocate_descriptor_sets(&alloc_info, &mut desc_sets)
            .unwrap();

        let mut camera_buffers = Vec::new();
        let mut camera_allocs = Vec::new();
        for desc_set in &desc_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "chunk_border_cam",
            );
            let buf_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: size_of::<CameraUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: *desc_set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buf_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            camera_buffers.push(buf);
            camera_allocs.push(alloc);
        }

        let max_verts = 4096;
        let (vertex_buffer, vertex_alloc) = util::create_host_buffer(
            device,
            allocator,
            (max_verts * size_of::<LineVertex>()) as u64,
            vk::BufferUsageFlags::VertexBuffer,
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
        data[..size_of::<CameraUniform>()].copy_from_slice(bytemuck::bytes_of(uniform));
    }

    /// Lines are eye-relative (including the third-person offset, the origin
    /// the view matrix renders from), subtracted in f64 so they stay on the
    /// grid at extreme coordinates. The grid cell comes from `pivot` (the
    /// player eye): vanilla outlines the entity's chunk, not the camera's.
    pub fn update_lines(&mut self, pivot: glam::DVec3, eye: glam::DVec3, min_y: i32, max_y: i32) {
        let chunk_x = (pivot.x.floor() as i32).div_euclid(16) * 16;
        let chunk_z = (pivot.z.floor() as i32).div_euclid(16) * 16;

        let mut verts: Vec<LineVertex> = Vec::new();
        let y_min = min_y as f64;
        let y_max = max_y as f64;

        let push_line = |verts: &mut Vec<LineVertex>,
                         x0: f64,
                         y0: f64,
                         z0: f64,
                         x1: f64,
                         y1: f64,
                         z1: f64,
                         color: [f32; 4]| {
            verts.push(LineVertex {
                position: [
                    (x0 - eye.x) as f32,
                    (y0 - eye.y) as f32,
                    (z0 - eye.z) as f32,
                ],
                color,
            });
            verts.push(LineVertex {
                position: [
                    (x1 - eye.x) as f32,
                    (y1 - eye.y) as f32,
                    (z1 - eye.z) as f32,
                ],
                color,
            });
        };

        // Vertical lines at chunk corners (red)
        for dx in [0, 16] {
            for dz in [0, 16] {
                let x = (chunk_x + dx) as f64;
                let z = (chunk_z + dz) as f64;
                push_line(&mut verts, x, y_min, z, x, y_max, z, RED);
            }
        }

        // Vertical lines along chunk edges (blue) - every block along edges
        for d in 1..16 {
            let x0 = chunk_x as f64;
            let x1 = (chunk_x + 16) as f64;
            let z0 = chunk_z as f64;
            let z1 = (chunk_z + 16) as f64;
            let p = (chunk_x + d) as f64;
            let q = (chunk_z + d) as f64;
            push_line(&mut verts, p, y_min, z0, p, y_max, z0, BLUE);
            push_line(&mut verts, p, y_min, z1, p, y_max, z1, BLUE);
            push_line(&mut verts, x0, y_min, q, x0, y_max, q, BLUE);
            push_line(&mut verts, x1, y_min, q, x1, y_max, q, BLUE);
        }

        // Horizontal lines at section boundaries (yellow)
        for section in 0..=((max_y - min_y) / 16) {
            let y = (min_y + section * 16) as f64;
            let x0 = chunk_x as f64;
            let x1 = (chunk_x + 16) as f64;
            let z0 = chunk_z as f64;
            let z1 = (chunk_z + 16) as f64;
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

    pub fn draw(&self, cmd: vk::CommandBuffer, frame: usize) {
        if self.vertex_count == 0 {
            return;
        }
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.desc_sets[frame]],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.draw(self.vertex_count, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        device.destroy_buffer(self.vertex_buffer, None);

        alloc
            .free(std::mem::replace(&mut self.vertex_alloc, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);

            alloc
                .free(std::mem::replace(&mut self.camera_allocs[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.desc_pool, None);
        device.destroy_descriptor_set_layout(self.desc_layout, None);
    }
}
