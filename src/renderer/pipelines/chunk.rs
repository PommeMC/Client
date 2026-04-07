use std::sync::{Arc, Mutex};

use ash::vk;
use gpu_allocator::vulkan::{Allocation, Allocator};

use crate::renderer::MAX_FRAMES_IN_FLIGHT;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::shader;
use crate::renderer::util;

pub struct ChunkPipeline {
    pub pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    pub descriptor_set_layout_camera: vk::DescriptorSetLayout,
    pub descriptor_set_layout_atlas: vk::DescriptorSetLayout,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    pub atlas_view: vk::ImageView,
    pub atlas_sampler: vk::Sampler,
}

impl ChunkPipeline {
    pub fn new(
        device: &ash::Device,
        color_format: vk::Format,
        depth_format: vk::Format,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UNIFORM_BUFFER,
            vk::ShaderStageFlags::VERTEX,
        );
        let atlas_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            vk::ShaderStageFlags::FRAGMENT,
        );

        let layouts = [camera_layout, atlas_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        let pipeline_layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .expect("failed to create pipeline layout");

        let pipeline = create_pipeline(device, color_format, depth_format, pipeline_layout);

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "camera_uniform",
            );
            camera_buffers.push(buf);
            camera_allocations.push(alloc);
        }

        Self {
            pipeline,
            pipeline_layout,
            descriptor_set_layout_camera: camera_layout,
            descriptor_set_layout_atlas: atlas_layout,
            camera_buffers,
            camera_allocations,
            atlas_view: atlas.view,
            atlas_sampler: atlas.sampler,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn rebind_atlas(&mut self, _device: &ash::Device, atlas: &TextureAtlas) {
        self.atlas_view = atlas.view;
        self.atlas_sampler = atlas.sampler;
    }

    pub fn bind(
        &self,
        device: &ash::Device,
        push_desc: &ash::khr::push_descriptor::Device,
        cmd: vk::CommandBuffer,
        frame: usize,
    ) {
        let buffer_info = [vk::DescriptorBufferInfo {
            buffer: self.camera_buffers[frame],
            offset: 0,
            range: std::mem::size_of::<CameraUniform>() as u64,
        }];
        let camera_write = vk::WriteDescriptorSet::default()
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
            .buffer_info(&buffer_info);

        let image_info = [vk::DescriptorImageInfo {
            sampler: self.atlas_sampler,
            image_view: self.atlas_view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let atlas_write = vk::WriteDescriptorSet::default()
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info);

        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            push_desc.cmd_push_descriptor_set(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                0,
                &[camera_write],
            );
            push_desc.cmd_push_descriptor_set(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.pipeline_layout,
                1,
                &[atlas_write],
            );
        }
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
        drop(alloc);

        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout_camera, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout_atlas, None);
        }
    }
}

fn create_pipeline(
    device: &ash::Device,
    color_format: vk::Format,
    depth_format: vk::Format,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("chunk.vert.spv");
    let frag_spv = shader::include_spirv!("chunk.frag.spv");

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

    use crate::renderer::chunk::mesher::ChunkVertex;
    let binding_descs = [ChunkVertex::binding_description()];
    let attr_descs = ChunkVertex::attribute_descriptions();

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
        .cull_mode(vk::CullModeFlags::BACK)
        .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
        .line_width(1.0);

    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);

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

    let color_formats = [color_format];
    let mut rendering_info = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(depth_format);

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
        .push_next(&mut rendering_info)];

    let pipeline = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
    }
    .expect("failed to create chunk pipeline")[0];

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    pipeline
}
