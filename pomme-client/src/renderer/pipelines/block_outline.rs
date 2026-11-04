//! The targeted block's outline: vanilla `LevelRenderer.submitBlockOutline`
//! through `RenderTypes.linesTranslucent()`, each of the outline shape's
//! edges a screen-space quad (`rendertype_lines`).

use crate::physics::block_shape::outline_edges;
use crate::renderer::camera::CameraUniform;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

/// Six vertices per edge; block outlines stay far below this.
const MAX_OUTLINE_VERTS: usize = 1536;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    /// Anchor-relative.
    position: [f32; 3],
    /// The edge's direction, vanilla's `Normal`.
    normal: [f32; 3],
    /// Which side of the line the vertex is pushed to, vanilla's
    /// `gl_VertexIndex % 2`.
    side: f32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LinePush {
    color: [f32; 4],
    screen_size: [f32; 2],
    line_width: f32,
    _pad: f32,
}

/// `ARGB.black(102)`, the outline without high contrast.
const OUTLINE_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 102.0 / 255.0];

pub struct BlockOutlinePipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffers: Vec<rhi::backend::Buffer>,
}

impl BlockOutlinePipeline {
    pub fn new(device: &rhi::backend::Device, targets: rhi::backend::PipelineTargets) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let pipeline_layout = rhi::backend::create_pipeline_layout(
            device,
            &[camera_layout],
            Some(rhi::PushConstants {
                visibility: rhi::ShaderStages::VERTEX,
                size: size_of::<LinePush>() as u32,
            }),
        );
        let pipeline = create_pipeline(device, targets, pipeline_layout);
        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "block_outline_camera",
        );
        let placeholder: [LineVertex; MAX_OUTLINE_VERTS] =
            [bytemuck::Zeroable::zeroed(); MAX_OUTLINE_VERTS];
        let vertex_buffers = (0..MAX_FRAMES_IN_FLIGHT)
            .map(|_| {
                rhi::backend::Buffer::with_data(
                    device,
                    "block_outline_vertices",
                    bytemuck::cast_slice(&placeholder),
                    rhi::BufferUsages::VERTEX,
                )
            })
            .collect();
        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            camera_sets,
            camera_buffers,
            vertex_buffers,
        }
    }

    pub fn update_camera(&mut self, frame: usize, uniform: &CameraUniform) {
        self.camera_buffers[frame].write(bytemuck::bytes_of(uniform));
    }

    /// Draws `boxes`' outline at `origin` (anchor-relative),
    /// `Window.getAppropriateLineWidth` wide.
    pub fn draw(
        &mut self,
        pass: &mut rhi::backend::RenderPassEncoder,
        frame: usize,
        boxes: &[crate::physics::block_shape::LocalBox],
        origin: glam::Vec3,
        screen_size: [f32; 2],
    ) {
        let mut vertices = Vec::new();
        for (start, end) in outline_edges(boxes) {
            let start = origin + glam::DVec3::from(start).as_vec3();
            let end = origin + glam::DVec3::from(end).as_vec3();
            let normal = (end - start).normalize().to_array();
            let vertex = |p: glam::Vec3, side| LineVertex {
                position: p.to_array(),
                normal,
                side,
            };
            vertices.extend([
                vertex(start, 1.0),
                vertex(start, -1.0),
                vertex(end, 1.0),
                vertex(end, 1.0),
                vertex(start, -1.0),
                vertex(end, -1.0),
            ]);
        }
        if vertices.is_empty() {
            return;
        }
        vertices.truncate(MAX_OUTLINE_VERTS);
        self.vertex_buffers[frame].write(bytemuck::cast_slice(&vertices));

        let push = LinePush {
            color: OUTLINE_COLOR,
            screen_size,
            line_width: (screen_size[0] / 1920.0 * 2.5).max(2.5),
            _pad: 0.0,
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_push_constants(rhi::ShaderStages::VERTEX, 0, bytemuck::bytes_of(&push));
        pass.set_bind_groups(0, &[self.camera_sets[frame]]);
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
        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
    }
}

/// `RenderPipelines.LINES_TRANSLUCENT`.
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
            cull_mode: rhi::CullMode::None,
            depth: Some(rhi::DepthState::WRITE),
            blend: Some(rhi::BlendState::TRANSLUCENT),
            ..rhi::RenderPipelineDesc::new(
                "block outline",
                shader::include_shader!("block_outline.vert"),
                shader::include_shader!("block_outline.frag"),
                &[rhi::vertex_buffer!(
                    LineVertex,
                    [Float32x3, Float32x3, Float32]
                )],
            )
        },
    )
}
