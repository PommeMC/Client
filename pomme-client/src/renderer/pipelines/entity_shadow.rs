//! Entity shadows: vanilla `EntityRenderer.extractShadow` and
//! `ShadowFeatureRenderer` (26.2), one quad per lit, full block under an
//! entity.

use glam::{DVec3, Vec3};

use crate::physics::block_shape::{LocalBox, is_collision_shape_full_block, outline_shape};
use crate::renderer::camera::CameraUniform;
use crate::renderer::{rhi, shader, util};
use crate::resources::{ResourceId, ResourceManager, resource_id};
use crate::world::block::{BlockStateData, BlockStateTable};
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
    pub block_state_table: BlockStateTable,
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
const SHADOW_TEXTURE: ResourceId = resource_id!("minecraft:textures/misc/shadow.png");

pub struct EntityShadowPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    texture_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    texture_set: rhi::backend::BindGroup,
    camera_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    texture: rhi::backend::Texture,
    texture_sampler: rhi::backend::Sampler,
    /// This frame's vertices, kept between frames for its capacity.
    verts: Vec<ShadowVertex>,
}

impl EntityShadowPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        resources: &ResourceManager,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let texture_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout =
            rhi::backend::create_pipeline_layout(device, &[camera_layout, texture_layout], None);

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "entity_shadow_camera",
        );

        let texture = load_shadow_texture(device, resources);

        let texture_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR);

        let texture_set =
            util::create_texture_group(device, texture_layout, texture.handle(), texture_sampler);

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_SHADOW_VERTS * size_of::<ShadowVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "entity_shadow_vertices",
        );

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            texture_layout,
            camera_sets,
            texture_set,
            camera_buffers,
            vertex_buffers,
            texture,
            texture_sampler,
            verts: Vec::with_capacity(MAX_SHADOW_VERTS),
        }
    }

    /// The caller must idle the device before replacing the shared texture set.
    pub fn reload_texture(&mut self, device: &rhi::backend::Device, resources: &ResourceManager) {
        let texture = load_shadow_texture(device, resources);
        self.texture_set.replace(
            device,
            self.texture_layout,
            &util::texture_binding(texture.handle(), self.texture_sampler),
        );
        std::mem::replace(&mut self.texture, texture).destroy(device);
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout);
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
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
        self.vertex_buffers[frame].write(bytes);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_groups(0, &[self.camera_sets[frame], self.texture_set]);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.draw(self.verts.len() as u32, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);
        std::mem::take(&mut self.texture).destroy(device);

        self.texture_sampler.destroy(device);
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.texture_set.destroy(device);
        self.camera_layout.destroy(device);
        self.texture_layout.destroy(device);
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
    let below = scene
        .block_state_table
        .block_state_data(scene.chunks.get_block_state(x, y - 1, z));
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
fn takes_a_shadow(block_state_data: &BlockStateData) -> bool {
    !is_invisible_block(block_state_data.id()) && is_collision_shape_full_block(block_state_data)
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
    device: &rhi::backend::Device,
    resources: &ResourceManager,
) -> rhi::backend::Texture {
    let image =
        util::load_resource_image(resources, &SHADOW_TEXTURE, util::MAX_RESOURCE_IMAGE_BYTES);
    let (pixels, w, h) = match image {
        Ok(image) => {
            let (w, h) = image.dimensions();
            (image.into_raw(), w, h)
        }
        Err(error) => {
            tracing::warn!(
                "Failed to load {SHADOW_TEXTURE}, using circular-gradient fallback: {error}"
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
        }
    };

    util::create_sampled_texture(device, "entity_shadow_texture", &pixels, w, h)
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
            depth: Some(rhi::DepthState::READ_ONLY),
            blend: Some(rhi::BlendState::OVER_REPLACE_ALPHA),
            ..rhi::RenderPipelineDesc::new(
                "entity shadow",
                shader::include_shader!("entity_shadow.vert"),
                shader::include_shader!("entity_shadow.frag"),
                &[rhi::vertex_buffer!(
                    ShadowVertex,
                    [Float32x3, Float32x4, Float32x2]
                )],
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadows_land_only_on_full_visible_blocks() {
        let blocks = BlockStateTable::native();
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
                takes_a_shadow(blocks.block_state_data(blocks.find_state(name, props))),
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
