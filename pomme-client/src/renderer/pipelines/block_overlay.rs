use std::f32::consts::{FRAC_PI_2, PI};

use azalea_block::BlockState;
use glam::{Quat, Vec3};

use crate::position::BlockPos;
use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::mesher::{CUBE_FACE_DIRS, cube_face_geometry};
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};
use crate::resources::{ResourceError, ResourceId, ResourceManager};
use crate::world::block::BlockStateData;
use crate::world::block::model::{BakedQuad, Direction, direction_from_positions};
use crate::world::block::render_assets::BlockRenderView;

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

/// Maps the wrapped crack UV into one stage's row of the vertical atlas.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct OverlayPush {
    v_base: f32,
    v_scale: f32,
}

pub struct BlockOverlayPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    texture_set: rhi::backend::BindGroup,
    camera_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    atlas: rhi::backend::Texture,
    atlas_sampler: rhi::backend::Sampler,
}

impl BlockOverlayPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        resources: &ResourceManager,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, texture_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::FRAGMENT,
                size: size_of::<OverlayPush>() as u32,
            }),
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "block_overlay_camera",
        );

        let atlas = load_destroy_atlas(device, resources);

        let atlas_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let texture_set =
            util::create_texture_group(device, texture_layout, atlas.handle(), atlas_sampler);

        let placeholder = vec![
            OverlayVertex {
                position: [0.0; 3],
                uv: [0.0; 2],
            };
            MAX_OVERLAY_VERTS
        ];
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&placeholder);
        let vertex_buffers = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                rhi::backend::Buffer::with_data(
                    device,
                    "block_overlay_vertices",
                    bytes,
                    rhi::BufferUsages::VERTEX,
                )
            })
            .collect();

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            camera_sets,
            texture_set,
            camera_buffers,
            vertex_buffers,
            atlas,
            atlas_sampler,
        }
    }

    /// The caller must idle the device before replacing the shared texture set.
    /// Geometry, stage UVs and the nearest sampler are retained.
    pub fn reload_texture(&mut self, device: &rhi::backend::Device, resources: &ResourceManager) {
        let atlas = load_destroy_atlas(device, resources);
        self.texture_set.replace(
            device,
            self.texture_layout,
            &util::texture_binding(atlas.handle(), self.atlas_sampler),
        );
        std::mem::replace(&mut self.atlas, atlas).destroy(device);
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        registry: BlockRenderView<'_>,
        state: BlockState,
        block_pos: &BlockPos,
        anchor: glam::DVec3,
        stage: u32,
    ) {
        let vertices = build_overlay_vertices(registry, state, block_pos, anchor);
        if vertices.is_empty() {
            return;
        }
        // TODO: one overlay per frame; multiple draws would need a write cursor.
        let bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&vertices);
        self.vertex_buffers[frame].write(bytes);

        pass.set_pipeline(&self.pipeline);
        let push = OverlayPush {
            v_base: stage as f32 / STAGE_COUNT as f32,
            v_scale: 1.0 / STAGE_COUNT as f32,
        };
        pass.set_push_constants(rhi::ShaderStages::FRAGMENT, 0, bytemuck::bytes_of(&push));
        pass.set_bind_groups(0, &[self.camera_sets[frame], self.texture_set]);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.draw(vertices.len() as u32, 1, 0, 0);
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);

        self.atlas_sampler.destroy(device);
        std::mem::take(&mut self.atlas).destroy(device);

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.texture_set.destroy(device);
        self.camera_layout.destroy(device);
        self.texture_layout.destroy(device);
    }
}

/// Anchor-relative (see Camera::anchor) model origin, including the block's
/// position offset like `LevelRenderer`'s breaking pass. Crack UVs project from
/// block-local coordinates, so neither shift touches the texture.
fn overlay_origin(
    block_state_data: &BlockStateData,
    pos: &BlockPos,
    anchor: glam::DVec3,
) -> [f32; 3] {
    let model_offset = block_state_data.block_position_offset(pos.x, pos.z);
    (glam::DVec3::new(pos.x as f64, pos.y as f64, pos.z as f64) + model_offset - anchor)
        .as_vec3()
        .to_array()
}

/// Builds the crumbling overlay for a block by re-tessellating its model with
/// vanilla's per-face crack projection (`SheetedDecalTextureGenerator`). There
/// is no geometry inflation: the overlay sits exactly on the block and relies
/// on the pipeline's polygon offset to avoid z-fighting, matching vanilla.
fn build_overlay_vertices(
    registry: BlockRenderView<'_>,
    state: BlockState,
    pos: &BlockPos,
    anchor: glam::DVec3,
) -> Vec<OverlayVertex> {
    let origin = overlay_origin(
        registry.block_state_table().block_state_data(state),
        pos,
        anchor,
    );
    let mut verts = Vec::new();

    if let Some(model) = registry.get_baked_model(state) {
        for quad in &model.quads {
            push_quad(&mut verts, origin, quad);
        }
    } else if let Some(quads) = registry.get_multipart_quads(state) {
        for quad in quads {
            push_quad(&mut verts, origin, quad);
        }
    } else if registry.get_textures(state).is_some() {
        // Blocks rendered as a plain opaque cube (no baked model): crack a unit cube.
        for dir in CUBE_FACE_DIRS {
            let (positions, _) = cube_face_geometry(dir);
            push_face(&mut verts, origin, &positions, dir);
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

fn push_quad(verts: &mut Vec<OverlayVertex>, origin: [f32; 3], quad: &BakedQuad) {
    // Vanilla `FaceBakery` stores the nearest cardinal for a quad without a
    // cull face, `UP` when it is degenerate.
    let dir = quad
        .cullface
        .or_else(|| direction_from_positions(&quad.positions))
        .unwrap_or(Direction::Up);
    push_face(verts, origin, &quad.positions, dir);
}

/// Emits one quad (two triangles, six vertices) with crack UVs projected from
/// the block-local vertex positions for `dir`, anchoring the crack pattern to
/// the block corner like vanilla's inverted block pose. The raw (possibly
/// negative) UVs are wrapped into the stage's atlas row by the fragment
/// shader, standing in for vanilla's REPEAT sampler.
fn push_face(
    verts: &mut Vec<OverlayVertex>,
    origin: [f32; 3],
    positions: &[[f32; 3]; 4],
    dir: Direction,
) {
    let local: [Vec3; 4] = std::array::from_fn(|i| Vec3::from_array(positions[i]));
    let world: [Vec3; 4] = std::array::from_fn(|i| Vec3::from_array(origin) + local[i]);
    let uv: [[f32; 2]; 4] = std::array::from_fn(|i| project_crack_uv(local[i], dir));

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

fn load_destroy_atlas(
    device: &rhi::backend::Device,
    resources: &ResourceManager,
) -> rhi::backend::Texture {
    let mut atlas_pixels = Vec::new();
    let mut tile_size = 16u32;

    for stage in 0..STAGE_COUNT {
        let key = format!("minecraft:textures/block/destroy_stage_{stage}.png");
        let image = ResourceId::new(&key)
            .and_then(|id| {
                resources
                    .get_resource(&id)?
                    .ok_or(ResourceError::Missing(id))?
                    .content()
                    .read_bytes(util::MAX_RESOURCE_IMAGE_BYTES)
            })
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                // Preserve legacy PNG conversion without a new decoded-size cap.
                util::load_png_reader(bytes.as_slice(), usize::MAX)
                    .map_err(|error| error.to_string())
            });
        match image {
            Ok((pixels, w, _h)) => {
                tile_size = w;
                atlas_pixels.extend_from_slice(&pixels);
            }
            Err(error) => {
                tracing::warn!("Failed to load {key}, using transparent stage fallback: {error}");
                atlas_pixels.extend(std::iter::repeat_n(
                    0u8,
                    (tile_size * tile_size * 4) as usize,
                ));
            }
        }
    }

    // Assumes uniformly sized square stages. Mixed sizes or missing stages before
    // a size change can leave fewer pixels than the upload dimensions require;
    // retained for parity pending broader asset validation.
    let atlas_w = tile_size;
    let atlas_h = tile_size * STAGE_COUNT;
    let texture =
        util::create_sampled_texture(device, "destroy_atlas", &atlas_pixels, atlas_w, atlas_h);
    tracing::info!("Block overlay: loaded {STAGE_COUNT} destroy stages ({atlas_w}x{atlas_h})");
    texture
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> rhi::backend::RenderPipeline {
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            // No inflation, so this camera-ward offset is what wins the depth
            // test. Mirror vanilla's constant-heavy crumbling offset (units 10,
            // slope 1): a large slope pulls edge-on side faces in front of
            // occluding neighbors.
            depth: Some(rhi::DepthState {
                bias: Some(rhi::DepthBias {
                    constant: -10,
                    slope: -1.0,
                }),
                ..rhi::DepthState::READ_ONLY
            }),
            blend: Some(rhi::BlendState {
                color: rhi::BlendComponent {
                    src: rhi::BlendFactor::DstColor,
                    dst: rhi::BlendFactor::SrcColor,
                },
                alpha: rhi::BlendComponent::REPLACE,
            }),
            ..rhi::RenderPipelineDesc::new(
                "block overlay",
                shader::include_shader!("block_overlay.vert"),
                shader::include_shader!("block_overlay.frag"),
                &[rhi::vertex_buffer!(OverlayVertex, [Float32x3, Float32x2])],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn randomized_model_offset_moves_destroy_overlay_with_bamboo() {
        let block_state_table = crate::world::block::BlockStateTable::native();
        let bamboo = block_state_table.find_state("bamboo", &[]);
        let block_state_data = block_state_table.block_state_data(bamboo);
        let pos = BlockPos::new(5, 64, -7);
        let origin = overlay_origin(block_state_data, &pos, glam::DVec3::ZERO);
        let offset = block_state_data.block_position_offset(pos.x, pos.z);

        assert_ne!(offset, glam::DVec3::ZERO);
        let block = glam::DVec3::new(pos.x as f64, pos.y as f64, pos.z as f64);
        assert_eq!(origin, (block + offset).as_vec3().to_array());
    }

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
            let (positions, _) = cube_face_geometry(dir);
            let uv: [[f32; 2]; 4] =
                std::array::from_fn(|i| project_crack_uv(Vec3::from_array(positions[i]), dir));
            let us = std::array::from_fn(|i| uv[i][0]);
            let vs = std::array::from_fn(|i| uv[i][1]);
            assert!((span(us) - 1.0).abs() < 1e-4, "{dir:?} u span {}", span(us));
            assert!((span(vs) - 1.0).abs() < 1e-4, "{dir:?} v span {}", span(vs));
        }
    }

    /// Partial faces sample their own block-local sub-region of the crack
    /// tile, regardless of where the block sits in the world.
    #[test]
    fn partial_face_keeps_block_local_uvs() {
        let positions = [
            [6.0 / 16.0, 1.0, 6.0 / 16.0],
            [6.0 / 16.0, 1.0, 10.0 / 16.0],
            [10.0 / 16.0, 1.0, 10.0 / 16.0],
            [10.0 / 16.0, 1.0, 6.0 / 16.0],
        ];
        let mut near = Vec::new();
        let mut far = Vec::new();
        push_face(&mut near, [0.0; 3], &positions, Direction::Up);
        push_face(&mut far, [123.0, -40.0, 987.0], &positions, Direction::Up);

        // Up faces project u = x, v = z.
        for vert in &near {
            assert!((vert.uv[0] - vert.position[0]).abs() < 1e-4, "u != x");
            assert!((vert.uv[1] - vert.position[2]).abs() < 1e-4, "v != z");
        }
        for (a, b) in near.iter().zip(&far) {
            assert_eq!(a.uv, b.uv, "origin leaked into crack UVs");
        }
    }
}
