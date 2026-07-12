use std::f32::consts::{FRAC_PI_2, PI};
use std::path::Path;
use std::slice;
use std::sync::{Arc, Mutex};

use azalea_block::BlockState;
use azalea_core::position::BlockPos;
use glam::{Quat, Vec3};
use pomme_gpu_allocator::vulkan::{Allocation, Allocator};
use pyronyx::vk;

use crate::assets::{AssetIndex, resolve_asset_path};
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::mesher::{CUBE_FACE_DIRS, cube_face_geometry};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, shader, util};
use crate::world::block::model::{BakedQuad, Direction};
use crate::world::block::registry::BlockRegistry;

const STAGE_COUNT: u32 = 10;
/// Vertex-buffer capacity (a multiple of 6). Block models stay well under this;
/// an oversized model is capped with a warning rather than truncated silently.
const MAX_OVERLAY_VERTS: usize = 1536;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

pub struct BlockOverlayPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    camera_layout: vk::DescriptorSetLayout,
    texture_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    camera_sets: Vec<vk::DescriptorSet>,
    texture_set: vk::DescriptorSet,
    camera_buffers: Vec<vk::Buffer>,
    camera_allocations: Vec<Allocation>,
    vertex_buffer: vk::Buffer,
    vertex_allocation: Allocation,
    atlas_image: vk::Image,
    atlas_view: vk::ImageView,
    atlas_sampler: vk::Sampler,
    atlas_allocation: Allocation,
}

impl BlockOverlayPipeline {
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
            ..Default::default()
        };
        let pipeline_layout = device
            .create_pipeline_layout(&layout_info, None)
            .expect("failed to create block overlay pipeline layout");

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
            .expect("failed to create block overlay descriptor pool");

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
            .expect("failed to allocate block overlay camera sets");

        let tex_alloc_info = vk::DescriptorSetAllocateInfo {
            descriptor_pool,
            descriptor_set_count: 1,
            set_layouts: &texture_layout,
            ..Default::default()
        };
        let mut texture_set = vk::DescriptorSet::null();
        device
            .allocate_descriptor_sets(&tex_alloc_info, slice::from_mut(&mut texture_set))
            .expect("failed to allocate block overlay texture set");

        let mut camera_buffers = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        let mut camera_allocations = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);

        for &set in &camera_sets {
            let (buf, alloc) = util::create_uniform_buffer(
                device,
                allocator,
                size_of::<CameraUniform>() as u64,
                "block_overlay_camera",
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
                image_info: std::ptr::null(),
                ..Default::default()
            };
            device.update_descriptor_sets(&[write], &[]);
            camera_buffers.push(buf);
            camera_allocations.push(alloc);
        }

        let (atlas_image, atlas_view, atlas_allocation) = load_destroy_atlas(
            device,
            queue,
            command_pool,
            allocator,
            jar_assets_dir,
            asset_index,
        );

        let atlas_sampler = unsafe { util::create_nearest_sampler(device) };

        let image_info = vk::DescriptorImageInfo {
            sampler: atlas_sampler,
            image_view: atlas_view,
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
            OverlayVertex {
                position: [0.0; 3],
                uv: [0.0; 2],
            };
            MAX_OVERLAY_VERTS
        ];
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&placeholder);
        let (vertex_buffer, vertex_allocation) = util::create_mapped_buffer(
            device,
            allocator,
            bytes,
            vk::BufferUsageFlags::VertexBuffer,
            "block_overlay_vertices",
        );

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
            vertex_buffer,
            vertex_allocation,
            atlas_image,
            atlas_view,
            atlas_sampler,
            atlas_allocation,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_allocations[frame].mapped_slice_mut().unwrap()[..bytes.len()]
            .copy_from_slice(bytes);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: usize,
        registry: &BlockRegistry,
        state: BlockState,
        block_pos: &BlockPos,
        anchor: glam::DVec3,
        stage: u32,
    ) {
        let vertices = build_overlay_vertices(registry, state, block_pos, anchor, stage);
        if vertices.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&vertices);
        self.vertex_allocation.mapped_slice_mut().unwrap()[..bytes.len()].copy_from_slice(bytes);

        cmd.bind_pipeline(vk::PipelineBindPoint::Graphics, self.pipeline);
        cmd.bind_descriptor_sets(
            vk::PipelineBindPoint::Graphics,
            self.pipeline_layout,
            0,
            &[self.camera_sets[frame], self.texture_set],
            &[],
        );
        cmd.bind_vertex_buffers(0, &[self.vertex_buffer], &[0]);
        cmd.draw(vertices.len() as u32, 1, 0, 0);
    }

    pub fn recreate_pipeline(&mut self, device: &vk::Device, render_pass: vk::RenderPass) {
        device.destroy_pipeline(self.pipeline, None);
        self.pipeline = create_pipeline(device, render_pass, self.pipeline_layout);
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

        device.destroy_buffer(self.vertex_buffer, None);
        alloc
            .free(std::mem::replace(&mut self.vertex_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();

        device.destroy_sampler(self.atlas_sampler, None);
        device.destroy_image_view(self.atlas_view, None);

        alloc
            .free(std::mem::replace(&mut self.atlas_allocation, unsafe {
                std::mem::zeroed()
            }))
            .ok();
        device.destroy_image(self.atlas_image, None);

        drop(alloc);

        device.destroy_pipeline(self.pipeline, None);
        device.destroy_pipeline_layout(self.pipeline_layout, None);
        device.destroy_descriptor_pool(self.descriptor_pool, None);
        device.destroy_descriptor_set_layout(self.camera_layout, None);
        device.destroy_descriptor_set_layout(self.texture_layout, None);
    }
}

/// Builds the crumbling overlay for a block by re-tessellating its model with
/// vanilla's per-face crack projection (`SheetedDecalTextureGenerator`). There
/// is no geometry inflation: the overlay sits exactly on the block and relies
/// on the pipeline's polygon offset to avoid z-fighting, matching vanilla.
fn build_overlay_vertices(
    registry: &BlockRegistry,
    state: BlockState,
    pos: &BlockPos,
    anchor: glam::DVec3,
    stage: u32,
) -> Vec<OverlayVertex> {
    // Anchor-relative (see Camera::anchor); the crack-UV projection is
    // min-normalized per quad, so the rebase doesn't shift the texture.
    let origin = (glam::DVec3::new(pos.x as f64, pos.y as f64, pos.z as f64) - anchor)
        .as_vec3()
        .to_array();
    let mut verts = Vec::new();

    if let Some(model) = registry.get_baked_model(state) {
        for quad in &model.quads {
            push_quad(&mut verts, origin, quad, stage);
        }
    } else if let Some(quads) = registry.get_multipart_quads(state) {
        for quad in quads {
            push_quad(&mut verts, origin, quad, stage);
        }
    } else if registry.get_textures(state).is_some() {
        // Blocks rendered as a plain opaque cube (no baked model): crack a unit cube.
        for dir in CUBE_FACE_DIRS {
            let (positions, _, _) = cube_face_geometry(dir);
            push_face(&mut verts, origin, &positions, dir, stage);
        }
    }

    if verts.len() > MAX_OVERLAY_VERTS {
        tracing::warn!(
            "block overlay model emitted {} verts; capping at {MAX_OVERLAY_VERTS}",
            verts.len()
        );
        verts.truncate(MAX_OVERLAY_VERTS);
    }
    verts
}

fn push_quad(verts: &mut Vec<OverlayVertex>, origin: [f32; 3], quad: &BakedQuad, stage: u32) {
    let dir = quad
        .cullface
        .unwrap_or_else(|| nearest_direction(&quad.positions));
    push_face(verts, origin, &quad.positions, dir, stage);
}

/// Emits one quad (two triangles, six vertices) with crack UVs projected from
/// the vertex world positions for `dir`, normalized into this stage's atlas
/// tile.
fn push_face(
    verts: &mut Vec<OverlayVertex>,
    origin: [f32; 3],
    positions: &[[f32; 3]; 4],
    dir: Direction,
    stage: u32,
) {
    let world: [Vec3; 4] = std::array::from_fn(|i| {
        Vec3::new(
            origin[0] + positions[i][0],
            origin[1] + positions[i][1],
            origin[2] + positions[i][2],
        )
    });
    let mut uv: [[f32; 2]; 4] = std::array::from_fn(|i| project_crack_uv(world[i], dir));

    // 1 world unit maps to one crack tile, so a partial face shows a matching
    // fraction. Shift to the quad's min so the tile lands in [0,1] (the vertical
    // atlas can't wrap V across stages) and offset V into this stage's row.
    let min_u = uv.iter().map(|c| c[0]).fold(f32::INFINITY, f32::min);
    let min_v = uv.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
    let v_scale = 1.0 / STAGE_COUNT as f32;
    let v_base = stage as f32 * v_scale;
    for c in &mut uv {
        c[0] -= min_u;
        c[1] = v_base + (c[1] - min_v) * v_scale;
    }

    for &idx in &[0usize, 1, 2, 0, 2, 3] {
        verts.push(OverlayVertex {
            position: world[idx].to_array(),
            uv: uv[idx],
        });
    }
}

/// Vanilla `SheetedDecalTextureGenerator`: project the world position onto the
/// crack plane for `dir` (`rotateY(π)`, `rotateX(-π/2)`, then the face
/// rotation), taking `(-x, -y)` as the texture coordinate (texture scale 1.0).
fn project_crack_uv(world: Vec3, dir: Direction) -> [f32; 2] {
    let mut p = Quat::from_rotation_y(PI) * world;
    p = Quat::from_rotation_x(-FRAC_PI_2) * p;
    p = face_rotation(dir) * p;
    [-p.x, -p.y]
}

/// `Direction.getRotation()` (Direction.java:157).
fn face_rotation(dir: Direction) -> Quat {
    match dir {
        Direction::Down => Quat::from_rotation_x(PI),
        Direction::Up => Quat::IDENTITY,
        Direction::North => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(PI),
        Direction::South => Quat::from_rotation_x(FRAC_PI_2),
        Direction::West => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(FRAC_PI_2),
        Direction::East => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(-FRAC_PI_2),
    }
}

/// Nearest axis-aligned face to a quad's geometric normal, matching vanilla
/// `Direction.getApproximateNearest` (used for quads without a cull face).
fn nearest_direction(positions: &[[f32; 3]; 4]) -> Direction {
    let p0 = Vec3::from_array(positions[0]);
    let n = (Vec3::from_array(positions[1]) - p0).cross(Vec3::from_array(positions[2]) - p0);
    let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
    if ax >= ay && ax >= az {
        if n.x >= 0.0 {
            Direction::East
        } else {
            Direction::West
        }
    } else if ay >= az {
        if n.y >= 0.0 {
            Direction::Up
        } else {
            Direction::Down
        }
    } else if n.z >= 0.0 {
        Direction::South
    } else {
        Direction::North
    }
}

fn load_destroy_atlas(
    device: &vk::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    allocator: &Arc<Mutex<Allocator>>,
    jar_assets_dir: &Path,
    asset_index: &Option<AssetIndex>,
) -> (vk::Image, vk::ImageView, Allocation) {
    let mut atlas_pixels = Vec::new();
    let mut tile_size = 16u32;

    for stage in 0..STAGE_COUNT {
        let key = format!("minecraft/textures/block/destroy_stage_{stage}.png");
        let path = resolve_asset_path(jar_assets_dir, asset_index, &key);
        if let Some((pixels, w, _h)) = util::load_png(&path) {
            tile_size = w;
            atlas_pixels.extend_from_slice(&pixels);
        } else {
            atlas_pixels.extend(std::iter::repeat_n(
                0u8,
                (tile_size * tile_size * 4) as usize,
            ));
        }
    }

    let atlas_w = tile_size;
    let atlas_h = tile_size * STAGE_COUNT;

    let (image, view, allocation) =
        util::create_gpu_image(device, allocator, atlas_w, atlas_h, "destroy_atlas");
    let (staging_buf, staging_alloc) =
        util::create_staging_buffer(device, allocator, &atlas_pixels, "destroy_atlas_staging");

    util::upload_image(
        device,
        queue,
        command_pool,
        staging_buf,
        image,
        atlas_w,
        atlas_h,
    );

    device.destroy_buffer(staging_buf, None);
    let _ = allocator.lock().unwrap().free(staging_alloc);

    tracing::info!("Block overlay: loaded {STAGE_COUNT} destroy stages ({atlas_w}x{atlas_h})");

    (image, view, allocation)
}

fn create_pipeline(
    device: &vk::Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> vk::Pipeline {
    let vert_spv = shader::include_spirv!("block_overlay.vert.spv");
    let frag_spv = shader::include_spirv!("block_overlay.frag.spv");

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
        stride: size_of::<OverlayVertex>() as u32,
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
            format: vk::Format::R32G32Sfloat,
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
        // No inflation, so this camera-ward offset is what wins the depth test.
        // Mirror vanilla's constant-heavy crumbling offset (units 10, slope 1):
        // a large slope pulls edge-on side faces in front of occluding neighbors.
        depth_bias_enable: vk::TRUE,
        depth_bias_constant_factor: -10.0,
        depth_bias_slope_factor: -1.0,
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
        src_color_blend_factor: vk::BlendFactor::DstColor,
        dst_color_blend_factor: vk::BlendFactor::SrcColor,
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

    let mut pipeline = vk::Pipeline::null();
    device
        .create_graphics_pipelines(
            vk::PipelineCache::null(),
            &pipeline_info,
            None,
            slice::from_mut(&mut pipeline),
        )
        .expect("failed to create block overlay pipeline");

    device.destroy_shader_module(vert_module, None);
    device.destroy_shader_module(frag_module, None);

    pipeline
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each axis-aligned cube face must project to exactly one crack tile: its
    /// four corners span 1.0 in both texture axes (no axis collapse, scale 1).
    #[test]
    fn projection_maps_each_face_to_a_unit_tile() {
        let span = |xs: [f32; 4]| {
            let min = xs.iter().copied().fold(f32::INFINITY, f32::min);
            let max = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            max - min
        };
        for dir in CUBE_FACE_DIRS {
            let (positions, _, _) = cube_face_geometry(dir);
            let uv: [[f32; 2]; 4] =
                std::array::from_fn(|i| project_crack_uv(Vec3::from_array(positions[i]), dir));
            let us = std::array::from_fn(|i| uv[i][0]);
            let vs = std::array::from_fn(|i| uv[i][1]);
            assert!((span(us) - 1.0).abs() < 1e-4, "{dir:?} u span {}", span(us));
            assert!((span(vs) - 1.0).abs() < 1e-4, "{dir:?} v span {}", span(vs));
        }
    }
}
