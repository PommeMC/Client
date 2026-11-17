use std::time::Instant;

use glam::DVec3;

use crate::renderer::camera::{Camera, CameraUniform};
use crate::renderer::{rhi, shader, util};
use crate::resources::{ResourceManager, resource_id};
use crate::world::border::WorldBorder;

const MAX_BORDER_VERTS: usize = 128;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldBorderVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

pub struct WorldBorderPipeline {
    start_time: Instant,
    pipeline: rhi::backend::RenderPipeline,
    pipeline_layout: rhi::backend::PipelineLayout,
    camera_layout: rhi::backend::BindGroupLayout,
    tex_layout: rhi::backend::BindGroupLayout,
    camera_sets: Vec<rhi::backend::BindGroup>,
    camera_buffers: Vec<rhi::backend::Buffer>,
    texture_set: rhi::backend::BindGroup,
    texture: rhi::backend::Texture,
    sampler: rhi::backend::Sampler,
    vertex_buffers: Vec<rhi::backend::Buffer>,
}

impl WorldBorderPipeline {
    pub fn new(
        device: &rhi::backend::Device,
        targets: rhi::backend::PipelineTargets,
        resources: &dyn ResourceManager,
    ) -> Self {
        let camera_layout = rhi::backend::create_bind_group_layout(device, util::VERTEX_UNIFORM);
        let tex_layout = rhi::backend::create_bind_group_layout(device, util::FRAGMENT_TEXTURE);
        let pipeline_layout =
            rhi::backend::create_pipeline_layout(device, &[camera_layout, tex_layout], None);

        let pipeline = create_pipeline(device, targets, pipeline_layout);

        let (camera_sets, camera_buffers) = util::create_uniform_sets(
            device,
            camera_layout,
            std::mem::size_of::<CameraUniform>() as u64,
            "border_camera",
        );

        let vertex_buffers = util::create_per_frame_host_buffers(
            device,
            (MAX_BORDER_VERTS * std::mem::size_of::<WorldBorderVertex>()) as u64,
            rhi::BufferUsages::VERTEX,
            "border_vertices",
        );

        let sampler = rhi::backend::create_sampler(device, &rhi::SamplerDesc::LINEAR_REPEAT);

        let texture = util::load_asset_texture(
            device,
            resources,
            &resource_id!("minecraft:textures/misc/forcefield.png"),
        );
        let texture_set = util::create_texture_group(device, tex_layout, texture.handle(), sampler);

        Self {
            start_time: Instant::now(),
            pipeline,
            pipeline_layout,
            camera_layout,
            tex_layout,
            camera_sets,
            camera_buffers,
            texture_set,
            texture,
            sampler,
            vertex_buffers,
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

    pub fn destroy(&mut self, device: &rhi::backend::Device) {
        util::destroy_buffers(device, &mut self.camera_buffers);
        util::destroy_buffers(device, &mut self.vertex_buffers);

        self.sampler.destroy(device);
        std::mem::take(&mut self.texture).destroy(device);

        self.pipeline.destroy(device);
        self.pipeline_layout.destroy(device);
        util::destroy_bind_groups(device, self.camera_sets.drain(..));
        self.texture_set.destroy(device);
        self.camera_layout.destroy(device);
        self.tex_layout.destroy(device);
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
        border: &WorldBorder,
        render_distance_blocks: f32,
        _min_y: i32,
        _max_y: i32,
    ) {
        let now = Instant::now();
        let time_secs = now.duration_since(self.start_time).as_secs_f32();
        let min_x = border.min_x(now);
        let max_x = border.max_x(now);
        let min_z = border.min_z(now);
        let max_z = border.max_z(now);

        let cam = *camera.position;
        let radius = render_distance_blocks.max(32.0) as f64;

        let dist_to_border = (cam.x - min_x)
            .min(max_x - cam.x)
            .min(cam.z - min_z)
            .min(max_z - cam.z);

        if cam.x < min_x - radius
            || cam.x > max_x + radius
            || cam.z < min_z - radius
            || cam.z > max_z + radius
            || dist_to_border >= radius
        {
            return;
        }

        let alpha = ((1.0 - (dist_to_border / radius).max(0.0)).powi(4).min(1.0) as f32)
            * border.color(now)[3];
        if alpha <= 0.0 {
            return;
        }

        let mut color = border.color(now);
        color[3] = alpha;

        let anchor = camera.anchor();
        let anim = time_secs.rem_euclid(3.0) / 3.0;

        let y_bot = (cam.y - radius).floor();
        let y_top = (cam.y + radius).ceil();
        let v_top = -(y_top as f32) * 0.5 + anim;
        let v_bot = -(y_bot as f32) * 0.5 + anim;

        let z_start = (cam.z - radius).floor().clamp(min_z, max_z);
        let z_end = (cam.z + radius).ceil().clamp(min_z, max_z);
        let u0_z = (z_start.floor() as i64 & 1) as f32 * 0.5 + anim;
        let u1_z = u0_z + (z_end - z_start) as f32 * 0.5;

        let x_start = (cam.x - radius).floor().clamp(min_x, max_x);
        let x_end = (cam.x + radius).ceil().clamp(min_x, max_x);
        let u0_x = (x_start.floor() as i64 & 1) as f32 * 0.5 + anim;
        let u1_x = u0_x + (x_end - x_start) as f32 * 0.5;

        let mut verts: Vec<WorldBorderVertex> = Vec::with_capacity(32);

        if (cam.x - min_x).abs() <= radius && z_start < z_end {
            push_wall_quad(
                &mut verts,
                anchor,
                DVec3::new(min_x, y_bot, z_start),
                DVec3::new(min_x, y_bot, z_end),
                DVec3::new(min_x, y_top, z_end),
                DVec3::new(min_x, y_top, z_start),
                u0_z,
                u1_z,
                v_bot,
                v_top,
                color,
            );
        }

        if (cam.x - max_x).abs() <= radius && z_start < z_end {
            push_wall_quad(
                &mut verts,
                anchor,
                DVec3::new(max_x, y_bot, z_end),
                DVec3::new(max_x, y_bot, z_start),
                DVec3::new(max_x, y_top, z_start),
                DVec3::new(max_x, y_top, z_end),
                u0_z,
                u1_z,
                v_bot,
                v_top,
                color,
            );
        }

        if (cam.z - min_z).abs() <= radius && x_start < x_end {
            push_wall_quad(
                &mut verts,
                anchor,
                DVec3::new(x_end, y_bot, min_z),
                DVec3::new(x_start, y_bot, min_z),
                DVec3::new(x_start, y_top, min_z),
                DVec3::new(x_end, y_top, min_z),
                u0_x,
                u1_x,
                v_bot,
                v_top,
                color,
            );
        }

        if (cam.z - max_z).abs() <= radius && x_start < x_end {
            push_wall_quad(
                &mut verts,
                anchor,
                DVec3::new(x_start, y_bot, max_z),
                DVec3::new(x_end, y_bot, max_z),
                DVec3::new(x_end, y_top, max_z),
                DVec3::new(x_start, y_top, max_z),
                u0_x,
                u1_x,
                v_bot,
                v_top,
                color,
            );
        }

        if verts.is_empty() {
            return;
        }

        let slice = bytemuck::cast_slice(&verts);
        self.vertex_buffers[frame].write(slice);

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffers[frame].handle());
        pass.set_bind_group(0, self.camera_sets[frame]);
        pass.set_bind_group(1, self.texture_set);
        pass.draw(verts.len() as u32, 1, 0, 0);
    }
}

fn push_wall_quad(
    verts: &mut Vec<WorldBorderVertex>,
    anchor: DVec3,
    p0: DVec3,
    p1: DVec3,
    p2: DVec3,
    p3: DVec3,
    u0: f32,
    u1: f32,
    v_bot: f32,
    v_top: f32,
    color: [f32; 4],
) {
    let v0_pos = (p0 - anchor).as_vec3().to_array();
    let v1_pos = (p1 - anchor).as_vec3().to_array();
    let v2_pos = (p2 - anchor).as_vec3().to_array();
    let v3_pos = (p3 - anchor).as_vec3().to_array();

    verts.push(WorldBorderVertex {
        position: v0_pos,
        uv: [u0, v_bot],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v1_pos,
        uv: [u1, v_bot],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v2_pos,
        uv: [u1, v_top],
        color,
    });

    verts.push(WorldBorderVertex {
        position: v0_pos,
        uv: [u0, v_bot],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v2_pos,
        uv: [u1, v_top],
        color,
    });
    verts.push(WorldBorderVertex {
        position: v3_pos,
        uv: [u0, v_top],
        color,
    });
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
            depth: Some(rhi::DepthState {
                write: false,
                compare: rhi::CompareFunction::LessEqual,
                bias: Some(rhi::DepthBias {
                    constant: -3,
                    slope: -3.0,
                }),
            }),
            blend: Some(rhi::BlendState::OVERLAY),
            cull_mode: rhi::CullMode::None,
            ..rhi::RenderPipelineDesc::new(
                "world_border",
                shader::include_shader!("world_border.vert"),
                shader::include_shader!("world_border.frag"),
                &[rhi::vertex_buffer!(
                    WorldBorderVertex,
                    [Float32x3, Float32x2, Float32x4]
                )],
            )
        },
    )
}
