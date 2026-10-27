use crate::renderer::camera::CameraUniform;
use crate::renderer::chunk::atlas::TextureAtlas;
use crate::renderer::chunk::buffer::CHUNK_VERTEX_BUFFERS;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};

/// Set 1: the atlas, its sampler and the sprite-rectangle buffer the fragment
/// shaders resolve sprite UVs with.
const ATLAS_LAYOUT: &[rhi::BindGroupLayoutEntry] = &[
    util::FRAGMENT_TEXTURE[0],
    util::FRAGMENT_TEXTURE[1],
    rhi::BindGroupLayoutEntry::storage(rhi::ShaderStages::FRAGMENT),
];

pub struct ChunkPipeline {
    /// Opaque terrain: no discard, early-Z. Drawn first (front-to-back).
    pub pipeline_solid: rhi::backend::RenderPipeline,
    /// Cutout terrain: alpha-test discard. Drawn after solid.
    pub pipeline_cutout: rhi::backend::RenderPipeline,
    /// Translucent water variant: alpha blending on, depth write off. Shares
    /// the terrain pipelines' layout and bind groups.
    pub water_pipeline: rhi::backend::RenderPipeline,
    pub pipeline_layout: rhi::backend::PipelineLayout,
    pub camera_layout: rhi::backend::BindGroupLayout,
    pub atlas_layout: rhi::backend::BindGroupLayout,
    pub camera_sets: Vec<rhi::backend::BindGroup>,
    pub atlas_set: rhi::backend::BindGroup,
    /// Set 2, which [`Lightmap`] owns.
    lightmap_set: rhi::backend::BindGroup,
    camera_buffers: Vec<rhi::backend::Buffer>,
}

impl ChunkPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        atlas: &TextureAtlas,
        lightmap: &Lightmap,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let atlas_layout = rhi::backend::create_bind_group_layout(device, ATLAS_LAYOUT);
        // Water pushes its section origin + fade per draw (water.vert); the
        // indirect passes read them per-instance and ignore the range.
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, atlas_layout, lightmap.layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 16,
            }),
        );

        let [pipeline_solid, pipeline_cutout, water_pipeline] =
            create_pipelines(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "camera_uniform",
        );

        let mut pipeline = Self {
            pipeline_solid,
            pipeline_cutout,
            water_pipeline,
            pipeline_layout,
            camera_layout,
            atlas_layout,
            camera_sets,
            atlas_set: rhi::backend::BindGroup::default(),
            lightmap_set: lightmap.group,
            camera_buffers,
        };
        pipeline.rebind_atlas(device, atlas);
        pipeline
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        let bytes = bytemuck::bytes_of(uniform);
        self.camera_buffers[frame].write(bytes);
    }

    pub fn rebind_atlas(&mut self, device: &rhi::backend::Device, atlas: &TextureAtlas) {
        let [texture, sampler] = util::texture_binding(atlas.texture.handle(), atlas.sampler);
        self.atlas_set.replace(
            device,
            self.atlas_layout,
            &[
                texture,
                sampler,
                (
                    2,
                    rhi::backend::BindingResource::storage(
                        &atlas.sprite_rects,
                        atlas.sprite_rects_bytes(),
                    ),
                ),
            ],
        );
    }

    pub fn bind(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize, cutout: bool) {
        let pipeline = if cutout {
            self.pipeline_cutout
        } else {
            self.pipeline_solid
        };
        self.bind_with(pass, pipeline, frame);
    }

    /// Bind the translucent water pipeline (same bind groups as `bind`).
    pub fn bind_water(&self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        self.bind_with(pass, self.water_pipeline, frame);
    }

    fn bind_with(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        pipeline: rhi::backend::RenderPipeline,
        frame: usize,
    ) {
        pass.set_pipeline(&pipeline);
        pass.set_bind_groups(
            0,
            &[self.camera_sets[frame], self.atlas_set, self.lightmap_set],
        );
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);

        self.pipeline_solid.destroy(device);
        self.pipeline_cutout.destroy(device);
        self.water_pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.atlas_set.destroy(device);
        self.camera_layout.destroy(device);
        self.atlas_layout.destroy(device);
    }
}

/// Builds the chunk pipelines: `solid` (chunk_solid.frag, no discard,
/// early-Z), `cutout` (chunk.frag, alpha-test discard) and translucent
/// `water`. Water reads only the packed vertices; the terrain pipelines also
/// read the per-instance meta.
fn create_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> [rhi::backend::RenderPipeline; 3] {
    let terrain = |label, fragment_shader| rhi::RenderPipelineDesc {
        cull_mode: rhi::CullMode::Back,
        depth: Some(rhi::DepthState::WRITE),
        ..rhi::RenderPipelineDesc::new(
            label,
            shader::include_shader!("chunk.vert"),
            fragment_shader,
            &CHUNK_VERTEX_BUFFERS,
        )
    };
    let water = rhi::RenderPipelineDesc {
        vertex_shader: shader::include_shader!("water.vert"),
        vertex_buffers: &CHUNK_VERTEX_BUFFERS[..1],
        depth: Some(rhi::DepthState::READ_ONLY),
        blend: Some(rhi::BlendState::TRANSLUCENT),
        ..terrain("water", shader::include_shader!("water.frag"))
    };
    [
        terrain("solid chunk", shader::include_shader!("chunk_solid.frag")),
        terrain("cutout chunk", shader::include_shader!("chunk.frag")),
        water,
    ]
    .map(|desc| rhi::backend::create_render_pipeline(device, targets, layout, &desc))
}
