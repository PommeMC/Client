use std::slice;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};

/// Vanilla `ParticleGroup.MAX_PARTICLES`: the particle store's hard cap,
/// and thus the per-frame vertex buffer size.
pub const MAX_PARTICLE_QUADS: usize = 16384;
const MAX_VERTS: usize = MAX_PARTICLE_QUADS * 6;

/// One camera-facing particle billboard, extracted per frame from the
/// particle store.
pub struct ParticleQuad {
    /// Partial-tick-lerped world-space position (quad center).
    pub pos: [f32; 3],
    /// Vanilla `quadSize`; the quad spans twice this.
    pub size: f32,
    pub u0: f32,
    pub u1: f32,
    pub v0: f32,
    pub v1: f32,
    /// Packed RGBA8; rgb already multiplied by tint and world light.
    pub color: u32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleVertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: u32,
}

pub struct ParticlePipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    atlas_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    atlas_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Option<Allocation>>,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Option<Allocation>>,
}

impl ParticlePipeline {
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
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create particle pipeline layout");

        let pipeline = create_pipeline(device, render_pass, pipeline_layout);

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
            .expect("failed to create particle descriptor pool");

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
            .expect("failed to allocate particle camera sets");

        let atlas_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &atlas_layout,
            ..Default::default()
        };
        let mut atlas_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&atlas_alloc_info, slice::from_mut(&mut atlas_set))
            .expect("failed to allocate particle atlas set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                std::mem::size_of::<CameraUniform>() as u64,
                "particle_camera",
            );
            let buffer_info = vk::DescriptorBufferInfo {
                buffer: buf,
                offset: 0,
                range: std::mem::size_of::<CameraUniform>() as u64,
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
            camera_allocations.push(Some(alloc));
        }

        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations: Vec<Option<Allocation>> =
            Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let vertex_bytes = (MAX_VERTS * std::mem::size_of::<ParticleVertex>()) as u64;
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_host_buffer(
                device,
                allocator,
                vertex_bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "particle_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(Some(alloc));
        }

        let this = Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            descriptor_pool,
            camera_sets,
            atlas_set,
            camera_buffers,
            camera_allocations,
            vertex_buffers,
            vertex_allocations,
        };
        this.bind_atlas(device, atlas);
        this
    }

    fn bind_atlas(&self, device: &vk::Device, atlas: &TextureAtlas) {
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

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        if let Some(alloc) = self.camera_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }
    }

    pub fn update_and_draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        camera: &Camera,
        quads: &[ParticleQuad],
    ) {
        if quads.is_empty() {
            return;
        }

        let (right, up) = camera.billboard_axes();
        let mut verts: Vec<ParticleVertex> =
            Vec::with_capacity(quads.len().min(MAX_PARTICLE_QUADS) * 6);
        for quad in quads.iter().take(MAX_PARTICLE_QUADS) {
            let center = Vec3::from(quad.pos);
            let corner = |nx: f32, ny: f32, u: f32, v: f32| ParticleVertex {
                position: (center + (right * nx + up * ny) * quad.size).into(),
                uv: [u, v],
                color: quad.color,
            };
            // Vanilla QuadParticleRenderState corner order and UV mapping.
            let corners = [
                corner(1.0, -1.0, quad.u1, quad.v1),
                corner(1.0, 1.0, quad.u1, quad.v0),
                corner(-1.0, 1.0, quad.u0, quad.v0),
                corner(-1.0, -1.0, quad.u0, quad.v1),
            ];
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(corners[i]);
            }
        }

        let bytes = bytemuck::cast_slice::<ParticleVertex, u8>(&verts);
        if let Some(alloc) = self.vertex_allocations[frame].as_mut() {
            alloc.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);
        }

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.atlas_set],
            &[],
        );
        cmd.draw(verts.len() as u32, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            device.destroy_buffer(self.camera_buffers[i], None);
            if let Some(a) = self.camera_allocations[i].take() {
                alloc.free(a).ok();
            }
            device.destroy_buffer(self.vertex_buffers[i], None);
            if let Some(a) = self.vertex_allocations[i].take() {
                alloc.free(a).ok();
            }
        }
        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.atlas_layout, None);
    }
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("particle.vert.spv");
    let frag_spv = shader::include_spirv!("particle.frag.spv");
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

    let binding_descs = [vk::VertexInputBindingDescription {
        binding: 0,
        stride: std::mem::size_of::<ParticleVertex>() as u32,
        input_rate: vk::VertexInputRate::Vertex,
    }];
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
            format: vk::Format::R32G32Sfloat,
            offset: 12,
        },
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R8G8B8A8Unorm,
            offset: 20,
        },
    ];
    let vertex_input = vk::PipelineVertexInputStateCreateInfo {
        vertex_binding_description_count: binding_descs.len() as u32,
        vertex_binding_descriptions: binding_descs.as_ptr(),
        vertex_attribute_description_count: attr_descs.len() as u32,
        vertex_attribute_descriptions: attr_descs.as_ptr(),
        ..Default::default()
    };
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo {
        topology: vk::PrimitiveTopology::TriangleList,
        ..Default::default()
    };
    let viewport_state = vk::PipelineViewportStateCreateInfo {
        viewport_count: 1,
        scissor_count: 1,
        ..Default::default()
    };
    // Billboards always face the camera, so culling is a no-op; vanilla's
    // back-face cull is skipped rather than fighting winding conventions.
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
    // Vanilla OPAQUE_PARTICLE: depth test AND write, no blending (alpha is
    // handled by the fragment discard).
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo {
        depth_test_enable: vk::TRUE,
        depth_write_enable: vk::TRUE,
        depth_compare_op: vk::CompareOp::Less,
        ..Default::default()
    };
    let blend_attachment = vk::PipelineColorBlendAttachmentState {
        blend_enable: vk::FALSE,
        color_write_mask: vk::ColorComponentFlags::RGBA,
        ..Default::default()
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

    let info = [vk::GraphicsPipelineCreateInfo {
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
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipeline = [vk::Pipeline::null()];
    device
        .create_graphics_pipelines(vk::PipelineCache::null(), &info, None, &mut pipeline)
        .expect("failed to create particle pipeline");

    device.destroy_shader_module(vert_mod, None);
    device.destroy_shader_module(frag_mod, None);

    pipeline[0]
}
