use crate::renderer::camera::CameraUniform;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};

/// Distant terrain: Voxy quads read straight from their arena, textured
/// with their block's model, under the camera's fog and the lightmap;
/// opaque, then see-through blended over it.
pub struct DistantPipeline {
    pipelines: [rhi::backend::RenderPipeline; 2],
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    /// Set 2, which [`Lightmap`] owns.
    lightmap_set: rhi::backend::BindGroup,
}

impl DistantPipeline {
    /// `arena_layout` is set 1
    /// ([`crate::renderer::distant::gpu::DistantBuffers::layout`]),
    /// bound per arena as it draws.
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        lightmap: &Lightmap,
        arena_layout: rhi::backend::BindGroupLayout,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        // The view projection (once a pass), then the section's origin and
        // scale.
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, arena_layout, lightmap.layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: 80,
            }),
        );
        let pipelines = create_pipelines(device, targets, pipeline_layout);
        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "distant_camera_uniform",
        );
        Self {
            pipelines,
            pipeline_layout,
            camera_layout,
            camera_sets,
            camera_buffers,
            lightmap_set: lightmap.group,
        }
    }

    pub fn rebuild(
        &mut self,
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
    ) {
        for pipeline in &mut self.pipelines {
            pipeline.destroy(device);
        }
        self.pipelines = create_pipelines(device, targets, self.pipeline_layout);
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.camera_buffers[frame].write(bytemuck::bytes_of(uniform));
    }

    pub fn bind(
        &self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        translucent: bool,
    ) {
        pass.set_pipeline(&self.pipelines[usize::from(translucent)]);
        pass.set_bind_group(0, self.camera_sets[frame]);
        pass.set_bind_group(2, self.lightmap_set);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        for pipeline in &mut self.pipelines {
            pipeline.destroy(device);
        }
        self.pipeline_layout.destroy(device);
        self.camera_layout.destroy(device);
    }
}

/// The opaque pipeline, then the see-through one, which blends as Voxy's
/// translucent pass does. Neither culls faces: the quads are drawn from
/// every side.
fn create_pipelines(
    device: &rhi::backend::Device,
    targets: rhi::backend::PipelineTargets,
    layout: rhi::backend::PipelineLayout,
) -> [rhi::backend::RenderPipeline; 2] {
    let opaque = rhi::RenderPipelineDesc {
        depth: Some(rhi::DepthState::WRITE),
        ..rhi::RenderPipelineDesc::new(
            "distant terrain",
            shader::include_shader!("distant.vert"),
            shader::include_shader!("distant.frag"),
            &[],
        )
    };
    let translucent = rhi::RenderPipelineDesc {
        label: "distant translucent",
        blend: Some(rhi::BlendState::TRANSLUCENT),
        ..opaque
    };
    [opaque, translucent]
        .map(|desc| rhi::backend::create_render_pipeline(device, targets, layout, &desc))
}
