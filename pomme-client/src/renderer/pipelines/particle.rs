use std::ops::Range;

use glam::Vec3;

use crate::particles::render::{ParticleLayer, ParticlesRenderState};
use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};

/// The legacy `ParticleStore`'s admission cap.
pub const MAX_PARTICLE_QUADS: usize = 16384;
const INITIAL_PARTICLE_QUADS: usize = 16384;
const QUAD_INDICES: &[usize] = &[0, 1, 2, 0, 2, 3];
const VERTICES_PER_QUAD: usize = QUAD_INDICES.len();
const INITIAL_VERTEX_BYTES: usize =
    INITIAL_PARTICLE_QUADS * VERTICES_PER_QUAD * std::mem::size_of::<ParticleVertex>();

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParticleAtlas {
    Combined,
    Particles,
}

const BATCHES: &[(ParticleAtlas, bool)] = &[
    (ParticleAtlas::Combined, false),
    (ParticleAtlas::Particles, false),
    (ParticleAtlas::Combined, true),
    (ParticleAtlas::Particles, true),
];

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
    /// Packed RGBA8, the particle's own colour.
    pub color: u32,
    /// The light lane (`LightLevels::lane`).
    pub light: f32,
    /// Vanilla `SingleQuadParticle.Layer`: false = opaque/cutout terrain
    /// layer, true = alpha-blended translucent layer.
    pub translucent: bool,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: u32,
    light: f32,
}

impl ParticleVertex {
    pub const BUFFER: rhi::VertexBufferLayout<'static> =
        rhi::vertex_buffer!(Self, [Float32x3, Float32x2, Unorm8x4, Float32]);
}

pub struct ParticlePipeline {
    pipeline: rhi::backend::RenderPipeline,
    translucent_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    atlas_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    atlas_set: rhi::backend::BindGroup,
    particle_atlas_set: rhi::backend::BindGroup,
    atlas_sampler: rhi::backend::Sampler,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffer_capacities: Vec<usize>,
    /// This frame's vertex ranges in `BATCHES` order.
    batches: [Range<u32>; BATCHES.len()],
}

impl ParticlePipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
        particle_atlas: &TextureAtlas,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let atlas_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, atlas_layout, lightmap.layout],
            None,
        );

        let pipeline = create_pipeline(device, targets, pipeline_layout, false);
        let translucent_pipeline = create_pipeline(device, targets, pipeline_layout, true);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "particle_camera",
        );
        // Both particle atlas bindings sample only LOD 0.
        let atlas_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            INITIAL_VERTEX_BYTES as u64,
            rhi::BufferUsages::VERTEX,
            "particle_vertices",
        );

        let mut this = Self {
            pipeline,
            translucent_pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            camera_sets,
            atlas_set: rhi::backend::BindGroup::default(),
            particle_atlas_set: rhi::backend::BindGroup::default(),
            atlas_sampler,
            camera_buffers,
            lightmap_set: lightmap.group,
            vertex_buffer_capacities: vec![INITIAL_VERTEX_BYTES; vertex_buffers.len()],
            vertex_buffers,
            batches: std::array::from_fn(|_| 0..0),
        };
        this.rebind_atlas(device, atlas);
        this.rebind_particle_atlas(device, particle_atlas);
        this
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.atlas_set.replace(
            device,
            self.atlas_layout,
            &util::texture_binding(atlas.texture.handle(), self.atlas_sampler),
        );
    }

    pub fn rebind_particle_atlas(
        &mut self,
        device: &rhi::backend::Device,
        particle_atlas: &TextureAtlas,
    ) {
        self.particle_atlas_set.replace(
            device,
            self.atlas_layout,
            &util::texture_binding(particle_atlas.texture.handle(), self.atlas_sampler),
        );
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    /// Expands legacy `quads` with `camera`'s axes and `engine_particles` with
    /// their own rotations. `eye_to_anchor` rebases engine centers for upload.
    /// `frame`'s previous GPU use must have completed; its buffer may be
    /// replaced.
    pub fn prepare(
        &mut self,
        device: &rhi::backend::Device,
        frame: usize,
        camera: &Camera,
        quads: &[ParticleQuad],
        engine_particles: &ParticlesRenderState,
        eye_to_anchor: Vec3,
    ) -> Result<(), String> {
        self.batches = std::array::from_fn(|_| 0..0);
        if quads.is_empty() && engine_particles.quads.is_empty() {
            return Ok(());
        }

        let quad_count = quads
            .len()
            .checked_add(engine_particles.quads.len())
            .ok_or_else(|| "particle quad count overflows usize".to_owned())?;
        let required_vertices = quad_count
            .checked_mul(VERTICES_PER_QUAD)
            .ok_or_else(|| "particle vertex count overflows usize".to_owned())?;
        u32::try_from(required_vertices)
            .map_err(|_| "particle vertex range exceeds u32".to_owned())?;
        let required_bytes = required_vertices
            .checked_mul(std::mem::size_of::<ParticleVertex>())
            .ok_or_else(|| "particle vertex byte size overflows usize".to_owned())?;
        isize::try_from(required_bytes)
            .map_err(|_| "particle vertex byte size exceeds isize".to_owned())?;
        u64::try_from(required_bytes)
            .map_err(|_| "particle vertex byte size exceeds u64".to_owned())?;
        let growth = if required_bytes > self.vertex_buffer_capacities[frame] {
            let capacity = required_bytes
                .checked_next_power_of_two()
                .ok_or_else(|| "particle vertex buffer capacity overflows usize".to_owned())?;
            isize::try_from(capacity)
                .map_err(|_| "particle vertex buffer capacity exceeds isize".to_owned())?;
            let capacity_u64 = u64::try_from(capacity)
                .map_err(|_| "particle vertex buffer capacity exceeds u64".to_owned())?;
            Some((capacity, capacity_u64))
        } else {
            None
        };

        let mut verts: Vec<ParticleVertex> = Vec::new();
        verts
            .try_reserve_exact(required_vertices)
            .map_err(|error| format!("failed to reserve particle vertices: {error}"))?;
        let (right, up) = camera.billboard_axes();
        let emit = |verts: &mut Vec<ParticleVertex>, translucent: bool| {
            for quad in quads.iter().filter(|q| q.translucent == translucent) {
                let center = Vec3::from(quad.pos);
                let corner = |nx: f32, ny: f32, u: f32, v: f32| ParticleVertex {
                    position: (center + (right * nx + up * ny) * quad.size).into(),
                    uv: [u, v],
                    color: quad.color,
                    light: quad.light,
                };
                // Vanilla QuadParticleRenderState corner order and UV mapping.
                let corners = [
                    corner(1.0, -1.0, quad.u1, quad.v1),
                    corner(1.0, 1.0, quad.u1, quad.v0),
                    corner(-1.0, 1.0, quad.u0, quad.v0),
                    corner(-1.0, -1.0, quad.u0, quad.v1),
                ];
                for &i in QUAD_INDICES {
                    verts.push(corners[i]);
                }
            }
        };
        let mut batches = std::array::from_fn(|_| 0..0);
        let mut start = 0;
        for (range, &(atlas, translucent)) in batches.iter_mut().zip(BATCHES) {
            if atlas == ParticleAtlas::Combined {
                emit(&mut verts, translucent);
            }
            for quad in engine_particles.quads.iter().filter(|quad| {
                let batch = match quad.layer {
                    ParticleLayer::Opaque => (ParticleAtlas::Particles, false),
                    ParticleLayer::Translucent => (ParticleAtlas::Particles, true),
                    ParticleLayer::BlockOpaque | ParticleLayer::ItemOpaque => {
                        (ParticleAtlas::Combined, false)
                    }
                    ParticleLayer::BlockTranslucent | ParticleLayer::ItemTranslucent => {
                        (ParticleAtlas::Combined, true)
                    }
                };
                batch == (atlas, translucent)
            }) {
                let center = quad.position + eye_to_anchor;
                let color = u32::from_le_bytes(
                    quad.color
                        .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
                );
                let light = f32::from(quad.light.block) + f32::from(quad.light.sky) * 256.0;
                let corner = |nx: f32, ny: f32, u: f32, v: f32| ParticleVertex {
                    position: (center + quad.rotation * Vec3::new(nx, ny, 0.0) * quad.size).into(),
                    uv: [u, v],
                    color,
                    light,
                };
                let [u0, u1, v0, v1] = quad.uv;
                let corners = [
                    corner(1.0, -1.0, u1, v1),
                    corner(1.0, 1.0, u1, v0),
                    corner(-1.0, 1.0, u0, v0),
                    corner(-1.0, -1.0, u0, v1),
                ];
                for &i in QUAD_INDICES {
                    verts.push(corners[i]);
                }
            }
            let end = u32::try_from(verts.len())
                .map_err(|_| "particle batch vertex range exceeds u32".to_owned())?;
            *range = start..end;
            start = end;
        }
        if let Some((capacity, capacity_u64)) = growth {
            let buffer = rhi::backend::Buffer::try_new(
                device,
                "particle_vertices",
                capacity_u64,
                rhi::BufferUsages::VERTEX,
                rhi::MemoryLocation::Host,
            )?;
            let old = std::mem::replace(&mut self.vertex_buffers[frame], buffer);
            old.destroy(device);
            self.vertex_buffer_capacities[frame] = capacity;
        }

        let bytes = bytemuck::cast_slice::<ParticleVertex, u8>(&verts);
        self.vertex_buffers[frame].write(bytes);
        self.batches = batches;
        Ok(())
    }

    /// Draws [`Self::prepare`]'s billboards.
    pub fn draw(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        for (&(atlas, translucent), range) in BATCHES.iter().zip(&self.batches) {
            if range.is_empty() {
                continue;
            }
            let pipeline = if translucent {
                self.translucent_pipeline
            } else {
                self.pipeline
            };
            let atlas_set = match atlas {
                ParticleAtlas::Combined => self.atlas_set,
                ParticleAtlas::Particles => self.particle_atlas_set,
            };
            pass.set_pipeline(&pipeline);
            pass.set_bind_groups(0, &[self.camera_sets[frame], atlas_set, self.lightmap_set]);
            pass.draw(range.end - range.start, 1, range.start, 0);
        }
    }

    /// Whether [`Self::prepare`] built any billboards of the layer.
    pub fn has_layer(&self, translucent: bool) -> bool {
        BATCHES
            .iter()
            .zip(&self.batches)
            .any(|(&(_, opacity), range)| opacity == translucent && !range.is_empty())
    }

    /// Draws one layer through a shader pack's program, which the pass has
    /// bound at sets 0-2.
    pub fn draw_pack(
        &self,
        pass: &rhi::backend::RenderPassEncoder,
        frame: usize,
        translucent: bool,
    ) {
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        for (&(atlas, opacity), range) in BATCHES.iter().zip(&self.batches) {
            if opacity != translucent || range.is_empty() {
                continue;
            }
            let atlas_set = match atlas {
                ParticleAtlas::Combined => self.atlas_set,
                ParticleAtlas::Particles => self.particle_atlas_set,
            };
            pass.set_bind_group(3, atlas_set);
            pass.draw(range.end - range.start, 1, range.start, 0);
        }
    }

    pub fn recreate_pipeline(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        self.pipeline.destroy(device);
        self.translucent_pipeline.destroy(device);
        self.pipeline = create_pipeline(device, targets, self.pipeline_layout, false);
        self.translucent_pipeline = create_pipeline(device, targets, self.pipeline_layout, true);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);

        self.pipeline.destroy(device);
        self.translucent_pipeline.destroy(device);
        self.atlas_sampler.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.atlas_set.destroy(device);
        self.particle_atlas_set.destroy(device);
        self.camera_layout.destroy(device);
        self.atlas_layout.destroy(device);
    }
}

fn create_pipeline(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
    translucent: bool,
) -> rhi::backend::RenderPipeline {
    // Billboards always face the camera, so culling is a no-op; vanilla's
    // back-face cull is skipped rather than fighting winding conventions.
    rhi::backend::create_render_pipeline(
        device,
        targets,
        layout,
        &rhi::RenderPipelineDesc {
            // Vanilla PARTICLE_SNIPPET: depth test AND write for both layers.
            // OPAQUE_PARTICLE has no blending (alpha is handled by the
            // fragment discard); TRANSLUCENT_PARTICLE adds standard alpha
            // blending.
            depth: Some(rhi::DepthState {
                compare: rhi::CompareFunction::Less,
                ..rhi::DepthState::WRITE
            }),
            blend: translucent.then_some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                "particle",
                shader::include_shader!("particle.vert"),
                shader::include_shader!("particle.frag"),
                &[ParticleVertex::BUFFER],
            )
        },
    )
}
