use glam::Vec3;

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};

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
struct ParticleVertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: u32,
    light: f32,
}

pub struct ParticlePipeline {
    pipeline: rhi::backend::RenderPipeline,
    translucent_pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    atlas_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    atlas_set: rhi::backend::BindGroup,
    atlas_sampler: rhi::backend::Sampler,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// The shared [`Lightmap`]'s group, set 2.
    lightmap_set: rhi::backend::BindGroup,
    vertex_buffers: Vec<rhi::backend::Buffer>,
}

impl ParticlePipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
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
        // Vanilla's particles atlas has no mip chain. Pomme stores particle
        // sprites in the combined atlas image, so clamp this consumer to LOD 0.
        let atlas_sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::NEAREST);

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_VERTS * std::mem::size_of::<ParticleVertex>()) as u64,
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
            atlas_sampler,
            camera_buffers,
            lightmap_set: lightmap.group,
            vertex_buffers,
        };
        this.rebind_atlas(device, atlas);
        this
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        self.atlas_set.replace(
            device,
            self.atlas_layout,
            &util::texture_binding(atlas.texture.handle(), self.atlas_sampler),
        );
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn update_and_draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
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
        // Opaque quads first, then translucent, so each layer is one
        // contiguous draw range (vanilla renders the layers separately).
        let emit = |verts: &mut Vec<ParticleVertex>, translucent: bool| {
            for quad in quads.iter().filter(|q| q.translucent == translucent) {
                if verts.len() >= MAX_VERTS {
                    return;
                }
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
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(corners[i]);
                }
            }
        };
        emit(&mut verts, false);
        let opaque_verts = verts.len();
        emit(&mut verts, true);

        let bytes = bytemuck::cast_slice::<ParticleVertex, u8>(&verts);
        self.vertex_buffers[frame].write(bytes);

        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        for (pipeline, range) in [
            (self.pipeline, 0..opaque_verts),
            (self.translucent_pipeline, opaque_verts..verts.len()),
        ] {
            if range.is_empty() {
                continue;
            }
            pass.set_pipeline(&pipeline);
            pass.set_bind_groups(
                0,
                &[self.camera_sets[frame], self.atlas_set, self.lightmap_set],
            );
            pass.draw(range.len() as u32, 1, range.start as u32, 0);
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
                &[rhi::vertex_buffer!(
                    ParticleVertex,
                    [Float32x3, Float32x2, Unorm8x4, Float32]
                )],
            )
        },
    )
}
