use crate::renderer::camera::CameraUniform;
use crate::renderer::{MAX_FRAMES_IN_FLIGHT, rhi, shader, util};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}

const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 0.6];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 0.6];
const BLUE: [f32; 4] = [0.0, 0.5, 1.0, 0.6];
const MAX_VERTICES: usize = 4096;

pub struct ChunkBorderPipeline {
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    vertex_buffers: Vec<rhi::backend::Buffer>,
    lines: Vec<LineVertex>,
    /// Frames whose vertex buffer predates the last `update_lines`.
    stale: [bool; MAX_FRAMES_IN_FLIGHT],
}

impl ChunkBorderPipeline {
    pub fn new(device: &rhi::backend::Device, targets: rhi::backend::PipelineTargets) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let pipeline_layout = rhi::backend::create_pipeline_layout(device, &[camera_layout], None);

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            size_of::<CameraUniform>() as u64,
            "chunk_border_cam",
        );

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_VERTICES * size_of::<LineVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "chunk_border_verts",
        );

        Self {
            pipeline,
            pipeline_layout,
            camera_layout,
            camera_sets,
            camera_buffers,
            vertex_buffers,
            lines: Vec::new(),
            stale: [false; MAX_FRAMES_IN_FLIGHT],
        }
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
        self.camera_buffers[frame].write(bytemuck::bytes_of(uniform));
    }

    /// Lines are eye-relative (including the third-person offset, the origin
    /// the view matrix renders from), subtracted in f64 so they stay on the
    /// grid at extreme coordinates. The grid cell comes from `pivot` (the
    /// player eye): vanilla outlines the entity's chunk, not the camera's.
    /// Each frame uploads them when it next draws.
    pub fn update_lines(&mut self, pivot: glam::DVec3, eye: glam::DVec3, min_y: i32, max_y: i32) {
        let chunk_x = (pivot.x.floor() as i32).div_euclid(16) * 16;
        let chunk_z = (pivot.z.floor() as i32).div_euclid(16) * 16;

        let mut verts: Vec<LineVertex> = Vec::new();
        let y_min = min_y as f64;
        let y_max = max_y as f64;

        let push_line = |verts: &mut Vec<LineVertex>,
                         x0: f64,
                         y0: f64,
                         z0: f64,
                         x1: f64,
                         y1: f64,
                         z1: f64,
                         color: [f32; 4]| {
            verts.push(LineVertex {
                position: [
                    (x0 - eye.x) as f32,
                    (y0 - eye.y) as f32,
                    (z0 - eye.z) as f32,
                ],
                color,
            });
            verts.push(LineVertex {
                position: [
                    (x1 - eye.x) as f32,
                    (y1 - eye.y) as f32,
                    (z1 - eye.z) as f32,
                ],
                color,
            });
        };

        // Vertical lines at chunk corners (red)
        for dx in [0, 16] {
            for dz in [0, 16] {
                let x = (chunk_x + dx) as f64;
                let z = (chunk_z + dz) as f64;
                push_line(&mut verts, x, y_min, z, x, y_max, z, RED);
            }
        }

        // Vertical lines along chunk edges (blue) - every block along edges
        for d in 1..16 {
            let x0 = chunk_x as f64;
            let x1 = (chunk_x + 16) as f64;
            let z0 = chunk_z as f64;
            let z1 = (chunk_z + 16) as f64;
            let p = (chunk_x + d) as f64;
            let q = (chunk_z + d) as f64;
            push_line(&mut verts, p, y_min, z0, p, y_max, z0, BLUE);
            push_line(&mut verts, p, y_min, z1, p, y_max, z1, BLUE);
            push_line(&mut verts, x0, y_min, q, x0, y_max, q, BLUE);
            push_line(&mut verts, x1, y_min, q, x1, y_max, q, BLUE);
        }

        // Horizontal lines at section boundaries (yellow)
        for section in 0..=((max_y - min_y) / 16) {
            let y = (min_y + section * 16) as f64;
            let x0 = chunk_x as f64;
            let x1 = (chunk_x + 16) as f64;
            let z0 = chunk_z as f64;
            let z1 = (chunk_z + 16) as f64;
            push_line(&mut verts, x0, y, z0, x1, y, z0, YELLOW);
            push_line(&mut verts, x0, y, z1, x1, y, z1, YELLOW);
            push_line(&mut verts, x0, y, z0, x0, y, z1, YELLOW);
            push_line(&mut verts, x1, y, z0, x1, y, z1, YELLOW);
        }

        verts.truncate(MAX_VERTICES);
        self.lines = verts;
        self.stale = [true; MAX_FRAMES_IN_FLIGHT];
    }

    pub fn draw(&mut self, pass: &mut rhi::backend::RenderPassEncoder, frame: usize) {
        if self.lines.is_empty() {
            return;
        }
        if self.stale[frame] {
            self.stale[frame] = false;
            let bytes = bytemuck::cast_slice(&self.lines);
            self.vertex_buffers[frame].write(bytes);
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, self.camera_sets[frame]);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.draw(self.lines.len() as u32, 1, 0, 0);
    }

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.vertex_buffers);
        util::destroy_buffers(device, &mut self.camera_buffers);

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.camera_layout.destroy(device);
    }
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
            topology: rhi::Topology::LineList,
            depth: Some(rhi::DepthState::READ_ONLY),
            blend: Some(rhi::BlendState::OVER_REPLACE_ALPHA),
            ..rhi::RenderPipelineDesc::new(
                "chunk border",
                shader::include_shader!("chunk_border.vert"),
                shader::include_shader!("chunk_border.frag"),
                &[rhi::vertex_buffer!(LineVertex, [Float32x3, Float32x4])],
            )
        },
    )
}
