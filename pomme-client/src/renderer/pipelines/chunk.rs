use std::slice;
use std::sync::{Arc, Mutex};

use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

pub struct ChunkPipeline {
    /// Opaque terrain: no discard, early-Z. Drawn first (front-to-back).
    pub pipeline_solid: vk::Pipeline,
    /// Cutout terrain: alpha-test discard. Drawn after solid.
    pub pipeline_cutout: vk::Pipeline,
    /// Translucent water variant: alpha blending on, depth write off. Shares
    /// the terrain pipelines' layout and descriptor sets.
    pub water_pipeline: vk::Pipeline,
    pub pipeline_layout: vk::PipelineLayout,
    pub descriptor_set_layout_camera: vk::DescriptorSetLayout,
    pub descriptor_set_layout_atlas: vk::DescriptorSetLayout,
    pub descriptor_pool: vk::DescriptorPool,
    pub camera_sets: Vec<vk::DescriptorSet>,
    pub atlas_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
}

impl ChunkPipeline {
    pub fn new(
        device: &vk::Device,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        atlas: &TextureAtlas,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let atlas_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [camera_layout, atlas_layout];
        // Water pushes its section origin + fade per draw (water.vert); the
        // indirect passes read them per-instance and ignore the range.
        let push_range = vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::Vertex,
            offset: 0,
            size: 16,
        };
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 1,
            push_constant_ranges: &push_range,
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create pipeline layout");

        let (pipeline_solid, pipeline_cutout) =
            create_pipelines(device, render_pass, pipeline_layout);
        let water_pipeline = create_water_pipeline(device, render_pass, pipeline_layout);

        let pool_sizes = [
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::UniformBuffer,
                descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
            },
            vk::DescriptorPoolSize {
                ty: vk::DescriptorType::CombinedImageSampler,
                descriptor_count: 1,
            },
        ];
        let pool_info = vk::DescriptorPoolCreateInfo {
            max_sets: (MAX_FRAMES_IN_FLIGHT + 1) as u32,
            pool_size_count: pool_sizes.len() as u32,
            pool_sizes: pool_sizes.as_ptr(),
            ..Default::default()
        };
        let descriptor_pool = device
            .create_descriptor_pool(&pool_info, None)
            .expect("failed to create descriptor pool");

        let camera_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let camera_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: camera_layouts.len() as u32,
            set_layouts: camera_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); camera_layouts.len()];
        device
            .allocate_descriptor_sets(&camera_alloc_info, &mut camera_sets)
            .expect("failed to allocate camera descriptor sets");

        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        let mut atlas_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut atlas_set))
            .expect("failed to allocate atlas descriptor set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "camera_uniform",
            );

            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: size_of::<CameraUniform>() as u64,
            };
            let write = vk::WriteDescriptorSet {
                dst_set: set,
                dst_binding: 0,
                descriptor_type: vk::DescriptorType::UniformBuffer,
                descriptor_count: 1,
                buffer_info: &buffer_info,
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);

            camera_buffers.push(buf);
            camera_allocations.push(alloc);
        }

        let image_info = vk::DescriptorImageInfo {
            sampler: atlas.sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let atlas_write = vk::WriteDescriptorSet {
            dst_set: atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[atlas_write], &[]);

        Self {
            pipeline_solid,
            pipeline_cutout,
            water_pipeline,
            pipeline_layout,
            descriptor_set_layout_camera: camera_layout,
            descriptor_set_layout_atlas: atlas_layout,
            descriptor_pool,
            camera_sets,
            atlas_set,
            camera_buffers,
            camera_allocations,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn rebind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
        let image_info = vk::DescriptorImageInfo {
            sampler: atlas.sampler,
            image_view: atlas.view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let write = vk::WriteDescriptorSet {
            dst_set: self.atlas_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[write], &[]);
    }

    pub fn bind(&self, cmd: vk::CommandBuffer, frame: usize, cutout: bool) {
        let pipeline = if cutout {
            self.pipeline_cutout
        } else {
            self.pipeline_solid
        };
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.atlas_set],
            &[],
        );
    }

    /// Bind the translucent water pipeline (same descriptor sets as `bind`).
    pub fn bind_water(&self, cmd: vk::CommandBuffer, frame: usize) {
        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.water_pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.atlas_set],
            &[],
        );
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            alloc
                .free(std::mem::replace(&mut self.camera_allocations[i], unsafe {
                    std::mem::zeroed()
                }))
                .ok();
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline_solid, None);
        device.destroy_pipeline(self.pipeline_cutout, None);
        device.destroy_pipeline(self.water_pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.descriptor_set_layout_camera, None);
        device.destroy_descriptor_set_layout(self.descriptor_set_layout_atlas, None);
    }
}

fn shader_stage(
    stage: vk::ShaderStageFlags,
    module: vk::ShaderModule,
) -> vk::PipelineShaderStageCreateInfo<'static> {
    vk::PipelineShaderStageCreateInfo {
        stage,
        module,
        name: c"main".as_ptr(),
        ..Default::default()
    }
}

/// Builds the two chunk pipelines: `solid` (chunk_solid.frag, no discard,
/// early-Z) and `cutout` (chunk.frag, alpha-test discard). Identical state
/// otherwise; both share the vertex shader and layout.
fn create_pipelines(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> (vk::Pipeline, vk::Pipeline) {
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
    }];
    let color_blend = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    let solid = create_chunk_variant(
        device,
        render_pass,
        layout,
        shader::include_spirv!("chunk.vert.spv"),
        shader::include_spirv!("chunk_solid.frag.spv"),
        &color_blend,
        true,
        true,
    );
    let cutout = create_chunk_variant(
        device,
        render_pass,
        layout,
        shader::include_spirv!("chunk.vert.spv"),
        shader::include_spirv!("chunk.frag.spv"),
        &color_blend,
        true,
        true,
    );
    (solid, cutout)
}

/// Translucent water: standard alpha blending, depth test on but depth write
/// off (so it never occludes geometry behind it).
fn create_water_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let blend_attachment = [vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::TRUE,
        src_color_blend_factor: vk::BlendFactor::SrcAlpha,
        dst_color_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        color_blend_op: vk::BlendOp::Add,
        src_alpha_blend_factor: vk::BlendFactor::One,
        dst_alpha_blend_factor: vk::BlendFactor::OneMinusSrcAlpha,
        alpha_blend_op: vk::BlendOp::Add,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }];
    let color_blend = vk::PipelineColorBlendStateCreateInfo {
        attachment_count: blend_attachment.len() as u32,
        attachments: blend_attachment.as_ptr(),
        ..Default::default()
    };

    create_chunk_variant(
        device,
        render_pass,
        layout,
        shader::include_spirv!("water.vert.spv"),
        shader::include_spirv!("water.frag.spv"),
        &color_blend,
        false,
        false,
    )
}

/// Build a chunk pipeline given the shader SPIR-V, color-blend, whether it
/// writes depth, and whether it reads the per-instance meta binding
/// (`chunk.vert`) or only the packed vertices (`water.vert`).
#[allow(clippy::too_many_arguments)]
fn create_chunk_variant(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    vert_spirv: &[u8],
    frag_spirv: &[u8],
    color_blend: &vk::PipelineColorBlendStateCreateInfo,
    depth_write: bool,
    instanced: bool,
) -> vk::Pipeline {
    let vert_module = shader::create_shader_module(device, vert_spirv);
    let frag_module = shader::create_shader_module(device, frag_spirv);

    let stages = [
        shader_stage(vk::ShaderStageFlags::Vertex, vert_module),
        shader_stage(vk::ShaderStageFlags::Fragment, frag_module),
    ];

    let pipeline = build_pipeline(
        device,
        render_pass,
        layout,
        &stages,
        color_blend,
        depth_write,
        instanced,
    );
    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);
    pipeline
}

/// Shared chunk pipeline state; callers supply the shader stages and
/// color-blend.
#[allow(clippy::too_many_arguments)]
fn build_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
    stages: &[vk::PipelineShaderStageCreateInfo],
    color_blend: &vk::PipelineColorBlendStateCreateInfo,
    depth_write: bool,
    instanced: bool,
) -> vk::Pipeline {
    use crate::renderer::chunk::buffer::{chunk_vertex_attributes, chunk_vertex_bindings};
    let binding_descs = chunk_vertex_bindings();
    let attr_descs = chunk_vertex_attributes();
    // Binding 0 / attributes 0-3 are the packed vertices; binding 1 /
    // attributes 4-5 the per-instance meta only the indirect passes use.
    let (binding_descs, attr_descs): (&[_], &[_]) = if instanced {
        (&binding_descs, &attr_descs)
    } else {
        (&binding_descs[..1], &attr_descs[..4])
    };
    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };

    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        primitive_restart_enable: vk::FALSE,
        ..Default::default()
    };

    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };

    let rasterizer = vk::PipelineRasterizationStateCreateInfo {
        polygon_mode: vk::PolygonMode::Fill,
        cull_mode: vk::CullModeFlags::Back,
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
        depth_write_enable: if depth_write { vk::TRUE } else { vk::FALSE },
        depth_compare_op: vk::CompareOp::LessOrEqual,
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
        color_blend_state: color_blend,
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
        .expect("failed to create chunk pipeline");
    pipeline
}
