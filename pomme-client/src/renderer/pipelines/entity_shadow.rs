//! Entity shadows: vanilla `EntityRenderer.extractShadow` and
//! `ShadowFeatureRenderer` (26.2), one quad per lit, full block under an
//! entity.

use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use glam::{DVec3, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::physics::block_shape::{LocalBox, is_collision_shape_full_block, outline_shape};
use crate::renderer::camera::CameraUniform;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::world::block::block_id;
use crate::world::block_entity::is_invisible_block;
use crate::world::chunk::ChunkStore;
use crate::world::light::lightmap_brightness;

/// Something that casts a shadow, with vanilla's `getShadowRadius` and
/// `getShadowStrength`; radius 0 casts none.
#[derive(Clone, Copy, Default)]
pub struct ShadowCaster {
    pub position: DVec3,
    /// Vanilla's render offset (a pufferfish's bob, an enderman's shake): it
    /// moves the quads, not which blocks they land on.
    pub offset: DVec3,
    pub radius: f32,
    pub strength: f32,
}

/// What a frame's shadows read: who casts them, onto which blocks, and the
/// light those blocks are in.
#[derive(Clone, Copy)]
pub struct ShadowScene<'a> {
    pub casters: &'a [ShadowCaster],
    pub chunks: &'a ChunkStore,
    pub sky_darken: u8,
    pub ambient_light: f32,
}

/// Per-vertex data for shadow quads.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// Two triangles per shadow piece.
const VERTS_PER_QUAD: usize = 6;
const MAX_SHADOW_QUADS: usize = 2730;
const MAX_SHADOW_VERTS: usize = MAX_SHADOW_QUADS * VERTS_PER_QUAD;
/// Vanilla `EntityRenderer.MAX_SHADOW_RADIUS`.
const MAX_SHADOW_RADIUS: f32 = 32.0;

pub struct EntityShadowPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    texture_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    vertex_buffers: Vec<vk::Buffer>,
    vertex_allocations: Vec<Allocation>,
    texture_image: vk::Image,
    texture_view: vk::ImageView,
    texture_sampler: vk::Sampler,
    texture_allocation: Allocation,
    /// This frame's vertices, kept between frames for its capacity.
    verts: Vec<ShadowVertex>,
}

impl EntityShadowPipeline {
    pub fn new(
        device: &vk::Device,
        queue: vk::Queue,
        command_pool: vk::CommandPool,
        render_pass: vk::RenderPass,
        allocator: &Arc<Mutex<Allocator>>,
        jar_assets_dir: &Path,
        asset_index: &Option<AssetIndex>,
    ) -> Self {
        let camera_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::UniformBuffer,
            vk::ShaderStageFlags::Vertex,
        );
        let texture_layout = util::create_descriptor_set_layout(
            device,
            vk::DescriptorType::CombinedImageSampler,
            vk::ShaderStageFlags::Fragment,
        );

        let layouts = [camera_layout, texture_layout];
        let layout_info = vk::PipelineLayoutCreateInfo {
            set_layout_count: layouts.len() as u32,
            set_layouts: layouts.as_ptr(),
            push_constant_range_count: 0,
            push_constant_ranges: std::ptr::null(),
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create entity shadow pipeline layout");

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
            .expect("failed to create entity shadow descriptor pool");

        let cam_layouts: Vec<_> = (0..MAX_FRAMES_IN_FLIGHT).map(|_| camera_layout).collect();
        let cam_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: cam_layouts.len() as u32,
            set_layouts: cam_layouts.as_ptr(),
            ..Default::default()
        };
        let mut camera_sets = vec![vk::DescriptorSet::null(); cam_layouts.len()];
        device
            .allocate_descriptor_sets(&cam_alloc_info, &mut camera_sets)
            .expect("failed to allocate entity shadow camera sets");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &texture_layout,
            ..Default::default()
        };
        let mut texture_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
            .expect("failed to allocate entity shadow texture set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "entity_shadow_camera",
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

        let (texture_image, texture_view, texture_allocation) = load_shadow_texture(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let texture_sampler = unsafe { util::create_linear_sampler(device) };

        let image_info = vk::DescriptorImageInfo {
            sampler: texture_sampler,
            image_view: texture_view,
            image_layout: vk::ImageLayout::ShaderReadOnlyOptimal,
        };
        let tex_write = vk::WriteDescriptorSet {
            dst_set: texture_set,
            dst_binding: 0,
            descriptor_type: vk::DescriptorType::CombinedImageSampler,
            descriptor_count: 1,
            buffer_info: std::ptr::null(),
            image_info: &image_info,
            ..Default::default()
        };
        device.update_descriptor_sets(&[tex_write], &[]);

        let placeholder = vec![
            ShadowVertex {
                position: [0.0; 3],
                color: [0.0; 4],
                uv: [0.0; 2],
            };
            MAX_SHADOW_VERTS
        ];
        let bytes = bytemuck::cast_slice::<ShadowVertex, u8>(&placeholder);
        let mut vertex_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut vertex_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for _ in 0..MAX_FRAMES_IN_FLIGHT {
            let (buf, alloc) = util::create_mapped_buffer(
                device,
                allocator,
                bytes,
                vk::BufferUsageFlags::VertexBuffer,
                "entity_shadow_vertices",
            );
            vertex_buffers.push(buf);
            vertex_allocations.push(alloc);
        }

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
            vertex_buffers,
            vertex_allocations,
            texture_image,
            texture_view,
            texture_sampler,
            texture_allocation,
            verts: Vec::with_capacity(MAX_SHADOW_VERTS),
        }
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        scene: &ShadowScene,
        anchor: DVec3,
        camera_pos: DVec3,
    ) {
        self.verts.clear();
        for caster in scene.casters {
            if !extract_shadow(&mut self.verts, scene, caster, anchor, camera_pos) {
                break;
            }
        }
        if self.verts.is_empty() {
            return;
        }

        let bytes = bytemuck::cast_slice::<ShadowVertex, u8>(&self.verts);
        self.vertex_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.texture_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffers[frame]], &[0]);
        cmd.draw(self.verts.len() as u32, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &vk::Device, allocator: &Arc<Mutex<Allocator>>) {
        let mut alloc = allocator.lock().unwrap();
        for buffer in self
            .camera_buffers
            .drain(..)
            .chain(self.vertex_buffers.drain(..))
        {
            device.destroy_buffer(buffer, None);
        }
        for allocation in self
            .camera_allocations
            .drain(..)
            .chain(self.vertex_allocations.drain(..))
            .chain([std::mem::take(&mut self.texture_allocation)])
        {
            alloc.free(allocation).ok();
        }
        drop(alloc);

        device.destroy_sampler(self.texture_sampler, None);
        device.destroy_image_view(self.texture_view, None);
        device.destroy_image(self.texture_image, None);
        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.texture_layout, None);
    }
}

/// Vanilla `extractShadow`, appending one quad per `extractShadowPiece`;
/// `false` once the buffer is full.
fn extract_shadow(
    verts: &mut Vec<ShadowVertex>,
    scene: &ShadowScene,
    caster: &ShadowCaster,
    anchor: DVec3,
    camera_pos: DVec3,
) -> bool {
    let radius = caster.radius.min(MAX_SHADOW_RADIUS);
    if radius <= 0.0 {
        return true;
    }
    let pos = caster.position;
    let pow =
        ((1.0 - pos.distance_squared(camera_pos) / 256.0) * f64::from(caster.strength)) as f32;
    if pow <= 0.0 {
        return true;
    }
    let r = f64::from(radius);
    let depth = (pow / 0.5 - 1.0).min(radius);
    let y0 = (pos.y - f64::from(depth)).floor() as i32;
    let y1 = pos.y.floor() as i32;
    let origin = (pos + caster.offset - anchor).as_vec3();
    for z in (pos.z - r).floor() as i32..=(pos.z + r).floor() as i32 {
        for x in (pos.x - r).floor() as i32..=(pos.x + r).floor() as i32 {
            for y in y0..=y1 {
                let Some((shape, alpha)) = shadow_piece(scene, pos, pow, x, y, z) else {
                    continue;
                };
                if verts.len() + VERTS_PER_QUAD > MAX_SHADOW_VERTS {
                    return false;
                }
                let rel = Vec3::new(
                    (f64::from(x) - pos.x) as f32,
                    (f64::from(y) - pos.y) as f32,
                    (f64::from(z) - pos.z) as f32,
                );
                push_quad(verts, origin, rel, shape, radius, alpha);
            }
        }
    }
    true
}

/// Vanilla `extractShadowPiece` for the cell at (x, y, z): the bounds of the
/// block below and the piece's alpha, or `None` where no shadow lands.
fn shadow_piece(
    scene: &ShadowScene,
    pos: DVec3,
    pow: f32,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(LocalBox, f32)> {
    let power_at_depth = pow - (pos.y - f64::from(y)) as f32 * 0.5;
    let below = scene.chunks.get_block_state(x, y - 1, z);
    if !takes_a_shadow(below) {
        return None;
    }
    let brightness = scene
        .chunks
        .max_local_raw_brightness(x, y, z, scene.sky_darken);
    if brightness <= 3 {
        return None;
    }
    let shape = bounds(outline_shape(below))?;
    let alpha = (power_at_depth * 0.5 * lightmap_brightness(brightness, scene.ambient_light))
        .clamp(0.0, 1.0);
    Some((shape, alpha))
}

/// Whether a shadow can lie on `state`: it renders, and fills its cell.
fn takes_a_shadow(state: azalea_block::BlockState) -> bool {
    !is_invisible_block(block_id(state)) && is_collision_shape_full_block(state)
}

/// Vanilla `ShadowFeatureRenderer`: the piece's quad, `rel` from the caster
/// to the cell and `origin` from the anchor to the caster.
fn push_quad(
    verts: &mut Vec<ShadowVertex>,
    origin: Vec3,
    rel: Vec3,
    shape: LocalBox,
    radius: f32,
    alpha: f32,
) {
    let x0 = rel.x + shape[0] as f32;
    let x1 = rel.x + shape[3] as f32;
    let y = rel.y + shape[1] as f32;
    let z0 = rel.z + shape[2] as f32;
    let z1 = rel.z + shape[5] as f32;
    let uv = |p: f32| -p / (2.0 * radius) + 0.5;
    let corner = |x: f32, z: f32| ShadowVertex {
        position: (origin + Vec3::new(x, y, z)).to_array(),
        color: [1.0, 1.0, 1.0, alpha],
        uv: [uv(x), uv(z)],
    };
    let [c0, c1, c2, c3] = [
        corner(x0, z0),
        corner(x0, z1),
        corner(x1, z1),
        corner(x1, z0),
    ];
    verts.extend_from_slice(&[c0, c1, c2, c0, c2, c3]);
}

/// Vanilla `VoxelShape.bounds`: the box around every part of the shape.
fn bounds(boxes: &[LocalBox]) -> Option<LocalBox> {
    let (&first, rest) = boxes.split_first()?;
    Some(rest.iter().fold(first, |mut b, part| {
        for i in 0..3 {
            b[i] = b[i].min(part[i]);
            b[i + 3] = b[i + 3].max(part[i + 3]);
        }
        b
    }))
}

fn load_shadow_texture(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation) {
    let key = "minecraft/textures/misc/shadow.png";
    let path = resolve_asset_path(jar_assets_dir, asset_index, key);
    let (pixels, w, h) = if let Some((p, width, height)) = util::load_png(&path) {
        (p, width, height)
    } else {
        tracing::warn!(
            "Failed to load shadow texture from {}, using fallback",
            path.display()
        );
        // 64x64 fallback circular gradient
        let mut p = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64 {
            for x in 0..64 {
                let dx = (x as f32 - 31.5) / 32.0;
                let dy = (y as f32 - 31.5) / 32.0;
                let dist = (dx * dx + dy * dy).sqrt();
                let a = ((1.0 - dist).clamp(0.0, 1.0) * 255.0) as u8;
                p.extend_from_slice(&[0, 0, 0, a]);
            }
        }
        (p, 64, 64)
    };

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, w, h, "entity_shadow_texture");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &pixels, "entity_shadow_staging");

    util::upload_image(device, queue, command_pool, staging_buf, image, w, h);

    device.destroy_buffer(staging_buf, None);
    let _ = allocator.lock().unwrap().free(staging_alloc);

    (image, view, allocation)
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("entity_shadow.vert.spv");
    let frag_spv = shader::include_spirv!("entity_shadow.frag.spv");

    let vert_module = shader::create_shader_module(device, vert_spv);
    let frag_module = shader::create_shader_module(device, frag_spv);

    let stages = [
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Vertex,
            module: vert_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
        vk::PipelineShaderStageCreateInfo {
            stage: vk::ShaderStageFlags::Fragment,
            module: frag_module,
            name: c"main".as_ptr(),
            ..Default::default()
        },
    ];

    let binding_desc = vk::VertexInputBindingDescription {
        binding: 0,
        stride: size_of::<ShadowVertex>() as u32,
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
        vk::VertexInputAttributeDescription {
            location: 2,
            binding: 0,
            format: vk::Format::R32G32Sfloat,
            offset: 28,
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
        topology: vk::PrimitiveTopology::TriangleList,
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
        layout,
        render_pass,
        subpass: 0,
        ..Default::default()
    }];

    let mut pipelines = vec![vk::Pipeline::null()];
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            &mut pipelines,
        )
        .expect("failed to create entity shadow graphics pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipelines[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::find_state;

    #[test]
    fn shadows_land_only_on_full_visible_blocks() {
        crate::world::block::init("26.2");
        for (name, props, expected) in [
            ("stone", &[][..], true),
            ("grass_block", &[("snowy", "false")][..], true),
            (
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")][..],
                false,
            ),
            (
                "stone_slab",
                &[("type", "double"), ("waterlogged", "false")][..],
                true,
            ),
            ("short_grass", &[][..], false),
            ("barrier", &[("waterlogged", "false")][..], false),
            ("water", &[("level", "0")][..], false),
        ] {
            assert_eq!(
                takes_a_shadow(find_state(name, props)),
                expected,
                "{name} {props:?}"
            );
        }
    }

    #[test]
    fn bounds_wrap_every_part() {
        let boxes = [
            [0.0, 0.0, 0.0, 1.0, 0.5, 1.0],
            [0.25, 0.5, 0.25, 0.75, 1.0, 0.75],
        ];
        assert_eq!(bounds(&boxes), Some([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]));
        assert_eq!(bounds(&[]), None);
    }
}
