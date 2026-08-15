use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::Allocator;
use pyronyx::vk;

use crate::renderer::buffer::Buffer;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader};

pub struct OcclusionPipeline {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    resource_layout: vk::DescriptorSetLayout,
    pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    cube_indices: Buffer,
}

impl OcclusionPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        camera_layout: vk::DescriptorSetLayout,
        representative_fragment_test: bool,
    ) -> Self {
        let bindings = [
            vk::DescriptorSetLayoutBinding {
                binding: 0,
                descriptor_type: vk::DescriptorType::StorageBuffer,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Vertex,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 1,
                descriptor_type: vk::DescriptorType::StorageBuffer,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Vertex,
                ..Default::default()
            },
            vk::DescriptorSetLayoutBinding {
                binding: 2,
                descriptor_type: vk::DescriptorType::StorageBuffer,
                descriptor_count: 1,
                stage_flags: vk::ShaderStageFlags::Fragment,
                ..Default::default()
            },
        ];
        let resource_layout = device
            .create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo {
                    binding_count: bindings.len() as u32,
                    bindings: bindings.as_ptr(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let layouts = [camera_layout, resource_layout];
        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex,
            offset: 0,
            size: size_of::<u32>() as u32,
        };
        let layout = device
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo {
                    set_layout_count: 2,
                    set_layouts: layouts.as_ptr(),
                    push_constant_range_count: 1,
                    push_constant_ranges: &push_range,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let pool = device
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo {
                    max_sets: (2 * MAX_FRAMES_IN_FLIGHT) as u32,
                    pool_size_count: 1,
                    pool_sizes: &vk::DescriptorPoolSize {
                        ty: vk::DescriptorType::StorageBuffer,
                        descriptor_count: (6 * MAX_FRAMES_IN_FLIGHT) as u32,
                    },
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let set_layouts = vec![resource_layout; 2 * MAX_FRAMES_IN_FLIGHT];
        let mut sets = vec![vk::DescriptorSet::null(); set_layouts.len()];
        device
            .allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo {
                    descriptor_pool: pool,
                    descriptor_set_count: sets.len() as u32,
                    set_layouts: set_layouts.as_ptr(),
                    ..Default::default()
                },
                &mut sets,
            )
            .unwrap();

        let indices: [u16; 36] = [
            0, 2, 1, 1, 2, 3, 4, 5, 6, 5, 7, 6, 0, 1, 4, 1, 5, 4, 2, 6, 3, 3, 6, 7, 0, 4, 2, 2, 4,
            6, 1, 3, 5, 3, 7, 5,
        ];
        let cube_indices = Buffer::mapped(
            device,
            allocator,
            bytemuck::cast_slice(&indices),
            vk::BufferUsageFlags::IndexBuffer,
            "occlusion_cube_indices",
        );

        let vert = shader::create_shader_module(device, shader::include_spirv!("aabb.vert.spv"));
        let frag = shader::create_shader_module(device, shader::include_spirv!("aabb.frag.spv"));
        let stages = [
            vk::PipelineShaderStageCreateInfo {
                stage: vk::ShaderStageFlags::Vertex,
                module: vert,
                name: c"main".as_ptr(),
                ..Default::default()
            },
            vk::PipelineShaderStageCreateInfo {
                stage: vk::ShaderStageFlags::Fragment,
                module: frag,
                name: c"main".as_ptr(),
                ..Default::default()
            },
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo {
            topology: vk::PrimitiveTopology::TriangleList,
            ..Default::default()
        };
        let viewport = vk::PipelineViewportStateCreateInfo {
            viewport_count: 1,
            scissor_count: 1,
            ..Default::default()
        };
        let raster = vk::PipelineRasterizationStateCreateInfo {
            polygon_mode: vk::PolygonMode::Fill,
            cull_mode: vk::CullModeFlags::None,
            front_face: vk::FrontFace::CounterClockwise,
            line_width: 1.0,
            ..Default::default()
        };
        let ms = vk::PipelineMultisampleStateCreateInfo {
            rasterization_samples: vk::SampleCountFlags::Type1,
            ..Default::default()
        };
        let depth = vk::PipelineDepthStencilStateCreateInfo {
            depth_test_enable: vk::TRUE,
            depth_write_enable: vk::FALSE,
            depth_compare_op: vk::CompareOp::GreaterOrEqual,
            ..Default::default()
        };
        let attachment = [vk::PipelineColorBlendAttachmentState {
            color_write_mask: vk::ColorComponentFlags::empty(),
            ..Default::default()
        }];
        let blend = vk::PipelineColorBlendStateCreateInfo {
            attachment_count: 1,
            attachments: attachment.as_ptr(),
            ..Default::default()
        };
        let dynamics = [vk::DynamicState::Viewport, vk::DynamicState::Scissor];
        let dynamic = vk::PipelineDynamicStateCreateInfo {
            dynamic_state_count: 2,
            dynamic_states: dynamics.as_ptr(),
            ..Default::default()
        };
        let info = vk::GraphicsPipelineCreateInfo {
            stage_count: 2,
            stages: stages.as_ptr(),
            vertex_input_state: &vertex_input,
            input_assembly_state: &assembly,
            viewport_state: &viewport,
            rasterization_state: &raster,
            multisample_state: &ms,
            depth_stencil_state: &depth,
            color_blend_state: &blend,
            dynamic_state: &dynamic,
            layout,
            render_pass,
            subpass: 0,
            ..Default::default()
        };
        let mut representative = vk::PipelineRepresentativeFragmentTestStateCreateInfoNV {
            representative_fragment_test_enable: if representative_fragment_test {
                vk::TRUE
            } else {
                vk::FALSE
            },
            ..Default::default()
        };
        let info = [if representative_fragment_test {
            info.next(&mut representative)
        } else {
            info
        }];
        let mut pipeline = vk::Pipeline::null();
        device
            .create_graphics_pipelines(
                vk::PipelineCache::null(),
                &info,
                None,
                slice::from_mut(&mut pipeline),
            )
            .unwrap();
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Self {
            pipeline,
            layout,
            resource_layout,
            pool,
            sets,
            cube_indices,
        }
    }

    pub fn draw(
        &self,
        device: &vk::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
        sections: bool,
        camera_set: vk::DescriptorSet,
        resources: (vk::Buffer, vk::Buffer, vk::Buffer, vk::Buffer),
    ) {
        let set = self.sets[frame * 2 + usize::from(sections)];
        let buffers = [resources.0, resources.1, resources.2];
        let infos: Vec<_> = buffers
            .iter()
            .map(|&buffer| vk::DescriptorBufferInfo {
                buffer,
                offset: 0,
                range: vk::WHOLE_SIZE,
            })
            .collect();
        let writes: Vec<_> = infos
            .iter()
            .enumerate()
            .map(|(binding, info)| vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: binding as u32,
                descriptor_type: vk::DescriptorType::StorageBuffer,
                descriptor_count: 1,
                buffer_info: info,
                ..Default::default()
            })
            .collect();
        device.update_descriptor_sets(&writes, &[]);
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.layout,
            0,
            &[camera_set, set],
            &[],
        );
        let word_stride = if sections { 16u32 } else { 12u32 };
        cmd.push_constants(
            self.layout,
            vk::ShaderStageFlags::Vertex,
            0,
            bytemuck::bytes_of(&word_stride),
        );
        cmd.bind_index_buffer(self.cube_indices.buffer, 0, vk::IndexType::Uint16);
        cmd.draw_indexed_indirect(resources.3, 0, 1, 20);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        std::mem::take(&mut self.cube_indices).destroy(device, allocator);
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.layout, None);
        device.destroy_descriptor_pool(self.pool, None);
        device.destroy_descriptor_set_layout(self.resource_layout, None);
    }
}
