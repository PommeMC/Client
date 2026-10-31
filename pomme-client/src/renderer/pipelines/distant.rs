use crate::renderer::camera::CameraUniform;
use crate::renderer::distant::gpu::DistantBuffers;
use crate::renderer::pipelines::lightmap::Lightmap;
use crate::renderer::{rhi, shader, util};

/// Set 1: the drawn-column mask `distant.frag` discards with.
const MASK_LAYOUT: &[rhi::BindGroupLayoutEntry] = &[rhi::BindGroupLayoutEntry::storage(
    rhi::ShaderStages::FRAGMENT,
)];

/// [`crate::renderer::distant::mesh::DistantVertex`]: packed x/z, y, light
/// and face, colour.
const VERTEX_BUFFERS: [rhi::VertexBufferLayout<'static>; 1] = [rhi::VertexBufferLayout {
    stride: 12,
    step_mode: rhi::VertexStepMode::Vertex,
    attributes: &[
        rhi::VertexAttribute {
            location: 0,
            format: rhi::VertexFormat::Uint32,
            offset: 0,
        },
        rhi::VertexAttribute {
            location: 1,
            format: rhi::VertexFormat::Uint16,
            offset: 4,
        },
        rhi::VertexAttribute {
            location: 2,
            format: rhi::VertexFormat::Uint8x2,
            offset: 6,
        },
        rhi::VertexAttribute {
            location: 3,
            format: rhi::VertexFormat::Unorm8x4,
            offset: 8,
        },
    ],
}];

/// Distant terrain: coloured boxes behind the real chunks, under the
/// camera's fog and the lightmap; opaque, then see-through blended over it.
pub struct DistantPipeline {
    pipelines: [rhi::backend::RenderPipeline; 2],
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    mask_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    mask_set: rhi::backend::BindGroup,
    /// Set 2, which [`Lightmap`] owns.
    lightmap_set: rhi::backend::BindGroup,
}

impl DistantPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        lightmap: &Lightmap,
        buffers: &DistantBuffers,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let mask_layout = rhi::backend::create_bind_group_layout(device, MASK_LAYOUT);
        // The view projection (once a frame), then the section origin.
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout, mask_layout, lightmap.layout],
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
        let mask_set = device.create_bind_group(
            mask_layout,
            &[(
                0,
                rhi::backend::BindingResource::storage(
                    buffers.mask(),
                    DistantBuffers::mask_bytes(),
                ),
            )],
        );
        Self {
            pipelines,
            pipeline_layout,
            camera_layout,
            mask_layout,
            camera_sets,
            camera_buffers,
            mask_set,
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
        pass.set_bind_groups(
            0,
            &[self.camera_sets[frame], self.mask_set, self.lightmap_set],
        );
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.mask_set.destroy(device);
        for pipeline in &mut self.pipelines {
            pipeline.destroy(device);
        }
        self.pipeline_layout.destroy(device);
        self.camera_layout.destroy(device);
        self.mask_layout.destroy(device);
    }
}

/// The opaque pipeline, then the see-through one, which blends and leaves
/// depth alone like chunk water. Faces are unculled: the boxes are drawn
/// from every side as easily.
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
            &VERTEX_BUFFERS,
        )
    };
    let translucent = rhi::RenderPipelineDesc {
        label: "distant water",
        depth: Some(rhi::DepthState::READ_ONLY),
        blend: Some(rhi::BlendState::TRANSLUCENT),
        ..opaque
    };
    [opaque, translucent]
        .map(|desc| rhi::backend::create_render_pipeline(device, targets, layout, &desc))
}
